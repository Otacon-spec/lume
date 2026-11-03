# Aggiungere un tema di icone

Un tema di icone è una cartella con un file `icon-theme.json` e degli SVG. Non si compila niente: si copia la cartella nella cartella di configurazione di Lume e il tema compare nelle Impostazioni.

Per i temi già presenti nel programma (quelli embedded) e per le licenze delle icone vedi «Aggiornare i temi di icone» in `CONTRIBUTING.md`: questa guida è solo per un pacchetto esterno.

## 1. Il manifest

Il file si chiama `icon-theme.json`. Un esempio completo:

```json
{
  "iconDefinitions": {
    "file": {"iconPath": "icons/file.svg"},
    "folder": {"iconPath": "icons/folder.svg"},
    "folder-open": {"iconPath": "icons/folder-open.svg"},
    "root": {"iconPath": "icons/root.svg"},
    "root-open": {"iconPath": "icons/root-open.svg"},
    "rust": {"iconPath": "icons/rust.svg"},
    "src": {"iconPath": "icons/src.svg"}
  },
  "file": "file",
  "folder": "folder",
  "folderExpanded": "folder-open",
  "rootFolder": "root",
  "rootFolderExpanded": "root-open",
  "fileExtensions": {"rs": "rust"},
  "fileNames": {"Cargo.toml": "rust"},
  "folderNames": {"src": "src"},
  "folderNamesExpanded": {"src": "src"}
}
```

- `iconDefinitions` dà un nome a ogni SVG; il percorso (`iconPath`) è relativo alla cartella del tema.
- `file`, `folder`, `folderExpanded`, `rootFolder` e `rootFolderExpanded` sono obbligatori: sono le icone di riserva per un file, una cartella chiusa, una cartella aperta e la radice del progetto (chiusa e aperta).
- `fileExtensions`, `fileNames`, `folderNames` e `folderNamesExpanded` sono facoltativi e associano un'estensione, un nome di file o un nome di cartella a un'icona. Esistono anche `rootFolderNames`, `rootFolderNamesExpanded` e `languageIds`, con la stessa forma, e un blocco `light` che ridefinisce le associazioni per il tema chiaro.
- Chiavi che non esistono nel formato vengono rifiutate: un errore di battitura non passa in silenzio.

## 2. Gli SVG

Ogni icona è un piccolo SVG. Il minimo che Lume accetta:

```svg
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path d="M1 1h14v14H1z"/></svg>
```

Lume mostra gli SVG così come sono, quindi il pacchetto non può contenere niente che faccia altro che disegnare. Vengono rifiutati:

- `<!DOCTYPE`, `<!ENTITY`, `<script`, `<foreignObject`, `@font-face`, `@import`, `<?xml-stylesheet`, `file://` e `data:`;
- gli elementi `<image>`, `<font>`, `<animate>`, `<animateMotion>`, `<animateTransform>` e `<set>`;
- gli attributi che cominciano con `on` (`onclick`…);
- ogni `href` che non punti a un elemento dello stesso file (`#id`);
- ogni `url()` esterno: `url(#id)` va bene, `url(https://…)` no.

Anche i percorsi del manifest sono controllati: niente indirizzi, niente percorsi assoluti, niente `..`, e il file deve restare dentro la cartella del tema.

## 3. I limiti

Se un limite viene superato il pacchetto è rifiutato per intero (non parzialmente):

| Costante | Valore | Che cosa limita |
|---|---|---|
| `MAX_MANIFEST_BYTES` | 1048576 | Dimensione di `icon-theme.json` in byte (1 MiB). |
| `MAX_SVG_BYTES` | 524288 | Dimensione di ogni SVG in byte (512 KiB). |
| `MAX_ASSETS` | 4096 | Numero di voci in `iconDefinitions`. |
| `MAX_ASSOCIATIONS` | 65536 | Numero totale di associazioni (estensioni, nomi, cartelle). |
| `MAX_PACK_BYTES` | 67108864 | Dimensione totale del pacchetto in byte (64 MiB). |

## 4. Dove metterlo

Copia la cartella (per esempio `notte`) in:

```text
%APPDATA%\lume\Lume\config\icon-themes\notte\
```

così che il manifest sia `%APPDATA%\lume\Lume\config\icon-themes\notte\icon-theme.json`. Se la variabile d'ambiente `LUME_DATA_DIR` è impostata, la cartella è `icon-themes` dentro quella. Nelle Impostazioni il tema si sceglie per nome; internamente il suo identificatore è `external:` seguito dal nome della cartella in minuscolo (`external:notte`); i caratteri diversi da lettere, cifre, `-`, `_` e `.` diventano `-`.

Lume legge i pacchetti all'avvio e quando premi «Rileggi pacchetti esterni» nelle Impostazioni: non serve riavviare. Ne legge al massimo 128 (`MAX_EXTERNAL_PACKS`, in `crates/lume-app/src/appearance.rs`); i pacchetti oltre il limite vengono ignorati e l'errore lo segnala.

## 5. Provarlo

Apri Lume su un progetto con file di più tipi, scegli il tema e controlla l'albero dei file e le altre liste di file. Un pacchetto che non si carica viene registrato come non disponibile, con il motivo dell'errore (il file o il limite che lo ha fatto rifiutare).
