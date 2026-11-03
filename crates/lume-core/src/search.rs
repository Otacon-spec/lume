//! Motore di ricerca testo nel progetto.
//!
//! Da non confondere: questo è il **motore** della ricerca nel testo dei
//! file. «Cerca ovunque» (`lume-app/src/search.rs`) trova file, simboli e
//! azioni per nome; il pannello «Cerca» (`lume-app/src/tools/search_tool.rs`)
//! è la vista che usa questo motore.
//!
//! Ricerca letterale o per espressione regolare, con le tre manopole del
//! prototipo ([`SearchOptions`]) e tre ambiti ([`SearchScope`]), rispettando
//! i `.gitignore`; salta i file oltre 1MB e i binari (byte NUL). Il lavoro
//! gira su un thread dedicato che produce [`SearchUpdate`] su un canale; la
//! cancellazione è cooperativa via `generation` condivisa.
//!
//! Nota implementativa: al posto del sink di comodo `grep_searcher::sinks::UTF8`
//! si usa un `Sink` custom equivalente, necessario per catturare anche il
//! byte-range del match nella riga (via `Matcher::find_iter` sul testo della
//! riga — così una riga con più occorrenze produce più `SearchHit`).

use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use grep_matcher::Matcher;
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder, Sink, SinkMatch};
use ignore::WalkBuilder;
use ropey::Rope;

mod events;
pub use events::{
    SEARCH_EVENT_CAPACITY, SearchEvents, SearchOrigin, SearchRun, spawn_search_owned,
};

enum Emitter {
    Legacy(Sender<SearchUpdate>),
    Owned(events::EventSender),
}
impl Emitter {
    fn send(&self, update: SearchUpdate) -> Result<(), SearchUpdate> {
        match self {
            Self::Legacy(sender) => sender.send(update).map_err(|error| error.0),
            Self::Owned(sender) => sender.send(update),
        }
    }
    fn is_canceled(&self) -> bool {
        match self {
            Self::Legacy(_) => false,
            Self::Owned(sender) => sender.is_canceled(),
        }
    }
}

/// Dimensione massima dei file cercati (1MB).
const MAX_FILE_SIZE: u64 = 1024 * 1024;
/// Cap totale sui match: raggiunto questo limite la ricerca si ferma.
const MAX_TOTAL_MATCHES: usize = 2000;
/// Ogni quanti file con match viene inviato un batch.
const BATCH_FILES: usize = 20;

/// Le tre manopole del prototipo, più lo smart-case che c'era già.
///
/// ATTENZIONE: `case_sensitive: None` **non** vuol dire «insensibile»: vuol dire
/// smart-case, cioè sensibile solo se il testo cercato contiene maiuscole. È
/// il comportamento di prima e resta il default, perché è quello che serve
/// nove volte su dieci. `Some(_)` è la scelta esplicita dell'utente.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchOptions {
    pub needle: String,
    pub case_sensitive: Option<bool>,
    /// Solo parole intere: `main` non trova `domain`.
    pub whole_word: bool,
    /// Il testo è un'espressione regolare invece che letterale.
    pub regex: bool,
}

impl SearchOptions {
    pub fn literal(needle: impl Into<String>) -> Self {
        Self {
            needle: needle.into(),
            ..Self::default()
        }
    }
}

/// Scelta di ambito nel pannello Cerca (viaggia fino all'app).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchArea {
    #[default]
    Progetto,
    /// File il cui percorso contiene un componente `src` (non solo `<radice>/src`).
    Sorgenti,
    Aperti,
}

/// Dove cercare.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchScope {
    Albero(PathBuf),
    /// Walk dall'albero del progetto, solo file con un componente di path `src`.
    Sorgenti(PathBuf),
    Aperti(Vec<PathBuf>),
}

fn path_has_src_component(path: &std::path::Path) -> bool {
    use std::path::Component;
    path.components()
        .any(|c| matches!(c, Component::Normal(name) if name == "src"))
}

/// Singola occorrenza del needle in una riga.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    /// Numero di riga, 1-based.
    pub line_number: u64,
    /// Testo della riga, senza il terminatore di riga.
    pub line_text: Arc<str>,
    /// Byte range del match dentro `line_text`.
    pub range_in_line: std::ops::Range<usize>,
}

/// Tutte le occorrenze trovate in un file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMatches {
    pub path: PathBuf,
    pub hits: Vec<SearchHit>,
}

/// Aggiornamento incrementale prodotto dal thread di ricerca.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchUpdate {
    Batch(Vec<FileMatches>),
    /// Fine ricerca: totale dei file con match e dei match.
    Done {
        files: usize,
        matches: usize,
        /// Il tetto di [`MAX_TOTAL_MATCHES`] ha fermato il giro: i numeri
        /// qui sopra sono un **minimo**, non un totale.
        ///
        /// ATTENZIONE: Senza questo flag «2000 risultati» sarebbe indistinguibile da
        /// «esattamente 2000 risultati», e una sostituzione su una lista
        /// troncata direbbe di aver finito un lavoro che ha appena cominciato.
        truncated: bool,
    },
    /// Ricerca non eseguibile (es. pattern non compilabile): distinto da
    /// `Done { 0, 0 }` che significa "nessun risultato".
    Error(String),
}

