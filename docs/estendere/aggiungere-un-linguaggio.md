# Aggiungere un linguaggio

Un linguaggio in Lume è una grammatica tree-sitter più una query di highlight. Il codice sta tutto in `crates/lume-syntax/src/language.rs`. L'esempio seguito in questa guida è Kotlin, già presente nel codice: ogni riga citata qui sotto è quella vera e un test (`crates/lume-syntax/tests/guida.rs`) fallisce se la guida e il codice divergono.

## 1. La grammatica

Serve un crate tree-sitter pubblicato su crates.io che esponga `LANGUAGE`. Si dichiara nel `Cargo.toml` alla radice, in `[workspace.dependencies]`:

```toml
tree-sitter-kotlin-ng = "1.1"
```

e si usa dal crate `lume-syntax` (`crates/lume-syntax/Cargo.toml`, sezione `[dependencies]`):

```toml
tree-sitter-kotlin-ng = { workspace = true }
```

Prima di aggiungere una dipendenza controlla la licenza del crate: deve essere compatibile con Apache-2.0; poi si rigenera `THIRD_PARTY_LICENSES.md` (vedi «Licenze di terze parti» in `CONTRIBUTING.md`).

## 2. Identificatore ed estensioni

In `language.rs` si aggiungono tre cose.

Una variante di `LanguageId`:

```rust
Kotlin,
```

Le estensioni riconosciute da `detect` (il confronto è sull'estensione del file):

```rust
"kt" | "kts" => Some(LanguageId::Kotlin),
```

E il braccio di `spec`, che restituisce la specifica caricata una sola volta:

```rust
LanguageId::Kotlin => KOTLIN.as_ref(),
```

`KOTLIN` è una `static` inizializzata alla prima richiesta (mai all'avvio: un linguaggio che non apri non costa nulla):

```rust
static KOTLIN: LazyLock<Result<LanguageSpec, String>> = LazyLock::new(|| {
    build_spec_with_folding(
        tree_sitter_kotlin_ng::LANGUAGE.into(),
        KOTLIN_HIGHLIGHTS,
        Some(include_str!("../queries/kotlin_folds.scm")),
    )
});
```

`build_spec` (usato nei test) compila la query e restituisce un `LanguageSpec`; se la query non è valida restituisce il motivo (`Err`), così un linguaggio rotto non fa cadere l'editor: il file si apre senza colori e la barra di stato mostra `ATTENZIONE: evidenziazione non disponibile: …`.

## 3. La query di highlight

Molte grammatiche pubblicano già una query pronta (per esempio XML usa `tree_sitter_xml::XML_HIGHLIGHT_QUERY`). Se la query è divisa in più file, `build_spec_parts` le unisce andando a capo fra l'una e l'altra (è quello che fa JavaScript con TypeScript e JSX).

`tree-sitter-kotlin-ng` non ne pubblica una, quindi Lume la porta in una costante, `KOTLIN_HIGHLIGHTS`, dentro `language.rs`. Una query è un elenco di pattern con capture:

```scheme
(function_declaration
  "fun" @keyword
  name: (identifier) @function)

[
  (line_comment)
  (block_comment)
] @comment
```

Per scrivere la query guarda i nomi dei nodi con `tree-sitter parse file.kt` (CLI di tree-sitter) e parti dai pochi nodi che contano: parole chiave, stringhe, numeri, commenti, tipi, funzioni.

## 4. Da capture a colore

Il nome della capture (`@function`, `@string.escape`, …) diventa una `TokenClass` tramite `class_for_capture`, che prova i prefissi nell'ordine della tabella qui sotto: il primo che combacia vince. Un nome che non combacia con nessuno diventa `Text` (nessun colore). Le classi sono 13: `Keyword`, `Type`, `Function`, `String`, `Number`, `Comment`, `Attribute`, `Constant`, `Operator`, `Punctuation`, `Variable`, `Property`, `Text`.

| Prefisso della capture | Classe |
|---|---|
| `text.title` | `Keyword` |
| `text.literal` | `String` |
| `text.uri`, `text.reference` | `Function` |
| `punctuation.special` | `Attribute` |
| `keyword`, `label`, `storage`, `conditional`, `repeat`, `include` | `Keyword` |
| `type`, `tag`, `namespace` | `Type` |
| `function`, `method` | `Function` |
| `string`, `escape`, `character` | `String` |
| `number`, `float`, `integer` | `Number` |
| `comment` | `Comment` |
| `attribute` | `Attribute` |
| `constant`, `boolean` | `Constant` |
| `operator` | `Operator` |
| `punctuation` | `Punctuation` |
| `property`, `field` | `Property` |
| `variable`, `parameter` | `Variable` |

Quindi `@punctuation.special` è un `Attribute` mentre `@punctuation.bracket` è una `Punctuation`: l'ordine conta. Se una capture nuova non trova posto nella tabella, non aggiungere un prefisso a `class_for_capture` per un solo linguaggio: rinomina la capture nella query.

## 5. Il test

Aggiungi il linguaggio ai due test di `crates/lume-syntax/tests/highlight.rs`:

- `additional_languages_are_detected_and_highlighted`: una voce per linguaggio con nome del file, `LanguageId` e una riga di codice; il test controlla che l'estensione sia riconosciuta e che almeno un token risulti colorato (cioè che la query si compili e catturi qualcosa);
- `detects_additional_language_extension_aliases`: una riga per ogni estensione (per Kotlin, `("build.kts", LanguageId::Kotlin)`).

Il test è quello che ti dice se la query si compila: una query sbagliata non rompe la compilazione, produce un linguaggio senza colori.

Poi esegui `cargo test -p lume-syntax` e apri un file del linguaggio in Lume per un controllo a occhio.

## Folding

Tutte le 18 lingue hanno query di folding. `build_spec_with_folding` compila anche la query `queries/<lingua>_folds.scm`: le capture `@fold` selezionano i costrutti; il produttore scarta i nodi su una sola riga e normalizza le regioni annidate. `build_spec_parts` riceve la stessa query dopo le parti di highlight. Una query di folding non valida conserva i colori e segnala il motivo in `folding_error`. Aggiungi esempi multilinea e su una riga in `tests/folding.rs`.
