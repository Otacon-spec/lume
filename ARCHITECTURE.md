# Architettura di Lume

Una mappa per chi entra nel codice. I dettagli stanno nei doc comment dei
file citati.

## I crate

| Crate | Cosa fa |
|---|---|
| `lume-app` | l'applicazione Iced: stato (`Lume` in `app.rs`), messaggi, viste, menù, tool concreti |
| `lume-core` | documenti su rope, undo, selezioni, ricerca, watcher; nessuna dipendenza UI; posizioni in char index |
| `lume-ui` | widget riusabili, il trait `Tool` e il `Registry`, l'editor, gli stili |
| `lume-theme` | token e temi (TOML embedded) |
| `lume-syntax` | evidenziazione incrementale con tree-sitter |
| `lume-lsp` | client LSP generico (processo, sessione, attore) |
| `lume-mcp` | protocollo MCP, trasporto locale (named pipe o socket Unix) e catalogo dei tool |
| `lume-language-servers` | catalogo, installazione e bundle dei language server |
| `lume-git` | Operazioni Git native con gitoxide e `GitCli` esplicita per push, cambio branch, stash apply/pop e merge locale con conflitti; il pannello Commit coordina worker, annullamento e refresh |
| `lume-jj` | integrazione Jujutsu opzionale tramite il comando `jj` |
| `lume-sql` | Azure SQL e Oracle, vault dei segreti |
| `lume-terminal` | PTY e emulazione VT |
| `lume-store` | persistenza SQLite (sessione, impostazioni, profili senza segreti) |
| `lume-trust` | tipo della fiducia nei workspace, usato dal controllo di `lume-lsp` |
| `lume-icons` | temi di icone per i file |
| `lume-notes` | Appunti |
| `lume-testkit` | strumenti richiesti dai test d'integrazione, usato solo come dev-dependency |

## Dove sta il codice di `Lume`

`Lume` (lo stato) e `Message` (gli eventi) stanno in
`crates/lume-app/src/app.rs`, insieme a `update`, `subscription`, alle
notifiche, ai menù e alle funzionalità; delega le `ToolRequest` al relativo coordinatore:
lì si smista, non si lavora. Il lavoro di ogni dominio sta in un file figlio
di `app/`, con un solo blocco `impl Lume`:

| File | Che cosa c'è |
|---|---|
| `app/ingresso.rs` | classificazione esaustiva di provenienza e persistenza dei messaggi |
| `app/riquadri.rs` | percorso privato di apertura/chiusura, modelli modali e sospensione delle risorse nascoste |
| `app/notes.rs` | Appunti: apertura, creazione, eliminazione e cartella |
| `app/sql.rs` | SQL: connessioni, console, query, vault e ibernazione |
| `app/lsp.rs` | language server, definizioni, riferimenti, rinomina, contorno e problemi |
| `app/git.rs` | Git: stato, diff, blame, stash e cronologia |
| `app/ricerca.rs` | trova nel file, ricerca globale, vai a riga e salti |
| `app/richieste_tool.rs` | worklist delle ToolRequest, dispatch ai domini e persistenza dello stato dei tool |
| `app/funzionalita.rs` | ciclo di vita delle funzionalità: comandi, conferme, risorse e riconciliazione dell’inattività |
| `app/sessione.rs` | sessione, progetto e file system, file recenti, chiusura e uscita |
| `app/editor.rs` | comandi dell'editor, sintassi, salvataggio e schede |
| `app/viste.rs` | vista centrale, colonne, pannelli e dock |
| `app/tests.rs`, `app/diff_tests.rs` | test spostati da `app.rs`; altri test app-specifici restano nei moduli dedicati |

I metodi dei file figli sono `pub(super)` quando servono al padre o ai moduli
fratelli; quelli già pubblici mantengono la loro visibilità. Un metodo nuovo
va nel file del dominio di cui usa lo stato; se non ne ha uno, in `app.rs`.
Il file figlio apre con `use super::*;`, quindi vede i campi privati di `Lume`
senza dichiararli di nuovo.

## Tool, Registry, ToolRequest

