//! Sistema menù: hamburger (default, con flyout al hover) e barra classica
//! (click apre, hover cambia menù). I dropdown vivono in un layer full-window
//! dello stack; l'allineamento orizzontale usa «ghost label» (stesso testo,
//! stesso padding, colore trasparente): il motore di layout riproduce le
//! larghezze reali senza bisogno di misurare.

use iced::widget::{Space, button, column, container, mouse_area, row, text};
use iced::{Alignment, Color, Element, Length, Padding};
use lume_theme::LumeTheme;

/// Altezza fissa delle righe dei menù (permette di calcolare la y dei flyout).
pub const ITEM_H: f32 = 27.0;
/// Larghezza del dropdown hamburger.
pub const ROOT_W: f32 = 220.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuMode {
    Hamburger,
    Classica,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAct {
    None,
    /// Commuta hamburger ↔ barra classica
    MenuBar,
    /// Cerca ovunque, scheda File
    SearchFile,
    FindInFiles,
    /// Dialog nativo di apertura cartella
    OpenFolder,
    /// Salva tutti i documenti sporchi
    SaveAll,
    /// Salva il documento attivo
    Save,
    /// Salva il documento attivo in un file scelto dall'utente.
    SaveAs,
    /// Ctrl+N: una scheda vuota, senza file.
    NewFile,
    /// Chiude la finestra passando dalla domanda sui file sporchi.
    Exit,
    /// Segnalibro sulla riga corrente (F11)
    ToggleBookmark,
    /// Mostra/nasconde il dock Terminale (Alt+F12)
    ToggleTerminal,
    /// Mostra o nasconde gli autori delle righe nell'editor.
    ToggleEditorBlame,
    /// Porta il dock sul Log Git
    ShowGitLog,
    /// Porta il dock su Problemi
    ShowProblems,
    /// Sposta la scheda attiva nell'altro gruppo dell'editor.
    SplitEditor,
    /// Fonde i due gruppi. **Fonde**, non chiude: far sparire schede sporche
    /// senza chiedere sarebbe il modo peggiore di chiudere una colonna.
    UnsplitEditor,
    /// Ctrl+B: porta dove il simbolo sotto il cursore è definito.
    GotoDefinition,
    /// Ctrl+F12: apre (o chiude) il pannello Struttura.
    ShowStructure,
    /// Alt+F7: cerca gli utilizzi del simbolo sotto il cursore.
    FindUsages,
    /// Maiusc+F6: rinomina il simbolo sotto il cursore, ovunque compaia.
    Rename,
    FormatDocument,
    /// Formatta solo la selezione corrente.
    FormatSelection,
    /// Azioni rapide offerte dal server per il documento o la selezione.
    CodeActions,
    /// Comprime o espande il blocco che contiene il cursore.
    FoldToggle,
    /// Espande tutti i blocchi del documento.
    UnfoldAll,
    /// Chiude la scheda su cui si è aperto il menù contestuale.
    CloseTab,
    /// Chiude le altre schede dello stesso gruppo, tenendo questa.
    CloseOtherTabs,
    /// Copia negli appunti il percorso del soggetto: la scheda o il nodo
    /// dell'albero su cui si è aperto il menù.
    CopyPath,
    /// Apre il file del nodo dell'albero su cui si è aperto il menù.
    OpenPath,
    /// Crea un file nella cartella dell'albero su cui si è aperto il menù.
    NewTreeFile,
    /// Crea una cartella nella cartella dell'albero su cui si è aperto il menù.
    NewTreeFolder,
    /// Elimina il file dell'albero su cui si è aperto il menù, portandolo nel
    /// Cestino di sistema.
    DeleteTreeFile,
    /// Rinomina il file dell'albero su cui si è aperto il menù.
    RenameTreeFile,
    /// Collega il file all'appunto aperto.
    LinkToOpenNote,
    /// Crea un appunto già collegato al file.
    LinkToNewNote,
    /// Ctrl+Alt+←: torna alla posizione di prima.
    NavBack,
    /// Ctrl+Alt+→: rifà il passo che si era annullato.
    NavForward,
    /// Ctrl+E: l'elenco dei file recenti.
    Recents,
    /// Ctrl+G: vai a riga.
    GotoLine,
    /// Aiuto → Scorciatoie…: l'elenco delle scorciatoie da tastiera.
    Shortcuts,
    /// Chiede a GitHub se c'e' una versione piu' recente. Solo su richiesta:
    /// il budget vieta lavoro da inattivo, quindi niente controllo periodico.
    CheckUpdates,
    /// Versione, licenza e repository in una notifica: è quel che serve a chi
    /// apre una segnalazione, e senza rete.
    About,
    /// Ctrl+Maiusc+X: apre (o chiude) il pannello Estensioni. Prima di questa
    /// voce era raggiungibile solo dalla scorciatoia.
    ShowExtensions,
    /// Ctrl+Z: annulla l'ultimo edit del documento attivo.
    Undo,
    /// Ctrl+Maiusc+Z / Ctrl+Y: rifà l'edit annullato.
    Redo,
    AddNextOccurrence,
    AddCursorAbove,
    AddCursorBelow,
    /// Ctrl+X: taglia la selezione (copia e cancella).
    Cut,
    /// Ctrl+C: copia la selezione.
    Copy,
    /// Ctrl+V: incolla dalla clipboard di sistema.
    Paste,
    /// Ctrl+F: apre il riquadro Trova nel documento attivo.
    Find,
    /// Ctrl+R: come `Find`, ma con anche il campo Sostituisci.
    Replace,
    /// Ctrl+Spazio: chiede i completamenti nel punto del cursore.
    Complete,
    /// Documentazione rapida: mostra la documentazione sotto il puntatore o cursore.
    QuickDoc,
    /// Mostra la firma e il parametro attivo nel punto del cursore.
    SignatureHelp,
    /// Apre il compositore di comandi Forge.
    Forge,
}

/// Stato dell'app riflesso dalle spunte ✓ dei menù (calcolato dalla shell
/// a ogni frame: i menù non tengono copie dello stato).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MenuChecks<'a> {
    /// Il dock mostra il terminale.
    pub terminal_open: bool,
    /// Le funzionalità spente nel pannello Estensioni. Le voci che vi
    /// appartengono **non si disegnano**: stessa regola del commento a
    /// `MENUS` — solo comandi con backend disponibile. Sistema anche un baco
    /// latente, perché «Log Git» e «Problemi» aprivano un dock che poteva
    /// non esserci.
    pub disabled_features: &'a std::collections::BTreeSet<&'static str>,
    /// Un server è avviato **e** dichiara di saper dire dove sta una
    /// definizione. È un asse diverso da `disabled_features`: quello è la
    /// scelta dell'utente, questo è cosa il server sa fare adesso.
    pub lsp_definition: bool,
    /// Come sopra, per gli utilizzi: è una capability a parte, e un server
    /// può dichiararne una e non l'altra.
    pub lsp_references: bool,
    /// E per la rinomina, che è la terza capability a parte.
    pub lsp_rename: bool,
    /// Server attivo con formattazione del documento.
    pub lsp_formatting: bool,
    /// Server attivo con formattazione di un intervallo (selezione).
    pub lsp_range_formatting: bool,
    /// Server attivo che offre azioni rapide.
    pub lsp_code_actions: bool,
    /// Il documento attivo ha blocchi ripiegabili (folding Tree-sitter).
    pub folding_available: bool,
    pub can_link_open_note: bool,
    pub can_link_new_note: bool,
    /// E per il completamento: quarta capability a parte.
    pub lsp_complete: bool,
    /// Il server attivo dichiara hover.
    pub lsp_hover: bool,
    pub lsp_signature_help: bool,
    /// Documento dell'editor a cui si può assegnare un percorso.
    pub can_save_as: bool,
    /// Il file attivo appartiene a un repository Git e il client Git è disponibile.
    pub git_blame_available: bool,
    pub editor_blame_enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuBarMessage {
    ToggleRoot,
    OpenClassic(usize),
    HoverClassic(usize),
    HoverSub(usize),
    Activate(MenuAct),
    Escape,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuIntent {
    None,
    Closed,
    Activate(MenuAct),
}

pub struct MenuState {
    pub mode: MenuMode,
    /// Hamburger: dropdown radice aperto.
    pub root_open: bool,
    /// Barra classica: indice del menù aperto.
    pub open: Option<usize>,
    /// Hamburger: indice del flyout aperto.
    pub sub: Option<usize>,
}

impl MenuState {
    pub fn new() -> Self {
        Self {
            mode: MenuMode::Hamburger,
            root_open: false,
            open: None,
            sub: None,
        }
    }

    pub fn any_open(&self) -> bool {
        self.root_open || self.open.is_some()
    }

    pub fn close_all(&mut self) {
        self.root_open = false;
        self.open = None;
        self.sub = None;
    }
}

pub struct MenuBar {
    state: MenuState,
}

impl Default for MenuBar {
    fn default() -> Self {
        Self {
            state: MenuState::new(),
        }
    }
}

impl MenuBar {
    pub fn state(&self) -> &MenuState {
        &self.state
    }

    pub fn is_open(&self) -> bool {
        self.state.any_open()
    }

    pub fn update(&mut self, message: MenuBarMessage) -> MenuIntent {
        match message {
            MenuBarMessage::ToggleRoot => {
                self.state.root_open = !self.state.root_open;
                self.state.sub = None;
                MenuIntent::None
            }
            MenuBarMessage::OpenClassic(index) => {
                self.state.open = if self.state.open == Some(index) {
                    None
                } else {
                    Some(index)
                };
                MenuIntent::None
            }
            MenuBarMessage::HoverClassic(index) => {
                if self.state.open.is_some() && self.state.open != Some(index) {
                    self.state.open = Some(index);
                }
                MenuIntent::None
            }
            MenuBarMessage::HoverSub(index) => {
                self.state.sub = Some(index);
                MenuIntent::None
            }
            MenuBarMessage::Activate(MenuAct::MenuBar) => {
                self.state.mode = if self.state.mode == MenuMode::Classica {
                    MenuMode::Hamburger
                } else {
                    MenuMode::Classica
                };
                self.state.close_all();
                MenuIntent::None
            }
            MenuBarMessage::Activate(action) => {
                self.state.close_all();
                MenuIntent::Activate(action)
            }
            MenuBarMessage::Escape => {
                self.state.close_all();
                MenuIntent::Closed
            }
        }
    }

    pub fn close(&mut self) {
        self.state.close_all();
    }

    pub fn overlay<'a>(
        &'a self,
        theme: &'a LumeTheme,
        checks: MenuChecks,
    ) -> Element<'a, MenuBarMessage> {
        match self.state.mode {
            MenuMode::Hamburger if self.state.root_open => {
                hamburger_dropdown(&self.state, theme, checks)
            }
            MenuMode::Classica => classic_dropdown(&self.state, theme, checks),
            _ => Space::new().into(),
        }
    }
}

