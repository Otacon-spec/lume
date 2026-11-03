use iced::widget::{Space, button, column, container, mouse_area, row, scrollable, stack, text};
use iced::{Alignment, Background, Border, Element, Length, Padding, Point};
use lume_core::DocId;
use lume_icons::FileIconContext;
use lume_theme::LumeTheme;
use lume_ui::file_icons::{FileIcon, FileIconSource};
use std::path::PathBuf;

use crate::editor_groups::GroupId;

/// Sotto questa distanza è un clic, non un trascinamento. Stessa soglia della
/// barra attività: due gesti diversi non devono avere due sensibilità.
const DRAG_THRESHOLD: f32 = 4.0;

#[derive(Debug, Default)]
pub struct EditorTabBar {
    drag: Option<TabDrag>,
}

/// Il gesto in corso su una scheda.
///
/// ATTENZIONE: lo stato del trascinamento vive **qui**, non su `Lume`: è stato
/// di vista. Lo prova `il_gesto_sulle_schede_si_spegne_al_rilascio` in `app.rs`.
///
/// La macchina è quella della barra attività — soglia 4px, ghost al cursore,
/// subscription solo durante il gesto — ma **non** la sua geometria: le icone
/// hanno passo fisso 32px, le schede sono a larghezza contenuto (commit
/// `3b56a83`), e calcolare lo slot dalla x sarebbe indovinare. Qui sono **le
/// schede a dire dove sta il cursore**, con `on_enter`, e il drop usa
/// l'ultimo sorvolo.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TabDrag {
    pub id: DocId,
    /// Diventa vero superata la soglia: prima è un clic, non un
    /// trascinamento.
    pub active: bool,
    origin: Option<Point>,
    pub cursor: Point,
    /// Ultimo posto sorvolato: gruppo, e scheda **prima della quale**
    /// inserire (`None` = in coda).
    hover: Option<(GroupId, Option<DocId>)>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EditorTabBarMessage {
    /// Premuto su una scheda: potrebbe diventare un trascinamento.
    Press(GroupId, DocId),
    /// Tasto rilasciato: clic se la soglia non è stata superata, altrimenti
    /// drop.
    Release,
    /// Il cursore è entrato in una scheda, o nella coda di una barra.
    Hover(GroupId, Option<DocId>),
    CursorMoved(Point),
    /// Tasto destro su una scheda: il punto è quello del clic, e serve
    /// perché il menù contestuale si apre dove si è premuto.
    RightPress(DocId, Point),
    Close(DocId),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EditorTabBarIntent {
    /// Niente da fare (movimento, sorvolo, rilascio senza gesto).
    None,
    /// Scheda trascinata: va spostata nel gruppo, prima di `before`.
    Drop {
        id: DocId,
        group: GroupId,
        before: Option<DocId>,
    },
    Activate(DocId),
    Close(DocId),
    /// Tasto destro su una scheda: l'app apre il menù contestuale della
    /// scheda, nel punto del clic.
    ContextMenu(DocId, Point),
}

/// Il messaggio del tasto destro: il punto del clic e la scheda.
fn right_press(punto: Point, id: &DocId) -> EditorTabBarMessage {
    EditorTabBarMessage::RightPress(*id, punto)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorTab {
    pub id: DocId,
    pub kind: crate::editor_groups::EditorKind,
    pub name: String,
    pub is_dirty: bool,
    pub is_active: bool,
    pub path: Option<PathBuf>,
}

pub const TAB_CLOSE_HITBOX: f32 = 24.0;
/// Padding orizzontale del prototipo (6px 14px) prima del nome.
const TAB_PAD_LEFT: f32 = 14.0;
const TAB_CLOSE_RIGHT_INSET: f32 = 6.0;
/// Le tab sono a larghezza contenuto: la parte cliccabile per l'attivazione
/// non scende mai sotto questa larghezza, così «×» resta separato dal nome.
pub const TAB_MIN_LABEL_WIDTH: f32 = 48.0;
/// Larghezza della coda in fondo alle schede: il posto dove «in fondo»
/// diventa un gesto. Fissa e **dentro** la fila, perché la fila vive in uno
/// `scrollable` orizzontale, che dà al contenuto larghezza infinita: un
/// `Length::Fill` là dentro non avrebbe un limite su cui risolversi.
const TAB_TAIL_WIDTH: f32 = 120.0;
const _: () = assert!(TAB_MIN_LABEL_WIDTH + TAB_CLOSE_HITBOX + TAB_CLOSE_RIGHT_INSET >= 72.0);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EditorTabChrome {
    pub show_dirty_indicator: bool,
    pub show_close_button: bool,
    pub close_hitbox: f32,
}

impl EditorTabChrome {
    pub fn new(is_dirty: bool) -> Self {
        Self {
            show_dirty_indicator: is_dirty,
            show_close_button: true,
            close_hitbox: TAB_CLOSE_HITBOX,
        }
    }

    /// Il tasto centrale applica la stessa chiusura esplicita del pulsante `×`.
    pub fn middle_click_message(self, id: DocId) -> EditorTabBarMessage {
        EditorTabBarMessage::Close(id)
    }
}

/// Pulsante di chiusura dedicato: non condivide l'hitbox con l'attivazione tab.
#[derive(Debug, Clone, Copy)]
struct EditorTabCloseButton {
    id: DocId,
}

impl EditorTabCloseButton {
    fn view(self, theme: &LumeTheme) -> Element<'_, EditorTabBarMessage> {
        button(
            container(text("×").size(13).color(theme.faint))
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(Alignment::Center)
                .align_y(Alignment::Center),
        )
        .width(TAB_CLOSE_HITBOX)
        .height(TAB_CLOSE_HITBOX)
        .padding(0)
        .style({
            let hover = theme.hover2;
            move |_, status| {
                let active = matches!(
                    status,
                    iced::widget::button::Status::Hovered | iced::widget::button::Status::Pressed
                );
                iced::widget::button::Style {
                    background: active.then_some(Background::Color(hover)),
                    border: Border {
                        radius: 4.0.into(),
                        ..Border::default()
                    },
                    ..iced::widget::button::Style::default()
                }
            }
        })
        .on_press(EditorTabBarMessage::Close(self.id))
        .into()
    }
}

pub(crate) fn resolve_tab_icon(source: &dyn FileIconSource, tab: &EditorTab) -> Option<FileIcon> {
    let file_name = match &tab.kind {
        crate::editor_groups::EditorKind::Note(_) => format!("{}.md", tab.name),
        _ => tab
            .path
            .as_deref()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| tab.name.clone()),
    };
    let context = FileIconContext::file(file_name);
    source.icon(&context).ok()
}

