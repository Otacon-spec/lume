//! La scheda hover: dal testo del server (markdown, pezzi marcati o testo
//! semplice) alle righe che il widget disegna. Pura: nessun I/O.
//!
//! Il testo viene dal server e non è nostro: qui si **riduce**, non si
//! interpreta. Immagini, tabelle e HTML restano testo.

use lsp_types::{Hover, HoverContents, MarkedString, MarkupKind};
use lume_core::DocId;
use lume_ui::editor::{CARD_MAX_COLS, HoverCard, HoverLine, clip_text};

/// Righe al massimo nella scheda; se ce ne sono di più l'ultima è «…».
pub const MAX_LINES: usize = 20;

/// Righe grezze lette al massimo: un server può mandare megabyte, e
/// leggerli tutti per mostrarne venti sarebbe lavoro buttato.
const MAX_RAW_LINES: usize = 200;

/// La domanda di hover in volo. Una sola: ogni chiusura la azzera.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoverWait {
    /// Gli id di richiesta sono contatori per workspace: da soli, fra due
    /// server, collidono.
    pub server: String,
    pub request: u64,
    pub doc: DocId,
    /// L'indice di char della parola puntata, nel testo di `version`.
    pub anchor: usize,
    /// La revisione del testo quando si è chiesto.
    pub version: u64,
}

/// La scheda in mostra, e il documento a cui appartiene.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoverShown {
    pub doc: DocId,
    pub card: HoverCard,
}

enum Parte<'a> {
    Markdown(&'a str),
    Codice(&'a str),
    Semplice(&'a str),
}

fn parti(contenuto: &HoverContents) -> Vec<Parte<'_>> {
    fn marcata(pezzo: &MarkedString) -> Parte<'_> {
        match pezzo {
            MarkedString::String(testo) => Parte::Markdown(testo),
            MarkedString::LanguageString(codice) => Parte::Codice(&codice.value),
        }
    }
    match contenuto {
        HoverContents::Scalar(pezzo) => vec![marcata(pezzo)],
        HoverContents::Array(pezzi) => pezzi.iter().map(marcata).collect(),
        HoverContents::Markup(markup) => vec![match markup.kind {
            MarkupKind::Markdown => Parte::Markdown(&markup.value),
            MarkupKind::PlainText => Parte::Semplice(&markup.value),
        }],
    }
}

fn prosa(testo: String) -> HoverLine {
    HoverLine {
        text: testo,
        code: false,
    }
}

fn vuota() -> HoverLine {
    prosa(String::new())
}

fn codice(riga: &str) -> HoverLine {
    HoverLine {
        text: riga.replace('\t', "    ").trim_end().to_owned(),
        code: true,
    }
}

fn e_vuota(riga: &HoverLine) -> bool {
    !riga.code && riga.text.is_empty()
}

/// `---`, `***`, `___`: una regola orizzontale.
fn e_regola(riga: &str) -> bool {
    riga.len() >= 3
        && ['-', '*', '_']
            .iter()
            .any(|c| riga.chars().all(|x| x == *c))
}

/// Toglie `**` e i backtick, e riduce `[testo](url)` a `testo`. I doppi
/// trattini bassi restano: `__init__` è un nome, non un grassetto.
fn strip_inline(riga: &str) -> String {
    let mut fuori = String::with_capacity(riga.len());
    let mut resto = riga;
    loop {
        if let Some(dopo) = resto.strip_prefix("**").or_else(|| resto.strip_prefix('`')) {
            resto = dopo;
            continue;
        }
        if let Some(dopo) = resto.strip_prefix('[')
            && let Some((testo, coda)) = dopo.split_once("](")
            && !testo.contains(']')
            && let Some((_, dopo_link)) = coda.split_once(')')
        {
            fuori.push_str(&strip_inline(testo));
            resto = dopo_link;
            continue;
        }
        let mut caratteri = resto.chars();
        let Some(c) = caratteri.next() else {
            break;
        };
        fuori.push(c);
        resto = caratteri.as_str();
    }
    fuori
}

