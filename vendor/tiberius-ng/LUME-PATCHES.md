# Patch locali di Lume a tiberius-ng 0.13.1

Questa cartella e' una copia di `tiberius-ng` 0.13.1 (crates.io), collegata con
`[patch.crates-io]` nel `Cargo.toml` di Lume. Ogni modifica rispetto
all'originale va elencata qui.

## 1. Radici TLS: webpki-roots
La feature `rustls` usa `webpki-roots` (le stesse radici di reqwest) invece di
`rustls-native-certs`, per non avere due stack di fiducia. Vedi il commento
sopra `tiberius-ng` nel `Cargo.toml` di Lume, l'intestazione di questo
`Cargo.toml`, `src/client/tls_stream/rustls_tls_stream.rs` e
`THIRD_PARTY_NOTICES.md`. Rimossi anche il target di test `custom-cert` e la
cartella `docker/` (conteneva chiavi private di prova).

## 2. `cancel_request` a livello di pacchetto (N31)
File: `src/client/connection.rs`, `Connection::cancel_request` e
`drain_until_attention_ack`.
- Prima decodificava token fino al `DONE_ATTN`. Dopo un decoder lasciato cadere
  a meta' token (un `select!` su un SELECT grosso) `buf` teneva byte di un
  token incompleto e i pacchetti successivi potevano partire dentro un token:
  risultato "invalid token type N" o attesa senza fine.
- Ora tronca `buf`, invia l'Attention e scarta pacchetti interi; riconosce la
  conferma dal token DONE/DONEPROC/DONEINPROC (a dimensione fissa, sempre
  l'ultimo del messaggio) con il bit `DONE_ATTN` negli ultimi byte del
  messaggio, anche se spezzato fra due pacchetti.
- La connessione e' segnata `poisoned` fino alla conferma: un futuro
  interrotto (timeout di chi chiama) o un errore lasciano la connessione
  inutilizzabile e la prossima operazione fallisce subito.
- `cancel_request` ritorna `Result<()>`; rimosso `TokenStream::flush_done_attention`
  (`src/tds/stream/token.rs`); `TokenDone::is_attention` e' sotto
  `#[cfg(test)]` (`src/tds/codec/token/token_done.rs`).
- `Client::cancel_query` (`src/client.rs`) non ha un timeout proprio: lo impone
  il chiamante (Lume: 5 s in `crates/lume-sql/src/azure.rs`).

## 3. Test `cancel_tests`
In `src/client/connection.rs`, modulo `cancel_tests`: una `Connection` vera su
`tokio::io::duplex` (byte veri; il lato "server" scrive solo pacchetti TDS,
non e' un server finto).

## I test del vendor non girano in CI
Il vendor non e' membro del workspace (`members = ["crates/*"]`): `cargo test
-p tiberius-ng` fallisce ("requires dev-dependencies and is not a member") e
nessun cancello di Lume lo esegue; i cancelli compilano solo la libreria.
Per rilanciare i test a mano: copiare questa cartella fuori dal repo, lasciare
nel `Cargo.toml` copiato solo i dev-dependencies `tokio`, `tokio-util` e
`uuid`, togliere i blocchi `[[example]]` e `[[test]]`, aggiungere `[workspace]`
in fondo, copiarci il `Cargo.lock` di Lume e lanciare:

    cargo test --lib --no-default-features --features tds73,rustls,chrono,rust_decimal,winauth -- cancel_
