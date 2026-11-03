# Server MCP di Lume

La feature Cargo opzionale `mcp` espone a un client MCP locale l'editor, i file,
le diagnostiche, i language server e Git del progetto aperto. Può anche
modificare file e rinominare simboli. La modalità del progetto decide quali
modifiche richiedono conferma: un progetto nuovo parte in **Non brave**, dove
ogni modifica chiede conferma in Lume. Non espone comandi arbitrari né query SQL.

Il trasporto è locale: una named pipe di Windows oppure un socket Unix dentro
una cartella privata su Linux e macOS. Il ponte è sempre
`lume --mcp-stdio` (`lume.exe` su Windows). Su Linux il trasporto Unix è
supportato e verificato dai test; su macOS usa lo stesso adattatore ma non è
ancora qualificato su host nativo. La feature non è inclusa nel build
predefinito:

```sh
cargo build --release -p lume-app --features mcp
```

La funzione è sperimentale. I test automatizzati Linux coprono il protocollo,
il socket, il ponte, la coda delle conferme e le integrazioni dell'app; non
sostituiscono la prova end-to-end con un client MCP esterno né la verifica
nativa degli ACL Windows, della cartella dati e del costo runtime.

## Avvio e arresto

1. Apri una cartella in Lume.
2. Apri **Estensioni**, quindi **Server MCP** → **Avvia**.
3. Controlla la radice canonica mostrata e conferma **Avvia**. Esc,
   **Annulla** e il clic fuori annullano la richiesta.

Il server non parte all'avvio di Lume o all'apertura del progetto. **Ferma**,
il cambio di cartella e la chiusura di Lume interrompono il server. La barra di
stato mostra la modalità e il numero di conferme pendenti solo mentre il server
è acceso; la modalità è salvata per progetto.

## Collegare un client

I pulsanti di copia compaiono mentre il workspace MCP è vivo. Il percorso
dell'eseguibile è quello del processo in esecuzione. Le configurazioni non
contengono il token, il percorso del socket o il nome della pipe, perché
cambiano a ogni Avvia.

- **Copia per Claude Code** genera un comando `claude mcp add` per
  `lume --mcp-stdio`.
- **Copia per Codex** genera un comando `codex mcp add` per lo stesso ponte;
  la forma esatta va verificata con la versione supportata del client.
- **Copia JSON (Cursor, Claude Desktop)** genera una voce `mcpServers`.
  Claude Desktop non seleziona necessariamente il progetto dalla directory
  corrente: con più istanze, aggiungi `--project` e il percorso del progetto
  agli argomenti.
- **Copia ID istanza** compare solo su Unix e copia l'identità pubblica
  dell'accensione corrente, da usare con `--instance <id>`.

Il ponte sceglie l'istanza in quest'ordine: `--instance <id>`, poi
`--project <cartella>`, poi la directory corrente, poi l'unica istanza accesa.
Se più istanze corrispondono allo stesso progetto, senza `--instance` il
ponte rifiuta l'ambiguità invece di scegliere l'ultima letta. L'ID è la parte
`<pid>-<nonce>` del nome del socket mostrata in **Estensioni**; cambia a ogni
Avvia, quindi una configurazione che lo fissa va aggiornata dopo il riavvio.
Senza ID, un progetto univoco non richiede aggiornamenti della configurazione.

Su Unix il socket è `<cartella dati>/mcp-unix-v1/<id>.sock`; su Windows la
pipe resta `\\.\pipe\lume-mcp-<pid>-<nonce>`. Il ponte non cancella i record
Unix in base alla sola esistenza di un PID. Un record valido viene potato
solo con un segnale certo: il suo socket esiste con lo stesso inode/dev e la
`connect` restituisce `ECONNREFUSED` (nessuno in ascolto), oppure il socket
del record è sparito. `ECONNREFUSED` vale come segnale certo su Linux: su
BSD/macOS la stessa `connect` può restituirla anche con un listener vivo e il
backlog pieno, quindi lì la potatura non scatta, il tentativo si ripete e la
qualifica macOS del trasporto resta aperta. La potatura ricontrolla identità,
token e inode/dev prima di cancellare; avviene quando il ponte prova un
candidato e una volta sola all'accensione del server, senza timer. Un record
illeggibile o non valido viene saltato e segnalato su stderr dal ponte con il
percorso del file, senza cancellarlo: potrebbe essere letto mentre viene
scritto. Un record che non descrive più il suo endpoint (inode/dev o PID del
peer diversi) viene escluso dalla scelta con lo stesso avviso, senza
impedire di scegliere un'altra istanza sana e senza essere cancellato.

La capacità di un processo GUI Windows di ricevere gli handle standard del
client non è ancora stata provata su Windows. Il codice usa il ponte comune
`lume --mcp-stdio`; se la prova o i client reali falliscono, servirà il
binario console separato `lume-mcp-bridge.exe`.

## Strumenti

La lista dinamica può contenere fino a 18 tool. I tool Git compaiono solo se Git
è attivo; i tool di diagnostica e LSP solo se il language server attivo
dichiara la capability richiesta. `run_command` non compare su alcuna
piattaforma; la sua invocazione viene rifiutata senza chiedere consenso.

