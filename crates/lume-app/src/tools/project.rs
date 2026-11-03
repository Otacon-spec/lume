//! Tool «Progetto»: albero reale del filesystem (FsTree lazy di lume-core).
//! L'app gli invia SetRoot/Refresh come ToolEvent; i clic sui file diventano
//! ToolRequest::OpenFile con path assoluto.

use std::path::PathBuf;

use iced::widget::{button, column, row, scrollable, text};
use iced::{Alignment, Element, Length, Padding};
use lume_core::FsTree;
use lume_icons::FileIconContext;
use lume_ui::file_icons::{FileIcon, FileIconSource};
use lume_ui::icons::Icon;
use lume_ui::styles;
use lume_ui::tool::{Tool, ToolCtx, ToolEvent, ToolId, ToolRequest};

pub const ID: ToolId = ToolId("project");

/// Eventi propri del tool (l'app usa SetRoot/Refresh per pilotarlo).
#[derive(Debug, Clone)]
pub enum Event {
    RowClicked(PathBuf, bool),
    /// Tasto destro su una riga: il punto è quello del clic, e serve perché
    /// il menù contestuale si apre dove si è premuto.
    RowRightClicked(PathBuf, bool, f32, f32),
    SetRoot(PathBuf),
    Refresh(Vec<PathBuf>),
}

/// Il messaggio del tasto destro sulla riga: il punto e la riga.
fn right_press(punto: iced::Point, riga: &(PathBuf, bool)) -> ToolEvent {
    ToolEvent::new(Event::RowRightClicked(
        riga.0.clone(),
        riga.1,
        punto.x,
        punto.y,
    ))
}

pub struct ProjectTool {
    tree: Option<FsTree>,
}

pub(crate) fn resolve_row_icon(
    source: &dyn FileIconSource,
    name: &str,
    is_directory: bool,
    is_open: bool,
) -> Option<FileIcon> {
    let context = if is_directory {
        FileIconContext::folder(name, is_open)
    } else {
        FileIconContext::file(name)
    };
    source.icon(&context).ok()
}

impl ProjectTool {
    pub fn new() -> Self {
        Self { tree: None }
    }
}

impl Tool for ProjectTool {
    fn id(&self) -> ToolId {
        ID
    }

    fn title(&self) -> &'static str {
        "Progetto"
    }

    fn icon(&self) -> Icon {
        Icon::Project
    }

    fn view<'a>(&'a self, ctx: &ToolCtx<'a>) -> Element<'a, ToolEvent> {
        let t = ctx.theme;
        let Some(tree) = &self.tree else {
            return iced::widget::container(
                text("Nessuna cartella aperta").size(12).color(t.muted),
            )
            .padding(12)
            .into();
        };
        let rows = tree.visible_rows().into_iter().map(|r| {
            let active = !r.is_dir && ctx.active_path == Some(r.path.as_path());
            let color = if active {
                t.text
            } else if r.is_dir {
                t.tree
            } else {
                t.sec
            };
            let mut content = row![].spacing(7).align_y(Alignment::Center);
            if r.is_dir {
                content = content.push(
                    text(if r.expanded { "▾" } else { "▸" })
                        .size(11)
                        .color(color),
                );
            }
            if let Some(handle) = ctx
                .file_icons
                .and_then(|source| resolve_row_icon(source, &r.name, r.is_dir, r.expanded))
            {
                let tint = handle.tint(t, t.sec);
                content = content.push(handle.view(15.0, tint));
            }
            content = content.push(text(r.name.clone()).size(12).color(color));
            let riga = button(content)
                .padding(Padding {
                    top: 3.0,
                    right: 12.0,
                    bottom: 3.0,
                    left: 12.0 + 12.0 * r.depth as f32,
                })
                .width(Length::Fill)
                .style(styles::hover_row(
                    color,
                    t.hover,
                    active.then_some(t.sel),
                    0.0,
                ))
                .on_press(ToolEvent::new(Event::RowClicked(r.path.clone(), r.is_dir)));
            // Il tasto destro sulla riga: il punto serve al menù contestuale.
            let riga = crate::right_click::RightClickArea::new(
                riga.into(),
                (r.path.clone(), r.is_dir),
                right_press,
            );
            riga.into()
        });
        scrollable(column(rows))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn update(&mut self, ev: &ToolEvent) -> Vec<ToolRequest> {
        match ev.downcast::<Event>() {
            Some(Event::SetRoot(root)) => {
                self.tree = Some(FsTree::new(root.clone()));
            }
            Some(Event::Refresh(paths)) => {
                if let Some(tree) = &mut self.tree {
                    for p in paths {
                        if let Some(parent) = p.parent() {
                            tree.refresh(parent);
                        }
                        tree.refresh(p);
                    }
                    return vec![ToolRequest::SetProjectWatchDirectories(
                        visible_watch_directories(tree),
                    )];
                }
            }
            Some(Event::RowClicked(path, is_dir)) => {
                if *is_dir {
                    if let Some(tree) = &mut self.tree {
                        tree.toggle(path);
                        return vec![ToolRequest::SetProjectWatchDirectories(
                            visible_watch_directories(tree),
                        )];
                    }
                } else {
                    return vec![ToolRequest::OpenFile(path.clone())];
                }
            }
            Some(Event::RowRightClicked(path, is_dir, x, y)) => {
                return vec![ToolRequest::ContextMenu {
                    path: path.clone(),
                    is_dir: *is_dir,
                    x: *x,
                    y: *y,
                }];
            }
            None => {}
        }
        Vec::new()
    }

    /// Chiuso l'albero, le sue directory non servono piu' al watcher: lo
    /// scope si restringe ai soli parent dei documenti aperti (e a `.git`),
    /// che l'app unisce in `fs_watch_directories`. Prima restavano
    /// registrate a vita tutte le cartelle espanse.
    fn on_visibility_changed(&mut self, open: bool) -> Vec<ToolRequest> {
        match (open, &self.tree) {
            // riapertura: lo scope torna a essere quello dell'albero
            (true, Some(tree)) => vec![ToolRequest::SetProjectWatchDirectories(
                visible_watch_directories(tree),
            )],
            // albero non ancora costruito (notifica di apertura all'avvio,
            // prima di SetRoot): non toccare lo scope, altrimenti si
            // azzererebbe quello appena impostato da set_project_root e non
            // arriverebbe piu' alcun evento con cui ripopolarlo
            (true, None) => Vec::new(),
            (false, _) => vec![ToolRequest::SetProjectWatchDirectories(Vec::new())],
        }
    }
}

