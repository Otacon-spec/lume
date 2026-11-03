#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
use std::time::Instant;

use lume_icons::{
    FileIconContext, IconAsset, IconAssetId, IconColorMode, IconThemeError, IconThemeMetadata,
    IconThemeProvider,
};
use lume_ui::file_icons::IconAssetCache;

const SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg"><path d="M0 0h1v1H0z"/></svg>"#;

struct BenchProvider {
    metadata: IconThemeMetadata,
}

impl IconThemeProvider for BenchProvider {
    fn metadata(&self) -> &IconThemeMetadata {
        &self.metadata
    }

    fn resolve(&self, _context: &FileIconContext) -> IconAssetId {
        "generic".into()
    }

    fn asset(&self, _id: &IconAssetId) -> Result<IconAsset, IconThemeError> {
        Ok(IconAsset::from_static(SVG, false))
    }
}

fn main() {
    let provider = BenchProvider {
        metadata: IconThemeMetadata::built_in("bench", "Bench", 0),
    };
    let ids = ["java", "rust", "folder", "folder-open", "generic"].map(IconAssetId::from);
    let mut cache = IconAssetCache::default();
    let started = Instant::now();
    for index in 0..10_000 {
        cache
            .handle(&provider, &ids[index % ids.len()], IconColorMode::Dark, 1)
            .unwrap();
    }
    let elapsed = started.elapsed();
    assert_eq!(cache.len(), ids.len());
    assert_eq!(cache.handle_creation_count(), ids.len());
    println!(
        "icon_prepare rows=10000 unique_handles={} handle_creations={} elapsed_us={}",
        cache.len(),
        cache.handle_creation_count(),
        elapsed.as_micros()
    );
}
