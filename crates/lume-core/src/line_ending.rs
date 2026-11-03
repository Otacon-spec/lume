//! Rilevamento e normalizzazione dei fine riga (LF / CRLF) e del BOM UTF-8.
//!
//! Il buffer interno lavora SEMPRE con `\n`: la conversione avviene in
//! ingresso (`normalize`) e in uscita (`LineEnding::restore`).

/// Fine riga di un documento. In caso di testo misto vince il dominante.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineEnding {
    #[default]
    Lf,
    CrLf,
}

/// BOM UTF-8 come stringa (U+FEFF).
pub const BOM: char = '\u{feff}';

impl LineEnding {
    /// Rileva il fine riga dominante del testo. Un `\r` isolato conta come
    /// voto per `Lf` (verrà comunque normalizzato a `\n`).
    pub fn detect(text: &str) -> LineEnding {
        let bytes = text.as_bytes();
        let mut crlf = 0usize;
        let mut lf = 0usize;
        for (i, &b) in bytes.iter().enumerate() {
            match b {
                b'\n' => {
                    if i > 0 && bytes.get(i - 1) == Some(&b'\r') {
                        crlf += 1;
                    } else {
                        lf += 1;
                    }
                }
                b'\r' if bytes.get(i + 1) != Some(&b'\n') => {
                    // \r isolato (vecchio Mac).
                    lf += 1;
                }
                _ => {}
            }
        }
        if crlf > lf {
            LineEnding::CrLf
        } else {
            LineEnding::Lf
        }
    }

    /// Rappresentazione testuale del fine riga.
    pub fn as_str(self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::CrLf => "\r\n",
        }
    }

    /// Riapplica il fine riga originale a un testo normalizzato a `\n`.
    pub fn restore(self, text: &str) -> String {
        match self {
            LineEnding::Lf => text.to_string(),
            LineEnding::CrLf => text.replace('\n', "\r\n"),
        }
    }
}

/// Rimuove l'eventuale BOM UTF-8 iniziale. Ritorna il testo senza BOM e un
/// flag che indica se era presente (da conservare per il salvataggio).
pub fn strip_bom(text: &str) -> (&str, bool) {
    match text.strip_prefix(BOM) {
        Some(rest) => (rest, true),
        None => (text, false),
    }
}

/// Converte ogni fine riga (`\r\n` e `\r` isolato) in `\n` e ritorna il
/// fine riga dominante rilevato, da usare poi in `restore`.
pub fn normalize(text: &str) -> (String, LineEnding) {
    let ending = LineEnding::detect(text);
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            out.push('\n');
        } else {
            out.push(c);
        }
    }
    (out, ending)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn detect_puro() {
        assert_eq!(LineEnding::detect("a\nb\n"), LineEnding::Lf);
        assert_eq!(LineEnding::detect("a\r\nb\r\n"), LineEnding::CrLf);
        assert_eq!(LineEnding::detect("senza newline"), LineEnding::Lf);
    }

    #[test]
    fn detect_misto_vince_dominante() {
        assert_eq!(LineEnding::detect("a\r\nb\r\nc\n"), LineEnding::CrLf);
        assert_eq!(LineEnding::detect("a\nb\nc\r\n"), LineEnding::Lf);
    }

    #[test]
    fn normalize_e_restore_roundtrip() {
        let (norm, le) = normalize("a\r\nb\r\n");
        assert_eq!(norm, "a\nb\n");
        assert_eq!(le, LineEnding::CrLf);
        assert_eq!(le.restore(&norm), "a\r\nb\r\n");
    }

    #[test]
    fn normalize_cr_isolato() {
        let (norm, le) = normalize("a\rb\rc");
        assert_eq!(norm, "a\nb\nc");
        assert_eq!(le, LineEnding::Lf);
    }

    #[test]
    fn bom_strip() {
        let (text, bom) = strip_bom("\u{feff}ciao");
        assert_eq!(text, "ciao");
        assert!(bom);
        let (text, bom) = strip_bom("ciao");
        assert_eq!(text, "ciao");
        assert!(!bom);
    }
}