| Tool | Comportamento |
|---|---|
| `list_open_files` | Elenca file aperti, versione, modifiche non salvate e scheda attiva. |
| `get_active_file` | Restituisce file attivo, selezione e testo selezionato. |
| `read_file` | Legge il buffer aperto, anche sporco, oppure il disco; massimo 1 MiB. |
| `list_files` | Elenca file rispettando `.gitignore`, esclude `.git`; massimo 500 di default, 5.000 al massimo. |
| `search_text` | Cerca testo o regex sul disco, non nei buffer non salvati; massimo 2.000 occorrenze. |
| `get_diagnostics` | Legge le diagnostiche pubblicate dai language server attivi. |
| `get_hover` | Restituisce documentazione e tipo del simbolo a `path`, `line`, `column`. |
| `goto_definition` | Restituisce le definizioni del simbolo nel punto indicato. |
| `find_usages` | Restituisce i riferimenti, fino a 200, con `total` e `truncated`. |
| `get_document_symbols` | Restituisce i simboli del file come struttura appiattita con `depth`, fino a 1.000. |
| `write_file` | Scrive il contenuto intero di un file del progetto; massimo 1 MiB. **Modifica.** |
| `replace_text` | Sostituisce testo in un file; `old_text` deve essere unico, salvo `replace_all`. **Modifica.** |
| `rename` | Rinomina un simbolo tramite language server e aggiorna i buffer. **Modifica.** |
| `git_status` | Stato Git dei file sotto la radice. |
| `git_diff_file` | Diff unificato di un file. |
| `git_log` | Log dei commit, fino a 200. |
| `git_blame` | Autore, data e commit per riga. |
| `open_in_editor` | Apre un file nell'editor, facoltativamente alla riga indicata. |

### Strumenti del language server

I tool LSP richiedono che il file sia già aperto nell'editor; una richiesta di
sola lettura non apre schede né sposta il cursore. `line` e `column` sono
1-based e `column` conta unità UTF-16, come LSP. Lume invia le modifiche
pendenti al server e rifiuta risposte diventate obsolete mentre il server
risponde. Le posizioni fuori dal testo o nel mezzo di una coppia surrogata
vengono rifiutate. Limiti: hover 16.384 caratteri, definizioni/riferimenti 200,
simboli 1.000.

Esempi abbreviati di risposta:

```json
{"found":true,"contents":"…","truncated":false}
{"definitions":[{"path":"src/lib.rs","line":3,"column":8,"end_line":3,"end_column":12}]}
{"references":[{"path":"src/lib.rs","line":3,"column":8,"end_line":3,"end_column":12}],"total":1,"truncated":false}
{"symbols":[{"name":"main","kind":"function","depth":0,"line":1,"column":4,"end_line":9}],"total":1,"truncated":false}
```

Un risultato LSP fuori dalla radice non espone un percorso locale: contiene
`uri` e `outside_root: true`. `get_hover` può rispondere `{"found":false}` se
non c'è documentazione.

### Strumenti che modificano

Esempi abbreviati delle risposte implementate:

```json
{"path":"src/nuovo.rs","bytes":120,"created":true,"open_in_editor":false}
{"path":"src/lib.rs","replacements":2,"bytes":3400,"open_in_editor":true}
{"renamed":true,"occurrences":7,"files":[{"path":"src/lib.rs","occurrences":7}],"files_total":1,"truncated":false,"saved":false}
```

- `write_file` accetta testo UTF-8 fino a 1 MiB. La cartella madre deve già
  esistere. Rifiuta cartelle, link simbolici/junction come bersaglio e buffer
  aperti con modifiche non salvate. Aggiorna eventuali buffer puliti.
- `replace_text` rifiuta `old_text` vuoto e, senza `replace_all`, richiede una
  sola occorrenza. Conserva CRLF; rifiuta buffer sporchi e contenuti oltre 1
  MiB. Aggiorna eventuali buffer puliti.
- `rename` usa la proposta del language server e mette le modifiche nei buffer,
  senza salvare (`saved: false`). I file chiusi coinvolti diventano schede
  sporche. L'utente decide se salvarli.

## Modalità

La modalità è per progetto e resta salvata tra i riavvii. Si sceglie nella
barra di stato, visibile solo con server acceso. Un valore mancante o illeggibile
ricade su **Non brave**.

| Modalità | Comportamento |
|---|---|
| Non brave | Ogni modifica chiede conferma in Lume. |
| Auto | Le modifiche ammesse dentro la radice passano senza conferma. |
| Brave | Le modifiche ammesse passano senza conferma. |

Le modifiche fuori dalla radice o sotto `.git` sono rifiutate in tutte le
modalità, senza card. `run_command` resta indisponibile anche in modalità Brave.

Le conferme appaiono come card in Lume con percorso o file coinvolti.
**Consenti** esegue l'azione; **Nega** o Esc rifiuta la richiesta. Le richieste
multiple sono gestite in ordine FIFO. La card mostra la posizione nella coda;
la barra di stato mostra quante sono pendenti e permette di portare la card in
primo piano. Una richiesta scade dopo 300 secondi. Se il client si scollega,
la card sparisce e l'azione non avviene. Passare a una modalità più permissiva
fa partire le richieste in coda che ora sono consentite; passare a una più
cauta non revoca azioni già consentite. Ferma, cambio progetto e chiusura
rifiutano le richieste pendenti.

