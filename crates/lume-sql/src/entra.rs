//! Token Entra ID per il **service principal** di Azure SQL.
//!
//! `tiberius-ng` espone `AuthMethod::aad_token`, ma *accetta* il token: non lo
//! *ottiene*. Il flusso client-credentials è una sola POST, quindi qui non
//! entra `azure_identity` e con lui l'albero dell'SDK Azure — il budget del
//! binario è già la cosa più stretta che abbiamo.
//!
//! **Nessuna cache.** Il token serve solo all'handshake: una connessione lo
//! chiede una volta e poi vive finché l'ibernazione non la chiude. Una cache
//! dovrebbe stare sopra le connessioni, cioè in `lume-app`, e varrebbe una
//! POST da ~100 ms alla riapertura. Non vale la complessità.

use std::time::Duration;

use zeroize::Zeroizing;

use crate::driver::{Secret, SqlError};

/// Il pubblico del token: è il servizio Azure SQL, non Graph.
const SCOPE: &str = "https://database.windows.net/.default";

/// Dove si chiede il token. Nei test è un server locale vero.
const AUTORITA: &str = "https://login.microsoftonline.com";

/// Tetti di una richiesta di token. Il worker SQL vive in un thread proprio e
/// l'ibernazione lo aspetta: una POST che non torna terrebbe vivo il thread
/// (e il suo runtime) oltre la chiusura della console.
#[derive(Clone, Copy)]
struct Limiti {
    /// Solo la connessione TCP/TLS.
    connessione: Duration,
    /// Dalla partenza alla fine del corpo, lettura compresa.
    totale: Duration,
    /// Byte massimi del corpo: un token pesa qualche KiB.
    corpo: usize,
}

const LIMITI: Limiti = Limiti {
    connessione: Duration::from_secs(10),
    totale: Duration::from_secs(20),
    corpo: 64 * 1024,
};

/// Oltre questo si taglia la descrizione d'errore del server.
const MAX_DESCRIZIONE: usize = 200;

/// Chiede un access token con le credenziali del service principal.
///
/// Attenzione: va chiamata **dentro** il runtime tokio del worker, con il client
/// asincrono: `reqwest::blocking` dentro un runtime va in stallo.
pub async fn access_token(
    tenant_id: &str,
    client_id: &str,
    client_secret: &Secret,
) -> Result<Secret, SqlError> {
    access_token_da(AUTORITA, tenant_id, client_id, client_secret, LIMITI).await
}

async fn access_token_da(
    autorita: &str,
    tenant_id: &str,
    client_id: &str,
    client_secret: &Secret,
    limiti: Limiti,
) -> Result<Secret, SqlError> {
    if tenant_id.is_empty() || client_id.is_empty() {
        return Err(SqlError::Auth(
            "Service principal incompleto: mancano tenant o client id.".to_owned(),
        ));
    }
    let url = url_del_token(autorita, tenant_id)?;
    // `reqwest` è compilato senza provider rustls (`…-no-provider`) e `build()`
    // va in panic se nessuno lo ha installato, anche per un URL `http://`.
    crate::install_crypto_provider();
    // Nessun redirect: un 307/308 ripeterebbe la POST, con il client secret
    // nel corpo, verso la destinazione scelta dal server.
    let client = reqwest::Client::builder()
        .connect_timeout(limiti.connessione)
        .timeout(limiti.totale)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| SqlError::Auth(error.to_string()))?;
    let risposta = client
        .post(url)
        .form(&[
            ("grant_type", "client_credentials"),
            ("client_id", client_id),
            ("client_secret", client_secret.expose()),
            ("scope", SCOPE),
        ])
        .send()
        .await
        .map_err(|error| errore_di_rete(&error, limiti))?;

    let status = risposta.status();
    let corpo = leggi_corpo(risposta, limiti).await?;
    if !status.is_success() {
        // Il corpo d'errore di Entra contiene `error_description`, che dice
        // cosa manca davvero; il codice HTTP da solo no.
        return Err(SqlError::Auth(describe_failure(
            status.as_u16(),
            &corpo,
            client_secret.expose(),
        )));
    }
    parse_token(&corpo)
}

/// Compone l'URL dell'endpoint. Il tenant entra nel **percorso**: se contiene
/// `/`, `?`, `#`, `@` o `\` la POST cambia destinazione.
fn url_del_token(autorita: &str, tenant_id: &str) -> Result<String, SqlError> {
    if !tenant_valido(tenant_id) {
        return Err(SqlError::Auth(
            "Tenant non valido: sono ammessi solo lettere, cifre, «-» e «.» (un GUID o un dominio)."
                .to_owned(),
        ));
    }
    Ok(format!("{autorita}/{tenant_id}/oauth2/v2.0/token"))
}

