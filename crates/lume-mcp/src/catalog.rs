//! I tool della v1, i loro schemi e chi li esegue.
//!
//! **Zero-mock anche qui** (README, «La regola zero-mock»): un tool il cui
//! backend è spento non compare in `tools/list`. Git disabilitato → niente
//! `git_*`; nessun language server attivo → niente `get_diagnostics`.
//! Rispondere «vuoto» sarebbe un tool che finge di aver guardato.
//!
//! # Il cancello
//!
//! Ogni tool dichiara il suo effetto. `crate::gate::decide` riceve modalità,
//! effetto e bersaglio verificato da chi possiede il percorso. Le annotazioni
//! MCP sono suggerimenti per il client e non sostituiscono questa decisione.

use serde_json::{Map, Value, json};

use crate::gate::Effect;

/// Gli argomenti di una `tools/call`: sempre un oggetto JSON.
pub type Args = Map<String, Value>;

/// Un errore del tool, non del protocollo: torna al client come risultato
/// con `isError: true`, così l'agente può correggere la richiesta.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolError(pub String);

impl ToolError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Quali backend sono accesi in questo momento. Lo decide l'app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Availability {
    pub git: bool,
    pub diagnostics: bool,
    pub hover: bool,
    pub definition: bool,
    pub references: bool,
    pub symbols: bool,
    pub rename: bool,
}

/// Chi esegue il tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// File system e Git operano nello stream, con la radice del progetto.
    Stream,
    /// Documenti e stato dell'editor operano nell'app.
    App,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ToolName {
    ListOpenFiles,
    GetActiveFile,
    ReadFile,
    ListFiles,
    SearchText,
    GetDiagnostics,
    GetHover,
    GotoDefinition,
    FindUsages,
    GetDocumentSymbols,
    WriteFile,
    ReplaceText,
    RunCommand,
    Rename,
    GitStatus,
    GitDiffFile,
    GitLog,
    GitBlame,
    OpenInEditor,
}

