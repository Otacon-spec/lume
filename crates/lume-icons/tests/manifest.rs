#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
use std::fs;
use std::path::{Path, PathBuf};

use lume_icons::limits::{
    MAX_ASSETS, MAX_ASSOCIATIONS, MAX_MANIFEST_BYTES, MAX_PACK_BYTES, MAX_SVG_BYTES,
};
use lume_icons::{
    FileIconContext, IconColorMode, IconThemeError, IconThemeProvider, ManifestIconTheme,
};
use tempfile::TempDir;

const SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path d="M1 1h14v14H1z"/></svg>"#;

struct Pack {
    root: TempDir,
    manifest: PathBuf,
}

impl Pack {
    fn new(manifest: &[u8]) -> Self {
        let root = tempfile::tempdir().unwrap();
        let manifest_path = root.path().join("theme.json");
        fs::write(&manifest_path, manifest).unwrap();
        Self {
            root,
            manifest: manifest_path,
        }
    }

    fn svg(&self, name: &str, bytes: &[u8]) {
        fs::write(self.root.path().join(name), bytes).unwrap();
    }

    fn load(&self) -> Result<ManifestIconTheme, IconThemeError> {
        ManifestIconTheme::load(self.root.path(), &self.manifest)
    }
}

fn complete_manifest(extra: &str) -> Vec<u8> {
    format!(
        r#"{{
          "iconDefinitions": {{
            "file": {{"iconPath":"file.svg"}},
            "folder": {{"iconPath":"folder.svg"}},
            "folder-open": {{"iconPath":"folder-open.svg"}},
            "root": {{"iconPath":"root.svg"}},
            "root-open": {{"iconPath":"root-open.svg"}},
            "java": {{"iconPath":"java.svg"}},
            "light-java": {{"iconPath":"light-java.svg"}}
          }},
          "file":"file", "folder":"folder", "folderExpanded":"folder-open",
          "rootFolder":"root", "rootFolderExpanded":"root-open",
          "fileExtensions":{{"java":"java"}},
          "fileNames":{{"pom.xml":"java"}},
          "folderNames":{{"src":"folder"}},
          "folderNamesExpanded":{{"src":"folder-open"}},
          "languageIds":{{"java":"java"}},
          "light":{{"fileExtensions":{{"java":"light-java"}}}}
          {extra}
        }}"#
    )
    .into_bytes()
}

fn install_assets(pack: &Pack) {
    for name in [
        "file.svg",
        "folder.svg",
        "folder-open.svg",
        "root.svg",
        "root-open.svg",
        "java.svg",
        "light-java.svg",
    ] {
        pack.svg(name, SVG);
    }
}

#[test]
fn supported_vscode_fields_and_light_overrides_are_real() {
    let pack = Pack::new(&complete_manifest(""));
    install_assets(&pack);
    let theme = pack.load().unwrap();

    let dark = FileIconContext::file("Main.java");
    let mut light = dark.clone();
    light.color_mode = IconColorMode::Light;
    let dark_id = theme.resolve(&dark);
    let light_id = theme.resolve(&light);
    assert_ne!(dark_id, light_id);
    assert_eq!(theme.asset(&dark_id).unwrap().bytes(), SVG);
    assert_eq!(theme.asset(&light_id).unwrap().bytes(), SVG);
}

#[test]
fn asset_cannot_escape_pack_root_or_use_absolute_or_remote_paths() {
    let outside = tempfile::NamedTempFile::new().unwrap();
    fs::write(outside.path(), SVG).unwrap();

    for path in [
        "../escape.svg".to_owned(),
        outside.path().display().to_string(),
        "https://example.invalid/icon.svg".to_owned(),
        "file:///C:/icon.svg".to_owned(),
    ] {
        let manifest = format!(
            r#"{{"iconDefinitions":{{"x":{{"iconPath":{path:?}}}}},"file":"x","folder":"x","folderExpanded":"x","rootFolder":"x","rootFolderExpanded":"x"}}"#
        );
        let pack = Pack::new(manifest.as_bytes());
        let error = pack.load().unwrap_err();
        assert!(
            matches!(
                error,
                IconThemeError::PathEscape { .. }
                    | IconThemeError::AbsoluteAssetPath(_)
                    | IconThemeError::RemoteAssetPath(_)
            ),
            "unexpected error for {path}: {error:?}"
        );
    }
}

