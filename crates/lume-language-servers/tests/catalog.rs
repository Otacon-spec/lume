#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
use std::path::Path;

use lume_language_servers::{
    CatalogError, DistributionMode, Platform, ResourceClass, ServerCatalog,
};
use serde_json::{Value, json};

fn valid_rust_server(url: &str) -> Value {
    json!({
        "id": "rust-analyzer",
        "name": "rust-analyzer",
        "executable": "rust-analyzer",
        "languages": ["rust"],
        "extensions": ["rs"],
        "resource_class": "heavy",
        "release": {
            "repository": "rust-lang/rust-analyzer",
            "version": "v1",
            "assets": [{
                "platform": {"os": "windows", "arch": "x86_64"},
                "name": "ra.zip",
                "url": url,
                "digest": format!("sha256:{}", "a".repeat(64)),
                "size": 1,
                "archive": "zip",
                "binary_path": "rust-analyzer.exe"
            }]
        }
    })
}

#[test]
fn catalog_selects_real_servers_without_process_side_effects() {
    let catalog = ServerCatalog::embedded().unwrap();
    assert_eq!(
        catalog.for_path(Path::new("src/lib.rs")).unwrap().id,
        "rust-analyzer"
    );
    assert_eq!(
        catalog.for_path(Path::new("Cargo.toml")).unwrap().id,
        "tombi"
    );
    assert_eq!(
        catalog.for_path(Path::new("README.md")).unwrap().id,
        "rumdl"
    );
    assert_eq!(catalog.servers().count(), 3);
    assert_eq!(DistributionMode::Core, DistributionMode::Core);
    assert_eq!(DistributionMode::Bundle, DistributionMode::Bundle);
}

#[test]
fn embedded_versions_commands_and_resource_classes_are_pinned() {
    let catalog = ServerCatalog::embedded().unwrap();
    let rust = catalog.get("rust-analyzer").unwrap();
    assert_eq!(rust.release.version, "2026-07-20");
    assert_eq!(rust.resource_class, ResourceClass::Heavy);
    assert!(rust.args.is_empty());
    let tombi = catalog.get("tombi").unwrap();
    assert_eq!(tombi.release.version, "v1.2.4");
    assert_eq!(tombi.args, ["lsp"]);
    let rumdl = catalog.get("rumdl").unwrap();
    assert_eq!(rumdl.release.version, "v0.2.39");
    assert_eq!(rumdl.args, ["server"]);
    for server in catalog.servers() {
        let asset = server.asset_for(&Platform::current()).unwrap();
        assert!(asset.digest.starts_with("sha256:"));
    }
}

#[test]
fn malformed_catalogs_fail_closed() {
    let valid_url = "https://github.com/rust-lang/rust-analyzer/releases/download/v1/ra.zip";
    let server = valid_rust_server(valid_url);
    let duplicate = json!({"schema": 1, "servers": [server.clone(), server]}).to_string();
    assert!(
        matches!(ServerCatalog::parse(&duplicate), Err(CatalogError::DuplicateServer(id)) if id == "rust-analyzer")
    );

    let remote =
        json!({"schema": 1, "servers": [valid_rust_server("http://evil.test/a.zip")]}).to_string();
    assert!(matches!(
        ServerCatalog::parse(&remote),
        Err(CatalogError::InvalidAssetUrl(_))
    ));
}
