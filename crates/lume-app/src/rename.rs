//! La rinomina di un simbolo: **validare tutto prima di toccare qualunque
//! cosa**.
//!
//! ATTENZIONE: Questa è la fetta che può mangiare il codice di qualcuno, ed è per
//! questo che è fatta così. Due decisioni la rendono sicura per costruzione:
//!
//! 1. **Non si scrive mai su disco.** Le modifiche vanno ai documenti in
//!    memoria, e i file chiusi si aprono come schede **sporche**. Il
//!    salvataggio dell'app è atomico per file, non per progetto: una scrittura
//!    multi-file potrebbe lasciare una rinomina a metà. Così «fallire a metà»
//!    non è una cosa da gestire: non esiste.
//! 2. **Si valida tutto, poi si applica.** Ogni intervallo di ogni file viene
//!    convertito e controllato *prima* che una sola `ChangeSet` venga
//!    applicata. Se una qualunque cosa non torna, non è successo niente.
//!
//! Il costo è visibile e onesto: N schede sporche e una notifica che dice
//! quanti file restano da salvare.

use std::collections::BTreeMap;
use std::path::PathBuf;

use lsp_types::{OneOf, PrepareRenameResponse, TextEdit, Uri, WorkspaceEdit};
use lume_core::ChangeSet;
use ropey::Rope;

/// Perché una rinomina non si può fare. Ognuno di questi è un **no detto
/// prima**, non un guasto scoperto a metà strada.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenameError {
    /// ATTENZIONE: rust-analyzer la manda quando si rinomina un `mod foo;`: vuole
    /// rinominare **il file**. Creare, spostare o cancellare file non è
    /// quello che questo comando promette, e applicarne metà sarebbe peggio.
    Operations,
    /// Il server ha risposto, ma senza modifiche.
    Nothing,
    /// Una URI che non è un file su disco: non c'è niente da aprire.
    NotAFile(String),
    Unreadable(PathBuf, String),
    Worker(String),
    /// Il testo su cui il server ha ragionato non è più quello che abbiamo.
    Stale(PathBuf),
    /// Il server ha elencato più di una modifica per lo stesso documento.
    Duplicate(PathBuf),
    /// Intervalli fuori dal testo, rovesciati o sovrapposti.
    Impossible(PathBuf),
}

impl std::fmt::Display for RenameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Operations => write!(
                f,
                "Questa rinomina vorrebbe rinominare dei file: non ancora supportato"
            ),
            Self::Nothing => write!(f, "Niente da rinominare"),
            Self::NotAFile(uri) => write!(f, "Non è un file su disco: {uri}"),
            Self::Unreadable(path, motivo) => {
                write!(f, "Lettura di {} fallita: {motivo}", path.display())
            }
            Self::Worker(error) => write!(f, "worker di rinomina non disponibile: {error}"),
            Self::Stale(path) => write!(
                f,
                "{} è cambiato mentre il server rispondeva: riprova",
                path.display()
            ),
            Self::Duplicate(path) => write!(
                f,
                "La rinomina cita più volte {}: piano rifiutato",
                path.display()
            ),
            Self::Impossible(path) => write!(
                f,
                "Le modifiche proposte per {} non stanno insieme",
                path.display()
            ),
        }
    }
}

/// Cosa dice il server sulla possibilità di rinominare **qui**.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenameStart {
    /// Non si rinomina: una parola chiave, uno spazio, un letterale. Il
    /// riquadro non si apre affatto.
    Impossibile,
    /// Il server propone lui il nome di partenza, o l'intervallo da cui
    /// leggerlo.
    Suggerito(String),
    /// Il server dice solo «sì»: il nome di partenza lo prende chi chiama,
    /// dalla parola sotto il cursore.
    Predefinito,
}

/// Legge la risposta a `prepareRename`, **in tutte e tre le sue forme**.
///
/// `Range` è la forma più comune e va letta sul nostro testo; la forma con
/// `placeholder` è quella che porta il nome già pronto (utile dove il testo
/// e il simbolo non coincidono, come un `r#tipo`); `DefaultBehavior` è un sì
/// senza dettagli. `None` è il no.
pub fn rename_start(response: Option<PrepareRenameResponse>, rope: &Rope) -> RenameStart {
    match response {
        None => RenameStart::Impossibile,
        Some(PrepareRenameResponse::DefaultBehavior { .. }) => RenameStart::Predefinito,
        Some(PrepareRenameResponse::RangeWithPlaceholder { placeholder, .. }) => {
            RenameStart::Suggerito(placeholder)
        }
        Some(PrepareRenameResponse::Range(range)) => {
            match (
                crate::lsp_pos::char_index_of(rope, range.start),
                crate::lsp_pos::char_index_of(rope, range.end),
            ) {
                (Some(da), Some(a)) if da <= a => {
                    RenameStart::Suggerito(rope.slice(da..a).to_string())
                }
                // un intervallo che il nostro testo non contiene parla di
                // un'altra revisione: meglio il nome che abbiamo sotto il
                // cursore di uno preso a caso
                _ => RenameStart::Predefinito,
            }
        }
    }
}

