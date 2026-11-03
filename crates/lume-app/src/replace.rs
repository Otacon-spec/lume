//! La sostituzione nei file: il pezzo che **scrive davvero sul disco**.
//!
//! ATTENZIONE: È il gemello di [`crate::rename`] e ne condivide il nucleo — molte
//! modifiche sparse nello stesso testo — ma **non la politica**, e
//! confonderle sarebbe l'errore:
//!
//! | | Rinomina | Sostituzione |
//! |---|---|---|
//! | Scrive su disco? | **mai** | sì, è la sua natura |
//! | Perché | «fallire a metà» diventa impossibile per costruzione | l'utente ha chiesto di riscrivere il progetto, e conferma sapendo quanti file tocca |
//!
//! Quel che resta uguale è la parte che conta: **si valida tutto prima di
//! toccare qualunque cosa**. Un file che non si legge più, o una riga che non
//! è più quella che la ricerca aveva trovato, fermano il lavoro *prima* della
//! prima scrittura.
//!
//! ATTENZIONE: **I file chiusi si riscrivono a byte, non a caratteri.** Passarli da
//! `Document::from_str` normalizzerebbe BOM e CRLF, cioè riscriverebbe da
//! capo file che l'utente non ha chiesto di toccare — e su questa macchina
//! `core.autocrlf` è acceso, quindi sarebbe *ogni* riga di *ogni* file. Lo
//! splice sui byte lascia intatto tutto quel che non è un'occorrenza.

use std::fmt;
use std::path::{Path, PathBuf};

use lume_core::{ChangeSet, SearchHit};
use ropey::Rope;

/// Perché una sostituzione non si può fare. Ognuno di questi è un **no detto
/// prima**, non un guasto scoperto a metà.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplaceError {
    /// La riga non è più quella che la ricerca aveva trovato: il file è
    /// cambiato sotto, e le colonne parlerebbero di un altro testo.
    Stale(PathBuf),
    /// Il file non si legge più (o è sparito).
    Unreadable(PathBuf, String),
    /// Due occorrenze si accavallano: non c'è modo di sostituirle entrambe.
    Overlapping(PathBuf),
    /// Non produrre un file che l'editor rifiuterebbe di riaprire, né overflow.
    TooLarge(PathBuf),
}

impl fmt::Display for ReplaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stale(path) => write!(
                f,
                "{} è cambiato dopo la ricerca: rifà la ricerca prima di sostituire",
                path.display()
            ),
            Self::Unreadable(path, motivo) => {
                write!(f, "Lettura di {} fallita: {motivo}", path.display())
            }
            Self::TooLarge(path) => write!(
                f,
                "Sostituzione di {} troppo grande: il risultato supera il limite file di 32 MiB",
                path.display()
            ),
            Self::Overlapping(path) => write!(
                f,
                "Occorrenze sovrapposte in {}: non si possono sostituire insieme",
                path.display()
            ),
        }
    }
}

/// Gli intervalli assoluti, in **byte**, delle occorrenze dentro il file.
///
/// Un giro solo sul contenuto: le occorrenze sono già in ordine di riga e il
/// cursore avanza con loro. Cercare l'inizio di ogni riga da capo sarebbe
/// quadratico proprio sui file dove la sostituzione serve di più.
///
/// ATTENZIONE: La guardia è qui: la riga deve essere **ancora identica** a quella che
/// la ricerca aveva letto, terminatore compreso. Senza, una modifica fatta
/// nel frattempo sposterebbe le colonne e si taglierebbe in mezzo a un'altra
/// parola — senza nessun errore.
fn intervalli_byte(
    path: &Path,
    bytes: &[u8],
    hits: &[SearchHit],
) -> Result<Vec<std::ops::Range<usize>>, ReplaceError> {
    let mut ordinati: Vec<&SearchHit> = hits.iter().collect();
    ordinati.sort_by_key(|hit| (hit.line_number, hit.range_in_line.start));

    let mut fuori: Vec<std::ops::Range<usize>> = Vec::with_capacity(hits.len());
    let mut riga = 1u64;
    let mut inizio = 0usize;
    let mut scansione = 0usize;
    for hit in ordinati {
        if hit.line_number < riga {
            return Err(ReplaceError::Overlapping(path.to_path_buf()));
        }
        while riga < hit.line_number {
            let Some(byte) = bytes.get(scansione) else {
                return Err(ReplaceError::Stale(path.to_path_buf()));
            };
            scansione += 1;
            if *byte == b'\n' {
                riga += 1;
                inizio = scansione;
            }
        }
        let fine_testo = inizio + hit.line_text.len();
        if bytes.get(inizio..fine_testo) != Some(hit.line_text.as_bytes()) {
            return Err(ReplaceError::Stale(path.to_path_buf()));
        }
        // ...e finisce dove finisce: senza questo, cercare `ago` troverebbe
        // buona anche una riga diventata `agora`
        if !matches!(bytes.get(fine_testo), None | Some(b'\n' | b'\r')) {
            return Err(ReplaceError::Stale(path.to_path_buf()));
        }
        let da = inizio + hit.range_in_line.start;
        let a = inizio + hit.range_in_line.end;
        if da > a
            || a > fine_testo
            || !hit.line_text.is_char_boundary(hit.range_in_line.start)
            || !hit.line_text.is_char_boundary(hit.range_in_line.end)
        {
            return Err(ReplaceError::Stale(path.to_path_buf()));
        }
        if fuori.last().is_some_and(|ultimo| da < ultimo.end) {
            return Err(ReplaceError::Overlapping(path.to_path_buf()));
        }
        fuori.push(da..a);
    }
    Ok(fuori)
}