pub struct Item {
    pub label: &'static str,
    pub shortcut: &'static str,
    pub arrow: bool,
    pub divider: bool,
    pub act: MenuAct,
    /// La voce mostra la spunta ✓ quando lo stato corrispondente è attivo.
    pub check: bool,
    /// La funzionalità che questa voce comanda, se ce n'è una. Spenta quella,
    /// la voce sparisce. La chiave è la stessa di `features::FeatureSpec::key`
    /// e di `Tool::feature()`.
    pub feature: Option<&'static str>,
}

impl Item {
    const fn act(label: &'static str, shortcut: &'static str, act: MenuAct, check: bool) -> Self {
        Self {
            label,
            shortcut,
            arrow: false,
            divider: false,
            act,
            check,
            feature: None,
        }
    }

    /// Come [`Item::act`], ma la voce vive solo finché la funzionalità è accesa.
    const fn of(
        label: &'static str,
        shortcut: &'static str,
        act: MenuAct,
        check: bool,
        feature: &'static str,
    ) -> Self {
        Self {
            feature: Some(feature),
            ..Self::act(label, shortcut, act, check)
        }
    }
}

impl Item {
    /// Una riga fra due gruppi di voci: non è un comando.
    const fn separator() -> Self {
        Self {
            label: "",
            shortcut: "",
            arrow: false,
            divider: true,
            act: MenuAct::None,
            check: false,
            feature: None,
        }
    }
}