/// Cerca secondo `options` dentro `scope`, rispettando i `.gitignore`; salta
/// file >1MB e binari. Ritorna subito: un thread produce [`SearchUpdate`] sul
/// receiver. Si cancella (senza `Done`) quando `generation` cambia rispetto a
/// `my_generation`.
pub fn spawn_search(
    scope: SearchScope,
    options: SearchOptions,
    generation: Arc<AtomicU64>,
    my_generation: u64,
) -> Receiver<SearchUpdate> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        run_search(
            &scope,
            &options,
            &generation,
            my_generation,
            &Emitter::Legacy(tx),
        );
    });
    rx
}

/// Costruisce il matcher dalle opzioni.
///
/// ATTENZIONE: **`build_literals` non fa l'escape di niente.** Il nome lo lascia
/// credere e il commento che stava qui lo diceva, ma in grep-regex 0.1.14 è
/// una riga sola — `self.build_many(literals)` — cioè esattamente `build`
/// con un'alternanza: i metacaratteri restano metacaratteri
/// (`grep-regex-0.1.14/src/matcher.rs:91-96`). La ricerca «letterale» di
/// Lume ha quindi sempre interpretato il testo come un'espressione regolare:
/// cercare `a.b` trovava anche `axb`, e cercare `foo(` dava un errore di
/// compilazione al posto di zero risultati.
///
/// La manopola vera è `fixed_strings`, che è dichiarata proprio così: «tutti
/// i caratteri, compresi quelli che sarebbero metacaratteri, si cercano alla
/// lettera». Da qui una sola strada di costruzione e un solo interruttore.
///
/// Lo smart-case lo decide `case_smart` di grep-regex, che guarda i
/// **letterali** dell'espressione: contarci le maiuscole a mano renderebbe
/// sensibile una regex per il solo fatto di contenere `\S` o `\W`.
fn build_matcher(options: &SearchOptions) -> Result<RegexMatcher, grep_regex::Error> {
    let mut builder = RegexMatcherBuilder::new();
    match options.case_sensitive {
        None => builder.case_smart(true),
        Some(sensibile) => builder.case_insensitive(!sensibile),
    };
    builder
        .word(options.whole_word)
        .fixed_strings(!options.regex)
        .build(&options.needle)
}

/// Cap sui match di un singolo documento in memoria. Più basso del cap
/// multi-file (`MAX_TOTAL_MATCHES`, per l'intero progetto): qui è **un**
/// file, ed evidenziarne 20 000 non aiuterebbe nessuno a leggerli.
pub const MAX_FIND_MATCHES: usize = 5000;

/// I match di un documento, al più [`MAX_FIND_MATCHES`], e se ce n'erano
/// altri oltre il tetto. Come `SearchUpdate::Done { truncated, .. }` per la
/// ricerca nei file: chi sostituisce deve sapere se ha finito.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RopeMatches {
    pub ranges: Vec<std::ops::Range<usize>>,
    pub truncated: bool,
}

/// Trova le occorrenze di `options` in `rope`, in **char index** assoluti
/// (non byte): `Document::set_selection` e `ChangeSet::replace_*` lavorano
/// in char, e un range sbagliato sposterebbe il salto o la sostituzione sul
/// carattere vicino appena il testo contiene un multi-byte.
///
/// Nessun I/O, nessun thread: un documento aperto è già in memoria, e
/// questa è la differenza con `run_search` (che cammina il filesystem).
/// Query vuota → nessun match (una regex vuota altrimenti ne troverebbe uno
/// a ogni posizione, che non è quel che un riquadro appena aperto deve
/// mostrare).
pub fn find_in_rope_capped(rope: &Rope, options: &SearchOptions) -> Result<RopeMatches, String> {
    if options.needle.is_empty() {
        return Ok(RopeMatches::default());
    }
    let matcher = build_matcher(options).map_err(|e| e.to_string())?;
    let mut hits = Vec::new();
    let mut truncated = false;
    'righe: for line_idx in 0..rope.len_lines() {
        let line = rope.line(line_idx);
        // Il matcher lavora su testo contiguo: una riga è già un pezzo
        // piccolo e contiguo, stesso schema di `run_search` sul Sink.
        let text = line.to_string();
        let line_char_start = rope.line_to_char(line_idx);
        let mut byte_pos = 0usize;
        loop {
            let found = matcher
                .find_at(text.as_bytes(), byte_pos)
                .map_err(|e| e.to_string())?;
            let Some(m) = found else { break };
            let char_start = line_char_start + text[..m.start()].chars().count();
            let char_end = line_char_start + text[..m.end()].chars().count();
            hits.push(char_start..char_end);
            // Si cerca **una** occorrenza oltre il tetto: è l'unico modo di
            // distinguere «esattamente 5000» da «più di 5000».
            if hits.len() > MAX_FIND_MATCHES {
                hits.truncate(MAX_FIND_MATCHES);
                truncated = true;
                break 'righe;
            }
            if m.is_empty() {
                // match a lunghezza zero (regex tipo `^` o `x*`): avanza fino al
                // prossimo confine di carattere — non un byte fisso, altrimenti un
                // carattere multi-byte (es. "é") verrebbe spezzato a metà, causando
                // panic nello slicing più sotto sul confine non valido risultante.
                let passo = text[m.end()..].chars().next().map_or(1, char::len_utf8);
                byte_pos = m.end() + passo;
                if byte_pos > text.len() {
                    break;
                }
            } else {
                byte_pos = m.end();
            }
        }
    }
    Ok(RopeMatches {
        ranges: hits,
        truncated,
    })
}

