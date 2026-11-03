//! Movimenti del cursore: funzioni pure su `&Rope`, posizioni in CHAR INDEX.
//!
//! I confini di grapheme sono calcolati con unicode-segmentation su una
//! finestra limitata alla riga: il testo è normalizzato a `\n`, che spezza
//! sempre i grapheme cluster, quindi la finestra è sufficiente e non paga
//! il costo di materializzare i chunk dell'intera rope.

use ropey::Rope;
use unicode_segmentation::UnicodeSegmentation;

/// Classe di carattere per il movimento a parole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CharClass {
    Word,  // alfanumerico + '_'
    Punct, // altra punteggiatura/simboli
    Space, // whitespace (incluso newline)
}

fn class_of(c: char) -> CharClass {
    if c.is_whitespace() {
        CharClass::Space
    } else if c.is_alphanumeric() || c == '_' {
        CharClass::Word
    } else {
        CharClass::Punct
    }
}

/// Lunghezza in char della riga `line`, escluso il newline finale.
pub fn line_content_len(rope: &Rope, line: usize) -> usize {
    let l = rope.line(line);
    let mut n = l.len_chars();
    if n > 0 && l.char(n - 1) == '\n' {
        n -= 1;
    }
    n
}

/// Inizio del grapheme precedente rispetto a `pos` (clamp a 0).
pub fn prev_grapheme(rope: &Rope, pos: usize) -> usize {
    let pos = pos.min(rope.len_chars());
    if pos == 0 {
        return 0;
    }
    // Il grapheme precedente vive nella riga che contiene il char pos-1
    // (un inizio riga è sempre un confine di grapheme).
    let line = rope.char_to_line(pos - 1);
    let line_start = rope.line_to_char(line);
    let window: String = rope.slice(line_start..pos).to_string();
    let mut start = line_start;
    for g in window.graphemes(true) {
        let n = g.chars().count();
        if start + n >= pos {
            return start;
        }
        start += n;
    }
    pos - 1 // difensivo, non raggiungibile
}

/// Fine del grapheme che inizia a (o contiene) `pos` (clamp a len).
pub fn next_grapheme(rope: &Rope, pos: usize) -> usize {
    let len = rope.len_chars();
    if pos >= len {
        return len;
    }
    let line = rope.char_to_line(pos);
    let line_start = rope.line_to_char(line);
    let line_end = line_start + rope.line(line).len_chars();
    let window: String = rope.slice(line_start..line_end).to_string();
    let mut start = line_start;
    for g in window.graphemes(true) {
        let n = g.chars().count();
        if start + n > pos {
            return (start + n).min(len);
        }
        start += n;
    }
    (pos + 1).min(len) // difensivo
}

/// Inizio della parola a sinistra: salta il whitespace, poi risale la run
/// di caratteri della stessa classe (parola o punteggiatura).
pub fn word_left(rope: &Rope, pos: usize) -> usize {
    let mut p = pos.min(rope.len_chars());
    let mut run_class: Option<CharClass> = None;
    for c in rope.chars_at(p).reversed() {
        let cls = class_of(c);
        match run_class {
            None => {
                p -= 1;
                if cls != CharClass::Space {
                    run_class = Some(cls);
                }
            }
            Some(k) => {
                if cls == k {
                    p -= 1;
                } else {
                    break;
                }
            }
        }
    }
    p
}

/// Fine della parola a destra: salta il whitespace, poi consuma la run di
/// caratteri della stessa classe.
pub fn word_right(rope: &Rope, pos: usize) -> usize {
    let len = rope.len_chars();
    let mut p = pos.min(len);
    let mut run_class: Option<CharClass> = None;
    for c in rope.chars_at(p) {
        let cls = class_of(c);
        match run_class {
            None => {
                p += 1;
                if cls != CharClass::Space {
                    run_class = Some(cls);
                }
            }
            Some(k) => {
                if cls == k {
                    p += 1;
                } else {
                    break;
                }
            }
        }
    }
    p
}

