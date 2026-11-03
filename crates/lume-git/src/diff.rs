//! Diff di un singolo file, in righe già pronte da disegnare.
//!
//! Il prototipo mostra due grondaie (numero riga vecchio, numero riga nuovo),
//! una colonna segno e il testo. Questo modulo produce esattamente quello.
//!
//! **Perché non si leggono i byte grezzi.** La tentazione è aprire il file dal
//! worktree, tirare fuori il blob di HEAD e passarli a `imara-diff`. Su
//! Windows con `core.autocrlf` attivo — cioè su questa macchina — il blob è a
//! LF e il file su disco è a CRLF: ogni riga risulterebbe cambiata. Passare
//! invece dal `diff_resource_cache` di gitoxide applica la stessa pipeline di
//! filtri che userebbe `git`, e in più riconosce da sé i file binari.

use std::ops::Range;
use std::path::{Path, PathBuf};

use gix::bstr::BStr;

use crate::{GitError, GitRepository, worktree::git_path};

/// Righe di contesto attorno a ogni modifica, come il default di `git`.
const CONTEXT: u32 = 3;

/// Tetto alle righe prodotte. Un file nuovo da 50.000 righe diventerebbe
/// altrettanti widget: il visualizzatore si pianterebbe, e nessuno legge un
/// diff così scorrendolo. Oltre il tetto si tronca e lo si dice.
const MAX_ROWS: usize = 5_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiffSign {
    Context,
    Added,
    Removed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiffRow {
    Line {
        /// Numero di riga nella versione base (1-based), assente se aggiunta.
        old_line: Option<u32>,
        /// Numero di riga nella versione nuova, assente se rimossa.
        new_line: Option<u32>,
        sign: DiffSign,
        text: String,
        /// Indice in `FileDiff::hunks` per le righe Added/Removed; `None`
        /// per le righe di contesto.
        hunk_idx: Option<usize>,
    },
    /// Righe identiche saltate fra due modifiche lontane.
    Gap { skipped: u32 },
}

/// Un hunk: blocco atomico di modifiche nel diff. Trasporta i range e il
/// testo concreto delle righe per permettere la messa in stage del singolo
/// hunk senza ricalcolare il diff.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiffHunk {
    /// Range 0-based nel file «vecchio» (HEAD/index): `input.before[old_range]`.
    pub old_range: Range<u32>,
    /// Range 0-based nel file «nuovo» (worktree): `input.after[new_range]`.
    pub new_range: Range<u32>,
    /// Righe rimosse (lato vecchio, senza terminatore).
    pub old_lines: Vec<String>,
    /// Righe aggiunte (lato nuovo, senza terminatore).
    pub new_lines: Vec<String>,
}

impl DiffHunk {
    /// Prima riga nel file vecchio, 1-based (0 per inserimento puro in cima).
    pub fn old_start_1based(&self) -> u32 {
        self.old_range.start + 1
    }

    /// Prima riga nel file nuovo, 1-based.
    pub fn new_start_1based(&self) -> u32 {
        self.new_range.start + 1
    }
}

/// Base del confronto, cioè cosa c'è «a sinistra».
///
/// `Worktree` e `Index` **partizionano** il cambiamento come fa git: quel che
/// è già in stage sta in `Index` e non ricompare in `Worktree`. È la proprietà
/// su cui poggia lo staging per hunk — ogni hunk vive in esattamente una delle
/// due viste, e il pulsante che lo accompagna applica sempre quel che mostra.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiffBase {
    /// Worktree contro index (`git diff`): quel che **non** è ancora in stage.
    Worktree,
    /// Index contro HEAD (`git diff --cached`): quel che finirebbe nel commit.
    Index,
    /// Un commit contro il suo primo parent: l'intestazione dice l'hash.
    Commit(String),
}

