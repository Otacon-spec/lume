//! Driver Oracle, su `oracledb` (driver ufficiale Oracle, *thin mode*).
//!
//! È **sincrono**: `oracledb::connect` è una `fn`, non una `async fn`. È
//! questo che ha deciso la forma del trait `Driver` e quindi tutto il worker
//! a thread — vedi `driver.rs`.
//!
//! ATTENZIONE: Limite noto della beta 26.0.0-beta.2: `Lob` non espone alcuna API di
//! lettura pubblica (solo `get_size`, `open`, `close`, `trim`), quindi CLOB e
//! BLOB si mostrano come segnaposto con la dimensione invece del contenuto.
//! Meglio un segnaposto onesto di una cella vuota che sembra un NULL.

use oracledb::{Config, Connection, Cursor, Metadata, Row};

use crate::driver::{
    ColumnInfo, ConnectionProfile, Driver, Engine, QueryBatch, QueryFlow, QuerySink, Secret,
    SqlError, Value, accounted_row_bytes,
};

/// Codici ORA che vogliono dire «il segreto che hai non vale più».
///
/// Ci sono solo questi tre perché il criterio è stretto: dopo un `forget` il
/// segreto va ridigitato, quindi ci finisce solo ciò che una ridigitazione
/// può davvero risolvere. Restano fuori **ORA-28000** (account bloccato: la
/// password può essere giusta) e **ORA-01031** (privilegi insufficienti:
/// l'autenticazione è riuscita).
/// Schema di catalogo Oracle: fuori dall'albero, altrimenti `ALL_TABLES` sommergerebbe
/// migliaia di viste di sistema.
const SCHEMA_CATALOGO: [&str; 15] = [
    "SYS",
    "SYSTEM",
    "XDB",
    "CTXSYS",
    "MDSYS",
    "ORDSYS",
    "WMSYS",
    "OLAPSYS",
    "EXFSYS",
    "DBSNMP",
    "APPQOSSYS",
    "GSMADMIN_INTERNAL",
    "AUDSYS",
    "OJVMSYS",
    "OUTLN",
];

const CREDENZIALI_RIFIUTATE: [&str; 3] = [
    // credenziale non valida, logon negato
    "ORA-01017",
    // password nulla
    "ORA-01005",
    // password scaduta: quella salvata non serve più
    "ORA-28001",
];

/// `oracledb` non espone il numero ORA: un errore del server arriva come
/// `ErrorKind::DbError` con il messaggio grezzo, che comincia per «ORA-…».
/// Cercare il codice nel testo è l'unico appiglio che la beta offre, ed è
/// per questo che sta in una funzione a sé, verificabile senza un server.
fn credenziali_rifiutate(kind: &oracledb::ErrorKind) -> bool {
    match kind {
        oracledb::ErrorKind::DbError(message) => CREDENZIALI_RIFIUTATE
            .iter()
            .any(|code| message.contains(code)),
        // Il driver si accorge da solo di non avere credenziali da mandare.
        oracledb::ErrorKind::NoCredentials => true,
        _ => false,
    }
}

fn connect_error(error: oracledb::Error) -> SqlError {
    if credenziali_rifiutate(error.kind()) {
        SqlError::Auth(error.to_string())
    } else {
        SqlError::Connect(error.to_string())
    }
}

/// Suggerimento aggiunto a un errore di connessione **cifrata**: il caso più
/// comune è un listener Oracle rimasto in chiaro (o con una CA privata che le
/// radici incorporate non conoscono). Non si scala mai a testo in chiaro da soli.
pub(crate) const SUGGERIMENTO_TLS: &str = "TLS attivo di default: se il listener Oracle è in chiaro, scrivere tcp://host:porta nel campo host";

/// Dove connettersi, già validato, e se lo si fa senza cifratura.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Destinazione {
    /// Sempre nella forma `schema://host[:porta]/servizio`: una stringa che
    /// `oracledb` legge come EZConnect e mai come alias TNS o descrittore.
    pub stringa: String,
    pub in_chiaro: bool,
}

fn host_non_valido() -> SqlError {
    SqlError::Connect(
        "host Oracle non valido: usare nome[:porta] oppure [ipv6][:porta], con tcp:// solo per il chiaro"
            .to_owned(),
    )
}

