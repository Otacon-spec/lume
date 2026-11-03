//! Solo formattazione dei tipi reali: nessun driver, server o credenziale usati.
use lume_sql::{ColumnInfo, QueryBatch, QueryEvent, Request, Response, ResultSet, SqlError, Value};
use std::time::Duration;

const CONTENUTO: &str = "contenuto_privato_della_fixture_54";

fn colonna() -> ColumnInfo {
    ColumnInfo {
        name: CONTENUTO.into(),
        type_name: CONTENUTO.into(),
    }
}

fn risultato() -> ResultSet {
    ResultSet {
        columns: vec![colonna()],
        rows: vec![vec![Value::Text(CONTENUTO.into())]],
        elapsed: Duration::from_millis(3),
    }
}

fn assert_privato(valore: &impl std::fmt::Debug) {
    for testo in [
        format!("{valore:?}"),
        format!("{valore:#?}"),
        format!("{:?}", Some(Ok::<_, SqlError>(valore))),
        format!("{:#?}", Some(Ok::<_, SqlError>(valore))),
    ] {
        assert!(
            !testo.contains(CONTENUTO),
            "contenuto emesso dal Debug: {testo}"
        );
    }
}

#[test]
fn celle_private_nel_debug_ma_leggibili_nella_griglia() {
    let testo = Value::Text(CONTENUTO.into());
    assert_privato(&testo);
    assert_eq!(testo.to_string(), CONTENUTO);
    for (valore, presentazione, diagnosi) in [
        (Value::Bool(true), "true", "Bool(…)"),
        (Value::Int(987654321234), "987654321234", "Int(…)"),
        (Value::Float(54321.125), "54321.125", "Float(…)"),
    ] {
        assert_eq!(format!("{valore:?}"), diagnosi);
        assert_eq!(format!("{valore:#?}"), diagnosi);
        assert_eq!(
            format!("{:?}", Some(valore.clone())),
            format!("Some({diagnosi})")
        );
        assert_eq!(valore.to_string(), presentazione);
    }
    let blob = Value::Bytes(vec![211, 212, 213]);
    assert_eq!(format!("{blob:?}"), "Bytes { byte: 3 }");
    assert_eq!(format!("{blob:#?}"), "Bytes {\n    byte: 3,\n}");
    assert_eq!(
        format!("{:?}", Some(blob.clone())),
        "Some(Bytes { byte: 3 })"
    );
    assert_eq!(blob.to_string(), "0x3 byte");
    assert_eq!(Value::Null.to_string(), "NULL");
    assert_eq!(format!("{:?}", Value::Null), "Null");
}

#[test]
fn nomi_di_colonne_e_tipi_non_si_copiano_nella_diagnostica() {
    let info = colonna();
    assert_privato(&info);
    assert_eq!(info.name, CONTENUTO);
    assert_eq!(info.type_name, CONTENUTO);
}

#[test]
fn risultati_e_batch_non_stampano_le_righe() {
    let result = risultato();
    assert_privato(&result);
    let batch = QueryBatch::new(result.rows.clone());
    assert_privato(&batch);
    assert_eq!(result.row_count(), 1);
    assert_eq!(batch.rows, result.rows);
    assert!(batch.accounted_bytes > 0);
}

#[test]
fn richieste_sql_non_stampano_testo_o_identificatori() {
    assert_privato(&Request::Columns(CONTENUTO.into()));
    assert_eq!(format!("{:?}", Request::Tables), "Tables");
}

#[test]
fn eventi_non_stampano_schema_batch_o_errore_remoto() {
    for evento in [
        QueryEvent::Started {
            columns: vec![colonna()],
        },
        QueryEvent::Batch(QueryBatch::new(risultato().rows)),
        QueryEvent::Failed(CONTENUTO.into()),
    ] {
        assert_privato(&evento);
    }
    for (evento, atteso) in [
        (
            QueryEvent::BudgetReached {
                rows: 2,
                accounted_bytes: 100,
            },
            "BudgetReached { righe: 2, byte_contabilizzati: 100 }",
        ),
        (
            QueryEvent::Stopped {
                rows: 2,
                accounted_bytes: 100,
            },
            "Stopped { righe: 2, byte_contabilizzati: 100 }",
        ),
        (
            QueryEvent::Completed {
                elapsed: Duration::from_millis(3),
            },
            "Completed { elapsed: 3ms }",
        ),
    ] {
        assert_eq!(format!("{evento:?}"), atteso);
        assert_eq!(format!("{:?}", Some(evento)), format!("Some({atteso})"));
    }
}

#[test]
fn risposte_annidate_non_stampano_tabelle_colonne_o_dati() {
    for risposta in [
        Response::Tables(vec![CONTENUTO.into()]),
        Response::Columns {
            table: CONTENUTO.into(),
            columns: vec![colonna()],
        },
    ] {
        assert_privato(&Some(Ok::<_, SqlError>(risposta)));
    }
}