impl ToolName {
    pub const ALL: [Self; 19] = [
        Self::ListOpenFiles,
        Self::GetActiveFile,
        Self::ReadFile,
        Self::ListFiles,
        Self::SearchText,
        Self::GetDiagnostics,
        Self::GetHover,
        Self::GotoDefinition,
        Self::FindUsages,
        Self::GetDocumentSymbols,
        Self::WriteFile,
        Self::ReplaceText,
        Self::RunCommand,
        Self::Rename,
        Self::GitStatus,
        Self::GitDiffFile,
        Self::GitLog,
        Self::GitBlame,
        Self::OpenInEditor,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ListOpenFiles => "list_open_files",
            Self::GetActiveFile => "get_active_file",
            Self::ReadFile => "read_file",
            Self::ListFiles => "list_files",
            Self::SearchText => "search_text",
            Self::GetDiagnostics => "get_diagnostics",
            Self::GetHover => "get_hover",
            Self::GotoDefinition => "goto_definition",
            Self::FindUsages => "find_usages",
            Self::GetDocumentSymbols => "get_document_symbols",
            Self::WriteFile => "write_file",
            Self::ReplaceText => "replace_text",
            Self::RunCommand => "run_command",
            Self::Rename => "rename",
            Self::GitStatus => "git_status",
            Self::GitDiffFile => "git_diff_file",
            Self::GitLog => "git_log",
            Self::GitBlame => "git_blame",
            Self::OpenInEditor => "open_in_editor",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|tool| tool.as_str() == name)
    }

    /// `read_file` è `Stream` anche se può chiedere all'app il buffer aperto:
    /// confinamento e lettura da disco stanno nello stream.
    pub const fn route(self) -> Route {
        match self {
            Self::ListOpenFiles
            | Self::GetActiveFile
            | Self::GetDiagnostics
            | Self::GetHover
            | Self::GotoDefinition
            | Self::FindUsages
            | Self::GetDocumentSymbols
            | Self::WriteFile
            | Self::ReplaceText
            | Self::Rename
            | Self::OpenInEditor => Route::App,
            Self::ReadFile
            | Self::ListFiles
            | Self::SearchText
            | Self::GitStatus
            | Self::GitDiffFile
            | Self::GitLog
            | Self::GitBlame => Route::Stream,
            Self::RunCommand => Route::Stream,
        }
    }

    pub const fn effect(self) -> Effect {
        match self {
            Self::WriteFile | Self::ReplaceText | Self::Rename => Effect::Modify,
            Self::RunCommand => Effect::Execute,
            _ => Effect::Read,
        }
    }

    pub const fn is_available(self, availability: Availability) -> bool {
        match self {
            Self::GitStatus | Self::GitDiffFile | Self::GitLog | Self::GitBlame => availability.git,
            Self::GetDiagnostics => availability.diagnostics,
            Self::GetHover => availability.hover,
            Self::GotoDefinition => availability.definition,
            Self::FindUsages => availability.references,
            Self::GetDocumentSymbols => availability.symbols,
            Self::Rename => availability.rename,
            // La decisione M-B richiede isolamento OS che neghi rete DB,
            // vault e controllo UI/IPC. Fino a prove native adeguate il tool
            // arbitrario resta nel catalogo interno ma non viene pubblicato.
            Self::RunCommand => false,
            Self::ListOpenFiles
            | Self::GetActiveFile
            | Self::ReadFile
            | Self::ListFiles
            | Self::SearchText
            | Self::OpenInEditor
            | Self::WriteFile
            | Self::ReplaceText => true,
        }
    }

    pub fn definition(self) -> Value {
        json!({
            "name": self.as_str(),
            "description": self.description(),
            "inputSchema": self.input_schema(),
            "annotations": self.annotations(),
        })
    }

    fn annotations(self) -> Value {
        let reads = matches!(self.effect(), Effect::Read);
        json!({
            "readOnlyHint": reads,
            "destructiveHint": !reads,
            "openWorldHint": self == Self::RunCommand,
        })
    }

    const fn description(self) -> &'static str {
        match self {
            Self::ListOpenFiles => {
                "Elenca i file aperti nell'editor di Lume: percorso relativo alla radice, versione, se ha modifiche non salvate (dirty) e se è la scheda attiva."
            }
            Self::GetActiveFile => {
                "Il file attivo nell'editor: percorso, riga e colonna (1-based, in caratteri) di anchor e head della selezione, testo selezionato."
            }
            Self::ReadFile => {
                "Legge un file del progetto. Se è aperto nell'editor restituisce il buffer, anche con modifiche non salvate (dirty: true); altrimenti il disco. Al massimo 1 MiB (truncated). Righe numerate e intervallo opzionali."
            }
            Self::ListFiles => {
                "Elenca i file del progetto rispettando .gitignore, senza .git. Filtri opzionali: sottocartella (path) e glob. Al massimo limit file (predefinito 500)."
            }
            Self::SearchText => {
                "Cerca testo o regex nei file del progetto sul disco (non nei buffer non salvati), rispettando .gitignore; salta i file oltre 1 MiB e i binari; al massimo 2000 occorrenze."
            }
            Self::GetDiagnostics => {
                "Le diagnostiche pubblicate dai language server attivi: file, riga, colonna (UTF-16 come LSP), gravità, messaggio. Filtri opzionali per file e gravità minima."
            }
            Self::GetHover => {
                "Il testo di aiuto del language server per il simbolo alla posizione indicata. Il file dev'essere aperto nell'editor; line e column sono 1-based e column usa unità UTF-16."
            }
            Self::GotoDefinition => {
                "Le definizioni del simbolo alla posizione indicata, relative alla radice del progetto quando possibile. Il file dev'essere aperto nell'editor; line e column sono 1-based e column usa unità UTF-16."
            }
            Self::FindUsages => {
                "Gli utilizzi del simbolo alla posizione indicata, fino a 200 risultati. Il file dev'essere aperto nell'editor; line e column sono 1-based e column usa unità UTF-16."
            }
            Self::GetDocumentSymbols => {
                "La struttura del file secondo il language server. Il file dev'essere aperto nell'editor."
            }
            Self::WriteFile => {
                "Scrive un file del progetto, creandolo o sovrascrivendolo (massimo 1 MiB). La cartella madre deve esistere. Le modifiche non salvate nell'editor impediscono la scrittura. Può chiedere conferma; non scrive fuori dalla radice né sotto .git."
            }
            Self::ReplaceText => {
                "Sostituisce old_text con new_text nel file. Senza replace_all old_text deve comparire una volta sola. Le modifiche non salvate impediscono la sostituzione. Può chiedere conferma; non scrive fuori dalla radice né sotto .git."
            }
            Self::RunCommand => {
                "Esegue un programma e argomenti separati (nessuna shell implicita) nella cartella del progetto. Timeout predefinito 60 s, massimo 600 s; restituisce gli ultimi 64 KiB di stdout e stderr. Chiede conferma tranne in modalità Brave; al timeout termina l'albero di processi."
            }
            Self::Rename => {
                "Rinomina il simbolo con il language server e applica gli edit ai buffer senza salvare. Può chiedere conferma; rifiuta edit fuori radice o sotto .git. Il file deve essere aperto nell'editor."
            }
            Self::GitStatus => {
                "Stato Git dei file sotto la radice del progetto: branch corrente e, per file, modifica nell'index e nel worktree."
            }
            Self::GitDiffFile => {
                "Diff unified di un file: base worktree (modifiche non in stage), index (in stage) o commit (contro il suo primo parent)."
            }
            Self::GitLog => {
                "Log dei commit da un riferimento (predefinito HEAD), al massimo 200; con include_files anche i file toccati (al massimo 20 commit)."
            }
            Self::GitBlame => {
                "Blame di un file contro HEAD: per riga commit, autore e data; dirty dice se il file ha modifiche non committate."
            }
            Self::OpenInEditor => {
                "Apre un file del progetto nell'editor di Lume, opzionalmente alla riga indicata (1-based)."
            }
        }
    }

    fn input_schema(self) -> Value {
        let path = json!({
            "type": "string",
            "description": "Percorso relativo alla radice del progetto (separatore / o \\)"
        });
        let line = json!({"type": "integer", "minimum": 1, "description": "Riga, 1-based"});
        let column = json!({"type": "integer", "minimum": 1, "description": "Colonna, 1-based, in unità UTF-16 (come LSP)"});
        match self {
            Self::ListOpenFiles | Self::GetActiveFile | Self::GitStatus => {
                json!({"type": "object", "properties": {}})
            }
            Self::ReadFile => json!({
                "type": "object",
                "properties": {
                    "path": path,
                    "start_line": {"type": "integer", "minimum": 1},
                    "end_line": {"type": "integer", "minimum": 1},
                    "line_numbers": {"type": "boolean"}
                },
                "required": ["path"]
            }),
            Self::ListFiles => json!({
                "type": "object",
                "properties": {
                    "path": path,
                    "glob": {"type": "string", "description": "Glob in stile .gitignore, es. *.rs o src/**/*.toml"},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 5000}
                }
            }),
            Self::SearchText => json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string"},
                    "regex": {"type": "boolean"},
                    "case_sensitive": {"type": "boolean", "description": "Assente: le maiuscole contano solo se la query ne contiene"},
                    "whole_word": {"type": "boolean"},
                    "path": path
                },
                "required": ["query"]
            }),
            Self::GetDiagnostics => json!({
                "type": "object",
                "properties": {
                    "path": path,
                    "severity": {"type": "string", "enum": ["error", "warning", "info", "hint"], "description": "Gravità minima"}
                }
            }),
            Self::GetHover | Self::GotoDefinition | Self::FindUsages => json!({
                "type": "object",
                "properties": {"path": path, "line": line, "column": column},
                "required": ["path", "line", "column"]
            }),
            Self::GetDocumentSymbols => json!({
                "type": "object",
                "properties": {"path": path},
                "required": ["path"]
            }),
            Self::WriteFile => json!({
                "type": "object",
                "properties": {
                    "path": path,
                    "content": {"type": "string", "description": "Testo UTF-8, massimo 1 MiB"}
                },
                "required": ["path", "content"]
            }),
            Self::ReplaceText => json!({
                "type": "object",
                "properties": {
                    "path": path,
                    "old_text": {"type": "string"},
                    "new_text": {"type": "string"},
                    "replace_all": {"type": "boolean", "default": false}
                },
                "required": ["path", "old_text", "new_text"]
            }),
            Self::RunCommand => json!({
                "type": "object",
                "properties": {
                    "program": {"type": "string"},
                    "args": {"type": "array", "items": {"type": "string"}},
                    "cwd": {"type": "string", "description": "Directory relativa alla radice; default ."},
                    "timeout_secs": {"type": "integer", "minimum": 1, "maximum": crate::exec::MAX_TIMEOUT_SECS}
                },
                "required": ["program"]
            }),
            Self::Rename => json!({
                "type": "object",
                "properties": {
                    "path": path,
                    "line": line,
                    "column": column,
                    "new_name": {"type": "string"}
                },
                "required": ["path", "line", "column", "new_name"]
            }),
            Self::GitDiffFile => json!({
                "type": "object",
                "properties": {
                    "path": path,
                    "base": {"type": "string", "enum": ["worktree", "index", "commit"]},
                    "commit": {"type": "string", "description": "Hash completo, solo con base = commit"}
                },
                "required": ["path"]
            }),
            Self::GitLog => json!({
                "type": "object",
                "properties": {
                    "reference": {"type": "string"},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 200},
                    "include_files": {"type": "boolean"}
                }
            }),
            Self::GitBlame => json!({
                "type": "object",
                "properties": {"path": path},
                "required": ["path"]
            }),
            Self::OpenInEditor => json!({
                "type": "object",
                "properties": {"path": path, "line": {"type": "integer", "minimum": 1}},
                "required": ["path"]
            }),
        }
    }
}

