use std::cell::RefCell;
use std::path::{Path, PathBuf};

use lume_icons::{
    FileIconContext, IconColorMode, IconThemeProvider, IconThemeRegistry, ManifestIconTheme,
};
use lume_ui::file_icons::{FileIcon, FileIconSource, IconAssetCache};

const MAX_EXTERNAL_PACKS: usize = 128;

#[derive(Debug, Clone)]
pub struct IconThemePreview {
    pub folder: FileIcon,
    pub folder_open: FileIcon,
    pub java: FileIcon,
    pub generic_file: FileIcon,
}

#[derive(Debug, Clone)]
pub struct IconThemeRow {
    pub id: String,
    pub name: String,
    pub preview: Option<IconThemePreview>,
    pub error: Option<String>,
    pub is_active: bool,
    pub is_preferred: bool,
}

#[derive(Debug, Clone)]
struct UnavailableIconTheme {
    id: String,
    name: String,
    error: String,
}

pub struct IconThemeState {
    registry: IconThemeRegistry,
    cache: RefCell<IconAssetCache>,
    preferred_id: String,
    external_root: Option<PathBuf>,
    unavailable: Vec<UnavailableIconTheme>,
    /// Vero dopo la prima scansione completa dei pacchetti esterni.
    scanned: bool,
    /// Solo test: scansioni complete e accessi alla cartella dei pacchetti.
    #[cfg(test)]
    scan_runs: u32,
    #[cfg(test)]
    fs_accesses: u32,
}

impl Default for IconThemeState {
    fn default() -> Self {
        Self::from_config_dir(lume_store::application_config_dir())
    }
}

impl IconThemeState {
    /// Stato all'avvio: registro incorporato e nessuna enumerazione dei
    /// pacchetti esterni (si fa su richiesta, vedi `scan_external_once`).
    pub(crate) fn from_config_dir(config_dir: Option<PathBuf>) -> Self {
        Self::new(config_dir.map(|directory| directory.join("icon-themes")))
    }

    pub(crate) fn new(external_root: Option<PathBuf>) -> Self {
        Self {
            registry: IconThemeRegistry::with_builtins(),
            cache: RefCell::new(IconAssetCache::default()),
            preferred_id: "material".to_owned(),
            external_root,
            unavailable: Vec::new(),
            scanned: false,
            #[cfg(test)]
            scan_runs: 0,
            #[cfg(test)]
            fs_accesses: 0,
        }
    }

    pub fn preferred_id(&self) -> &str {
        &self.preferred_id
    }

    pub fn active_id(&self) -> &str {
        self.registry.active_id()
    }

    pub fn apply_preference(&mut self, preferred_id: impl Into<String>) {
        self.preferred_id = preferred_id.into();
        if self.registry.activate(&self.preferred_id).is_err() {
            // Ripristino mirato: si carica solo il pacchetto salvato, non tutti.
            let preferred_id = self.preferred_id.clone();
            if self.restore_external(&preferred_id)
                && self.registry.activate(&self.preferred_id).is_ok()
            {
                return;
            }
            self.registry.recover();
            if !self
                .unavailable
                .iter()
                .any(|theme| theme.id == self.preferred_id)
            {
                self.unavailable.push(UnavailableIconTheme {
                    id: self.preferred_id.clone(),
                    name: self.preferred_id.clone(),
                    error: "pacchetto preferito non trovato".to_owned(),
                });
            }
        }
    }

    pub fn select(&mut self, provider_id: &str) -> bool {
        if self.registry.activate(provider_id).is_ok() {
            self.preferred_id = provider_id.to_owned();
            true
        } else {
            false
        }
    }

    pub fn rescan(&mut self) {
        let preferred_id = self.preferred_id.clone();
        self.registry = IconThemeRegistry::with_builtins();
        self.cache = RefCell::new(IconAssetCache::default());
        self.unavailable.clear();
        self.scan_external();
        self.apply_preference(preferred_id);
    }

