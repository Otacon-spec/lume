//! Popup del completamento e scheda dell'hover: dati di sola lettura più
//! funzioni pure per dimensioni, posizione, finestra scorrevole, tasti.
//!
//! Il widget non sa da dove vengano le voci: le riceve già pronte (Task 12
//! e 13) e pubblica comandi. Tutto ciò che si può sbagliare senza un
//! renderer vive qui e si prova qui.

use iced::keyboard::{Key, Modifiers, key::Named};
use iced::{Point, Rectangle, Size};

use super::commands::EditorCommand;
use super::widget::{FONT_SIZE, LINE_H};

/// Larghezza di una cella del monospazio: la stessa stima che il widget usa
/// per la larghezza del contenuto (`FONT_SIZE * 0.65`).
pub const CELL_W: f32 = FONT_SIZE * 0.65;
/// Righe visibili del popup: le altre si raggiungono con le frecce.
pub const POPUP_VISIBLE: usize = 10;
/// Margine interno orizzontale dei riquadri.
pub const BOX_PAD_X: f32 = 8.0;
/// Margine interno verticale dei riquadri.
pub const BOX_PAD_Y: f32 = 4.0;
/// Colonne massime dell'etichetta di una voce.
pub const LABEL_MAX_COLS: usize = 40;
/// Colonne massime del dettaglio di una voce.
pub const DETAIL_MAX_COLS: usize = 40;
/// Colonne massime di una riga della scheda.
pub const CARD_MAX_COLS: usize = 100;

/// Una voce del popup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PopupRow {
    pub label: String,
    pub detail: Option<String>,
}

/// Il popup di completamento: `anchor` è il char dove comincia la parola
/// che si sta completando.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorPopup {
    pub anchor: usize,
    pub rows: Vec<PopupRow>,
    pub selected: usize,
}

/// Una riga della scheda hover: codice (sfondo a parte) o prosa.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoverLine {
    pub text: String,
    pub code: bool,
}

/// La scheda hover: `anchor` è il char dove comincia la parola puntata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoverCard {
    pub anchor: usize,
    pub lines: Vec<HoverLine>,
}

/// Accorcia a `max_cols` char con un puntino di sospensione. Si conta in
/// char, mai in byte: un'emoji non si spezza.
pub fn clip_text(text: &str, max_cols: usize) -> String {
    if max_cols == 0 {
        return String::new();
    }
    if text.chars().count() <= max_cols {
        return text.to_owned();
    }
    let mut corto: String = text.chars().take(max_cols.saturating_sub(1)).collect();
    corto.push('…');
    corto
}

fn cols(text: &str, max: usize) -> usize {
    text.chars().count().min(max)
}

/// Dimensioni del popup: la riga più larga (etichetta, due celle, dettaglio)
/// e al massimo `visible` righe.
pub fn popup_size(rows: &[PopupRow], visible: usize) -> Size {
    let larga = rows
        .iter()
        .map(|row| {
            cols(&row.label, LABEL_MAX_COLS)
                + row
                    .detail
                    .as_deref()
                    .map_or(0, |detail| 2 + cols(detail, DETAIL_MAX_COLS))
        })
        .max()
        .unwrap_or(0);
    Size::new(
        larga as f32 * CELL_W + 2.0 * BOX_PAD_X,
        rows.len().min(visible) as f32 * LINE_H + 2.0 * BOX_PAD_Y,
    )
}

/// Dimensioni della scheda hover.
pub fn card_size(lines: &[HoverLine]) -> Size {
    let larga = lines
        .iter()
        .map(|line| cols(&line.text, CARD_MAX_COLS))
        .max()
        .unwrap_or(0);
    Size::new(
        larga as f32 * CELL_W + 2.0 * BOX_PAD_X,
        lines.len() as f32 * LINE_H + 2.0 * BOX_PAD_Y,
    )
}