pub struct MenuDef {
    pub name: &'static str,
    pub items: &'static [Item],
}

/// Solo comandi con backend disponibile.
pub const MENUS: [MenuDef; 6] = [
    MenuDef {
        name: "File",
        items: &[
            Item::act("Nuovo file", "Ctrl+N", MenuAct::NewFile, false),
            Item::act("Apri…", "Ctrl+O", MenuAct::OpenFolder, false),
            Item::separator(),
            Item::act("Salva", "Ctrl+S", MenuAct::Save, false),
            Item::act("Salva con nome…", "", MenuAct::SaveAs, false),
            Item::act("Salva tutto", "Ctrl+Maiusc+S", MenuAct::SaveAll, false),
            Item::separator(),
            Item::act("Esci", "Alt+F4", MenuAct::Exit, false),
        ],
    },
    MenuDef {
        name: "Modifica",
        items: &[
            Item::act("Annulla", "Ctrl+Z", MenuAct::Undo, false),
            Item::act("Ripeti", "Ctrl+Maiusc+Z", MenuAct::Redo, false),
            Item::act("Taglia", "Ctrl+X", MenuAct::Cut, false),
            Item::act("Copia", "Ctrl+C", MenuAct::Copy, false),
            Item::act("Incolla", "Ctrl+V", MenuAct::Paste, false),
            Item::act(
                "Seleziona prossima occorrenza",
                "Ctrl+D",
                MenuAct::AddNextOccurrence,
                false,
            ),
            Item::act("Aggiungi cursore sopra", "", MenuAct::AddCursorAbove, false),
            Item::act("Aggiungi cursore sotto", "", MenuAct::AddCursorBelow, false),
            Item::act("Trova…", "Ctrl+F", MenuAct::Find, false),
            Item::act("Sostituisci…", "Ctrl+R", MenuAct::Replace, false),
            Item::act("Comprimi / espandi blocco", "", MenuAct::FoldToggle, false),
            Item::act("Espandi tutti i blocchi", "", MenuAct::UnfoldAll, false),
            Item::of(
                "Formatta documento",
                "",
                MenuAct::FormatDocument,
                false,
                "lsp",
            ),
            Item::of(
                "Formatta selezione",
                "",
                MenuAct::FormatSelection,
                false,
                "lsp",
            ),
            Item::of("Azioni rapide", "", MenuAct::CodeActions, false, "lsp"),
            Item::of("Completa", "Ctrl+Spazio", MenuAct::Complete, false, "lsp"),
        ],
    },
    MenuDef {
        name: "Visualizza",
        items: &[
            Item::act("Barra dei menù classica", "", MenuAct::MenuBar, true),
            Item::act("Dividi a destra", "", MenuAct::SplitEditor, false),
            Item::act("Chiudi divisione", "", MenuAct::UnsplitEditor, false),
            Item::of(
                "Terminale",
                "Alt+F12",
                MenuAct::ToggleTerminal,
                true,
                "terminale",
            ),
            Item::of("Log Git", "", MenuAct::ShowGitLog, false, "git"),
            Item::of(
                "Autori delle righe",
                "",
                MenuAct::ToggleEditorBlame,
                true,
                "git",
            ),
            Item::of("Problemi", "", MenuAct::ShowProblems, false, "lsp"),
            Item::act(
                "Estensioni…",
                "Ctrl+Maiusc+X",
                MenuAct::ShowExtensions,
                false,
            ),
        ],
    },
    MenuDef {
        name: "Naviga",
        items: &[
            Item::act("Vai al file…", "Ctrl+P", MenuAct::SearchFile, false),
            Item::act(
                "Cerca nei file…",
                "Ctrl+Maiusc+F",
                MenuAct::FindInFiles,
                false,
            ),
            Item::act("File recenti", "Ctrl+E", MenuAct::Recents, false),
            Item::act("Vai a riga…", "Ctrl+G", MenuAct::GotoLine, false),
            Item::act("Indietro", "Ctrl+Alt+←", MenuAct::NavBack, false),
            Item::act("Avanti", "Ctrl+Alt+→", MenuAct::NavForward, false),
            Item::of(
                "Vai alla definizione",
                "Ctrl+B",
                MenuAct::GotoDefinition,
                false,
                "lsp",
            ),
            Item::of(
                "Struttura",
                "Ctrl+F12",
                MenuAct::ShowStructure,
                false,
                "lsp",
            ),
            Item::of(
                "Trova utilizzi",
                "Alt+F7",
                MenuAct::FindUsages,
                false,
                "lsp",
            ),
            Item::of("Rinomina…", "Maiusc+F6", MenuAct::Rename, false, "lsp"),
            Item::of("Documentazione rapida", "", MenuAct::QuickDoc, false, "lsp"),
            Item::of(
                "Firma della funzione",
                "Ctrl+Maiusc+Spazio",
                MenuAct::SignatureHelp,
                false,
                "lsp",
            ),
            Item::act("Segnalibro", "F11", MenuAct::ToggleBookmark, false),
        ],
    },
    MenuDef {
        name: "Esegui",
        items: &[Item::act(
            "Forge · configurazioni…",
            "",
            MenuAct::Forge,
            false,
        )],
    },
    MenuDef {
        name: "Aiuto",
        items: &[
            Item::act("Scorciatoie…", "", MenuAct::Shortcuts, false),
            Item::act("Controlla aggiornamenti…", "", MenuAct::CheckUpdates, false),
            Item::act("Informazioni su Lume", "", MenuAct::About, false),
        ],
    },
];

