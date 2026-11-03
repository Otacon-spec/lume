//! Registro delle lingue supportate: rilevamento per estensione e spec
//! (grammatica + query di highlight) inizializzate in modo lazy.

use std::path::Path;
use std::sync::LazyLock;

use tree_sitter::{Language, Query};

use crate::token::TokenClass;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LanguageId {
    Rust,
    Toml,
    Markdown,
    Java,
    C,
    Cpp,
    CSharp,
    Python,
    JavaScript,
    TypeScript,
    Tsx,
    Json,
    Yaml,
    Html,
    Css,
    Sql,
    Kotlin,
    Xml,
}

/// Rileva la lingua dall'estensione del file.
pub fn detect(path: &Path) -> Option<LanguageId> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "rs" => Some(LanguageId::Rust),
        "toml" => Some(LanguageId::Toml),
        "md" => Some(LanguageId::Markdown),
        "java" => Some(LanguageId::Java),
        "c" | "h" => Some(LanguageId::C),
        "cc" | "cpp" | "cxx" | "hh" | "hpp" | "hxx" => Some(LanguageId::Cpp),
        "cs" => Some(LanguageId::CSharp),
        "py" | "pyi" | "pyw" => Some(LanguageId::Python),
        "js" | "mjs" | "cjs" | "jsx" => Some(LanguageId::JavaScript),
        "ts" | "mts" | "cts" => Some(LanguageId::TypeScript),
        "tsx" => Some(LanguageId::Tsx),
        "json" => Some(LanguageId::Json),
        "yaml" | "yml" => Some(LanguageId::Yaml),
        "html" | "htm" => Some(LanguageId::Html),
        "css" => Some(LanguageId::Css),
        "sql" => Some(LanguageId::Sql),
        "kt" | "kts" => Some(LanguageId::Kotlin),
        "xml" | "xsd" | "xsl" | "xslt" | "svg" => Some(LanguageId::Xml),
        _ => None,
    }
}

pub struct LanguageSpec {
    pub language: Language,
    pub query: Query,
    /// Classe per ogni capture, allineata per indice a `query.capture_names()`.
    pub capture_classes: Vec<TokenClass>,
    /// Query di folding, se la lingua la supporta. `None` significa «nessun
    /// controllo di folding esposto» (la lingua resta senza blocchi).
    pub folding_query: Option<Query>,
    /// Perché la query di folding non compila, se non compila.
    pub folding_error: Option<String>,
}

/// Mappa il nome di una capture alla classe semantica, per prefisso.
fn class_for_capture(name: &str) -> TokenClass {
    use TokenClass::*;
    if name.starts_with("text.title") {
        Keyword
    } else if name.starts_with("text.literal") {
        String
    } else if name.starts_with("text.uri") || name.starts_with("text.reference") {
        Function
    } else if name.starts_with("punctuation.special") {
        Attribute
    } else if name.starts_with("keyword")
        || name.starts_with("label")
        || name.starts_with("storage")
        || name.starts_with("conditional")
        || name.starts_with("repeat")
        || name.starts_with("include")
    {
        Keyword
    } else if name.starts_with("type") || name.starts_with("tag") || name.starts_with("namespace") {
        Type
    } else if name.starts_with("function") || name.starts_with("method") {
        Function
    } else if name.starts_with("string")
        || name.starts_with("escape")
        || name.starts_with("character")
    {
        String
    } else if name.starts_with("number") || name.starts_with("float") || name.starts_with("integer")
    {
        Number
    } else if name.starts_with("comment") {
        Comment
    } else if name.starts_with("attribute") {
        Attribute
    } else if name.starts_with("constant") || name.starts_with("boolean") {
        Constant
    } else if name.starts_with("operator") {
        Operator
    } else if name.starts_with("punctuation") {
        Punctuation
    } else if name.starts_with("property") || name.starts_with("field") {
        Property
    } else if name.starts_with("variable") || name.starts_with("parameter") {
        Variable
    } else {
        Text
    }
}

#[cfg(test)]
fn build_spec(language: Language, query_src: &str) -> Result<LanguageSpec, String> {
    build_spec_with_folding(language, query_src, None)
}

