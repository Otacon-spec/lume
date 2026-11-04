# Lume 0.4.0 alpha — note di rilascio

Lume è un IDE desktop leggero, scritto in Rust: editor con evidenziazione
tree-sitter, language server gestiti dall'app, terminale integrato, Git,
ricerca nel progetto, console SQL e Appunti.

La 0.4.0 è la **prima release pubblica** e ha lo stato di **alpha qualificata
su Linux x86_64**. Le funzioni di base — aprire, modificare, salvare,
chiudere, ripristinare la sessione, menu contestuale, scritture Git, watcher
dei file e Appunti — sono state provate su Linux con test, finestre vere sotto
Xvfb e file su disco. Quello che segue non è stato provato e non va
considerato garantito.

## Che cosa c'è

- Editor con evidenziazione per Rust, C, C++, C#, Java, Kotlin, Python,
  JavaScript, TypeScript, JSON, YAML, TOML, Markdown, HTML, CSS, XML e SQL;
  due gruppi di schede, Trova e Sostituisci, multicursore, folding dal
  margine.
- Language server per rust-analyzer, tombi (TOML) e rumdl (Markdown):
  completamento, hover, firma della funzione, diagnostica nel pannello
  Problemi e nel margine, vai alla definizione, usi, rinomina, struttura,
  azioni rapide e formattazione.
- Terminale integrato con una PTY reale.
- Git: stato, diff, stage per hunk, commit, log, branch, fetch, pull, push,
  blame, stash e merge locale con conflitti. Push, cambio branch, stash
  apply/pop e merge con conflitti usano il programma `git` del sistema e
  spariscono dall'interfaccia quando `git` non c'è.
- Cerca ovunque (file, azioni, simboli) e ricerca nel testo del progetto.
- Console SQL per Azure SQL / SQL Server e Oracle, con le password nel
  magazzino credenziali del sistema operativo.
- Appunti: note Markdown con collegamenti al codice.
- Pannello Jujutsu opzionale per i workspace che contengono `.jj`.
- Temi colore e temi icone; errori mostrati dall'app conservati in
  `errors.log`, con i percorsi del profilo utente anonimizzati.
- Server MCP sperimentale, non compilato per impostazione predefinita (vedi
  «Limiti noti»).

## Installare su Linux

L'artefatto è un archivio, non un installer:

- nome `lume-0.4.0-linux-x86_64-<sha12>.tar.gz`, dove `<sha12>` sono i primi
  dodici caratteri del commit da cui è stato costruito;
- accanto all'archivio ci sono il file `.sha256` e una copia di
  `MANIFEST.json`, che elenca per ogni file dell'archivio la dimensione e lo
  SHA-256 e riporta il commit completo, la versione e la toolchain usata.

Per verificare e avviare:

```
sha256sum -c lume-0.4.0-linux-x86_64-<sha12>.tar.gz.sha256
tar -xzf lume-0.4.0-linux-x86_64-<sha12>.tar.gz
cd lume-0.4.0-linux-x86_64-<sha12>
./avvia-lume.sh
```

`installa-launcher.sh`, incluso nell'archivio, registra la copia estratta nel
menu applicazioni dell'utente. L'archivio non contiene i language server:
Lume li scarica al primo uso e ne verifica l'impronta SHA-256. Per compilare
dai sorgenti servono Rust stabile, edition 2024, e un compilatore C per la
libdbus compilata dal sorgente (su Ubuntu `build-essential`).

## Limiti noti di questa alpha

### Piattaforme

- **Pacchetti: Linux x86_64 e Windows x64.** Su Windows l'intera suite di
  test passa nella CI pubblica, ma l'uso reale non è collaudato sul codice di
  questa release: chooser dei file, ACL, Cestino, `rename`, SMB, WSL e
  perdita di alimentazione sono stati provati solo su un binario del 23
  ottobre. Il pacchetto Windows non è firmato.
- **macOS non è in questa release.** La prima esecuzione in CI ha trovato,
  fra l'altro, che i language server non partono; il lavoro continua per una
  versione successiva.