fn porta_non_valida() -> SqlError {
    SqlError::Connect("porta Oracle non valida: deve essere un numero da 1 a 65535".to_owned())
}

fn servizio_non_valido() -> SqlError {
    SqlError::Connect(
        "servizio Oracle non valido: usare solo lettere, cifre, «_», «-» e «.»".to_owned(),
    )
}

/// `:porta` con 1-5 cifre e valore 1..=65535. L'input non si ripete negli
/// errori: è testo dell'utente e potrebbe contenere una password.
fn porta_valida(porta: &str) -> Result<u16, SqlError> {
    if porta.is_empty() || porta.len() > 5 || !porta.bytes().all(|b| b.is_ascii_digit()) {
        return Err(porta_non_valida());
    }
    match porta.parse::<u16>() {
        Ok(0) | Err(_) => Err(porta_non_valida()),
        Ok(valore) => Ok(valore),
    }
}

/// Separa `host[:porta]` o `[ipv6][:porta]`. Restituisce l'host già nella
/// forma da scrivere nella stringa (con le parentesi per l'IPv6) e la porta.
fn host_e_porta(resto: &str) -> Result<(String, Option<u16>), SqlError> {
    if let Some(dopo) = resto.strip_prefix('[') {
        let (indirizzo, coda) = dopo.split_once(']').ok_or_else(host_non_valido)?;
        let esadecimale = !indirizzo.is_empty()
            && indirizzo.contains(':')
            && indirizzo
                .bytes()
                .all(|b| b.is_ascii_hexdigit() || b == b':');
        if !esadecimale {
            return Err(host_non_valido());
        }
        let porta = match coda {
            "" => None,
            _ => Some(porta_valida(
                coda.strip_prefix(':').ok_or_else(host_non_valido)?,
            )?),
        };
        return Ok((format!("[{indirizzo}]"), porta));
    }
    let mut parti = resto.split(':');
    let nome = parti.next().unwrap_or_default();
    let porta = parti.next();
    if parti.next().is_some() {
        // Più di un `:` fuori dalle parentesi: né un host né un IPv6 valido.
        return Err(host_non_valido());
    }
    let nome_ok = !nome.is_empty()
        && nome
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_');
    if !nome_ok {
        return Err(host_non_valido());
    }
    let porta = porta.map(porta_valida).transpose()?;
    Ok((nome.to_owned(), porta))
}

/// Trasforma i due campi del profilo (`host`, `database`) nella stringa di
/// connessione da dare a `oracledb`, oppure rifiuta.
pub(crate) fn destinazione(host: &str, database: &str) -> Result<Destinazione, SqlError> {
    let host = host.trim();
    let (schema, resto) = match host.split_once("://") {
        Some((schema, resto)) => (schema.to_ascii_lowercase(), resto),
        None => ("tcps".to_owned(), host),
    };
    let in_chiaro = match schema.as_str() {
        "tcps" => false,
        "tcp" => true,
        _ => {
            return Err(SqlError::Connect(
                "schema non supportato: usare tcps:// (predefinito) o tcp://".to_owned(),
            ));
        }
    };
    let (nome, porta) = host_e_porta(resto)?;
    let servizio = database.trim();
    let servizio_ok = !servizio.is_empty()
        && servizio
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.');
    if !servizio_ok {
        return Err(servizio_non_valido());
    }
    let porta = porta.map(|p| format!(":{p}")).unwrap_or_default();
    Ok(Destinazione {
        stringa: format!("{schema}://{nome}{porta}/{servizio}"),
        in_chiaro,
    })
}

/// Aggiunge il suggerimento TLS ai soli errori di **connessione** su un canale
/// cifrato. Un errore di credenziali (`Auth`) non c'entra con TLS.
fn con_suggerimento(errore: SqlError, in_chiaro: bool) -> SqlError {
    match errore {
        SqlError::Connect(messaggio) if !in_chiaro => {
            SqlError::Connect(format!("{messaggio}. {SUGGERIMENTO_TLS}"))
        }
        altro => altro,
    }
}

