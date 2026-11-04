//! Provider crittografico di rustls per i client HTTPS di reqwest.
//!
//! Il workspace compila reqwest con `rustls-no-provider`, per non avere
//! `ring` accanto ad `aws-lc-rs` (lo usa oracledb). Il provider va quindi
//! installato prima di creare un client HTTPS. Senza, rustls va in panic alla
//! prima connessione: download dei language server, controllo degli
//! aggiornamenti e lo strumento `lume-server-bundle` della release Windows.

/// Installa una sola volta `aws-lc-rs` come provider di processo. Idempotente:
/// se un altro componente, per esempio lume-sql, l'ha già installato, va
/// bene uguale, perché il provider è lo stesso.
pub fn install_crypto_provider() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn dopo_l_installazione_un_client_https_si_costruisce() {
        super::install_crypto_provider();
        super::install_crypto_provider();
        assert!(rustls::crypto::CryptoProvider::get_default().is_some());
        assert!(reqwest::blocking::Client::builder().build().is_ok());
    }
}