## Sicurezza e limiti

- Il server usa una named pipe locale `\\.\pipe\lume-mcp-<pid>-<nonce>` su
  Windows e un socket filesystem su Unix; rifiuta connessioni remote e non
  apre una porta TCP.
- Su Unix la cartella `<cartella dati>/mcp-unix-v1/` deve essere reale,
  dell'UID effettivo e `0700`; la catena dalla radice al parent non può
  contenere link simbolici né essere scrivibile da gruppo o altri. Se esiste
  con protezioni diverse l'avvio fallisce senza `chmod` impliciti. Socket e
  record sono `<id>.sock` e `<id>.json`, entrambi `0600`. Il percorso del
  socket oltre il limite nativo viene rifiutato; in quel caso serve una
  cartella dati più breve.
- Su Unix server e ponte leggono l'UID del peer dal kernel
  (`UnixStream::peer_cred`); il ponte verifica anche il PID del peer contro il
  record prima di collegarsi e rivalida record e credenziali sulla connessione
  finale. Una selezione oltre la deadline di tre secondi, una coda occupata o
  un record cambiato falliscono con un errore esplicito e senza ripiegare su
  un'altra istanza.
- Ogni avvio crea un token casuale di 256 bit. Resta in memoria e nel record di
  discovery nella cartella dati dell'utente; il confronto è a tempo costante.
  Se `LUME_DATA_DIR` è leggibile da altri account, il token può essere esposto.
  La scoperta Unix è separata da `mcp/`: un vecchio ponte Windows non cancella
  i record nuovi.
- La scoperta Unix accetta fino a 64 istanze attive e 16 KiB per record.
  Una scansione esamina al massimo 256 file di record e pota i record
  stantii prima di contare: le voci oltre il tetto vengono saltate con un
  avviso, non fanno più fallire la selezione. Il tetto di quattro client
  vale per le connessioni che hanno superato il preambolo: le prove della
  scoperta e le connessioni che non hanno ancora mandato il preambolo non lo
  consumano (al massimo 16 in attesa, con la deadline del preambolo). Oltre
  quattro client autenticati, il server legge il preambolo della nuova
  connessione e solo allora risponde con un errore JSON-RPC esplicito
  (`-32000`, «tutte le connessioni MCP di questa istanza sono occupate»),
  poi chiude: nessun byte esce prima del preambolo. Il ponte inoltra
  l'errore al client ed esce con il codice di servizio non disponibile (69)
  e lo stesso messaggio; una chiusura del server prima della prima risposta
  (token rifiutato, preambolo scaduto, Ferma) è riportata come tale e non
  come «occupato». La selezione fallita riporta la cartella
  `<cartella dati>/mcp-unix-v1/` e il rimedio da seguire.
- Traversal e percorsi fuori dalla radice, link che escono dalla radice e
  percorsi sotto `.git` sono rifiutati per le scritture e i tool confinati.
  Sono rifiutati anche i nomi `.git` equivalenti secondo le regole Windows.
- **Comandi arbitrari non disponibili.** `run_command` non compare in
  `tools/list`; `tools/call` lo rifiuta senza inoltrare una richiesta
  all'app. Resta disattivato finché non saranno definite e provate le decisioni
  di isolamento contro accesso al vault, rete DB e controllo UI/IPC.
- I contenuti dei file non vengono registrati nei log o nella Cronologia; la
  Cronologia conserva l'etichetta dell'azione e i percorsi coinvolti.
- Le verifiche del percorso non difendono da hard link creati da un soggetto
  che ha già accesso alla cartella, né da tutte le corse TOCTOU fra controllo
  e scrittura. Per `rename`, language server e app possono leggere file fuori
  radice per calcolare la proposta, ma gli edit fuori radice vengono rifiutati
  prima dell'applicazione.
- DACL della pipe, ACL della cartella dati Windows e il rifiuto di un peer
  Unix con un UID diverso non sono stati verificati con un secondo account
  nativo: servono host e account reali, non un UID simulato. Non è disponibile
  una misura runtime comparabile del server acceso e spento.
- La Cronologia salva le nuove azioni «Agente: scrittura» e «Agente: comando»
  come varianti serializzate. Una build precedente non le conosce e può non
  riuscire a deserializzare lo snapshot di sessione; il downgrade non è
  compatibile finché non viene aggiunta una migrazione o una rappresentazione
  retrocompatibile.

## Fuori da questa versione

Anteprima di modifica `edit_file`, denylist di lettura (`.env`, `*.pem`),
trasporto HTTP, risorse dal watcher, cataloghi e SQL in sola lettura, tool LSP
su file chiusi, salvataggio automatico dei file toccati da `rename` e una voce
Cronologia per la rinomina non sono inclusi. La qualifica macOS del trasporto
Unix, la prova con client MCP esterni e le misure native di costo restano
lavoro separato.