- **Installazione su un host pulito** non è stata provata.

### Salvataggio

- Su Linux il replace atomico non conserva le ACL POSIX nominate né gli
  attributi estesi, e non è garantito contro una perdita di alimentazione o
  contro uno scrittore esterno che modifichi il file fra il controllo e il
  rename. La sostituzione è atomica per file, non è una transazione
  multi-file.

### Database

- I driver SQL Server/Azure SQL e Oracle sono compilati e coperti da test di
  protocollo, ma non sono stati eseguiti contro un database reale: TLS, tipi
  LOB, streaming, cancellazione e privilegi read-only restano non
  qualificati. Il limite di inattività e la cancellazione Oracle sono
  verificati solo dai test.

### MCP

- Il server MCP è **sperimentale e fuori dal default**: si ottiene compilando
  con la feature Cargo `mcp`. Non è provato su Windows né su macOS: il
  trasporto qualificato è quello Unix, su Linux.
- Non è stato collegato alcun client MCP esterno reale: il trasporto è
  provato con socket veri e con il ponte `lume --mcp-stdio`.
- La pulizia dei record di scoperta abbandonati si basa sul rifiuto della
  connessione, che è un segnale certo solo su Linux: su macOS e BSD quei
  record non vengono rimossi in automatico (la cartella è
  `mcp-unix-v1` nella cartella dati di Lume).
- I tool SQL non esistono e `run_command` non è pubblicato né eseguibile.
- Il confine delle scritture (`write_file`, `replace_text`, `rename`) non è
  provato contro symlink, hard link o alias creati da un altro processo fra
  il controllo e la scrittura.

### Editor e language server

- È qualificato un solo server reale, rust-analyzer: un secondo server
  distinto (Java, Python, C#) non è provato.
- Il routing usa un server per documento: due server non si combinano sullo
  stesso file.
- Le prove native di completamento, definizione, diagnostica e rinomina con
  un server reale sono precedenti a questa release e non sono state
  rieseguite sul binario distribuito.

### Git

- Fetch, pull, push, merge e stash sono provati su repository locali:
  autenticazione, TLS, credential helper e SSH verso un remote reale non sono
  stati esercitati.
- Il contenimento dei processi `git` è provato su Linux; su Windows il Job
  Object è compilato ma non provato a runtime.

### Jujutsu

- Il pannello è opzionale e richiede `jj` nel `PATH`. Su Linux è verificata la
  raccolta dell'ultimo processo all'uscita. Caricamento, azioni, interfaccia,
  cambio di radice e quarantena non sono qualificati in questa release.

### Sicurezza e firma

- **Gli artefatti non sono firmati né notarizzati**: manca il certificato di
  firma. Il sistema operativo può mostrare un avviso al primo avvio.
- La libdbus usata dal vault su Linux è compilata dal sorgente e inclusa
  nell'eseguibile (1.14.4): una sua correzione di sicurezza arriva solo
  ricompilando Lume.
- Il vault è stato provato con gnome-keyring. KWallet parla la stessa API
  Secret Service ma non è stato provato.
- La CI su GitHub non è mai stata eseguita: i workflow sono validati solo
  come YAML.

### Prestazioni

- I budget di avvio (< 100 ms), digitazione (< 8 ms), 60 fps e riposo
  (< 150 MiB di private bytes) sono definiti su Windows release e non sono
  misurati su questa release. Le misure Linux restano diagnostiche.

### DAP e SSH

- **Non sono implementati.** Esistono solo il design confermato e nessun
  runtime: in Lume non c'è un client di debug né lo sviluppo remoto.

## Dati e aggiornamenti

- I formati dei dati possono cambiare fra una versione alpha e l'altra: tieni
  i tuoi file sotto controllo di versione.
- Il database dell'app è alla versione 5: una versione più vecchia di Lume non
  lo apre più.
- Lume non si aggiorna da solo e non contatta la rete all'avvio. «Controlla
  aggiornamenti» interroga le release su GitHub solo quando lo chiedi.