/// Riga precedente alla colonna `goal_col` (in char, sticky: la mantiene il
/// chiamante). Sulla prima riga il cursore non si muove.
pub fn line_up(rope: &Rope, pos: usize, goal_col: usize) -> usize {
    let pos = pos.min(rope.len_chars());
    let line = rope.char_to_line(pos);
    if line == 0 {
        return pos;
    }
    let target = line - 1;
    rope.line_to_char(target) + goal_col.min(line_content_len(rope, target))
}

/// Riga successiva alla colonna `goal_col` (sticky). Sull'ultima riga il
/// cursore non si muove.
pub fn line_down(rope: &Rope, pos: usize, goal_col: usize) -> usize {
    let pos = pos.min(rope.len_chars());
    let line = rope.char_to_line(pos);
    if line + 1 >= rope.len_lines() {
        return pos;
    }
    let target = line + 1;
    rope.line_to_char(target) + goal_col.min(line_content_len(rope, target))
}

/// Home intelligente: primo carattere non-whitespace della riga, oppure
/// colonna 0 se il cursore è già lì (alternanza).
pub fn line_home_smart(rope: &Rope, pos: usize) -> usize {
    let pos = pos.min(rope.len_chars());
    let line = rope.char_to_line(pos);
    let line_start = rope.line_to_char(line);
    let content_len = line_content_len(rope, line);
    let mut first_nws = 0usize;
    for c in rope.line(line).chars().take(content_len) {
        if c == ' ' || c == '\t' {
            first_nws += 1;
        } else {
            break;
        }
    }
    let target = line_start + first_nws;
    if pos == target { line_start } else { target }
}

/// Fine riga (prima del newline).
pub fn line_end(rope: &Rope, pos: usize) -> usize {
    let pos = pos.min(rope.len_chars());
    let line = rope.char_to_line(pos);
    rope.line_to_char(line) + line_content_len(rope, line)
}

/// Inizio del documento.
pub fn doc_start(_rope: &Rope) -> usize {
    0
}

/// Fine del documento.
pub fn doc_end(rope: &Rope) -> usize {
    rope.len_chars()
}