/// Come `build_spec`, ma con una query di folding opzionale. Una query di
/// folding che non compila non fa cadere l'highlight: la lingua resta senza
/// blocchi e il motivo arriva a `folding_error`.
fn build_spec_with_folding(
    language: Language,
    query_src: &str,
    folding_src: Option<&str>,
) -> Result<LanguageSpec, String> {
    // Una query non compilabile (grammatica aggiornata, query no) non deve
    // far cadere l'IDE: la lingua resta senza highlight e il motivo arriva
    // alla barra di stato (`spec_error`): la build di release non ha console.
    let query = Query::new(&language, query_src).map_err(|errore| errore.to_string())?;
    let capture_classes = query
        .capture_names()
        .iter()
        .map(|name| class_for_capture(name))
        .collect();
    let (folding_query, folding_error) = match folding_src {
        None => (None, None),
        Some(src) => match Query::new(&language, src) {
            Ok(q) => (Some(q), None),
            Err(errore) => (None, Some(errore.to_string())),
        },
    };
    Ok(LanguageSpec {
        language,
        query,
        capture_classes,
        folding_query,
        folding_error,
    })
}

/// Compone query compatibili della stessa grammatica, ad esempio base
/// JavaScript + estensioni TypeScript/JSX. La stringa esiste solo durante la
/// costruzione: `Query` mantiene la rappresentazione compilata.
fn build_spec_parts(
    language: Language,
    query_parts: &[&str],
    folding: &str,
) -> Result<LanguageSpec, String> {
    build_spec_with_folding(language, &query_parts.join("\n"), Some(folding))
}

// `tree-sitter-kotlin-ng` espone grammatica nativa aggiornata, ma non una
// query di highlight. Questa query minima usa esclusivamente nodi pubblici
// della grammatica e mantiene il percorso Tree-sitter incrementale.
const KOTLIN_HIGHLIGHTS: &str = r#"
(class_declaration
  "class" @keyword
  name: (identifier) @type)

(function_declaration
  "fun" @keyword
  name: (identifier) @function)

[
  (line_comment)
  (block_comment)
] @comment

[
  (string_literal)
  (multiline_string_literal)
  (character_literal)
] @string

[
  (number_literal)
  (float_literal)
] @number
"#;

/// Query di folding per Rust: cattura i costrutti multilinea. I nodi
/// singola-riga producono regioni vuote e vengono scartati dal produttore.
const RUST_FOLDING_QUERY: &str = r#"
(function_item) @fold
(impl_item) @fold
(struct_item) @fold
(enum_item) @fold
(trait_item) @fold
(mod_item) @fold
(foreign_mod_item) @fold
(macro_definition) @fold
(match_expression) @fold
(if_expression) @fold
(for_expression) @fold
(while_expression) @fold
(loop_expression) @fold
(closure_expression) @fold
(block_comment) @fold
(use_list) @fold
(array_expression) @fold
(tuple_expression) @fold
(block) @fold
"#;

// Una static lazy per lingua: nulla viene costruito finché non si apre
// un file di quella lingua (budget di avvio).
static RUST: LazyLock<Result<LanguageSpec, String>> = LazyLock::new(|| {
    build_spec_with_folding(
        tree_sitter_rust::LANGUAGE.into(),
        tree_sitter_rust::HIGHLIGHTS_QUERY,
        Some(RUST_FOLDING_QUERY),
    )
});

static TOML: LazyLock<Result<LanguageSpec, String>> = LazyLock::new(|| {
    build_spec_with_folding(
        tree_sitter_toml_ng::LANGUAGE.into(),
        tree_sitter_toml_ng::HIGHLIGHTS_QUERY,
        Some(include_str!("../queries/toml_folds.scm")),
    )
});

// Per Markdown si usa la grammatica block-level (niente injection inline).
static MARKDOWN: LazyLock<Result<LanguageSpec, String>> = LazyLock::new(|| {
    build_spec_with_folding(
        tree_sitter_md::LANGUAGE.into(),
        tree_sitter_md::HIGHLIGHT_QUERY_BLOCK,
        Some(include_str!("../queries/markdown_folds.scm")),
    )
});

static JAVA: LazyLock<Result<LanguageSpec, String>> = LazyLock::new(|| {
    build_spec_with_folding(
        tree_sitter_java::LANGUAGE.into(),
        tree_sitter_java::HIGHLIGHTS_QUERY,
        Some(include_str!("../queries/java_folds.scm")),
    )
});

static C: LazyLock<Result<LanguageSpec, String>> = LazyLock::new(|| {
    build_spec_with_folding(
        tree_sitter_c::LANGUAGE.into(),
        tree_sitter_c::HIGHLIGHT_QUERY,
        Some(include_str!("../queries/c_folds.scm")),
    )
});

