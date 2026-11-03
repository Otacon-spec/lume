#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
//! Test di integrazione per lo staging per hunk.
//!
//! Ogni test crea un repository temporaneo reale, non usa snapshot di stringhe.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use lume_git::{ChangeKind, DiffSign, GitRepository};

// ─────────────────────────────────────────────────────────────────────────────
// Infrastruttura comune
// ─────────────────────────────────────────────────────────────────────────────

fn temp_root(tag: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("lume_hunk_{tag}_{nonce}"));
    fs::create_dir_all(&root).unwrap();
    root
}

fn signature() -> gix::actor::SignatureRef<'static> {
    gix::actor::SignatureRef::from_bytes(b"Lume Test <lume@example.test> 1700000000 +0100").unwrap()
}

fn commit_files(
    repository: &gix::Repository,
    root: &Path,
    files: &[(&str, &[u8])],
    parents: Vec<gix::ObjectId>,
    message: &str,
) -> gix::ObjectId {
    let mut entries = Vec::new();
    for (name, content) in files {
        fs::write(root.join(name), content).unwrap();
        let blob = repository.write_blob(content).unwrap().detach();
        entries.push(gix::objs::tree::Entry {
            mode: gix::objs::tree::EntryKind::Blob.into(),
            filename: (*name).into(),
            oid: blob,
        });
    }
    entries.sort_by(|a, b| a.filename.cmp(&b.filename));
    let tree = repository
        .write_object(gix::objs::Tree { entries })
        .unwrap()
        .detach();
    repository
        .commit_as(signature(), signature(), "HEAD", message, tree, parents)
        .unwrap()
        .detach()
}

// ─────────────────────────────────────────────────────────────────────────────
// Test
// ─────────────────────────────────────────────────────────────────────────────

/// Staging dell'unico hunk di una modifica semplice: dopo `stage_hunk` il
/// blob nell'index deve contenere la versione «staged», verificata tramite
/// commit e lettura dell'albero.
///
/// Nota: gix usa un index VUOTO su repo senza file index (commit diretti, come
/// in questo test), quindi `file.index = Some(Removed)` è il comportamento
/// atteso PRIMA dello staging; si verifica solo lo stato POST-staging.
#[test]
fn stage_hunk_singolo_aggiorna_index() {
    let root = temp_root("singolo");
    let repository = gix::init(&root).unwrap();
    let cli = lume_git::GitCli::locate().expect("git è richiesto da questo test");
    let cancelled = std::sync::atomic::AtomicBool::new(false);
    for args in [
        ["config", "user.name", "Lume Test"],
        ["config", "user.email", "lume@example.test"],
    ] {
        let output = cli.run(&root, &args, &cancelled).unwrap();
        assert!(output.success(), "{}", output.failure_text());
    }
    commit_files(
        &repository,
        &root,
        &[("a.txt", b"uno\ndue\ntre\n")],
        Vec::new(),
        "primo",
    );
    // Modifica una riga nel worktree.
    fs::write(root.join("a.txt"), b"uno\nDUE\ntre\n").unwrap();

    let git = GitRepository::discover(&root).unwrap();

    // Calcola il diff e prende l'unico hunk.
    let diff = git.diff_worktree_file(Path::new("a.txt")).unwrap();
    assert_eq!(diff.hunks.len(), 1, "deve esserci esattamente un hunk");
    let hunk = &diff.hunks[0];
    assert_eq!(hunk.old_lines, vec!["due"]);
    assert_eq!(hunk.new_lines, vec!["DUE"]);

    // Mette in stage il hunk.
    git.stage_hunk(Path::new("a.txt"), hunk).unwrap();

    // Dopo lo stage: l'index è cambiato rispetto a HEAD (la modifica è in stage).
    let status = git.status().unwrap();
    let file = status
        .iter()
        .find(|f| f.path == Path::new("a.txt"))
        .unwrap();
    assert_eq!(
        file.index,
        Some(ChangeKind::Modified),
        "l'index deve riflettere la modifica rispetto a HEAD"
    );
    assert!(file.is_staged(), "il file deve essere in stage");
    // Con un solo hunk completamente in stage, il file non è parzialmente in stage.
    assert!(
        !file.is_partially_staged(),
        "con un singolo hunk completamente in stage non è parziale"
    );

    // «Locali» confronta index↔worktree: messo in stage l'unico hunk, qui non
    // resta piu' niente da vedere. (Fino a r9 questo diff era HEAD↔worktree e
    // continuava a mostrare (1,1) anche dopo lo stage: era il difetto che
    // faceva ricomparire il pulsante «metti in stage» su un hunk gia' in
    // stage, dove poi falliva.)
    let diff_after = git.diff_worktree_file(Path::new("a.txt")).unwrap();
    assert_eq!(
        (diff_after.added, diff_after.removed),
        (0, 0),
        "tutto in stage: «Locali» non ha piu' differenze da mostrare"
    );

    // E la modifica si vede nell'altra vista, quella dell'index.
    let in_stage = git.diff_index_file(Path::new("a.txt")).unwrap();
    assert_eq!(
        (in_stage.added, in_stage.removed),
        (1, 1),
        "la riga cambiata e' in stage: la mostra la vista «In stage»"
    );

    // Verifica il contenuto in stage committando e leggendo il blob dall'albero.
    let committed = git
        .commit("verifica stage hunk")
        .expect("il commit deve riuscire con la modifica in stage");
    let repo = gix::discover(&root).unwrap();
    let commit_id = gix::ObjectId::from_hex(committed.as_bytes()).unwrap();
    let tree = repo.find_commit(commit_id).unwrap().tree().unwrap();
    let entry = tree
        .lookup_entry_by_path("a.txt")
        .unwrap()
        .expect("a.txt deve essere nel commit");
    let blob = repo.find_object(entry.object_id()).unwrap();
    assert_eq!(
        blob.data.as_slice(),
        b"uno\nDUE\ntre\n",
        "il blob committato deve avere il contenuto della versione staged"
    );

    fs::remove_dir_all(root).unwrap();
}

