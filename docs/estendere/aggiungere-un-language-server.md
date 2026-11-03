# Aggiungere un language server

Lume non lancia mai un server che non sia nel suo catalogo: il catalogo è un file JSON dentro il crate `lume-language-servers` (`crates/lume-language-servers/assets/server-releases.json`) e ogni voce è controllata da `validate_server` prima di essere accettata. Questo è voluto: un server è un eseguibile scaricato da internet e avviato sul computer di chi usa Lume, quindi ogni voce dichiara l'indirizzo esatto, l'impronta SHA-256 e la dimensione del file, e Lume rifiuta tutto il resto.

Per aggiornare la versione di un server che c'è già (senza aggiungerne uno) vedi la sezione «Aggiornare il catalogo dei language server» di `CONTRIBUTING.md`. Questa guida riguarda solo un server **nuovo**.

## 1. La voce del catalogo

Una voce è un oggetto dell'array `servers`. L'esempio seguente è completo e ogni chiave è spiegata nella tabella sotto:

```json
{
  "id": "esempio-ls",
  "name": "esempio-ls",
  "executable": "esempio-ls",
  "args": ["--stdio"],
  "languages": ["esempio"],
  "extensions": ["esl"],
  "workspace_markers": ["esempio.toml", ".git"],
  "resource_class": "light",
  "release": {
    "repository": "esempio/esempio-ls",
    "version": "v1.2.3",
    "assets": [
      {
        "platform": {"os": "windows", "arch": "x86_64"},
        "name": "esempio-ls-v1.2.3-x86_64-pc-windows-msvc.zip",
        "url": "https://github.com/esempio/esempio-ls/releases/download/v1.2.3/esempio-ls-v1.2.3-x86_64-pc-windows-msvc.zip",
        "digest": "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        "size": 5917660,
        "archive": "zip",
        "binary_path": "esempio-ls.exe"
      }
    ]
  }
}
```

| Chiave | Significato |
|---|---|
| `schema` | Versione del formato del file (oggi `1`). Sta in cima al file, accanto a `servers`, non dentro una voce. |
| `servers` | L'array delle voci. Un `id` o un'estensione ripetuti sono errori. |
| `id` | Identificatore stabile: la chiave con cui il resto del codice riconosce il server. |
| `name` | Nome mostrato all'utente. Anche ogni file scaricato ha un `name`: il nome del file nella release. |
| `executable` | Nome del programma da avviare. |
| `args` | Argomenti da passare all'avvio; per parlare in LSP su stdin/stdout quasi tutti i server ne vogliono uno (`--stdio`, `lsp`, `server`). |
| `languages` | Identificatori di linguaggio LSP (`rust`, `toml`, `markdown`…). |
| `extensions` | Estensioni dei file gestiti, senza punto. Due server non possono dichiarare la stessa estensione. |
| `workspace_markers` | File o cartelle che segnano la radice del progetto per questo server (`Cargo.toml`, `.git`…). |
| `resource_class` | `light` o `heavy`: quanto pesa il server in esecuzione. `heavy` è per i server che indicizzano il progetto (rust-analyzer). |
| `release` | Da dove scaricare: repository, versione e file per ogni piattaforma. |
| `repository` | Repository GitHub nella forma `proprietario/nome`. |
| `version` | Tag della release, esattamente come compare nell'indirizzo. |
| `assets` | Un file per ogni piattaforma supportata, al massimo 16. |
| `platform` | La piattaforma del file: un oggetto con `os` e `arch`. |
| `os` | `windows`, `linux` o `macos` (come `std::env::consts::OS`). |
| `arch` | Architettura, come `std::env::consts::ARCH` (oggi `x86_64`). |
| `url` | Deve cominciare con `https://github.com/<repository>/releases/download/<version>/`. |
| `digest` | `sha256:` seguito da 64 cifre esadecimali. Su Windows: `Get-FileHash -Algorithm SHA256 <file>`. |
| `size` | Dimensione in byte del file scaricato, da 1 a 64 MiB. Su Windows: `(Get-Item <file>).Length`. |
| `archive` | Formato del file: `zip`, `tar-gz` (archivio) oppure `gzip` (un solo file compresso). |
| `binary_path` | Percorso dell'eseguibile dentro l'archivio, relativo e senza `..`. |

