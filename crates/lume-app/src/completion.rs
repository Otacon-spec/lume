//! La sessione di completamento: che cosa mostrare, come filtrarlo, che
//! cosa scrivere alla conferma. Puro: niente iced, niente I/O.
//!
//! ATTENZIONE: Due cose qui si rompono in silenzio e valgono i test più lunghi:
//! la colonna di una modifica del server conta in **UTF-16** (`lsp_pos`),
//! e la sostituzione arriva **fino al cursore di adesso**, non fino alla
//! fine che il server aveva in mente quando ha risposto.

use std::iter::Peekable;
use std::ops::Range;
use std::str::Chars;

use lsp_types::{
    CompletionItem, CompletionResponse, CompletionTextEdit, InsertTextFormat, Position,
};
use lume_core::DocId;
use ropey::Rope;

use crate::lsp_pos::char_index_of;

/// Al massimo tante righe nel popup.
pub const MAX_ROWS: usize = 100;

/// Alfanumerico o `_`: la stessa classe di `movement::class_of`.
pub fn is_word_char(carattere: char) -> bool {
    carattere.is_alphanumeric() || carattere == '_'
}

/// Che cosa scrivere al posto di che cosa.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replacement {
    /// Intervallo in **char** del documento.
    pub range: Range<usize>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    label: String,
    detail: Option<String>,
    sort_key: String,
    filter_text: String,
    /// Il testo già ripulito dai segnaposto.
    text: String,
    /// Inizio della modifica del server, solo se su una riga sola.
    edit_start: Option<Position>,
}

impl Entry {
    fn from_item(item: CompletionItem) -> Self {
        let (text, edit_start) = match item.text_edit {
            Some(CompletionTextEdit::Edit(edit)) => (Some(edit.new_text), Some(edit.range)),
            Some(CompletionTextEdit::InsertAndReplace(edit)) => {
                (Some(edit.new_text), Some(edit.insert))
            }
            None => (None, None),
        };
        let text = text
            .or(item.insert_text)
            .unwrap_or_else(|| item.label.clone());
        let text = if item.insert_text_format == Some(InsertTextFormat::SNIPPET) {
            plain_from_snippet(&text)
        } else {
            text
        };
        Self {
            sort_key: item.sort_text.unwrap_or_else(|| item.label.clone()),
            filter_text: item.filter_text.unwrap_or_else(|| item.label.clone()),
            detail: item.detail,
            label: item.label,
            text,
            edit_start: edit_start
                .filter(|range| range.start.line == range.end.line)
                .map(|range| range.start),
        }
    }
}

/// Un completamento restituito dal server, con la versione su cui è stato chiesto.
#[derive(Debug, Clone)]
pub struct CompletionSession {
    pub doc: DocId,
    pub version: u64,
    anchor: usize,
    entries: Vec<Entry>,
    /// Indici in `entries`, già filtrati e ordinati.
    shown: Vec<usize>,
    selected: usize,
}

impl CompletionSession {
    /// `None` se dopo il filtro non resta niente da mostrare.
    pub fn new(
        doc: DocId,
        version: u64,
        rope: &Rope,
        head: usize,
        response: CompletionResponse,
    ) -> Option<Self> {
        let items = match response {
            CompletionResponse::Array(items) => items,
            CompletionResponse::List(list) => list.items,
        };
        let mut session = Self {
            doc,
            version,
            anchor: word_start(rope, head),
            entries: items.into_iter().map(Entry::from_item).collect(),
            shown: Vec::new(),
            selected: 0,
        };
        session.refilter(rope, head).then_some(session)
    }

    pub const fn anchor(&self) -> usize {
        self.anchor
    }

    pub const fn selected(&self) -> usize {
        self.selected
    }

    /// Etichetta e dettaglio delle voci visibili, nell'ordine di lista.
    pub fn rows(&self) -> Vec<(&str, Option<&str>)> {
        self.shown
            .iter()
            .filter_map(|&index| self.entries.get(index))
            .map(|entry| (entry.label.as_str(), entry.detail.as_deref()))
            .collect()
    }

