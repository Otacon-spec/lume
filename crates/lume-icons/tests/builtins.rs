#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

use lume_icons::{
    CatppuccinIconTheme, FileIconContext, IconThemeProvider, IconThemeRegistry, LumeDotIconTheme,
    MaterialIconTheme, TintRole,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};

#[test]
fn colored_builtins_cover_required_languages_and_folders() {
    let providers: Vec<Box<dyn IconThemeProvider>> = vec![
        Box::new(MaterialIconTheme::new()),
        Box::new(CatppuccinIconTheme::new()),
    ];
    for provider in providers {
        let generic_file = provider.resolve(&FileIconContext::file("unknown.lume-no-association"));
        for name in [
            "Main.java",
            "pom.xml",
            "build.gradle",
            "application.yml",
            "main.rs",
            "Cargo.toml",
            "main.c",
            "main.h",
            "main.cpp",
            "main.hpp",
            "Program.cs",
            "README.md",
            ".gitignore",
            "Dockerfile",
        ] {
            assert_ne!(
                provider.resolve(&FileIconContext::file(name)),
                generic_file,
                "{} lacks {name}",
                provider.metadata().id
            );
        }

        for (name, open) in [
            ("src", false),
            ("src", true),
            (".git", false),
            (".git", true),
        ] {
            let generic = provider.resolve(&FileIconContext::folder("unknown-folder", open));
            assert_ne!(
                provider.resolve(&FileIconContext::folder(name, open)),
                generic,
                "{} lacks folder {name} open={open}",
                provider.metadata().id
            );
        }
    }
}

#[test]
fn lume_dot_marks_rust_with_num_files_with_faint_and_folders_with_nothing() {
    let theme = LumeDotIconTheme::new();
    let code = theme.resolve(&FileIconContext::file("main.rs"));
    let other = theme.resolve(&FileIconContext::file("Cargo.toml"));
    let folder = theme.resolve(&FileIconContext::folder("src", true));
    assert_ne!(code, other);
    assert_eq!(theme.asset(&code).unwrap().tint_role(), Some(TintRole::Num));
    assert_eq!(
        theme.asset(&other).unwrap().tint_role(),
        Some(TintRole::Faint)
    );
    assert_eq!(theme.asset(&folder).unwrap().tint_role(), None);
    assert!(theme.asset(&folder).unwrap().bytes().starts_with(b"<svg"));
    assert!(theme.asset(&code).unwrap().tintable());

    // registrato tra i builtin, ma Material resta il tema attivo di default
    let registry = IconThemeRegistry::with_builtins();
    assert_eq!(registry.active_id(), "material");
    assert!(registry.provider("lume-dot").is_some());
    let names = registry
        .providers()
        .map(|provider| provider.metadata().name.clone())
        .collect::<Vec<_>>();
    assert_eq!(names.last().map(String::as_str), Some("Lume Dot"));
}

#[test]
fn every_builtin_association_has_a_parseable_svg() {
    let material = MaterialIconTheme::new();
    let catppuccin = CatppuccinIconTheme::new();
    for (provider, ids) in [
        (
            &material as &dyn IconThemeProvider,
            material.referenced_asset_ids(),
        ),
        (
            &catppuccin as &dyn IconThemeProvider,
            catppuccin.referenced_asset_ids(),
        ),
    ] {
        for id in ids {
            let asset = provider.asset(&id).unwrap();
            let source = std::str::from_utf8(asset.bytes()).unwrap();
            let document = roxmltree::Document::parse(source).unwrap();
            assert_eq!(document.root_element().tag_name().name(), "svg");
        }
    }
}

#[derive(Deserialize)]
struct Inventory {
    sources: BTreeMap<String, Source>,
    assets: BTreeMap<String, AssetRecord>,
}

#[derive(Deserialize)]
struct Source {
    repository: String,
    tag: String,
    commit: String,
    license: String,
    archive_sha256: String,
}

#[derive(Deserialize)]
struct AssetRecord {
    sha256: String,
    upstream_path: String,
}

#[test]
fn pinned_inventory_hashes_and_licenses_match() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let inventory: Inventory =
        serde_json::from_slice(&fs::read(root.join("assets/ASSET_MANIFEST.json")).unwrap())
            .unwrap();
    assert_eq!(inventory.sources["material"].tag, "v5.33.1");
    assert_eq!(
        inventory.sources["material"].commit,
        "e6f371b1e54c49200ac0c6bc44e0728b487aff2d"
    );
    assert_eq!(inventory.sources["catppuccin-mocha"].tag, "v1.26.0");
    assert_eq!(
        inventory.sources["catppuccin-mocha"].commit,
        "b6915da9f6889b683a110aa747de96c2820a537d"
    );
    for source in inventory.sources.values() {
        assert!(source.repository.starts_with("https://github.com/"));
        assert_eq!(source.license, "MIT");
        assert_eq!(source.archive_sha256.len(), 64);
    }
    let inventoried = inventory.assets.keys().cloned().collect::<BTreeSet<_>>();
    let on_disk = ["material", "catppuccin-mocha"]
        .into_iter()
        .flat_map(|pack| {
            fs::read_dir(root.join("assets").join(pack))
                .unwrap()
                .map(move |entry| (pack, entry.unwrap().path()))
        })
        .filter(|(_, path)| path.extension().is_some_and(|extension| extension == "svg"))
        .map(|(pack, path)| format!("{pack}/{}", path.file_name().unwrap().to_string_lossy()))
        .collect::<BTreeSet<_>>();
    assert_eq!(inventoried, on_disk);

    for (relative, record) in inventory.assets {
        assert!(!record.upstream_path.is_empty());
        let actual = Sha256::digest(fs::read(root.join("assets").join(relative)).unwrap());
        assert_eq!(format!("{actual:x}"), record.sha256);
    }
    for pack in ["material", "catppuccin-mocha"] {
        let license = fs::read_to_string(root.join("assets").join(pack).join("LICENSE")).unwrap();
        assert!(license.contains("MIT License"));
    }
}