pub fn tools_list(availability: Availability) -> Value {
    let tools: Vec<Value> = ToolName::ALL
        .into_iter()
        .filter(|tool| tool.is_available(availability))
        .map(ToolName::definition)
        .collect();
    json!({ "tools": tools })
}

pub fn arg_str<'a>(args: &'a Args, key: &str) -> Result<&'a str, ToolError> {
    arg_opt_str(args, key)?.ok_or_else(|| ToolError::new(format!("manca l'argomento «{key}»")))
}

pub fn arg_opt_str<'a>(args: &'a Args, key: &str) -> Result<Option<&'a str>, ToolError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.as_str())),
        Some(_) => Err(ToolError::new(format!("«{key}» deve essere una stringa"))),
    }
}

pub fn arg_opt_u64(args: &Args, key: &str) -> Result<Option<u64>, ToolError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| ToolError::new(format!("«{key}» deve essere un intero non negativo"))),
    }
}

pub fn arg_opt_str_array(args: &Args, key: &str) -> Result<Option<Vec<String>>, ToolError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(values)) => values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                value.as_str().map(str::to_owned).ok_or_else(|| {
                    ToolError::new(format!("«{key}[{index}]» deve essere una stringa"))
                })
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        Some(_) => Err(ToolError::new(format!(
            "«{key}» deve essere un array di stringhe"
        ))),
    }
}

