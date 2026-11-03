//! Il breadcrumb sopra ogni gruppo dell'editor: dove sono, in due metà.
//!
//! La prima metà è il **percorso** relativo alla radice del progetto e non
//! costa niente. La seconda è il **simbolo** che contiene il cursore, e
//! quella vuole `documentSymbol` — che è il motivo per cui il breadcrumb sta
//! insieme al language server e non prima: mezzo breadcrumb è peggio di nessun
//! breadcrumb.
//!
//! Quando il simbolo non c'è — server fermo, capability non dichiarata,
//! cursore fuori da ogni simbolo — resta il percorso da solo. Che è quello
//! che il breadcrumb di qualunque IDE fa in quel caso, e non è un ripiego.

use iced::widget::{container, row, text};
use iced::{Alignment, Element, Length, Padding};
use lume_theme::LumeTheme;

/// Altezza della striscia. Il prototipo la fa sottile: è contesto, non
/// contenuto.
pub const HEIGHT: f32 = 22.0;

/// Le briciole: prima le cartelle e il file, poi i simboli.
///
/// `root` è la radice del progetto: il percorso si mostra **relativo** a
/// quella, perché `C:\Users\…\progetti\lume\crates\lume-app\src` non è
/// contesto, è rumore. Se il file sta fuori dalla radice — la definizione di
/// un simbolo nella libreria standard, per dire — si mostra il nome del file
/// da solo: mezza catena di cartelle di sistema direbbe meno di niente.
pub fn crumbs_for(
    root: Option<&std::path::Path>,
    path: Option<&std::path::Path>,
    symbols: &[String],
) -> Vec<String> {
    let mut briciole = Vec::new();
    if let Some(path) = path {
        let relativo = root
            .and_then(|root| path.strip_prefix(root).ok())
            .unwrap_or_else(|| std::path::Path::new(path.file_name().unwrap_or(path.as_os_str())));
        briciole.extend(
            relativo
                .components()
                .map(|parte| parte.as_os_str().to_string_lossy().into_owned()),
        );
    }
    briciole.extend(symbols.iter().cloned());
    briciole
}

pub fn view<'a, M: 'a>(theme: &LumeTheme, crumbs: &[String]) -> Element<'a, M> {
    let mut riga = row![].spacing(4).align_y(Alignment::Center);
    for (indice, briciola) in crumbs.iter().enumerate() {
        if indice > 0 {
            riga = riga.push(text("›").size(11).color(theme.faint));
        }
        // l'ultima è dove sono davvero, e si legge di più
        let colore = if indice + 1 == crumbs.len() {
            theme.sec
        } else {
            theme.muted
        };
        riga = riga.push(
            text(briciola.clone())
                .size(11)
                .color(colore)
                .wrapping(text::Wrapping::None),
        );
    }
    container(riga)
        .width(Length::Fill)
        .height(Length::Fixed(HEIGHT))
        .padding(Padding::from([0.0, 10.0]))
        .clip(true)
        .into()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::crumbs_for;
    use std::path::Path;

    /// ATTENZIONE: Il percorso si mostra **relativo alla radice**: assoluto sarebbe
    /// una riga di rumore che spinge fuori schermo l'unica parte che
    /// interessa, cioè la fine.
    #[test]
    fn il_percorso_e_relativo_alla_radice_e_i_simboli_vengono_dopo() {
        let root = Path::new("C:/progetti/lume");
        let file = Path::new("C:/progetti/lume/crates/lume-app/src/app.rs");
        assert_eq!(
            crumbs_for(Some(root), Some(file), &[]),
            vec!["crates", "lume-app", "src", "app.rs"]
        );
        assert_eq!(
            crumbs_for(
                Some(root),
                Some(file),
                &["impl Lume".to_owned(), "goto_definition".to_owned()]
            ),
            vec![
                "crates",
                "lume-app",
                "src",
                "app.rs",
                "impl Lume",
                "goto_definition"
            ]
        );
    }

    /// Un file fuori dalla radice — la definizione di un simbolo nella
    /// libreria standard — mostra il nome e basta: mezza catena di cartelle
    /// di sistema direbbe meno di niente.
    #[test]
    fn un_file_fuori_dalla_radice_mostra_il_nome_e_basta() {
        let root = Path::new("C:/progetti/lume");
        assert_eq!(
            crumbs_for(
                Some(root),
                Some(Path::new("C:/rust/library/core/src/option.rs")),
                &[]
            ),
            vec!["option.rs"]
        );
        // e senza radice vale lo stesso: non c'è niente a cui essere relativi
        assert_eq!(
            crumbs_for(None, Some(Path::new("C:/x/y/main.rs")), &[]),
            vec!["main.rs"]
        );
        // un documento senza file (la console SQL) ha solo il suo simbolo, se
        // ne ha: nessuna briciola inventata
        assert!(crumbs_for(Some(root), None, &[]).is_empty());
    }
}
