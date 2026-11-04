//! I segreti nella cassaforte del sistema operativo.
//!
//! Il vincolo della spec è che password e client secret **non** finiscano mai
//! nei file di progetto né nel database delle impostazioni. `keyring-core`
//! non sceglie niente da sé: la scelta è qui, una volta per processo, e
//! replica quella che faceva la feature `v1` del facade `keyring` 4.2 —
//! Gestore credenziali su Windows, Portachiavi su macOS, Secret Service su
//! Linux. Il facade non si può usare con `default-features = false` (senza
//! `v1` o `cli` il suo `lib.rs` ferma la compilazione) e `v1` lega Linux a
//! zbus 5, «unmaintained» senza versione corretta (RUSTSEC-2026-0370/0371/
//! 0372). Gli store sono quindi dipendenze esplicite, sotto il `cfg` della
//! piattaforma, e questo modulo fa la loro inizializzazione una tantum.
//!
//! **La cassaforte può non esserci**: macchina irrigidita, sessione Linux
//! senza il demone, portachiavi bloccato. Non è un errore fatale — si torna a
//! chiedere il segreto a ogni connessione, che è esattamente il comportamento
//! che c'era prima di questo modulo. Per questo `get` distingue «non c'è
//! niente» (`Ok(None)`, si chiede e basta) da «la cassaforte non risponde»
//! (`Err`, si chiede **e** lo si dice una volta).
//!
//! ATTENZIONE: **Limite noto:** il magazzino restituisce il segreto come
//! `String`, e quel buffer intermedio non viene azzerato. `Secret::new` lo
//! prende per valore senza copiarlo, quindi l'unica copia che resta in memoria
//! liberata è dentro il magazzino. Azzerarla richiederebbe l'API a byte dello
//! store e una `String` che non passa mai di mano: non è lavoro di questo giro.

use std::sync::{Arc, OnceLock};

use crate::Secret;

/// Nome del servizio sotto cui i segreti finiscono nel magazzino. Su Windows
/// è il prefisso che si legge nel Gestore credenziali, quindi va tenuto
/// riconoscibile a occhio.
pub const SERVICE: &str = "Lume";

#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    /// Non c'è nessun magazzino utilizzabile su questa macchina.
    #[error("cassaforte di sistema non disponibile: {0}")]
    Unavailable(String),
    /// C'è, ma questa operazione è fallita.
    #[error("cassaforte di sistema: {0}")]
    Failed(String),
}

/// Dove vivono password e client secret.
///
/// È un trait e non una funzione libera perché i test dell'app devono poter
/// iniettare un doppio — stessa ragione per cui `DriverFactory` è un
/// puntatore a funzione: la cosa vera parla con il sistema operativo.
pub trait SecretStore: Send + Sync + std::fmt::Debug {
    /// `Ok(None)` quando non c'è niente in cassaforte: si chiede e basta.
    fn get(&self, key: &str) -> Result<Option<Secret>, VaultError>;
    fn set(&self, key: &str, secret: &Secret) -> Result<(), VaultError>;
    /// Idempotente: dimenticare qualcosa che non c'è non è un errore.
    fn forget(&self, key: &str) -> Result<(), VaultError>;
}

/// La cassaforte vera.
#[derive(Debug)]
pub struct OsVault {
    service: String,
}

impl OsVault {
    pub fn new() -> Self {
        Self {
            service: SERVICE.to_owned(),
        }
    }

    /// Con un servizio diverso da quello di produzione: serve al test che
    /// parla con il Gestore credenziali vero senza rischiare di calpestare i
    /// segreti dell'utente.
    pub fn with_service(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }

    fn entry(&self, key: &str) -> Result<keyring_core::Entry, VaultError> {
        match store() {
            Ok(()) => {
                keyring_core::Entry::new(&self.service, key).map_err(|error| classify(&error))
            }
            Err(error) => Err(VaultError::Unavailable(error.to_string())),
        }
    }
}

impl Default for OsVault {
    fn default() -> Self {
        Self::new()
    }
}

impl SecretStore for OsVault {
    fn get(&self, key: &str) -> Result<Option<Secret>, VaultError> {
        match self.entry(key)?.get_password() {
            Ok(password) => Ok(Some(Secret::new(password))),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(error) => Err(classify(&error)),
        }
    }

    fn set(&self, key: &str, secret: &Secret) -> Result<(), VaultError> {
        self.entry(key)?
            .set_password(secret.expose())
            .map_err(|error| classify(&error))
    }

    fn forget(&self, key: &str) -> Result<(), VaultError> {
        match self.entry(key)?.delete_credential() {
            // Dimenticare quel che non c'è è il risultato voluto, non un
            // fallimento: chiamarlo due volte deve poter succedere.
            Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
            Err(error) => Err(classify(&error)),
        }
    }
}