/// Il contenuto nuovo di un file **chiuso**, byte per byte.
///
/// Non modifica niente: restituisce il buffer da scrivere, così la scrittura
/// resta un passo a sé che avviene solo quando *tutti* i file hanno risposto
/// di sì.
pub fn replace_in_bytes(
    path: &Path,
    bytes: &[u8],
    hits: &[SearchHit],
    replacement: &str,
) -> Result<Vec<u8>, ReplaceError> {
    let intervalli = intervalli_byte(path, bytes, hits)?;
    let removed = intervalli.iter().map(|range| range.end - range.start).sum();
    let size = output_size(path, bytes.len(), removed, hits.len(), replacement.len())?;
    let mut fuori: Vec<u8> = Vec::with_capacity(size);
    let mut cursore = 0usize;
    for intervallo in intervalli {
        if let Some(prima) = bytes.get(cursore..intervallo.start) {
            fuori.extend_from_slice(prima);
        }
        fuori.extend_from_slice(replacement.as_bytes());
        cursore = intervallo.end;
    }
    if let Some(coda) = bytes.get(cursore..) {
        fuori.extend_from_slice(coda);
    }
    Ok(fuori)
}

/// Stesso limite del reader: evitare output che non si possono riaprire.
/// Il controllo precede le allocazioni del payload/insert ripetuti.
fn output_size(
    path: &Path,
    original: usize,
    removed: usize,
    count: usize,
    replacement: usize,
) -> Result<usize, ReplaceError> {
    let size = replacement
        .checked_mul(count)
        .and_then(|added| {
            original
                .checked_sub(removed)
                .and_then(|left| left.checked_add(added))
        })
        .ok_or_else(|| ReplaceError::TooLarge(path.to_path_buf()))?;
    if u64::try_from(size).unwrap_or(u64::MAX) > crate::file_read::MAX_FILE_BYTES {
        return Err(ReplaceError::TooLarge(path.to_path_buf()));
    }
    Ok(size)
}