/// La posizione LSP nel catalogo è 1-based; `u32` evita troncamenti quando
/// il valore diventa una posizione di protocollo.
pub fn arg_position(args: &Args) -> Result<(u32, u32), ToolError> {
    let one_based = |key: &str| -> Result<u32, ToolError> {
        let value = arg_opt_u64(args, key)?
            .ok_or_else(|| ToolError::new(format!("manca l'argomento «{key}»")))?;
        match u32::try_from(value) {
            Ok(value) if value > 0 => Ok(value),
            _ => Err(ToolError::new(format!(
                "«{key}» parte da 1 e sta in 32 bit"
            ))),
        }
    };
    Ok((one_based("line")?, one_based("column")?))
}

pub fn arg_opt_bool(args: &Args, key: &str) -> Result<Option<bool>, ToolError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(flag)) => Ok(Some(*flag)),
        Some(_) => Err(ToolError::new(format!("«{key}» deve essere true o false"))),
    }
}

pub fn arg_bool(args: &Args, key: &str) -> Result<bool, ToolError> {
    Ok(arg_opt_bool(args, key)?.unwrap_or(false))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use serde_json::{Value, json};

    fn nomi(lista: &Value) -> Vec<String> {
        lista["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap().to_owned())
            .collect()
    }

    fn tutto_acceso() -> Availability {
        Availability {
            git: true,
            diagnostics: true,
            hover: true,
            definition: true,
            references: true,
            symbols: true,
            rename: true,
        }
    }

    #[test]
    fn senza_git_ne_lsp_restano_i_tool_con_motore_locale() {
        let expected = vec![
            "list_open_files",
            "get_active_file",
            "read_file",
            "list_files",
            "search_text",
            "write_file",
            "replace_text",
            "open_in_editor",
        ];
        let lista = tools_list(Availability::default());
        assert_eq!(nomi(&lista), expected);
    }

    #[test]
    fn run_command_rimane_nascosto_senza_isolamento() {
        assert!(!ToolName::RunCommand.is_available(tutto_acceso()));
        assert!(!nomi(&tools_list(tutto_acceso())).contains(&"run_command".to_owned()));
    }

    #[test]
    fn con_tutto_acceso_ci_sono_al_massimo_diciotto_tool() {
        let names = nomi(&tools_list(tutto_acceso()));
        assert_eq!(names.len(), 18);
        assert_eq!(ToolName::ALL.len(), 19);
        for name in [
            "git_blame",
            "get_diagnostics",
            "get_hover",
            "goto_definition",
            "find_usages",
            "get_document_symbols",
            "write_file",
            "replace_text",
            "rename",
        ] {
            assert!(names.contains(&name.to_owned()), "{name}");
        }
    }

    #[test]
    fn con_git_e_diagnostiche_il_conteggio_dinamico_corrisponde() {
        let lista = tools_list(Availability {
            git: true,
            diagnostics: true,
            ..Availability::default()
        });
        let nomi = nomi(&lista);
        assert_eq!(nomi.len(), 13);
        assert!(nomi.contains(&"git_blame".to_owned()));
        assert!(nomi.contains(&"get_diagnostics".to_owned()));
    }

    #[test]
    fn ogni_tool_lsp_richiede_la_propria_capability() {
        let cases = [
            (
                "get_hover",
                Availability {
                    hover: true,
                    ..Availability::default()
                },
            ),
            (
                "goto_definition",
                Availability {
                    definition: true,
                    ..Availability::default()
                },
            ),
            (
                "find_usages",
                Availability {
                    references: true,
                    ..Availability::default()
                },
            ),
            (
                "get_document_symbols",
                Availability {
                    symbols: true,
                    ..Availability::default()
                },
            ),
            (
                "rename",
                Availability {
                    rename: true,
                    ..Availability::default()
                },
            ),
        ];
        let unavailable = nomi(&tools_list(Availability::default()));
        for (name, availability) in cases {
            let available = nomi(&tools_list(availability));
            assert!(available.contains(&name.to_owned()), "{name}");
            assert!(
                !unavailable.contains(&name.to_owned()),
                "{name} without capability"
            );
        }
    }

    #[test]
    fn ogni_definizione_ha_nome_descrizione_e_schema_oggetto() {
        for tool in ToolName::ALL {
            let definizione = tool.definition();
            assert_eq!(definizione["name"], tool.as_str());
            assert!(
                !definizione["description"].as_str().unwrap().is_empty(),
                "{tool:?}"
            );
            assert_eq!(definizione["inputSchema"]["type"], "object", "{tool:?}");
            assert_eq!(ToolName::from_name(tool.as_str()), Some(tool));
        }
        for nome in ["write_file", "replace_text", "run_command", "rename"] {
            assert!(ToolName::from_name(nome).is_some(), "{nome}");
        }
        for nome in [
            "get_hover",
            "goto_definition",
            "find_usages",
            "get_document_symbols",
        ] {
            assert!(ToolName::from_name(nome).is_some(), "{nome}");
        }
    }

    #[test]
    fn chi_esegue_cosa() {
        assert_eq!(ToolName::ListFiles.route(), Route::Stream);
        assert_eq!(ToolName::ReadFile.route(), Route::Stream);
        assert_eq!(ToolName::GetActiveFile.route(), Route::App);
        assert_eq!(ToolName::OpenInEditor.route(), Route::App);
        assert_eq!(ToolName::WriteFile.route(), Route::App);
        assert_eq!(ToolName::ReplaceText.route(), Route::App);
        assert_eq!(ToolName::Rename.route(), Route::App);
        assert_eq!(ToolName::RunCommand.route(), Route::Stream);
    }

    #[test]
    fn gli_effetti_e_le_annotazioni_coincidono() {
        for tool in ToolName::ALL {
            let annotations = tool.definition()["annotations"].clone();
            let read = tool.effect() == Effect::Read;
            assert_eq!(annotations["readOnlyHint"], read, "{tool:?}");
            assert_eq!(annotations["destructiveHint"], !read, "{tool:?}");
            assert_eq!(
                annotations["openWorldHint"],
                tool == ToolName::RunCommand,
                "{tool:?}"
            );
        }
    }

    #[test]
    fn gli_argomenti_si_leggono_col_tipo_giusto() {
        let args: Args = serde_json::from_value(json!({
            "path": "src/a.rs", "limit": 3, "regex": true, "nullo": null, "sbagliato": 7
        }))
        .unwrap();
        assert_eq!(arg_str(&args, "path").unwrap(), "src/a.rs");
        assert_eq!(arg_opt_u64(&args, "limit").unwrap(), Some(3));
        assert!(arg_bool(&args, "regex").unwrap());
        assert!(!arg_bool(&args, "assente").unwrap());
        assert_eq!(arg_opt_bool(&args, "assente").unwrap(), None);
        assert_eq!(arg_opt_str(&args, "nullo").unwrap(), None);
        assert!(arg_str(&args, "assente").is_err());
        assert!(arg_opt_str(&args, "sbagliato").is_err());
        assert!(arg_opt_u64(&args, "path").is_err());
    }

    #[test]
    fn le_posizioni_hanno_riga_colonna_positive_e_senza_troncamenti() {
        let args: Args = serde_json::from_value(json!({"line": 3, "column": 7})).unwrap();
        assert_eq!(arg_position(&args).unwrap(), (3, 7));
        for invalid in [
            json!({"line": 0, "column": 1}),
            json!({"line": 1}),
            json!({"line": 1, "column": 5_000_000_000_u64}),
        ] {
            let args: Args = serde_json::from_value(invalid).unwrap();
            assert!(arg_position(&args).is_err());
        }
    }

    #[test]
    fn array_stringhe_timeout_e_campi_richiesti_hanno_schema_corretto() {
        let args: Args = serde_json::from_value(json!({
            "args": ["/C", "echo"], "mixed": ["a", 1], "scalar": "a"
        }))
        .unwrap();
        assert_eq!(
            arg_opt_str_array(&args, "args").unwrap(),
            Some(vec!["/C".into(), "echo".into()])
        );
        assert_eq!(arg_opt_str_array(&args, "missing").unwrap(), None);
        assert!(arg_opt_str_array(&args, "mixed").is_err());
        assert!(arg_opt_str_array(&args, "scalar").is_err());
        let command = ToolName::RunCommand.definition()["inputSchema"].clone();
        assert_eq!(
            command["properties"]["timeout_secs"]["maximum"],
            crate::exec::MAX_TIMEOUT_SECS
        );
        assert_eq!(command["properties"]["args"]["items"]["type"], "string");
        let required = |tool: ToolName| -> Vec<String> {
            tool.definition()["inputSchema"]["required"]
                .as_array()
                .unwrap()
                .iter()
                .map(|field| field.as_str().unwrap().to_owned())
                .collect()
        };
        assert_eq!(required(ToolName::WriteFile), ["path", "content"]);
        assert_eq!(
            required(ToolName::ReplaceText),
            ["path", "old_text", "new_text"]
        );
        assert_eq!(required(ToolName::RunCommand), ["program"]);
        assert_eq!(
            required(ToolName::Rename),
            ["path", "line", "column", "new_name"]
        );
    }
}