impl DiffBase {
    pub fn label(&self) -> String {
        match self {
            Self::Worktree => "Locali".to_owned(),
            Self::Index => "In stage".to_owned(),
            Self::Commit(id) => id.get(..7).unwrap_or(id).to_owned(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileDiff {
    pub path: PathBuf,
    pub base: DiffBase,
    pub rows: Vec<DiffRow>,
    /// Tutti gli hunk del diff, nell'ordine in cui compaiono. Le righe
    /// `DiffRow::Line` con `hunk_idx = Some(i)` appartengono a `hunks[i]`.
    pub hunks: Vec<DiffHunk>,
    pub added: u32,
    pub removed: u32,
    /// Nessuna riga da mostrare: gitoxide ha riconosciuto un binario.
    pub binary: bool,
    /// Le righe si fermano a `MAX_ROWS`.
    pub truncated: bool,
}

impl GitRepository {
    /// Attribuisce al contenuto corrente del buffer le righe che coincidono
    /// con HEAD. Le righe inserite o sostituite nel buffer non hanno autore.
    pub fn blame_aligned(
        &self,
        path: &Path,
        text: &str,
    ) -> Result<Vec<Option<crate::BlameLine>>, GitError> {
        let blame = self.blame(path)?;
        let relative = self.relative_path(path)?;
        let rela = git_path(relative)?;
        let head = self.head_blob(rela.as_ref())?;
        let Some(head) = head else {
            return Ok(text.lines().map(|_| None).collect());
        };
        let blob = self
            .repository
            .find_blob(head)
            .map_err(|error| GitError::Diff(error.to_string()))?;
        // Document::from_str normalizza CRLF nel buffer. Allinea il blob
        // grezzo allo stesso modello prima del confronto, senza passare da
        // UTF-8 lossy né alterare i CR isolati.
        let mut normalized_head = Vec::with_capacity(blob.data.len());
        for line in blob.data.split_inclusive(|byte| *byte == b'\n') {
            if let Some(without_crlf) = line.strip_suffix(b"\r\n") {
                normalized_head.extend_from_slice(without_crlf);
                normalized_head.push(b'\n');
            } else {
                normalized_head.extend_from_slice(line);
            }
        }
        let input =
            gix::diff::blob::InternedInput::new(normalized_head.as_slice(), text.as_bytes());
        let diff = gix::diff::blob::diff_with_slider_heuristics(
            gix::diff::blob::Algorithm::Histogram,
            &input,
        );
        let new_len = text.lines().count();
        let mut aligned = vec![None; new_len];
        let mut old = 0usize;
        let mut new = 0usize;
        for hunk in diff.hunks() {
            let before = hunk.before.start as usize;
            let after = hunk.after.start as usize;
            while old < before && new < after {
                let Some(slot) = aligned.get_mut(new) else {
                    break;
                };
                *slot = blame.get(old).cloned();
                old += 1;
                new += 1;
            }
            old = hunk.before.end as usize;
            new = hunk.after.end as usize;
        }
        while old < blame.len() && new < new_len {
            let Some(slot) = aligned.get_mut(new) else {
                break;
            };
            *slot = blame.get(old).cloned();
            old += 1;
            new += 1;
        }
        Ok(aligned)
    }

    /// Diff del file nel worktree contro l'**index** — la base «Locali» del
    /// pannello Commit, cioè quel che non è ancora in stage. Un file non
    /// versionato risulta tutto in aggiunta, uno cancellato tutto in rimozione.
    ///
    /// **Perché l'index e non HEAD.** Il lato vecchio è il contenuto su cui
    /// [`GitRepository::stage_hunk`] applica l'hunk, e `hunk.old_range` ne è
    /// l'offset: se qui ci fosse HEAD i due divergerebbero appena un hunk
    /// entra in stage, e mettere in stage il successivo fallirebbe o — peggio,
    /// con un inserimento puro — riuscirebbe nel punto sbagliato. Con l'index
    /// l'offset è esatto per costruzione. È anche ciò che rende le due viste
    /// una partizione: un hunk già in stage non ricompare qui.
    ///
    /// Il fallback su HEAD serve al file mai entrato nell'index: stessa
    /// politica di `blob_bytes_from_index_or_head`.
    pub fn diff_worktree_file(&self, path: &Path) -> Result<FileDiff, GitError> {
        let relative = self.relative_path(path)?;
        let rela = git_path(relative)?;
        let old = match self.index_blob(rela.as_ref())? {
            Some(id) => Some(id),
            None => self.head_blob(rela.as_ref())?,
        };

        let mut platform = self
            .repository
            .diff_resource_cache(
                gix::diff::blob::pipeline::Mode::ToGit,
                gix::diff::blob::pipeline::WorktreeRoots {
                    // Solo il lato nuovo viene dal disco: il vecchio è
                    // l'oggetto in HEAD, e senza root non viene cercato lì.
                    old_root: None,
                    new_root: Some(self.work_dir.clone()),
                },
            )
            .map_err(|error| GitError::Diff(error.to_string()))?;

        set_side(
            &mut platform,
            &self.repository,
            old.unwrap_or_else(null_id),
            rela.as_ref(),
            gix::diff::blob::ResourceKind::OldOrSource,
        )?;
        set_side(
            &mut platform,
            &self.repository,
            // Id nullo più una worktree root: gitoxide legge dal disco, e se
            // il file non c'è la risorsa è `Missing` — cioè cancellato.
            null_id(),
            rela.as_ref(),
            gix::diff::blob::ResourceKind::NewOrDestination,
        )?;

        finish(platform, relative.to_path_buf(), DiffBase::Worktree)
    }

    /// Diff del file **in stage** contro HEAD — la base «In stage», cioè quel
    /// che finirebbe nel prossimo commit.
    ///
    /// Gli hunk che escono di qui sono quelli che
    /// [`GitRepository::unstage_hunk`] sa togliere: il suo lato «nuovo» è
    /// l'index, ed è su quel contenuto che l'hunk viene riapplicato al
    /// contrario. Un hunk preso dal diff del worktree qui non funzionerebbe.
    ///
    /// Entrambi i lati sono blob già in oggetto, quindi niente worktree root:
    /// un id nullo significa «assente» — file nuovo in stage (niente in HEAD)
    /// o rimosso dall'index — e non «leggi dal disco».
    pub fn diff_index_file(&self, path: &Path) -> Result<FileDiff, GitError> {
        let relative = self.relative_path(path)?;
        let rela = git_path(relative)?;
        let old = self.head_blob(rela.as_ref())?;
        let new = self.index_blob(rela.as_ref())?;

        let mut platform = self
            .repository
            .diff_resource_cache(
                gix::diff::blob::pipeline::Mode::ToGit,
                gix::diff::blob::pipeline::WorktreeRoots::default(),
            )
            .map_err(|error| GitError::Diff(error.to_string()))?;
        set_side(
            &mut platform,
            &self.repository,
            old.unwrap_or_else(null_id),
            rela.as_ref(),
            gix::diff::blob::ResourceKind::OldOrSource,
        )?;
        set_side(
            &mut platform,
            &self.repository,
            new.unwrap_or_else(null_id),
            rela.as_ref(),
            gix::diff::blob::ResourceKind::NewOrDestination,
        )?;

        finish(platform, relative.to_path_buf(), DiffBase::Index)
    }

    /// Diff del file dentro un commit, contro il suo primo parent — la base
    /// usata dalla lista FILE MODIFICATI del Log Git.
    pub fn diff_commit_file(&self, commit: &str, path: &Path) -> Result<FileDiff, GitError> {
        let relative = self.relative_path(path)?;
        let rela = git_path(relative)?;
        let oid = gix::ObjectId::from_hex(commit.as_bytes())
            .map_err(|error| GitError::Diff(error.to_string()))?;
        let commit_object = self
            .repository
            .find_commit(oid)
            .map_err(|error| GitError::Diff(error.to_string()))?;
        let tree = commit_object
            .tree()
            .map_err(|error| GitError::Diff(error.to_string()))?;
        let new = blob_in_tree(&tree, rela.as_ref());
        let old = match commit_object.parent_ids().next() {
            Some(parent) => {
                let parent = parent
                    .object()
                    .map_err(|error| GitError::Diff(error.to_string()))?
                    .peel_to_commit()
                    .map_err(|error| GitError::Diff(error.to_string()))?;
                let tree = parent
                    .tree()
                    .map_err(|error| GitError::Diff(error.to_string()))?;
                blob_in_tree(&tree, rela.as_ref())
            }
            None => None,
        };

        let mut platform = self
            .repository
            .diff_resource_cache(
                gix::diff::blob::pipeline::Mode::ToGit,
                gix::diff::blob::pipeline::WorktreeRoots::default(),
            )
            .map_err(|error| GitError::Diff(error.to_string()))?;
        set_side(
            &mut platform,
            &self.repository,
            old.unwrap_or_else(null_id),
            rela.as_ref(),
            gix::diff::blob::ResourceKind::OldOrSource,
        )?;
        set_side(
            &mut platform,
            &self.repository,
            new.unwrap_or_else(null_id),
            rela.as_ref(),
            gix::diff::blob::ResourceKind::NewOrDestination,
        )?;

        finish(
            platform,
            relative.to_path_buf(),
            DiffBase::Commit(commit.to_owned()),
        )
    }

    /// Blob del file in HEAD, se c'è. `None` copre sia «HEAD non esiste»
    /// (repository senza commit) sia «file non versionato».
    /// Blob del file **nell'index**, se c'è. `None` significa «non in stage».
    ///
    /// Un file in conflitto non ha un solo entry ma tre (base/ours/theirs), e
    /// `entry_by_path` ne restituisce comunque uno: diffarlo darebbe una vista
    /// leggibile e sbagliata, e uno staging per hunk su quella vista
    /// scriverebbe un entry non in conflitto accanto agli altri tre, cioè un
    /// index rotto. Meglio dire di no.
    pub(crate) fn index_blob(&self, rela: &BStr) -> Result<Option<gix::ObjectId>, GitError> {
        let index = crate::index_write::fresh_index(
            &self.repository,
            crate::index_write::MissingIndex::Head,
        )
        .map_err(GitError::Diff)?;
        let Some(entry) = index.entry_by_path(rela) else {
            return Ok(None);
        };
        if entry.stage() != gix::index::entry::Stage::Unconflicted {
            return Err(GitError::Diff(format!(
                "«{rela}» ha un conflitto non risolto: niente diff dell'index"
            )));
        }
        Ok(Some(entry.id))
    }

    pub(crate) fn head_blob(&self, rela: &BStr) -> Result<Option<gix::ObjectId>, GitError> {
        let Ok(head) = self.repository.head() else {
            return Ok(None);
        };
        let Some(id) = head.id() else {
            return Ok(None);
        };
        let Ok(commit) = id
            .object()
            .map_err(|_| ())
            .and_then(|object| object.peel_to_commit().map_err(|_| ()))
        else {
            return Ok(None);
        };
        let tree = commit
            .tree()
            .map_err(|error| GitError::Diff(error.to_string()))?;
        Ok(blob_in_tree(&tree, rela))
    }
}

/// L'id nullo dice a gitoxide «questa versione non esiste»: file nuovo sul
/// lato vecchio, cancellato sul lato nuovo.
fn null_id() -> gix::ObjectId {
    gix::ObjectId::null(gix::hash::Kind::Sha1)
}

fn blob_in_tree(tree: &gix::Tree<'_>, rela: &BStr) -> Option<gix::ObjectId> {
    // Nomi raw su Unix e conversione fallibile su Windows, senza inventare
    // un blob assente soltanto perché il path non è una stringa UTF-8.
    let path = gix::path::try_from_bstr(rela).ok()?;
    let entry = tree.lookup_entry_by_path(path.as_ref()).ok()??;
    Some(entry.object_id())
}

fn set_side(
    platform: &mut gix::diff::blob::Platform,
    repository: &gix::Repository,
    id: gix::ObjectId,
    rela: &BStr,
    kind: gix::diff::blob::ResourceKind,
) -> Result<(), GitError> {
    platform
        .set_resource(
            id,
            gix::object::tree::EntryKind::Blob,
            rela,
            kind,
            &repository.objects,
        )
        .map_err(|error| GitError::Diff(error.to_string()))
}

fn finish(
    mut platform: gix::diff::blob::Platform,
    path: PathBuf,
    base: DiffBase,
) -> Result<FileDiff, GitError> {
    let outcome = platform
        .prepare_diff()
        .map_err(|error| GitError::Diff(error.to_string()))?;

    let algorithm = match outcome.operation {
        gix::diff::blob::platform::prepare_diff::Operation::InternalDiff { algorithm } => algorithm,
        // Un binario, o un comando di diff esterno configurato dall'utente
        // che qui non si esegue: in entrambi i casi non ci sono righe.
        _ => {
            return Ok(FileDiff {
                path,
                base,
                rows: Vec::new(),
                hunks: Vec::new(),
                added: 0,
                removed: 0,
                binary: true,
                truncated: false,
            });
        }
    };

    let input = outcome.interned_input();
    let diff = gix::diff::blob::diff_with_slider_heuristics(algorithm, &input);
    let (rows, hunks, added, removed, truncated) = rows_from(&diff, &input);
    Ok(FileDiff {
        path,
        base,
        rows,
        hunks,
        added,
        removed,
        binary: false,
        truncated,
    })
}

/// Da hunk a righe. `imara-diff` dà solo gli intervalli cambiati: contesto e
/// numeri di riga si ricavano camminando le due sequenze in parallelo.
///
/// Restituisce `(righe, hunks, aggiunte, rimozioni, troncato)`.
fn rows_from(
    diff: &gix::diff::blob::Diff,
    input: &gix::diff::blob::InternedInput<&[u8]>,
) -> (Vec<DiffRow>, Vec<DiffHunk>, u32, u32, bool) {
    let mut rows = Vec::new();
    let mut hunks: Vec<DiffHunk> = Vec::new();
    let mut added = 0;
    let mut removed = 0;
    // Posizione corrente nelle due sequenze, 0-based; i numeri mostrati sono
    // 1-based.
    let mut before = 0u32;
    let mut after = 0u32;

    for hunk in diff.hunks() {
        let hunk_idx = hunks.len();

        let gap = hunk.before.start.saturating_sub(before);
        if gap > CONTEXT * 2 {
            // Coda del blocco precedente, salto, testa di questo.
            emit_context(&mut rows, input, before..before + CONTEXT, after);
            rows.push(DiffRow::Gap {
                skipped: gap - CONTEXT * 2,
            });
            let head = hunk.before.start - CONTEXT;
            emit_context(
                &mut rows,
                input,
                head..hunk.before.start,
                after + (head - before),
            );
        } else {
            emit_context(&mut rows, input, before..hunk.before.start, after);
        }

        // Raccoglie il testo delle righe prima di emetterle, per costruire
        // il DiffHunk senza iterare due volte i range.
        let old_lines: Vec<String> = hunk
            .before
            .clone()
            .map(|idx| line(input, input.before.get(idx as usize).copied()))
            .collect();
        let new_lines: Vec<String> = hunk
            .after
            .clone()
            .map(|idx| line_after(input, idx))
            .collect();

        for (offset, text) in old_lines.iter().enumerate() {
            let index = hunk.before.start + offset as u32;
            rows.push(DiffRow::Line {
                old_line: Some(index + 1),
                new_line: None,
                sign: DiffSign::Removed,
                text: text.clone(),
                hunk_idx: Some(hunk_idx),
            });
            removed += 1;
        }
        for (offset, text) in new_lines.iter().enumerate() {
            let index = hunk.after.start + offset as u32;
            rows.push(DiffRow::Line {
                old_line: None,
                new_line: Some(index + 1),
                sign: DiffSign::Added,
                text: text.clone(),
                hunk_idx: Some(hunk_idx),
            });
            added += 1;
        }

        hunks.push(DiffHunk {
            old_range: hunk.before.clone(),
            new_range: hunk.after.clone(),
            old_lines,
            new_lines,
        });

        before = hunk.before.end;
        after = hunk.after.end;
    }

    let tail = u32::try_from(input.before.len()).unwrap_or(u32::MAX);
    let remaining = tail.saturating_sub(before);
    if remaining > CONTEXT {
        emit_context(&mut rows, input, before..before + CONTEXT, after);
        rows.push(DiffRow::Gap {
            skipped: remaining - CONTEXT,
        });
    } else {
        emit_context(&mut rows, input, before..tail, after);
    }

    let truncated = rows.len() > MAX_ROWS;
    if truncated {
        rows.truncate(MAX_ROWS);
        rows.push(DiffRow::Gap { skipped: 0 });
    }
    (rows, hunks, added, removed, truncated)
}

fn emit_context(
    rows: &mut Vec<DiffRow>,
    input: &gix::diff::blob::InternedInput<&[u8]>,
    range: Range<u32>,
    after_start: u32,
) {
    for (offset, index) in range.clone().enumerate() {
        let offset = u32::try_from(offset).unwrap_or(0);
        rows.push(DiffRow::Line {
            old_line: Some(index + 1),
            new_line: Some(after_start + offset + 1),
            sign: DiffSign::Context,
            text: line(input, input.before.get(index as usize).copied()),
            hunk_idx: None,
        });
    }
}

fn line_after(input: &gix::diff::blob::InternedInput<&[u8]>, index: u32) -> String {
    line(input, input.after.get(index as usize).copied())
}

fn line(
    input: &gix::diff::blob::InternedInput<&[u8]>,
    token: Option<gix::diff::blob::Token>,
) -> String {
    let Some(token) = token else {
        return String::new();
    };
    let bytes = input.interner[token];
    String::from_utf8_lossy(bytes)
        .trim_end_matches(['\n', '\r'])
        .to_owned()
}