/// Solo i range di [`find_in_rope_capped`], per chi non deve dichiarare il
/// troncamento.
pub fn find_in_rope(
    rope: &Rope,
    options: &SearchOptions,
) -> Result<Vec<std::ops::Range<usize>>, String> {
    find_in_rope_capped(rope, options).map(|trovati| trovati.ranges)
}

fn run_search(
    scope: &SearchScope,
    options: &SearchOptions,
    generation: &AtomicU64,
    my_generation: u64,
    tx: &Emitter,
) {
    // Prima anche di costruire matcher/walk: su albero vuoto o senza src
    // il corpo del ciclo può non essere mai raggiunto.
    if generation.load(Ordering::SeqCst) != my_generation || tx.is_canceled() {
        return;
    }
    if options.needle.is_empty() {
        let _ = tx.send(SearchUpdate::Done {
            files: 0,
            matches: 0,
            truncated: false,
        });
        return;
    }
    let matcher = match build_matcher(options) {
        Ok(m) => m,
        Err(e) => {
            let _ = tx.send(SearchUpdate::Error(e.to_string()));
            return;
        }
    };
    let mut searcher = SearcherBuilder::new()
        .line_number(true)
        .binary_detection(BinaryDetection::quit(0)) // byte NUL => file binario
        .build();

    // Un buffer di input per il solo job attivo, liberato a fine ricerca.
    // Il tetto include un byte sentinella per non pubblicare un prefisso
    // apparentemente valido di un file cresciuto oltre soglia dopo metadata.
    let mut input = Vec::new();
    let mut batch: Vec<FileMatches> = Vec::new();
    let mut files = 0usize;
    let mut total = 0usize;

    for path in candidati_with_cancel(scope, generation, my_generation, || tx.is_canceled()) {
        // Controllare anche l'abbandono esplicito, senza cambiare la gen
        // condivisa di una ricerca successiva. I/O iniziata non interrotta.
        if generation.load(Ordering::SeqCst) != my_generation || tx.is_canceled() {
            return;
        }
        // Aprire una volta: metadata e read dello stesso FD, non riaprire
        // per nome dopo il controllo. Non è confinamento anti-TOCTOU.
        let mut open = std::fs::OpenOptions::new();
        open.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            // Una FIFO può sostituire un candidato dopo il walk, o essere
            // presente nell'ambito Aperti. Non attendere un writer prima
            // della verifica del tipo sul FD. I file regolari restano sincroni.
            open.custom_flags(libc::O_NONBLOCK);
        }
        let Ok(mut file) = open.open(&path) else {
            continue;
        };
        if file
            .metadata()
            .map(|m| !m.is_file() || m.len() > MAX_FILE_SIZE)
            .unwrap_or(true)
        {
            continue;
        }
        let Ok(Some(content)) = read_search_content(&mut file, &mut input, || {
            generation.load(Ordering::SeqCst) != my_generation || tx.is_canceled()
        }) else {
            continue;
        };
        let mut sink = CollectSink {
            matcher: &matcher,
            hits: Vec::new(),
            remaining: MAX_TOTAL_MATCHES - total,
        };
        // Conservare decoder/BOM/binary e strategia incrementale di prima,
        // ma il reader opera solo sul contenuto raw già limitato.
        if searcher
            .search_reader(&matcher, content, &mut sink)
            .is_err()
        {
            continue; // file illeggibile: lo si salta
        }
        if sink.hits.is_empty() {
            continue;
        }
        let hits = sink.hits;
        total += hits.len();
        files += 1;
        batch.push(FileMatches { path, hits });
        if batch.len() >= BATCH_FILES
            && tx
                .send(SearchUpdate::Batch(std::mem::take(&mut batch)))
                .is_err()
        {
            return; // receiver droppato
        }
        if total >= MAX_TOTAL_MATCHES {
            break;
        }
    }
    // La cancellazione può aver terminato il walk prima del prossimo file
    // candidato: non trasformarla in un completamento a zero risultati.
    if generation.load(Ordering::SeqCst) != my_generation || tx.is_canceled() {
        return;
    }
    if !batch.is_empty() && tx.send(SearchUpdate::Batch(batch)).is_err() {
        return;
    }
    // ATTENZIONE: «Troncato» si decide **qui e in un posto solo**. Scriverlo anche
    // dove si tronca e dove si esce sembra più preciso ed è invece peggio:
    // le due assegnazioni si coprono a vicenda, e togliendone una nessun
    // test se ne accorge. Il tetto è l'unica ragione per cui il giro può
    // fermarsi prima della fine, quindi `total` al tetto **è** il segnale.
    //
    // Resta un'imprecisione, in favore di sicurezza: esattamente 2000
    // occorrenze in tutto il progetto si dichiarano «almeno 2000». Saperlo
    // costerebbe un file in più da leggere per scoprire che non c'è.
    let _ = tx.send(SearchUpdate::Done {
        files,
        matches: total,
        truncated: total >= MAX_TOTAL_MATCHES,
    });
}