/// Testo da inserire per un newline con auto-indent: `\n` + whitespace
/// iniziale della riga corrente, più 4 spazi extra se il testo prima del
/// cursore (trimmato) termina con `{`, `(` o `[`.
pub fn auto_indent_for_newline(rope: &Rope, at: usize) -> String {
    let at = at.min(rope.len_chars());
    let line = rope.char_to_line(at);
    let line_start = rope.line_to_char(line);
    let before: String = rope.slice(line_start..at).to_string();
    let indent: String = before
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    let mut out = String::with_capacity(1 + indent.len() + 4);
    out.push('\n');
    out.push_str(&indent);
    if matches!(before.trim_end().chars().last(), Some('{' | '(' | '[')) {
        out.push_str("    ");
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    fn rope(s: &str) -> Rope {
        Rope::from_str(s)
    }

    /// "a👍🏽é́b": emoji con skin-tone modifier e 'é' precomposta + combining
    /// acute. Char: a(0) 👍(1) 🏽(2) é(3) U+0301(4) b(5) → confini 0,1,3,5,6.
    fn testo_graphemes() -> Rope {
        rope("a\u{1F44D}\u{1F3FD}\u{e9}\u{301}b")
    }

    #[test]
    fn grapheme_motion_avanti() {
        let r = testo_graphemes();
        assert_eq!(next_grapheme(&r, 0), 1);
        assert_eq!(next_grapheme(&r, 1), 3); // 👍🏽 = 2 char
        assert_eq!(next_grapheme(&r, 3), 5); // é + combining = 2 char
        assert_eq!(next_grapheme(&r, 5), 6);
        assert_eq!(next_grapheme(&r, 6), 6); // clamp a fine documento
        assert_eq!(next_grapheme(&r, 99), 6);
    }

    #[test]
    fn grapheme_motion_indietro() {
        let r = testo_graphemes();
        assert_eq!(prev_grapheme(&r, 6), 5);
        assert_eq!(prev_grapheme(&r, 5), 3);
        assert_eq!(prev_grapheme(&r, 3), 1);
        assert_eq!(prev_grapheme(&r, 1), 0);
        assert_eq!(prev_grapheme(&r, 0), 0);
        // posizione dentro un grapheme: torna al suo inizio
        assert_eq!(prev_grapheme(&r, 2), 1);
    }

    #[test]
    fn grapheme_attraverso_newline() {
        let r = rope("ab\ncd");
        // pos 3 = inizio riga 2: il grapheme precedente è '\n'
        assert_eq!(prev_grapheme(&r, 3), 2);
        assert_eq!(next_grapheme(&r, 2), 3);
    }

    #[test]
    fn word_motion() {
        //         0123456789012345
        let r = rope("foo_bar baz. qux");
        assert_eq!(word_right(&r, 0), 7); // fine di "foo_bar"
        assert_eq!(word_right(&r, 7), 11); // fine di "baz"
        assert_eq!(word_right(&r, 11), 12); // fine di "."
        assert_eq!(word_right(&r, 12), 16); // fine di "qux"
        assert_eq!(word_right(&r, 16), 16);

        assert_eq!(word_left(&r, 16), 13); // inizio di "qux"
        assert_eq!(word_left(&r, 13), 11); // inizio di "."
        assert_eq!(word_left(&r, 11), 8); // inizio di "baz"
        assert_eq!(word_left(&r, 8), 0); // inizio di "foo_bar"
        assert_eq!(word_left(&r, 0), 0);
    }

    #[test]
    fn line_up_down_con_goal_column() {
        // righe: "abc"(0..3) "de"(4..6) "fghij"(7..12)
        let r = rope("abc\nde\nfghij");
        // dalla riga 2 col 4 salgo: la riga "de" è corta → clamp a col 2
        assert_eq!(line_up(&r, 11, 4), 6);
        // dalla riga 1 col 4 (sticky) salgo: "abc" → col 3
        assert_eq!(line_up(&r, 6, 4), 3);
        // prima riga: fermo
        assert_eq!(line_up(&r, 3, 4), 3);
        // discesa con goal sticky
        assert_eq!(line_down(&r, 3, 3), 6); // "de" → col 2
        assert_eq!(line_down(&r, 6, 3), 10); // "fghij" → col 3
        assert_eq!(line_down(&r, 10, 3), 10); // ultima riga: fermo
    }

    #[test]
    fn home_smart_alterna() {
        let r = rope("    code here");
        assert_eq!(line_home_smart(&r, 13), 4); // → primo non-whitespace
        assert_eq!(line_home_smart(&r, 4), 0); // già lì → colonna 0
        assert_eq!(line_home_smart(&r, 0), 4); // da col 0 → indent
    }

    #[test]
    fn line_end_e_doc() {
        let r = rope("abc\ndef\n");
        assert_eq!(line_end(&r, 1), 3);
        assert_eq!(line_end(&r, 5), 7);
        assert_eq!(doc_start(&r), 0);
        assert_eq!(doc_end(&r), 8);
    }

    #[test]
    fn auto_indent_dentro_blocco() {
        let r = rope("    if x {");
        assert_eq!(auto_indent_for_newline(&r, 10), "\n        ");
        let r = rope("fn f(");
        assert_eq!(auto_indent_for_newline(&r, 5), "\n    ");
        let r = rope("  let v = [");
        assert_eq!(auto_indent_for_newline(&r, 11), "\n      ");
    }

    #[test]
    fn auto_indent_riga_semplice() {
        let r = rope("    let x = 1;");
        assert_eq!(auto_indent_for_newline(&r, 14), "\n    ");
        // enter a metà riga: conta solo il testo prima del cursore
        let r = rope("  foo { bar");
        assert_eq!(auto_indent_for_newline(&r, 7), "\n      ");
    }

    #[test]
    fn nessun_panic_su_rope_vuota() {
        let r = rope("");
        assert_eq!(prev_grapheme(&r, 0), 0);
        assert_eq!(next_grapheme(&r, 0), 0);
        assert_eq!(word_left(&r, 0), 0);
        assert_eq!(word_right(&r, 0), 0);
        assert_eq!(line_up(&r, 0, 5), 0);
        assert_eq!(line_down(&r, 0, 5), 0);
        assert_eq!(line_home_smart(&r, 0), 0);
        assert_eq!(line_end(&r, 0), 0);
        assert_eq!(auto_indent_for_newline(&r, 0), "\n");
    }
}
