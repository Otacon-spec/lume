# Contribuire a Lume

## Lingua

Il progetto è in italiano: commenti, messaggi all'utente, nomi dei test,
messaggi di commit, documentazione. Unica eccezione: `THIRD_PARTY_NOTICES.md` e `THIRD_PARTY_LICENSES.md`,
che riportano testi di terze parti nella loro lingua.

## Le regole che non si discutono

- **Zero-mock.** Un controllo senza un backend che funziona non si disegna:
  né in grigio né disabilitato (README, «La regola zero-mock»).
- **Nessun lavoro da inattivo.** Niente timer periodici (`time::every`,
  `interval`), niente poll. Una risorsa pesante vive dentro una subscription
  e sparisce quando lo stato smette di chiederla; un tool rilascia le sue
  cache in `Tool::on_visibility_changed`. Vedi `ARCHITECTURE.md`.
- **Git passa da gitoxide** (`crates/lume-git`), con **una sola eccezione
  dichiarata** (sezione «Eccezione: git CLI» qui sotto). Fuori da quella,
  niente CLI `git` nel codice di Lume; nei test si usa liberamente.
- **Budget sulle risorse a runtime** (README): RAM in private bytes, thread,
  processi figli, lavoro da inattivo. Una modifica che lo sfonda non entra, o
  entra dichiarandolo. Il peso dell'eseguibile non è un vincolo.
- Nel codice non di test niente `unwrap`, `expect` né indicizzazione con `[]`:
  i lint del workspace li segnalano e la CI li rende errori.

## Eccezione: git CLI

Scelta di progetto: gitoxide non sa fare il push e non è il
posto giusto per le operazioni che riscrivono il lavoro dell'utente in modi
che Git ha collaudato per vent'anni. Per quattro operazioni, e solo quelle,
è previsto l'uso del binario `git` trovato su `PATH`:

- push;
- cambio di branch (`git switch`);
- stash apply e pop (`git stash apply|pop --index`);
- merge con conflitti (`git merge`, `git commit`, `git merge --abort`).

Queste operazioni sono collegate al pannello Git e partono solo su richiesta.
Pull usa gitoxide per fast-forward e merge pulito; se rileva conflitti, non
modifica worktree, index o riferimenti. Il merge esplicito di un branch locale
usa invece Git CLI e mantiene `MERGE_HEAD` e le voci di conflitto nell'index,
così l'utente può risolverle e continuare o annullare il merge. Stash push/drop
usano gitoxide; apply/pop usano Git CLI. Fetch, push, switch, stash apply/pop
e merge girano su worker e possono essere annullati. GitCli possiede e raccoglie
i lettori stdout/stderr. Su Linux il supervisor isolato è subreaper:
termina il gruppo e raccoglie i discendenti adottati, compresi gli helper
che creano una nuova sessione. Il binario `lume-git-supervisor` va costruito
prima dei test app/Git ed è distribuito accanto all'applicazione. Su Windows
il processo nasce sospeso, entra nel Job Object e solo allora riparte;
il codice è cross-compilato, la qualifica nativa resta aperta. Su macOS il
cleanup del gruppo non qualifica gli helper che escono dal gruppo. Questo
contenimento non è una sandbox. Gitoxide può inoltre avviare `git-upload-pack`
o `ssh`: quei percorsi richiedono prove distinte dal wrapper CLI.

Regole per chi completa l'integrazione:

- In `lume-git/src`, `Command::new` è ammesso nel client `cli.rs` e nel
  supervisor `bin/lume-git-supervisor.rs`. Gli altri crate non avviano Git
  direttamente: passano da `GitCli`. Lo verifica
  `crates/lume-git/tests/eccezione_cli.rs`.
- `git` si cerca su `PATH` come si cercano i language server, senza sonda:
  se non c'è, i pulsanti che lo richiedono **non vengono disegnati**
  (zero-mock) e niente si avvia.
- Nessun processo Git CLI da inattivo: parte solo da un'azione esplicita.
  Quando l'operazione supporta cancellazione, la UI resta occupata fino
  all'esito del worker; il cambio progetto invalida la risposta precedente.
  Il cleanup dei figli resta limitato come descritto sopra.
- Nomi di branch e di remote devono essere validati prima di arrivare alla
  riga di comando; i comandi devono separare opzioni e nomi con `--` dove
  la sintassi di Git lo consente.
- I test che provano queste operazioni usano repository veri in cartelle
  temporanee (`tempfile`), mai il repository su cui si sta lavorando.

## Eccezione: jj CLI (opzionale)

