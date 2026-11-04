//! Stato applicativo freddo del server MCP. Il server, il trasporto e le
//! connessioni appartengono allo stream creato dalla subscription.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use lume_mcp::server::Endpoint;

/// La sonda Windows degli handle non è stata registrata: mantenere il ponte
/// comune `lume.exe --mcp-stdio` finché la prova reale non giustifica l'exe
/// console separato.
pub const SEPARATE_BRIDGE: bool = false;

pub fn status_text(workspace: Option<&McpWorkspace>) -> Option<String> {
    let workspace = workspace?;
    Some(if workspace.mailbox.is_none() {
        "MCP · avvio…".to_owned()
    } else if workspace.clients.is_empty() {
        "MCP · in ascolto".to_owned()
    } else {
        format!("MCP · {} client", workspace.clients.len())
    })
}

pub fn detail_text(workspace: Option<&McpWorkspace>) -> String {
    let Some(workspace) = workspace else {
        return "non avviato".to_owned();
    };
    let trasporto = workspace.endpoint.describe();
    if workspace.mailbox.is_none() {
        return format!("{trasporto} · avvio…");
    }
    if workspace.clients.is_empty() {
        return format!("{trasporto} · in ascolto · nessun client");
    }
    let names: Vec<&str> = workspace.clients.values().map(String::as_str).collect();
    format!("{trasporto} · {} client: {}", names.len(), names.join(", "))
}

/// Programma e argomenti del server stdio, senza token o pipe effimeri.
pub fn bridge_invocation(exe: &Path) -> (PathBuf, Vec<&'static str>) {
    if SEPARATE_BRIDGE {
        (exe.with_file_name("lume-mcp-bridge.exe"), Vec::new())
    } else {
        (exe.to_path_buf(), vec!["--mcp-stdio"])
    }
}

pub fn claude_code_command(exe: &Path) -> String {
    let (program, args) = bridge_invocation(exe);
    let mut command = format!(
        r#"claude mcp add --transport stdio lume -- "{}""#,
        program.display()
    );
    for arg in args {
        command.push(' ');
        command.push_str(arg);
    }
    command
}

pub fn codex_command(exe: &Path) -> String {
    let (program, args) = bridge_invocation(exe);
    let mut command = format!(r#"codex mcp add lume -- "{}""#, program.display());
    for arg in args {
        command.push(' ');
        command.push_str(arg);
    }
    command
}

pub fn mcp_servers_json(exe: &Path) -> String {
    let (program, args) = bridge_invocation(exe);
    let value = json!({"mcpServers":{"lume":{"command":program.to_string_lossy(),"args":args}}});
    serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string())
}

use lume_mcp::confine::to_slash;
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedSender;

use lume_mcp::{AppQuery, Effect, Mode, Target};

use crate::mcp_gate::Gate;

#[derive(Debug)]
pub enum McpAction {
    Query(AppQuery),
    Rename {
        edit: Box<crate::rename::PreparedEdit>,
        files: Vec<(String, usize)>,
    },
}

#[derive(Debug)]
pub struct RenameAsk {
    pub conn: u64,
    pub new_name: String,
    pub abandoned: bool,
}

pub fn effect_of(query: &AppQuery) -> Effect {
    match query {
        AppQuery::WriteFile { .. } | AppQuery::ReplaceText { .. } | AppQuery::Rename { .. } => {
            Effect::Modify
        }
        AppQuery::RunCommand { .. } => Effect::Execute,
        _ => Effect::Read,
    }
}

pub fn target_of(query: &AppQuery) -> Target {
    let relative = match query {
        AppQuery::WriteFile { relative, .. }
        | AppQuery::ReplaceText { relative, .. }
        | AppQuery::Rename { relative, .. }
        | AppQuery::OpenInEditor { relative, .. }
        | AppQuery::Buffer { relative }
        | AppQuery::Hover { relative, .. }
        | AppQuery::Definition { relative, .. }
        | AppQuery::References { relative, .. }
        | AppQuery::DocumentSymbols { relative } => Some(relative.as_path()),
        AppQuery::Diagnostics { relative, .. } => relative.as_deref(),
        AppQuery::RunCommand { cwd, .. } => Some(cwd.as_path()),
        AppQuery::OpenFiles | AppQuery::ActiveFile => None,
    };
    relative.map(Target::of_relative).unwrap_or(Target::Inside)
}