/// Un tenant è un GUID o un nome di dominio: ASCII alfanumerico, `-` e `.`,
/// senza punti iniziali, finali o doppi. Meno di così rifiuterebbe tenant
/// veri; di più lascia passare caratteri con un significato nell'URL.
fn tenant_valido(tenant: &str) -> bool {
    !tenant.is_empty()
        && tenant.len() <= 253
        && tenant
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'.')
        && !tenant.starts_with('.')
        && !tenant.ends_with('.')
        && !tenant.contains("..")
}

fn errore_di_rete(error: &reqwest::Error, limiti: Limiti) -> SqlError {
    if error.is_timeout() {
        return SqlError::Auth(format!(
            "Entra ID: nessuna risposta entro {:?}.",
            limiti.totale
        ));
    }
    SqlError::Auth(error.to_string())
}

/// Legge il corpo a pezzi e si ferma al tetto: un server che non smette mai
/// di scrivere non deve riempire la RAM. Il buffer si azzera a `Drop`
/// perché la risposta di successo contiene il token.
async fn leggi_corpo(
    mut risposta: reqwest::Response,
    limiti: Limiti,
) -> Result<Zeroizing<Vec<u8>>, SqlError> {
    let mut corpo = Zeroizing::new(Vec::new());
    while let Some(pezzo) = risposta
        .chunk()
        .await
        .map_err(|error| errore_di_rete(&error, limiti))?
    {
        if corpo.len().saturating_add(pezzo.len()) > limiti.corpo {
            return Err(SqlError::Auth(format!(
                "Entra ID: risposta oltre {} byte, interrotta.",
                limiti.corpo
            )));
        }
        corpo.extend_from_slice(&pezzo);
    }
    Ok(corpo)
}

/// Estrae l'access token **spostandolo** fuori dal JSON (`Value::take`), così
/// nel valore intermedio non ne resta una copia.
fn parse_token(corpo: &[u8]) -> Result<Secret, SqlError> {
    let mut parsed: serde_json::Value =
        serde_json::from_slice(corpo).map_err(|error| SqlError::Auth(error.to_string()))?;
    match parsed.get_mut("access_token").map(serde_json::Value::take) {
        Some(serde_json::Value::String(token)) if !token.is_empty() => Ok(Secret::new(token)),
        _ => Err(SqlError::Auth("Risposta senza access_token.".to_owned())),
    }
}

