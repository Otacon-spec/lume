//! Compatibilità fra il vecchio magazzino zbus e il nuovo libdbus.
//!
//! Il test non inventa un Secret Service e non finge una voce: legge quella
//! che un programma esterno ha scritto con `keyring` 4.2 (feature `v1`, store
//! zbus) nella sessione D-Bus corrente, poi la aggiorna e la cancella con il
//! codice di Lume. Si esegue solo su richiesta esplicita, con
//! `LUME_VAULT_COMPATIBILITA=1`; senza la variabile salta, perché su una
//! macchina qualunque quella voce non esiste.
//!
//! La ricetta completa (bus privato, gnome-keyring sbloccato, HOME temporanea)
//! è nella ricevuta `docs/consolidamento/esiti/supply-chain-rustls-20261102.md`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use lume_sql::{OsVault, Secret, SecretStore};

/// Servizio e chiave scritti dal programma di semina. Sono fissi perché la
/// ricetta esterna li conosce; il servizio è distinto da quello di
/// produzione, così un residuo si riconosce a occhio.
const SERVIZIO: &str = "Lume (compatibilità zbus)";
const CHIAVE: &str = "voce-scritta-da-zbus";
const VALORE_VECCHIO: &str = "scritto-da-zbus";
const VALORE_NUOVO: &str = "scritto-da-libdbus";

#[test]
fn una_voce_scritta_dal_magazzino_zbus_resta_leggibile_aggiornabile_e_cancellabile() {
    if std::env::var_os("LUME_VAULT_COMPATIBILITA").as_deref() != Some(std::ffi::OsStr::new("1")) {
        eprintln!(
            "SALTATO: manca LUME_VAULT_COMPATIBILITA=1 (la voce va seminata dal vecchio store)"
        );
        return;
    }

    let vault = OsVault::with_service(SERVIZIO);

    let letto = vault
        .get(CHIAVE)
        .expect("lettura della voce seminata dal magazzino zbus")
        .expect("la voce seminata dal magazzino zbus deve esistere");
    assert_eq!(
        letto.expose(),
        VALORE_VECCHIO,
        "il nuovo store deve leggere il valore scritto da zbus"
    );

    vault
        .set(CHIAVE, &Secret::new(VALORE_NUOVO.to_owned()))
        .expect("aggiornamento della voce scritta da zbus");
    assert_eq!(
        vault
            .get(CHIAVE)
            .expect("rilettura dopo l'aggiornamento")
            .as_ref()
            .map(Secret::expose),
        Some(VALORE_NUOVO),
        "l'aggiornamento deve restare nello stesso item"
    );

    vault
        .forget(CHIAVE)
        .expect("cancellazione della voce scritta da zbus");
    assert!(
        vault
            .get(CHIAVE)
            .expect("lettura dopo la cancellazione")
            .is_none(),
        "la voce doveva sparire dal magazzino"
    );
    vault
        .forget(CHIAVE)
        .expect("dimenticare due volte non è un errore");
}