pub struct McpWorkspace {
    /// Epoca dell'accensione: gli eventi tardivi si scartano.
    pub epoch: u64,
    /// Radice canonica autorizzata dall'utente.
    pub root: PathBuf,
    /// Trasporto reale della piattaforma, con la sua scoperta.
    pub endpoint: Endpoint,
    pub token: lume_mcp::Token,
    pub mailbox: Option<UnboundedSender<lume_mcp::McpCommand>>,
    pub clients: BTreeMap<u64, String>,
    pub sent: lume_mcp::Availability,
    pub lsp_waits: crate::mcp_lsp::Waits,
    pub renames: HashMap<u64, RenameAsk>,
    pub rename_jobs: HashMap<u64, u64>,
    rename_generation: u64,
    pub mode: Mode,
    pub mode_fixed: bool,
    pub gate: Gate,
    pub actions: HashMap<u64, McpAction>,
}

impl McpWorkspace {
    /// `data_dir` è la radice dati di Lume, non la cartella di scoperta:
    /// ogni piattaforma costruisce il proprio endpoint.
    pub fn new(
        epoch: u64,
        root: PathBuf,
        data_dir: Option<PathBuf>,
        availability: lume_mcp::Availability,
    ) -> Result<Self, String> {
        let token = lume_mcp::Token::generate()?;
        #[cfg(windows)]
        let endpoint = {
            let nonce = lume_mcp::discovery::new_nonce()?;
            Endpoint::Pipe {
                name: lume_mcp::discovery::pipe_name(std::process::id(), &nonce),
                discovery_dir: data_dir.as_deref().map(lume_mcp::discovery::discovery_dir),
            }
        };
        #[cfg(unix)]
        let endpoint = {
            let data_dir = data_dir.ok_or_else(|| {
                "cartella dati di Lume non disponibile: il socket Unix non può nascere".to_owned()
            })?;
            Endpoint::Unix(lume_mcp::unix::Endpoint::new(&data_dir)?)
        };
        Ok(Self {
            epoch,
            root,
            endpoint,
            token,
            mailbox: None,
            clients: BTreeMap::new(),
            sent: availability,
            lsp_waits: crate::mcp_lsp::Waits::default(),
            renames: HashMap::new(),
            rename_jobs: HashMap::new(),
            rename_generation: 0,
            mode: Mode::NonBrave,
            mode_fixed: false,
            gate: Gate::default(),
            actions: HashMap::new(),
        })
    }

    pub fn spawn(&self) -> McpSpawn {
        McpSpawn {
            epoch: self.epoch,
            endpoint: self.endpoint.clone(),
            token: self.token.clone(),
            root: self.root.clone(),
            availability: self.sent,
        }
    }

    /// Percorso del record di scoperta dell'accensione corrente (pipe
    /// Windows). Su Unix la guardia possiede già i percorsi.
    #[cfg(windows)]
    pub fn discovery_path(&self) -> Option<PathBuf> {
        match &self.endpoint {
            #[cfg(windows)]
            Endpoint::Pipe { discovery_dir, .. } => discovery_dir
                .as_deref()
                .map(|dir| lume_mcp::discovery::record_path(dir, std::process::id())),
            #[cfg(unix)]
            Endpoint::Unix(endpoint) => Some(endpoint.record_path()),
        }
    }

    /// Pulizia esplicita prima dell'uscita: rimuove solo record e socket
    /// ancora corrispondenti all'identità e agli inode posseduti.
    pub fn cleanup_discovery(&self) {
        match &self.endpoint {
            #[cfg(windows)]
            Endpoint::Pipe { .. } => {
                if let Some(path) = self.discovery_path() {
                    let _ = std::fs::remove_file(path);
                }
            }
            #[cfg(unix)]
            Endpoint::Unix(endpoint) => {
                let _ = lume_mcp::unix::cleanup(endpoint, &self.token);
            }
        }
    }

    pub fn answer(&self, id: u64, answer: Result<lume_mcp::AppAnswer, String>) {
        if let Some(mailbox) = &self.mailbox {
            let _ = mailbox.send(lume_mcp::McpCommand::Answer { id, answer });
        }
    }