/// Le modifiche del server, appiattite e ordinate per file.
///
/// Le `WorkspaceEdit` hanno **due** contenitori — `changes` (vecchio) e
/// `document_changes` (nuovo) — e i server usano l'uno o l'altro. Guardarne
/// uno solo darebbe una rinomina che non fa niente, in silenzio.
pub fn rename_edits(edit: &WorkspaceEdit) -> Result<Vec<(Uri, Vec<TextEdit>)>, RenameError> {
    let mut per_file: Vec<(Uri, Vec<TextEdit>)> = Vec::new();
    match &edit.document_changes {
        Some(lsp_types::DocumentChanges::Operations(_)) => return Err(RenameError::Operations),
        Some(lsp_types::DocumentChanges::Edits(edits)) => {
            for documento in edits {
                let modifiche: Vec<TextEdit> = documento
                    .edits
                    .iter()
                    .map(|edit| match edit {
                        OneOf::Left(edit) => edit.clone(),
                        OneOf::Right(annotato) => annotato.text_edit.clone(),
                    })
                    .collect();
                per_file.push((documento.text_document.uri.clone(), modifiche));
            }
        }
        None => {
            for (uri, modifiche) in edit.changes.iter().flatten() {
                per_file.push((uri.clone(), modifiche.clone()));
            }
        }
    }
    per_file.retain(|(_, modifiche)| !modifiche.is_empty());
    if per_file.is_empty() {
        return Err(RenameError::Nothing);
    }
    // ATTENZIONE: Ordine stabile: la notifica finale conta i file, e `changes` è una
    // mappa hash — senza questo, due rinomine identiche darebbero due liste
    // in ordine diverso, e il test sarebbe una moneta.
    per_file.sort_by(|(a, _), (b, _)| a.as_str().cmp(b.as_str()));
    Ok(per_file)
}

/// Il piano: una `ChangeSet` per file, **tutte costruite prima che una sola
/// venga applicata**.
///
/// `testi` porta il rope di ogni file coinvolto, indicizzato per URI: gli
/// aperti dal workspace, i chiusi letti dal disco *prima* di arrivare qui.
/// Se manca, è un file che non si è riusciti a leggere, e il chiamante l'ha
/// già trasformato in un errore.
pub fn rename_plan(
    edits: &[(Uri, Vec<TextEdit>)],
    testi: &BTreeMap<String, (PathBuf, Rope)>,
) -> Result<Vec<(String, ChangeSet)>, RenameError> {
    let mut piano = Vec::with_capacity(edits.len());
    for (uri, modifiche) in edits {
        let Some((path, rope)) = testi.get(uri.as_str()) else {
            return Err(RenameError::NotAFile(uri.as_str().to_owned()));
        };
        let mut intervalli = Vec::with_capacity(modifiche.len());
        for modifica in modifiche {
            let (Some(da), Some(a)) = (
                crate::lsp_pos::char_index_of(rope, modifica.range.start),
                crate::lsp_pos::char_index_of(rope, modifica.range.end),
            ) else {
                return Err(RenameError::Impossible(path.clone()));
            };
            intervalli.push((da, a, modifica.new_text.clone()));
        }
        // `replace_many` è **costruttore e validatore insieme**: rifiuta
        // intervalli rovesciati, fuori dal testo o sovrapposti. Ed è il
        // motivo per cui una rinomina di sette occorrenze si annulla con un
        // Ctrl+Z solo, invece che con sette.
        let changeset = ChangeSet::replace_many(&intervalli, rope.len_chars())
            .ok_or_else(|| RenameError::Impossible(path.clone()))?;
        piano.push((uri.as_str().to_owned(), changeset));
    }
    Ok(piano)
}

/// Un piano validato ma non ancora applicato. Conserva le revisioni usate per
/// calcolare le modifiche, così il consenso può arrivare in seguito senza
/// applicare intervalli stantii.
#[derive(Debug)]
pub struct PreparedEdit {
    pub files: Vec<PreparedFile>,
    #[cfg(test)]
    pub worker_thread: std::thread::ThreadId,
}

