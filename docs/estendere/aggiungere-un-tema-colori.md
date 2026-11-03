# Aggiungere un tema di colori

Un tema di colori è un file TOML dentro il crate `lume-theme` (`crates/lume-theme/assets/themes/`), incluso nel programma a tempo di compilazione. Non c'è un formato esterno da caricare a runtime: aggiungere un tema vuol dire aggiungere un file e una riga di codice.

I temi che ci sono oggi:

| Id | File | Nome mostrato |
|---|---|---|
| `officina` | `officina.toml` | Officina |
| `grafite` | `grafite.toml` | Grafite |
| `ossido` | `ossido.toml` | Ossido |
| `inchiostro` | `inchiostro.toml` | Inchiostro |
| `terracotta` | `terracotta.toml` | Terracotta |

## 1. Il file

Copia `officina.toml` in un file nuovo, per esempio `sabbia.toml`, cambia `id` e `name` e poi i colori. Tutti i campi vanno presenti. Un esempio completo (un tema chiaro):

```toml
id = "sabbia"
name = "Sabbia"

deep = "#e8e2d4"
bg = "#f4efe4"
surface = "#faf6ec"
inset = "#ece6d8"
border = "#d8d0bd"
border2 = "#cbc2ac"
text = "#2b2a26"
ui = "#45423a"
sec = "#6b675b"
muted = "#8f8a7b"
icon = "#7a7566"
faint = "#aaa593"
tree = "#5a564a"
hover = "#ebe5d6"
hover2 = "#e0d9c7"
accent = "#b5651d"
accent-on = "#8f4e12"
accent-dim = "#d9b48a"
sel = "#e9d5b3"
good = "#3f8f5a"
good-bg = "#dcebd9"
good-br = "#b9d6b9"
warn = "#a87a1c"
warn-bg = "#f3e6c2"
warn-br = "#e0cf9b"
bad = "#b5453d"
fn = "#2f6f9f"
str = "#5f7d2e"
num = "#a8552b"
comment = "#8a8570"
hl = "#f0dfa8"
```

I colori sono stringhe `#rrggbb`.

## 2. Che cosa è ogni colore

Il codice non descrive le chiavi una per una: la tabella riassume come le usano i temi e le schermate di oggi (l'elenco dei posti è indicativo, non completo). Il modo affidabile di scegliere un valore è aprire uno dei cinque file e guardare la distanza fra una chiave e la vicina.

| Chiavi | A che cosa servono |
|---|---|
| `id`, `name` | Identificatore (minuscolo, senza spazi, unico) e nome mostrato nella scelta del tema. |
| `deep`, `bg`, `surface`, `inset` | Superfici, dalla più arretrata alla più in rilievo: `deep` (il terminale), `bg` (lo sfondo generale), `surface` (pannelli e riquadri sopra lo sfondo), `inset` (zone incassate, per esempio nel diff). |
| `border`, `border2` | Bordi, il secondo più marcato (usato nel diff). |
| `text`, `ui`, `sec`, `muted`, `faint` | Testo, dal più forte al più debole: `text` per il contenuto, `ui` per i controlli, `sec` per il testo secondario, `muted` e `faint` per gli aiuti (`faint` compare nel breadcrumb). |
| `icon`, `tree` | Icone (barra delle attività, barra di stato) e voci dell'albero dei file. |
| `hover`, `hover2` | Sfondo al passaggio del mouse, in due intensità. |
| `accent`, `accent-on`, `accent-dim` | Colore d'accento; il testo che ci sta sopra; la versione attenuata. |
| `sel` | Selezione. |
| `good`, `good-bg`, `good-br` | Stato positivo: testo, sfondo, bordo. |
| `warn`, `warn-bg`, `warn-br` | Avviso: testo, sfondo, bordo. |
| `bad` | Errore. |
| `fn`, `str`, `num`, `comment` | Evidenziazione della sintassi: funzioni, stringhe, numeri, commenti. |
| `hl` | Evidenziazione di una corrispondenza. |

## 3. Registrare il tema

In `crates/lume-theme/src/tokens.rs`:

1. aggiungi il file **in fondo** a `EMBEDDED` e porta la lunghezza da 5 a 6 (`const EMBEDDED: [&str; 6]`); l'ordine è quello della scelta del tema, quindi i temi esistenti non si spostano:

```rust
include_str!("../assets/themes/sabbia.toml"),
```

2. aggiorna il test `i_cinque_temi_embedded_si_caricano`: aggiungi `"sabbia"` all'elenco degli id (il nome dice «cinque» ma resta com'è: la guida e il suo test lo citano; se lo rinomini, aggiorna anche `crates/lume-theme/tests/guida.rs` e questa guida).

Il tema compare fra quelli selezionabili dalle Impostazioni senza altro codice: la finestra scorre l'elenco restituito da `LumeTheme::builtin()`. Attenzione: un tema che non si legge non blocca l'avvio, viene semplicemente saltato con un messaggio `lume-theme: tema embedded non valido` su stderr. Se il tuo tema non compare, guarda lì (il test `i_cinque_temi_embedded_si_caricano` lo cattura prima).

## 4. Provarlo

`cargo test -p lume-theme`, poi apri Lume, scegli il tema e attraversa i pannelli: editor con un file acceso e uno spento, terminale, diff, barra di stato, finestre di conferma. Un tema chiaro non è mai stato provato a schermo: controlla soprattutto che `accent-on` resti leggibile sopra `accent`, e che `sel` e `hl` non coprano il testo.