/// Vero se questo profilo si connetterebbe senza cifratura: solo Oracle, solo
/// con `tcp://` scritto dall'utente. L'albero SQL lo mostra in rosso.
#[must_use]
pub fn is_cleartext(profile: &ConnectionProfile) -> bool {
    matches!(profile.engine, Engine::Oracle)
        && destinazione(&profile.host, &profile.database).is_ok_and(|d| d.in_chiaro)
}

pub struct OracleDriver {
    connection: Connection,
}

impl OracleDriver {
    pub fn connect(profile: &ConnectionProfile, secret: &Secret) -> Result<Self, SqlError> {
        let crate::driver::AuthKind::SqlLogin { user } = &profile.auth else {
            // Il service principal è di Entra, cioè di Azure SQL. Su Oracle
            // non esiste, e fingere una connessione sarebbe peggio.
            return Err(SqlError::Auth(
                "Oracle accetta solo utente e password.".to_owned(),
            ));
        };
        // `destinazione` valida host e servizio e compone sempre
        // `schema://host[:porta]/servizio`: TLS di default, chiaro solo se
        // l'utente ha scritto `tcp://`. Vedi `SUGGERIMENTO_TLS`.
        let destinazione = destinazione(&profile.host, &profile.database)?;
        let config = Config::default()
            .set_credentials(user, secret.expose())
            .set_connect_string(&destinazione.stringa)
            .map_err(|error| SqlError::Connect(error.to_string()))?;
        let connection = oracledb::connect(config)
            .map_err(|error| con_suggerimento(connect_error(error), destinazione.in_chiaro))?;
        Ok(Self { connection })
    }

    fn run(&mut self, sql: &str, params: &[&dyn oracledb::ToDbValue]) -> Result<Cursor, SqlError> {
        self.connection
            .query(sql, params)
            .map_err(|error| SqlError::Query(error.to_string()))
    }
}

/// Spezza `OWNER.TABLE` come su Azure SQL (`schema.table`). Senza punto resta
/// solo il nome tabella: l'owner è quello della sessione connessa.
fn parse_qualified_table(table: &str) -> (Option<String>, String) {
    let table = table.trim();
    let Some((owner, name)) = table.rsplit_once('.') else {
        return (None, table.to_uppercase());
    };
    (
        Some(owner.trim().to_uppercase()),
        name.trim().to_uppercase(),
    )
}

fn schema_catalogo_denylist() -> String {
    SCHEMA_CATALOGO
        .iter()
        .map(|schema| format!("'{schema}'"))
        .collect::<Vec<_>>()
        .join(", ")
}

impl Driver for OracleDriver {
    fn tables(&mut self) -> Result<Vec<String>, SqlError> {
        let denylist = schema_catalogo_denylist();
        let sql = format!(
            "SELECT owner || '.' || table_name \
             FROM all_tables \
             WHERE owner NOT IN ({denylist}) \
             ORDER BY owner, table_name"
        );
        let cursor = self.run(&sql, &[])?;
        let mut tables = Vec::new();
        for row in cursor {
            let row = row.map_err(|error| SqlError::Query(error.to_string()))?;
            let name: String = row
                .get(0)
                .map_err(|error| SqlError::Query(error.to_string()))?;
            tables.push(name);
        }
        Ok(tables)
    }

    fn columns(&mut self, table: &str) -> Result<Vec<ColumnInfo>, SqlError> {
        // Bind e non interpolazione: il nome tabella arriva dall'albero, ma
        // un profilo può puntare a un database di cui non ci si fida.
        let (owner, table_name) = parse_qualified_table(table);
        let cursor = if let Some(owner) = owner.as_deref() {
            self.run(
                "SELECT column_name, data_type FROM all_tab_columns \
                 WHERE owner = :1 AND table_name = :2 ORDER BY column_id",
                &[&owner, &table_name.as_str()],
            )?
        } else {
            self.run(
                "SELECT column_name, data_type FROM user_tab_columns \
                 WHERE table_name = :1 ORDER BY column_id",
                &[&table_name.as_str()],
            )?
        };
        let mut columns = Vec::new();
        for row in cursor {
            let row = row.map_err(|error| SqlError::Query(error.to_string()))?;
            let name: String = row
                .get(0)
                .map_err(|error| SqlError::Query(error.to_string()))?;
            let type_name: String = row
                .get(1)
                .map_err(|error| SqlError::Query(error.to_string()))?;
            columns.push(ColumnInfo { name, type_name });
        }
        Ok(columns)
    }