#[derive(Debug)]
pub struct PreparedFile {
    pub path: PathBuf,
    pub changeset: ChangeSet,
    pub occurrences: usize,
    pub source: Rope,
    pub source_fingerprint: Option<[u8; 32]>,
    pub source_document: Option<lume_core::DocId>,
    pub source_version: Option<u64>,
}

/// Snapshot dei documenti già aperti. I documenti chiusi vengono letti solo
/// dentro `prepare_workspace_edit_async`.
#[derive(Debug)]
pub struct RenameSource {
    pub path: PathBuf,
    pub text: Rope,
    pub document: Option<lume_core::DocId>,
    pub version: Option<u64>,
    pub fingerprint: Option<[u8; 32]>,
    pub synced: bool,
}

/// Prepara testo e fingerprint fuori dal thread UI. Il worker riceve solo
/// snapshot owned, quindi non può osservare o modificare lo stato dell'app.
pub async fn prepare_workspace_edit_async(
    edit: WorkspaceEdit,
    open_sources: BTreeMap<String, RenameSource>,
) -> Result<PreparedEdit, RenameError> {
    tokio::task::spawn_blocking(move || prepare_workspace_edit_blocking(edit, open_sources))
        .await
        .map_err(|error| RenameError::Worker(error.to_string()))?
}

pub(crate) fn prepare_workspace_edit_blocking(
    edit: WorkspaceEdit,
    mut open_sources: BTreeMap<String, RenameSource>,
) -> Result<PreparedEdit, RenameError> {
    let edits = rename_edits(&edit)?;
    let mut sources = BTreeMap::new();
    let mut fingerprints = BTreeMap::new();
    let mut documents = BTreeMap::new();
    let mut seen_uris = std::collections::BTreeSet::new();

    if let Some(lsp_types::DocumentChanges::Edits(document_edits)) = &edit.document_changes {
        for document_edit in document_edits {
            let Some(proposed) = document_edit.text_document.version else {
                continue;
            };
            let uri = document_edit.text_document.uri.as_str();
            let path = crate::lsp_uri::uri_to_path(&document_edit.text_document.uri)
                .ok_or_else(|| RenameError::NotAFile(uri.to_owned()))?;
            let current = open_sources.get(uri).and_then(|source| {
                source
                    .version
                    .and_then(|version| i32::try_from(version).ok())
            });
            if current != Some(proposed) {
                return Err(RenameError::Stale(path));
            }
        }
    }

    for (uri, _) in &edits {
        let uri_key = uri.as_str().to_owned();
        if !seen_uris.insert(uri_key.clone()) {
            let path = crate::lsp_uri::uri_to_path(uri)
                .ok_or_else(|| RenameError::NotAFile(uri_key.clone()))?;
            return Err(RenameError::Duplicate(path));
        }
        let path = crate::lsp_uri::uri_to_path(uri)
            .ok_or_else(|| RenameError::NotAFile(uri_key.clone()))?;
        let loaded = if let Some(source) = open_sources.remove(&uri_key) {
            if !source.synced {
                return Err(RenameError::Stale(path));
            }
            source
        } else {
            let text = crate::file_read::read_testo_capped(&path, crate::file_read::MAX_FILE_BYTES)
                .map_err(|error| RenameError::Unreadable(path.clone(), error.to_string()))?;
            let fingerprint = text.fingerprint();
            let (without_bom, _) = lume_core::strip_bom(&text);
            let (normalized, _) = lume_core::normalize(without_bom);
            RenameSource {
                path: path.clone(),
                text: Rope::from_str(&normalized),
                document: None,
                version: None,
                fingerprint: Some(fingerprint),
                synced: true,
            }
        };
        if loaded.path != path {
            return Err(RenameError::Stale(path));
        }
        fingerprints.insert(uri_key.clone(), loaded.fingerprint);
        documents.insert(uri_key.clone(), (loaded.document, loaded.version));
        sources.insert(uri_key, (path, loaded.text));
    }

    let plan = rename_plan(&edits, &sources)?;
    let mut files = Vec::with_capacity(plan.len());
    for (uri, changeset) in plan {
        let Some((path, source)) = sources.get(&uri) else {
            return Err(RenameError::NotAFile(uri));
        };
        let occurrences = edits
            .iter()
            .find(|(edit_uri, _)| edit_uri.as_str() == uri)
            .map_or(0, |(_, changes)| changes.len());
        let (source_document, source_version) = documents.get(&uri).copied().unwrap_or_default();
        files.push(PreparedFile {
            path: path.clone(),
            changeset,
            occurrences,
            source: source.clone(),
            source_fingerprint: fingerprints.get(&uri).copied().flatten(),
            source_document,
            source_version,
        });
    }
    Ok(PreparedEdit {
        files,
        #[cfg(test)]
        worker_thread: std::thread::current().id(),
    })
}