    /// Notifica il client solo quando cambia l'insieme dei tool disponibili.
    pub fn sync_tools(&mut self, availability: lume_mcp::Availability) -> bool {
        let Some(mailbox) = &self.mailbox else {
            return false;
        };
        if availability == self.sent {
            return false;
        }
        if mailbox
            .send(lume_mcp::McpCommand::ToolsChanged(availability))
            .is_err()
        {
            return false;
        }
        self.sent = availability;
        true
    }

    /// Registra un nuovo preflight; riusare un request id rende stantio il
    /// risultato del worker precedente con lo stesso id.
    pub fn begin_rename_job(&mut self, request_id: u64) -> Option<u64> {
        let generation = self.rename_generation.checked_add(1)?;
        self.rename_generation = generation;
        self.rename_jobs.insert(request_id, generation);
        Some(generation)
    }

    /// Consuma solo il preflight ancora proprietario dell'id.
    pub fn finish_rename_job(&mut self, request_id: u64, generation: u64) -> bool {
        if self.rename_jobs.get(&request_id) != Some(&generation) {
            return false;
        }
        self.rename_jobs.remove(&request_id);
        true
    }
}

/// Chiave stabile di `Subscription::run_with`: la disponibilità viene
/// sincronizzata a caldo e non deve riavviare il listener.
#[derive(Debug, Clone)]
pub struct McpSpawn {
    pub epoch: u64,
    pub endpoint: Endpoint,
    pub token: lume_mcp::Token,
    pub root: PathBuf,
    pub availability: lume_mcp::Availability,
}

impl Hash for McpSpawn {
    fn hash<H: Hasher>(&self, state: &mut H) {
        "mcp".hash(state);
        self.epoch.hash(state);
    }
}

pub const MAX_SELECTED_CHARS: usize = 65_536;
pub const MAX_DIAGNOSTICS: usize = 500;

/// Converte un percorso assoluto nel suo nome relativo alla radice servita.
pub fn relative_to_root(root: &Path, path: &Path) -> Option<PathBuf> {
    if let Ok(relative) = path.strip_prefix(root) {
        return Some(relative.to_path_buf());
    }
    if cfg!(windows) {
        let mut parts = path.components();
        let same_on_windows = root.components().all(|root_part| {
            parts.next().is_some_and(|part| {
                part.as_os_str()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&root_part.as_os_str().to_string_lossy())
            })
        });
        if same_on_windows {
            return Some(parts.collect());
        }
    }
    // La radice MCP è canonica, il documento può essere stato aperto con un
    // percorso che attraversa un antenato symlink: il prefisso testuale
    // differisce anche se il file è dentro il progetto. Il confronto decisivo
    // è fra percorsi canonici, non fra le loro forme scritte.
    if !root.is_absolute() || !path.is_absolute() {
        return None;
    }
    let root = lume_core::canonical_path_lenient(root).ok()?;
    let path = lume_core::canonical_path_lenient(path).ok()?;
    path.strip_prefix(&root).ok().map(Path::to_path_buf)
}

pub fn same_relative(a: &Path, b: &Path) -> bool {
    let (a, b) = (to_slash(a), to_slash(b));
    if cfg!(windows) {
        a.eq_ignore_ascii_case(&b)
    } else {
        a == b
    }
}

pub fn buffer_snapshot(doc: &lume_core::Document) -> lume_mcp::TextSnapshot {
    let rope = doc.rope();
    let limit = rope.len_bytes().min(lume_mcp::tools::fs::MAX_READ_BYTES);
    let end = rope.byte_to_char(limit);
    lume_mcp::TextSnapshot {
        text: rope.slice(..end).to_string(),
        dirty: doc.is_dirty(),
        truncated: end < rope.len_chars(),
    }
}