    pub fn rows(&self) -> Vec<IconThemeRow> {
        let mut rows = self
            .registry
            .providers()
            .map(|provider| {
                let preview = self.preview(provider);
                IconThemeRow {
                    id: provider.metadata().id.clone(),
                    name: provider.metadata().name.clone(),
                    error: preview.as_ref().err().cloned(),
                    preview: preview.ok(),
                    is_active: provider.metadata().id == self.active_id(),
                    is_preferred: provider.metadata().id == self.preferred_id,
                }
            })
            .collect::<Vec<_>>();
        let mut unavailable = self
            .unavailable
            .iter()
            .map(|unavailable| IconThemeRow {
                id: unavailable.id.clone(),
                name: unavailable.name.clone(),
                preview: None,
                error: Some(unavailable.error.clone()),
                is_active: false,
                is_preferred: unavailable.id == self.preferred_id,
            })
            .collect::<Vec<_>>();
        unavailable.sort_by(|left, right| {
            left.name
                .to_lowercase()
                .cmp(&right.name.to_lowercase())
                .then_with(|| left.id.cmp(&right.id))
        });
        rows.extend(unavailable);
        rows
    }

    #[allow(clippy::indexing_slicing)] // handles ha esattamente 4 elementi (preview_asset_ids)
    fn preview(&self, provider: &dyn IconThemeProvider) -> Result<IconThemePreview, String> {
        let contexts = [
            FileIconContext::folder("src", false),
            FileIconContext::folder("src", true),
            FileIconContext::file("Main.java"),
            FileIconContext::file("unknown.lume-no-association"),
        ];
        let ids = contexts.map(|context| provider.resolve(&context));
        let mut cache = self.cache.borrow_mut();
        let handles = ids
            .each_ref()
            .map(|id| cache.resolved_icon(provider, id, IconColorMode::Dark, 1))
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        Ok(IconThemePreview {
            folder: handles[0].clone(),
            folder_open: handles[1].clone(),
            java: handles[2].clone(),
            generic_file: handles[3].clone(),
        })
    }

    /// Scansione completa dei pacchetti esterni, una sola volta: aggiorna le
    /// righe del selettore e non tocca mai preferenza né tema attivo. Le
    /// chiamate successive non fanno nulla (per riscansionare c'è `rescan`).
    pub fn scan_external_once(&mut self) {
        if self.scanned {
            return;
        }
        self.scan_external();
    }

    fn mark_unavailable(&mut self, id: String, name: String, error: String) {
        self.unavailable.retain(|existing| existing.id != id);
        self.unavailable
            .push(UnavailableIconTheme { id, name, error });
    }

    /// Radice dei pacchetti e la sua forma canonica, se la cartella esiste.
    fn canonical_external_root(&mut self) -> Option<(PathBuf, PathBuf)> {
        let root = self.external_root.clone()?;
        if !root.is_dir() {
            return None;
        }
        match root.canonicalize() {
            Ok(canonical) => Some((root, canonical)),
            Err(error) => {
                self.mark_unavailable(
                    "external:scan".to_owned(),
                    "Pacchetti esterni".to_owned(),
                    error.to_string(),
                );
                None
            }
        }
    }