    /// Rifà il filtro col testo tra l'ancora e il cursore. `false` = non
    /// resta niente (o il cursore è uscito dalla parola): la sessione va
    /// chiusa. La selezione torna in cima.
    pub fn refilter(&mut self, rope: &Rope, head: usize) -> bool {
        self.shown.clear();
        self.selected = 0;
        if head < self.anchor || head > rope.len_chars() {
            return false;
        }
        let prefisso: String = rope.slice(self.anchor..head).chars().collect();
        if !prefisso.chars().all(is_word_char) {
            return false;
        }
        let minuscolo = prefisso.to_lowercase();
        let mut trovate: Vec<(bool, usize)> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.filter_text.to_lowercase().starts_with(&minuscolo))
            // `false` ordina prima di `true`: le esatte in cima
            .map(|(index, entry)| (!entry.filter_text.starts_with(&prefisso), index))
            .collect();
        trovate.sort_by(|(rango_a, a), (rango_b, b)| {
            let (Some(voce_a), Some(voce_b)) = (self.entries.get(*a), self.entries.get(*b)) else {
                return std::cmp::Ordering::Equal;
            };
            rango_a
                .cmp(rango_b)
                .then_with(|| voce_a.sort_key.cmp(&voce_b.sort_key))
                .then_with(|| voce_a.label.cmp(&voce_b.label))
        });
        self.shown = trovate
            .into_iter()
            .take(MAX_ROWS)
            .map(|(_, index)| index)
            .collect();
        !self.shown.is_empty()
    }

    pub fn select_next(&mut self) {
        if !self.shown.is_empty() {
            self.selected = (self.selected + 1) % self.shown.len();
        }
    }

    pub fn select_prev(&mut self) {
        if !self.shown.is_empty() {
            self.selected = (self.selected + self.shown.len() - 1) % self.shown.len();
        }
    }

    /// Che cosa scrivere per la voce selezionata. `None` se non c'è una
    /// voce o il cursore non sta nel testo.
    pub fn accept(&self, rope: &Rope, head: usize) -> Option<Replacement> {
        if head > rope.len_chars() {
            return None;
        }
        let entry = self.entries.get(*self.shown.get(self.selected)?)?;
        let riga_cursore = rope.char_to_line(head);
        let inizio = entry
            .edit_start
            .and_then(|posizione| char_index_of(rope, posizione))
            .filter(|&inizio| inizio <= head && rope.char_to_line(inizio) == riga_cursore)
            .unwrap_or(self.anchor.min(head));
        Some(Replacement {
            range: inizio..head,
            text: entry.text.clone(),
        })
    }
}

pub fn word_start(rope: &Rope, head: usize) -> usize {
    let mut inizio = head.min(rope.len_chars());
    while inizio > 0 && is_word_char(rope.char(inizio - 1)) {
        inizio -= 1;
    }
    inizio
}

/// Il testo di uno snippet senza i segnaposto: `$0`, `$1` e `${1}` spariscono,
/// `${1:x}` diventa `x`, `${1|a,b|}` diventa `a`, `$NOME` sparisce, `\$` `\}`
/// `\\` perdono la barra. Un `$` che non apre niente resta com'è.
pub fn plain_from_snippet(snippet: &str) -> String {
    let mut fuori = String::with_capacity(snippet.len());
    expand(&mut snippet.chars().peekable(), false, &mut fuori);
    fuori
}

fn expand(chars: &mut Peekable<Chars<'_>>, nested: bool, out: &mut String) {
    while let Some(carattere) = chars.next() {
        match carattere {
            '\\' => match chars.peek().copied() {
                Some(seguente @ ('$' | '}' | '\\')) => {
                    out.push(seguente);
                    chars.next();
                }
                _ => out.push('\\'),
            },
            '}' if nested => return,
            '$' => dollar(chars, out),
            altro => out.push(altro),
        }
    }
}