// ---------------------------------------------------------------- rendering

/// Dropdown hamburger (220px) + eventuale flyout a destra.
fn hamburger_dropdown<'a>(
    state: &'a MenuState,
    t: &'a LumeTheme,
    checks: MenuChecks,
) -> Element<'a, MenuBarMessage> {
    // righe: i 5 nomi menù con freccia ▸; footer «Barra dei menù classica»
    let mut items = column![];
    for (i, m) in MENUS.iter().enumerate() {
        let selected = state.sub == Some(i);
        let r = button(
            row![
                text(m.name).size(12.5).color(t.text),
                Space::new().width(Length::Fill),
                text("▸").size(11).color(t.muted),
            ]
            .align_y(Alignment::Center),
        )
        .width(Length::Fill)
        .height(ITEM_H)
        .padding(Padding::from([0.0, 11.0]))
        .style(lume_ui::styles::hover_row(
            t.text,
            t.hover2,
            selected.then_some(t.hover2),
            5.0,
        ))
        .on_press(MenuBarMessage::HoverSub(i));
        items = items.push(mouse_area(r).on_enter(MenuBarMessage::HoverSub(i)));
    }
    items = items.push(divider(t));
    items = items.push(
        button(
            row![
                Space::new().width(13),
                text("Barra dei menù classica").size(12).color(t.tree)
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .width(Length::Fill)
        .height(ITEM_H)
        .padding(Padding::from([0.0, 11.0]))
        .style(lume_ui::styles::hover_row(t.tree, t.hover2, None, 5.0))
        .on_press(MenuBarMessage::Activate(MenuAct::MenuBar)),
    );

    let dropdown = container(items)
        .width(ROOT_W)
        .padding(5)
        .style(|_| lume_ui::styles::popup(t));

    // flyout del menù hovred, allineato alla riga (top = -6 relativo alla voce)
    let flyout: Element<'_, MenuBarMessage> =
        match state.sub.and_then(|i| MENUS.get(i).map(|m| (i, m))) {
            Some((i, menu)) => {
                let y = (5.0 + i as f32 * ITEM_H - 6.0).max(0.0);
                column![
                    Space::new().height(y),
                    submenu_panel(state, t, checks, menu)
                ]
                .into()
            }
            None => Space::new().into(),
        };

    // ancoraggio x: il dropdown parte sotto l'hamburger (padding 12 della barra)
    row![
        Space::new().width(crate::titlebar::CLASSIC_MENUS_X),
        column![Space::new().height(0), dropdown],
        Space::new().width(4),
        flyout,
    ]
    .into()
}

/// Dropdown della barra classica sotto la voce aperta, ancorato con ghost label.
fn classic_dropdown<'a>(
    state: &'a MenuState,
    t: &'a LumeTheme,
    checks: MenuChecks,
) -> Element<'a, MenuBarMessage> {
    let Some((open, menu)) = state.open.and_then(|i| MENUS.get(i).map(|m| (i, m))) else {
        return Space::new().into();
    };
    // ghost: stessi label/padding della barra reale, testo trasparente
    let mut anchor = row![].spacing(1);
    for m in MENUS.iter().take(open) {
        anchor = anchor.push(
            container(text(m.name).size(12.5).color(Color::TRANSPARENT))
                .padding(Padding::from([5.0, 9.0])),
        );
    }
    row![
        Space::new().width(crate::titlebar::CLASSIC_MENUS_X),
        anchor,
        column![
            Space::new().height(5),
            submenu_panel(state, t, checks, menu)
        ],
    ]
    .into()
}

