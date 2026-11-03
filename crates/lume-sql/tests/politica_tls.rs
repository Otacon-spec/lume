//! Politica delle radici TLS (N20), verificata sul `Cargo.lock`.
//!
//! Non è un handshake vero: quello richiede un server. È il controllo che
//! nessun crate riporti in Lume una seconda lista di radici. Le radici del
//! sistema operativo sono l'unico posto dove l'utente può aggiungere una CA
//! aziendale; una lista incorporata (`webpki-roots`) la ignora.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::Path;

fn lockfile() -> String {
    let percorso = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.lock");
    std::fs::read_to_string(&percorso)
        .unwrap_or_else(|e| panic!("{} non leggibile: {e}", percorso.display()))
}

/// I blocchi `[[package]]` del lock come (nome, dipendenze). Una voce di
/// dipendenza è `"nome"` oppure `"nome versione"` (o `"nome versione (fonte)"`
/// quando ci sono più versioni): serve solo il primo token.
fn pacchetti(lock: &str) -> Vec<(String, Vec<String>)> {
    let mut trovati = Vec::new();
    for blocco in lock.split("[[package]]").skip(1) {
        let mut nome = None;
        let mut dipendenze = Vec::new();
        let mut nell_elenco = false;
        for riga in blocco.lines() {
            let riga = riga.trim();
            if nome.is_none()
                && let Some(valore) = riga.strip_prefix("name = ")
            {
                nome = Some(valore.trim_matches('"').to_owned());
                continue;
            }
            if riga == "dependencies = [" {
                nell_elenco = true;
                continue;
            }
            if nell_elenco {
                if riga == "]" {
                    nell_elenco = false;
                    continue;
                }
                let voce = riga.trim_end_matches(',').trim_matches('"');
                if let Some(primo) = voce.split_whitespace().next() {
                    dipendenze.push(primo.to_owned());
                }
            }
        }
        if let Some(nome) = nome {
            trovati.push((nome, dipendenze));
        }
    }
    trovati
}

/// I nomi (ordinati, senza doppioni) dei pacchetti che dipendono da `pacchetto`.
fn dipendenti_di(lock: &str, pacchetto: &str) -> Vec<String> {
    let mut nomi: Vec<String> = pacchetti(lock)
        .into_iter()
        .filter(|(_, dipendenze)| dipendenze.iter().any(|d| d == pacchetto))
        .map(|(nome, _)| nome)
        .collect();
    nomi.sort();
    nomi.dedup();
    nomi
}

/// Se il parser sbagliasse e non trovasse nulla, gli altri test passerebbero
/// a vuoto: questo li ancora a fatti noti del workspace.
#[test]
fn il_parser_del_lock_trova_i_pacchetti_noti() {
    let lock = lockfile();
    assert!(
        dipendenti_di(&lock, "tiberius-ng").contains(&"lume-sql".to_owned()),
        "lume-sql deve dipendere da tiberius-ng"
    );
    assert!(
        dipendenti_di(&lock, "oracledb").contains(&"lume-sql".to_owned()),
        "lume-sql deve dipendere da oracledb"
    );
}

#[test]
fn webpki_roots_serve_solo_a_oracle() {
    let dipendenti = dipendenti_di(&lockfile(), "webpki-roots");
    assert_eq!(
        dipendenti,
        vec!["oracledb".to_owned()],
        "solo oracledb (che le cabla e non si corregge da qui) può usare webpki-roots; \
         gli altri devono usare le radici del sistema"
    );
}

#[test]
fn reqwest_e_una_sola_versione() {
    let quante = pacchetti(&lockfile())
        .iter()
        .filter(|(nome, _)| nome == "reqwest")
        .count();
    assert_eq!(
        quante, 1,
        "due versioni di reqwest = due politiche di radici (serve L.3, vedi P2)"
    );
}

#[test]
fn tiberius_e_reqwest_usano_il_verificatore_di_piattaforma() {
    let dipendenti = dipendenti_di(&lockfile(), "rustls-platform-verifier");
    for atteso in ["tiberius-ng", "reqwest"] {
        assert!(
            dipendenti.contains(&atteso.to_owned()),
            "{atteso} deve usare rustls-platform-verifier; lo usano: {dipendenti:?}"
        );
    }
}