Scelta di progetto: se il progetto contiene una cartella `.jj`, Lume mostra il
pannello «Jujutsu» e delega al programma `jj` trovato su `PATH` per `log`,
`status`, `describe`, `new` e `git push`. Nessun crate dipende da `jj-lib` o
`jj-cli`; Lume non scrive dentro `.jj` e non crea repository Jujutsu.

Regole per chi modifica l'integrazione:

- `Command::new` compare solo in `crates/lume-jj/src/cli.rs`. Il test
  `crates/lume-jj/tests/eccezione_cli.rs` vieta altri lanci, timer, sonde
  `jj --version` e dipendenze da `jj-lib`/`jj-cli`.
- Senza `.jj` il pannello è nascosto e non partono processi. Il workspace si
  cerca solo al cambio progetto; `jj` si cerca solo quando si apre il pannello.
- Senza `jj` nel `PATH` il pannello spiega come installarlo e non mostra
  pulsanti. Non si avviano processi in background: solo apertura, Aggiorna o
  un'azione esplicita; le operazioni si possono annullare.
- Il messaggio dell'utente arriva come un unico argomento `--message=<testo>`;
  il push usa il remote predefinito e bookmark già tracciati.
- I test usano repository temporanei; l'integrazione col `jj` vero salta se
  `jj` o `git` non sono disponibili.

## Prima di aprire una PR

    cargo fmt --all --check
    cargo clippy --workspace --all-targets --no-deps -- -D warnings
    cargo test --workspace

La CI (`.github/workflows/ci.yml`) esegue gli stessi comandi (con `--locked` e la toolchain fissata) su Windows, più
il controllo delle licenze, `cargo deny check` e la build con la MSRV; a ogni
push su `main` riporta anche i byte di `lume.exe` nel riepilogo del job, solo
come informazione. Il workflow di release richiama la CI e pubblica solo
dopo il suo successo sul commit da distribuire.

### Prerequisiti dei test

- `git` nel `PATH`, con `user.name` e `user.email` configurati: i test di
  `lume-git` creano repository e commit veri.
- `rustc` nel `PATH`.
- Una shell per la PTY: `cmd.exe` su Windows, `sh` altrove.
- `tasklist`/`taskkill` su Windows, `kill` altrove: i test del terminale
  verificano che un processo figlio muoia davvero.
- Il test del vault scrive nel magazzino credenziali del sistema operativo
  reale con il servizio isolato «Lume (test)», poi rilegge e cancella la voce.
  Usa Gestore credenziali su Windows e Secret Service su Linux; se il vault
  non è disponibile il test salta. La CI Linux lo avvia e richiede il test.

Se uno di questi strumenti manca, il test che ne ha bisogno **salta** e lo dice
(«SALTATO: manca `git` nel PATH …»): il messaggio si vede con
`cargo test -- --show-output`. Con `LUME_TEST_RICHIEDI_STRUMENTI=1` lo stesso
test **fallisce** con lo stesso messaggio; la CI usa questa modalità, così un
runner senza `git` non produce mai un falso verde. Un test nuovo che dipende da
uno strumento esterno chiama `lume_testkit::richiedi!(Strumento::…)` come
prima istruzione; i test che rilanciano il proprio eseguibile usano
`Strumento::EseguibileCorrente`. Nessun test usa un finto `git`, una finta
shell o un finto server: se lo strumento manca, il test salta.

## Misurare

- Avvio: `$env:LUME_STARTUP_TRACE=1; lume.exe` stampa le tappe fino al primo
  frame.
- Dati isolati: `LUME_DATA_DIR=<cartella>` sposta sessione, log e server in
  una cartella a scelta.
- Build di release: `pwsh tools/build_release_bundle.ps1 -SoloExe` costruisce
  `target\release\lume.exe` con i percorsi rimappati e ne stampa i byte
  (informazione, non un limite);
  senza `-SoloExe` produce anche lo zip con i language server.
- Log dei crash: `lume.exe --prova-crash`.
- Memoria e processi figli: guarda i private bytes e i figli del processo con
  gli strumenti del sistema, partendo da una sessione vuota e con
  `LUME_DATA_DIR` isolata, come descritto nel budget del README.

## Aggiornare i temi di icone

`pwsh tools/update_icon_themes.ps1` scarica gli archivi fissati nello script
(Material Icon Theme v5.33.1 e Catppuccin Icons v1.26.0: sono gli unici URL
ammessi dai suoi parametri), rigenera le icone in `crates/lume-icons/assets/`
con il `LICENSE` di ciascun tema e riscrive `ASSET_MANIFEST.json`. Per
cambiare versione si cambiano gli URL nello script. Poi
`cargo test -p lume-icons`.

## Licenze di terze parti