/// Staging parziale: con due hunk distanti, mettendo in stage solo il primo
/// il file deve risultare *parzialmente* in stage.
#[test]
fn stage_primo_hunk_di_due_produce_staging_parziale() {
    let root = temp_root("parziale");
    let repository = gix::init(&root).unwrap();
    // File con 20 righe per avere due hunk distanti.
    let before: String = (1u32..=20).map(|i| format!("riga {i}\n")).collect();
    commit_files(
        &repository,
        &root,
        &[("lungo.txt", before.as_bytes())],
        Vec::new(),
        "primo",
    );
    // Modifica riga 2 e riga 19: due hunk separati.
    let after = before
        .replace("riga 2\n", "RIGA 2\n")
        .replace("riga 19\n", "RIGA 19\n");
    fs::write(root.join("lungo.txt"), after.as_bytes()).unwrap();

    let git = GitRepository::discover(&root).unwrap();
    let diff = git.diff_worktree_file(Path::new("lungo.txt")).unwrap();
    assert_eq!(diff.hunks.len(), 2, "due modifiche distanti → due hunk");

    // Mette in stage solo il primo hunk.
    git.stage_hunk(Path::new("lungo.txt"), &diff.hunks[0])
        .unwrap();

    // Il file è parzialmente in stage: index ha il primo hunk, worktree ne ha un altro.
    let status = git.status().unwrap();
    let file = status
        .iter()
        .find(|f| f.path == Path::new("lungo.txt"))
        .unwrap();
    assert!(file.is_staged(), "almeno un hunk in stage");
    assert!(
        file.is_partially_staged(),
        "deve essere parzialmente in stage: index ha hunk 1, worktree ha hunk 2"
    );

    // Verifica che l'index contenga la modifica giusta: diff index→worktree
    // deve ancora avere un hunk (riga 19).
    let diff_after = git.diff_worktree_file(Path::new("lungo.txt")).unwrap();
    assert!(
        diff_after.added > 0 || diff_after.removed > 0,
        "il secondo hunk è ancora non in stage"
    );

    fs::remove_dir_all(root).unwrap();
}