pub fn active_file_json(relative: &Path, doc: &lume_core::Document) -> Value {
    let rope = doc.rope();
    let len = rope.len_chars();
    let position = |index: usize| {
        let index = index.min(len);
        let line = rope.char_to_line(index);
        json!({
            "line": line + 1,
            "column": index - rope.line_to_char(line) + 1,
        })
    };
    let selection = doc.selection();
    let (start, end) = (selection.min().min(len), selection.max().min(len));
    let selected: String = rope
        .slice(start..end)
        .chars()
        .take(MAX_SELECTED_CHARS)
        .collect();
    json!({
        "path": to_slash(relative),
        "version": doc.version(),
        "dirty": doc.is_dirty(),
        "anchor": position(selection.anchor),
        "head": position(selection.head),
        "selected_text": selected,
        "selection_truncated": end - start > MAX_SELECTED_CHARS,
    })
}

fn severity(value: Option<lsp_types::DiagnosticSeverity>) -> (u8, &'static str) {
    use lsp_types::DiagnosticSeverity;
    match value {
        Some(value) if value == DiagnosticSeverity::WARNING => (2, "warning"),
        Some(value) if value == DiagnosticSeverity::INFORMATION => (3, "info"),
        Some(value) if value == DiagnosticSeverity::HINT => (4, "hint"),
        _ => (1, "error"),
    }
}