/// Esito dell'inizializzazione una tantum del magazzino.
///
/// `keyring-core` tiene un solo magazzino predefinito per processo. La prima
/// operazione sceglie quello della piattaforma e ne conserva l'esito: se
/// fallisce non si ritenta a ogni lettura, ma ogni operazione riporta
/// all'utente lo stesso errore.
static STORE: OnceLock<Result<(), keyring_core::Error>> = OnceLock::new();

fn store() -> Result<(), &'static keyring_core::Error> {
    STORE.get_or_init(init_store).as_ref().map(|_| ())
}

fn init_store() -> Result<(), keyring_core::Error> {
    let store = create_store()?;
    keyring_core::set_default_store(store);
    Ok(())
}

#[cfg(target_os = "windows")]
fn create_store() -> keyring_core::Result<Arc<keyring_core::CredentialStore>> {
    Ok(windows_native_keyring_store::Store::new()?)
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn create_store() -> keyring_core::Result<Arc<keyring_core::CredentialStore>> {
    Ok(apple_native_keyring_store::keychain::Store::new()?)
}

#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "ios", target_os = "android"))
))]
fn create_store() -> keyring_core::Result<Arc<keyring_core::CredentialStore>> {
    Ok(dbus_secret_service_keyring_store::Store::new()?)
}

#[cfg(not(any(
    target_os = "windows",
    target_os = "macos",
    target_os = "ios",
    all(unix, not(target_os = "android"))
)))]
fn create_store() -> keyring_core::Result<Arc<keyring_core::CredentialStore>> {
    Err(keyring_core::Error::Invalid(
        "platform".to_owned(),
        "Lume usa Gestore credenziali, Portachiavi o Secret Service".to_owned(),
    ))
}

/// «Non c'è cassaforte» e «la cassaforte ha detto di no» portano l'utente a
/// due conclusioni diverse, quindi restano due varianti. Un magazzino che non
/// si riesce nemmeno a creare è assente; un'operazione rifiutata per accesso
/// negato (portachiavi bloccato) è anch'essa assenza, non un guasto del
/// codice.
fn classify(error: &keyring_core::Error) -> VaultError {
    match error {
        keyring_core::Error::NoDefaultStore
        | keyring_core::Error::NotSupportedByStore(_)
        | keyring_core::Error::NoStorageAccess(_) => VaultError::Unavailable(error.to_string()),
        _ => VaultError::Failed(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    /// L'unico test che parla con il **Gestore credenziali vero**: senza,
    /// l'uso dell'API di `keyring-core` resterebbe non verificato come i
    /// driver. Il servizio è distinto da quello di produzione, così un residuo
    /// lasciato da un panic si riconosce a occhio e non si confonde con un
    /// segreto dell'utente.
    #[test]
    fn un_giro_completo_contro_la_cassaforte_del_sistema() {
        let vault = OsVault::with_service("Lume (test)");
        let key = format!(
            "prova-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("orologio")
                .as_nanos()
        );

        // In locale una cassaforte può non essere disponibile. Il job Linux
        // avvia Secret Service e imposta questa variabile, così l'assenza del
        // backend diventa un errore esplicito invece di un falso verde.
        if let Err(VaultError::Unavailable(_)) = vault.get(&key) {
            assert_ne!(
                std::env::var_os("LUME_REQUIRE_SYSTEM_VAULT").as_deref(),
                Some(std::ffi::OsStr::new("1")),
                "cassaforte di sistema richiesta ma non disponibile"
            );
            eprintln!("cassaforte di sistema assente: prova d'integrazione non eseguita");
            return;
        }

        assert!(
            vault.get(&key).expect("lettura").is_none(),
            "una chiave mai scritta non deve esistere"
        );

        // Un Secret Service raggiungibile ma senza collezione predefinita
        // (runner CI senza keyring sbloccato) risponde alla lettura e rifiuta
        // la scrittura: stessa regola dell'assenza.
        match vault.set(&key, &Secret::new("p4ssw0rd".into())) {
            Ok(()) => {}
            Err(VaultError::Unavailable(motivo)) => {
                assert_ne!(
                    std::env::var_os("LUME_REQUIRE_SYSTEM_VAULT").as_deref(),
                    Some(std::ffi::OsStr::new("1")),
                    "cassaforte di sistema richiesta ma non scrivibile: {motivo}"
                );
                eprintln!(
                    "cassaforte di sistema non scrivibile: prova d'integrazione non eseguita"
                );
                return;
            }
            Err(errore) => panic!("scrittura: {errore:?}"),
        }
        assert_eq!(
            vault
                .get(&key)
                .expect("rilettura")
                .as_ref()
                .map(Secret::expose),
            Some("p4ssw0rd"),
        );

        vault.forget(&key).expect("cancellazione");
        assert!(
            vault
                .get(&key)
                .expect("lettura dopo cancellazione")
                .is_none(),
            "il segreto doveva sparire dal magazzino"
        );
        vault
            .forget(&key)
            .expect("dimenticare due volte non è un errore");
    }
}