#[cfg(test)]
mod visibility_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn aprire_senza_albero_non_tocca_lo_scope() {
        let mut tool = ProjectTool::new();
        assert!(
            tool.on_visibility_changed(true).is_empty(),
            "senza albero l'apertura non deve azzerare il watch di set_project_root"
        );
    }

    #[test]
    fn chiudere_l_albero_svuota_lo_scope_del_watcher() {
        let mut tool = ProjectTool::new();
        let requests = tool.on_visibility_changed(false);
        match requests.as_slice() {
            [ToolRequest::SetProjectWatchDirectories(dirs)] => {
                assert!(dirs.is_empty(), "a pannello chiuso non si osserva nulla");
            }
            other => panic!("atteso SetProjectWatchDirectories, trovato {other:?}"),
        }
    }
}

fn visible_watch_directories(tree: &FsTree) -> Vec<PathBuf> {
    tree.visible_rows()
        .into_iter()
        .filter(|row| row.is_dir && row.expanded)
        .map(|row| row.path)
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn make_tempdir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("lume_project_{tag}_{}_{nanos}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn watch_directories(requests: &[ToolRequest]) -> &[PathBuf] {
        match requests {
            [ToolRequest::SetProjectWatchDirectories(paths)] => paths,
            other => panic!("attesa una richiesta di watch, ricevuto: {other:?}"),
        }
    }

    #[test]
    fn watch_scope_contiene_solo_directory_espanse_e_visibili() {
        let root = make_tempdir("watch_scope");
        let src = root.join("src");
        let nested = src.join("nested");
        fs::create_dir_all(&nested).unwrap();

        let mut tool = ProjectTool::new();
        tool.update(&ToolEvent::new(Event::SetRoot(root.clone())));

        let expanded_src = tool.update(&ToolEvent::new(Event::RowClicked(src.clone(), true)));
        assert_eq!(
            watch_directories(&expanded_src),
            &[root.clone(), src.clone()]
        );

        let expanded_nested = tool.update(&ToolEvent::new(Event::RowClicked(nested.clone(), true)));
        assert_eq!(
            watch_directories(&expanded_nested),
            &[root.clone(), src.clone(), nested]
        );

        let collapsed_src = tool.update(&ToolEvent::new(Event::RowClicked(src, true)));
        assert_eq!(
            watch_directories(&collapsed_src),
            std::slice::from_ref(&root)
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn il_tasto_destro_sulla_riga_chiede_il_menu_contestuale() {
        let root = make_tempdir("menu_riga");
        let file = root.join("prova.txt");
        fs::write(&file, "uno\n").unwrap();

        let mut tool = ProjectTool::new();
        tool.update(&ToolEvent::new(Event::SetRoot(root.clone())));
        let richieste = tool.update(&ToolEvent::new(Event::RowRightClicked(
            file.clone(),
            false,
            12.0,
            34.0,
        )));
        match richieste.as_slice() {
            [ToolRequest::ContextMenu { path, is_dir, x, y }] => {
                assert_eq!(path, &file);
                assert!(!is_dir);
                assert_eq!((*x, *y), (12.0, 34.0));
            }
            other => panic!("attesa la richiesta del menu, trovato {other:?}"),
        }

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refresh_dopo_delete_rimuove_subito_lo_scope_obsoleto() {
        let root = make_tempdir("watch_refresh_delete");
        let src = root.join("src");
        fs::create_dir(&src).unwrap();

        let mut tool = ProjectTool::new();
        tool.update(&ToolEvent::new(Event::SetRoot(root.clone())));
        let expanded = tool.update(&ToolEvent::new(Event::RowClicked(src.clone(), true)));
        assert_eq!(watch_directories(&expanded), &[root.clone(), src.clone()]);

        fs::remove_dir_all(&src).unwrap();
        let refreshed = tool.update(&ToolEvent::new(Event::Refresh(vec![src])));
        assert_eq!(watch_directories(&refreshed), std::slice::from_ref(&root));

        fs::remove_dir_all(root).unwrap();
    }
}