    /// Carica un solo pacchetto con lo stesso confinamento alla radice e la
    /// stessa validazione della scansione completa. Un pacchetto già
    /// registrato non viene registrato due volte.
    fn load_pack(&mut self, canonical_root: &Path, entry: &std::fs::DirEntry) {
        let path = entry.path();
        if !path.is_dir() {
            return;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let id = external_id(&name);
        if self.registry.provider(&id).is_some() {
            return;
        }
        let canonical_pack = match path.canonicalize() {
            Ok(path) if path.starts_with(canonical_root) => path,
            Ok(path) => {
                let error = format!("pack path escapes icon-themes root: {}", path.display());
                self.mark_unavailable(id, name, error);
                return;
            }
            Err(error) => {
                self.mark_unavailable(id, name, error.to_string());
                return;
            }
        };
        match ManifestIconTheme::load(&canonical_pack, Path::new("icon-theme.json")) {
            Ok(provider) => match self.registry.register(provider) {
                Ok(()) => self.unavailable.retain(|existing| existing.id != id),
                Err(error) => self.mark_unavailable(id, name, error.to_string()),
            },
            Err(error) => self.mark_unavailable(id, name, error.to_string()),
        }
    }

    /// Ripristino mirato all'avvio: se l'ID salvato è esterno si cerca la sua
    /// cartella (solo i nomi, nessun manifest degli altri pacchetti) e si
    /// carica quella. Dopo la scansione completa non serve più.
    fn restore_external(&mut self, id: &str) -> bool {
        if self.scanned || !id.starts_with("external:") {
            return false;
        }
        let Some((root, canonical_root)) = self.canonical_external_root() else {
            return false;
        };
        #[cfg(test)]
        {
            self.fs_accesses += 1;
        }
        let Ok(entries) = std::fs::read_dir(&root) else {
            return false;
        };
        let found = entries
            .filter_map(Result::ok)
            .take(MAX_EXTERNAL_PACKS)
            .find(|entry| external_id(&entry.file_name().to_string_lossy()) == id);
        match found {
            Some(entry) => {
                self.load_pack(&canonical_root, &entry);
                true
            }
            None => false,
        }
    }

    fn scan_external(&mut self) {
        self.scanned = true;
        #[cfg(test)]
        {
            self.scan_runs += 1;
        }
        let Some((root, canonical_root)) = self.canonical_external_root() else {
            return;
        };
        #[cfg(test)]
        {
            self.fs_accesses += 1;
        }
        let mut directories = match std::fs::read_dir(&root) {
            Ok(entries) => entries
                .filter_map(Result::ok)
                .take(MAX_EXTERNAL_PACKS + 1)
                .collect::<Vec<_>>(),
            Err(error) => {
                self.mark_unavailable(
                    "external:scan".to_owned(),
                    "Pacchetti esterni".to_owned(),
                    error.to_string(),
                );
                return;
            }
        };
        if directories.len() > MAX_EXTERNAL_PACKS {
            directories.truncate(MAX_EXTERNAL_PACKS);
            self.mark_unavailable(
                "external:pack-limit".to_owned(),
                "Limite pacchetti esterni".to_owned(),
                format!("sono supportati al massimo {MAX_EXTERNAL_PACKS} pacchetti"),
            );
        }
        directories.sort_by_key(std::fs::DirEntry::file_name);
        for entry in directories {
            self.load_pack(&canonical_root, &entry);
        }
    }

    #[cfg(test)]
    pub(crate) fn scan_runs(&self) -> u32 {
        self.scan_runs
    }

    #[cfg(test)]
    fn builtins_only_for_test() -> Self {
        Self::new(None)
    }

    #[cfg(test)]
    fn add_unavailable_for_test(&mut self, id: &str, name: &str, error: &str) {
        self.unavailable.push(UnavailableIconTheme {
            id: id.to_owned(),
            name: name.to_owned(),
            error: error.to_owned(),
        });
    }
}

impl FileIconSource for IconThemeState {
    fn icon(&self, context: &FileIconContext) -> Result<FileIcon, lume_icons::IconThemeError> {
        let provider = self.registry.active();
        let asset_id = provider.resolve(context);
        self.cache
            .borrow_mut()
            .resolved_icon(provider, &asset_id, context.color_mode, 1)
    }
}

fn external_id(name: &str) -> String {
    let stable = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    format!("external:{stable}")
}

/// Cartelle di configurazione temporanee vere, condivise dai test.
#[cfg(test)]
pub(crate) mod fixtures {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use std::fs;
    use std::path::{Path, PathBuf};

    /// Cartella di configurazione con `icon-themes/<nome>` validi.
    pub(crate) fn root_with_packs(names: &[&str]) -> PathBuf {
        // Il tempo non è un'identità esclusiva: create_dir_all può adottare
        // una cartella già creata da un altro test. TempDir crea il nome in
        // esclusiva; i permessi Unix sono espliciti perché la modalità
        // predefinita dipende dall'umask del runner. keep trasferisce la sola
        // pulizia al chiamante esistente.
        let mut builder = tempfile::Builder::new();
        builder.prefix("lume-icon-fixture-owned-");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            builder.permissions(fs::Permissions::from_mode(0o700));
        }
        let root = builder
            .tempdir()
            .expect("creazione esclusiva della fixture temi")
            .keep();
        for name in names {
            write_pack(&root.join("icon-themes"), name);
        }
        root
    }