Il test del catalogo (`tests/catalog.rs`, `embedded_versions_commands_and_resource_classes_are_pinned`) pretende che ogni server abbia un file per la piattaforma su cui il test gira: oggi la CI è solo Windows x86_64, ma le voci esistenti dichiarano anche linux e macos (tutte x86_64), e una voce senza la piattaforma di chi lancia i test li fa fallire sul suo computer.

## 2. La lista ufficiale: `validate_server`

Il catalogo non è un elenco libero: `validate_server` (in `crates/lume-language-servers/src/catalog.rs`) confronta l'`id` con una lista scritta nel codice e il `repository` con quello atteso per quell'id. Un `id` che non è nella lista viene rifiutato con `UnknownServer`; un repository diverso da quello atteso con `InvalidRepository`. Serve a impedire che una modifica al solo JSON faccia scaricare un eseguibile da un posto non deciso nel codice.

Per aggiungere il server si aggiunge quindi **anche** un braccio al `match` di `validate_server`:

```rust
"esempio-ls" => "esempio/esempio-ls",
```

Il resto del controllo (indirizzo, impronta, dimensione, piattaforme non duplicate, percorso relativo) è comune a tutti i server e non cambia.

## 3. Estensioni e conflitti

Ogni estensione appartiene a un solo server: due voci con la stessa estensione danno `DuplicateExtension` all'avvio del catalogo, non un comportamento a sorte. Se vuoi che un secondo server gestisca file già serviti da un altro, il catalogo non lo permette: serve prima una decisione di progetto, non un cambio di JSON.

## 4. Opzioni di avvio e prove

Se il server ha impostazioni di avvio che conviene fissare (di solito per non fargli eseguire codice del progetto, come `cargo build` o le macro procedurali) si aggiunge un braccio a `initialization_options`, in `crates/lume-app/src/lsp_mode.rs`. Per i server che non lo richiedono non si scrive niente: restituisce `None`.

Le prove da aggiornare:

- `tests/catalog.rs` (in `lume-language-servers`): il numero di server nel test `catalog_selects_real_servers_without_process_side_effects`, e una riga per la versione e gli argomenti in `embedded_versions_commands_and_resource_classes_are_pinned`;
- se il server ha bisogno di opzioni di avvio, un test accanto a `initialization_options`.

## 5. Licenze nello zip

Lo zip di distribuzione porta con sé il testo della licenza di ogni server: MIT e Apache-2.0 chiedono che il testo viaggi con la copia. Quando aggiungi un server al catalogo, nel bundler (`crates/lume-language-servers/src/bin/lume-server-bundle.rs`):

1. copia il testo della licenza in `crates/lume-language-servers/assets/licenses/`, preso dal repository del server al tag del catalogo (il nome del file lì è libero);
2. aggiungi una voce a `THIRD_PARTY_LICENSE_FILES`, con il nome che avrà nello zip, `<id>-LICENSE-<tipo>.txt`, dove `<id>` è l'`id` della voce del catalogo e `<tipo>` è `MIT` o `APACHE` (un server con doppia licenza ha due file), e il testo incluso con `include_str!`;
3. aggiungi il server a `THIRD_PARTY_SERVERS`: il nome, l'indirizzo `https://github.com/<repository>` (lo stesso `release.repository` del catalogo), la licenza e i file citati come `licenses\<file>`.

Il test `ogni_server_del_catalogo_ha_i_testi_di_licenza_nel_bundle` fallisce se ne manca uno dei tre. L'altra direzione, cioè che ogni file citato esista e non sia vuoto, la controlla `lo_zip_porta_con_se_tutti_i_testi_di_licenza`.

Poi esegui `cargo test -p lume-language-servers -p lume-app`.
