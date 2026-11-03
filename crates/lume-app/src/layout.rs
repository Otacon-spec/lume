//! Stato e logica dei ridimensionamenti con trascinamento (pannello laterale
//! e dock). La subscription agli eventi mouse globali è attiva SOLO durante
//! un drag (budget prestazionale).

use iced::{Point, Size};
use lume_ui::tool::Side;

use crate::metrics::*;

/// Quale maniglia è stata premuta.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HandleKind {
    /// Maniglia verticale del pannello laterale.
    Panel,
    /// Maniglia orizzontale sopra il dock.
    Dock,
    /// Maniglia verticale fra i due gruppi dell'editor.
    Split,
}

/// Quel che serve a `drag_moved` oltre alla posizione. Era una lista di
/// parametri, ed è diventata una struttura quando il terzo si è aggiunto al
/// quarto: una firma con quattro `f32` di fila è un invito a scambiarli.
#[derive(Debug, Clone, Copy)]
pub struct DragContext {
    pub panel_side: Side,
    pub dock_height: f32,
    /// Larghezza dell'area editor. Serve solo alla maniglia di divisione, per
    /// convertire i pixel trascinati in una **frazione**: il layout non deve
    /// mai conoscere i pixel, ci pensa `Length::FillPortion`.
    pub editor_width: f32,
}

/// L'esito di un trascinamento: chi lo riceve sa già cosa applicare.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DragOutcome {
    /// Il pannello si è aggiornato da sé (la larghezza è di `LayoutState`).
    Applied,
    DockHeight(f32),
}

/// Drag in corso: l'origine si inizializza al primo CursorMoved
/// (il press della mouse_area non fornisce la posizione).
#[derive(Debug, Clone, Copy)]
pub struct DragState {
    pub kind: HandleKind,
    /// (coordinata iniziale, valore iniziale)
    pub origin: Option<(f32, f32)>,
}

pub struct LayoutState {
    pub panel_width: f32,
    /// Quanto spazio prende il gruppo 0 dell'editor, da 0 a 1.
    pub split_ratio: f32,
    pub drag: Option<DragState>,
    pub window: Size,
}

/// Una colonna non può ridursi a una fessura: sotto queste frazioni il
/// contenuto non è più leggibile e la maniglia non si riafferra.
pub const SPLIT_MIN: f32 = 0.15;
pub const SPLIT_MAX: f32 = 0.85;

impl LayoutState {
    pub fn new() -> Self {
        Self {
            panel_width: PANEL_DEFAULT,
            split_ratio: lume_store::DEFAULT_SPLIT_RATIO,
            drag: None,
            window: Size::new(WINDOW_W, WINDOW_H),
        }
    }

    pub fn start_drag(&mut self, kind: HandleKind) {
        self.drag = Some(DragState { kind, origin: None });
    }

