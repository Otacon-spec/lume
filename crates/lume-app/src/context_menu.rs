//! Menu contestuale: dell'editor, delle schede e dell'albero di progetto.
//!
//! Taglia, Copia e Incolla usano gli stessi comandi delle scorciatoie.
//! Le voci LSP compaiono solo con la relativa capability; quelle degli
//! Appunti richiedono un soggetto collegabile.

use std::path::PathBuf;

use iced::widget::{button, column, container, text};
use iced::{Background, Border, Element, Length, Padding};
use lume_core::{DocId, Range};
use lume_theme::LumeTheme;
use lume_ui::styles;

use crate::menu::{MenuAct, MenuChecks};

/// Su che cosa si è aperto il menu contestuale.
///
/// Il menu è uno solo e si apre su superfici diverse: il testo di un
/// documento, una scheda, una riga dell'albero. Il soggetto dice quale, e
/// le voci cambiano di conseguenza: sull'editor ci sono le azioni di
/// modifica, su una scheda chiudere e copiare il percorso, sull'albero
/// aprire il file e copiare il percorso.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Soggetto {
    /// Nel testo di un documento.
    Editor {
        /// Il punto del testo su cui si è cliccato.
        pos: usize,
        /// La selezione, se il clic è caduto dentro.
        selezione: Option<Range>,
    },
    /// Su una scheda della barra dell'editor.
    Scheda {
        id: DocId,
        /// Il file della scheda, se c'è: una scheda senza titolo non ha
        /// un percorso da copiare.
        path: Option<PathBuf>,
    },
    /// Su una riga dell'albero di progetto.
    Albero { path: PathBuf, is_dir: bool },
}

impl Soggetto {
    /// Il percorso del soggetto, se ce n'è uno.
    pub fn path(&self) -> Option<&std::path::Path> {
        match self {
            Soggetto::Editor { .. } => None,
            Soggetto::Scheda { path, .. } => path.as_deref(),
            Soggetto::Albero { path, .. } => Some(path.as_path()),
        }
    }

    /// La selezione, se il soggetto è il testo di un documento.
    pub fn selezione(&self) -> Option<Range> {
        match self {
            Soggetto::Editor { selezione, .. } => *selezione,
            _ => None,
        }
    }

    /// Il punto del testo cliccato, se il soggetto è il testo.
    pub fn pos(&self) -> Option<usize> {
        match self {
            Soggetto::Editor { pos, .. } => Some(*pos),
            _ => None,
        }
    }

    /// Il file che la voce «Apri» deve aprire: solo sull'albero, e solo un
    /// file — su una cartella non c'è nulla da aprire.
    pub fn file_da_aprire(&self) -> Option<&std::path::Path> {
        match self {
            Soggetto::Albero { path, is_dir } if !is_dir => Some(path.as_path()),
            _ => None,
        }
    }
}

/// Dove si è cliccato, in coordinate della finestra.
#[derive(Debug, Clone, PartialEq)]
pub struct ContextMenu {
    pub x: f32,
    pub y: f32,
    pub soggetto: Soggetto,
}

/// Larghezza del menu, condivisa dalle azioni di modifica e dalle voci LSP.
const WIDTH: f32 = 210.0;

/// Azioni di modifica e voci contestuali disponibili sul documento corrente.
pub fn items(soggetto: &Soggetto, checks: MenuChecks<'_>) -> Vec<(&'static str, MenuAct)> {
    match soggetto {
        Soggetto::Editor { .. } => items_editor(checks),
        Soggetto::Scheda { path, .. } => {
            let mut voci = vec![("Chiudi scheda", MenuAct::CloseTab)];
            // «Chiudi le altre»: tiene questa scheda e chiude le altre del
            // gruppo. Se una delle altre ha modifiche non salvate, il comando
            // lo dice e non chiude niente: perdere lavoro non è un dettaglio.
            voci.push(("Chiudi le altre", MenuAct::CloseOtherTabs));
            if path.is_some() {
                voci.push(("Copia percorso", MenuAct::CopyPath));
            }
            voci
        }
        Soggetto::Albero { is_dir, .. } => {
            let mut voci = Vec::new();
            if *is_dir {
                voci.push(("Nuovo file…", MenuAct::NewTreeFile));
                voci.push(("Nuova cartella…", MenuAct::NewTreeFolder));
            } else {
                voci.push(("Apri", MenuAct::OpenPath));
                voci.push(("Rinomina…", MenuAct::RenameTreeFile));
            }
            // Il backend Windows non garantisce ancora il recupero delle
            // directory: non esponiamo un comando senza tale capacità.
            if !*is_dir || !cfg!(windows) {
                voci.push(("Elimina (Cestino)…", MenuAct::DeleteTreeFile));
            }
            voci.push(("Copia percorso", MenuAct::CopyPath));
            voci
        }
    }
}