/// Pannello voci di un menù (flyout hamburger o dropdown classico), min 250px.
/// Una voce si disegna? Fuori da `submenu_panel` perché quello restituisce
/// un `Element` opaco, e questa è la decisione su cui si asserisce.
pub fn item_visible(item: &Item, checks: MenuChecks<'_>) -> bool {
    if item
        .feature
        .is_some_and(|feature| checks.disabled_features.contains(feature))
    {
        return false;
    }
    act_visible(item.act, checks)
}

/// Il secondo asse da solo: **questo comando ha un soggetto adesso?**
///
/// Vive a parte perché ha due clienti — le voci di menù e il menù
/// contestuale dell'editor — e due copie della stessa regola sarebbero due
/// posti da tenere allineati. Nota che il menù contestuale deve controllare
/// **anche** il primo asse (`disabled_features`), e lo fa da sé: quello
/// dipende dalla voce, questo dall'azione.
pub fn act_visible(act: MenuAct, checks: MenuChecks<'_>) -> bool {
    // ATTENZIONE: Il secondo asse, e non è lo stesso del primo: la funzionalità può
    // essere accesa e il comando comunque **senza soggetto**. «Vai alla
    // definizione» ha senso solo se un server avviato l'ha dichiarata fra le
    // sue capability; prima di allora è una voce che si preme e non fa
    // niente, cioè il mock che questo lavoro toglie.
    match act {
        MenuAct::GotoDefinition => checks.lsp_definition,
        MenuAct::FindUsages => checks.lsp_references,
        MenuAct::Rename => checks.lsp_rename,
        MenuAct::FormatDocument => checks.lsp_formatting,
        MenuAct::FormatSelection => checks.lsp_range_formatting,
        MenuAct::CodeActions => checks.lsp_code_actions,
        MenuAct::FoldToggle | MenuAct::UnfoldAll => checks.folding_available,
        MenuAct::LinkToOpenNote => checks.can_link_open_note,
        MenuAct::LinkToNewNote => checks.can_link_new_note,
        MenuAct::Complete => checks.lsp_complete,
        MenuAct::QuickDoc => checks.lsp_hover,
        MenuAct::SignatureHelp => checks.lsp_signature_help,
        MenuAct::SaveAs => checks.can_save_as,
        MenuAct::ToggleEditorBlame => checks.git_blame_available,
        _ => true,
    }
}