fn read_search_content<'a>(
    file: &mut std::fs::File,
    buffer: &'a mut Vec<u8>,
    canceled: impl Fn() -> bool,
) -> std::io::Result<Option<&'a [u8]>> {
    if canceled() {
        return Ok(None);
    }
    let limit = usize::try_from(MAX_FILE_SIZE)
        .map_err(|_| std::io::Error::other("Soglia Cerca non rappresentabile"))?;
    let capacity = limit + 1;
    if buffer.len() != capacity {
        buffer
            .try_reserve_exact(capacity.saturating_sub(buffer.len()))
            .map_err(|_| std::io::Error::other("Memoria insufficiente per l'input Cerca"))?;
        buffer.resize(capacity, 0);
    }
    let mut filled = 0;
    loop {
        if canceled() {
            return Ok(None);
        }
        let destination = buffer
            .get_mut(filled..)
            .ok_or_else(|| std::io::Error::other("Intervallo input Cerca non valido"))?;
        let read = match file.read(destination) {
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        if read == 0 {
            return Ok(buffer.get(..filled));
        }
        filled += read;
        if filled > limit {
            return Ok(None);
        }
    }
}

/// I file da guardare, secondo l'ambito.
///
/// ATTENZIONE: L'ambito «file aperti» **non cammina l'albero**: se lo facesse e poi
/// scartasse quel che non è nella lista costerebbe come cercare ovunque, e
/// l'ambito serve proprio a non farlo.
fn walk_files(
    root: &std::path::Path,
    mut cancelled: impl FnMut() -> bool,
) -> impl Iterator<Item = PathBuf> {
    let mut walk = WalkBuilder::new(root)
        .hidden(false) // coerente col pannello Progetto: dotfile inclusi
        .require_git(false) // .gitignore rispettato anche fuori da un repo
        .filter_entry(|e| e.file_name() != ".git" && e.file_name() != ".jj")
        .build();
    std::iter::from_fn(move || {
        loop {
            // Prima di next e prima dei filtri file/src: anche directory ed
            // errori osservano cancellazione. I/O già iniziata resta cooperativa.
            if cancelled() {
                return None;
            }
            let entry = match walk.next()? {
                Ok(entry) => entry,
                Err(_) => continue,
            };
            if entry.file_type().is_some_and(|kind| kind.is_file()) {
                return Some(entry.into_path());
            }
        }
    })
}

#[cfg(test)]
fn candidati<'a>(
    scope: &'a SearchScope,
    generation: &'a AtomicU64,
    expected: u64,
) -> Box<dyn Iterator<Item = PathBuf> + 'a> {
    candidati_with_cancel(scope, generation, expected, || false)
}

fn candidati_with_cancel<'a>(
    scope: &'a SearchScope,
    generation: &'a AtomicU64,
    expected: u64,
    abandoned: impl Fn() -> bool + 'a,
) -> Box<dyn Iterator<Item = PathBuf> + 'a> {
    if generation.load(Ordering::SeqCst) != expected || abandoned() {
        return Box::new(std::iter::empty());
    }
    let cancelled = move || generation.load(Ordering::SeqCst) != expected || abandoned();
    match scope {
        SearchScope::Aperti(percorsi) => Box::new(
            percorsi
                .clone()
                .into_iter()
                .take_while(move |_| !cancelled()),
        ),
        SearchScope::Albero(root) => Box::new(walk_files(root, cancelled)),
        SearchScope::Sorgenti(root) => {
            Box::new(walk_files(root, cancelled).filter(|path| path_has_src_component(path)))
        }
    }
}

/// Sink che, per ogni riga con match, riesegue `find_iter` sul testo della
/// riga per catturare TUTTE le occorrenze con il loro byte-range.
struct CollectSink<'a> {
    matcher: &'a RegexMatcher,
    hits: Vec<SearchHit>,
    remaining: usize,
}

impl<'a> CollectSink<'a> {
    #[cfg(test)]
    fn with_budget(matcher: &'a RegexMatcher, remaining: usize) -> Self {
        Self {
            matcher,
            hits: Vec::new(),
            remaining,
        }
    }
}