/// Le voci dell'editor: azioni di modifica e capacità del server.
fn items_editor(checks: MenuChecks<'_>) -> Vec<(&'static str, MenuAct)> {
    // ATTENZIONE: Anche il **primo** asse. `act_visible` guarda solo il secondo — cosa
    // il server sa fare — e in pratica spegnere la funzionalità ferma anche
    // il server, quindi i due coinciderebbero. «In pratica» non è una
    // garanzia: solo le prime tre voci appartengono al language server.
    let lsp = crate::features::spec(crate::features::Feature::LanguageServer).key;
    let mut voci = Vec::new();
    if !checks.disabled_features.contains(lsp)
        && crate::menu::act_visible(MenuAct::GotoDefinition, checks)
    {
        voci.push(("Vai alla definizione", MenuAct::GotoDefinition));
    }
    if !checks.disabled_features.contains(lsp)
        && crate::menu::act_visible(MenuAct::FindUsages, checks)
    {
        voci.push(("Trova utilizzi", MenuAct::FindUsages));
    }
    if !checks.disabled_features.contains(lsp) && crate::menu::act_visible(MenuAct::Rename, checks)
    {
        voci.push(("Rinomina…", MenuAct::Rename));
    }
    if crate::menu::act_visible(MenuAct::LinkToOpenNote, checks) {
        voci.push(("Collega all'appunto aperto", MenuAct::LinkToOpenNote));
    }
    if crate::menu::act_visible(MenuAct::LinkToNewNote, checks) {
        voci.push(("Nuovo appunto collegato", MenuAct::LinkToNewNote));
    }
    voci.extend([
        ("Taglia", MenuAct::Cut),
        ("Copia", MenuAct::Copy),
        ("Incolla", MenuAct::Paste),
    ]);
    voci
}

