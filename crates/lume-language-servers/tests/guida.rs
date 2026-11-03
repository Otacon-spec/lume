#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
use std::collections::BTreeSet;

use lume_language_servers::{CatalogError, ServerCatalog};
use serde_json::Value;

const GUIDA: &str = include_str!("../../../docs/estendere/aggiungere-un-language-server.md");
const CATALOGO: &str = include_str!("../assets/server-releases.json");
const CATALOG_RS: &str = include_str!("../src/catalog.rs");
const LSP_MODE_RS: &str = include_str!("../../lume-app/src/lsp_mode.rs");
const BUNDLER: &str = include_str!("../src/bin/lume-server-bundle.rs");

fn primo_blocco_json() -> String {
    let dopo = GUIDA
        .split("```json")
        .nth(1)
        .expect("la guida ha un blocco ```json");
    dopo.split("```").next().unwrap().to_owned()
}

fn chiavi(valore: &Value, fuori: &mut BTreeSet<String>) {
    match valore {
        Value::Object(mappa) => {
            for (chiave, figlio) in mappa {
                fuori.insert(chiave.clone());
                chiavi(figlio, fuori);
            }
        }
        Value::Array(elementi) => elementi.iter().for_each(|figlio| chiavi(figlio, fuori)),
        _ => {}
    }
}

fn catalogo_con(server: &str) -> String {
    format!(r#"{{"schema":1,"servers":[{server}]}}"#)
}

#[test]
fn la_guida_spiega_ogni_chiave_del_catalogo() {
    let mut trovate = BTreeSet::new();
    chiavi(
        &serde_json::from_str::<Value>(CATALOGO).unwrap(),
        &mut trovate,
    );
    chiavi(
        &serde_json::from_str::<Value>(&primo_blocco_json()).unwrap(),
        &mut trovate,
    );
    for chiave in trovate {
        assert!(
            GUIDA.contains(&format!("`{chiave}`")),
            "la guida non spiega la chiave `{chiave}`"
        );
    }
}

#[test]
fn l_esempio_e_rifiutato_finche_manca_la_voce_nella_lista_ufficiale() {
    let esempio = primo_blocco_json();
    let Err(errore) = ServerCatalog::parse(&catalogo_con(&esempio)) else {
        panic!("l'esempio della guida non dovrebbe passare senza la voce in validate_server");
    };
    assert_eq!(errore, CatalogError::UnknownServer("esempio-ls".to_owned()));
}

#[test]
fn l_esempio_e_valido_quando_la_voce_c_e() {
    let come_rumdl = primo_blocco_json()
        .replace("esempio/esempio-ls", "rvben/rumdl")
        .replace("esempio-ls", "rumdl");
    let catalogo = ServerCatalog::parse(&catalogo_con(&come_rumdl)).unwrap();
    assert!(catalogo.get("rumdl").is_some());
}

#[test]
fn la_guida_cita_i_simboli_e_i_simboli_esistono_ancora() {
    for (simbolo, sorgente, firma) in [
        ("validate_server", CATALOG_RS, "fn validate_server("),
        (
            "initialization_options",
            LSP_MODE_RS,
            "pub fn initialization_options(",
        ),
        ("UnknownServer", CATALOG_RS, "UnknownServer(String)"),
        ("DuplicateExtension", CATALOG_RS, "DuplicateExtension("),
    ] {
        assert!(
            GUIDA.contains(&format!("`{simbolo}`")),
            "la guida non cita `{simbolo}`"
        );
        assert!(sorgente.contains(firma), "`{simbolo}` non esiste più");
    }
}

#[test]
fn la_guida_spiega_le_licenze_nello_zip() {
    assert!(
        GUIDA.contains("## 5. Licenze nello zip"),
        "la guida deve avere la sezione «5. Licenze nello zip»"
    );
    for simbolo in [
        "THIRD_PARTY_LICENSE_FILES",
        "THIRD_PARTY_SERVERS",
        "-LICENSE-",
        "ogni_server_del_catalogo_ha_i_testi_di_licenza_nel_bundle",
    ] {
        assert!(GUIDA.contains(simbolo), "la guida non nomina `{simbolo}`");
    }
    for definizione in [
        "const THIRD_PARTY_LICENSE_FILES",
        "const THIRD_PARTY_SERVERS",
        "fn ogni_server_del_catalogo_ha_i_testi_di_licenza_nel_bundle",
    ] {
        assert!(
            BUNDLER.contains(definizione),
            "il bundler non definisce più `{definizione}`: aggiorna la guida"
        );
    }
}