fn dollar(chars: &mut Peekable<Chars<'_>>, out: &mut String) {
    match chars.peek().copied() {
        Some('{') => {
            chars.next();
            while chars.peek().is_some_and(|&c| is_word_char(c)) {
                chars.next();
            }
            match chars.peek().copied() {
                // segnaposto con testo: il testo resta (e si chiude sulla `}`)
                Some(':') => {
                    chars.next();
                    expand(chars, true, out);
                }
                // scelte: resta la prima
                Some('|') => {
                    chars.next();
                    let mut prima = String::new();
                    let mut finita = false;
                    for c in chars.by_ref() {
                        match c {
                            '|' => break,
                            ',' => finita = true,
                            altro if !finita => prima.push(altro),
                            _ => {}
                        }
                    }
                    out.push_str(&prima);
                    for c in chars.by_ref() {
                        if c == '}' {
                            break;
                        }
                    }
                }
                // `${1}`, `${nome}`, trasformazioni: spariscono fino alla `}`
                _ => {
                    let mut profondita = 1usize;
                    for c in chars.by_ref() {
                        match c {
                            '{' => profondita += 1,
                            '}' => {
                                profondita -= 1;
                                if profondita == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        Some(c) if is_word_char(c) => {
            while chars.peek().is_some_and(|&c| is_word_char(c)) {
                chars.next();
            }
        }
        _ => out.push('$'),
    }
}

/// Il carattere appena scritto apre da solo il completamento?
///
/// `line_before` è il testo della riga **fino al cursore, carattere appena
/// scritto compreso**. Sì per: la prima lettera (non cifra) di una parola,
/// `.` (ma non `..`), `::` e `->`. Un testo di più caratteri (un incolla) o
/// uno spazio no. L'elenco è fisso nella v1: non si legge da
/// `triggerCharacters` del server.
pub fn should_open(inserted: &str, line_before: &str) -> bool {
    let mut caratteri = inserted.chars();
    let (Some(scritto), None) = (caratteri.next(), caratteri.next()) else {
        return false;
    };
    let precedente = line_before.chars().rev().nth(1);
    match scritto {
        c if is_word_char(c) => !c.is_ascii_digit() && !precedente.is_some_and(is_word_char),
        '.' => precedente != Some('.'),
        ':' => precedente == Some(':'),
        '>' => precedente == Some('-'),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use lsp_types::{
        CompletionItem, CompletionList, CompletionResponse, CompletionTextEdit, InsertTextFormat,
        Position, Range as LspRange, TextEdit,
    };
    use lume_core::DocId;
    use ropey::Rope;

    use super::{CompletionSession, MAX_ROWS, plain_from_snippet, should_open};

    fn voce(label: &str) -> CompletionItem {
        CompletionItem {
            label: label.to_owned(),
            ..CompletionItem::default()
        }
    }

    fn lista(voci: Vec<CompletionItem>) -> CompletionResponse {
        CompletionResponse::List(CompletionList {
            is_incomplete: false,
            items: voci,
        })
    }

    fn sessione(testo: &str, voci: Vec<CompletionItem>) -> Option<(CompletionSession, Rope)> {
        let rope = Rope::from_str(testo);
        let testa = rope.len_chars();
        CompletionSession::new(DocId(1), 1, &rope, testa, lista(voci)).map(|s| (s, rope))
    }

    fn etichette(sessione: &CompletionSession) -> Vec<String> {
        sessione
            .rows()
            .into_iter()
            .map(|(etichetta, _)| etichetta.to_owned())
            .collect()
    }

    #[test]
    fn l_ancora_e_l_inizio_della_parola_sotto_il_cursore() {
        let (s, _) = sessione("let x = ve", vec![voce("vec"), voce("value")]).unwrap();
        assert_eq!(s.doc, DocId(1));
        assert_eq!(s.version, 1);
        assert_eq!(s.anchor(), 8);
        let (s, _) = sessione("x.", vec![voce("len")]).unwrap();
        assert_eq!(s.anchor(), 2, "dopo il punto il prefisso è vuoto");
    }

    #[test]
    fn il_filtro_ignora_le_maiuscole_ma_le_esatte_vengono_prima() {
        let (s, _) = sessione(
            "Ve",
            vec![voce("vec"), voce("Vector"), voce("Vec"), voce("zeta")],
        )
        .unwrap();
        assert_eq!(etichette(&s), vec!["Vec", "Vector", "vec"]);
    }

    #[test]
    fn il_filtro_usa_filter_text_e_l_ordine_usa_sort_text() {
        let mut a = voce("push(…)");
        a.filter_text = Some("push".to_owned());
        a.sort_text = Some("2".to_owned());
        let mut b = voce("pub_fn");
        b.sort_text = Some("1".to_owned());
        let (s, _) = sessione("pu", vec![a, b]).unwrap();
        assert_eq!(etichette(&s), vec!["pub_fn", "push(…)"]);
    }

    #[test]
    fn i_dettagli_viaggiano_con_le_righe() {
        let mut a = voce("push");
        a.detail = Some("fn(&mut self, T)".to_owned());
        let (s, _) = sessione("p", vec![a, voce("pop")]).unwrap();
        let righe = s.rows();
        assert_eq!(righe[0], ("pop", None));
        assert_eq!(righe[1], ("push", Some("fn(&mut self, T)")));
    }

    /// Review Focus 3: zero voci, o voci che il prefisso scarta tutte, non
    /// aprono un popup vuoto.
    #[test]
    fn nessuna_voce_visibile_non_apre_niente() {
        assert!(sessione("x", vec![]).is_none(), "zero voci");
        assert!(
            sessione("zz", vec![voce("alpha")]).is_none(),
            "tutte scartate"
        );
        let (mut s, rope) = sessione("a", vec![voce("alpha")]).unwrap();
        let testa = rope.len_chars();
        assert!(s.refilter(&rope, testa));
        let scritto = Rope::from_str("az");
        assert!(!s.refilter(&scritto, 2), "il prefisso non trova più niente");
        assert!(s.rows().is_empty());
    }

    #[test]
    fn il_popup_si_chiude_se_il_cursore_torna_prima_dell_ancora() {
        let (mut s, _) = sessione("ab cd", vec![voce("cdef")]).unwrap();
        let rope = Rope::from_str("ab cd");
        assert!(!s.refilter(&rope, 1), "cursore prima dell'ancora");
    }

    #[test]
    fn una_parola_con_un_carattere_estraneo_chiude_il_popup() {
        let (mut s, _) = sessione("a", vec![voce("alpha")]).unwrap();
        let rope = Rope::from_str("a b");
        assert!(!s.refilter(&rope, 3), "lo spazio finisce la parola");
    }

    #[test]
    fn le_righe_sono_al_massimo_cento() {
        let voci: Vec<_> = (0..250).map(|i| voce(&format!("voce{i:03}"))).collect();
        let (s, _) = sessione("voce", voci).unwrap();
        assert_eq!(s.rows().len(), MAX_ROWS);
    }

    #[test]
    fn la_selezione_gira_in_tondo() {
        let (mut s, _) = sessione("", vec![voce("a"), voce("b"), voce("c")]).unwrap();
        assert_eq!(s.selected(), 0);
        s.select_prev();
        assert_eq!(s.selected(), 2, "da capo va in fondo");
        s.select_next();
        assert_eq!(s.selected(), 0, "dal fondo torna in cima");
        s.select_next();
        assert_eq!(s.selected(), 1);
    }

    #[test]
    fn il_filtro_riporta_la_selezione_in_cima() {
        let (mut s, _) = sessione("", vec![voce("ab"), voce("ac"), voce("ad")]).unwrap();
        s.select_next();
        s.select_next();
        let rope = Rope::from_str("a");
        assert!(s.refilter(&rope, 1));
        assert_eq!(s.selected(), 0);
    }

    #[test]
    fn senza_text_edit_si_sostituisce_dall_ancora_al_cursore() {
        let (s, rope) = sessione("let v = ve", vec![voce("vec")]).unwrap();
        let sostituzione = s.accept(&rope, rope.len_chars()).unwrap();
        assert_eq!(sostituzione.range, 8..10);
        assert_eq!(sostituzione.text, "vec");
    }

    #[test]
    fn il_testo_segue_la_precedenza_text_edit_insert_text_label() {
        let mut a = voce("etichetta");
        a.insert_text = Some("da_insert_text".to_owned());
        a.text_edit = Some(CompletionTextEdit::Edit(TextEdit {
            range: LspRange::new(Position::new(0, 0), Position::new(0, 1)),
            new_text: "da_text_edit".to_owned(),
        }));
        let mut b = voce("etichetta");
        b.insert_text = Some("da_insert_text".to_owned());
        let c = voce("etichetta");
        for (item, atteso) in [(a, "da_text_edit"), (b, "da_insert_text"), (c, "etichetta")] {
            let (s, rope) = sessione("e", vec![item]).unwrap();
            assert_eq!(s.accept(&rope, 1).unwrap().text, atteso);
        }
    }

    /// Review Focus 4: la modifica del server conta in UTF-16. Dopo
    /// un'emoji (due unità, un solo char) prendere la colonna per un char
    /// sposterebbe l'inizio di una lettera: il test ha la modifica che
    /// comincia **prima** dell'ancora, così i due conteggi non coincidono.
    #[test]
    fn la_modifica_del_server_si_legge_in_utf16() {
        // char: 😀=0  .=1  f=2  o=3   UTF-16: 😀=0..1  .=2  f=3  o=4
        let rope = Rope::from_str("😀.fo");
        let mut voce_edit = voce("foo()");
        voce_edit.text_edit = Some(CompletionTextEdit::Edit(TextEdit {
            range: LspRange::new(Position::new(0, 2), Position::new(0, 5)),
            new_text: ".foo()".to_owned(),
        }));
        let s = CompletionSession::new(DocId(1), 1, &rope, 4, lista(vec![voce_edit])).unwrap();
        let sostituzione = s.accept(&rope, 4).unwrap();
        assert_eq!(sostituzione.range, 1..4, "da '.' al cursore, non 2..4");
        assert_eq!(sostituzione.text, ".foo()");
    }

    #[test]
    fn la_fine_della_modifica_e_il_cursore_di_adesso() {
        // il server risponde quando c'è scritto "fo"; nel frattempo l'utente
        // ha aggiunto una "o": si sostituisce fino al cursore, non a col 2
        let rope = Rope::from_str("foo");
        let mut v = voce("foobar");
        v.text_edit = Some(CompletionTextEdit::Edit(TextEdit {
            range: LspRange::new(Position::new(0, 0), Position::new(0, 2)),
            new_text: "foobar".to_owned(),
        }));
        let s = CompletionSession::new(DocId(1), 1, &rope, 3, lista(vec![v])).unwrap();
        assert_eq!(s.accept(&rope, 3).unwrap().range, 0..3);
    }

    #[test]
    fn una_modifica_su_un_altra_riga_ripiega_sull_ancora() {
        let rope = Rope::from_str("ab\ncd");
        let mut v = voce("cdef");
        v.text_edit = Some(CompletionTextEdit::Edit(TextEdit {
            range: LspRange::new(Position::new(0, 0), Position::new(0, 2)),
            new_text: "cdef".to_owned(),
        }));
        let s = CompletionSession::new(DocId(1), 1, &rope, 5, lista(vec![v])).unwrap();
        assert_eq!(
            s.accept(&rope, 5).unwrap().range,
            3..5,
            "ancora, non riga 0"
        );
    }

    #[test]
    fn una_modifica_dopo_il_cursore_si_ignora() {
        // il cursore sta dopo "cd", il testo continua: la modifica (col 7)
        // comincia dopo il cursore
        let rope = Rope::from_str("ab cd xy");
        let mut v = voce("cdef");
        v.text_edit = Some(CompletionTextEdit::Edit(TextEdit {
            range: LspRange::new(Position::new(0, 7), Position::new(0, 7)),
            new_text: "cdef".to_owned(),
        }));
        let s = CompletionSession::new(DocId(1), 1, &rope, 5, lista(vec![v])).unwrap();
        assert_eq!(s.accept(&rope, 5).unwrap().range, 3..5);
    }

    #[test]
    fn uno_snippet_perde_i_segnaposto_all_inserimento() {
        let rope = Rope::from_str("pu");
        let mut v = voce("push");
        v.insert_text = Some("push(${1:valore})$0".to_owned());
        v.insert_text_format = Some(InsertTextFormat::SNIPPET);
        let s = CompletionSession::new(DocId(1), 1, &rope, 2, lista(vec![v])).unwrap();
        assert_eq!(s.accept(&rope, 2).unwrap().text, "push(valore)");
        let mut semplice = voce("costo");
        semplice.insert_text = Some("a$1".to_owned());
        let s = CompletionSession::new(DocId(1), 1, &Rope::from_str("c"), 1, lista(vec![semplice]))
            .unwrap();
        assert_eq!(
            s.accept(&Rope::from_str("c"), 1).unwrap().text,
            "a$1",
            "testo semplice: nessuna interpretazione"
        );
    }

    #[test]
    fn accettare_con_il_cursore_fuori_dal_testo_non_va_in_panico() {
        let (s, rope) = sessione("ve", vec![voce("vec")]).unwrap();
        assert!(s.accept(&rope, 99).is_none());
    }

    #[test]
    fn i_segnaposto_degli_snippet() {
        assert_eq!(plain_from_snippet("push($0)"), "push()");
        assert_eq!(
            plain_from_snippet("for ${1:item} in ${2:iter} {\n\t$0\n}"),
            "for item in iter {\n\t\n}"
        );
        assert_eq!(plain_from_snippet("${1|a,b,c|}"), "a", "la prima scelta");
        assert_eq!(plain_from_snippet("\\$x \\} \\\\"), "$x } \\");
        assert_eq!(
            plain_from_snippet("$TM_FILENAME.rs"),
            ".rs",
            "variabile: vuota"
        );
        assert_eq!(
            plain_from_snippet("costo $"),
            "costo $",
            "dollaro solitario"
        );
        assert_eq!(
            plain_from_snippet("${1:a${2:b}}"),
            "ab",
            "segnaposto annidati"
        );
        assert_eq!(plain_from_snippet("${1}x"), "x");
        assert_eq!(plain_from_snippet("senza niente"), "senza niente");
    }

    #[test]
    fn quando_il_completamento_si_apre_da_solo() {
        // prima lettera di una parola
        assert!(should_open("a", "let a"));
        assert!(should_open("_", "let _"));
        // la seconda lettera no: il popup, se c'è, si filtra da sé
        assert!(!should_open("b", "let ab"));
        // una cifra non comincia un identificatore
        assert!(!should_open("1", "let x = 1"));
        // punto, due punti doppi, freccia
        assert!(should_open(".", "v."));
        assert!(!should_open(".", "0.."), "l'intervallo non è un accesso");
        assert!(should_open(":", "std::"));
        assert!(
            !should_open(":", "x:"),
            "due punti singoli: annotazione di tipo"
        );
        assert!(should_open(">", "fn f() ->"));
        assert!(!should_open(">", "a >"), "confronto, non freccia");
        // più caratteri insieme (incolla) o spazi: niente
        assert!(!should_open("ab", "ab"));
        assert!(!should_open(" ", "a "));
        assert!(!should_open("", ""));
    }
}
