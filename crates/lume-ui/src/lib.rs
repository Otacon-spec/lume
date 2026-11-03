//! Widget e presentazione riusabili: trait Tool, icone, stili, editor view.

pub mod commit_graph;
pub mod editor;
pub mod file_icons;
pub mod icons;
#[cfg(feature = "performance-diagnostics")]
pub mod performance_diagnostics;
pub mod styles;
pub mod tool;

pub use icons::Icon;
pub use tool::{Registry, Side, Tool, ToolCtx, ToolEvent, ToolId, ToolKind, ToolRequest};

/// Font del codice (embedded in main.rs via `.font(...)`).
pub const MONO: iced::Font = iced::Font::with_name("JetBrains Mono");

pub const JETBRAINS_MONO: &[u8] = include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf");
pub const JETBRAINS_MONO_MEDIUM: &[u8] =
    include_bytes!("../../../assets/fonts/JetBrainsMono-Medium.ttf");
pub const JETBRAINS_MONO_SEMIBOLD: &[u8] =
    include_bytes!("../../../assets/fonts/JetBrainsMono-SemiBold.ttf");

/// Font UI del prototipo (Inter, OFL): 400/500/600 embedded, nessuna
/// dipendenza dai font di sistema.
pub const INTER: &[u8] = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
pub const INTER_MEDIUM: &[u8] = include_bytes!("../../../assets/fonts/Inter-Medium.ttf");
pub const INTER_SEMIBOLD: &[u8] = include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf");

/// Font UI a peso normale (default dell'applicazione).
pub const UI: iced::Font = iced::Font::with_name("Inter");

/// Font UI con peso esplicito (500 per titoli di dock/pannelli, 600 per
/// le icone-lettera dei risultati di ricerca).
pub const fn ui_font(weight: iced::font::Weight) -> iced::Font {
    iced::Font { weight, ..UI }
}