static CPP: LazyLock<Result<LanguageSpec, String>> = LazyLock::new(|| {
    build_spec_with_folding(
        tree_sitter_cpp::LANGUAGE.into(),
        tree_sitter_cpp::HIGHLIGHT_QUERY,
        Some(include_str!("../queries/cpp_folds.scm")),
    )
});

static C_SHARP: LazyLock<Result<LanguageSpec, String>> = LazyLock::new(|| {
    build_spec_with_folding(
        tree_sitter_c_sharp::LANGUAGE.into(),
        tree_sitter_c_sharp::HIGHLIGHTS_QUERY,
        Some(include_str!("../queries/c_sharp_folds.scm")),
    )
});

static PYTHON: LazyLock<Result<LanguageSpec, String>> = LazyLock::new(|| {
    build_spec_with_folding(
        tree_sitter_python::LANGUAGE.into(),
        tree_sitter_python::HIGHLIGHTS_QUERY,
        Some(include_str!("../queries/python_folds.scm")),
    )
});

// La grammatica JavaScript include JSX; uniamo entrambe le query native.
static JAVASCRIPT: LazyLock<Result<LanguageSpec, String>> = LazyLock::new(|| {
    build_spec_parts(
        tree_sitter_javascript::LANGUAGE.into(),
        &[
            tree_sitter_javascript::HIGHLIGHT_QUERY,
            tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
        ],
        include_str!("../queries/javascript_folds.scm"),
    )
});

// La query TypeScript contiene solo le estensioni del linguaggio: conserva
// quindi la query JavaScript base, più la sua semantica aggiuntiva.
static TYPESCRIPT: LazyLock<Result<LanguageSpec, String>> = LazyLock::new(|| {
    build_spec_parts(
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        &[
            tree_sitter_javascript::HIGHLIGHT_QUERY,
            tree_sitter_typescript::HIGHLIGHTS_QUERY,
        ],
        include_str!("../queries/typescript_folds.scm"),
    )
});

static TSX: LazyLock<Result<LanguageSpec, String>> = LazyLock::new(|| {
    build_spec_parts(
        tree_sitter_typescript::LANGUAGE_TSX.into(),
        &[
            tree_sitter_javascript::HIGHLIGHT_QUERY,
            tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
            tree_sitter_typescript::HIGHLIGHTS_QUERY,
        ],
        include_str!("../queries/tsx_folds.scm"),
    )
});

static JSON: LazyLock<Result<LanguageSpec, String>> = LazyLock::new(|| {
    build_spec_with_folding(
        tree_sitter_json::LANGUAGE.into(),
        tree_sitter_json::HIGHLIGHTS_QUERY,
        Some(include_str!("../queries/json_folds.scm")),
    )
});

static YAML: LazyLock<Result<LanguageSpec, String>> = LazyLock::new(|| {
    build_spec_with_folding(
        tree_sitter_yaml::LANGUAGE.into(),
        tree_sitter_yaml::HIGHLIGHTS_QUERY,
        Some(include_str!("../queries/yaml_folds.scm")),
    )
});

static HTML: LazyLock<Result<LanguageSpec, String>> = LazyLock::new(|| {
    build_spec_with_folding(
        tree_sitter_html::LANGUAGE.into(),
        tree_sitter_html::HIGHLIGHTS_QUERY,
        Some(include_str!("../queries/html_folds.scm")),
    )
});

static CSS: LazyLock<Result<LanguageSpec, String>> = LazyLock::new(|| {
    build_spec_with_folding(
        tree_sitter_css::LANGUAGE.into(),
        tree_sitter_css::HIGHLIGHTS_QUERY,
        Some(include_str!("../queries/css_folds.scm")),
    )
});

static SQL: LazyLock<Result<LanguageSpec, String>> = LazyLock::new(|| {
    build_spec_with_folding(
        tree_sitter_sequel::LANGUAGE.into(),
        tree_sitter_sequel::HIGHLIGHTS_QUERY,
        Some(include_str!("../queries/sql_folds.scm")),
    )
});

static KOTLIN: LazyLock<Result<LanguageSpec, String>> = LazyLock::new(|| {
    build_spec_with_folding(
        tree_sitter_kotlin_ng::LANGUAGE.into(),
        KOTLIN_HIGHLIGHTS,
        Some(include_str!("../queries/kotlin_folds.scm")),
    )
});

