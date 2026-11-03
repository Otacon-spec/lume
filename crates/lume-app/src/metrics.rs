//! Costanti geometriche del frame (misure esatte del prototipo).

pub const TITLEBAR_H: f32 = 38.0;
pub const STATUS_H: f32 = 26.0;
pub const ACTIVITY_W: f32 = 42.0;

pub const PANEL_MIN: f32 = 170.0;
pub const PANEL_MAX: f32 = 480.0;
pub const PANEL_DEFAULT: f32 = 220.0;

/// Dock in fondo alla colonna centrale (Terminale / Log Git / Problemi).
pub const DOCK_MIN: f32 = lume_ui::tool::DOCK_HEIGHT_RANGE.0;
pub const DOCK_MAX: f32 = lume_ui::tool::DOCK_HEIGHT_RANGE.2;
/// Header del dock (titolo, badge, «—»).
pub const DOCK_HEADER_H: f32 = 34.0;

/// Riga delle schede dell'editor.
///
/// Va fissata: la coda della fila è `height(Fill)` — le serve, altrimenti il
/// bersaglio del rilascio «in fondo al gruppo» sarebbe alto zero — e dentro
/// uno `scrollable` senza altezza quel `Fill` si prendeva **tutta la colonna**,
/// lasciando una fascia vuota fra le schede e il codice. Il `Fill` ora riempie
/// la barra, che è quel che voleva dire.
///
/// 30 = testo 12px + padding 6+6 del prototipo, più i 2px della striscia
/// d'attivazione sopra la scheda.
pub const TABBAR_H: f32 = 30.0;

/// Maniglia di ridimensionamento (col/row-resize).
pub const HANDLE: f32 = 4.0;

pub const WINDOW_W: f32 = 1280.0;
pub const WINDOW_H: f32 = 820.0;