impl Sink for CollectSink<'_> {
    type Error = std::io::Error;

    fn matched(&mut self, _searcher: &Searcher, mat: &SinkMatch<'_>) -> Result<bool, Self::Error> {
        // strippa il terminatore di riga (\n, \r\n)
        let mut line = mat.bytes();
        while let [rest @ .., b'\n' | b'\r'] = line {
            line = rest;
        }
        let line_text: Arc<str> = String::from_utf8_lossy(line).into_owned().into();
        let line_number = mat.line_number().unwrap_or(0);
        // NoError: find_iter su RegexMatcher non può fallire
        // Il range si riferisce a line_text, come richiesto dal consumer.
        // Una conversione lossy può allungare i byte prima del match: cercare
        // su quella rappresentazione, non riutilizzare offset sul file raw.
        let _ = self.matcher.find_iter(line_text.as_bytes(), |m| {
            if self.remaining == 0 {
                return false;
            }
            self.hits.push(SearchHit {
                line_number,
                line_text: line_text.clone(),
                range_in_line: m.start()..m.end(),
            });
            self.remaining -= 1;
            self.remaining > 0
        });
        Ok(self.remaining > 0)
    }
}

#[cfg(test)]
mod input_tests;
#[cfg(test)]
mod walk_cancel_tests;
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Tempdir manuale con nome unico (pid + nanos); da rimuovere a fine test.
    fn make_tempdir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("lume_core_{tag}_{}_{nanos}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Raccoglie tutti gli update fino alla chiusura del canale.
    fn collect(rx: Receiver<SearchUpdate>) -> (Vec<FileMatches>, Option<(usize, usize)>) {
        let mut all = Vec::new();
        let mut done = None;
        for update in rx {
            match update {
                SearchUpdate::Batch(b) => all.extend(b),
                SearchUpdate::Done { files, matches, .. } => done = Some((files, matches)),
                SearchUpdate::Error(e) => panic!("errore inatteso dalla ricerca: {e}"),
            }
        }
        (all, done)
    }

    /// Quanti match trova questa combinazione di opzioni sotto `root`.
    fn quanti(root: &std::path::Path, options: SearchOptions) -> usize {
        let generation = Arc::new(AtomicU64::new(0));
        let rx = spawn_search(
            SearchScope::Albero(root.to_path_buf()),
            options,
            generation,
            0,
        );
        collect(rx).1.expect("Done ricevuto").1
    }

    #[test]
    fn literal_gitignore_e_range_dei_match() {
        let root = make_tempdir("search");
        // a.txt: 1 match in riga 1 + 2 match sulla stessa riga 3
        fs::write(
            root.join("a.txt"),
            "hello mondo\nniente qui\nun hello e un altro hello\n",
        )
        .unwrap();
        fs::write(root.join("b.txt"), "solo hello una volta\n").unwrap();
        fs::write(root.join("ignorato.txt"), "hello hello\n").unwrap();
        fs::write(root.join(".gitignore"), "ignorato.txt\n").unwrap();

        let generation = Arc::new(AtomicU64::new(1));
        let rx = spawn_search(
            SearchScope::Albero(root.clone()),
            SearchOptions::literal("hello"),
            generation,
            1,
        );
        let (all, done) = collect(rx);

        let (files, matches) = done.expect("Done ricevuto");
        assert_eq!(files, 2);
        assert_eq!(matches, 4);
        // il file ignorato da .gitignore non compare
        assert!(!all.iter().any(|f| f.path.ends_with("ignorato.txt")));

        let a = all.iter().find(|f| f.path.ends_with("a.txt")).unwrap();
        assert_eq!(a.hits.len(), 3);
        // riga 1: "hello mondo", match in testa
        assert_eq!(a.hits[0].line_number, 1);
        assert_eq!(&*a.hits[0].line_text, "hello mondo");
        assert_eq!(a.hits[0].range_in_line, 0..5);
        // riga 3: due match sulla STESSA riga, con range distinti
        let riga3: Vec<_> = a.hits.iter().filter(|h| h.line_number == 3).collect();
        assert_eq!(riga3.len(), 2);
        assert_eq!(&*riga3[0].line_text, "un hello e un altro hello");
        assert_eq!(riga3[0].range_in_line, 3..8);
        assert_eq!(riga3[1].range_in_line, 20..25);

        let b = all.iter().find(|f| f.path.ends_with("b.txt")).unwrap();
        assert_eq!(b.hits.len(), 1);
        assert_eq!(b.hits[0].line_number, 1);
        assert_eq!(b.hits[0].range_in_line, 5..10);

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn la_ricerca_non_entra_in_jj() {
        let root = make_tempdir("search_jj");
        fs::create_dir_all(root.join(".jj").join("repo")).unwrap();
        fs::write(root.join(".jj").join("repo").join("store.txt"), "hello\n").unwrap();
        fs::write(root.join("a.txt"), "hello\n").unwrap();
        assert_eq!(quanti(&root, SearchOptions::literal("hello")), 1);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn smart_case() {
        let root = make_tempdir("search_case");
        fs::write(root.join("c.txt"), "Ciao\nciao\n").unwrap();
        let generation = Arc::new(AtomicU64::new(0));

        // needle minuscolo: ricerca insensibile => 2 match
        let rx = spawn_search(
            SearchScope::Albero(root.clone()),
            SearchOptions::literal("ciao"),
            generation.clone(),
            0,
        );
        let (_, done) = collect(rx);
        assert_eq!(done.unwrap().1, 2);

        // needle con maiuscola: ricerca sensibile => 1 match
        let rx = spawn_search(
            SearchScope::Albero(root.clone()),
            SearchOptions::literal("Ciao"),
            generation,
            0,
        );
        let (_, done) = collect(rx);
        assert_eq!(done.unwrap().1, 1);

        // ...e la scelta esplicita scavalca lo smart-case nei due versi
        assert_eq!(
            quanti(
                &root,
                SearchOptions {
                    case_sensitive: Some(true),
                    ..SearchOptions::literal("ciao")
                }
            ),
            1,
            "sensibile per scelta: solo la riga minuscola"
        );
        assert_eq!(
            quanti(
                &root,
                SearchOptions {
                    case_sensitive: Some(false),
                    ..SearchOptions::literal("Ciao")
                }
            ),
            2,
            "insensibile per scelta: tutte e due, malgrado la maiuscola"
        );

        fs::remove_dir_all(&root).unwrap();
    }

    /// ATTENZIONE: La prova che le due strade sono davvero diverse: `a.b` come
    /// **letterale** trova solo se stesso, come **regex** trova qualunque
    /// carattere in mezzo. Sbagliare la scelta non dà nessun errore — dà
    /// risultati sbagliati in silenzio, che è il modo peggiore.
    ///
    /// È anche il test che ha trovato il difetto vecchio: prima di questa
    /// fase la ricerca «letterale» passava da `build_literals`, che malgrado
    /// il nome non fa l'escape, e `a.b` trovava tutte e tre le righe.
    #[test]
    fn letterale_e_regex_sullo_stesso_testo_danno_risultati_diversi() {
        let root = make_tempdir("search_regex");
        fs::write(root.join("e.txt"), "a.b\naxb\nazb\n").unwrap();

        assert_eq!(
            quanti(&root, SearchOptions::literal("a.b")),
            1,
            "letterale: il punto è un punto"
        );
        assert_eq!(
            quanti(
                &root,
                SearchOptions {
                    regex: true,
                    ..SearchOptions::literal("a.b")
                }
            ),
            3,
            "regex: il punto è un jolly"
        );
        // e una regex che non compila non è «nessun risultato»: è un errore
        let generation = Arc::new(AtomicU64::new(0));
        let rx = spawn_search(
            SearchScope::Albero(root.clone()),
            SearchOptions {
                regex: true,
                ..SearchOptions::literal("a(")
            },
            generation,
            0,
        );
        assert!(
            rx.into_iter().any(|u| matches!(u, SearchUpdate::Error(_))),
            "una parentesi aperta è un errore, non zero risultati"
        );

        fs::remove_dir_all(&root).unwrap();
    }

    /// Parola intera: `main` non trova `domain`.
    #[test]
    fn parola_intera_non_prende_i_pezzi_di_parola() {
        let root = make_tempdir("search_word");
        fs::write(root.join("f.txt"), "fn main()\nmod domain;\n").unwrap();

        assert_eq!(quanti(&root, SearchOptions::literal("main")), 2);
        assert_eq!(
            quanti(
                &root,
                SearchOptions {
                    whole_word: true,
                    ..SearchOptions::literal("main")
                }
            ),
            1,
            "«domain» contiene «main» ma non è «main»"
        );

        fs::remove_dir_all(&root).unwrap();
    }

    /// L'ambito «file aperti» restringe davvero: un file che c'è sull'albero
    /// e non nella lista non si guarda nemmeno. Se camminasse l'albero per
    /// poi scartare, l'ambito non farebbe risparmiare niente.
    #[test]
    fn lambito_dei_file_aperti_non_guarda_gli_altri() {
        let root = make_tempdir("search_scope");
        let aperto = root.join("aperto.txt");
        let chiuso = root.join("chiuso.txt");
        fs::write(&aperto, "ago\n").unwrap();
        fs::write(&chiuso, "ago\n").unwrap();

        let generation = Arc::new(AtomicU64::new(0));
        let rx = spawn_search(
            SearchScope::Albero(root.clone()),
            SearchOptions::literal("ago"),
            generation.clone(),
            0,
        );
        assert_eq!(collect(rx).1.unwrap(), (2, 2), "sull'albero ci sono tutti");

        let rx = spawn_search(
            SearchScope::Aperti(vec![aperto.clone()]),
            SearchOptions::literal("ago"),
            generation,
            0,
        );
        let (all, done) = collect(rx);
        assert_eq!(done.unwrap(), (1, 1));
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].path, aperto);

        fs::remove_dir_all(&root).unwrap();
    }

    /// ATTENZIONE: Il tetto di [`MAX_TOTAL_MATCHES`] ferma il giro, e senza dirlo
    /// «2000 risultati» sarebbe indistinguibile da «esattamente 2000». Non è
    /// una sfumatura: è il numero su cui la sostituzione chiede conferma, e
    /// una lista troncata dichiarerebbe finito un lavoro appena cominciato.
    #[test]
    fn oltre_il_tetto_il_totale_si_dichiara_incompleto() {
        let root = make_tempdir("search_cap");
        let righe = "ago\n".repeat(MAX_TOTAL_MATCHES + 1);
        fs::write(root.join("tanti.txt"), righe).unwrap();

        let generation = Arc::new(AtomicU64::new(0));
        let rx = spawn_search(
            SearchScope::Albero(root.clone()),
            SearchOptions::literal("ago"),
            generation,
            0,
        );
        let mut done = None;
        for update in rx {
            if let SearchUpdate::Done {
                matches, truncated, ..
            } = update
            {
                done = Some((matches, truncated));
            }
        }
        assert_eq!(
            done,
            Some((MAX_TOTAL_MATCHES, true)),
            "fermata al tetto, e lo dice"
        );

        // sotto il tetto invece il totale è un totale
        fs::write(root.join("tanti.txt"), "ago\n").unwrap();
        assert_eq!(quanti(&root, SearchOptions::literal("ago")), 1);
        let generation = Arc::new(AtomicU64::new(0));
        let rx = spawn_search(
            SearchScope::Albero(root.clone()),
            SearchOptions::literal("ago"),
            generation,
            0,
        );
        assert!(
            rx.into_iter().any(|u| matches!(
                u,
                SearchUpdate::Done {
                    truncated: false,
                    ..
                }
            )),
            "un risultato solo non è troncato"
        );

        fs::remove_dir_all(&root).unwrap();
    }

    /// Il cap deve fermare il sink nella riga corrente: raccogliere prima
    /// tutti i match per poi troncarli renderebbe il tetto inutile contro una
    /// riga ripetitiva molto lunga.
    #[test]
    fn il_sink_si_ferma_appena_esaurisce_il_budget_della_riga() {
        let matcher = build_matcher(&SearchOptions::literal("ago")).unwrap();
        let mut sink = CollectSink::with_budget(&matcher, 2);
        let mut searcher = SearcherBuilder::new().line_number(true).build();
        let _ = searcher.search_slice(&matcher, b"ago ago ago\n", &mut sink);

        assert_eq!(sink.hits.len(), 2);
    }

    #[test]
    fn le_occorrenze_della_stessa_riga_condividono_il_testo() {
        let root = make_tempdir("search_shared_line");
        fs::write(root.join("ripetuta.txt"), "ago e ancora ago\n").unwrap();

        let generation = Arc::new(AtomicU64::new(0));
        let rx = spawn_search(
            SearchScope::Albero(root.clone()),
            SearchOptions::literal("ago"),
            generation,
            0,
        );
        let (all, _) = collect(rx);
        let hits = &all[0].hits;
        assert_eq!(hits.len(), 2);
        assert!(Arc::ptr_eq(&hits[0].line_text, &hits[1].line_text));

        fs::remove_dir_all(&root).unwrap();
    }

    /// L'ambito «src» guarda i file con un componente di path `src`, non solo
    /// `<radice>/src`: in un workspace Cargo le sorgenti vivono sotto `crates/…/src`.
    #[test]
    fn lambito_sorgenti_filtra_per_componente_src() {
        let root = make_tempdir("search_src");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("tests")).unwrap();
        fs::write(root.join("src/lib.rs"), "ago\n").unwrap();
        fs::write(root.join("tests/foo.rs"), "ago\n").unwrap();

        let generation = Arc::new(AtomicU64::new(0));
        let rx = spawn_search(
            SearchScope::Sorgenti(root.clone()),
            SearchOptions::literal("ago"),
            generation,
            0,
        );
        let (all, done) = collect(rx);
        assert_eq!(done.unwrap(), (1, 1));
        assert_eq!(all.len(), 1);
        assert!(all[0].path.ends_with("src/lib.rs"));

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn cancellazione_via_generation() {
        let root = make_tempdir("search_cancel");
        fs::write(root.join("d.txt"), "needle\n").unwrap();
        // generation già diversa da my_generation: il thread esce SENZA Done
        let generation = Arc::new(AtomicU64::new(2));
        let rx = spawn_search(
            SearchScope::Albero(root.clone()),
            SearchOptions::literal("needle"),
            generation,
            1,
        );
        let (all, done) = collect(rx);
        assert!(all.is_empty());
        assert!(done.is_none());

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn find_in_rope_trova_le_occorrenze_in_char_index() {
        let rope = Rope::from_str("uno due uno tre uno");
        let hits = find_in_rope(&rope, &SearchOptions::literal("uno")).unwrap();
        assert_eq!(hits, vec![0..3, 8..11, 16..19]);
    }

    #[test]
    fn find_in_rope_query_vuota_non_matcha_tutto() {
        let rope = Rope::from_str("qualunque cosa");
        let hits = find_in_rope(&rope, &SearchOptions::literal("")).unwrap();
        assert!(hits.is_empty());
    }

    #[test]
    fn find_in_rope_conta_i_caratteri_non_i_byte() {
        // «è» è 2 byte in UTF-8 ma 1 char: il match dopo deve stare al char
        // giusto, non spostato di un byte.
        let rope = Rope::from_str("perché uno");
        let hits = find_in_rope(&rope, &SearchOptions::literal("uno")).unwrap();
        let riga = "perché uno";
        let atteso_char = riga.chars().take_while(|&c| c != 'u').count();
        assert_eq!(hits, vec![atteso_char..atteso_char + 3]);
    }

    #[test]
    fn find_in_rope_regex_non_valida_e_un_errore_non_un_panic() {
        let rope = Rope::from_str("testo");
        let mut opzioni = SearchOptions::literal("(");
        opzioni.regex = true;
        assert!(find_in_rope(&rope, &opzioni).is_err());
    }

    #[test]
    fn find_in_rope_rispetta_maiuscole_parola_intera_e_regex() {
        let rope = Rope::from_str("Main main mainframe");
        let mut opzioni = SearchOptions::literal("main");
        opzioni.case_sensitive = Some(true);
        opzioni.whole_word = true;
        // solo "main" alle 5..9, non "Main" (case) né dentro "mainframe" (parola)
        assert_eq!(find_in_rope(&rope, &opzioni).unwrap(), vec![5..9]);

        let mut regex = SearchOptions::literal(r"m\w+");
        regex.regex = true;
        assert_eq!(
            find_in_rope(&rope, &regex).unwrap().len(),
            3,
            "Main, main, mainframe"
        );
    }

    #[test]
    fn find_in_rope_regex_lunghezza_zero_registra_il_match() {
        // La regex `^` matcha all'inizio di ogni riga (zero-width).
        // Senza fix, il primo match (a inizio riga, alla posizione corrente)
        // veniva scartato: il loop avanzava byte_pos e continuava senza
        // registrare il match. Con fix, deve essere registrato.
        // Testo SENZA trailing newline per evitare una riga vuota extra.
        let rope = Rope::from_str("uno\ndue\ntre");
        let mut opzioni = SearchOptions::literal("^");
        opzioni.regex = true;
        let hits = find_in_rope(&rope, &opzioni).unwrap();
        // Tre righe: 0, 4, 8 (conteggio in char, senza newline finale).
        // Ogni match è zero-width: 0..0, 4..4, 8..8.
        assert_eq!(hits, vec![0..0, 4..4, 8..8]);
    }

    #[test]
    fn find_in_rope_cap_max_find_matches_blocca_zero_width() {
        // La regex `b*` matcha «zero o più b». Su una stringa lunga di soli
        // caratteri che non sono b, matcha circa una volta per carattere
        // (incluso all'inizio, fine, e tra ogni coppia). Con fix, ogni match
        // viene registrato: ~5002 match su 5001 caratteri. Il cap deve
        // fermare a 5000.
        // Senza fix, il primo match (zero-width, alla posizione iniziale) non
        // veniva registrato, quindi ritornava Vec::new() o un numero sbagliato.
        let mut opzioni = SearchOptions::literal("b*");
        opzioni.regex = true;
        let text = "a".repeat(MAX_FIND_MATCHES + 1); // 5001 caratteri
        let rope = Rope::from_str(&text);
        let hits = find_in_rope(&rope, &opzioni).unwrap();
        // Deve capare a MAX_FIND_MATCHES, non più.
        assert_eq!(
            hits.len(),
            MAX_FIND_MATCHES,
            "il cap blocca esattamente a {}, non oltre",
            MAX_FIND_MATCHES
        );
    }

    #[test]
    fn find_in_rope_capped_distingue_il_tetto_esatto_dal_troncamento() {
        let esatti = Rope::from_str(&"a ".repeat(MAX_FIND_MATCHES));
        let tutti = find_in_rope_capped(&esatti, &SearchOptions::literal("a")).unwrap();
        assert_eq!(tutti.ranges.len(), MAX_FIND_MATCHES);
        assert!(
            !tutti.truncated,
            "esattamente il tetto non è un troncamento"
        );

        let oltre = Rope::from_str(&"a ".repeat(MAX_FIND_MATCHES + 1));
        let troncati = find_in_rope_capped(&oltre, &SearchOptions::literal("a")).unwrap();
        assert_eq!(troncati.ranges.len(), MAX_FIND_MATCHES);
        assert!(troncati.truncated, "la 5001ª esiste e va dichiarata");
    }

    #[test]
    fn find_in_rope_restituisce_gli_stessi_range_di_find_in_rope_capped() {
        let rope = Rope::from_str("uno due uno");
        let opzioni = SearchOptions::literal("uno");
        assert_eq!(
            find_in_rope(&rope, &opzioni).unwrap(),
            find_in_rope_capped(&rope, &opzioni).unwrap().ranges
        );
    }

    #[test]
    fn find_in_rope_zero_width_su_multi_byte_non_va_in_panic() {
        // "perché" contiene 'é' (2 byte in UTF-8): un pattern a lunghezza zero
        // come `x*` (matcha "zero o più x", quindi matcha vuoto ovunque non ci
        // sia una x) prima del fix avanzava di un byte fisso dopo ogni match
        // zero-width, spezzando 'é' a metà e causando panic nello slicing
        // successivo. Deve solo non andare in panic e restituire Ok.
        let rope = ropey::Rope::from_str("perché è così");
        let mut opzioni = SearchOptions::literal("x*");
        opzioni.regex = true;
        let risultato = find_in_rope(&rope, &opzioni);
        assert!(
            risultato.is_ok(),
            "non deve andare in panic su testo multi-byte"
        );
    }
}