Ogni icona delle barre laterali è un `Tool` autonomo: icona, titolo,
pannello, stato ed eventi propri (`crates/lume-ui/src/tool.rs`). Le barre e
l'app non conoscono i tool concreti: li leggono dal `Registry`. Un tool non
tocca lo stato dell'app: dopo un evento restituisce delle `ToolRequest`
(apri un file, avvia una ricerca, chiudi il pannello…) che l'app esegue in
`drain_tool_requests`, in coda a `Lume::update`; `app/richieste_tool.rs::apply_tool_requests` ne
esegue la worklist, incluse quelle emesse durante la chiusura di un tool.

## Il rilascio delle risorse

Il budget vieta lavoro da inattivo, quindi ogni risorsa ha un posto preciso
in cui muore.

- **Un tool** rilascia le sue cache in `Tool::on_visibility_changed(false)`:
  è l'unico punto in cui viene avvisato della chiusura (`update` non viene
  mai chiamato alla chiusura). Ogni percorso che chiude un pannello passa
  dai setter del `Registry`, mai da un'assegnazione diretta di
  `active_panel`/`active_dock`.
- **Cache di vista dell'app** (Appunti): l'elenco dei riepiloghi e le note
  caricate stanno in `NotesState`, non nel tool. Alla chiusura il tool
  svuota la sua copia e chiede `ToolRequest::NotesUnload`; l'app scarica
  `loaded` e `summaries` e di ogni scheda ancora aperta tiene solo la
  guardia di salvataggio (intestazione, percorso, identità del file letto,
  sola lettura: niente corpo). Alla riapertura una riscansione rilegge i
  riepiloghi; finché non arriva, il pannello dice «Caricamento…».
- **L'app** possiede le risorse pesanti (processi, socket, thread) e le
  rilascia smettendo di produrne la subscription. Due idiomi di Iced:
  1. la subscription esiste solo finché lo stato la chiede (il risveglio
     del terminale);
  2. la risorsa nasce dentro lo stream di `Subscription::run_with(chiave, …)`
     e ci resta (`fs_subscription`, i language server): quando la chiave
     cambia o il ramo non viene più restituito, Iced chiude lo stream e il
     `Drop` della risorsa fa il resto.

## Funzionalità: abilitata e viva

`crates/lume-app/src/features.rs` tiene due assi da non confondere:
`enabled` (deciso nel pannello Estensioni, persistito) e *vivo* (tiene
risorse in questo momento; non si memorizza, si deriva da chi le possiede).
Il SQL si iberna dopo 5 minuti senza viste aperte; i language server si
fermano solo a comando.

## Tabella d'inattività

`crates/lume-app/src/idle.rs` è l'unico posto in cui si decide quando una
funzionalità diventa **Dormiente**. `FeatureSpec::idle` dice per quanto:
`Some(durata)` per il Database (`sql_mode::IDLE_TIMEOUT`, cinque minuti),
`None` per tutte le altre.

**Cinque stati.** La tabella distingue solo gli ultimi tre; i primi due li
decidono il build e l'utente.

| Stato | Significato | Chi lo decide |
|---|---|---|
| Assente | non compilata (feature Cargo `sql` spenta: niente riga Database nelle Estensioni) | il build |
| Spenta | disabilitata nel pannello Estensioni, persistita | `Registry::disabled_features` |
| Ferma | abilitata, nessuna risorsa viva | `feature_live_count == 0` |
| Attiva | risorsa viva **e** almeno una vista in uso | `feature_is_in_use` |
| Dormiente | risorsa viva, nessuna vista in uso, timer armato | `IdleTable` |

Nota sul Database: per lui «risorsa viva» non vale alla lettera il conteggio
delle connessioni. Ferma e Dormiente guardano l'**esistenza dello
`SqlWorkspace`** (`feature_holds_resources`: `self.sql.is_some()`), perché
anche senza connessioni aperte l'area di lavoro tiene riquadri e risultati.
`feature_live_count == 0` (le connessioni) resta il numero mostrato nel
pannello Estensioni, ma non è ciò che decide lo stato; per le altre
funzionalità i due criteri coincidono.

Nota sul Terminale (eccezione a «Ferma = nessuna risorsa viva»): una scheda
«Processo terminato» tiene la sessione PTY finché non si preme Arresta o la
si chiude, così il lettore può consegnare l'ultimo output
(`Tool::needs_wakeups`, che tiene vivo il ponte di risveglio a vista aperta).
Intanto `live_count` dice 0 e le Estensioni mostrano «Ferma», ma il PTY e il
suo lettore esistono ancora. `needs_wakeups` lo interroga oggi solo l'app, e
solo per il Terminale.