`THIRD_PARTY_LICENSES.md` si rigenera quando cambia `Cargo.lock`, con
cargo-about 0.9.2:

    cargo about generate -c about.toml about.hbs -o THIRD_PARTY_LICENSES.md

La CI fallisce se il file non è aggiornato. Il test
`le_licenze_di_terze_parti_hanno_i_titolari_veri` del bundler fallisce se un
crate nuovo porta un testo di licenza col titolare segnaposto: in quel caso
serve una voce `clarify` in `about.toml` (file di licenza del pacchetto, o del
repository al commit pubblicato, con il suo SHA-256).

## Aggiornare il catalogo dei language server

Il catalogo è `crates/lume-language-servers/assets/server-releases.json`.
Per ogni server e piattaforma:

1. `version` e `url` dalla release ufficiale su GitHub; l'URL deve cominciare
   con `https://github.com/<repository>/releases/download/<version>/`, e il
   repository è fissato per id in `validate_server` (`src/catalog.rs`).
2. `digest` = `sha256:` più lo SHA-256 dell'archivio scaricato
   (`Get-FileHash <file> -Algorithm SHA256`), `size` = i suoi byte.
3. `archive` e `binary_path` come dentro l'archivio.
4. Se cambia la licenza, aggiorna il testo in
   `crates/lume-language-servers/assets/licenses/` e l'elenco
   `THIRD_PARTY_SERVERS` in `src/bin/lume-server-bundle.rs`.
5. `cargo test -p lume-language-servers`.

Le struct del catalogo hanno `deny_unknown_fields`: un campo nuovo nel JSON
richiede prima il campo nella struct.

## Estendere Lume

Guide passo passo per i quattro modi più comuni di estendere Lume. Ognuna ha
un test che la tiene allineata al codice: se cambi il codice e la guida
diventa falsa, il test fallisce.

- [Aggiungere un linguaggio](docs/estendere/aggiungere-un-linguaggio.md): grammatica tree-sitter e query di evidenziazione (crate `lume-syntax`).
- [Aggiungere un language server](docs/estendere/aggiungere-un-language-server.md): una voce nel catalogo, con la sua impronta SHA-256 (crate `lume-language-servers`).
- [Aggiungere un tema di colori](docs/estendere/aggiungere-un-tema-colori.md): un file TOML in `lume-theme`.
- [Aggiungere un tema di icone](docs/estendere/aggiungere-un-tema-icone.md): una cartella con `icon-theme.json` e gli SVG, senza toccare il codice.

Per aggiungere uno strumento (un pannello con vita propria) non c'è una guida a parte: il contratto è nella sezione «Tool, Registry, ToolRequest» di [ARCHITECTURE.md](ARCHITECTURE.md). Per **aggiornare** (non aggiungere) il catalogo dei server o i temi di icone vedi le sezioni «Aggiornare il catalogo dei language server» e «Aggiornare i temi di icone» qui sopra.

## Commit

Stile conventional, in italiano: `fix(lume-app): …`, `feat(lume-core): …`,
`docs: …`, `chore: …`, `ci: …`. Un commit, una ragione.

## Nomi e marcatori

Tutto ciò che segue è controllato da `tools/verifica-convenzioni.sh`, che gira
in CI: se la regola non è rispettata, il controllo fallisce e dice dove.

- **Nomi dei file Rust**: minuscolo con underscore (`editor_empty.rs`). Solo i
  binari in `src/bin/` possono avere il trattino, come il nome dell'eseguibile.
- **Attenzione nei commenti**: si scrive `ATTENZIONE:` (parola), mai l'emoji.
  I glifi «⚠» dell'interfaccia (barra di stato, pannello Git) sono testo per
  l'utente e restano.
- **I test provano il comportamento**: nessun test legge un file `.rs` con
  `include_str!` (unica eccezione: i test `crates/*/tests/guida.rs`, che
  controllano che le guide «Estendere Lume» citino il codice vero). Se una
  regola di struttura merita di essere protetta, si scrive il test che la viola
  e si vede fallire.
- **`unsafe`**: ogni blocco ha sopra un commento `// SAFETY:` che dice le
  precondizioni (`tools/verifica-safety.sh`); i moduli che lo ospitano lo
  dichiarano con `#[allow(unsafe_code)]`.

### Tre ricerche

| Dove | Che cosa | Come si chiama nell'interfaccia |
|---|---|---|
| `crates/lume-core/src/search.rs` | il motore: cerca nel testo dei file | (nessuno: è la libreria) |
| `crates/lume-app/src/tools/search_tool.rs` | il pannello e le manopole | pannello «Cerca» |
| `crates/lume-app/src/search.rs` | l'overlay che trova per nome | «Cerca ovunque» (doppio Maiusc) |
