//! Le posizioni: il **char** di `lume-core` contro l'**UTF-16** del protocollo.
//!
//! ATTENZIONE: È il punto dove si sbaglia nel modo peggiore. Le nostre capability
//! dichiarano `positionEncodings: ["utf-16"]` (capability `general` in `lume-lsp/src/session.rs`) e ropey
//! lavora in char: sull'ASCII i due conteggi coincidono, quindi una
//! conversione sbagliata **funziona su quasi tutto** e sbaglia in silenzio
//! appena in una riga entra un'emoji, un ideogramma o un simbolo fuori dal
//! piano base. Il salto atterra qualche colonna più in là, la rinomina
//! mangia una lettera di troppo, e niente lo segnala.
//!
//! Le due funzioni sono pure e stanno qui da sole apposta: sono provabili
//! senza far girare l'app, ed è l'unico modo di prendere quel difetto.

use lsp_types::Position;
use ropey::Rope;

/// Da indice di carattere a posizione del protocollo.
///
/// `None` se l'indice è oltre la fine del testo.
pub fn position_of(rope: &Rope, char_index: usize) -> Option<Position> {
    let line = rope.try_char_to_line(char_index).ok()?;
    let inizio = rope.try_line_to_char(line).ok()?;
    // ATTENZIONE: `char_to_utf16_cu` conta **da inizio file**: la colonna è la
    // differenza fra i due, non il valore assoluto. Dimenticarlo dà colonne
    // giuste sulla prima riga e crescenti all'infinito su tutte le altre.
    let character = rope
        .try_char_to_utf16_cu(char_index)
        .ok()?
        .checked_sub(rope.try_char_to_utf16_cu(inizio).ok()?)?;
    Some(Position {
        line: u32::try_from(line).ok()?,
        character: u32::try_from(character).ok()?,
    })
}

/// Da posizione del protocollo a indice di carattere.
///
/// Due indulgenze volute, ed entrambe le chiede la specifica LSP:
/// - una colonna oltre la fine della riga si **riporta a fine riga**, invece
///   di sconfinare nella successiva;
/// - una colonna che cade **in mezzo a una coppia surrogata** dà il carattere
///   a cui appartiene, invece di un errore. Un server che conta in UTF-16 può
///   legittimamente spezzare lì, e rifiutare renderebbe un simbolo
///   inavvicinabile.
///
/// `None` solo se la riga non esiste: quella è una risposta che parla di un
/// altro testo, e va scartata.
pub fn char_index_of(rope: &Rope, position: Position) -> Option<usize> {
    let line = usize::try_from(position.line).ok()?;
    if line >= rope.len_lines() {
        return None;
    }
    let inizio = rope.try_line_to_char(line).ok()?;
    let base = rope.try_char_to_utf16_cu(inizio).ok()?;
    let tetto = rope.try_char_to_utf16_cu(fine_riga(rope, line)?).ok()?;
    let voluto = base
        .saturating_add(usize::try_from(position.character).ok()?)
        .min(tetto);
    rope.try_utf16_cu_to_char(voluto).ok()
}

/// L'indice di carattere a fine riga, **fine riga escluso**.
///
/// Serve al tetto: senza, una colonna esagerata scavalcherebbe il `\n` e
/// atterrerebbe sulla riga dopo — che è peggio di un errore, perché sembra
/// aver funzionato.
fn fine_riga(rope: &Rope, line: usize) -> Option<usize> {
    let inizio = rope.try_line_to_char(line).ok()?;
    let riga = rope.get_line(line)?;
    let mut lunghezza = riga.len_chars();
    if lunghezza > 0 && riga.char(lunghezza - 1) == '\n' {
        lunghezza -= 1;
    }
    if lunghezza > 0 && riga.char(lunghezza - 1) == '\r' {
        lunghezza -= 1;
    }
    Some(inizio + lunghezza)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::{char_index_of, position_of};
    use lsp_types::Position;
    use ropey::Rope;

    /// ATTENZIONE: Il test che vale più di tutti quelli della fase, e la sua forma non
    /// è casuale: l'emoji sta su una riga **precedente** a quella che si
    /// misura. Con l'emoji sulla stessa riga (`"a😀b"` e basta) una
    /// conversione che dimentica di sottrarre l'inizio riga passerebbe lo
    /// stesso, perché lì inizio riga è zero. Qui no: `b` è il quinto
    /// carattere e la quinta unità UTF-16 del file, ma è il **secondo** della
    /// sua riga.
    #[test]
    fn un_emoji_sposta_le_colonne_e_le_righe_restano_al_loro_posto() {
        let rope = Rope::from_str("x😀\nab");
        // x=0  😀=1  \n=2  a=3  b=4
        assert_eq!(position_of(&rope, 4), Some(Position::new(1, 1)), "b");
        assert_eq!(position_of(&rope, 3), Some(Position::new(1, 0)), "a");
        assert_eq!(
            position_of(&rope, 2),
            Some(Position::new(0, 3)),
            "fine della prima riga: l'emoji vale due unità"
        );
        assert_eq!(position_of(&rope, 1), Some(Position::new(0, 1)), "l'emoji");

        // e l'andata e ritorno torna, per ogni carattere del testo
        for indice in 0..=rope.len_chars() {
            let posizione = position_of(&rope, indice).expect("dentro il testo");
            assert_eq!(
                char_index_of(&rope, posizione),
                Some(indice),
                "andata e ritorno su {indice}"
            );
        }
        assert_eq!(position_of(&rope, rope.len_chars() + 1), None, "oltre");
    }

    /// Le due indulgenze che chiede la specifica, e la sola durezza che serve.
    #[test]
    fn una_colonna_esagerata_si_ferma_a_fine_riga_e_una_a_meta_emoji_non_esplode() {
        let rope = Rope::from_str("x😀\nab\r\nc");
        assert_eq!(
            char_index_of(&rope, Position::new(0, 99)),
            Some(2),
            "si ferma prima del \\n, non sconfina nella riga dopo"
        );
        assert_eq!(
            char_index_of(&rope, Position::new(1, 99)),
            Some(5),
            "e con il \\r\\n si ferma prima di tutti e due"
        );

        // in mezzo alla coppia surrogata: si dà il carattere a cui appartiene
        let piccola = Rope::from_str("a😀b");
        assert_eq!(char_index_of(&piccola, Position::new(0, 2)), Some(1));
        assert_eq!(char_index_of(&piccola, Position::new(0, 3)), Some(2), "b");

        // una riga che non c'è è una risposta che parla di un altro testo
        assert_eq!(char_index_of(&rope, Position::new(9, 0)), None);
    }
}
