//! Icone SVG del prototipo: griglia 18×18, stroke 1.5, tinta a runtime
//! via `svg::Style { color }` (il bianco viene sostituito dal colore richiesto).

use std::collections::HashMap;
use std::sync::LazyLock;

use iced::widget::svg;
use iced::{Color, Element};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Icon {
    Project,
    Search,
    Git,
    GitLog,
    Terminal,
    Db,
    Bookmarks,
    History,
    Notes,
    Settings,
    Hamburger,
    BranchTag,
    Problems,
    // Barra strumenti del pannello Commit e della console SQL: path copiati
    // alla lettera dal prototipo, che li disegna su griglia 16×16.
    Refresh,
    Undo,
    Eye,
    ChevronExpand,
    ChevronCollapse,
    Play,
    Stop,
    SplitColumns,
    Plus,
    /// Barra attività: il pannello Estensioni.
    Extensions,
    /// Barra attività: il pannello Struttura. Un albero con due gomiti — è
    /// quel che la struttura di un file è.
    Structure,
    /// Dock «Trova utilizzi»: quattro frecce che convergono su un punto —
    /// «chi punta a questo».
    Usages,
    // Icone del pannello Git e del Log.
    Push,
    Pull,
    Fetch,
    Branch,
    Stash,
    Merge,
    Check,
    /// Pannello «Jujutsu»: cambiamenti e push.
    Jujutsu,
}

const ALL: [Icon; 33] = [
    Icon::Project,
    Icon::Search,
    Icon::Git,
    Icon::GitLog,
    Icon::Terminal,
    Icon::Db,
    Icon::Bookmarks,
    Icon::History,
    Icon::Notes,
    Icon::Settings,
    Icon::Hamburger,
    Icon::BranchTag,
    Icon::Problems,
    Icon::Refresh,
    Icon::Undo,
    Icon::Eye,
    Icon::ChevronExpand,
    Icon::ChevronCollapse,
    Icon::Play,
    Icon::Stop,
    Icon::SplitColumns,
    Icon::Plus,
    Icon::Extensions,
    Icon::Structure,
    Icon::Usages,
    Icon::Push,
    Icon::Pull,
    Icon::Fetch,
    Icon::Branch,
    Icon::Stash,
    Icon::Merge,
    Icon::Check,
    Icon::Jujutsu,
];

