# Sicurezza

## Segnalare una vulnerabilità

Non aprire una issue pubblica. Usa la segnalazione privata di GitHub:
scheda **Security** del repository → **Report a vulnerability**. Rispondo
lì. Lume è un'alfa mantenuta da una persona: non ci sono tempi garantiti, ma
una segnalazione di sicurezza passa davanti al resto.

## Modello di fiducia, com'è oggi

- **Language server.** Lume esegue i language server che trova: quello nello
  zip (`servers\`), quello scaricato da Lume, o quello nel `PATH`. Il binario
  nel `PATH` si esegue **senza controllare l'hash** (Lume lo avvia solo con `--version` per leggerne la versione). Per i server
  scaricati Lume verifica lo SHA-256 dell'archivio contro il catalogo al
  momento dell'installazione e scrive una ricevuta con l'hash del binario;
  quelli dello zip e quelli scaricati vengono poi confrontati con la loro
  ricevuta a ogni ricerca. Questo garantisce l'integrità, non l'autenticità,
  contro chi può scrivere nella cartella dati.
- **Workspace non fidati.** Non esiste un archivio di decisioni di fiducia: `lume-lsp`
  rifiuta ogni comando di origine workspace che non sia `Trusted`
  (`ServerCommand::validate`) e oggi la ricerca dei server non ne produce.
  La protezione di un progetto aperto poggia sulle opzioni con cui Lume
  avvia rust-analyzer (build script, proc macro e check-on-save spenti,
  `crates/lume-app/src/lsp_mode.rs`, `initialization_options`). Non aprire
  progetti di cui non ti fidi con altri language server.
- **TLS.** HTTPS, download dei language server e Azure SQL usano rustls con
  le radici del sistema (`rustls-platform-verifier`), CA aziendali incluse.
  Azure SQL impone la cifratura. Oracle parte in TLS di default (`tcps://`);
  il testo in chiaro esiste solo se scrivi `tcp://` tu. Le radici di Oracle
  restano quelle incorporate nel driver (`webpki-roots`), non quelle del
  sistema.
- **Segreti.** Le password dei database stanno nel magazzino credenziali del
  sistema operativo (Gestore credenziali su Windows, Portachiavi su macOS,
  Secret Service su Linux), mai nei file del progetto o nel database delle
  impostazioni. Su Linux il client è `dbus-secret-service` con libdbus 1.14.4
  compilata dal sorgente e inclusa nell'eseguibile: non usa la libdbus della
  distribuzione, quindi i suoi aggiornamenti di sicurezza arrivano solo
  ricompilando Lume (voce aperta, da rivalutare a ogni aggiornamento di
  `libdbus-sys`). `Secret` e `Redacted` azzerano i propri buffer al drop; il
  magazzino restituisce però una `String` intermedia che non viene azzerata.
- **Salvataggi.** Il temporaneo Windows nasce con una DACL protetta non più
  permissiva di quella dell'originale, verificata prima di scrivere il
  contenuto. Per i file nuovi l'accesso è limitato all'utente effettivo e
  SYSTEM. `ReplaceFileW` conserva la DACL dell'originale senza ignorare gli
  errori di sicurezza. Se il filesystem non supporta ACL persistenti o non
  può riprodurre le protezioni, il salvataggio fallisce senza fallback.
  Prima di sostituire un file esistente si verifica anche l'apertura in
  scrittura senza troncamento: la scrivibilità della directory non basta.
  L'atomicità è per file, non protegge da modifiche concorrenti e non
  garantisce rollback dei nomi per gli errori parziali di `ReplaceFileW`.
  Su Unix le ACL estese non sono conservate (vedi README).
- **Aggiornamenti.** Lume non si aggiorna da solo e non contatta la rete
  all'avvio. «Controlla aggiornamenti» interroga
  `api.github.com/repos/Otacon-spec/lume/releases/latest` solo su richiesta.
- **Eseguibile non firmato**: vedi il README.
- **Log dei crash**: niente contenuto dei documenti; la cartella utente vi
  compare come `~`.