/// Dove mettere un riquadro di dimensione `size` rispetto alla riga che
/// comincia in `anchor` (alta `line_h`): sotto la riga, o sopra se
/// `prefer_above`; se la scelta non ci sta si prova l'altra, e se nessuna
/// ci sta si prende la più spaziosa. Dentro `bounds` sempre: mai fuori né a
/// destra né a sinistra, e mai più grande dell'area.
pub fn place_box(
    anchor: Point,
    line_h: f32,
    size: Size,
    bounds: Rectangle,
    prefer_above: bool,
) -> Rectangle {
    let width = size.width.min(bounds.width);
    let height = size.height.min(bounds.height);
    let x = anchor.x.min(bounds.x + bounds.width - width).max(bounds.x);
    let sotto_y = anchor.y + line_h;
    let sopra_y = anchor.y - height;
    let ci_sta_sotto = sotto_y + height <= bounds.y + bounds.height;
    let ci_sta_sopra = sopra_y >= bounds.y;
    let sotto = match (prefer_above, ci_sta_sopra, ci_sta_sotto) {
        (true, true, _) => false,
        (true, false, true) => true,
        (false, _, true) => true,
        (false, true, false) => false,
        // nessuno dei due ci sta: dove c'è più aria
        _ => bounds.y + bounds.height - sotto_y >= anchor.y - bounds.y,
    };
    let y = if sotto { sotto_y } else { sopra_y };
    let y = y.min(bounds.y + bounds.height - height).max(bounds.y);
    Rectangle {
        x,
        y,
        width,
        height,
    }
}

/// La prima riga visibile del popup, dato l'offset di prima: la finestra si
/// sposta solo se la selezione ne esce, e mai oltre `total - visible`.
pub fn window_top(top: usize, selected: usize, total: usize, visible: usize) -> usize {
    let massimo = total.saturating_sub(visible);
    let top = top.min(massimo);
    if selected < top {
        selected
    } else if selected >= top + visible {
        (selected + 1 - visible).min(massimo)
    } else {
        top
    }
}

fn is_word(carattere: char) -> bool {
    // la stessa classe di `movement::class_of` in lume-core
    carattere.is_alphanumeric() || carattere == '_'
}

/// La colonna (in char) dove comincia la parola che contiene il char `col`
/// della riga, o `None` se lì non c'è una parola.
pub fn word_start_in_line(text: &str, col: usize) -> Option<usize> {
    let caratteri: Vec<char> = text.chars().collect();
    if !caratteri.get(col).copied().is_some_and(is_word) {
        return None;
    }
    let mut inizio = col;
    while inizio > 0 && caratteri.get(inizio - 1).copied().is_some_and(is_word) {
        inizio -= 1;
    }
    Some(inizio)
}

/// Il mouse è sopra `now`; l'ultima cosa pubblicata era `last`. `Some(x)` =
/// pubblica `HoverAt(x)`; `None` = tace. Limite alla fonte: un messaggio per
/// parola, non per pixel; e nessuno se l'app non ha l'hover.
pub fn hover_change(
    last: Option<usize>,
    now: Option<usize>,
    enabled: bool,
) -> Option<Option<usize>> {
    (enabled && last != now).then_some(now)
}

/// Che cosa sa il widget dell'overlay quando arriva un tasto.
#[derive(Debug, Clone, Copy)]
pub struct OverlayKeys {
    pub popup_open: bool,
    pub card_open: bool,
    pub complete_enabled: bool,
}