/// La modifica da applicare a un documento **aperto**.
///
/// ATTENZIONE: Un documento aperto non si tocca da sotto, nemmeno se pulito: la
/// modifica passa dal suo `Document`, così l'annulla dell'editor la copre e
/// non c'è nessuna corsa con il watcher del filesystem. Ed è **una sola**
/// `ChangeSet`: annullare una sostituzione di sette occorrenze con sette
/// Ctrl+Z sarebbe una trappola, non una funzione.
pub fn changeset_for(
    path: &Path,
    rope: &Rope,
    hits: &[SearchHit],
    replacement: &str,
) -> Result<ChangeSet, ReplaceError> {
    let mut intervalli = Vec::with_capacity(hits.len());
    let mut removed = 0usize;
    for hit in hits {
        let indice = usize::try_from(hit.line_number)
            .ok()
            .and_then(|numero| numero.checked_sub(1))
            .ok_or_else(|| ReplaceError::Stale(path.to_path_buf()))?;
        if indice >= rope.len_lines() {
            return Err(ReplaceError::Stale(path.to_path_buf()));
        }
        let riga = rope.line(indice);
        // il rope è normalizzato a LF: il terminatore da togliere è uno solo
        let testo = riga.to_string();
        if testo.strip_suffix('\n').unwrap_or(&testo) != &*hit.line_text {
            return Err(ReplaceError::Stale(path.to_path_buf()));
        }
        if hit.range_in_line.start > hit.range_in_line.end
            || hit.range_in_line.end > hit.line_text.len()
            || !hit.line_text.is_char_boundary(hit.range_in_line.start)
            || !hit.line_text.is_char_boundary(hit.range_in_line.end)
        {
            return Err(ReplaceError::Stale(path.to_path_buf()));
        }
        removed = removed
            .checked_add(hit.range_in_line.end - hit.range_in_line.start)
            .ok_or_else(|| ReplaceError::TooLarge(path.to_path_buf()))?;
        let inizio = rope.line_to_char(indice);
        let (Some(da), Some(a)) = (
            riga.try_byte_to_char(hit.range_in_line.start).ok(),
            riga.try_byte_to_char(hit.range_in_line.end).ok(),
        ) else {
            return Err(ReplaceError::Stale(path.to_path_buf()));
        };
        intervalli.push((inizio + da, inizio + a));
    }
    intervalli.sort_unstable_by_key(|&(da, a)| (da, a));
    if intervalli.windows(2).any(|pair| {
        pair.first()
            .zip(pair.get(1))
            .is_some_and(|(left, right)| right.0 < left.1)
    }) {
        return Err(ReplaceError::Overlapping(path.to_path_buf()));
    }
    output_size(
        path,
        rope.len_bytes(),
        removed,
        hits.len(),
        replacement.len(),
    )?;
    let intervalli: Vec<_> = intervalli
        .into_iter()
        .map(|(da, a)| (da, a, replacement.to_owned()))
        .collect();
    // `replace_many` è costruttore **e** validatore: rifiuta intervalli
    // rovesciati, fuori dal testo o sovrapposti.
    ChangeSet::replace_many(&intervalli, rope.len_chars())
        .ok_or_else(|| ReplaceError::Overlapping(path.to_path_buf()))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::{ReplaceError, changeset_for, replace_in_bytes};
    use lume_core::SearchHit;
    use ropey::Rope;
    use std::path::Path;

    fn hit(line_number: u64, line_text: &str, range: std::ops::Range<usize>) -> SearchHit {
        SearchHit {
            line_number,
            line_text: line_text.into(),
            range_in_line: range,
        }
    }

    #[test]
    fn intervalli_a_meta_utf8_non_producono_file_corrotto() {
        let path = Path::new("utf8.txt");
        let hits = [hit(1, "èago", 1..2)];
        assert!(matches!(
            replace_in_bytes(path, "èago\n".as_bytes(), &hits, "X"),
            Err(ReplaceError::Stale(_))
        ));
        assert!(matches!(
            changeset_for(path, &Rope::from_str("èago\n"), &hits, "X"),
            Err(ReplaceError::Stale(_))
        ));
    }

    #[test]
    fn crescita_e_overflow_si_rifiutano_prima_di_allocare_payload() {
        let path = Path::new("large.txt");
        assert!(super::output_size(path, 3, 0, usize::MAX, 2).is_err());
        assert!(
            super::output_size(path, 3, 0, 1, crate::file_read::MAX_FILE_BYTES as usize).is_err()
        );
        assert_eq!(super::output_size(path, 3, 3, 1, 3).unwrap(), 3);
    }

    /// ATTENZIONE: Il test che giustifica lo splice sui byte: un file con BOM, righe
    /// CRLF e nessun a-capo finale deve restare **identico** dappertutto
    /// tranne che nelle occorrenze. Passandolo da un `Rope` normalizzato si
    /// riscriverebbe ogni riga, e su una macchina con `core.autocrlf` acceso
    /// il diff sarebbe il file intero.
    #[test]
    fn i_file_chiusi_si_riscrivono_senza_toccare_bom_crlf_e_coda() {
        let originale = b"\xEF\xBB\xBFuno ago due\r\nniente\r\nago e ago";
        let hits = [
            hit(1, "\u{feff}uno ago due", 7..10),
            hit(3, "ago e ago", 0..3),
            hit(3, "ago e ago", 6..9),
        ];
        let nuovo =
            replace_in_bytes(Path::new("f.txt"), originale, &hits, "AGO").expect("sostituito");
        assert_eq!(
            nuovo,
            b"\xEF\xBB\xBFuno AGO due\r\nniente\r\nAGO e AGO".to_vec(),
            "BOM, CRLF e la coda senza a-capo restano dov'erano"
        );
    }

    /// Una riga cambiata sotto ferma tutto: le colonne di prima parlerebbero
    /// di un altro testo, e si taglierebbe in mezzo a una parola senza che
    /// niente lo segnali.
    #[test]
    fn una_riga_cambiata_sotto_non_si_tocca() {
        let hits = [hit(2, "cerca ago qui", 6..9)];
        let esito = replace_in_bytes(Path::new("f.txt"), b"prima\ncerca AGO qui\n", &hits, "x");
        assert_eq!(
            esito,
            Err(ReplaceError::Stale(std::path::PathBuf::from("f.txt")))
        );

        // e nemmeno una riga che *comincia* uguale ma è diventata più lunga
        let hits = [hit(1, "ago", 0..3)];
        assert_eq!(
            replace_in_bytes(Path::new("f.txt"), b"agora\n", &hits, "x"),
            Err(ReplaceError::Stale(std::path::PathBuf::from("f.txt")))
        );

        // ...né una riga che non c'è più affatto
        let hits = [hit(9, "ago", 0..3)];
        assert_eq!(
            replace_in_bytes(Path::new("f.txt"), b"ago\n", &hits, "x"),
            Err(ReplaceError::Stale(std::path::PathBuf::from("f.txt")))
        );

        // ATTENZIONE: E vale **soprattutto** per i documenti aperti, che sono quelli
        // in cui l'utente sta scrivendo: fra la ricerca e il clic su
        // «Sostituisci tutto» c'è tutto il tempo di cambiare la riga.
        let rope = ropey::Rope::from_str("cerca AGO qui\n");
        assert!(matches!(
            changeset_for(
                Path::new("f.rs"),
                &rope,
                &[hit(1, "cerca ago qui", 6..9)],
                "x"
            ),
            Err(ReplaceError::Stale(_))
        ));
        assert!(
            matches!(
                changeset_for(
                    Path::new("f.rs"),
                    &rope,
                    &[hit(9, "cerca AGO qui", 6..9)],
                    "x"
                ),
                Err(ReplaceError::Stale(_))
            ),
            "e una riga oltre la fine del testo"
        );
    }

    /// I documenti aperti passano dal loro `Rope`, e ne esce **una** modifica
    /// sola: un Ctrl+Z, non uno per occorrenza.
    #[test]
    fn i_documenti_aperti_ricevono_una_modifica_sola() {
        let rope = ropey::Rope::from_str("uno ago due\nago\n");
        let hits = [hit(1, "uno ago due", 4..7), hit(2, "ago", 0..3)];
        let changeset = changeset_for(Path::new("f.rs"), &rope, &hits, "AGO").expect("piano");
        let mut documento =
            lume_core::Document::from_str("uno ago due\nago\n", None, lume_core::DocId(0));
        documento.apply(lume_core::Transaction {
            changes: changeset,
            selection_after: None,
        });
        assert_eq!(documento.rope().to_string(), "uno AGO due\nAGO\n");

        // e una sola revisione: l'annulla riporta tutto in un colpo
        documento.undo();
        assert_eq!(documento.rope().to_string(), "uno ago due\nago\n");
    }

    /// L'accento sulla conversione: con caratteri multibyte prima
    /// dell'occorrenza, byte e caratteri non coincidono. Prenderli per buoni
    /// funzionerebbe su tutto l'ASCII e sbaglierebbe sul resto.
    #[test]
    fn le_colonne_sono_byte_e_il_rope_conta_caratteri() {
        let rope = ropey::Rope::from_str("però ago\n");
        // «però » è 6 byte e 5 caratteri
        let hits = [hit(1, "però ago", 6..9)];
        let changeset = changeset_for(Path::new("f.rs"), &rope, &hits, "X").expect("piano");
        let mut documento = lume_core::Document::from_str("però ago\n", None, lume_core::DocId(0));
        documento.apply(lume_core::Transaction {
            changes: changeset,
            selection_after: None,
        });
        assert_eq!(documento.rope().to_string(), "però X\n");
    }
}