impl EditorTabBar {
    pub fn annulla_trascinamento(&mut self) {
        self.drag = None;
    }

    pub fn update(&mut self, message: EditorTabBarMessage) -> EditorTabBarIntent {
        match message {
            EditorTabBarMessage::Close(id) => EditorTabBarIntent::Close(id),
            EditorTabBarMessage::RightPress(id, punto) => {
                EditorTabBarIntent::ContextMenu(id, punto)
            }
            EditorTabBarMessage::Press(group, id) => {
                self.drag = Some(TabDrag {
                    id,
                    active: false,
                    origin: None,
                    cursor: Point::ORIGIN,
                    hover: Some((group, Some(id))),
                });
                EditorTabBarIntent::None
            }
            EditorTabBarMessage::Release => match self.drag.take() {
                // sotto la soglia è un clic, e il clic attiva la scheda: senza
                // questo ramo premere una scheda non farebbe più niente
                Some(drag) if !drag.active => EditorTabBarIntent::Activate(drag.id),
                // lasciarla cadere **su sé stessa** non è uno spostamento:
                // niente intento, invece di uno scivolamento in coda che
                // nessuno ha chiesto. La coda della barra invece è un posto
                // vero, e lì `before: None` significa davvero «in fondo».
                Some(drag) => match drag.hover {
                    Some((_, Some(before))) if before == drag.id => EditorTabBarIntent::None,
                    Some((group, before)) => EditorTabBarIntent::Drop {
                        id: drag.id,
                        group,
                        before,
                    },
                    None => EditorTabBarIntent::None,
                },
                None => EditorTabBarIntent::None,
            },
            EditorTabBarMessage::Hover(group, before) => {
                if let Some(drag) = &mut self.drag {
                    drag.hover = Some((group, before));
                }
                EditorTabBarIntent::None
            }
            EditorTabBarMessage::CursorMoved(position) => {
                if let Some(drag) = &mut self.drag {
                    let origin = *drag.origin.get_or_insert(position);
                    if !drag.active && position.distance(origin) > DRAG_THRESHOLD {
                        drag.active = true;
                    }
                    drag.cursor = position;
                }
                EditorTabBarIntent::None
            }
        }
    }

    /// Il gesto in corso, se c'è: la subscription ai movimenti del mouse vive
    /// **solo** finché questo è `Some`.
    pub const fn drag(&self) -> Option<&TabDrag> {
        self.drag.as_ref()
    }