/// Traduce un tasto in un comando dell'overlay. `None` = non è per lui e il
/// tasto va all'editor come sempre.
///
/// - Ctrl+Spazio chiede il completamento (solo se il server lo offre).
/// - A popup aperto: su/giù scorrono, Invio e Tab accettano, Esc chiude.
///   Con Ctrl o Alt premuti i tasti restano dell'editor.
/// - Con la sola scheda aperta: Esc la chiude.
pub fn overlay_key(state: OverlayKeys, key: &Key, modifiers: Modifiers) -> Option<EditorCommand> {
    let Key::Named(nome) = key else {
        return None;
    };
    if *nome == Named::Space {
        return (modifiers.command()
            && !modifiers.shift()
            && !modifiers.alt()
            && state.complete_enabled)
            .then_some(EditorCommand::TriggerComplete);
    }
    if modifiers.command() || modifiers.alt() {
        return None;
    }
    match nome {
        Named::Escape if state.popup_open || state.card_open => Some(EditorCommand::PopupDismiss),
        Named::ArrowUp if state.popup_open => Some(EditorCommand::PopupUp),
        Named::ArrowDown if state.popup_open => Some(EditorCommand::PopupDown),
        Named::Enter | Named::Tab if state.popup_open => Some(EditorCommand::PopupAccept),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use iced::keyboard::{Key, Modifiers, key::Named};
    use iced::{Rectangle, Size};

    use super::{
        BOX_PAD_X, CELL_W, HoverLine, OverlayKeys, PopupRow, card_size, clip_text, hover_change,
        overlay_key, place_box, popup_size, window_top, word_start_in_line,
    };
    use crate::editor::commands::EditorCommand;
    use crate::editor::widget::LINE_H;

    fn riga(label: &str, detail: Option<&str>) -> PopupRow {
        PopupRow {
            label: label.to_owned(),
            detail: detail.map(str::to_owned),
        }
    }

    fn area() -> Rectangle {
        Rectangle {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        }
    }

    #[test]
    fn il_testo_lungo_si_accorcia_con_i_puntini() {
        assert_eq!(clip_text("push", 10), "push");
        assert_eq!(
            clip_text("abcdefghij", 10),
            "abcdefghij",
            "esattamente al limite"
        );
        assert_eq!(clip_text("abcdefghijk", 10), "abcdefghi…");
        assert_eq!(
            clip_text("😀😀😀😀", 3),
            "😀😀…",
            "si conta in char, mai a metà emoji"
        );
        assert_eq!(clip_text("", 5), "");
        assert_eq!(clip_text("x", 0), "", "zero colonne non produce overflow");
    }

    #[test]
    fn la_larghezza_del_popup_segue_le_righe_e_ha_un_tetto() {
        let stretto = popup_size(&[riga("a", None)], 10);
        let largo = popup_size(
            &[riga("un_nome_piuttosto_lungo", Some("fn(i32) -> String"))],
            10,
        );
        assert!(largo.width > stretto.width);
        let enorme = "x".repeat(500);
        let tetto = popup_size(&[riga(&enorme, Some(&enorme))], 10);
        assert!(
            tetto.width < 100.0 * CELL_W,
            "il tetto vale anche per testi enormi"
        );
    }

    #[test]
    fn l_altezza_del_popup_conta_al_massimo_le_righe_visibili() {
        let righe: Vec<_> = (0..30).map(|i| riga(&format!("v{i}"), None)).collect();
        let tre = popup_size(&righe[..3], 10);
        let tutte = popup_size(&righe, 10);
        assert!(tutte.height > tre.height);
        assert_eq!(
            tutte.height,
            popup_size(&righe[..10], 10).height,
            "dieci e basta"
        );
    }

    #[test]
    fn la_scheda_ha_l_altezza_delle_sue_righe() {
        let righe: Vec<_> = (0..3)
            .map(|i| HoverLine {
                text: format!("riga {i}"),
                code: i == 0,
            })
            .collect();
        let una = card_size(&righe[..1]);
        let tre = card_size(&righe);
        assert!((tre.height - una.height - 2.0 * LINE_H).abs() < 0.01);
        assert!(tre.width >= una.width);
    }

    #[test]
    fn il_riquadro_sta_sotto_la_riga_se_ci_sta() {
        let r = place_box(
            iced::Point::new(100.0, 50.0),
            LINE_H,
            Size::new(200.0, 100.0),
            area(),
            false,
        );
        assert_eq!((r.x, r.y), (100.0, 50.0 + LINE_H));
    }

    #[test]
    fn il_riquadro_si_ribalta_sopra_se_sotto_non_ci_sta() {
        let r = place_box(
            iced::Point::new(100.0, 560.0),
            LINE_H,
            Size::new(200.0, 100.0),
            area(),
            false,
        );
        assert_eq!(r.y, 560.0 - 100.0, "sopra la riga");
    }

    #[test]
    fn la_scheda_preferisce_stare_sopra_e_scende_se_non_ci_sta() {
        let sopra = place_box(
            iced::Point::new(100.0, 300.0),
            LINE_H,
            Size::new(200.0, 100.0),
            area(),
            true,
        );
        assert_eq!(sopra.y, 300.0 - 100.0);
        let sotto = place_box(
            iced::Point::new(100.0, 40.0),
            LINE_H,
            Size::new(200.0, 100.0),
            area(),
            true,
        );
        assert_eq!(sotto.y, 40.0 + LINE_H, "in cima non c'è posto: scende");
    }

    #[test]
    fn il_riquadro_non_esce_a_destra_ne_a_sinistra() {
        let a_destra = place_box(
            iced::Point::new(750.0, 100.0),
            LINE_H,
            Size::new(200.0, 50.0),
            area(),
            false,
        );
        assert_eq!(a_destra.x, 600.0, "si ferma a 800 - 200");
        let a_sinistra = place_box(
            iced::Point::new(-30.0, 100.0),
            LINE_H,
            Size::new(200.0, 50.0),
            area(),
            false,
        );
        assert_eq!(a_sinistra.x, 0.0);
    }

    #[test]
    fn un_riquadro_piu_grande_dell_area_viene_stretto_e_non_va_in_panico() {
        let r = place_box(
            iced::Point::new(10.0, 10.0),
            LINE_H,
            Size::new(2000.0, 2000.0),
            area(),
            false,
        );
        assert!(r.width <= 800.0 && r.height <= 600.0);
        assert!(r.x >= 0.0 && r.y >= 0.0);
    }

    #[test]
    fn la_finestra_del_popup_segue_la_selezione() {
        // 30 voci, 10 visibili
        assert_eq!(window_top(0, 0, 30, 10), 0);
        assert_eq!(window_top(0, 9, 30, 10), 0, "l'ultima visibile: ferma");
        assert_eq!(window_top(0, 10, 30, 10), 1, "scende di una");
        assert_eq!(window_top(5, 2, 30, 10), 2, "risale fino alla selezione");
        assert_eq!(window_top(0, 29, 30, 10), 20, "in fondo");
        assert_eq!(
            window_top(20, 0, 30, 10),
            0,
            "il giro in tondo torna in cima"
        );
        assert_eq!(window_top(7, 1, 4, 10), 0, "meno voci della finestra");
        assert_eq!(
            window_top(99, 5, 30, 10),
            5,
            "un vecchio offset fuori scala si corregge"
        );
    }

    #[test]
    fn la_parola_sotto_il_mouse_comincia_dove_comincia() {
        let testo = "let foo_bar = 1";
        assert_eq!(word_start_in_line(testo, 4), Some(4), "sulla prima lettera");
        assert_eq!(
            word_start_in_line(testo, 8),
            Some(4),
            "in mezzo, con il trattino basso"
        );
        assert_eq!(
            word_start_in_line(testo, 10),
            Some(4),
            "sull'ultima lettera"
        );
        assert_eq!(word_start_in_line(testo, 3), None, "sullo spazio");
        assert_eq!(word_start_in_line(testo, 11), None, "sullo spazio dopo");
        assert_eq!(
            word_start_in_line(testo, 100),
            None,
            "oltre la fine della riga"
        );
        assert_eq!(word_start_in_line("", 0), None, "riga vuota");
        // le colonne sono in char: l'emoji conta uno
        assert_eq!(word_start_in_line("😀ab", 2), Some(1));
        assert_eq!(
            word_start_in_line("😀ab", 0),
            None,
            "l'emoji non è una parola"
        );
    }

    #[test]
    fn il_mouse_pubblica_solo_quando_la_parola_cambia() {
        // abilitato
        assert_eq!(
            hover_change(None, Some(4), true),
            Some(Some(4)),
            "entra in una parola"
        );
        assert_eq!(
            hover_change(Some(4), Some(4), true),
            None,
            "stessa parola: niente"
        );
        assert_eq!(
            hover_change(Some(4), Some(9), true),
            Some(Some(9)),
            "altra parola"
        );
        assert_eq!(
            hover_change(Some(4), None, true),
            Some(None),
            "esce dalla parola"
        );
        assert_eq!(hover_change(None, None, true), None, "fuori e resta fuori");
        // spento: mai
        assert_eq!(hover_change(None, Some(4), false), None);
        assert_eq!(hover_change(Some(4), None, false), None);
    }

    fn tasti(popup: bool, scheda: bool, completa: bool) -> OverlayKeys {
        OverlayKeys {
            popup_open: popup,
            card_open: scheda,
            complete_enabled: completa,
        }
    }

    fn tasto(nome: Named) -> Key {
        Key::Named(nome)
    }

    #[test]
    fn con_il_popup_aperto_i_tasti_lo_governano() {
        let s = Modifiers::empty();
        let aperto = tasti(true, false, true);
        assert_eq!(
            overlay_key(aperto, &tasto(Named::ArrowUp), s),
            Some(EditorCommand::PopupUp)
        );
        assert_eq!(
            overlay_key(aperto, &tasto(Named::ArrowDown), s),
            Some(EditorCommand::PopupDown)
        );
        assert_eq!(
            overlay_key(aperto, &tasto(Named::Enter), s),
            Some(EditorCommand::PopupAccept)
        );
        assert_eq!(
            overlay_key(aperto, &tasto(Named::Tab), s),
            Some(EditorCommand::PopupAccept)
        );
        assert_eq!(
            overlay_key(aperto, &tasto(Named::Escape), s),
            Some(EditorCommand::PopupDismiss)
        );
        // le altre frecce e le lettere non si toccano: il popup si filtra da sé
        assert_eq!(overlay_key(aperto, &tasto(Named::ArrowLeft), s), None);
        assert_eq!(overlay_key(aperto, &tasto(Named::Backspace), s), None);
    }

    #[test]
    fn senza_popup_enter_tab_e_frecce_restano_dell_editor() {
        let s = Modifiers::empty();
        let chiuso = tasti(false, false, true);
        for nome in [
            Named::ArrowUp,
            Named::ArrowDown,
            Named::Enter,
            Named::Tab,
            Named::Escape,
        ] {
            assert_eq!(overlay_key(chiuso, &tasto(nome), s), None, "{nome:?}");
        }
    }

    #[test]
    fn escape_chiude_anche_la_sola_scheda() {
        let s = Modifiers::empty();
        assert_eq!(
            overlay_key(tasti(false, true, true), &tasto(Named::Escape), s),
            Some(EditorCommand::PopupDismiss)
        );
        assert_eq!(
            overlay_key(tasti(false, true, true), &tasto(Named::Enter), s),
            None
        );
    }

    #[test]
    fn ctrl_spazio_chiede_il_completamento_solo_se_abilitato() {
        let ctrl = Modifiers::COMMAND;
        assert_eq!(
            overlay_key(tasti(false, false, true), &tasto(Named::Space), ctrl),
            Some(EditorCommand::TriggerComplete)
        );
        assert_eq!(
            overlay_key(tasti(true, false, true), &tasto(Named::Space), ctrl),
            Some(EditorCommand::TriggerComplete),
            "anche a popup aperto: si richiede"
        );
        assert_eq!(
            overlay_key(tasti(false, false, false), &tasto(Named::Space), ctrl),
            None,
            "server senza completamento: il tasto non è nostro"
        );
        assert_eq!(
            overlay_key(
                tasti(false, false, true),
                &tasto(Named::Space),
                Modifiers::empty()
            ),
            None,
            "lo spazio da solo è un carattere"
        );
    }

    #[test]
    fn ctrl_maiusc_spazio_lascia_la_firma_al_listener_globale() {
        for popup_aperto in [false, true] {
            assert_eq!(
                overlay_key(
                    tasti(popup_aperto, false, true),
                    &tasto(Named::Space),
                    Modifiers::COMMAND | Modifiers::SHIFT,
                ),
                None,
                "Ctrl+Maiusc+Spazio non deve chiedere il completamento"
            );
        }
    }

    #[test]
    fn i_tasti_con_ctrl_o_alt_non_governano_il_popup() {
        let aperto = tasti(true, false, true);
        assert_eq!(
            overlay_key(aperto, &tasto(Named::ArrowDown), Modifiers::COMMAND),
            None
        );
        assert_eq!(
            overlay_key(aperto, &tasto(Named::Enter), Modifiers::ALT),
            None
        );
    }

    #[test]
    fn i_margini_interni_sono_dentro_la_larghezza() {
        let s = popup_size(&[riga("a", None)], 10);
        assert!((s.width - (CELL_W + 2.0 * BOX_PAD_X)).abs() < 0.01);
    }
}