/// Una riga di prosa markdown: titoli senza cancelletti, regole vuote,
/// formattazione in linea tolta.
fn prosa_markdown(riga: &str) -> HoverLine {
    let netta = riga.trim();
    if netta.is_empty() || e_regola(netta) {
        return vuota();
    }
    let cancelletti = riga.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&cancelletti) && riga.chars().nth(cancelletti) == Some(' ') {
        let titolo: String = riga.chars().skip(cancelletti + 1).collect();
        return prosa(strip_inline(titolo.trim()));
    }
    prosa(strip_inline(riga.trim_end()).replace('\t', "    "))
}

fn markdown_grezzo(md: &str) -> Vec<HoverLine> {
    let mut righe = Vec::new();
    let mut nel_codice = false;
    for riga in md.lines().take(MAX_RAW_LINES) {
        if riga.trim_start().starts_with("```") {
            nel_codice = !nel_codice;
            continue;
        }
        righe.push(if nel_codice {
            codice(riga)
        } else {
            prosa_markdown(riga)
        });
    }
    righe
}

/// Le righe grezze diventano quelle da mostrare: niente vuote in testa, in
/// coda o due di fila; al massimo `MAX_LINES`; ogni riga entro
/// `CARD_MAX_COLS` colonne.
fn rifinisci(grezze: Vec<HoverLine>) -> Vec<HoverLine> {
    let mut fuori: Vec<HoverLine> = Vec::new();
    for riga in grezze {
        if e_vuota(&riga) && fuori.last().is_none_or(e_vuota) {
            continue;
        }
        fuori.push(riga);
    }
    while fuori.last().is_some_and(e_vuota) {
        fuori.pop();
    }
    if fuori.len() > MAX_LINES {
        fuori.truncate(MAX_LINES - 1);
        fuori.push(prosa("…".to_owned()));
    }
    for riga in &mut fuori {
        riga.text = clip_text(&riga.text, CARD_MAX_COLS);
    }
    fuori
}

/// Da un testo markdown alle righe della scheda.
pub fn markdown_to_lines(md: &str) -> Vec<HoverLine> {
    rifinisci(markdown_grezzo(md))
}