fn submenu_panel<'a>(
    state: &'a MenuState,
    t: &'a LumeTheme,
    checks: MenuChecks,
    def: &'static MenuDef,
) -> Element<'a, MenuBarMessage> {
    let mut col = column![];
    for it in def.items {
        if !item_visible(it, checks) {
            continue;
        }
        if it.divider {
            col = col.push(divider(t));
            continue;
        }
        let checked = match it.act {
            MenuAct::MenuBar => state.mode == MenuMode::Classica,
            MenuAct::ToggleTerminal => checks.terminal_open,
            MenuAct::ToggleEditorBlame => checks.editor_blame_enabled,
            _ => false,
        } && it.check;
        let right = if it.arrow { "▸" } else { it.shortcut };
        let r = button(
            row![
                container(
                    text(if checked { "✓" } else { "" })
                        .size(11.5)
                        .color(t.accent)
                )
                .width(13),
                text(it.label).size(12.5).color(t.text),
                Space::new().width(Length::Fill),
                container(text(right).size(11).color(t.muted)).padding(Padding {
                    left: 18.0,
                    ..Padding::ZERO
                }),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .width(Length::Fill)
        .height(ITEM_H)
        .padding(Padding::from([0.0, 11.0]))
        .style(lume_ui::styles::hover_row(t.text, t.hover2, None, 5.0))
        .on_press(MenuBarMessage::Activate(if it.arrow {
            MenuAct::None
        } else {
            it.act
        }));
        col = col.push(r);
    }
    // il prototipo usa min-width:250; iced collassa i figli Fill dentro un
    // contenitore Shrink, quindi larghezza fissa (nessuna voce la supera)
    container(col)
        .width(250)
        .padding(5)
        .style(|_| lume_ui::styles::popup(t))
        .into()
}

fn divider(t: &LumeTheme) -> Element<'static, MenuBarMessage> {
    let border = t.border;
    container(
        container(Space::new())
            .width(Length::Fill)
            .height(1)
            .style(move |_| lume_ui::styles::bg(border)),
    )
    .width(Length::Fill)
    .padding(Padding {
        top: 5.0,
        right: 8.0,
        bottom: 5.0,
        left: 8.0,
    })
    .into()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::{
        MENUS, MenuAct, MenuBar, MenuBarMessage, MenuChecks, MenuIntent, act_visible, item_visible,
    };
    use std::collections::BTreeSet;

    #[test]
    fn menu_hover_switch_and_escape_are_local() {
        let mut menu = MenuBar::default();
        assert_eq!(
            menu.update(MenuBarMessage::OpenClassic(0)),
            MenuIntent::None
        );
        assert_eq!(
            menu.update(MenuBarMessage::HoverClassic(1)),
            MenuIntent::None
        );
        assert_eq!(menu.state().open, Some(1));
        assert_eq!(menu.update(MenuBarMessage::Escape), MenuIntent::Closed);
        assert!(!menu.is_open());
    }

    /// Una voce che comanda una funzionalità spenta non si disegna: aprirebbe
    /// un pannello che non c'è più. Le voci di nucleo restano sempre.
    #[test]
    fn le_voci_di_una_funzionalita_spenta_non_si_disegnano() {
        let spente = BTreeSet::from(["terminale"]);
        let visualizza = &MENUS[2];

        let visibili: Vec<&str> = visualizza
            .items
            .iter()
            .filter(|item| item_visible(item, checks(&spente, true)))
            .map(|item| item.label)
            .collect();

        assert!(!visibili.contains(&"Terminale"));
        assert!(visibili.contains(&"Barra dei menù classica"));
        assert!(
            visibili.contains(&"Log Git"),
            "spegnere il terminale non tocca Git"
        );

        let nessuna = BTreeSet::new();
        assert!(
            visualizza
                .items
                .iter()
                .all(|item| item_visible(item, checks(&nessuna, true)))
        );
    }

    #[test]
    fn formattazione_documento_visibile_solo_col_backend() {
        let voce = MENUS
            .iter()
            .find(|menu| menu.name == "Modifica")
            .unwrap()
            .items
            .iter()
            .find(|item| item.label == "Formatta documento")
            .expect("il comando di formattazione deve essere raggiungibile da Modifica");
        let nessuna = BTreeSet::new();
        assert!(item_visible(voce, checks(&nessuna, true)));
        assert!(!item_visible(voce, checks(&nessuna, false)));
        let spente = BTreeSet::from(["lsp"]);
        assert!(!item_visible(voce, checks(&spente, true)));
    }

    #[test]
    fn signature_help_e_visibile_solo_se_il_server_la_supporta() {
        let spente = BTreeSet::new();
        let mut disponibilita = checks(&spente, true);
        disponibilita.lsp_signature_help = false;
        assert!(!act_visible(MenuAct::SignatureHelp, disponibilita));
        disponibilita.lsp_signature_help = true;
        assert!(act_visible(MenuAct::SignatureHelp, disponibilita));
        assert!(MENUS.iter().flat_map(|menu| menu.items.iter()).any(|item| {
            item.act == MenuAct::SignatureHelp && item.shortcut == "Ctrl+Maiusc+Spazio"
        }));
    }

    fn checks<'a>(spente: &'a BTreeSet<&'static str>, lsp: bool) -> MenuChecks<'a> {
        MenuChecks {
            terminal_open: false,
            disabled_features: spente,
            lsp_definition: lsp,
            lsp_references: lsp,
            lsp_rename: lsp,
            lsp_formatting: lsp,
            lsp_range_formatting: lsp,
            lsp_code_actions: lsp,
            folding_available: lsp,
            can_link_open_note: false,
            can_link_new_note: false,
            lsp_complete: lsp,
            lsp_hover: lsp,
            lsp_signature_help: lsp,
            can_save_as: lsp,
            git_blame_available: true,
            editor_blame_enabled: false,
        }
    }

    #[test]
    fn blame_editor_compare_solo_con_git_e_repository_attivo() {
        let visualizza = &MENUS[2];
        let nessuna = BTreeSet::new();
        let visibile = |available| {
            visualizza.items.iter().any(|item| {
                item.label == "Autori delle righe"
                    && item_visible(
                        item,
                        MenuChecks {
                            git_blame_available: available,
                            ..checks(&nessuna, true)
                        },
                    )
            })
        };
        assert!(visibile(true));
        assert!(!visibile(false));
    }

    /// ATTENZIONE: Due assi, e servono tutti e due. La funzionalità accesa non basta:
    /// «Vai alla definizione» ha senso solo se un server **avviato** l'ha
    /// dichiarata. Con il solo primo asse la voce comparirebbe sempre — a
    /// language server fermo, in un progetto che non ne ha, in un file di
    /// testo — e premerla non farebbe niente.
    #[test]
    fn vai_alla_definizione_compare_solo_se_qualcuno_sa_rispondere() {
        let naviga = &MENUS[3];
        let nessuna = BTreeSet::new();
        let visibili = |lsp_definition: bool| -> Vec<&str> {
            naviga
                .items
                .iter()
                .filter(|item| item_visible(item, checks(&nessuna, lsp_definition)))
                .map(|item| item.label)
                .collect()
        };
        assert!(visibili(true).contains(&"Vai alla definizione"));
        assert!(!visibili(false).contains(&"Vai alla definizione"));
        assert!(visibili(true).contains(&"Trova utilizzi"));
        assert!(!visibili(false).contains(&"Trova utilizzi"));
        assert!(visibili(true).contains(&"Rinomina…"));
        assert!(!visibili(false).contains(&"Rinomina…"));
        assert!(
            visibili(false).contains(&"Vai al file…"),
            "le altre voci del menù non c'entrano niente"
        );

        // e l'altro asse continua a valere: spenta la funzionalità, la voce
        // sparisce anche con un server che saprebbe rispondere
        let spente = BTreeSet::from(["lsp"]);
        assert!(
            !naviga
                .items
                .iter()
                .filter(|item| item_visible(item, checks(&spente, true)))
                .any(|item| item.label == "Vai alla definizione")
        );
    }

    /// Annulla/Ripeti/Taglia/Copia/Incolla hanno già un backend reale (sono
    /// già raggiungibili da tastiera quando l'editor ha il fuoco) ma prima
    /// di questo gap non c'era un menù «Modifica» da cui vederli.
    #[test]
    fn il_menu_modifica_esiste_con_le_voci_di_edit() {
        let modifica = &MENUS[1];
        assert_eq!(modifica.name, "Modifica");
        let etichette: Vec<&str> = modifica.items.iter().map(|i| i.label).collect();
        for voce in ["Annulla", "Ripeti", "Taglia", "Copia", "Incolla"] {
            assert!(etichette.contains(&voce), "manca «{voce}»: {etichette:?}");
        }
    }

    #[test]
    fn il_menu_file_ha_nuovo_file_in_testa_ed_esci_in_coda() {
        let file = &MENUS[0];
        assert_eq!(file.name, "File");
        let nessuna = BTreeSet::new();
        let voci: Vec<&super::Item> = file
            .items
            .iter()
            .filter(|item| item_visible(item, checks(&nessuna, true)))
            .collect();
        let prima = voci.first().unwrap();
        assert_eq!((prima.label, prima.act), ("Nuovo file", MenuAct::NewFile));
        assert_eq!(prima.shortcut, "Ctrl+N");
        let ultima = voci.last().unwrap();
        assert_eq!((ultima.label, ultima.act), ("Esci", MenuAct::Exit));
        assert_eq!(ultima.shortcut, "Alt+F4");
        assert_eq!(voci.iter().filter(|item| item.divider).count(), 2);
        assert!(!prima.divider && !ultima.divider);
    }

    #[test]
    fn un_separatore_non_ha_ne_etichetta_ne_azione() {
        let riga = super::Item::separator();
        assert!(riga.divider);
        assert_eq!(riga.act, MenuAct::None);
        assert_eq!(riga.label, "");
        assert_eq!(riga.shortcut, "");
        assert!(riga.feature.is_none());
        assert!(!riga.arrow && !riga.check);
    }

    #[test]
    fn il_menu_modifica_ha_anche_trova_e_sostituisci() {
        let modifica = &MENUS[1];
        let etichette: Vec<&str> = modifica.items.iter().map(|i| i.label).collect();
        assert!(etichette.contains(&"Trova…"));
        assert!(etichette.contains(&"Sostituisci…"));
    }

    #[test]
    fn gli_indici_dei_menu_sono_questi_ed_esegui_precede_aiuto() {
        assert_eq!(MENUS[2].name, "Visualizza");
        assert_eq!(MENUS[3].name, "Naviga");
        assert_eq!(MENUS[4].name, "Esegui");
        assert_eq!(MENUS[5].name, "Aiuto");
        let voce = &MENUS[4].items[0];
        assert_eq!(voce.label, "Forge · configurazioni…");
        assert_eq!(voce.act, MenuAct::Forge);
        assert_eq!(voce.shortcut, "");
        assert_eq!(voce.feature, None);
    }

    /// Le chiavi vivono in tre file — `menu.rs`, i `Tool::feature()` e la
    /// tabella di `features.rs` — e un refuso in una delle tre non si vede:
    /// la voce semplicemente non sparirebbe mai.
    #[test]
    fn ogni_chiave_di_menu_esiste_davvero_fra_le_funzionalita() {
        for menu in &MENUS {
            for item in menu.items {
                if let Some(key) = item.feature {
                    assert!(
                        crate::features::by_key(key).is_some(),
                        "{}: la voce «{}» dichiara la funzionalità «{key}», che non esiste",
                        menu.name,
                        item.label
                    );
                }
            }
        }
    }

    #[test]
    fn completa_compare_solo_se_il_server_lo_dichiara() {
        let modifica = &MENUS[1];
        let nessuna = BTreeSet::new();
        let voci = |lsp: bool| -> Vec<&str> {
            modifica
                .items
                .iter()
                .filter(|item| item_visible(item, checks(&nessuna, lsp)))
                .map(|item| item.label)
                .collect()
        };
        assert!(voci(true).contains(&"Completa"));
        assert!(
            !voci(false).contains(&"Completa"),
            "senza server è una voce che non fa niente"
        );
        assert!(
            voci(false).contains(&"Incolla"),
            "le altre voci non c'entrano"
        );
        let spente = BTreeSet::from(["lsp"]);
        assert!(
            !modifica
                .items
                .iter()
                .filter(|item| item_visible(item, checks(&spente, true)))
                .any(|item| item.label == "Completa"),
            "funzionalità spenta: la voce sparisce anche con un server che saprebbe"
        );
    }

    #[test]
    fn documentazione_rapida_compare_solo_se_il_server_fa_hover() {
        let naviga = &MENUS[3];
        let nessuna = BTreeSet::new();
        let voci = |lsp: bool| -> Vec<&str> {
            naviga
                .items
                .iter()
                .filter(|item| item_visible(item, checks(&nessuna, lsp)))
                .map(|item| item.label)
                .collect()
        };
        assert!(voci(true).contains(&"Documentazione rapida"));
        assert!(!voci(false).contains(&"Documentazione rapida"));
        assert!(
            voci(false).contains(&"Vai a riga…"),
            "le altre voci non c'entrano"
        );
    }

    /// «Salva con nome…» sta subito dopo «Salva», e compare solo se
    /// l'editor ha un documento a cui darlo: sulla console SQL o senza schede
    /// premerlo non farebbe niente.
    #[test]
    fn salva_con_nome_compare_solo_con_un_documento_salvabile() {
        let file = &MENUS[0];
        let nessuna = BTreeSet::new();
        let etichette = |salvabile: bool| -> Vec<&str> {
            let mut c = checks(&nessuna, true);
            c.can_save_as = salvabile;
            file.items
                .iter()
                .filter(|item| item_visible(item, c))
                .filter(|item| !item.divider)
                .map(|item| item.label)
                .collect()
        };

        assert_eq!(
            etichette(true),
            vec![
                "Nuovo file",
                "Apri…",
                "Salva",
                "Salva con nome…",
                "Salva tutto",
                "Esci"
            ]
        );
        assert_eq!(
            etichette(false),
            vec!["Nuovo file", "Apri…", "Salva", "Salva tutto", "Esci"]
        );
    }
}