/// `unstage_hunk` riporta l'index allo stato HEAD per l'hunk scelto.
#[test]
fn unstage_hunk_ripristina_index_per_quel_hunk() {
    let root = temp_root("unstage");
    let repository = gix::init(&root).unwrap();
    commit_files(
        &repository,
        &root,
        &[("b.txt", b"alfa\nbeta\ngamma\n")],
        Vec::new(),
        "primo",
    );
    // Modifica il worktree.
    fs::write(root.join("b.txt"), b"alfa\nBETA\ngamma\n").unwrap();

    let git = GitRepository::discover(&root).unwrap();
    let diff = git.diff_worktree_file(Path::new("b.txt")).unwrap();
    assert_eq!(diff.hunks.len(), 1);

    // Prima mette in stage.
    git.stage_hunk(Path::new("b.txt"), &diff.hunks[0]).unwrap();
    let status = git.status().unwrap();
    assert!(
        status
            .iter()
            .find(|f| f.path == Path::new("b.txt"))
            .unwrap()
            .is_staged(),
        "deve essere in stage prima di unstage"
    );

    // Poi fa unstage dell'hunk: torna tutto come prima.
    git.unstage_hunk(Path::new("b.txt"), &diff.hunks[0])
        .unwrap();
    let status = git.status().unwrap();
    let file = status
        .iter()
        .find(|f| f.path == Path::new("b.txt"))
        .unwrap();
    assert_eq!(
        file.index, None,
        "dopo unstage dell'unico hunk, niente in stage"
    );
    assert_eq!(file.worktree, Some(ChangeKind::Modified));
    assert!(!file.is_partially_staged());

    fs::remove_dir_all(root).unwrap();
}

/// Staging di un hunk che aggiunge righe in fondo (inserimento puro).
#[test]
fn stage_hunk_inserimento_puro_in_fondo() {
    let root = temp_root("inserimento");
    let repository = gix::init(&root).unwrap();
    commit_files(
        &repository,
        &root,
        &[("c.txt", b"prima\nseconda\n")],
        Vec::new(),
        "primo",
    );
    // Aggiunge due righe in fondo.
    fs::write(root.join("c.txt"), b"prima\nseconda\nterza\nquarta\n").unwrap();

    let git = GitRepository::discover(&root).unwrap();
    let diff = git.diff_worktree_file(Path::new("c.txt")).unwrap();
    assert_eq!((diff.added, diff.removed), (2, 0));
    assert_eq!(diff.hunks.len(), 1);

    // Verifica che le righe dell'hunk siano corrette.
    let hunk = &diff.hunks[0];
    assert!(
        hunk.old_lines.is_empty(),
        "inserimento puro: nessuna riga rimossa"
    );
    assert_eq!(hunk.new_lines, vec!["terza", "quarta"]);

    // Verifica le sign delle righe nel diff.
    let added_rows: Vec<_> = diff
        .rows
        .iter()
        .filter(|r| {
            matches!(
                r,
                lume_git::DiffRow::Line {
                    sign: DiffSign::Added,
                    ..
                }
            )
        })
        .collect();
    assert_eq!(added_rows.len(), 2);

    git.stage_hunk(Path::new("c.txt"), hunk).unwrap();

    let status = git.status().unwrap();
    let file = status
        .iter()
        .find(|f| f.path == Path::new("c.txt"))
        .unwrap();
    assert!(file.is_staged());
    assert!(!file.is_partially_staged());

    fs::remove_dir_all(root).unwrap();
}