/// Da una risposta `textDocument/hover` alle righe della scheda; vuoto se
/// il server non ha niente da dire.
pub fn hover_lines(hover: &Hover) -> Vec<HoverLine> {
    let mut grezze = Vec::new();
    for parte in parti(&hover.contents) {
        grezze.push(vuota());
        match parte {
            Parte::Markdown(testo) => grezze.extend(markdown_to_lines(testo)),
            Parte::Codice(testo) => {
                grezze.extend(testo.lines().take(MAX_RAW_LINES).map(codice));
            }
            Parte::Semplice(testo) => grezze.extend(
                testo
                    .lines()
                    .take(MAX_RAW_LINES)
                    .map(|riga| prosa(riga.trim_end().replace('\t', "    "))),
            ),
        }
    }
    rifinisci(grezze)
}
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use lsp_types::{
        Hover, HoverContents, LanguageString, MarkedString, MarkupContent, MarkupKind,
    };
    use lume_ui::editor::{CARD_MAX_COLS, HoverLine};

    use super::{MAX_LINES, hover_lines, markdown_to_lines};

    fn prosa(testo: &str) -> HoverLine {
        HoverLine {
            text: testo.to_owned(),
            code: false,
        }
    }

    fn codice(testo: &str) -> HoverLine {
        HoverLine {
            text: testo.to_owned(),
            code: true,
        }
    }

    fn hover(contents: HoverContents) -> Hover {
        Hover {
            contents,
            range: None,
        }
    }

    #[test]
    fn i_blocchi_di_codice_diventano_righe_di_codice_senza_i_recinti() {
        let md = "```rust\nstd::vec\n```\n\n```rust\npub fn push(&mut self, value: T)\n```\n\n---\n\nAppends an element.";
        assert_eq!(
            markdown_to_lines(md),
            [
                codice("std::vec"),
                prosa(""),
                codice("pub fn push(&mut self, value: T)"),
                prosa(""),
                prosa("Appends an element."),
            ],
            "la regola --- è una riga vuota e le vuote di fila si riducono a una"
        );
    }

    #[test]
    fn titoli_link_grassetti_e_backtick_si_riducono_a_testo() {
        let md =
            "## Esempi\n\nVedi [Vec](https://doc.rust-lang.org) e **molto** `usa`.\narr[0] resta";
        assert_eq!(
            markdown_to_lines(md),
            [
                prosa("Esempi"),
                prosa(""),
                prosa("Vedi Vec e molto usa."),
                prosa("arr[0] resta"),
            ]
        );
    }

    #[test]
    fn un_cancelletto_senza_spazio_non_e_un_titolo() {
        assert_eq!(markdown_to_lines("#include <x>"), [prosa("#include <x>")]);
    }

    #[test]
    fn i_doppi_trattini_bassi_di_un_identificatore_restano() {
        assert_eq!(
            markdown_to_lines("__init__ e a_b"),
            [prosa("__init__ e a_b")]
        );
    }

    #[test]
    fn le_tabulazioni_del_codice_diventano_spazi() {
        assert_eq!(
            markdown_to_lines("```\n\tfn a()\n```"),
            [codice("    fn a()")]
        );
    }

    #[test]
    fn un_recinto_mai_chiuso_tiene_il_resto_come_codice() {
        assert_eq!(
            markdown_to_lines("testo\n```rust\nfn a()"),
            [prosa("testo"), codice("fn a()")]
        );
    }

    #[test]
    fn il_fine_riga_windows_non_lascia_ritorni_a_capo() {
        assert_eq!(markdown_to_lines("a\r\nb"), [prosa("a"), prosa("b")]);
    }

    #[test]
    fn il_niente_e_niente() {
        assert!(markdown_to_lines("").is_empty());
        assert!(markdown_to_lines("\n\n  \n").is_empty());
        assert!(markdown_to_lines("---").is_empty());
    }

    #[test]
    fn oltre_venti_righe_l_ultima_diventa_un_puntino() {
        let md: String = (0..30).map(|i| format!("riga {i}\n")).collect();
        let righe = markdown_to_lines(&md);
        assert_eq!(righe.len(), MAX_LINES);
        assert_eq!(righe[0], prosa("riga 0"));
        assert_eq!(righe[MAX_LINES - 2], prosa("riga 18"));
        assert_eq!(righe[MAX_LINES - 1], prosa("…"));
    }

    #[test]
    fn un_testo_enorme_non_si_legge_tutto() {
        let md: String = (0..10_000).map(|i| format!("riga {i}\n")).collect();
        let righe = markdown_to_lines(&md);
        assert_eq!(righe.len(), MAX_LINES);
        assert_eq!(righe[MAX_LINES - 1], prosa("…"));
    }

    #[test]
    fn una_riga_lunga_si_accorcia_in_caratteri_non_in_byte() {
        let righe = markdown_to_lines(&"😀".repeat(150));
        assert_eq!(righe.len(), 1);
        assert_eq!(righe[0].text.chars().count(), CARD_MAX_COLS);
        assert!(righe[0].text.ends_with('…'));
        assert!(righe[0].text.starts_with('😀'), "un'emoji non si spezza");
    }

    #[test]
    fn i_tre_formati_del_protocollo() {
        // un solo pezzo di markdown
        assert_eq!(
            hover_lines(&hover(HoverContents::Scalar(MarkedString::String(
                "**x**".to_owned()
            )))),
            [prosa("x")]
        );
        // un pezzo di codice con il linguaggio
        assert_eq!(
            hover_lines(&hover(HoverContents::Scalar(MarkedString::LanguageString(
                LanguageString {
                    language: "rust".to_owned(),
                    value: "fn a()".to_owned(),
                }
            )))),
            [codice("fn a()")]
        );
        // più pezzi, separati da una riga vuota
        assert_eq!(
            hover_lines(&hover(HoverContents::Array(vec![
                MarkedString::String("intro".to_owned()),
                MarkedString::LanguageString(LanguageString {
                    language: "rust".to_owned(),
                    value: "let x".to_owned(),
                }),
            ]))),
            [prosa("intro"), prosa(""), codice("let x")]
        );
    }

    #[test]
    fn il_testo_semplice_non_si_legge_come_markdown() {
        let righe = hover_lines(&hover(HoverContents::Markup(MarkupContent {
            kind: MarkupKind::PlainText,
            value: "**non markdown**\nseconda".to_owned(),
        })));
        assert_eq!(righe, [prosa("**non markdown**"), prosa("seconda")]);
    }

    #[test]
    fn un_hover_vuoto_non_ha_righe() {
        assert!(
            hover_lines(&hover(HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: "  \n".to_owned(),
            })))
            .is_empty()
        );
    }
}