    #[test]
    fn radici_fixture_concorrenti_sono_distinte_possedute_e_private() {
        let workers = (0..4)
            .map(|_| {
                std::thread::spawn(|| {
                    (0..8)
                        .map(|_| root_with_packs(&["isolato"]))
                        .collect::<Vec<_>>()
                })
            })
            .collect::<Vec<_>>();
        let mut roots = Vec::new();
        let mut panicked = false;
        // Raccogli tutti i thread prima di propagare un errore: nessuna
        // directory viene rimossa mentre un worker può ancora scriverci.
        for worker in workers {
            match worker.join() {
                Ok(paths) => roots.extend(paths),
                Err(_) => panicked = true,
            }
        }
        assert!(
            !panicked,
            "tutti i worker fixture devono essere raccolti senza panic"
        );
        assert_eq!(roots.len(), 32);
        assert_eq!(
            roots.iter().collect::<std::collections::HashSet<_>>().len(),
            32,
            "nessuna radice condivisa dai test concorrenti"
        );
        for root in &roots {
            assert!(root.is_absolute());
            assert!(root.join("icon-themes/isolato/icon-theme.json").is_file());
            assert!(root.join("icon-themes/isolato/x.svg").is_file());
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    fs::metadata(root).unwrap().permissions().mode() & 0o777,
                    0o700
                );
            }
        }
        // Solo le radici restituite dal costruttore esclusivo, mai glob/prefix
        // cleanup o directory di un'altra fixture, anche in caso di panic.
        for root in roots {
            fs::remove_dir_all(root).unwrap();
        }
    }

    pub(crate) fn write_pack(root: &Path, name: &str) {
        let pack = root.join(name);
        fs::create_dir_all(&pack).unwrap();
        fs::write(
            pack.join("x.svg"),
            br#"<svg xmlns="http://www.w3.org/2000/svg"><path d="M0 0h1v1H0z"/></svg>"#,
        )
        .unwrap();
        fs::write(
            pack.join("icon-theme.json"),
            br#"{"iconDefinitions":{"x":{"iconPath":"x.svg"}},"file":"x","folder":"x","folderExpanded":"x","rootFolder":"x","rootFolderExpanded":"x"}"#,
        )
        .unwrap();
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use std::fs;

    use super::IconThemeState;
    use super::fixtures::{root_with_packs, write_pack};

    #[test]
    fn missing_external_pack_recovers_without_forgetting_preference() {
        let mut state = IconThemeState::builtins_only_for_test();
        state.apply_preference("external:night");
        assert_eq!(state.active_id(), "lume-mono");
        assert_eq!(state.preferred_id(), "external:night");
        assert_eq!(
            state
                .rows()
                .into_iter()
                .find(|row| row.id == "external:night")
                .unwrap()
                .error
                .as_deref(),
            Some("pacchetto preferito non trovato")
        );
    }

    #[test]
    fn first_run_uses_material_and_previews_are_real_assets() {
        let state = IconThemeState::builtins_only_for_test();
        assert_eq!(state.active_id(), "material");
        let row = state
            .rows()
            .into_iter()
            .find(|row| row.id == "material")
            .unwrap();
        let preview = row.preview.unwrap();
        assert_ne!(preview.folder.handle.id(), preview.folder_open.handle.id());
        assert_ne!(preview.java.handle.id(), preview.generic_file.handle.id());
    }

    #[test]
    fn invalid_external_pack_is_a_disabled_row_with_real_error() {
        let mut state = IconThemeState::builtins_only_for_test();
        state.add_unavailable_for_test("external:broken", "Broken", "invalid SVG");
        let row = state
            .rows()
            .into_iter()
            .find(|row| row.id == "external:broken")
            .unwrap();
        assert!(row.preview.is_none());
        assert_eq!(row.error.as_deref(), Some("invalid SVG"));
    }

    #[test]
    fn explicit_rescan_restores_a_preferred_external_pack() {
        let root = std::env::temp_dir().join(format!(
            "lume_icon_scan_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let mut state = IconThemeState::new(Some(root.clone()));
        state.apply_preference("external:night");
        assert_eq!(state.active_id(), "lume-mono");

        let pack = root.join("night");
        fs::create_dir_all(&pack).unwrap();
        fs::write(
            pack.join("x.svg"),
            br#"<svg xmlns="http://www.w3.org/2000/svg"><path d="M0 0h1v1H0z"/></svg>"#,
        )
        .unwrap();
        fs::write(
            pack.join("icon-theme.json"),
            br#"{"iconDefinitions":{"x":{"iconPath":"x.svg"}},"file":"x","folder":"x","folderExpanded":"x","rootFolder":"x","rootFolderExpanded":"x"}"#,
        )
        .unwrap();

        state.rescan();
        assert_eq!(state.active_id(), "external:night");
        assert_eq!(state.preferred_id(), "external:night");
        fs::remove_dir_all(root).unwrap();
    }

    fn ids(state: &IconThemeState) -> Vec<String> {
        state.rows().into_iter().map(|row| row.id).collect()
    }

    #[test]
    fn missing_external_directory_is_harmless_at_startup_and_on_first_scan() {
        let root = std::env::temp_dir().join(format!("lume_icon_none_{}", std::process::id()));
        let mut state = IconThemeState::from_config_dir(Some(root));
        state.apply_preference("material");
        assert_eq!(state.active_id(), "material");
        state.scan_external_once();
        assert_eq!(state.active_id(), "material");
        assert!(state.rows().iter().all(|row| row.error.is_none()));
        assert_eq!(state.scan_runs, 1);
    }

    #[test]
    fn construction_does_not_scan_external_packs() {
        let root = root_with_packs(&["alpha", "night"]);
        let state = IconThemeState::from_config_dir(Some(root.clone()));
        assert_eq!(state.scan_runs, 0);
        assert_eq!(state.fs_accesses, 0);
        assert!(ids(&state).iter().all(|id| !id.starts_with("external:")));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn saved_external_theme_is_active_before_first_view_without_loading_others() {
        let root = root_with_packs(&["alpha", "night"]);
        let mut state = IconThemeState::from_config_dir(Some(root.clone()));
        state.apply_preference("external:night");
        assert_eq!(state.active_id(), "external:night");
        assert_eq!(state.preferred_id(), "external:night");
        let ids = ids(&state);
        assert!(ids.contains(&"external:night".to_owned()));
        assert!(!ids.contains(&"external:alpha".to_owned()));
        assert_eq!(state.scan_runs, 0, "nessuna scansione completa all'avvio");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn builtin_preference_makes_zero_external_access() {
        let root = root_with_packs(&["alpha", "night"]);
        // Un pacchetto rotto: se venisse letto comparirebbe come riga non disponibile.
        fs::create_dir_all(root.join("icon-themes").join("broken")).unwrap();
        let mut state = IconThemeState::from_config_dir(Some(root.clone()));
        state.apply_preference("lume-mono");
        assert_eq!(state.active_id(), "lume-mono");
        assert_eq!(state.fs_accesses, 0);
        assert_eq!(state.scan_runs, 0);
        assert!(ids(&state).iter().all(|id| !id.starts_with("external:")));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn first_selector_access_discovers_other_packs_exactly_once() {
        let root = root_with_packs(&["alpha", "night"]);
        let mut state = IconThemeState::from_config_dir(Some(root.clone()));
        state.apply_preference("external:night");

        state.scan_external_once();
        let ids_after_scan = ids(&state);
        assert!(ids_after_scan.contains(&"external:alpha".to_owned()));
        assert_eq!(
            ids_after_scan
                .iter()
                .filter(|id| *id == "external:night")
                .count(),
            1,
            "il pacchetto selezionato non si registra due volte"
        );
        assert!(
            state.rows().iter().all(|row| row.error.is_none()),
            "nessuna riga d'errore spuria dal pacchetto già caricato"
        );
        assert_eq!(state.scan_runs, 1);

        // Un pacchetto nuovo non compare alle aperture successive: niente riscansione.
        write_pack(&root.join("icon-themes"), "zeta");
        state.scan_external_once();
        state.scan_external_once();
        assert_eq!(state.scan_runs, 1);
        assert!(!ids(&state).contains(&"external:zeta".to_owned()));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scanning_never_changes_preferred_or_active_theme() {
        let root = root_with_packs(&["alpha", "night"]);
        let mut state = IconThemeState::from_config_dir(Some(root.clone()));
        state.apply_preference("external:night");
        state.scan_external_once();
        assert_eq!(state.preferred_id(), "external:night");
        assert_eq!(state.active_id(), "external:night");

        let mut builtin = IconThemeState::from_config_dir(Some(root.clone()));
        builtin.apply_preference("lume-mono");
        builtin.scan_external_once();
        assert_eq!(builtin.preferred_id(), "lume-mono");
        assert_eq!(builtin.active_id(), "lume-mono");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unavailable_saved_theme_keeps_its_preference_across_the_scan() {
        let root = root_with_packs(&["alpha"]);
        let mut state = IconThemeState::from_config_dir(Some(root.clone()));
        state.apply_preference("external:night");
        assert_eq!(state.active_id(), "lume-mono");
        state.scan_external_once();
        assert_eq!(state.preferred_id(), "external:night");
        assert_eq!(state.active_id(), "lume-mono");
        let night_rows = state
            .rows()
            .into_iter()
            .filter(|row| row.id == "external:night")
            .count();
        assert_eq!(night_rows, 1);
        fs::remove_dir_all(root).unwrap();
    }
}