/// Il messaggio d'errore per l'utente. Il testo del server è **non fidato**:
/// si toglie il client secret se lo ripete, si tolgono i caratteri di
/// controllo e si taglia a [`MAX_DESCRIZIONE`] caratteri.
fn describe_failure(status: u16, corpo: &[u8], segreto: &str) -> String {
    let description = serde_json::from_slice::<serde_json::Value>(corpo)
        .ok()
        .and_then(|parsed| {
            parsed
                .get("error_description")
                .and_then(serde_json::Value::as_str)
                .map(|text| text.lines().next().unwrap_or(text).to_owned())
        });
    match description {
        Some(description) => {
            let pulita = if segreto.is_empty() {
                description
            } else {
                description.replace(segreto, "«redatto»")
            };
            let pulita: String = pulita
                .chars()
                .filter(|carattere| !carattere.is_control())
                .take(MAX_DESCRIZIONE)
                .collect();
            format!("Entra ID {status}: {pulita}")
        }
        None => format!("Entra ID {status}"),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    const PROVA: Limiti = Limiti {
        connessione: Duration::from_secs(1),
        totale: Duration::from_secs(1),
        corpo: 4096,
    };
    const SEGRETO: &str = "SEGRETO-DI-PROVA-123";

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    /// Legge la richiesta HTTP intera (testa e corpo) da un client vero.
    fn leggi_richiesta(flusso: &mut TcpStream) -> Vec<u8> {
        flusso
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut dati = Vec::new();
        let mut pezzo = [0u8; 1024];
        loop {
            let letti = flusso.read(&mut pezzo).unwrap_or(0);
            if letti == 0 {
                return dati;
            }
            dati.extend_from_slice(&pezzo[..letti]);
            let Some(fine) = dati.windows(4).position(|finestra| finestra == b"\r\n\r\n") else {
                continue;
            };
            let testa = String::from_utf8_lossy(&dati[..fine]).to_lowercase();
            let attesi = testa
                .lines()
                .find_map(|riga| riga.strip_prefix("content-length:"))
                .and_then(|valore| valore.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if dati.len() >= fine + 4 + attesi {
                return dati;
            }
        }
    }

    /// Un server HTTP vero, che risponde una volta con `risposta` e chiude.
    fn serve(risposta: String) -> (String, std::thread::JoinHandle<Vec<u8>>) {
        let ascolto = TcpListener::bind("127.0.0.1:0").unwrap();
        let autorita = format!("http://{}", ascolto.local_addr().unwrap());
        let filo = std::thread::spawn(move || {
            let (mut flusso, _) = ascolto.accept().unwrap();
            let richiesta = leggi_richiesta(&mut flusso);
            flusso.write_all(risposta.as_bytes()).unwrap();
            richiesta
        });
        (autorita, filo)
    }

    fn json(status: &str, corpo: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{corpo}",
            corpo.len()
        )
    }

    fn chiedi(autorita: &str, limiti: Limiti) -> Result<Secret, SqlError> {
        runtime().block_on(access_token_da(
            autorita,
            "contoso.onmicrosoft.com",
            "client",
            &Secret::new(SEGRETO.to_owned()),
            limiti,
        ))
    }

    #[test]
    fn il_token_si_legge_dalla_risposta_e_torna_in_un_secret() {
        let corpo = br#"{"token_type":"Bearer","expires_in":3599,"access_token":"eyJ0eXAi"}"#;
        assert_eq!(parse_token(corpo).unwrap().expose(), "eyJ0eXAi");
    }

    #[test]
    fn una_risposta_senza_token_o_con_token_vuoto_e_un_errore_di_autenticazione() {
        for corpo in [
            &br#"{"token_type":"Bearer"}"#[..],
            br#"{"access_token":""}"#,
            br#"{"access_token":42}"#,
            br#"[]"#,
            b"",
            b"non json",
        ] {
            let errore = parse_token(corpo).expect_err("doveva essere un errore");
            assert!(matches!(errore, SqlError::Auth(_)), "{corpo:?}");
        }
    }

    #[test]
    fn lerrore_di_entra_riporta_la_descrizione_non_solo_il_codice() {
        let corpo = br#"{"error":"invalid_client","error_description":"AADSTS7000215: Invalid client secret provided.\r\nTrace ID: 1"}"#;
        assert_eq!(
            describe_failure(401, corpo, ""),
            "Entra ID 401: AADSTS7000215: Invalid client secret provided."
        );
        assert_eq!(describe_failure(500, b"<html/>", ""), "Entra ID 500");
    }

    #[test]
    fn un_service_principal_incompleto_non_fa_nemmeno_la_richiesta() {
        let error = runtime()
            .block_on(access_token("", "client", &Secret::new("s".into())))
            .expect_err("doveva essere un errore");
        assert!(matches!(error, SqlError::Auth(_)));
    }

    #[test]
    fn il_tenant_ostile_non_compone_lurl() {
        for ostile in [
            "a/b",
            "a?x=1",
            "a@evil.example",
            "a#frammento",
            "a b",
            "a\\b",
            "a:8080",
            "a\nb",
            "a\0b",
            "..",
            ".",
            ".contoso.com",
            "contoso.com.",
            "contoso..com",
            "è.contoso.com",
            "\u{1F600}",
            &"x".repeat(300),
        ] {
            let errore = url_del_token("https://login.microsoftonline.com", ostile)
                .expect_err("un tenant ostile non deve comporre un URL");
            assert!(
                matches!(&errore, SqlError::Auth(testo) if testo.contains("Tenant")),
                "{ostile:?}: {errore}"
            );
        }
    }

    #[test]
    fn i_tenant_veri_compongono_lurl_giusto() {
        for buono in [
            "contoso.onmicrosoft.com",
            "72f988bf-86f1-41af-91ab-2d7cd011db47",
            "common",
            "organizations",
            "a",
        ] {
            assert_eq!(
                url_del_token("https://login.microsoftonline.com", buono).unwrap(),
                format!("https://login.microsoftonline.com/{buono}/oauth2/v2.0/token")
            );
        }
    }

    #[test]
    fn il_server_muto_scade_entro_il_tetto() {
        let ascolto = TcpListener::bind("127.0.0.1:0").unwrap();
        let autorita = format!("http://{}", ascolto.local_addr().unwrap());
        let (fine, attesa_fine) = mpsc::channel::<()>();
        let filo = std::thread::spawn(move || {
            let (flusso, _) = ascolto.accept().unwrap();
            // Tiene il socket aperto e non scrive mai.
            let _ = attesa_fine.recv_timeout(Duration::from_secs(10));
            drop(flusso);
        });

        let inizio = Instant::now();
        let errore = chiedi(&autorita, PROVA).expect_err("un server muto deve dare errore");
        let trascorso = inizio.elapsed();

        assert!(
            trascorso < Duration::from_secs(5),
            "ha aspettato {trascorso:?}"
        );
        assert!(
            matches!(&errore, SqlError::Auth(testo) if testo.contains("nessuna risposta")),
            "{errore}"
        );
        fine.send(()).unwrap();
        filo.join().unwrap();
    }

    #[test]
    fn un_corpo_infinito_e_troncato_al_tetto() {
        let ascolto = TcpListener::bind("127.0.0.1:0").unwrap();
        let autorita = format!("http://{}", ascolto.local_addr().unwrap());
        let (uscito, attesa_uscita) = mpsc::channel::<usize>();
        std::thread::spawn(move || {
            let (mut flusso, _) = ascolto.accept().unwrap();
            let _ = leggi_richiesta(&mut flusso);
            // Nessun Content-Length: il corpo dura finché il client non chiude.
            flusso
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n")
                .unwrap();
            let blocco = [b'a'; 8192];
            let mut scritti = 0usize;
            while flusso.write_all(&blocco).is_ok() {
                scritti += blocco.len();
                if scritti > 256 * 1024 * 1024 {
                    break;
                }
            }
            let _ = uscito.send(scritti);
        });

        let errore = chiedi(&autorita, PROVA).expect_err("un corpo infinito deve dare errore");
        assert!(
            matches!(&errore, SqlError::Auth(testo) if testo.contains("oltre 4096 byte")),
            "{errore}"
        );
        // Il client ha chiuso la connessione: il server smette di scrivere
        // molto prima dei 256 MiB con cui si fermerebbe da solo.
        let scritti = attesa_uscita
            .recv_timeout(Duration::from_secs(10))
            .expect("il server doveva accorgersi della chiusura");
        assert!(
            scritti < 64 * 1024 * 1024,
            "il client ha letto troppo: {scritti}"
        );
    }

    #[test]
    fn nessun_segreto_negli_errori_di_entra() {
        // Un server che ripete il segreto ricevuto nel proprio messaggio.
        let descrizione =
            format!("AADSTS7000215: secret {SEGRETO} rifiutato, ancora {SEGRETO}\\r\\nTrace ID: x");
        let corpo = format!(r#"{{"error":"invalid_client","error_description":"{descrizione}"}}"#);
        let (autorita, filo) = serve(json("401 Unauthorized", &corpo));
        let errore = chiedi(&autorita, PROVA).err().unwrap().to_string();
        let richiesta = String::from_utf8_lossy(&filo.join().unwrap()).into_owned();

        assert!(
            richiesta.contains(SEGRETO),
            "il segreto viaggia nel corpo della POST"
        );
        assert!(
            !errore.contains(SEGRETO),
            "il messaggio ripete il segreto: {errore}"
        );
        assert!(errore.contains("401"), "{errore}");

        // Una descrizione enorme non gonfia il messaggio.
        let lunga = format!(r#"{{"error_description":"{}"}}"#, "x".repeat(50_000));
        let testo = describe_failure(400, lunga.as_bytes(), SEGRETO);
        assert!(testo.chars().count() < 400, "{}", testo.chars().count());
    }

    #[test]
    fn una_risposta_non_json_non_riporta_il_corpo() {
        let corpo = format!("{SEGRETO} <html>non json</html>");
        let (autorita, filo) = serve(json("200 OK", &corpo));
        let errore = chiedi(&autorita, PROVA).err().unwrap().to_string();
        filo.join().unwrap();
        assert!(!errore.contains(SEGRETO), "{errore}");
        assert!(!errore.contains("html"), "{errore}");
    }

    #[test]
    fn nessun_redirect_porta_il_segreto_altrove() {
        let altrove = TcpListener::bind("127.0.0.1:0").unwrap();
        altrove.set_nonblocking(true).unwrap();
        let destinazione = format!("http://{}/rubato", altrove.local_addr().unwrap());
        let risposta = format!(
            "HTTP/1.1 307 Temporary Redirect\r\nLocation: {destinazione}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        let (autorita, filo) = serve(risposta);

        let errore = chiedi(&autorita, PROVA).err().unwrap().to_string();
        filo.join().unwrap();

        assert!(errore.contains("307"), "{errore}");
        assert!(
            matches!(altrove.accept(), Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock),
            "il client ha seguito il redirect con il segreto nel corpo"
        );
    }

    #[test]
    fn una_risposta_valida_da_un_server_vero_da_il_token() {
        let (autorita, filo) = serve(json(
            "200 OK",
            r#"{"access_token":"tok-123","expires_in":3599}"#,
        ));
        let token = chiedi(&autorita, PROVA).unwrap();
        let richiesta = String::from_utf8_lossy(&filo.join().unwrap()).into_owned();
        assert_eq!(token.expose(), "tok-123");
        assert!(richiesta.starts_with("POST /contoso.onmicrosoft.com/oauth2/v2.0/token "));
        assert!(richiesta.contains("grant_type=client_credentials"));
        assert!(richiesta.contains("client_id=client"));
    }
}