**Vincolo su `idle: Some`.** Lo stream del timer invia un solo messaggio e
finisce; un timer nuovo nasce solo se cambia l'epoca. Una funzionalità con
`idle: Some(_)` deve quindi avere in `stop_feature` un ramo che libera
davvero qualcosa e porta `feature_holds_resources` a falso: altrimenti
resterebbe Dormiente per sempre, senza mai riarmarsi.

**Transizioni** (`IdleTable::reconcile`, chiamata in coda a `update` da
`app/funzionalita.rs::reconcile_feature_visibility`; idempotente):

- Ferma → Attiva: si apre una risorsa e una vista è in uso.
- Attiva → Dormiente: si chiude l'ultima vista. Il Database ha **due**
  sorgenti (pannello `db`, console SQL): ne basta una aperta per restare
  Attiva. Sul fronte si liberano i risultati
  (`SqlWorkspace::release_results`), la parte grossa e invisibile; il socket
  resta, perché riaprire non deve ripagare l'handshake.
- Dormiente → Attiva: si riapre una qualunque vista. Il timer sparisce
  perché il ramo smette di essere prodotto.
- Dormiente → Ferma: scade il timer (`Message::IdleElapsed`) o l'utente
  preme «Ferma». Entrambe le strade passano da `stop_feature`, quindi
  portano allo stesso stato.

