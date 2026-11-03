use std::collections::{HashMap, HashSet, VecDeque};

use iced::widget::svg;
use iced::{Color, Element};
use lume_icons::{
    FileIconContext, IconAsset, IconAssetId, IconColorMode, IconThemeError, IconThemeProvider,
    LumeMonoIconTheme, TintRole,
};
use lume_theme::LumeTheme;

/// Sorgente unica delle icone file usata da tutte le superfici dell'app.
/// Il consumer descrive il file; il tema attivo decide asset e fallback.
pub const CACHE_CAPACITY: usize = 2_048;

#[derive(Debug, Clone)]
pub struct FileIcon {
    pub handle: svg::Handle,
    pub tintable: bool,
    /// Ruolo di tinta dichiarato dal tema (Lume Dot: `num`/`faint`).
    pub tint_role: Option<TintRole>,
}

impl FileIcon {
    /// Colore con cui tingere l'icona: il token del ruolo, altrimenti
    /// `default` (il colore della superficie che la ospita).
    pub fn tint(&self, theme: &LumeTheme, default: Color) -> Color {
        match self.tint_role {
            Some(TintRole::Num) => theme.num,
            Some(TintRole::Faint) => theme.faint,
            None => default,
        }
    }

    pub fn view<Message: 'static>(self, size: f32, tint: Color) -> Element<'static, Message> {
        let mut widget = svg(self.handle).width(size).height(size);
        if self.tintable {
            widget = widget.style(move |_, _| svg::Style { color: Some(tint) });
        }
        widget.into()
    }
}

pub trait FileIconSource {
    fn icon(&self, context: &FileIconContext) -> Result<FileIcon, IconThemeError>;
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    provider_id: String,
    asset_id: IconAssetId,
    color_mode: IconColorMode,
    scale_bucket: u8,
}

#[derive(Debug, Clone)]
struct CacheEntry {
    icon: FileIcon,
    last_used: u64,
}

#[derive(Debug)]
pub struct IconAssetCache {
    entries: HashMap<CacheKey, CacheEntry>,
    clock: u64,
    diagnostics: VecDeque<IconThemeError>,
    diagnosed_assets: HashSet<(String, IconAssetId)>,
    handle_creations: usize,
}

impl Default for IconAssetCache {
    fn default() -> Self {
        Self {
            entries: HashMap::with_capacity(CACHE_CAPACITY),
            clock: 0,
            diagnostics: VecDeque::new(),
            diagnosed_assets: HashSet::with_capacity(CACHE_CAPACITY),
            handle_creations: 0,
        }
    }
}