    fn query_stream(
        &mut self,
        sql: &str,
        sink: &mut dyn QuerySink,
    ) -> Result<std::time::Duration, SqlError> {
        let sql = strip_terminator(sql);
        let started = std::time::Instant::now();
        let cursor = self.run(sql, &[])?;
        let columns: Vec<ColumnInfo> = cursor
            .columns()
            .iter()
            .map(|meta| ColumnInfo {
                name: meta.name().to_owned(),
                type_name: display_type(meta),
            })
            .collect();
        let shape: Vec<&'static oracledb::DbType> =
            cursor.columns().iter().map(Metadata::db_type).collect();
        if sink.started(columns)? == QueryFlow::Stop {
            return Ok(started.elapsed());
        }

        let mut pending = QueryBatch::new(Vec::new());
        for row in cursor {
            let row = row.map_err(|error| SqlError::Query(error.to_string()))?;
            let mut cells = Vec::with_capacity(shape.len());
            for (index, db_type) in shape.iter().enumerate() {
                cells.push(cell(&row, index, db_type));
            }
            if !pending.can_accept(&cells) && !pending.rows.is_empty() {
                if sink.batch(pending)? == QueryFlow::Stop {
                    return Ok(started.elapsed());
                }
                pending = QueryBatch::new(Vec::new());
            }
            pending.accounted_bytes = pending
                .accounted_bytes
                .saturating_add(accounted_row_bytes(&cells));
            pending.rows.push(cells);
        }
        if !pending.rows.is_empty() && sink.batch(pending)? == QueryFlow::Stop {
            return Ok(started.elapsed());
        }
        Ok(started.elapsed())
    }
}

/// Oracle **rifiuta** il punto e virgola finale, che invece SQL Server accetta
/// e che `Engine::select_top` mette per uniformità. Toglierlo qui evita di
/// far pagare la differenza a chi scrive la query.
fn strip_terminator(sql: &str) -> &str {
    sql.trim().trim_end_matches(';').trim_end()
}

/// `DbType::name()` restituisce «DB_TYPE_NUMBER»; nella griglia serve
/// «NUMBER». Il nome interno `ora_name` è `pub(crate)`, quindi si toglie il
/// prefisso.
fn display_type(meta: &Metadata) -> String {
    meta.db_type()
        .name()
        .strip_prefix("DB_TYPE_")
        .unwrap_or_else(|| meta.db_type().name())
        .to_owned()
}

/// Una cella. `DbValue` non è nominabile da fuori (il modulo è privato),
/// quindi non si può implementare `FromDbValue` e il tipo di destinazione va
/// scelto qui, dal metadato della colonna.
fn cell(row: &Row, index: usize, db_type: &oracledb::DbType) -> Value {
    let name = db_type.name();
    // I LOB vanno intercettati **prima** dei predicati: `is_string_type()` è
    // vero anche per CLOB, ma il valore arriva come `Lob` e la conversione a
    // `String` fallirebbe.
    if matches!(name, "DB_TYPE_CLOB" | "DB_TYPE_NCLOB" | "DB_TYPE_BLOB") {
        return match row.get::<Option<oracledb::Lob>>(index) {
            Ok(Some(mut lob)) => Value::Text(match lob.get_size() {
                Ok(size) => format!("<{} · {size} byte>", display_short(name)),
                Err(_) => format!("<{}>", display_short(name)),
            }),
            Ok(None) => Value::Null,
            Err(error) => Value::Text(format!("<{error}>")),
        };
    }
    match name {
        "DB_TYPE_BOOLEAN" => option(row.get::<Option<bool>>(index), Value::Bool),
        "DB_TYPE_BINARY_DOUBLE" => option(row.get::<Option<f64>>(index), Value::Float),
        "DB_TYPE_BINARY_FLOAT" => option(row.get::<Option<f32>>(index), |v| Value::Float(v.into())),
        "DB_TYPE_NUMBER" | "DB_TYPE_BINARY_INTEGER" => {
            option(row.get::<Option<oracledb::OracleNumber>>(index), |number| {
                let text = number.to_string();
                // NUMBER è decimale a precisione arbitraria: un intero resta
                // intero (e ordina come tale), il resto diventa f64 e, se non
                // ci sta, si mostra il testo esatto invece di arrotondarlo.
                match text.parse::<i64>() {
                    Ok(value) => Value::Int(value),
                    Err(_) => match text.parse::<f64>() {
                        Ok(value) => Value::Float(value),
                        Err(_) => Value::Text(text),
                    },
                }
            })
        }
        _ if db_type.is_binary_type() => option(row.get::<Option<Vec<u8>>>(index), Value::Bytes),
        _ if db_type.is_date_type() => option(
            row.get::<Option<oracledb::OracleTimestamp>>(index),
            |value| Value::Text(value.to_string()),
        ),
        "DB_TYPE_INTERVAL_DS" => option(
            row.get::<Option<oracledb::OracleIntervalDS>>(index),
            |value| Value::Text(value.to_string()),
        ),
        "DB_TYPE_INTERVAL_YM" => option(
            row.get::<Option<oracledb::OracleIntervalYM>>(index),
            |value| Value::Text(value.to_string()),
        ),
        _ if db_type.is_string_type() => option(row.get::<Option<String>>(index), Value::Text),
        // Tipo non mappato (JSON, VECTOR, BFILE, cursori annidati): il nome
        // del tipo dice cosa c'è, meglio di una cella che sembra vuota.
        _ => Value::Text(format!("<{}>", display_short(name))),
    }
}