/// `hunk_idx` nelle righe del diff punta all'hunk corretto di `FileDiff::hunks`.
#[test]
fn hunk_idx_nelle_righe_coincide_con_hunks_del_diff() {
    let root = temp_root("hunk_idx");
    let repository = gix::init(&root).unwrap();
    let before: String = (1u32..=30).map(|i| format!("r{i}\n")).collect();
    commit_files(
        &repository,
        &root,
        &[("d.txt", before.as_bytes())],
        Vec::new(),
        "primo",
    );
    let after = before.replace("r5\n", "R5\n").replace("r25\n", "R25\n");
    fs::write(root.join("d.txt"), after.as_bytes()).unwrap();

    let git = GitRepository::discover(&root).unwrap();
    let diff = git.diff_worktree_file(Path::new("d.txt")).unwrap();
    assert_eq!(diff.hunks.len(), 2);

    // Ogni riga Added/Removed deve avere hunk_idx che punta a un hunk valido.
    for row in &diff.rows {
        if let lume_git::DiffRow::Line { sign, hunk_idx, .. } = row {
            match sign {
                DiffSign::Added | DiffSign::Removed => {
                    let idx = hunk_idx.expect("Added/Removed deve avere hunk_idx");
                    assert!(idx < diff.hunks.len(), "hunk_idx fuori dai limiti");
                }
                DiffSign::Context => {
                    assert!(
                        hunk_idx.is_none(),
                        "le righe di contesto non hanno hunk_idx"
                    );
                }
            }
        }
    }

    // Le righe con lo stesso hunk_idx appartengono allo stesso hunk.
    let idx_0_rows: Vec<_> = diff
        .rows
        .iter()
        .filter(|r| {
            matches!(
                r,
                lume_git::DiffRow::Line {
                    hunk_idx: Some(0),
                    ..
                }
            )
        })
        .collect();
    assert!(!idx_0_rows.is_empty(), "ci sono righe nell'hunk 0");

    let idx_1_rows: Vec<_> = diff
        .rows
        .iter()
        .filter(|r| {
            matches!(
                r,
                lume_git::DiffRow::Line {
                    hunk_idx: Some(1),
                    ..
                }
            )
        })
        .collect();
    assert!(!idx_1_rows.is_empty(), "ci sono righe nell'hunk 1");

    fs::remove_dir_all(root).unwrap();
}

// ─────────────────────────────────────────────────────────────────────────────
// Deriva degli offset fra hunk (il motivo per cui «Locali» confronta l'index)
// ─────────────────────────────────────────────────────────────────────────────

/// Contenuto del blob registrato nell'index per `name`.
///
/// I test che seguono guardano **i byte nell'index**, non `status()`: un hunk
/// applicato nel punto sbagliato produce comunque uno stato «parzialmente in
/// stage» perfettamente plausibile, ed e' esattamente cosi' che il difetto
/// resterebbe invisibile.
fn contenuto_nell_index(root: &Path, name: &str) -> Vec<u8> {
    let repository = gix::discover(root).unwrap();
    let index = repository.index_or_load_from_head_or_empty().unwrap();
    let entry = index
        .entry_by_path(gix::bstr::BStr::new(name.as_bytes()))
        .expect("il file deve avere un entry nell'index");
    repository.find_object(entry.id).unwrap().data.to_vec()
}

