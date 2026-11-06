# Registro delle modifiche

Formato ispirato a [Keep a Changelog](https://keepachangelog.com/it-IT/1.1.0/);
versioni [SemVer](https://semver.org/lang/it/). Le versioni fino alla 0.3.0
non hanno avuto una release pubblica; la 0.4.0 è la prima release pubblica,
in alpha e qualificata su Linux.

## [Non rilasciato]

Nessuna modifica dopo la 0.4.1.

## [0.4.1] — 2026-11-06

Correzioni sulla 0.4.0 per Windows e Linux; limiti noti invariati, vedi le
[note di rilascio](docs/note-di-rilascio-0.4.1.md).

### Aggiunto
- «Aiuto → Scarica aggiornamento…»: controlla la release più recente e, se
  è nuova, scarica lo zip (Windows) o il `.tar.gz` (Linux) nella cartella
  Download (o nella home, se il sistema non ne indica una), ne verifica dimensione e sha256 e mostra il percorso. Lume non
  estrae il pacchetto e non sostituisce sé stesso: l'estrazione resta un
  gesto dell'utente.

### Corretto
- Su Windows `lume.exe` parte anche senza «Visual C++ Redistributable»: il
  runtime C è incluso nell'eseguibile (prima: errore `VCRUNTIME140.dll` non
  trovato).
- «Controlla aggiornamenti…» riconosce i pacchetti con il nome con cui sono
  pubblicati (`lume-<versione>-<piattaforma>-<sha>.zip`/`.tar.gz`); prima
  su Windows e Linux non li trovava mai.
- Scroll dell'editor: le righe visibili si disegnano in un solo livello
  invece di uno per riga, e i colori della sintassi vengono chiesti prima che
  lo scroll esca dalla parte già colorata. Misurato su Linux: meno CPU per
  frame con il renderer software. Lo sfarfallio durante lo scroll veloce su
  Windows non è ancora riprodotto né dichiarato risolto.

## [0.4.0] — 2026-11-05

Prima release pubblica: **alpha** qualificata su Linux x86_64, con pacchetto
anche per Windows x64 (test in CI, uso reale non collaudato); macOS non è in
questa release. Raccoglie le
voci della 0.3.0 del 2026-09-30, che non ha avuto una release pubblica, e
quelle elencate qui sotto. I limiti della qualifica sono in «Limiti noti di
questa alpha» nel README e nelle
[note di rilascio](docs/note-di-rilascio-0.4.0.md).

### Aggiunto
- Riquadro «Aiuto → Scorciatoie…» e tabella nel README, derivati dai menù e filtrati secondo funzionalità e capacità LSP attive.
- Server MCP sperimentale dietro la feature Cargo opzionale `mcp`: tool su editor, file, diagnostiche e Git, named pipe con token per sessione, avvio esplicito e ponte `lume.exe --mcp-stdio`.
- Server MCP: quattro tool del language server (`get_hover`, `goto_definition`,
  `find_usages`, `get_document_symbols`) e tre che modificano
  (`write_file`, `replace_text`, `rename`), governati da una modalità per
  progetto (Non brave, Auto, Brave) scelta nella barra di stato, con card di
  conferma in coda. Le modifiche sotto `.git` o fuori dalla radice sono sempre
  vietate. `run_command` non è pubblicato né invocabile su alcuna piattaforma;
  resta disattivato in attesa delle decisioni e prove d'isolamento richieste.
- Completamento e hover nell'editor quando supportati dal language server,
  diagnostica nel pannello Problemi e nel margine.
- Gli errori mostrati nell'app vengono conservati in `errors.log`, con tre
  generazioni a rotazione e i percorsi del profilo utente anonimizzati.
- Il pannello Git consente push, cambio branch, pull, stash apply/pop e merge
  locale interattivo. I conflitti del merge restano nell'index Git; la barra
  abilita continua solo dopo la risoluzione e chiede conferma prima di abort.
- Le azioni che richiedono `git` sono nascoste quando l'eseguibile non è
  disponibile; il pannello ricontrolla la capacità prima di inviare l'azione.
- `stash drop` acquisisce lock esclusivi per il reflog e per `refs/stash`,
  concatena le OID e pubblica i file tramite rename; rifiuta lock concorrenti
  senza seguire symlink.
- Salvataggi atomici per i file dell'editor e quelli chiusi modificati da
  «Sostituisci tutto», con test di scritture interrotte e cleanup. Su Windows
  il temporaneo nasce con DACL protetta e il commit conserva la DACL
  dell'originale; negli errori parziali si segnala il temporaneo recuperabile.
- «Annulla fetch», con annullamento cooperativo del worker Git e scarto delle
  risposte del progetto precedente.
- `ROADMAP.md` con l'ordine di completamento e i criteri di completamento.
- Multicursore: selezioni multiple conservate nel documento e nello storico,
  edit atomici e ricerca per ogni selezione, da mouse, tastiera e menu.
  Incolla ripete il testo in ogni selezione; Esc chiude prima il popup e poi
  conserva soltanto il cursore principale.
- Folding dal margine per Rust e per le altre diciassette lingue.
- Azioni rapide del language server con elenco delle modifiche e loro
  applicazione; formattazione della selezione e del documento dal menu;
  firma della funzione e parametro attivo quando il server li dichiara.
- Albero del progetto: crea file e cartelle, rinomina seguendo i documenti
  aperti, elimina file e cartelle portandoli nel Cestino senza perdere i
  documenti aperti; menu contestuali su schede e albero, con «Chiudi le
  altre».
- Pannello Jujutsu, opzionale, per i workspace che contengono `.jj`:
  cambiamenti, stato, descrivi, nuovo cambiamento e push. Senza `.jj` il
  pannello è nascosto; senza `jj` nel `PATH` spiega come installarlo e non
  mostra pulsanti.
- All'uscita la raccolta dell'ultimo processo `jj` avviene fuori dal thread
  dell'interfaccia, senza bloccarlo; una raccolta non confermata lascia
  l'avviso di chiusura già esistente.
- MCP su Linux: socket privato in una cartella di scoperta versionata,
  controllo del peer, `--instance` per scegliere l'istanza nel ponte
  `lume --mcp-stdio` e identità dell'istanza copiabile dalla riga
  Estensioni. I record di scoperta lasciati da un'istanza caduta vengono
  rimossi quando il loro socket rifiuta la connessione o non esiste più; i
  record illeggibili vengono saltati con un avviso; un quinto client riceve
  un rifiuto esplicito «occupato» invece di restare appeso.
- Il vault SQL su Linux usa Secret Service tramite libdbus compilata dal
  sorgente (`dbus-secret-service`); zbus 5 esce dalla catena delle
  dipendenze, compresa quella di iced.
- Release: nome `lume-<versione>-<target>-<sha12>`, `MANIFEST.json` con
  SHA-256 e byte di ogni file, `.sha256` accanto all'artefatto e rifiuto di
  sovrascrivere un output esistente; il workflow pubblica dal commit esatto
  del tag.
- I file dell'editor si leggono fuori dal thread dell'interfaccia, con un
  tetto di 32 MiB.

### Corretto

- Windows: percorsi confrontati nella stessa forma (`D:\...` e non la forma
  verbatim `\\?\D:\...`): «Sostituisci nei file» scrive di nuovo i file
  chiusi, le note del progetto sono riconosciute e un file aperto da Esplora
  risorse non cambia più la cartella del progetto.
- Percorsi sotto una cartella antenata che è un link simbolico: sostituzione
  nei file, note, confine MCP, Cestino e watcher confrontano forme canoniche.
- Windows: il ridisegno parziale dei numeri di riga non lascia più pixel
  vecchi durante lo scorrimento.
- MCP su Windows: la pipe accetta di nuovo client dopo la chiusura di uno di
  loro; se non può, lo dichiara invece di restare muta.
- macOS: i comandi `git` riusciti non risultano più falliti (preparazione
  per una release macOS successiva).
- Caricamento degli Appunti: preferenze e scansioni superate vengono
  scartate prima degli effetti, risultati ed errori sono autenticati, e il
  cambio cartella annulla il «Nuovo appunto» pendente con un avviso.
- Salvataggio e letture durante le operazioni Git non si perdono: gli errori
  Git e la cronologia dei salvataggi restano visibili.
- Cerca rilascia la vista chiusa e la chiusura del piano dei riquadri non
  lascia overlay orfani.
- Salva, Salva con nome e «Sostituisci nei file» su Unix (provato su Linux): dopo il rename si sincronizza anche la cartella, così un salvataggio confermato sopravvive a una perdita di corrente; se la sincronizzazione fallisce il file resta salvato e la status bar lo segnala. Lo stesso per il salvataggio, la rinomina, la creazione e la copia degli appunti e per «Rinomina» dall'albero del progetto.
- Ctrl+S e subito dopo la chiusura della finestra: finito il salvataggio, la chiusura procede da sola invece di lasciare aperto «0 file con modifiche non salvate».
- I salvataggi rifiutano i file non scrivibili senza aggirare le protezioni
  attraverso la sostituzione nella directory; su Unix non richiedono lettura
  aggiuntiva per un documento già aperto e rimasto scrivibile.
- Il commit Windows chiude l'handle del temporaneo prima di `ReplaceFileW`,
  mantenendo la gestione del nome e il recupero negli errori parziali.
- Un annullamento tardivo del fetch non nasconde gli errori di connessione,
  preparazione o ricezione non causati dall'interruzione.
- Le URI LSP su Unix non aggiungono una quarta barra e non trasformano i
  backslash nei nomi dei file in separatori.
- I test LSP e Appunti distinguono percorsi e disponibilità del Cestino per
  piattaforma; le fixture ZIP dei language server dichiarano il formato anche
  dove il catalogo usa gzip. La guardia di avvio LSP su Unix non usa `expect`.
- La documentazione Oracle riporta TLS di default; quella Git distingue la
  CLI collegata alla UI e dichiara i limiti di contenimento di processi helper
  su Unix e Windows.

### Modificato
- Gli overlay modali (dialoghi, menu, riquadri) condividono una sola politica
  di visibilità, attesa e precedenza: un riquadro alla volta, la chiusura
  attende i worker avviati e non lascia riquadri orfani.
- Le scritture — salvataggi, salvataggi con nome, rinomine, Cestino e Git —
  passano da un coordinatore unico con esiti autenticati: una sola
  scrittura per file alla volta e nessun commit se l'autorizzazione è
  scaduta, negata o revocata.
- Il coordinamento del ciclo di vita delle funzionalità vive in `app/funzionalita.rs`; politiche, risorse, conferme e subscription mantengono il comportamento precedente.
- Il coordinatore delle richieste dei tool vive in `app/richieste_tool.rs`, con worklist e dispatch invariati; stato e messaggi restano in `app.rs`.
- **Compatibilità della Cronologia:** le nuove azioni «Agente: scrittura» e
  «Agente: comando» non sono riconosciute dalle build precedenti e possono
  impedire la deserializzazione dello snapshot di sessione. Il downgrade non è
  compatibile finché non viene aggiunta una migrazione o una rappresentazione
  retrocompatibile.

### Cambiato
- I tetti dell'undo rientrano nel budget di memoria: 48 MiB in tutto e 16 MiB
  per documento (prima 256 e 64 MiB).
- La pubblicazione delle release dipende dalla CI sul commit distribuito:
  fmt, Clippy, test rigidi, MSRV, licenze e supply chain.
- Il rilevamento del tema di sistema di iced non è più compilato, perché
  portava zbus 5 (non mantenuto) nella catena delle dipendenze: il tema è
  quello scelto dall'utente.

### Sicurezza
- rustls 0.23.46 chiude quattro advisory; aggiornati anche `zip` e
  `keyring-core`.
- Attribuzioni di terze parti rigenerate sul lock corrente; `cargo-deny` e
  `cargo-about` sono gate della release.

## [0.3.0] — 2026-09-30

### Aggiunto
- `lume.exe --prova-crash` per verificare il log dei crash.
- Icona e informazioni di versione nell'eseguibile.
- CI su GitHub Actions per Windows, con controllo delle licenze, cargo-deny e MSRV.

### Corretto
- «Sostituisci tutto» dichiara quando le occorrenze superano 5000.
- L'evidenziazione di Trova non resta sotto un altro riquadro.
- Simboli e Struttura seguono il gruppo di editor col fuoco.
- «Informazioni su Lume» resta in barra di stato finché non la si chiude, con «Copia».
- Il log dei crash e il binario di release non contengono la cartella utente.
- `THIRD_PARTY_LICENSES.md` riporta i titolari veri; lo zip contiene il LICENSE di aws-lc-sys.

### Cambiato
- MSRV 1.89.
- Budget del README sulle risorse a runtime (RAM in private bytes, thread, processi figli); il peso del binario non è un vincolo.
