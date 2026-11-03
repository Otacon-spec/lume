#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
use std::collections::HashMap;

use lume_icons::{
    FileIconContext, FileIconResolver, IconAsset, IconAssetId, IconThemeAssociations,
    IconThemeError, IconThemeMetadata, IconThemeProvider, IconThemeRegistry, LUME_MONO_ID,
    LumeMonoIconTheme,
};

fn id(value: &str) -> IconAssetId {
    IconAssetId::from(value)
}

struct FixtureTheme {
    metadata: IconThemeMetadata,
    associations: IconThemeAssociations,
    assets: HashMap<IconAssetId, IconAsset>,
}

impl FixtureTheme {
    fn with_conflicts() -> Self {
        let mut associations = IconThemeAssociations::new(
            id("file"),
            id("folder"),
            id("folder-open"),
            id("root"),
            id("root-open"),
        );
        associations.insert_file_name("Dockerfile", id("exact"));
        associations.insert_file_extension("ts", id("ts"));
        associations.insert_file_extension("test.ts", id("test-ts"));
        associations.insert_language_id("typescript", id("language"));
        associations.insert_folder_name("src", id("src"));
        associations.insert_open_folder_name("src", id("src-open"));
        associations.insert_root_folder_name("workspace", id("workspace"));
        associations.insert_open_root_folder_name("workspace", id("workspace-open"));

        let assets = [
            "file",
            "folder",
            "folder-open",
            "root",
            "root-open",
            "exact",
            "ts",
            "test-ts",
            "language",
            "src",
            "src-open",
            "workspace",
            "workspace-open",
        ]
        .into_iter()
        .map(|asset_id| {
            (
                id(asset_id),
                IconAsset::from_static(b"<svg xmlns='http://www.w3.org/2000/svg'/>", false),
            )
        })
        .collect();

        Self {
            metadata: IconThemeMetadata::built_in("fixture", "Fixture", 1),
            associations,
            assets,
        }
    }
}

impl IconThemeProvider for FixtureTheme {
    fn metadata(&self) -> &IconThemeMetadata {
        &self.metadata
    }

    fn resolve(&self, context: &FileIconContext) -> IconAssetId {
        self.associations.resolve(context)
    }

    fn asset(&self, asset_id: &IconAssetId) -> Result<IconAsset, IconThemeError> {
        self.assets
            .get(asset_id)
            .cloned()
            .ok_or_else(|| IconThemeError::MissingAsset(asset_id.clone()))
    }
}

#[test]
fn resolution_uses_required_file_precedence_and_casefolding() {
    let provider = FixtureTheme::with_conflicts();
    let resolver = FileIconResolver::new(&provider);
    assert_eq!(
        resolver.resolve(&FileIconContext::file("DOCKERFILE")),
        id("exact")
    );

    let mut compound = FileIconContext::file("app.test.ts");
    compound.language_id = Some("typescript".into());
    assert_eq!(resolver.resolve(&compound), id("test-ts"));

    let mut language = FileIconContext::file("extensionless");
    language.language_id = Some("TypeScript".into());
    assert_eq!(resolver.resolve(&language), id("language"));
}

#[test]
fn folder_resolution_preserves_named_root_and_open_state() {
    let provider = FixtureTheme::with_conflicts();
    let resolver = FileIconResolver::new(&provider);
    assert_eq!(
        resolver.resolve(&FileIconContext::folder("src", false)),
        id("src")
    );
    assert_eq!(
        resolver.resolve(&FileIconContext::folder("src", true)),
        id("src-open")
    );
    assert_eq!(
        resolver.resolve(&FileIconContext::root_folder("workspace", false)),
        id("workspace")
    );
    assert_eq!(
        resolver.resolve(&FileIconContext::root_folder("workspace", true)),
        id("workspace-open")
    );
}

#[test]
fn registry_rejects_duplicates_sorts_and_keeps_recovery() {
    let mut registry = IconThemeRegistry::with_recovery(LumeMonoIconTheme::new()).unwrap();
    assert!(matches!(
        registry.register(LumeMonoIconTheme::new()),
        Err(IconThemeError::DuplicateId(value)) if value == LUME_MONO_ID
    ));
    registry.register(FixtureTheme::with_conflicts()).unwrap();
    assert_eq!(registry.active_id(), LUME_MONO_ID);
    assert_eq!(
        registry
            .providers()
            .map(|item| item.metadata().id.as_str())
            .collect::<Vec<_>>(),
        vec!["fixture", LUME_MONO_ID]
    );
    assert!(matches!(
        registry.activate("missing"),
        Err(IconThemeError::UnknownProvider(value)) if value == "missing"
    ));
}

#[test]
fn monochrome_provider_always_returns_real_recovery_assets() {
    let mono = LumeMonoIconTheme::new();
    for context in [
        FileIconContext::file("main.rs"),
        FileIconContext::folder("src", false),
        FileIconContext::folder("src", true),
        FileIconContext::root_folder("repo", false),
        FileIconContext::root_folder("repo", true),
    ] {
        let asset_id = mono.resolve(&context);
        let asset = mono.asset(&asset_id).unwrap();
        assert!(asset.bytes().starts_with(b"<svg"));
        assert!(asset.tintable());
    }
}

#[test]
fn standard_builtins_default_to_material_in_rank_order() {
    let registry = IconThemeRegistry::with_builtins();
    assert_eq!(registry.active_id(), "material");
    assert_eq!(
        registry
            .providers()
            .map(|provider| provider.metadata().id.as_str())
            .collect::<Vec<_>>(),
        vec!["material", "catppuccin-mocha", "lume-mono", "lume-dot"]
    );
}
