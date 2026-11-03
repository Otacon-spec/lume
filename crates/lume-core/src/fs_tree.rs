//! Albero directory LAZY per il pannello Progetto.
//!
//! I contenuti di una directory vengono letti da disco solo alla prima
//! espansione (`children: None` = non ancora listata). Scelta implementativa
//! per il filtro ignore: `ignore::WalkBuilder` con `max_depth(1)` rooted
//! sulla directory da listare, invece di un `GitignoreBuilder` globale sul
//! root. Motivo: l'opzione `parents` (attiva di default) fa risalire il
//! walker fino ai `.gitignore` dei genitori, così anche i `.gitignore`
//! annidati nelle sottocartelle vengono rispettati senza dover ricostruire
//! un matcher a ogni espansione. `require_git(false)` applica le regole
//! anche fuori da un repository git; `.git` è sempre esclusa a prescindere.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

/// Nodo dell'albero: file o directory.
#[derive(Debug, Clone)]
pub struct FsNode {
    pub name: String,
    pub is_dir: bool,
    pub expanded: bool,
    /// `None` = directory non ancora letta da disco (lazy).
    /// Per i file è sempre `None`.
    pub children: Option<Vec<PathBuf>>,
}

/// Riga visibile del pannello Progetto, prodotta dal DFS dei nodi espansi.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowSpec {
    pub depth: usize,
    pub path: PathBuf,
    pub is_dir: bool,
    pub expanded: bool,
    pub name: String,
}

/// Albero directory lazy. La radice compare in [`FsTree::visible_rows`] a
/// depth 0 con il nome della cartella root (come il prototipo «▾ forge»).
#[derive(Debug)]
pub struct FsTree {
    root: PathBuf,
    nodes: HashMap<PathBuf, FsNode>,
}

impl FsTree {
    /// Crea l'albero leggendo SOLO il primo livello; la root parte espansa.
    pub fn new(root: PathBuf) -> Self {
        let name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.display().to_string());
        let mut tree = FsTree {
            root: root.clone(),
            nodes: HashMap::new(),
        };
        tree.nodes.insert(
            root.clone(),
            FsNode {
                name,
                is_dir: true,
                expanded: true,
                children: None,
            },
        );
        let listed = Self::list_dir(&root);
        tree.store_children(&root, listed);
        tree
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Expand/collapse di una directory; alla prima espansione fa la
    /// `read_dir` lazy. No-op su file o path sconosciuti.
    pub fn toggle(&mut self, dir: &Path) {
        let Some(node) = self.nodes.get(dir) else {
            return;
        };
        if !node.is_dir {
            return;
        }
        let was_expanded = node.expanded;
        let was_loaded = node.children.is_some();
        if !was_expanded {
            if was_loaded {
                self.refresh(dir);
            } else {
                let listed = Self::list_dir(dir);
                self.store_children(dir, listed);
            }
        }
        if let Some(node) = self.nodes.get_mut(dir) {
            node.expanded = !was_expanded;
        }
    }

    /// Ri-lista `changed_parent` SOLO se era già stata caricata; preserva lo
    /// stato `expanded` (e i figli già letti) dei nodi che sopravvivono.
    /// I nodi spariti dal disco vengono rimossi ricorsivamente.
    pub fn refresh(&mut self, changed_parent: &Path) {
        let Some(node) = self.nodes.get(changed_parent) else {
            return;
        };
        let Some(old_children) = node.children.clone() else {
            return; // mai letta: verrà listata alla prima espansione
        };
        let listed = Self::list_dir(changed_parent);
        for stale in old_children {
            if !listed.iter().any(|(p, _, _)| p == &stale) {
                self.remove_recursive(&stale);
            }
        }
        self.store_children(changed_parent, listed);
    }

    /// DFS dei nodi espansi. La root compare a depth 0.
    pub fn visible_rows(&self) -> Vec<RowSpec> {
        let mut rows = Vec::new();
        self.push_rows(&self.root.clone(), 0, &mut rows);
        rows
    }

    fn push_rows(&self, path: &Path, depth: usize, rows: &mut Vec<RowSpec>) {
        let Some(node) = self.nodes.get(path) else {
            return;
        };
        rows.push(RowSpec {
            depth,
            path: path.to_path_buf(),
            is_dir: node.is_dir,
            expanded: node.expanded,
            name: node.name.clone(),
        });
        if node.is_dir
            && node.expanded
            && let Some(children) = &node.children
        {
            for child in children {
                self.push_rows(child, depth + 1, rows);
            }
        }
    }