static XML: LazyLock<Result<LanguageSpec, String>> = LazyLock::new(|| {
    build_spec_with_folding(
        tree_sitter_xml::LANGUAGE_XML.into(),
        tree_sitter_xml::XML_HIGHLIGHT_QUERY,
        Some(include_str!("../queries/xml_folds.scm")),
    )
});

/// Spec della lingua, `None` se la query di highlight non compila.
fn slot(id: LanguageId) -> Result<&'static LanguageSpec, &'static String> {
    match id {
        LanguageId::Rust => RUST.as_ref(),
        LanguageId::Toml => TOML.as_ref(),
        LanguageId::Markdown => MARKDOWN.as_ref(),
        LanguageId::Java => JAVA.as_ref(),
        LanguageId::C => C.as_ref(),
        LanguageId::Cpp => CPP.as_ref(),
        LanguageId::CSharp => C_SHARP.as_ref(),
        LanguageId::Python => PYTHON.as_ref(),
        LanguageId::JavaScript => JAVASCRIPT.as_ref(),
        LanguageId::TypeScript => TYPESCRIPT.as_ref(),
        LanguageId::Tsx => TSX.as_ref(),
        LanguageId::Json => JSON.as_ref(),
        LanguageId::Yaml => YAML.as_ref(),
        LanguageId::Html => HTML.as_ref(),
        LanguageId::Css => CSS.as_ref(),
        LanguageId::Sql => SQL.as_ref(),
        LanguageId::Kotlin => KOTLIN.as_ref(),
        LanguageId::Xml => XML.as_ref(),
    }
}

/// Spec della lingua, `None` se la query di highlight non compila.
pub fn spec(id: LanguageId) -> Option<&'static LanguageSpec> {
    slot(id).ok()
}

/// Perché la query di highlight della lingua non compila; `None` se compila.
/// Senza console, la diagnostica di compilazione deve essere visibile nell'IDE.
pub fn spec_error(id: LanguageId) -> Option<&'static str> {
    slot(id).err().map(String::as_str)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::{LanguageId, build_spec, spec, spec_error};

    #[test]
    fn una_query_che_non_compila_restituisce_il_motivo() {
        let Err(motivo) = build_spec(
            tree_sitter_rust::LANGUAGE.into(),
            "(nodo_che_non_esiste) @keyword",
        ) else {
            panic!("una query con un nodo inesistente non deve compilare");
        };
        assert!(
            motivo.contains("nodo_che_non_esiste"),
            "il motivo deve dire quale nodo manca, invece è: {motivo}"
        );
    }

    #[test]
    fn ogni_lingua_supportata_ha_una_query_che_compila() {
        use LanguageId::*;
        for id in [
            Rust, Toml, Markdown, Java, C, Cpp, CSharp, Python, JavaScript, TypeScript, Tsx, Json,
            Yaml, Html, Css, Sql, Kotlin, Xml,
        ] {
            assert_eq!(
                spec_error(id),
                None,
                "{id:?}: la query di highlight non compila"
            );
            assert!(spec(id).is_some(), "{id:?}: nessuna spec");
        }
    }

    #[test]
    fn la_query_di_folding_di_rust_compila() {
        let spec = spec(LanguageId::Rust).unwrap();
        assert!(
            spec.folding_query.is_some(),
            "Rust deve avere una query di folding"
        );
        assert!(spec.folding_error.is_none(), "{:?}", spec.folding_error);
    }

    #[test]
    fn tutte_le_lingue_hanno_una_query_di_folding_valida() {
        use LanguageId::*;
        for id in [
            Rust, Toml, Markdown, Java, C, Cpp, CSharp, Python, JavaScript, TypeScript, Tsx, Json,
            Yaml, Html, Css, Sql, Kotlin, Xml,
        ] {
            let spec = spec(id).unwrap();
            assert!(
                spec.folding_query.is_some(),
                "{id:?}: {:?}",
                spec.folding_error
            );
            assert!(
                spec.folding_error.is_none(),
                "{id:?}: {:?}",
                spec.folding_error
            );
        }
    }

    #[test]
    fn una_query_di_folding_che_non_compila_lascia_la_lingua_senza_blocchi() {
        let result = super::build_spec_with_folding(
            tree_sitter_rust::LANGUAGE.into(),
            tree_sitter_rust::HIGHLIGHTS_QUERY,
            Some("(nodo_che_non_esiste) @fold"),
        );
        let spec = result.unwrap();
        assert!(spec.folding_query.is_none());
        assert!(
            spec.folding_error
                .as_deref()
                .unwrap()
                .contains("nodo_che_non_esiste")
        );
    }
}