    pub fn view<'a>(
        &'a self,
        group: GroupId,
        tabs: impl IntoIterator<Item = EditorTab>,
        theme: &'a LumeTheme,
        file_icons: &'a dyn FileIconSource,
    ) -> Element<'a, EditorTabBarMessage> {
        let mut row = row![];
        for tab in tabs {
            let label_color = if tab.is_active { theme.text } else { theme.sec };
            let chrome = EditorTabChrome::new(tab.is_dirty);
            let strip = container(Space::new().width(Length::Fill).height(2)).style({
                let color = tab.is_active.then_some(theme.accent);
                move |_| container::Style {
                    background: color.map(Background::Color),
                    ..container::Style::default()
                }
            });
            let tab_id = tab.id;
            let mut label = row![].spacing(7).align_y(Alignment::Center);
            if let Some(handle) = resolve_tab_icon(file_icons, &tab) {
                let tint = handle.tint(theme, theme.sec);
                label = label.push(handle.view(14.0, tint));
            }
            label = label.push(text(tab.name).size(12).color(label_color));
            let activate_content = row![label].align_y(Alignment::Center);
            let activate_content = if chrome.show_dirty_indicator {
                activate_content.push(
                    container(text("●").size(9).color(theme.accent))
                        .width(12)
                        .align_x(Alignment::Center),
                )
            } else {
                activate_content
            };
            let activate = mouse_area(container(activate_content).padding(Padding {
                top: 6.0,
                right: 4.0,
                bottom: 6.0,
                left: TAB_PAD_LEFT,
            }))
            .on_press(EditorTabBarMessage::Press(group, tab_id))
            // ATTENZIONE: il rilascio arriva **anche** da qui, non solo dalla
            // subscription globale: quella nasce dall'aggiornamento seguente
            // alla pressione, e un clic secco che porta premuto e rilasciato
            // nello stesso lotto di eventi la scavalcherebbe. Il doppione
            // che arriva poi da `DragEnd` trova `drag == None` e non fa
            // niente.
            .on_release(EditorTabBarMessage::Release)
            .on_enter(EditorTabBarMessage::Hover(group, Some(tab_id)))
            .interaction(iced::mouse::Interaction::Pointer);
            let content = row![
                activate,
                container(EditorTabCloseButton { id: tab_id }.view(theme)).padding(Padding {
                    top: 2.0,
                    right: TAB_CLOSE_RIGHT_INSET,
                    bottom: 2.0,
                    left: 0.0,
                })
            ]
            .align_y(Alignment::Center);
            // larghezza contenuto: lo stack dà alla strip (Fill) la larghezza
            // reale della tab senza fissarla a priori
            let body = container(column![Space::new().height(2), content]).style({
                let background = tab.is_active.then_some(theme.bg);
                move |_| container::Style {
                    background: background.map(Background::Color),
                    ..container::Style::default()
                }
            });
            // Il tasto destro sulla scheda: il punto serve al menù
            // contestuale, e solo un widget personalizzato lo vede.
            let item = crate::right_click::RightClickArea::new(
                stack![body, strip].into(),
                tab_id,
                right_press,
            );
            row = row.push(mouse_area(item).on_middle_press(chrome.middle_click_message(tab_id)));
        }
        // La coda: sorvolarla vuol dire «in fondo a questo gruppo». Sta
        // **dentro** la fila, dopo l'ultima scheda, perché è lì che il gesto
        // la cerca — e perché fuori si sarebbe presa la larghezza che serve
        // allo scorrimento.
        row = row.push(
            mouse_area(Space::new().width(TAB_TAIL_WIDTH).height(Length::Fill))
                .on_enter(EditorTabBarMessage::Hover(group, None)),
        );
        let surface = theme.surface;
        // ATTENZIONE: `Fill`, non `Shrink`: uno `scrollable` largo quanto il suo
        // contenuto ha vista e contenuto della stessa misura, e **non scorre
        // mai**. Con quindici schede la barra sbordava e basta.
        let tabs = scrollable(row)
            .direction(scrollable::Direction::Horizontal(
                scrollable::Scrollbar::new().width(3).scroller_width(3),
            ))
            .width(Length::Fill);
        // Il guscio esterno ripete «in coda» all'ingresso nella barra: è
        // l'unico modo di far cadere una scheda in un gruppo **vuoto**, che
        // di schede da sorvolare non ne ha e la cui fila è alta zero.
        mouse_area(
            container(tabs)
                .width(Length::Fill)
                // ATTENZIONE: Altezza fissata, non `Shrink`. La coda della fila è
                // `height(Fill)` perché il rilascio «in fondo al gruppo» deve
                // avere un bersaglio anche a gruppo vuoto; senza un'altezza
                // qui, quel `Fill` si prendeva tutta la colonna dell'editor e
                // fra le schede e il codice restava una fascia vuota alta
                // centinaia di pixel.
                .height(crate::metrics::TABBAR_H)
                .style(move |_| lume_ui::styles::bg(surface)),
        )
        .on_enter(EditorTabBarMessage::Hover(group, None))
        .into()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use lume_core::DocId;

    use iced::Point;

    use super::{EditorTabBar, EditorTabBarIntent, EditorTabBarMessage, EditorTabChrome, GroupId};

    #[test]
    fn click_and_close_are_local_intents() {
        let mut tabs = EditorTabBar::default();
        // premere e rilasciare senza muoversi **è** un clic: la stessa
        // pressione poteva diventare un trascinamento, quindi l'attivazione
        // non può più decidersi alla pressione
        tabs.update(EditorTabBarMessage::Press(GroupId::Primary, DocId(2)));
        assert_eq!(
            tabs.update(EditorTabBarMessage::Release),
            EditorTabBarIntent::Activate(DocId(2))
        );
        assert_eq!(
            tabs.update(EditorTabBarMessage::Close(DocId(1))),
            EditorTabBarIntent::Close(DocId(1))
        );
    }

    /// Sotto la soglia resta un clic; superata, il rilascio è un drop **dove
    /// il cursore è passato per ultimo** — le schede sono a larghezza
    /// contenuto e la posizione non si calcola dalla x.
    #[test]
    fn oltre_la_soglia_il_rilascio_diventa_un_drop() {
        let mut tabs = EditorTabBar::default();
        tabs.update(EditorTabBarMessage::Press(GroupId::Primary, DocId(1)));
        tabs.update(EditorTabBarMessage::CursorMoved(Point::new(10.0, 10.0)));
        tabs.update(EditorTabBarMessage::CursorMoved(Point::new(12.0, 11.0)));
        assert!(
            tabs.drag().is_some_and(|drag| !drag.active),
            "due pixel non sono un trascinamento"
        );

        tabs.update(EditorTabBarMessage::CursorMoved(Point::new(200.0, 12.0)));
        assert!(tabs.drag().is_some_and(|drag| drag.active));
        tabs.update(EditorTabBarMessage::Hover(
            GroupId::Secondary,
            Some(DocId(7)),
        ));

        assert_eq!(
            tabs.update(EditorTabBarMessage::Release),
            EditorTabBarIntent::Drop {
                id: DocId(1),
                group: GroupId::Secondary,
                before: Some(DocId(7)),
            }
        );
        assert!(tabs.drag().is_none(), "il gesto si chiude col rilascio");
    }

    /// Lasciarla cadere **su sé stessa** non è uno spostamento. La coda della
    /// barra invece è un posto vero: lì «in fondo» è una richiesta.
    #[test]
    fn lasciarla_cadere_su_se_stessa_non_la_sposta() {
        let trascina = |before| {
            let mut tabs = EditorTabBar::default();
            tabs.update(EditorTabBarMessage::Press(GroupId::Primary, DocId(1)));
            tabs.update(EditorTabBarMessage::CursorMoved(Point::new(0.0, 0.0)));
            tabs.update(EditorTabBarMessage::CursorMoved(Point::new(80.0, 0.0)));
            tabs.update(EditorTabBarMessage::Hover(GroupId::Primary, before));
            tabs.update(EditorTabBarMessage::Release)
        };

        assert_eq!(trascina(Some(DocId(1))), EditorTabBarIntent::None);
        assert_eq!(
            trascina(None),
            EditorTabBarIntent::Drop {
                id: DocId(1),
                group: GroupId::Primary,
                before: None,
            }
        );
    }

    #[test]
    fn middle_click_emits_close_for_its_tab() {
        let chrome = EditorTabChrome::new(false);
        assert_eq!(
            chrome.middle_click_message(DocId(42)),
            EditorTabBarMessage::Close(DocId(42))
        );
    }

    /// Il tasto destro chiede il menu contestuale **con il punto**: il menu
    /// si apre dove si è premuto, e il punto arriva fin qui perché solo un
    /// widget personalizzato lo vede.
    #[test]
    fn il_tasto_destro_chiede_il_menu_contestuale_con_il_punto() {
        let mut tabs = EditorTabBar::default();
        let punto = Point::new(120.0, 30.0);
        assert_eq!(
            tabs.update(EditorTabBarMessage::RightPress(DocId(7), punto)),
            EditorTabBarIntent::ContextMenu(DocId(7), punto)
        );
    }

    #[test]
    fn dirty_tabs_keep_a_real_close_target() {
        let chrome = EditorTabChrome::new(true);
        assert!(chrome.show_dirty_indicator);
        assert!(chrome.show_close_button);
        assert!(chrome.close_hitbox >= 24.0);
    }
}
