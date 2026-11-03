//! L'elenco dei file recenti (Ctrl+E): la cronologia di navigazione letta
//! all'indietro, un file per percorso.
//!
//! ATTENZIONE: **Si guida da tastiera**, ed è il motivo per cui la riga selezionata
//! sta qui e non nel widget: Ctrl+E è un comando da tastiera, e un elenco che
//! si può solo cliccare farebbe togliere la mano dai tasti proprio nel gesto
//! in cui non si vuole.

use std::path::Path;

use iced::widget::{Space, button, column, container, row, text};
use iced::{Alignment, Background, Border, Color, Element, Length, Padding};
use lume_theme::LumeTheme;
use lume_ui::MONO;
use lume_ui::styles;

/// Un tasto che, mentre l'elenco è aperto, appartiene a lui e non alla
/// ricerca globale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecentsKey {
    Move(i32),
    Accept,
}

/// L'elenco ha un solo verso: si sceglie una riga. Chiuderlo senza scegliere
/// passa dall'Esc, che è già `CloseAll` — un secondo messaggio per la stessa
/// decisione sarebbe un secondo posto da tenere allineato.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecentsMessage {
    /// L'indice della riga scelta.
    Open(usize),
}

/// Una riga già pronta da mostrare: `file · cartella :riga`, come il
/// prototipo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub name: String,
    pub folder: String,
    pub line: usize,
}

/// Costruisce le righe da una posizione.
///
/// La cartella è **quella che contiene il file**, non il percorso intero: in
/// un elenco di dieci righe il percorso completo spinge fuori schermo l'unica
/// parte che distingue una riga dall'altra.
pub fn row_for(path: &Path, riga: usize) -> Row {
    Row {
        name: path
            .file_name()
            .map(|nome| nome.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned()),
        folder: path
            .parent()
            .and_then(Path::file_name)
            .map(|nome| nome.to_string_lossy().into_owned())
            .unwrap_or_default(),
        // le righe si contano da 1 per chi le legge
        line: riga + 1,
    }
}

pub fn view<'a>(t: &LumeTheme, rows: &[Row], selected: usize) -> Element<'a, RecentsMessage> {
    let mut lista = column![].spacing(1);
    if rows.is_empty() {
        lista = lista.push(
            container(
                text("Nessun file visitato in questa sessione.")
                    .size(12)
                    .color(t.muted),
            )
            .padding(Padding::from([6, 10])),
        );
    }
    for (indice, riga) in rows.iter().enumerate() {
        let scelta = indice == selected;
        let contenuto = row![
            text(riga.name.clone())
                .size(12.5)
                .color(if scelta { t.text } else { t.ui }),
            text(format!("· {}", riga.folder)).size(11).color(t.muted),
            Space::new().width(Length::Fill),
            text(format!(":{}", riga.line))
                .size(11)
                .font(MONO)
                .color(t.faint),
        ]
        .spacing(6)
        .align_y(Alignment::Center);
        let sfondo = if scelta { Some(t.hover) } else { None };
        lista = lista.push(
            button(contenuto)
                .width(Length::Fill)
                .padding(Padding::from([4, 10]))
                .style(styles::hover_row(t.ui, t.hover, sfondo, 4.0))
                .on_press(RecentsMessage::Open(indice)),
        );
    }

    let scheda = container(column![
        container(text("File recenti").size(11).color(t.muted)).padding(Padding::from([6, 10])),
        lista,
    ])
    .width(Length::Fixed(460.0))
    .padding(Padding::from([6, 6]))
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
    });

    container(scheda)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Center)
        .padding(Padding {
            top: 110.0,
            ..Padding::ZERO
        })
        .style(move |_| container::Style {
            background: Some(Background::Color(Color {
                a: 0.35,
                ..Color::BLACK
            })),
            ..container::Style::default()
        })
        .into()
}

/// Muove la selezione, **senza uscire dalla lista**.
///
/// Non si avvolge: in un elenco di tre righe passare dall'ultima alla prima
/// premendo giù è il modo di perdere il segno.
pub fn move_selection(selected: usize, delta: i32, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let massimo = len - 1;
    if delta < 0 {
        selected.saturating_sub(delta.unsigned_abs() as usize)
    } else {
        selected.saturating_add(delta as usize).min(massimo)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::{move_selection, row_for};
    use std::path::Path;

    /// `file · cartella :riga`, con la riga contata da 1 come la legge la
    /// gente e la cartella che **contiene** il file — non il percorso intero,
    /// che spingerebbe fuori schermo la parte che distingue le righe.
    #[test]
    fn una_riga_dice_file_cartella_e_numero() {
        let riga = row_for(Path::new("C:/repo/crates/lume-app/src/app.rs"), 41);
        assert_eq!(riga.name, "app.rs");
        assert_eq!(riga.folder, "src");
        assert_eq!(riga.line, 42);

        // e un file senza cartella non inventa un percorso
        let sciolto = row_for(Path::new("main.rs"), 0);
        assert_eq!(
            (sciolto.name.as_str(), sciolto.folder.as_str()),
            ("main.rs", "")
        );
        assert_eq!(sciolto.line, 1);
    }

    /// La selezione si ferma agli estremi. Avvolgersi in un elenco di tre
    /// righe è il modo di perdere il segno.
    #[test]
    fn la_selezione_non_esce_dalla_lista() {
        assert_eq!(move_selection(0, -1, 3), 0);
        assert_eq!(move_selection(0, 1, 3), 1);
        assert_eq!(move_selection(2, 1, 3), 2);
        assert_eq!(move_selection(2, -1, 3), 1);
        assert_eq!(
            move_selection(5, 1, 0),
            0,
            "lista vuota: niente da scegliere"
        );
    }
}