fn display_short(name: &str) -> &str {
    name.strip_prefix("DB_TYPE_").unwrap_or(name)
}

/// Un errore di conversione **non** deve far fallire l'intera griglia: si
/// mostra nella cella e le altre righe restano leggibili.
fn option<T>(value: Result<Option<T>, oracledb::Error>, map: impl FnOnce(T) -> Value) -> Value {
    match value {
        Ok(Some(value)) => map(value),
        Ok(None) => Value::Null,
        Err(error) => Value::Text(format!("<{error}>")),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn il_nome_qualificato_si_spezza_in_owner_e_tabella() {
        assert_eq!(
            parse_qualified_table("HR.EMPLOYEES"),
            (Some("HR".into()), "EMPLOYEES".into())
        );
        assert_eq!(
            parse_qualified_table("  apps.orders  "),
            (Some("APPS".into()), "ORDERS".into())
        );
        assert_eq!(
            parse_qualified_table("EMPLOYEES"),
            (None, "EMPLOYEES".into())
        );
    }

    #[test]
    fn il_punto_e_virgola_finale_sparisce_perche_oracle_lo_rifiuta() {
        assert_eq!(
            strip_terminator("SELECT *\nFROM USERS\nFETCH FIRST 500 ROWS ONLY;"),
            "SELECT *\nFROM USERS\nFETCH FIRST 500 ROWS ONLY"
        );
        assert_eq!(strip_terminator("  SELECT 1 ;  "), "SELECT 1");
        assert_eq!(
            strip_terminator("SELECT ';' FROM dual"),
            "SELECT ';' FROM dual",
            "un punto e virgola dentro una stringa non e' un terminatore"
        );
    }

    #[test]
    fn il_nome_del_tipo_perde_il_prefisso_interno() {
        assert_eq!(display_short("DB_TYPE_NUMBER"), "NUMBER");
        assert_eq!(display_short("CLOB"), "CLOB");
    }

    /// La classificazione decide se un segreto viene buttato via dalla
    /// cassaforte: se sbaglia, o si resta bloccati con una password vecchia o
    /// si ridigita una password giusta a ogni tentativo.
    #[test]
    fn solo_i_codici_ora_che_una_ridigitazione_risolve_contano_come_auth() {
        let db = |message: &str| oracledb::ErrorKind::DbError(message.to_owned());

        for message in [
            "ORA-01017: invalid credential or not authorized; logon denied",
            "ORA-01005: null password given; logon denied",
            "ORA-28001: the password has expired",
        ] {
            assert!(
                credenziali_rifiutate(&db(message)),
                "doveva contare come autenticazione: {message}"
            );
        }
        assert!(credenziali_rifiutate(&oracledb::ErrorKind::NoCredentials));

        for message in [
            // la password può essere giusta: sbloccare l'account è altro
            "ORA-28000: the account is locked",
            // autenticato, ma senza i diritti: ridigitare non cambia niente
            "ORA-01031: insufficient privileges",
            "ORA-12541: TNS:no listener",
        ] {
            assert!(
                !credenziali_rifiutate(&db(message)),
                "non doveva contare come autenticazione: {message}"
            );
        }
        assert!(!credenziali_rifiutate(&oracledb::ErrorKind::DeadConnection));
    }

    #[test]
    fn un_profilo_con_service_principal_non_e_valido_per_oracle() {
        let profile = ConnectionProfile {
            id: "o".into(),
            label: "oracle · prod".into(),
            engine: crate::driver::Engine::Oracle,
            host: "db.example.com:1521".into(),
            database: "ORCLPDB1".into(),
            auth: crate::driver::AuthKind::ServicePrincipal {
                tenant_id: "t".into(),
                client_id: "c".into(),
            },
        };
        let error = OracleDriver::connect(&profile, &Secret::new("s".into()))
            .err()
            .expect("il service principal e' di Entra, non di Oracle");
        assert!(matches!(error, SqlError::Auth(_)));
    }

    fn profilo_oracle(host: &str, database: &str) -> ConnectionProfile {
        ConnectionProfile {
            id: "o".into(),
            label: "oracle · test".into(),
            engine: crate::driver::Engine::Oracle,
            host: host.into(),
            database: database.into(),
            auth: crate::driver::AuthKind::SqlLogin {
                user: "lume".into(),
            },
        }
    }

    /// Senza schema si cifra; il chiaro esiste solo se l'utente scrive `tcp://`
    /// e lo scrive lui. Le maiuscole non devono degradare a testo in chiaro
    /// né, al contrario, far passare un `TCP://` per cifrato.
    #[test]
    fn il_chiaro_solo_se_esplicito() {
        let d = destinazione("db.example.com", "ORCLPDB1").unwrap();
        assert_eq!(d.stringa, "tcps://db.example.com/ORCLPDB1");
        assert!(!d.in_chiaro);

        let d = destinazione("  db.example.com:1522  ", "ORCLPDB1").unwrap();
        assert_eq!(d.stringa, "tcps://db.example.com:1522/ORCLPDB1");
        assert!(!d.in_chiaro);

        let d = destinazione("TCPS://db.example.com", "ORCLPDB1").unwrap();
        assert_eq!(d.stringa, "tcps://db.example.com/ORCLPDB1");
        assert!(!d.in_chiaro);

        let d = destinazione("tcp://db.example.com:1521", "ORCLPDB1").unwrap();
        assert_eq!(d.stringa, "tcp://db.example.com:1521/ORCLPDB1");
        assert!(d.in_chiaro);

        let d = destinazione("TCP://db.example.com", "ORCLPDB1").unwrap();
        assert_eq!(d.stringa, "tcp://db.example.com/ORCLPDB1");
        assert!(d.in_chiaro);

        let d = destinazione("[::1]:1522", "ORCLPDB1").unwrap();
        assert_eq!(d.stringa, "tcps://[::1]:1522/ORCLPDB1");
        assert!(!d.in_chiaro);
    }

    /// Ogni voce è una stringa che `oracledb` leggerebbe in modo diverso da
    /// «un host»: lista di host, descrittore completo, alias, credenziali,
    /// query, schema sconosciuto, porta fuori scala.
    #[test]
    fn input_ostili_dellhost() {
        let ostili = [
            "",
            "   ",
            "db.example.com/altro",
            "db.example.com?x=1",
            "user:pw@db.example.com",
            "db1,db2",
            "db1;db2",
            "(DESCRIPTION=(ADDRESS=(PROTOCOL=tcp)(HOST=x)(PORT=1)))",
            "db example.com",
            "db.example.com\nx",
            "db.example.com\0",
            "http://db.example.com",
            "ftp://db.example.com",
            "tcp:/db.example.com",
            "://db.example.com",
            "tcps://",
            "db.example.com:99999",
            "db.example.com:0",
            "db.example.com:",
            "db.example.com:12a",
            "db.example.com:-1",
            "db.example.com:1521:1",
            "[::1",
            "[]",
            "[zz::1]:1521",
            "[::1]x",
            "[::1]:",
            "perché.example.com",
            "db\\example",
        ];
        for host in ostili {
            assert!(
                destinazione(host, "ORCLPDB1").is_err(),
                "doveva essere rifiutato: {host:?}"
            );
        }
    }

    /// Un errore di validazione non deve ripetere il campo: se l'utente ha
    /// incollato «utente:password@host» nel campo host, la password finirebbe
    /// nella riga d'errore dell'albero e nei log.
    #[test]
    fn gli_errori_di_validazione_non_ripetono_linput() {
        let errore = destinazione("lume:Sup3rSegreta@db.example.com", "ORCLPDB1").unwrap_err();
        let testo = errore.to_string();
        assert!(!testo.contains("Sup3rSegreta"), "{testo}");
        assert!(!testo.contains("lume:"), "{testo}");
        let errore = destinazione("db.example.com", "svc;DROP").unwrap_err();
        assert!(!errore.to_string().contains("DROP"));
    }

    #[test]
    fn input_ostili_del_servizio() {
        for servizio in [
            "", "  ", "A/B", "A?x", "A B", "(x)", "A:1", "A,B", "A;B", "A\nB",
        ] {
            assert!(
                destinazione("db.example.com", servizio).is_err(),
                "doveva essere rifiutato: {servizio:?}"
            );
        }
        assert!(destinazione("db.example.com", "ORCLPDB1.example.com").is_ok());
        assert!(destinazione("db.example.com", "free_pdb-1").is_ok());
    }

    /// La stringa che produciamo deve essere davvero EZConnect per
    /// `oracledb`: se non lo fosse, il parser cercherebbe un alias TNS in
    /// `config_dir`, cioè leggerebbe un file che non abbiamo scelto noi.
    #[test]
    fn la_stringa_prodotta_e_accettata_da_oracledb() {
        for (host, servizio) in [
            ("db.example.com", "ORCLPDB1"),
            ("db.example.com:1522", "ORCLPDB1"),
            ("tcp://10.0.0.5:1522", "XEPDB1"),
            ("[::1]:1521", "XEPDB1"),
            ("TCPS://[fe80::1]", "XEPDB1"),
        ] {
            let d = destinazione(host, servizio).unwrap();
            Config::default()
                .set_connect_string(&d.stringa)
                .unwrap_or_else(|e| panic!("{} non accettata: {e}", d.stringa));
        }
    }

    #[test]
    fn il_suggerimento_tls_solo_sugli_errori_di_connessione_cifrata() {
        // Connessione cifrata fallita: il suggerimento serve.
        let SqlError::Connect(testo) =
            con_suggerimento(SqlError::Connect("connessione rifiutata".into()), false)
        else {
            panic!("doveva restare un errore di connessione");
        };
        assert!(testo.starts_with("connessione rifiutata"));
        assert!(testo.contains(SUGGERIMENTO_TLS));

        // Già in chiaro: il suggerimento sarebbe assurdo.
        assert!(matches!(
            con_suggerimento(SqlError::Connect("x".into()), true),
            SqlError::Connect(t) if t == "x"
        ));
        // Un errore di credenziali non c'entra con TLS.
        assert!(matches!(
            con_suggerimento(SqlError::Auth("ORA-01017".into()), false),
            SqlError::Auth(t) if t == "ORA-01017"
        ));
    }

    #[test]
    fn is_cleartext_vale_solo_per_oracle_con_tcp_esplicito() {
        assert!(is_cleartext(&profilo_oracle("tcp://db.example.com", "S")));
        assert!(is_cleartext(&profilo_oracle(
            "TCP://db.example.com:1521",
            "S"
        )));
        assert!(!is_cleartext(&profilo_oracle("db.example.com", "S")));
        assert!(!is_cleartext(&profilo_oracle("tcps://db.example.com", "S")));
        // Un host non valido non è «in chiaro»: non si connette affatto.
        assert!(!is_cleartext(&profilo_oracle("tcp://a,b", "S")));
        // Un profilo Azure con la stessa stringa non è affare di Oracle.
        let mut azure = profilo_oracle("tcp://x.database.windows.net", "prod");
        azure.engine = crate::driver::Engine::AzureSql;
        assert!(!is_cleartext(&azure));
    }
}