/// Mettere in stage gli hunk **uno alla volta** deve portare l'index a
/// coincidere col worktree, anche quando il primo hunk sposta il file di piu'
/// righe di quante `find_hunk_position` ne cerchi intorno (raggio 8).
///
/// `stage_hunk` applica l'hunk al blob dell'**index** usando `old_range.start`.
/// Finche' «Locali» confrontava HEAD col worktree, quell'offset era relativo a
/// HEAD: bastava che il primo hunk aggiungesse piu' di 8 righe perche' il
/// secondo non si trovasse piu' e l'operazione fallisse. Con la base index il
/// problema non esiste per costruzione — l'offset e' gia' nelle coordinate del
/// contenuto su cui viene applicato.
#[test]
fn stage_del_secondo_hunk_dopo_il_primo_non_va_alla_deriva() {
    let root = temp_root("deriva");
    let repository = gix::init(&root).unwrap();
    let before: String = (1u32..=40).map(|i| format!("riga {i}\n")).collect();
    commit_files(
        &repository,
        &root,
        &[("lungo.txt", before.as_bytes())],
        Vec::new(),
        "primo",
    );

    // Il primo hunk gonfia il file di 11 righe: ben oltre il raggio di ricerca.
    let gonfio: String = (1u32..=12).map(|i| format!("nuova 2.{i}\n")).collect();
    let after = before
        .replace("riga 2\n", &gonfio)
        .replace("riga 38\n", "RIGA 38\n");
    fs::write(root.join("lungo.txt"), after.as_bytes()).unwrap();

    let git = GitRepository::discover(&root).unwrap();
    let diff = git.diff_worktree_file(Path::new("lungo.txt")).unwrap();
    assert_eq!(diff.hunks.len(), 2, "due modifiche distanti → due hunk");

    git.stage_hunk(Path::new("lungo.txt"), &diff.hunks[0])
        .unwrap();

    // Il secondo giro: si ricalcola e si mette in stage quel che resta.
    let residuo = git.diff_worktree_file(Path::new("lungo.txt")).unwrap();
    assert_eq!(
        residuo.hunks.len(),
        1,
        "dopo lo stage del primo hunk «Locali» deve mostrare solo il secondo, \
         non entrambi: e' il confronto con l'index"
    );
    git.stage_hunk(Path::new("lungo.txt"), &residuo.hunks[0])
        .expect("il secondo hunk deve andare in stage senza deriva");

    assert_eq!(
        contenuto_nell_index(&root, "lungo.txt"),
        after.as_bytes(),
        "messi in stage tutti gli hunk, l'index deve coincidere col worktree"
    );

    fs::remove_dir_all(root).unwrap();
}

/// Un hunk di **puro inserimento** (nessuna riga rimossa) messo in stage dopo
/// un altro hunk deve finire nel punto giusto.
///
/// E' il caso peggiore: con `old_lines` vuoto `find_hunk_position` non ha
/// niente da confrontare e accetta la posizione attesa cosi' com'e'. Se
/// l'offset e' sbagliato non c'e' errore — le righe entrano nel punto sbagliato
/// e l'index sembra a posto. Per questo l'asserzione e' sui byte.
#[test]
fn inserimento_puro_dopo_uno_stage_finisce_al_posto_giusto() {
    let root = temp_root("inserimento");
    let repository = gix::init(&root).unwrap();
    let before: String = (1u32..=40).map(|i| format!("riga {i}\n")).collect();
    commit_files(
        &repository,
        &root,
        &[("lungo.txt", before.as_bytes())],
        Vec::new(),
        "primo",
    );

    let gonfio: String = (1u32..=12).map(|i| format!("nuova 2.{i}\n")).collect();
    // Secondo hunk: inserimento puro dopo «riga 38», niente rimozioni.
    let after = before
        .replace("riga 2\n", &gonfio)
        .replace("riga 38\n", "riga 38\nINSERITA A\nINSERITA B\n");
    fs::write(root.join("lungo.txt"), after.as_bytes()).unwrap();

    let git = GitRepository::discover(&root).unwrap();
    let diff = git.diff_worktree_file(Path::new("lungo.txt")).unwrap();
    assert_eq!(diff.hunks.len(), 2);

    git.stage_hunk(Path::new("lungo.txt"), &diff.hunks[0])
        .unwrap();
    let residuo = git.diff_worktree_file(Path::new("lungo.txt")).unwrap();
    assert_eq!(residuo.hunks.len(), 1);
    git.stage_hunk(Path::new("lungo.txt"), &residuo.hunks[0])
        .unwrap();

    assert_eq!(
        contenuto_nell_index(&root, "lungo.txt"),
        after.as_bytes(),
        "l'inserimento puro e' finito nel punto sbagliato dell'index"
    );

    fs::remove_dir_all(root).unwrap();
}