impl IconAssetCache {
    pub fn capacity(&self) -> usize {
        CACHE_CAPACITY
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn diagnostics(&self) -> impl Iterator<Item = &IconThemeError> {
        self.diagnostics.iter()
    }

    pub fn handle(
        &mut self,
        provider: &dyn IconThemeProvider,
        asset_id: &IconAssetId,
        color_mode: IconColorMode,
        scale_bucket: u8,
    ) -> Result<svg::Handle, IconThemeError> {
        self.resolved_icon(provider, asset_id, color_mode, scale_bucket)
            .map(|icon| icon.handle)
    }

    pub fn resolved_icon(
        &mut self,
        provider: &dyn IconThemeProvider,
        asset_id: &IconAssetId,
        color_mode: IconColorMode,
        scale_bucket: u8,
    ) -> Result<FileIcon, IconThemeError> {
        let key = CacheKey {
            provider_id: provider.metadata().id.clone(),
            asset_id: asset_id.clone(),
            color_mode,
            scale_bucket,
        };
        let now = self.next_clock();
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.last_used = now;
            return Ok(entry.icon.clone());
        }

        let icon = self.load_with_fallback(provider, asset_id, color_mode)?;
        if self.entries.len() == CACHE_CAPACITY
            && let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| key.clone())
        {
            self.entries.remove(&oldest);
        }
        self.entries.insert(
            key,
            CacheEntry {
                icon: icon.clone(),
                last_used: now,
            },
        );
        Ok(icon)
    }

    fn load_with_fallback(
        &mut self,
        provider: &dyn IconThemeProvider,
        requested: &IconAssetId,
        color_mode: IconColorMode,
    ) -> Result<FileIcon, IconThemeError> {
        match self.load(provider, requested) {
            Ok(handle) => return Ok(handle),
            Err(error) => self.record_diagnostic(provider.metadata().id.as_str(), requested, error),
        }

        let mut fallback_context = FileIconContext::file("");
        fallback_context.color_mode = color_mode;
        let fallback_id = provider.resolve(&fallback_context);
        if &fallback_id != requested {
            match self.load(provider, &fallback_id) {
                Ok(handle) => return Ok(handle),
                Err(error) => {
                    self.record_diagnostic(provider.metadata().id.as_str(), &fallback_id, error)
                }
            }
        }

        let recovery = LumeMonoIconTheme::new();
        let recovery_id = recovery.resolve(&FileIconContext::file(""));
        self.load(&recovery, &recovery_id)
    }

    fn load(
        &mut self,
        provider: &dyn IconThemeProvider,
        asset_id: &IconAssetId,
    ) -> Result<FileIcon, IconThemeError> {
        let asset = provider.asset(asset_id)?;
        validate_svg(provider.metadata().id.as_str(), asset_id, &asset)?;
        self.handle_creations = self.handle_creations.saturating_add(1);
        Ok(FileIcon {
            handle: svg::Handle::from_memory(asset.bytes().to_vec()),
            tintable: asset.tintable(),
            tint_role: asset.tint_role(),
        })
    }

    fn record_diagnostic(
        &mut self,
        provider_id: &str,
        asset_id: &IconAssetId,
        error: IconThemeError,
    ) {
        if self.diagnosed_assets.len() >= CACHE_CAPACITY {
            return;
        }
        let key = (provider_id.to_owned(), asset_id.clone());
        if self.diagnosed_assets.insert(key) {
            self.diagnostics.push_back(error);
        }
    }

    fn next_clock(&mut self) -> u64 {
        if self.clock == u64::MAX {
            for entry in self.entries.values_mut() {
                entry.last_used = 0;
            }
            self.clock = 1;
        } else {
            self.clock += 1;
        }
        self.clock
    }

    #[doc(hidden)]
    pub fn handle_creation_count(&self) -> usize {
        self.handle_creations
    }

    #[cfg(test)]
    fn contains(
        &self,
        provider_id: &str,
        asset_id: &IconAssetId,
        color_mode: IconColorMode,
        scale_bucket: u8,
    ) -> bool {
        self.entries.contains_key(&CacheKey {
            provider_id: provider_id.to_owned(),
            asset_id: asset_id.clone(),
            color_mode,
            scale_bucket,
        })
    }
}