#[test]
fn errori_redatti_in_debug_senza_cambiare_il_messaggio_utente() {
    for (errore, prefisso) in [
        (SqlError::Connect(CONTENUTO.into()), "connessione fallita: "),
        (SqlError::Auth(CONTENUTO.into()), "autenticazione fallita: "),
        (SqlError::Query(CONTENUTO.into()), "query fallita: "),
        (
            SqlError::Unavailable(CONTENUTO.into()),
            "connessione non disponibile: ",
        ),
    ] {
        assert_privato(&errore);
        assert_privato(&Err::<(), _>(&errore));
        assert_eq!(errore.to_string(), format!("{prefisso}{CONTENUTO}"));
    }
    assert_eq!(SqlError::Superseded.to_string(), "richiesta superata");
    assert_eq!(format!("{:?}", SqlError::Superseded), "Superseded");
    assert_eq!(format!("{:#?}", SqlError::Superseded), "Superseded");
}

#[test]
fn a_metadati_uguali_debug_non_dipende_dal_contenuto() {
    fn stessa_diagnosi(a: &impl std::fmt::Debug, b: &impl std::fmt::Debug) {
        assert_eq!(format!("{a:?}"), format!("{b:?}"));
        assert_eq!(format!("{a:#?}"), format!("{b:#?}"));
    }
    let diverso = "x".repeat(CONTENUTO.len());
    stessa_diagnosi(
        &Value::Text(CONTENUTO.into()),
        &Value::Text(diverso.clone()),
    );
    stessa_diagnosi(&Value::Bool(true), &Value::Bool(false));
    stessa_diagnosi(&Value::Int(1), &Value::Int(-900));
    stessa_diagnosi(&Value::Float(f64::NAN), &Value::Float(f64::INFINITY));
    stessa_diagnosi(
        &Value::Bytes(vec![1, 2, 3]),
        &Value::Bytes(vec![91, 92, 93]),
    );
    let info = ColumnInfo {
        name: diverso.clone(),
        type_name: diverso.clone(),
    };
    stessa_diagnosi(&colonna(), &info);
    let a = risultato();
    let b = ResultSet {
        columns: vec![info.clone()],
        rows: vec![vec![Value::Text(diverso.clone())]],
        elapsed: a.elapsed,
    };
    stessa_diagnosi(&a, &b);
    let batch_a = QueryBatch::new(a.rows.clone());
    let batch_b = QueryBatch::new(b.rows.clone());
    assert_eq!(batch_a.accounted_bytes, batch_b.accounted_bytes);
    stessa_diagnosi(&batch_a, &batch_b);
    stessa_diagnosi(
        &Request::Columns(CONTENUTO.into()),
        &Request::Columns(diverso.clone()),
    );
    stessa_diagnosi(
        &QueryEvent::Started {
            columns: a.columns.clone(),
        },
        &QueryEvent::Started {
            columns: b.columns.clone(),
        },
    );
    stessa_diagnosi(&QueryEvent::Batch(batch_a), &QueryEvent::Batch(batch_b));
    stessa_diagnosi(
        &QueryEvent::Failed(CONTENUTO.into()),
        &QueryEvent::Failed(diverso.clone()),
    );
    stessa_diagnosi(
        &Response::Tables(vec![CONTENUTO.into()]),
        &Response::Tables(vec![diverso.clone()]),
    );
    stessa_diagnosi(
        &Response::Columns {
            table: CONTENUTO.into(),
            columns: a.columns.clone(),
        },
        &Response::Columns {
            table: diverso.clone(),
            columns: b.columns.clone(),
        },
    );
    for (a, b) in [
        (
            SqlError::Connect(CONTENUTO.into()),
            SqlError::Connect(diverso.clone()),
        ),
        (
            SqlError::Auth(CONTENUTO.into()),
            SqlError::Auth(diverso.clone()),
        ),
        (
            SqlError::Query(CONTENUTO.into()),
            SqlError::Query(diverso.clone()),
        ),
        (
            SqlError::Unavailable(CONTENUTO.into()),
            SqlError::Unavailable(diverso),
        ),
    ] {
        stessa_diagnosi(&a, &b);
    }
    assert_eq!(
        a.rows
            .first()
            .and_then(|row| row.first())
            .map(ToString::to_string),
        Some(CONTENUTO.to_owned())
    );
}

#[test]
fn lunghezze_diagnostiche_sono_byte_non_caratteri() {
    let valore = Value::Text("è".into());
    assert_eq!(format!("{valore:?}"), "Text { byte: 2 }");
    assert_eq!(valore.to_string(), "è");
    assert_eq!(
        format!("{:?}", Request::Columns("è".into())),
        "Columns { nome_byte: 2 }"
    );
    assert_eq!(
        format!(
            "{:?}",
            ColumnInfo {
                name: "è".into(),
                type_name: "😺".into()
            }
        ),
        "ColumnInfo { nome_byte: 2, tipo_byte: 4 }"
    );
}
