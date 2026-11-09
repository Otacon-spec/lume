//! I tool concreti registrati nel Registry. Ognuno è un modulo autonomo:
//! icona + pannello + stato + eventi propri (principio: un'icona = una classe).

pub mod bookmarks;
#[cfg(feature = "sql")]
pub mod db;
pub mod debug;
pub mod ext;
pub mod git_native;
pub mod gitlog;
pub mod history;
pub mod jj;
pub mod maven;
pub mod notes;
pub mod problems;
pub mod project;
pub mod search_tool;
pub mod structure;

pub mod terminal;
pub mod usages;
use lume_ui::{Registry, Side, ToolId};

pub fn persisted_id(value: &str) -> Option<ToolId> {
    match value {
        "project" => Some(project::ID),
        "search" => Some(search_tool::ID),
        "git" => Some(git_native::ID),
        "terminal" => Some(terminal::ID),
        "gitlog" => Some(gitlog::ID),
        "problems" => Some(problems::ID),
        "bookmarks" => Some(bookmarks::ID),
        "history" => Some(history::ID),
        "notes" => Some(notes::ID),
        #[cfg(feature = "sql")]
        "db" => Some(db::ID),
        "ext" => Some(ext::ID),
        "structure" => Some(structure::ID),
        "usages" => Some(usages::ID),
        "jj" => Some(jj::ID),
        "debug" => Some(debug::ID),
        "maven" => Some(maven::ID),
        _ => None,
    }
}

/// Registra solo tool con backend reale. I tool incompleti non entrano nella UI.
pub fn default_registry() -> (Registry, std::sync::Arc<tokio::sync::Notify>) {
    let mut r = Registry::new();
    r.register(Box::new(project::ProjectTool::new()), Side::Left);
    r.register(Box::new(search_tool::SearchTool::new()), Side::Left);
    r.register(Box::new(git_native::GitTool::new()), Side::Left);
    let (terminal, terminal_wakeup) = terminal::TerminalTool::new();
    r.register(Box::new(terminal), Side::Left);
    r.register(Box::new(gitlog::GitLogTool::new()), Side::Left);
    #[cfg(feature = "sql")]
    r.register(Box::new(db::DbTool::new()), Side::Left);
    r.register(Box::new(problems::ProblemsTool::new()), Side::Left);
    r.register(Box::new(ext::ExtensionsTool::new()), Side::Left);
    r.register(Box::new(usages::UsagesTool::new()), Side::Left);
    r.register(Box::new(jj::JjTool::new()), Side::Left);
    let _ = r.set_unavailable(jj::ID, true);
    r.register(Box::new(debug::DebugTool::new()), Side::Left);
    r.register(Box::new(bookmarks::BookmarksTool::new()), Side::Right);
    r.register(Box::new(history::HistoryTool::new()), Side::Right);
    r.register(Box::new(notes::NotesTool::new()), Side::Right);
    r.register(Box::new(structure::StructureTool::new()), Side::Right);
    // Compare solo con un pom.xml nella radice (`sync_maven_tool`).
    r.register(Box::new(maven::MavenTool::new()), Side::Right);
    let _ = r.set_unavailable(maven::ID, true);
    (r, terminal_wakeup)
}

#[cfg(test)]
mod notes_tool_tests {
    use super::*;

    #[test]
    fn notes_tool_keeps_stable_identity_and_can_be_enabled() {
        let (mut registry, _terminal_wakeup) = default_registry();

        assert_eq!(persisted_id("notes"), Some(notes::ID));
        assert_eq!(registry.side_of(notes::ID), Side::Right);
        assert_eq!(
            registry.get(notes::ID).and_then(|tool| tool.feature()),
            Some("appunti")
        );
        assert!(registry.bar(Side::Right).contains(&notes::ID));
        assert!(registry.is_available(notes::ID));
        assert!(
            matches!(registry.click(notes::ID).as_slice(), [(id, lume_ui::tool::ToolRequest::NotesLoad)] if *id == notes::ID)
        );
        assert!(registry.is_open(notes::ID));
    }

    #[test]
    fn jj_e_registrato_ma_non_disponibile_senza_workspace() {
        let (registry, _terminal_wakeup) = default_registry();
        assert_eq!(persisted_id("jj"), Some(jj::ID));
        assert_eq!(registry.side_of(jj::ID), Side::Left);
        assert!(registry.get(jj::ID).is_some());
        assert!(!registry.is_available(jj::ID));
        assert!(!registry.bar(Side::Left).contains(&jj::ID));
    }
}