fn validate_svg(
    provider_id: &str,
    asset_id: &IconAssetId,
    asset: &IconAsset,
) -> Result<(), IconThemeError> {
    let source =
        std::str::from_utf8(asset.bytes()).map_err(|error| IconThemeError::InvalidSvgAsset {
            provider: provider_id.to_owned(),
            id: asset_id.clone(),
            message: error.to_string(),
        })?;
    let document =
        roxmltree::Document::parse(source).map_err(|error| IconThemeError::InvalidSvgAsset {
            provider: provider_id.to_owned(),
            id: asset_id.clone(),
            message: error.to_string(),
        })?;
    if document.root_element().tag_name().name() != "svg" {
        return Err(IconThemeError::InvalidSvgAsset {
            provider: provider_id.to_owned(),
            id: asset_id.clone(),
            message: "root element is not svg".to_owned(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use std::collections::HashSet;

    use lume_icons::{
        FileIconContext, IconAsset, IconAssetId, IconColorMode, IconThemeError, IconThemeMetadata,
        IconThemeProvider,
    };

    use super::{CACHE_CAPACITY, IconAssetCache};

    const SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg"><path d="M0 0h1v1H0z"/></svg>"#;

    struct FixtureProvider {
        metadata: IconThemeMetadata,
        invalid: HashSet<IconAssetId>,
        tintable: bool,
    }

    impl FixtureProvider {
        fn new(id: &str) -> Self {
            Self {
                metadata: IconThemeMetadata::built_in(id, id, 0),
                invalid: HashSet::new(),
                tintable: false,
            }
        }

        fn tintable(id: &str) -> Self {
            Self {
                metadata: IconThemeMetadata::built_in(id, id, 0),
                invalid: HashSet::new(),
                tintable: true,
            }
        }

        fn invalid(id: &str, asset: IconAssetId) -> Self {
            Self {
                metadata: IconThemeMetadata::built_in(id, id, 0),
                invalid: HashSet::from([asset]),
                tintable: false,
            }
        }
    }

    impl IconThemeProvider for FixtureProvider {
        fn metadata(&self) -> &IconThemeMetadata {
            &self.metadata
        }

        fn resolve(&self, _context: &FileIconContext) -> IconAssetId {
            "generic".into()
        }

        fn asset(&self, id: &IconAssetId) -> Result<IconAsset, IconThemeError> {
            let bytes = if self.invalid.contains(id) {
                b"not svg".as_slice()
            } else {
                SVG
            };
            Ok(IconAsset::new(bytes.to_vec(), self.tintable))
        }
    }

    #[test]
    fn cache_hit_reuses_handle_without_reparsing() {
        let provider = FixtureProvider::new("a");
        let mut cache = IconAssetCache::default();
        let id: IconAssetId = "java".into();
        let first = cache
            .handle(&provider, &id, IconColorMode::Dark, 1)
            .unwrap();
        let second = cache
            .handle(&provider, &id, IconColorMode::Dark, 1)
            .unwrap();
        assert_eq!(first, second);
        assert_eq!(cache.handle_creation_count(), 1);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn tintable_flag_survives_cache_hits_for_monochrome_rendering() {
        let provider = FixtureProvider::tintable("mono");
        let mut cache = IconAssetCache::default();
        let id: IconAssetId = "generic".into();
        let first = cache
            .resolved_icon(&provider, &id, IconColorMode::Dark, 1)
            .unwrap();
        let second = cache
            .resolved_icon(&provider, &id, IconColorMode::Dark, 1)
            .unwrap();
        assert!(first.tintable);
        assert!(second.tintable);
        assert_eq!(first.handle, second.handle);
        assert_eq!(cache.handle_creation_count(), 1);
    }

    #[test]
    fn provider_color_and_scale_are_separate_keys() {
        let provider_a = FixtureProvider::new("a");
        let provider_b = FixtureProvider::new("b");
        let mut cache = IconAssetCache::default();
        let id: IconAssetId = "java".into();
        cache
            .handle(&provider_a, &id, IconColorMode::Dark, 1)
            .unwrap();
        cache
            .handle(&provider_b, &id, IconColorMode::Dark, 1)
            .unwrap();
        cache
            .handle(&provider_a, &id, IconColorMode::Light, 1)
            .unwrap();
        cache
            .handle(&provider_a, &id, IconColorMode::Dark, 2)
            .unwrap();
        assert_eq!(cache.len(), 4);
        assert_eq!(cache.handle_creation_count(), 4);
    }

    #[test]
    fn lru_evicts_exactly_at_2049_entries() {
        let provider = FixtureProvider::new("a");
        let mut cache = IconAssetCache::default();
        for index in 0..CACHE_CAPACITY {
            cache
                .handle(
                    &provider,
                    &IconAssetId::from(format!("asset-{index}")),
                    IconColorMode::Dark,
                    1,
                )
                .unwrap();
        }
        let oldest: IconAssetId = "asset-0".into();
        cache
            .handle(&provider, &oldest, IconColorMode::Dark, 1)
            .unwrap();
        cache
            .handle(
                &provider,
                &IconAssetId::from("overflow"),
                IconColorMode::Dark,
                1,
            )
            .unwrap();
        assert_eq!(cache.len(), CACHE_CAPACITY);
        assert!(cache.contains("a", &oldest, IconColorMode::Dark, 1));
        assert!(!cache.contains("a", &IconAssetId::from("asset-1"), IconColorMode::Dark, 1));
    }

    #[test]
    fn malformed_asset_uses_generic_and_deduplicates_diagnostic() {
        let bad: IconAssetId = "bad".into();
        let provider = FixtureProvider::invalid("a", bad.clone());
        let mut cache = IconAssetCache::default();
        let first = cache
            .handle(&provider, &bad, IconColorMode::Dark, 1)
            .unwrap();
        let second = cache
            .handle(&provider, &bad, IconColorMode::Dark, 1)
            .unwrap();
        assert_eq!(first, second);
        assert_eq!(cache.handle_creation_count(), 1);
        assert_eq!(cache.diagnostics().count(), 1);
    }

    #[test]
    fn cache_contract_exists_with_fixed_capacity() {
        let cache = IconAssetCache::default();
        assert_eq!(cache.capacity(), CACHE_CAPACITY);
        assert!(cache.is_empty());
    }
}