**Scadenza valida o scartata.** L'epoca avanza a ogni ingresso *e* a ogni
uscita dalla dormienza. Una scadenza vale solo se la funzionalità è ancora
Dormiente, l'epoca è quella corrente e nessuna vista è tornata in uso
(`IdleTable::accepts`, con l'uso letto *al momento della scadenza*): un
messaggio rimasto in coda dietro una riapertura viene scartato. Le cache
economiche da ricalcolare non usano timer: si marcano stantie e si ricaricano
alla riapertura (`git_stale`, `git_log_stale`).

**Proprietà.** Il timer è una risorsa dell'app: nasce dentro lo stream di
`Subscription::run_with((feature, epoca, durata), …)` (`idle_subscription`) e
vive solo mentre la funzionalità è Dormiente. La riapertura toglie il ramo da
`Subscription::batch`, iced scarta lo stream e lo sleep muore con lui. I tool
possiedono le proprie cache di vista e le rilasciano in
`Tool::on_visibility_changed` (quelle che vivono nell'app, come gli Appunti,
seguono lo schema `NotesUnload` descritto sopra); le due cose non si
sostituiscono.

**Nessun timer, per scelta.** Non scadono mai da sole, perché chiuderle
perderebbe lavoro dell'utente o costringerebbe a ripagare un indice:
Terminale (shell e PTY), watcher dei file, documenti e stato di sintassi,
language server (l'indice è il costo: si ferma solo a comando). Git,
Segnalibri, Cronologia e Appunti non tengono risorse vive da far scadere. Il
test `nessun_altra_funzionalita_ha_un_timer_d_inattivita` (`features.rs`)
fissa l'elenco: solo il Database ha `idle: Some(_)`.

## Salvataggi e operazioni in volo

`scritture.rs::Coordinatore` decide l’ammissione delle mutazioni migrate,
possiede lease, coda dei salvataggi, automatici rimandati, presa per documento,
epoca e seguito. `guarda` include anche i documenti con autosave rimandato;
`Lume::update` è l’unico punto che trasforma il seguito in Job, Task e avvisi.
La rinomina sincrona avanza l’epoca senza lasciare un lease; i commit interni
di Sostituisci avanzano l’epoca sullo stesso lease fino al report finale.
Stage, Commit, Push e Fetch conservano la guardia Git propria: non sono
mutazioni del worktree ammesse con un falso lease. Creazioni di file, cartelle
e note restano fuori dalla migrazione. Il coordinatore non concede il commit I/O.

`save.rs` materializza lo snapshot e prepara la scrittura su un worker
bloccante; l'app conserva il job e riceve `Ready`. Prima di autorizzare il
commit rivalida ticket, documento, percorso, revisione ed epoche. Il gate
consente la revoca fino a `CommitStarted`; la ricevuta autentica aggiorna
solo lo snapshot salvato, lasciando dirty gli edit più recenti. «Salva con
nome» e NoteStore partecipano allo stesso coordinamento con le rispettive
guardie di binding e identità.

`lume-core::file_write` prepara un temporaneo nella directory del file,
sincronizza i dati e sostituisce la destinazione al commit. L'atomicità è
per file, non per progetto; «Sostituisci tutto» può avere esiti parziali.
L'impronta dei byte letti e le verifiche sul worker rilevano cambiamenti
esterni; non costituiscono una transazione con altri writer fino al rename.
Il watcher confronta i byte fuori dalla UI, conserva i buffer dirty e
rimanda le riletture durante una scrittura. Le risposte, inclusi gli errori,
devono appartenere al percorso e alla revisione correnti.

I comandi Git CLI passano su Linux da `lume-git-supervisor`, subreaper
isolato che raccoglie anche i discendenti adottati. Il binario deve essere
presente accanto all'applicazione. Su Windows il processo viene assegnato
sospeso a un Job Object prima di riprendere il thread; la prova nativa resta
aperta. Questo contratto non qualifica i processi figli dei percorsi gix o
il runtime macOS. Le mutazioni Git del worktree richiedono buffer puliti;
stage, unstage e push mantengono il coordinamento dei writer senza questa
barriera sul testo.

Il fetch vive su un worker bloccante: l'app possiede una guardia con un flag
di annullamento condiviso con gix. «Annulla» alza il flag ma non dichiara
finito il lavoro; è il risultato del worker a distinguere annullamento ed
errori reali. Il cambio progetto elimina la guardia e avanza la generazione,
così una risposta vecchia non libera o sovrascrive l'operazione nuova.
L'annullamento resta cooperativo, non un'interruzione garantita di ogni
attesa del trasporto. Gli errori di connessione e preparazione non diventano
annullamenti per effetto di un flag tardivo. Durante la ricezione si riconoscono
le cause di interruzione della versione gix bloccata in `Cargo.lock`: alcune
non hanno un tipo dedicato e richiedono anche la diagnostica upstream. Dopo
un aggiornamento di gix questo riconoscimento va verificato; una causa
sconosciuta resta un errore, non viene nascosta come annullamento.

## Zero-mock

Un controllo senza backend non si disegna. Vale per i menù (commento sopra
`MENUS` in `menu.rs`), per i pannelli e per i pulsanti: se il backend manca o
è spento, il controllo sparisce.

## Routing LSP e costo del testo

`lsp_set.rs` associa il documento al server che lo ha aperto: il routing
attuale seleziona un server per documento. Non fornisce una combinazione
simultanea di server come TypeScript ed ESLint per la stessa scheda.

`app/lsp.rs::lsp_flush_change` clona uno snapshot della rope e accoda
`ChangeSnapshot` nella mailbox limitata dell'attore. Le richieste posizionali
forzano prima il flush; l'attore materializza il testo prima di applicare
Change e prima del comando successivo. Il mark-sent resta subordinato
all'ammissione, Full mantiene il retry sulla revisione corrente.
La materializzazione non avviene più nel flush UI, ma il suo costo resta
nell'attore; `didOpen` e gli altri percorsi full-text hanno audit separato.
Le misure diagnostiche del flush non qualificano trasporto, server, renderer,
ritenzione memoria o digitazione/scroll vicino al limite di 32 MiB: OPUS-09
conserva queste prove aperte.

## Riquadri modali e messaggi

`app/ingresso.rs` classifica ogni variante di `Message` con provenienza e punto di persistenza nello stesso match esaustivo. Il gate unico, prima dei gestori, permette messaggi propri, asincroni e della finestra; i gesti rifiutati mostrano un avviso, lo sfondo viene scartato. Questa classificazione non sostituisce le autenticazioni di ticket, lease, epoca e revisione dei domini.

`overlay_manager.rs` possiede permessi, precedenza della chiusura, FIFO deduplicata, revoca e promozione, senza conoscere Iced o i modelli. `app/riquadri.rs` contiene la primitiva privata e l'adattatore di `Lume`: rivalida i modelli prima della promozione e pulisce solo la vista effettivamente chiusa o sostituita. L'attesa è una vista di un modello esistente, non un comando utente da eseguire più tardi.

Quando Cerca ovunque viene sostituita, la ricerca viene sospesa e il worker rilasciato; query e dati economici della vista restano, e la riapertura esplicita riavvia il lavoro quando serve. Uscita e risposte dei worker mantengono le proprie autenticazioni; la coda non può far riapparire modali durante l'uscita.