impl PreparedEdit {
    pub fn occurrences(&self) -> usize {
        self.files.iter().map(|file| file.occurrences).sum()
    }
}

/// Quante occorrenze tocca il piano. È il numero che va nella notifica: «in
/// M file» da solo non dice quanto lavoro ha fatto.
#[cfg(test)]
pub fn occurrences(edits: &[(Uri, Vec<TextEdit>)]) -> usize {
    edits.iter().map(|(_, modifiche)| modifiche.len()).sum()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::{RenameError, occurrences, rename_edits, rename_plan};
    use lsp_types::{Position, Range, TextEdit, Uri, WorkspaceEdit};
    use ropey::Rope;
    use std::collections::BTreeMap;

    fn uri(value: &str) -> Uri {
        value.parse().unwrap()
    }

    fn modifica(riga: u32, da: u32, a: u32, testo: &str) -> TextEdit {
        TextEdit {
            range: Range::new(Position::new(riga, da), Position::new(riga, a)),
            new_text: testo.to_owned(),
        }
    }

    fn testi(rope: &str) -> BTreeMap<String, (std::path::PathBuf, Rope)> {
        BTreeMap::from([(
            "file:///x/main.rs".to_owned(),
            (std::path::PathBuf::from("/x/main.rs"), Rope::from_str(rope)),
        )])
    }

    /// ATTENZIONE: Tre forme anche qui, e la differenza fra la terza e il `None` è
    /// quella che conta: `DefaultBehavior` è un **sì** senza dettagli, e
    /// confonderlo con il no toglierebbe la rinomina proprio dove funziona.
    #[test]
    fn prepare_rename_si_legge_in_tutte_e_tre_le_forme() {
        use super::{RenameStart, rename_start};
        use lsp_types::PrepareRenameResponse;
        let rope = Rope::from_str("fn mian() {}\n");

        assert_eq!(rename_start(None, &rope), RenameStart::Impossibile);
        assert_eq!(
            rename_start(
                Some(PrepareRenameResponse::DefaultBehavior {
                    default_behavior: true
                }),
                &rope
            ),
            RenameStart::Predefinito
        );
        assert_eq!(
            rename_start(
                Some(PrepareRenameResponse::RangeWithPlaceholder {
                    range: Range::new(Position::new(0, 3), Position::new(0, 7)),
                    placeholder: "mian".to_owned(),
                }),
                &rope
            ),
            RenameStart::Suggerito("mian".to_owned())
        );
        // la forma con il solo intervallo si legge **sul nostro testo**
        assert_eq!(
            rename_start(
                Some(PrepareRenameResponse::Range(Range::new(
                    Position::new(0, 3),
                    Position::new(0, 7)
                ))),
                &rope
            ),
            RenameStart::Suggerito("mian".to_owned())
        );
        // e un intervallo che il nostro testo non contiene non inventa un
        // nome: si ripiega sulla parola sotto il cursore
        assert_eq!(
            rename_start(
                Some(PrepareRenameResponse::Range(Range::new(
                    Position::new(9, 0),
                    Position::new(9, 4)
                ))),
                &rope
            ),
            RenameStart::Predefinito
        );
    }

    #[test]
    fn la_preparazione_workspace_legge_i_file_chiusi_su_worker() {
        use super::prepare_workspace_edit_async;
        use std::time::{SystemTime, UNIX_EPOCH};

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("lume_rename_worker_{nonce}"));
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("closed.rs");
        std::fs::write(&file, "fn mian() {}\n").unwrap();
        let uri = crate::lsp_uri::path_to_uri(&file).unwrap();
        let edit = WorkspaceEdit {
            changes: Some(std::collections::HashMap::from([(
                uri,
                vec![modifica(0, 3, 7, "main")],
            )])),
            ..WorkspaceEdit::default()
        };
        let caller = std::thread::current().id();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let prepared = runtime
            .block_on(prepare_workspace_edit_async(edit, BTreeMap::new()))
            .unwrap();

        assert_ne!(prepared.worker_thread, caller);
        assert_eq!(prepared.files.len(), 1);
        assert_eq!(prepared.files[0].source.to_string(), "fn mian() {}\n");
        assert!(prepared.files[0].source_fingerprint.is_some());
        let _ = std::fs::remove_dir_all(root);
    }

    /// ATTENZIONE: Le `WorkspaceEdit` hanno due contenitori e i server usano l'uno o
    /// l'altro. Leggerne uno solo darebbe una rinomina che **non fa niente**,
    /// in silenzio — l'utente digita il nome nuovo, preme invio, e non
    /// succede nulla.
    #[test]
    fn le_modifiche_si_leggono_da_tutti_e_due_i_contenitori() {
        let vecchio = WorkspaceEdit {
            changes: Some(
                [(uri("file:///x/main.rs"), vec![modifica(0, 3, 7, "nuovo")])]
                    .into_iter()
                    .collect(),
            ),
            ..WorkspaceEdit::default()
        };
        let letto = rename_edits(&vecchio).unwrap();
        assert_eq!(letto.len(), 1);
        assert_eq!(occurrences(&letto), 1);

        #[allow(deprecated)]
        let nuovo = WorkspaceEdit {
            document_changes: Some(lsp_types::DocumentChanges::Edits(vec![
                lsp_types::TextDocumentEdit {
                    text_document: lsp_types::OptionalVersionedTextDocumentIdentifier {
                        uri: uri("file:///x/main.rs"),
                        version: None,
                    },
                    edits: vec![
                        lsp_types::OneOf::Left(modifica(0, 3, 7, "nuovo")),
                        lsp_types::OneOf::Left(modifica(1, 0, 4, "nuovo")),
                    ],
                },
            ])),
            ..WorkspaceEdit::default()
        };
        assert_eq!(occurrences(&rename_edits(&nuovo).unwrap()), 2);

        // e «niente da fare» si distingue da «non ho capito»
        assert_eq!(
            rename_edits(&WorkspaceEdit::default()),
            Err(RenameError::Nothing)
        );
    }

    /// ATTENZIONE: Rinominare **un file** non è quello che questo comando promette.
    /// rust-analyzer manda `Operations` quando si rinomina un `mod foo;`, e
    /// applicarne la metà testuale lascerebbe il progetto che non compila —
    /// un «non ancora supportato» è infinitamente meglio.
    #[test]
    fn rinominare_file_si_rifiuta_invece_di_farne_meta() {
        let edit = WorkspaceEdit {
            document_changes: Some(lsp_types::DocumentChanges::Operations(vec![
                lsp_types::DocumentChangeOperation::Op(lsp_types::ResourceOp::Rename(
                    lsp_types::RenameFile {
                        old_uri: uri("file:///x/foo.rs"),
                        new_uri: uri("file:///x/bar.rs"),
                        options: None,
                        annotation_id: None,
                    },
                )),
            ])),
            ..WorkspaceEdit::default()
        };
        assert_eq!(rename_edits(&edit), Err(RenameError::Operations));
    }

    /// Il piano si costruisce **tutto** prima che una sola modifica venga
    /// applicata: intervalli sovrapposti, rovesciati o fuori dal testo sono
    /// un no detto adesso, non un documento mangiato a metà.
    #[test]
    fn intervalli_che_non_stanno_insieme_fermano_tutto_prima() {
        let edits = vec![(
            uri("file:///x/main.rs"),
            vec![modifica(0, 3, 7, "nuovo"), modifica(0, 5, 9, "nuovo")],
        )];
        assert!(matches!(
            rename_plan(&edits, &testi("fn mian() {}\n")),
            Err(RenameError::Impossible(_))
        ));

        // una riga che non esiste nel nostro testo parla di un altro file
        let fuori = vec![(uri("file:///x/main.rs"), vec![modifica(9, 0, 4, "nuovo")])];
        assert!(matches!(
            rename_plan(&fuori, &testi("fn mian() {}\n")),
            Err(RenameError::Impossible(_))
        ));

        // e un file di cui non abbiamo il testo non si tira a indovinare
        assert!(matches!(
            rename_plan(&fuori, &BTreeMap::new()),
            Err(RenameError::NotAFile(_))
        ));
    }

    /// Il caso buono, e la prova che il risultato è **una sola revisione**:
    /// `replace_many` costruisce una `ChangeSet` per file, quindi annullare
    /// una rinomina di due occorrenze costa un Ctrl+Z, non due.
    #[test]
    fn due_occorrenze_nello_stesso_file_diventano_una_revisione_sola() {
        let edits = vec![(
            uri("file:///x/main.rs"),
            vec![modifica(0, 3, 7, "main"), modifica(1, 4, 8, "main")],
        )];
        let sorgente = "fn mian() {}\n    mian();\n";
        let piano = rename_plan(&edits, &testi(sorgente)).unwrap();
        assert_eq!(piano.len(), 1, "un file, una ChangeSet");

        let mut rope = Rope::from_str(sorgente);
        piano[0].1.apply(&mut rope);
        assert_eq!(rope.to_string(), "fn main() {}\n    main();\n");
    }
}