    /// Lista un singolo livello di `dir`, filtrando le entry ignorate stile
    /// .gitignore e le cartelle dei sistemi di versionamento (`.git`, `.jj`).
    /// Ordina: directory prima, poi file, nomi case-insensitive.
    fn list_dir(dir: &Path) -> Vec<(PathBuf, String, bool)> {
        let walker = WalkBuilder::new(dir)
            .max_depth(Some(1))
            .hidden(false) // mostra i dotfile (es. .gitignore stesso)
            .require_git(false) // applica i .gitignore anche fuori da un repo
            .filter_entry(|e| e.file_name() != ".git" && e.file_name() != ".jj")
            .build();
        let mut out: Vec<(PathBuf, String, bool)> = walker
            .flatten()
            .filter(|entry| entry.depth() > 0) // salta la dir stessa
            .map(|entry| {
                let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                let name = entry.file_name().to_string_lossy().into_owned();
                (entry.into_path(), name, is_dir)
            })
            .collect();
        out.sort_by(|a, b| {
            // is_dir: true prima di false, poi nome case-insensitive
            b.2.cmp(&a.2)
                .then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase()))
        });
        out
    }

    /// Registra i figli listati sul nodo `dir`. I nodi già presenti NON
    /// vengono sovrascritti: così `refresh` preserva `expanded` e i figli
    /// già caricati.
    fn store_children(&mut self, dir: &Path, listed: Vec<(PathBuf, String, bool)>) {
        let mut paths = Vec::with_capacity(listed.len());
        for (path, name, is_dir) in listed {
            self.nodes.entry(path.clone()).or_insert(FsNode {
                name,
                is_dir,
                expanded: false,
                children: None,
            });
            paths.push(path);
        }
        if let Some(node) = self.nodes.get_mut(dir) {
            node.children = Some(paths);
        }
    }

    fn remove_recursive(&mut self, path: &Path) {
        if let Some(node) = self.nodes.remove(path)
            && let Some(children) = node.children
        {
            for child in children {
                self.remove_recursive(&child);
            }
        }
    }
}

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

    #[test]
    fn lazy_primo_livello_toggle_e_gitignore() {
        let root = make_tempdir("fstree");
        fs::create_dir(root.join("src")).unwrap();
        fs::create_dir(root.join("b_dir")).unwrap();
        fs::write(root.join("src").join("main.rs"), "fn main() {}").unwrap();
        fs::write(root.join("a.txt"), "a").unwrap();
        fs::write(root.join("ignored.txt"), "x").unwrap();
        fs::write(root.join(".gitignore"), "ignored.txt\n").unwrap();

        let mut tree = FsTree::new(root.clone());
        assert_eq!(tree.root(), root.as_path());

        let rows = tree.visible_rows();
        // riga 0: la root, a depth 0, espansa
        assert_eq!(rows[0].depth, 0);
        assert!(rows[0].is_dir);
        assert!(rows[0].expanded);
        // primo livello: directory prima (b_dir, src), poi i file in ordine
        // case-insensitive; il file ignorato è assente
        let names: Vec<&str> = rows[1..].iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["b_dir", "src", ".gitignore", "a.txt"]);
        assert!(rows[1..].iter().all(|r| r.depth == 1));
        // src non ancora espansa: contenuto non letto (lazy)
        assert!(!rows.iter().any(|r| r.name == "main.rs"));

        // toggle espande e legge la dir alla prima espansione
        tree.toggle(&root.join("src"));
        let rows = tree.visible_rows();
        let main = rows
            .iter()
            .find(|r| r.name == "main.rs")
            .expect("main.rs visibile dopo il toggle");
        assert_eq!(main.depth, 2);
        assert!(!main.is_dir);

        // secondo toggle: collapse
        tree.toggle(&root.join("src"));
        assert!(!tree.visible_rows().iter().any(|r| r.name == "main.rs"));

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn la_cartella_jj_non_compare_nell_albero() {
        let root = make_tempdir("fstree_jj");
        fs::create_dir_all(root.join(".jj").join("repo")).unwrap();
        fs::write(root.join(".jj").join("repo").join("store.txt"), "x").unwrap();
        fs::write(root.join("a.txt"), "a").unwrap();
        let tree = FsTree::new(root.clone());
        let rows = tree.visible_rows();
        let names: Vec<&str> = rows[1..].iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["a.txt"]);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn riapertura_rilegge_le_modifiche_avvenute_da_collassato() {
        let root = make_tempdir("fstree_reexpand");
        let src = root.join("src");
        fs::create_dir(&src).unwrap();
        fs::write(src.join("before.rs"), "prima").unwrap();

        let mut tree = FsTree::new(root.clone());
        tree.toggle(&src);
        assert!(
            tree.visible_rows()
                .iter()
                .any(|row| row.name == "before.rs")
        );

        tree.toggle(&src);
        fs::remove_file(src.join("before.rs")).unwrap();
        fs::write(src.join("after.rs"), "dopo").unwrap();
        tree.toggle(&src);

        let rows = tree.visible_rows();
        assert!(rows.iter().any(|row| row.name == "after.rs"));
        assert!(!rows.iter().any(|row| row.name == "before.rs"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refresh_preserva_expanded_dei_figli() {
        let root = make_tempdir("fstree_refresh");
        fs::create_dir(root.join("src")).unwrap();
        fs::write(root.join("src").join("lib.rs"), "").unwrap();

        let mut tree = FsTree::new(root.clone());
        tree.toggle(&root.join("src")); // espande src

        // nuovo file su disco + refresh del parent già caricato
        fs::write(root.join("nuovo.txt"), "n").unwrap();
        tree.refresh(&root);
        let rows = tree.visible_rows();
        assert!(rows.iter().any(|r| r.name == "nuovo.txt"));
        // src è rimasta espansa e i suoi figli sono ancora visibili
        assert!(rows.iter().any(|r| r.name == "lib.rs"));

        // refresh di una dir mai caricata: no-op (nessun panico, nessuna lettura)
        tree.refresh(&root.join("inesistente"));

        fs::remove_dir_all(&root).unwrap();
    }
}