#[test]
fn symlink_or_junction_escape_is_rejected() {
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("x.svg"), SVG).unwrap();
    let manifest = br#"{"iconDefinitions":{"x":{"iconPath":"outside/x.svg"}},"file":"x","folder":"x","folderExpanded":"x","rootFolder":"x","rootFolderExpanded":"x"}"#;
    let pack = Pack::new(manifest);
    create_directory_link(outside.path(), &pack.root.path().join("outside"))
        .expect("security gate requires directory reparse-point support");
    assert!(matches!(
        pack.load(),
        Err(IconThemeError::PathEscape { .. })
    ));
}

#[test]
fn duplicate_definition_invalid_utf8_and_missing_references_fail_closed() {
    let duplicate = Pack::new(
        br#"{"iconDefinitions":{"x":{"iconPath":"x.svg"},"x":{"iconPath":"y.svg"}},"file":"x","folder":"x","folderExpanded":"x","rootFolder":"x","rootFolderExpanded":"x"}"#,
    );
    assert!(matches!(
        duplicate.load(),
        Err(IconThemeError::InvalidManifest(_))
    ));

    let invalid_utf8 = Pack::new(&[0xff, 0xfe]);
    assert!(matches!(
        invalid_utf8.load(),
        Err(IconThemeError::InvalidUtf8 { .. })
    ));

    let invalid_svg = Pack::new(
        br#"{"iconDefinitions":{"x":{"iconPath":"x.svg"}},"file":"x","folder":"x","folderExpanded":"x","rootFolder":"x","rootFolderExpanded":"x"}"#,
    );
    invalid_svg.svg("x.svg", &[0xff, 0xfe]);
    assert!(matches!(
        invalid_svg.load(),
        Err(IconThemeError::InvalidUtf8 { .. })
    ));

    let missing = Pack::new(
        br#"{"iconDefinitions":{"x":{"iconPath":"x.svg"}},"file":"missing","folder":"x","folderExpanded":"x","rootFolder":"x","rootFolderExpanded":"x"}"#,
    );
    missing.svg("x.svg", SVG);
    assert!(matches!(
        missing.load(),
        Err(IconThemeError::MissingDefinition(value)) if value == "missing"
    ));
}

#[test]
fn dangerous_svg_constructs_are_rejected() {
    for forbidden in [
        "<script>alert(1)</script>",
        "<!ENTITY x SYSTEM 'file:///secret'>",
        "<style>@font-face{src:url(https://example.invalid/a)}</style>",
        "<image href='https://example.invalid/a.svg'/>",
        "<foreignObject/>",
        "<animate attributeName='x' from='0' to='10'/>",
        "<font/>",
        "<style>@import 'https://example.invalid/a.css';</style>",
        "<style>.x{fill:url( 'https://example.invalid/a.svg' )}</style>",
        "<path onload='alert(1)'/>",
    ] {
        let manifest = br#"{"iconDefinitions":{"x":{"iconPath":"x.svg"}},"file":"x","folder":"x","folderExpanded":"x","rootFolder":"x","rootFolderExpanded":"x"}"#;
        let pack = Pack::new(manifest);
        pack.svg(
            "x.svg",
            format!(r#"<svg xmlns="http://www.w3.org/2000/svg">{forbidden}</svg>"#).as_bytes(),
        );
        assert!(matches!(
            pack.load(),
            Err(IconThemeError::ForbiddenSvg { .. })
        ));
    }
}

#[test]
fn exact_resource_limits_are_public_and_enforced() {
    assert_eq!(MAX_MANIFEST_BYTES, 1_048_576);
    assert_eq!(MAX_SVG_BYTES, 524_288);
    assert_eq!(MAX_ASSETS, 4_096);
    assert_eq!(MAX_ASSOCIATIONS, 65_536);
    assert_eq!(MAX_PACK_BYTES, 67_108_864);

    let oversized_manifest = Pack::new(&vec![b' '; MAX_MANIFEST_BYTES + 1]);
    assert!(matches!(
        oversized_manifest.load(),
        Err(IconThemeError::ManifestTooLarge { .. })
    ));

    let manifest = br#"{"iconDefinitions":{"x":{"iconPath":"x.svg"}},"file":"x","folder":"x","folderExpanded":"x","rootFolder":"x","rootFolderExpanded":"x"}"#;
    let oversized_svg = Pack::new(manifest);
    oversized_svg.svg("x.svg", &vec![b'x'; MAX_SVG_BYTES + 1]);
    assert!(matches!(
        oversized_svg.load(),
        Err(IconThemeError::SvgTooLarge { .. })
    ));
}

#[cfg(unix)]
fn create_directory_link(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn create_directory_link(target: &Path, link: &Path) -> std::io::Result<()> {
    junction::create(target, link)
}