    /// Aggiorna il valore trascinato. Pannello e divisione si aggiornano
    /// in-place (sono dimensioni di `LayoutState`); l'altezza del dock vive
    /// nel registry e torna al chiamante già clampata.
    pub fn drag_moved(&mut self, pos: Point, ctx: DragContext) -> Option<DragOutcome> {
        let drag = self.drag.as_mut()?;
        match drag.kind {
            HandleKind::Panel => {
                let (x0, w0) = *drag.origin.get_or_insert((pos.x, self.panel_width));
                let d = pos.x - x0;
                let w = match ctx.panel_side {
                    Side::Left => w0 + d,
                    Side::Right => w0 - d,
                };
                self.panel_width = w.clamp(PANEL_MIN, PANEL_MAX);
                Some(DragOutcome::Applied)
            }
            HandleKind::Dock => {
                let (y0, h0) = *drag.origin.get_or_insert((pos.y, ctx.dock_height));
                Some(DragOutcome::DockHeight(
                    (h0 - (pos.y - y0)).clamp(DOCK_MIN, DOCK_MAX),
                ))
            }
            HandleKind::Split => {
                let (x0, r0) = *drag.origin.get_or_insert((pos.x, self.split_ratio));
                // una finestra larga zero non è un caso reale, ma dividere per
                // essa lo diventerebbe: senza larghezza non c'è frazione
                if ctx.editor_width > 1.0 {
                    self.split_ratio =
                        (r0 + (pos.x - x0) / ctx.editor_width).clamp(SPLIT_MIN, SPLIT_MAX);
                }
                Some(DragOutcome::Applied)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(panel_side: Side, dock_height: f32) -> DragContext {
        DragContext {
            panel_side,
            dock_height,
            editor_width: 1000.0,
        }
    }

    #[test]
    fn il_dock_si_ridimensiona_verso_l_alto_entro_i_clamp() {
        let mut layout = LayoutState::new();
        layout.start_drag(HandleKind::Dock);
        // primo movimento: fissa l'origine (nessuna variazione)
        assert_eq!(
            layout.drag_moved(Point::new(0.0, 600.0), ctx(Side::Left, 300.0)),
            Some(DragOutcome::DockHeight(300.0))
        );
        // trascinare in alto di 50px alza il dock di 50px
        assert_eq!(
            layout.drag_moved(Point::new(0.0, 550.0), ctx(Side::Left, 300.0)),
            Some(DragOutcome::DockHeight(350.0))
        );
        // oltre il massimo: clamp
        assert_eq!(
            layout.drag_moved(Point::new(0.0, 0.0), ctx(Side::Left, 300.0)),
            Some(DragOutcome::DockHeight(DOCK_MAX))
        );
        // sotto il minimo: clamp
        assert_eq!(
            layout.drag_moved(Point::new(0.0, 900.0), ctx(Side::Left, 300.0)),
            Some(DragOutcome::DockHeight(DOCK_MIN))
        );
    }

    #[test]
    fn il_pannello_destro_cresce_trascinando_a_sinistra() {
        let mut layout = LayoutState::new();
        layout.start_drag(HandleKind::Panel);
        assert_eq!(
            layout.drag_moved(Point::new(1000.0, 0.0), ctx(Side::Right, 0.0)),
            Some(DragOutcome::Applied)
        );
        layout.drag_moved(Point::new(900.0, 0.0), ctx(Side::Right, 0.0));
        assert_eq!(layout.panel_width, PANEL_DEFAULT + 100.0);
        layout.drag_moved(Point::new(1400.0, 0.0), ctx(Side::Right, 0.0));
        assert_eq!(layout.panel_width, PANEL_MIN);
    }

    #[test]
    fn il_pannello_sinistro_cresce_trascinando_a_destra_entro_i_clamp() {
        let mut layout = LayoutState::new();
        layout.start_drag(HandleKind::Panel);
        assert_eq!(
            layout.drag_moved(Point::new(300.0, 0.0), ctx(Side::Left, 0.0)),
            Some(DragOutcome::Applied)
        );
        assert_eq!(layout.panel_width, PANEL_DEFAULT);
        layout.drag_moved(Point::new(400.0, 0.0), ctx(Side::Left, 0.0));
        assert_eq!(layout.panel_width, PANEL_DEFAULT + 100.0);
        layout.drag_moved(Point::new(2000.0, 0.0), ctx(Side::Left, 0.0));
        assert_eq!(layout.panel_width, PANEL_MAX);
        layout.drag_moved(Point::new(-2000.0, 0.0), ctx(Side::Left, 0.0));
        assert_eq!(layout.panel_width, PANEL_MIN);
    }

    /// La maniglia di divisione lavora in **frazioni**, non in pixel: è quel
    /// che permette al layout di non conoscere la geometria e alla sessione di
    /// restare valida su una finestra di un'altra dimensione.
    #[test]
    fn la_divisione_si_trascina_in_frazioni_e_non_collassa() {
        let mut layout = LayoutState::new();
        layout.start_drag(HandleKind::Split);
        assert_eq!(
            layout.drag_moved(Point::new(500.0, 0.0), ctx(Side::Left, 0.0)),
            Some(DragOutcome::Applied)
        );
        assert_eq!(layout.split_ratio, lume_store::DEFAULT_SPLIT_RATIO);

        // 200px su 1000 di area editor = un quinto
        layout.drag_moved(Point::new(700.0, 0.0), ctx(Side::Left, 0.0));
        assert!((layout.split_ratio - 0.7).abs() < 1e-6);

        // trascinare fino al bordo non fa sparire una colonna
        layout.drag_moved(Point::new(5000.0, 0.0), ctx(Side::Left, 0.0));
        assert_eq!(layout.split_ratio, SPLIT_MAX);
        layout.drag_moved(Point::new(-5000.0, 0.0), ctx(Side::Left, 0.0));
        assert_eq!(layout.split_ratio, SPLIT_MIN);
    }

    /// Una finestra larga zero non è un caso reale, ma dividerci dentro lo
    /// diventerebbe: senza larghezza non c'è frazione, e il rapporto non si
    /// muove invece di diventare `NaN`.
    #[test]
    fn senza_larghezza_la_divisione_non_diventa_nan() {
        let mut layout = LayoutState::new();
        layout.start_drag(HandleKind::Split);
        let stretto = DragContext {
            panel_side: Side::Left,
            dock_height: 0.0,
            editor_width: 0.0,
        };
        layout.drag_moved(Point::new(0.0, 0.0), stretto);
        layout.drag_moved(Point::new(300.0, 0.0), stretto);
        assert_eq!(layout.split_ratio, lume_store::DEFAULT_SPLIT_RATIO);
    }
}