pub fn view<'a, M: Clone + 'a>(
    t: &LumeTheme,
    voci: &[(&'static str, MenuAct)],
    su: impl Fn(MenuAct) -> M,
) -> Element<'a, M> {
    let mut righe = column![].spacing(1);
    for (etichetta, act) in voci {
        righe = righe.push(
            button(text(*etichetta).size(12).color(t.text))
                .width(Length::Fill)
                .padding(Padding::from([4, 10]))
                .style(styles::hover_row(t.text, t.hover, None, 4.0))
                .on_press(su(*act)),
        );
    }
    container(righe)
        .width(Length::Fixed(WIDTH))
        .padding(Padding::from([4, 4]))
        .style({
            let t = t.clone();
            move |_| container::Style {
                background: Some(Background::Color(t.surface)),
                border: Border {
                    color: t.border,
                    width: 1.0,
                    radius: 6.0.into(),
                },
                ..container::Style::default()
            }
        })
        .into()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::Soggetto;
    use super::items;
    use crate::menu::{MenuAct, MenuChecks};
    use lume_core::DocId;
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    fn checks<'a>(
        spente: &'a BTreeSet<&'static str>,
        def: bool,
        rif: bool,
        ren: bool,
    ) -> MenuChecks<'a> {
        MenuChecks {
            terminal_open: false,
            disabled_features: spente,
            lsp_definition: def,
            lsp_references: rif,
            lsp_rename: ren,
            lsp_formatting: false,
            lsp_range_formatting: false,
            lsp_code_actions: false,
            folding_available: false,
            can_link_open_note: false,
            can_link_new_note: false,
            lsp_complete: false,
            lsp_hover: false,
            lsp_signature_help: false,
            can_save_as: false,
            git_blame_available: false,
            editor_blame_enabled: false,
        }
    }

    /// ATTENZIONE: Le tre voci compaiono **una a una**, secondo quel che il server
    /// dichiara: un server che sa definire ma non rinominare dà due voci, non
    /// tre. A server fermo restano disponibili le azioni di modifica.
    #[test]
    fn il_menu_mostra_solo_le_voci_che_hanno_un_soggetto() {
        let nessuna = BTreeSet::new();
        let etichette = |voci: Vec<(&'static str, MenuAct)>| -> Vec<&'static str> {
            voci.into_iter()
                .filter(|(_, a)| !matches!(a, MenuAct::Cut | MenuAct::Copy | MenuAct::Paste))
                .map(|(etichetta, _)| etichetta)
                .collect()
        };

        assert_eq!(
            etichette(items(
                &Soggetto::Editor {
                    pos: 0,
                    selezione: None
                },
                checks(&nessuna, true, true, true)
            )),
            vec!["Vai alla definizione", "Trova utilizzi", "Rinomina…"]
        );
        assert_eq!(
            etichette(items(
                &Soggetto::Editor {
                    pos: 0,
                    selezione: None
                },
                checks(&nessuna, true, false, false)
            )),
            vec!["Vai alla definizione"],
            "una capability sola, una voce sola"
        );
        assert!(
            items(
                &Soggetto::Editor {
                    pos: 0,
                    selezione: None
                },
                checks(&nessuna, false, false, false)
            )
            .len()
                == 3,
            "server fermo: restano le azioni di modifica"
        );

        // e la funzionalità spenta dal pannello Estensioni le toglie tutte,
        // anche con un server che saprebbe rispondere
        let spente = BTreeSet::from(["lsp"]);
        assert_eq!(
            items(
                &Soggetto::Editor {
                    pos: 0,
                    selezione: None
                },
                checks(&spente, true, true, true)
            )
            .len(),
            3
        );
    }

    #[test]
    fn link_voci_are_independent_from_lsp_and_follow_their_subjects() {
        let none = BTreeSet::new();
        let lsp_off = BTreeSet::from(["lsp"]);
        let with_notes = |disabled: &BTreeSet<&'static str>, open: bool, new: bool| {
            let mut checks = checks(disabled, false, false, false);
            checks.can_link_open_note = open;
            checks.can_link_new_note = new;
            items(
                &Soggetto::Editor {
                    pos: 0,
                    selezione: None,
                },
                checks,
            )
            .into_iter()
            .filter(|(_, a)| !matches!(a, MenuAct::Cut | MenuAct::Copy | MenuAct::Paste))
            .map(|(label, _)| label)
            .collect::<Vec<_>>()
        };
        assert_eq!(
            with_notes(&none, true, true),
            ["Collega all'appunto aperto", "Nuovo appunto collegato"]
        );
        assert_eq!(with_notes(&none, false, true), ["Nuovo appunto collegato"]);
        assert!(with_notes(&none, false, false).is_empty());
        assert_eq!(
            with_notes(&lsp_off, true, true),
            ["Collega all'appunto aperto", "Nuovo appunto collegato"]
        );
        let mut checks = checks(&none, true, false, false);
        checks.can_link_new_note = true;
        let labels: Vec<_> = items(
            &Soggetto::Editor {
                pos: 0,
                selezione: None,
            },
            checks,
        )
        .into_iter()
        .filter(|(_, a)| !matches!(a, MenuAct::Cut | MenuAct::Copy | MenuAct::Paste))
        .map(|(label, _)| label)
        .collect();
        assert_eq!(labels, ["Vai alla definizione", "Nuovo appunto collegato"]);
    }

    /// La scheda offre ciò che riguarda la scheda: chiuderla e copiare il
    /// percorso. Una scheda senza file non ha un percorso da copiare.
    #[test]
    fn la_scheda_offre_chiudi_e_copia_percorso() {
        let nessuna = BTreeSet::new();
        let voci = items(
            &Soggetto::Scheda {
                id: DocId(1),
                path: Some(PathBuf::from("/tmp/prova.rs")),
            },
            checks(&nessuna, false, false, false),
        );
        assert_eq!(
            voci,
            vec![
                ("Chiudi scheda", MenuAct::CloseTab),
                ("Chiudi le altre", MenuAct::CloseOtherTabs),
                ("Copia percorso", MenuAct::CopyPath)
            ]
        );
        let senza_file = items(
            &Soggetto::Scheda {
                id: DocId(2),
                path: None,
            },
            checks(&nessuna, false, false, false),
        );
        assert_eq!(
            senza_file,
            vec![
                ("Chiudi scheda", MenuAct::CloseTab),
                ("Chiudi le altre", MenuAct::CloseOtherTabs)
            ]
        );
    }

    /// L'albero apre il file, offre di rinominarlo e di eliminarlo nel
    /// Cestino, e copia il percorso; su una cartella non c'è nulla da aprire,
    /// non c'è da rinominare e non c'è da eliminare (le cartelle non si
    /// eliminano in questo passo).
    #[test]
    fn l_albero_apre_il_file_e_copia_il_percorso() {
        let nessuna = BTreeSet::new();
        let file = items(
            &Soggetto::Albero {
                path: PathBuf::from("/tmp/prova.rs"),
                is_dir: false,
            },
            checks(&nessuna, false, false, false),
        );
        assert_eq!(
            file,
            vec![
                ("Apri", MenuAct::OpenPath),
                ("Rinomina…", MenuAct::RenameTreeFile),
                ("Elimina (Cestino)…", MenuAct::DeleteTreeFile),
                ("Copia percorso", MenuAct::CopyPath)
            ]
        );
        let cartella = items(
            &Soggetto::Albero {
                path: PathBuf::from("/tmp/cartella"),
                is_dir: true,
            },
            checks(&nessuna, false, false, false),
        );
        let mut attese = vec![
            ("Nuovo file…", MenuAct::NewTreeFile),
            ("Nuova cartella…", MenuAct::NewTreeFolder),
        ];
        if !cfg!(windows) {
            attese.push(("Elimina (Cestino)…", MenuAct::DeleteTreeFile));
        }
        attese.push(("Copia percorso", MenuAct::CopyPath));
        assert_eq!(cartella, attese);
    }
}
