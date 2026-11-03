//! Insieme dei language server posseduti dal workspace app.
use crate::lsp_mode::LspWorkspace;
use lume_core::DocId;
use lume_language_servers::ServerSpec;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Default)]
pub struct LspSet {
    servers: Vec<LspWorkspace>,
    /// Ultima generazione per server anche dopo `clear`: un worker del
    /// progetto precedente non deve coincidere con il primo riavvio.
    last_epochs: HashMap<String, u64>,
}
impl LspSet {
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.servers.is_empty()
    }
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.servers.len()
    }
    pub fn iter(&self) -> impl Iterator<Item = &LspWorkspace> {
        self.servers.iter()
    }
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut LspWorkspace> {
        self.servers.iter_mut()
    }
    pub fn get(&self, id: &str) -> Option<&LspWorkspace> {
        self.servers.iter().find(|lsp| lsp.spec.id == id)
    }
    pub fn get_mut(&mut self, id: &str) -> Option<&mut LspWorkspace> {
        self.servers.iter_mut().find(|lsp| lsp.spec.id == id)
    }
    pub fn ensure(&mut self, spec: &'static ServerSpec, root: PathBuf) {
        if self.get(&spec.id).is_none() {
            let mut lsp = LspWorkspace::new(spec, root);
            if let Some(epoch) = self.last_epochs.get(&spec.id) {
                lsp.epoch = *epoch;
            }
            self.servers.push(lsp);
        }
    }
    pub fn serving(&self, path: &Path) -> Option<&LspWorkspace> {
        self.servers.iter().find(|lsp| lsp.handles(path))
    }
    pub fn holding(&self, id: DocId) -> Option<&LspWorkspace> {
        self.servers.iter().find(|lsp| lsp.opened.contains_key(&id))
    }
    pub fn holding_mut(&mut self, id: DocId) -> Option<&mut LspWorkspace> {
        self.servers
            .iter_mut()
            .find(|lsp| lsp.opened.contains_key(&id))
    }
    pub fn focus(&self, path: Option<&Path>) -> Option<&LspWorkspace> {
        path.and_then(|path| self.serving(path))
            .or_else(|| self.servers.iter().find(|lsp| lsp.state.is_live()))
    }
    pub fn any_live(&self) -> bool {
        self.servers.iter().any(|lsp| lsp.state.is_live())
    }
    pub fn clear(&mut self) {
        for lsp in &mut self.servers {
            lsp.cancel_download();
            self.last_epochs
                .entry(lsp.spec.id.clone())
                .and_modify(|epoch| *epoch = (*epoch).max(lsp.epoch))
                .or_insert(lsp.epoch);
        }
        self.servers.clear();
    }

    #[cfg(test)]
    pub(crate) fn only(&self) -> Option<&LspWorkspace> {
        self.servers.first()
    }
    #[cfg(test)]
    pub(crate) fn only_mut(&mut self) -> Option<&mut LspWorkspace> {
        self.servers.first_mut()
    }
    #[cfg(test)]
    pub(crate) fn insert(&mut self, lsp: LspWorkspace) {
        let id = lsp.spec.id.clone();
        self.last_epochs
            .entry(id.clone())
            .and_modify(|epoch| *epoch = (*epoch).max(lsp.epoch))
            .or_insert(lsp.epoch);
        self.servers.retain(|existing| existing.spec.id != id);
        self.servers.push(lsp);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::LspSet;
    use crate::lsp_mode::LspState;
    use lume_core::DocId;
    use lume_language_servers::{ServerCatalog, ServerSpec};
    use std::path::{Path, PathBuf};
    fn spec(id: &str) -> &'static ServerSpec {
        ServerCatalog::embedded().unwrap().get(id).unwrap()
    }
    fn con_due() -> LspSet {
        let mut set = LspSet::default();
        set.ensure(spec("rust-analyzer"), PathBuf::from("."));
        set.ensure(spec("tombi"), PathBuf::from("."));
        set
    }
    #[test]
    fn ensure_non_duplica_e_non_tocca_lo_stato_di_chi_c_e() {
        let mut set = con_due();
        set.get_mut("rust-analyzer").unwrap().state = LspState::Avvio;
        set.ensure(spec("rust-analyzer"), PathBuf::from("altra"));
        assert_eq!(set.len(), 2);
        let ra = set.get("rust-analyzer").unwrap();
        assert!(matches!(ra.state, LspState::Avvio));
        assert_eq!(ra.root, PathBuf::from("."));
    }
    #[test]
    fn un_file_lo_serve_il_server_che_ne_conosce_l_estensione() {
        let set = con_due();
        assert_eq!(
            set.serving(Path::new("src/main.rs"))
                .map(|s| s.spec.id.as_str()),
            Some("rust-analyzer")
        );
        assert_eq!(
            set.serving(Path::new("Cargo.toml"))
                .map(|s| s.spec.id.as_str()),
            Some("tombi")
        );
        assert!(set.serving(Path::new("immagine.png")).is_none());
    }
    #[test]
    fn un_documento_appartiene_al_server_che_lo_ha_aperto() {
        let mut set = con_due();
        let uri: lsp_types::Uri = "file:///w/Cargo.toml".parse().unwrap();
        set.get_mut("tombi").unwrap().mark_opened(DocId(3), uri, 1);
        assert_eq!(
            set.holding(DocId(3)).map(|s| s.spec.id.as_str()),
            Some("tombi")
        );
        assert!(set.holding(DocId(4)).is_none());
        assert!(set.holding_mut(DocId(3)).is_some());
    }
    #[test]
    fn il_fuoco_e_il_server_del_file_o_altrimenti_il_primo_vivo() {
        let mut set = con_due();
        assert!(set.focus(Some(Path::new("immagine.png"))).is_none());
        assert!(!set.any_live());
        set.get_mut("tombi").unwrap().state = LspState::Avvio;
        assert!(set.any_live());
        assert_eq!(
            set.focus(Some(Path::new("immagine.png")))
                .map(|s| s.spec.id.as_str()),
            Some("tombi")
        );
        assert_eq!(
            set.focus(Some(Path::new("main.rs")))
                .map(|s| s.spec.id.as_str()),
            Some("rust-analyzer")
        );
        set.clear();
        assert!(set.is_empty());
    }

    #[test]
    fn clear_annulla_i_download_in_corso() {
        let mut set = con_due();
        let flag = lume_language_servers::CancelFlag::default();
        set.get_mut("tombi").unwrap().download_cancel = Some(flag.clone());
        set.clear();
        assert!(
            flag.is_cancelled(),
            "cambiare progetto non lascia scaricare a vuoto"
        );
        assert!(set.is_empty());
    }

    #[test]
    fn una_nuova_generazione_dopo_clear_non_riusa_quella_del_server_vecchio() {
        let mut set = LspSet::default();
        let spec = spec("rust-analyzer");
        set.ensure(spec, PathBuf::from("/progetto-vecchio"));
        let vecchia = set.get_mut("rust-analyzer").unwrap().bump();

        set.clear();
        set.ensure(spec, PathBuf::from("/progetto-nuovo"));
        let nuova = set.get_mut("rust-analyzer").unwrap().bump();

        assert!(
            nuova > vecchia,
            "gli eventi LSP vecchi devono restare distinguibili dopo il reset"
        );
    }
}