/// Come si disegna il corpo: a contorno (la maggioranza) o pieno (Play e
/// Stop, che nel prototipo sono `fill="currentColor"`).
#[derive(Clone, Copy)]
enum Paint {
    Stroke(&'static str),
    Fill,
}

impl Icon {
    fn source(self) -> String {
        // (viewBox, come dipingere, corpo) — path identici al prototipo
        let (vb, paint, body) = match self {
            Icon::Project => (18, Paint::Stroke("1.5"), r#"<path d="M2 5h5l2 2h7v7H2z"/>"#),
            Icon::Search => (
                18,
                Paint::Stroke("1.5"),
                r#"<circle cx="8" cy="8" r="4.5"/><path d="M11.5 11.5L15 15"/>"#,
            ),
            Icon::Git => (
                18,
                Paint::Stroke("1.5"),
                r#"<circle cx="5" cy="4.5" r="2"/><circle cx="5" cy="13.5" r="2"/><circle cx="13" cy="9" r="2"/><path d="M5 6.5v5M11 9H7"/>"#,
            ),
            Icon::GitLog => (
                18,
                Paint::Stroke("1.5"),
                r#"<circle cx="6" cy="4" r="1.8"/><circle cx="6" cy="14" r="1.8"/><circle cx="13" cy="8" r="1.8"/><path d="M6 5.8v6.4M6 7.5c0-2.5 7-1 7-1.7"/>"#,
            ),
            Icon::Terminal => (
                18,
                Paint::Stroke("1.5"),
                r#"<path d="M4 6l4 3-4 3M9.5 13H14"/>"#,
            ),
            Icon::Db => (
                18,
                Paint::Stroke("1.5"),
                r#"<ellipse cx="9" cy="4.5" rx="5" ry="2"/><path d="M4 4.5v9c0 1.1 2.2 2 5 2s5-.9 5-2v-9M4 9c0 1.1 2.2 2 5 2s5-.9 5-2"/>"#,
            ),
            Icon::Bookmarks => (
                18,
                Paint::Stroke("1.5"),
                r#"<path d="M9 2l1.7 5.3L16 9l-5.3 1.7L9 16l-1.7-5.3L2 9l5.3-1.7z"/>"#,
            ),
            Icon::History => (
                18,
                Paint::Stroke("1.5"),
                r#"<circle cx="9" cy="10" r="4"/><path d="M9 3v3M3.5 6.5L6 8.5M14.5 6.5L12 8.5"/>"#,
            ),
            Icon::Notes => (
                18,
                Paint::Stroke("1.5"),
                r#"<path d="M4 2.5h7.5L15 6v9.5H4z"/><path d="M11.5 2.5V6H15M6.5 9.5h5M6.5 12.5h3.5"/>"#,
            ),
            Icon::Settings => (
                18,
                Paint::Stroke("1.5"),
                r#"<circle cx="9" cy="9" r="2.5"/><path d="M9 2v3M9 13v3M2 9h3M13 9h3M4.2 4.2l2.1 2.1M11.7 11.7l2.1 2.1M13.8 4.2l-2.1 2.1M6.3 11.7l-2.1 2.1"/>"#,
            ),
            Icon::Hamburger => (
                16,
                Paint::Stroke("1.6"),
                r#"<path d="M2.5 4h11M2.5 8h11M2.5 12h11"/>"#,
            ),
            Icon::BranchTag => (
                16,
                Paint::Stroke("1.5"),
                r#"<path d="M2 2h6l6 6-6 6-6-6z"/><circle cx="6" cy="6" r="1.2"/>"#,
            ),
            Icon::Problems => (
                18,
                Paint::Stroke("1.5"),
                r#"<path d="M9 2.5l7 12H2z"/><path d="M9 6.5v4M9 13h.01"/>"#,
            ),
            Icon::Refresh => (
                16,
                Paint::Stroke("1.4"),
                r#"<path d="M13 8a5 5 0 1 1-1.5-3.6"/><path d="M13 2.5v3h-3"/>"#,
            ),
            Icon::Undo => (
                16,
                Paint::Stroke("1.4"),
                r#"<path d="M5.5 4.5H10a3 3 0 0 1 0 6H4"/><path d="M7.5 2.5l-2 2 2 2"/>"#,
            ),
            Icon::Eye => (
                16,
                Paint::Stroke("1.4"),
                r#"<path d="M1.5 8s2.5-4.5 6.5-4.5S14.5 8 14.5 8 12 12.5 8 12.5 1.5 8 1.5 8z"/><circle cx="8" cy="8" r="2"/>"#,
            ),
            Icon::ChevronExpand => (
                16,
                Paint::Stroke("1.4"),
                r#"<path d="M5.5 5.5L8 3l2.5 2.5M5.5 10.5L8 13l2.5-2.5"/>"#,
            ),
            Icon::ChevronCollapse => (
                16,
                Paint::Stroke("1.4"),
                r#"<path d="M5.5 3L8 5.5 10.5 3M5.5 13L8 10.5 10.5 13"/>"#,
            ),
            Icon::Play => (16, Paint::Fill, r#"<path d="M4 2.5l9 5.5-9 5.5z"/>"#),
            Icon::Stop => (
                16,
                Paint::Fill,
                r#"<rect x="3" y="3" width="10" height="10" rx="1.5"/>"#,
            ),
            Icon::SplitColumns => (
                16,
                Paint::Stroke("1.4"),
                r#"<rect x="1.5" y="2.5" width="5.5" height="11" rx="1"/><rect x="9" y="2.5" width="5.5" height="11" rx="1"/>"#,
            ),
            Icon::Plus => (16, Paint::Stroke("1.5"), r#"<path d="M8 3v10M3 8h10"/>"#),
            Icon::Usages => (
                18,
                Paint::Stroke("1.5"),
                r#"<circle cx="9" cy="9" r="2"/><path d="M2.5 2.5l3.5 3.5M15.5 2.5L12 6M2.5 15.5L6 12M15.5 15.5L12 12"/>"#,
            ),
            Icon::Structure => (
                18,
                Paint::Stroke("1.5"),
                r#"<path d="M2.5 3.5h13"/><path d="M5.5 3.5v5h9"/><path d="M8.5 8.5v6h6"/>"#,
            ),
            Icon::Extensions => (
                18,
                Paint::Stroke("1.5"),
                r#"<rect x="2.5" y="2.5" width="5.5" height="5.5" rx="1.2"/><rect x="10" y="2.5" width="5.5" height="5.5" rx="1.2"/><rect x="2.5" y="10" width="5.5" height="5.5" rx="1.2"/><path d="M12.75 10.2v5.2M10.15 12.8h5.2"/>"#,
            ),
            Icon::Jujutsu => (
                18,
                Paint::Stroke("1.5"),
                r#"<circle cx="6" cy="3.5" r="1.8"/><circle cx="6" cy="9" r="1.8"/><circle cx="6" cy="14.5" r="1.8"/><path d="M6 5.3v2M6 10.8v2M7.8 9h5.2M13 7l2.5 2-2.5 2z"/>"#,
            ),
            Icon::Push => (
                16,
                Paint::Stroke("1.4"),
                r#"<path d="M8 13V3"/><path d="M4.5 6.5L8 3l3.5 3.5"/><path d="M3 13.5h10"/>"#,
            ),
            Icon::Pull => (
                16,
                Paint::Stroke("1.4"),
                r#"<path d="M8 3v10"/><path d="M4.5 9.5L8 13l3.5-3.5"/><path d="M3 2.5h10"/>"#,
            ),
            Icon::Fetch => (
                16,
                Paint::Stroke("1.4"),
                r#"<path d="M4.5 11.5a3 3 0 0 1-.4-5.9 4 4 0 0 1 7.6 1A2.5 2.5 0 0 1 11.5 11.5"/><path d="M8 8v6"/><path d="M6 12l2 2 2-2"/>"#,
            ),
            Icon::Branch => (
                16,
                Paint::Stroke("1.4"),
                r#"<circle cx="4.5" cy="3.5" r="1.5"/><circle cx="4.5" cy="12.5" r="1.5"/><circle cx="11.5" cy="5.5" r="1.5"/><path d="M4.5 5v6M11.5 7c0 3-7 2-7 4"/>"#,
            ),
            Icon::Stash => (
                16,
                Paint::Stroke("1.4"),
                r#"<path d="M2 6h12v7.5H2z"/><path d="M2 6l1.5-3h9L14 6"/><path d="M6 9h4"/>"#,
            ),
            Icon::Merge => (
                16,
                Paint::Stroke("1.4"),
                r#"<circle cx="3.5" cy="3.5" r="1.5"/><circle cx="12.5" cy="3.5" r="1.5"/><circle cx="8" cy="12.5" r="1.5"/><path d="M3.5 5c0 4 4.5 3 4.5 6M12.5 5c0 4-4.5 3-4.5 6"/>"#,
            ),
            Icon::Check => (
                16,
                Paint::Stroke("1.4"),
                r#"<path d="M3 8.5l3.5 3.5L13 4.5"/>"#,
            ),
        };
        // Il bianco è il segnaposto che `svg::Style { color }` sostituisce a
        // runtime: vale per il tratto come per il riempimento.
        let paint = match paint {
            Paint::Stroke(width) => {
                format!(r##"fill="none" stroke="#fff" stroke-width="{width}""##)
            }
            Paint::Fill => r##"fill="#fff" stroke="none""##.to_owned(),
        };
        format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="{vb}" height="{vb}" viewBox="0 0 {vb} {vb}" {paint}>{body}</svg>"##
        )
    }

    pub fn handle(self) -> svg::Handle {
        static HANDLES: LazyLock<HashMap<Icon, svg::Handle>> = LazyLock::new(|| {
            ALL.iter()
                .map(|&i| (i, svg::Handle::from_memory(i.source().into_bytes())))
                .collect()
        });
        HANDLES
            .get(&self)
            .cloned()
            .unwrap_or_else(|| svg::Handle::from_memory(self.source().into_bytes()))
    }

    /// Elemento svg tintato, quadrato di lato `size`.
    pub fn view<'a, M: 'a>(self, size: f32, color: Color) -> Element<'a, M> {
        svg(self.handle())
            .width(size)
            .height(size)
            .style(move |_, _| svg::Style { color: Some(color) })
            .into()
    }
}

#[cfg(test)]
mod tests {
    use super::{ALL, Icon};

    #[test]
    fn ogni_icona_ha_una_sorgente_svg_valida_e_distinta() {
        let mut sources = Vec::new();
        for icon in ALL {
            let source = icon.source();
            assert!(source.starts_with("<svg"), "{icon:?} non è un svg");
            assert!(source.ends_with("</svg>"), "{icon:?} non è chiuso");
            assert!(
                source.contains("#fff"),
                "{icon:?} non ha il segnaposto tintabile"
            );
            sources.push(source);
        }
        let unique: std::collections::HashSet<_> = sources.iter().collect();
        assert_eq!(
            unique.len(),
            ALL.len(),
            "due icone condividono lo stesso disegno: probabile copia-incolla"
        );
    }

    #[test]
    fn le_icone_piene_non_hanno_tratto() {
        for icon in [Icon::Play, Icon::Stop] {
            let source = icon.source();
            assert!(source.contains(r##"fill="#fff""##), "{icon:?} non è piena");
            assert!(
                source.contains(r##"stroke="none""##),
                "{icon:?} ha un tratto"
            );
        }
    }

    #[test]
    fn le_icone_git_stanno_sulla_griglia_16_a_tratto_come_la_barra_strumenti() {
        for icon in [
            Icon::Push,
            Icon::Pull,
            Icon::Fetch,
            Icon::Branch,
            Icon::Stash,
            Icon::Merge,
            Icon::Check,
        ] {
            let source = icon.source();
            assert!(
                source.contains(r#"viewBox="0 0 16 16"#),
                "{icon:?}: la barra strumenti usa la griglia 16×16"
            );
            assert!(
                source.contains(r##"stroke="#fff""##) && source.contains(r#"stroke-width="1.4""#),
                "{icon:?}: deve essere a tratto 1.4, come Refresh e Undo"
            );
        }
    }

    #[test]
    fn jujutsu_e_nell_elenco_e_ha_un_disegno_proprio() {
        assert!(ALL.contains(&Icon::Jujutsu));
        let jujutsu = Icon::Jujutsu.source();
        for altra in ALL.iter().filter(|&&icon| icon != Icon::Jujutsu) {
            assert_ne!(jujutsu, altra.source(), "{altra:?} ha lo stesso disegno");
        }
    }
}