pub fn diagnostics_json(
    root: &Path,
    problems: &[lume_lsp::Problem],
    filter: Option<&Path>,
    max_severity: u8,
) -> Value {
    let mut diagnostics = Vec::new();
    let mut truncated = false;
    for problem in problems {
        let Some(path) = crate::lsp_uri::uri_to_path(&problem.uri) else {
            continue;
        };
        let Some(relative) = relative_to_root(root, &path) else {
            continue;
        };
        if filter.is_some_and(|filter| !same_relative(&relative, filter)) {
            continue;
        }
        let (rank, name) = severity(problem.severity);
        if rank > max_severity {
            continue;
        }
        if diagnostics.len() == MAX_DIAGNOSTICS {
            truncated = true;
            break;
        }
        let code = problem.code.as_ref().map(|code| match code {
            lsp_types::NumberOrString::Number(number) => number.to_string(),
            lsp_types::NumberOrString::String(text) => text.clone(),
        });
        diagnostics.push(json!({
            "path": to_slash(&relative),
            "line": problem.range.start.line + 1,
            "column_utf16": problem.range.start.character + 1,
            "severity": name,
            "source": problem.source,
            "code": code,
            "message": problem.message,
        }));
    }
    json!({"diagnostics": diagnostics, "truncated": truncated})
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use std::hash::Hasher;
    use std::path::Path;

    fn hash(spawn: &McpSpawn) -> u64 {
        let mut hasher = std::hash::DefaultHasher::new();
        spawn.hash(&mut hasher);
        hasher.finish()
    }

    #[test]
    fn effect_of_classifica_lettura_modifica_e_comando() {
        let path = PathBuf::from("src/main.rs");
        assert_eq!(effect_of(&AppQuery::OpenFiles), Effect::Read);
        assert_eq!(
            effect_of(&AppQuery::Hover {
                relative: path.clone(),
                line: 1,
                column: 1
            }),
            Effect::Read
        );
        assert_eq!(
            effect_of(&AppQuery::WriteFile {
                relative: path,
                content: String::new()
            }),
            Effect::Modify
        );
        assert_eq!(
            effect_of(&AppQuery::RunCommand {
                program: "x".into(),
                args: vec![],
                cwd: PathBuf::new(),
                timeout_secs: 1
            }),
            Effect::Execute
        );
    }

    #[test]
    fn target_of_riclassifica_percorso_fuori_radice_e_git() {
        assert_eq!(
            target_of(&AppQuery::WriteFile {
                relative: PathBuf::from("src/a.rs"),
                content: String::new()
            }),
            Target::Inside
        );
        assert_eq!(
            target_of(&AppQuery::WriteFile {
                relative: PathBuf::from("../outside"),
                content: String::new()
            }),
            Target::Outside
        );
        assert_eq!(
            target_of(&AppQuery::WriteFile {
                relative: PathBuf::from(".git/config"),
                content: String::new()
            }),
            Target::GitDir
        );
        assert_eq!(
            target_of(&AppQuery::RunCommand {
                program: "x".into(),
                args: vec![],
                cwd: PathBuf::new(),
                timeout_secs: 1
            }),
            Target::Inside
        );
        assert_eq!(
            target_of(&AppQuery::RunCommand {
                program: "x".into(),
                args: vec![],
                cwd: PathBuf::from(".."),
                timeout_secs: 1
            }),
            Target::Outside
        );
    }

    fn workspace(epoch: u64) -> McpWorkspace {
        // Qui non si tocca il disco: `Endpoint::new` genera solo identità e
        // percorsi. La verifica della catena avviene all'avvio del server.
        McpWorkspace::new(
            epoch,
            PathBuf::from(r"C:\progetto"),
            Some(std::env::temp_dir()),
            lume_mcp::Availability::default(),
        )
        .unwrap()
    }

    #[test]
    fn lo_stream_si_identifica_per_epoca() {
        let a = workspace(1);
        let mut b = workspace(1);
        b.sent = lume_mcp::Availability {
            git: true,
            diagnostics: true,
            ..lume_mcp::Availability::default()
        };
        assert_eq!(hash(&a.spawn()), hash(&b.spawn()));
        assert_ne!(hash(&a.spawn()), hash(&workspace(2).spawn()));
        assert_ne!(a.endpoint, b.endpoint, "endpoint nuovo a ogni accensione");
        assert_ne!(a.token, b.token);
    }

    #[test]
    fn un_preflight_mcp_tardivo_non_consumo_la_generazione_riusata() {
        let mut state = workspace(7);
        let old = state.begin_rename_job(42).unwrap();
        let current = state.begin_rename_job(42).unwrap();
        assert_ne!(old, current);
        assert!(!state.finish_rename_job(42, old));
        assert!(state.finish_rename_job(42, current));
        assert!(!state.finish_rename_job(42, current));
    }

    #[test]
    fn il_token_non_compare_nel_debug_dello_spawn() {
        let accensione = workspace(1);
        let testo = format!("{:?}", accensione.spawn());
        assert!(!testo.contains(&accensione.token.to_hex()));
        assert!(testo.contains("redatto"));
    }

    #[test]
    fn spia_e_riga_dicono_solo_cose_vere() {
        assert_eq!(status_text(None), None);
        assert_eq!(detail_text(None), "non avviato");
        let mut acceso = workspace(1);
        let trasporto = acceso.endpoint.describe();
        assert_eq!(status_text(Some(&acceso)).as_deref(), Some("MCP · avvio…"));
        assert_eq!(detail_text(Some(&acceso)), format!("{trasporto} · avvio…"));
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        acceso.mailbox = Some(tx);
        assert_eq!(
            status_text(Some(&acceso)).as_deref(),
            Some("MCP · in ascolto")
        );
        assert_eq!(
            detail_text(Some(&acceso)),
            format!("{trasporto} · in ascolto · nessun client")
        );
        acceso.clients.insert(1, "claude-code".to_owned());
        acceso.clients.insert(2, "codex".to_owned());
        assert_eq!(
            status_text(Some(&acceso)).as_deref(),
            Some("MCP · 2 client")
        );
        assert_eq!(
            detail_text(Some(&acceso)),
            format!("{trasporto} · 2 client: claude-code, codex")
        );
    }

    #[test]
    fn la_configurazione_copiata_regge_spazi_e_accenti() {
        let exe = Path::new(r"C:\Program Files\Lume è\lume.exe");
        let (programma, argomenti) = bridge_invocation(exe);
        let comando = claude_code_command(exe);
        assert!(comando.starts_with(&format!(
            r#"claude mcp add --transport stdio lume -- "{}""#,
            programma.display()
        )));
        for argomento in &argomenti {
            assert!(comando.ends_with(argomento) || comando.contains(&format!(" {argomento}")));
        }
        let codex = codex_command(exe);
        assert!(codex.starts_with(&format!(
            r#"codex mcp add lume -- "{}""#,
            programma.display()
        )));
        assert_eq!(
            codex.strip_prefix("codex mcp add lume -- "),
            comando.strip_prefix("claude mcp add --transport stdio lume -- "),
        );
        let json: serde_json::Value = serde_json::from_str(&mcp_servers_json(exe)).unwrap();
        assert_eq!(
            json["mcpServers"]["lume"]["command"],
            programma.to_string_lossy().as_ref()
        );
        assert_eq!(
            json["mcpServers"]["lume"]["args"],
            serde_json::json!(argomenti)
        );
        if !SEPARATE_BRIDGE {
            assert_eq!(programma, exe);
            assert_eq!(argomenti, ["--mcp-stdio"]);
        }
    }

    #[test]
    fn sync_tools_aspetta_la_mailbox_e_manda_solo_i_cambi() {
        let mut accensione = workspace(1);
        let git = lume_mcp::Availability {
            git: true,
            diagnostics: false,
            ..lume_mcp::Availability::default()
        };
        assert!(!accensione.sync_tools(git));
        assert_eq!(accensione.sent, lume_mcp::Availability::default());
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        accensione.mailbox = Some(tx);
        assert!(accensione.sync_tools(git));
        assert!(!accensione.sync_tools(git));
        assert!(matches!(
            rx.try_recv(),
            Ok(lume_mcp::McpCommand::ToolsChanged(value)) if value == git
        ));
        assert!(rx.try_recv().is_err());
    }

    #[cfg(unix)]
    #[test]
    fn la_radice_si_riconosce_attraverso_un_antenato_symlink() {
        let cartella = tempfile::tempdir().unwrap();
        let radice = lume_core::canonical_path(cartella.path())
            .unwrap()
            .join("progetto");
        std::fs::create_dir_all(radice.join("src")).unwrap();
        std::fs::write(radice.join("src/a.rs"), b"fn a() {}\n").unwrap();
        let link = cartella.path().join("alias");
        std::os::unix::fs::symlink(&radice, &link).unwrap();
        assert_eq!(
            relative_to_root(&radice, &link.join("src/a.rs")),
            Some(PathBuf::from("src/a.rs"))
        );
        // Un ultimo tratto non ancora creato resta relativo.
        assert_eq!(
            relative_to_root(&radice, &link.join("src/nuovo.rs")),
            Some(PathBuf::from("src/nuovo.rs"))
        );
        assert_eq!(
            relative_to_root(&radice, &link.join("..").join("fuori.rs")),
            None
        );
    }

    #[cfg(windows)]
    #[test]
    fn la_radice_si_riconosce_anche_con_la_lettera_unita_diversa() {
        let root = Path::new(r"C:\Lavoro\lume");
        assert_eq!(
            relative_to_root(root, Path::new(r"C:\Lavoro\lume\src\a.rs")),
            Some(PathBuf::from(r"src\a.rs"))
        );
        assert_eq!(relative_to_root(root, Path::new(r"C:\altro\a.rs")), None);
        assert_eq!(
            relative_to_root(root, Path::new(r"c:\lavoro\lume\a.rs")),
            Some(PathBuf::from("a.rs"))
        );
        assert!(same_relative(Path::new(r"SRC\A.rs"), Path::new("src/a.rs")));
    }

    #[test]
    fn il_buffer_si_taglia_senza_spezzare_un_carattere() {
        let limit = lume_mcp::tools::fs::MAX_READ_BYTES;
        let mut text = "a".repeat(limit - 1);
        text.push_str("èfine");
        let doc = lume_core::Document::from_str(&text, None, lume_core::DocId(1));
        let snapshot = buffer_snapshot(&doc);
        assert!(snapshot.truncated);
        assert_eq!(snapshot.text, "a".repeat(limit - 1));
        let short = buffer_snapshot(&lume_core::Document::from_str(
            "ciao\n",
            None,
            lume_core::DocId(2),
        ));
        assert!(!short.truncated);
        assert_eq!(short.text, "ciao\n");
    }

    #[test]
    fn la_selezione_usa_caratteri_unicode_e_righe_da_uno() {
        let mut doc = lume_core::Document::from_str("perché\nciao\n", None, lume_core::DocId(1));
        doc.set_selection(lume_core::Range::new(3, 9));
        let value = active_file_json(Path::new("src/a.rs"), &doc);
        assert_eq!(value["path"], "src/a.rs");
        assert_eq!(value["anchor"], serde_json::json!({"line": 1, "column": 4}));
        assert_eq!(value["head"], serde_json::json!({"line": 2, "column": 3}));
        assert_eq!(value["selected_text"], "ché\nci");
        assert_eq!(value["selection_truncated"], false);
    }
}
