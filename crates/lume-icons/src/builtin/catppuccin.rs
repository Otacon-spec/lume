use crate::builtin::data::{BuiltinThemeData, StandardAssetIds, standard_associations};
use crate::{
    FileIconContext, IconAsset, IconAssetId, IconThemeError, IconThemeMetadata, IconThemeProvider,
};

pub const CATPPUCCIN_MOCHA_ID: &str = "catppuccin-mocha";

pub struct CatppuccinIconTheme {
    data: BuiltinThemeData,
}

impl CatppuccinIconTheme {
    pub fn new() -> Self {
        let ids = StandardAssetIds {
            file: "file",
            folder: "folder",
            folder_open: "folder-open",
            root: "root",
            root_open: "root-open",
            java: "java",
            java_build: "java",
            gradle: "gradle",
            spring_config: "properties",
            rust: "rust",
            rust_manifest: "cargo",
            toml: "toml",
            c: "c",
            c_header: "c-header",
            cpp: "cpp",
            cpp_header: "cpp-header",
            csharp: "csharp",
            markdown: "markdown",
            git: "git",
            docker: "docker",
            license: "license",
            folder_src: "folder-src",
            folder_src_open: "folder-src-open",
            folder_git: "folder-git",
            folder_git_open: "folder-git-open",
        };
        let assets: [(&str, &[u8]); 25] = [
            (
                "file",
                include_bytes!("../../assets/catppuccin-mocha/file.svg"),
            ),
            (
                "folder",
                include_bytes!("../../assets/catppuccin-mocha/folder.svg"),
            ),
            (
                "folder-open",
                include_bytes!("../../assets/catppuccin-mocha/folder-open.svg"),
            ),
            (
                "root",
                include_bytes!("../../assets/catppuccin-mocha/root.svg"),
            ),
            (
                "root-open",
                include_bytes!("../../assets/catppuccin-mocha/root-open.svg"),
            ),
            (
                "java",
                include_bytes!("../../assets/catppuccin-mocha/java.svg"),
            ),
            (
                "cargo",
                include_bytes!("../../assets/catppuccin-mocha/cargo.svg"),
            ),
            (
                "gradle",
                include_bytes!("../../assets/catppuccin-mocha/gradle.svg"),
            ),
            (
                "properties",
                include_bytes!("../../assets/catppuccin-mocha/properties.svg"),
            ),
            (
                "yaml",
                include_bytes!("../../assets/catppuccin-mocha/yaml.svg"),
            ),
            (
                "rust",
                include_bytes!("../../assets/catppuccin-mocha/rust.svg"),
            ),
            (
                "toml",
                include_bytes!("../../assets/catppuccin-mocha/toml.svg"),
            ),
            ("c", include_bytes!("../../assets/catppuccin-mocha/c.svg")),
            (
                "c-header",
                include_bytes!("../../assets/catppuccin-mocha/c-header.svg"),
            ),
            (
                "cpp",
                include_bytes!("../../assets/catppuccin-mocha/cpp.svg"),
            ),
            (
                "cpp-header",
                include_bytes!("../../assets/catppuccin-mocha/cpp-header.svg"),
            ),
            (
                "csharp",
                include_bytes!("../../assets/catppuccin-mocha/csharp.svg"),
            ),
            (
                "markdown",
                include_bytes!("../../assets/catppuccin-mocha/markdown.svg"),
            ),
            (
                "git",
                include_bytes!("../../assets/catppuccin-mocha/git.svg"),
            ),
            (
                "docker",
                include_bytes!("../../assets/catppuccin-mocha/docker.svg"),
            ),
            (
                "license",
                include_bytes!("../../assets/catppuccin-mocha/license.svg"),
            ),
            (
                "folder-src",
                include_bytes!("../../assets/catppuccin-mocha/folder-src.svg"),
            ),
            (
                "folder-src-open",
                include_bytes!("../../assets/catppuccin-mocha/folder-src-open.svg"),
            ),
            (
                "folder-git",
                include_bytes!("../../assets/catppuccin-mocha/folder-git.svg"),
            ),
            (
                "folder-git-open",
                include_bytes!("../../assets/catppuccin-mocha/folder-git-open.svg"),
            ),
        ];
        Self {
            data: BuiltinThemeData::new(
                IconThemeMetadata::built_in(CATPPUCCIN_MOCHA_ID, "Catppuccin Mocha", 1),
                standard_associations(&ids),
                assets,
            ),
        }
    }

    pub fn referenced_asset_ids(&self) -> Vec<IconAssetId> {
        self.data.associations.referenced_asset_ids()
    }
}

impl Default for CatppuccinIconTheme {
    fn default() -> Self {
        Self::new()
    }
}

impl IconThemeProvider for CatppuccinIconTheme {
    fn metadata(&self) -> &IconThemeMetadata {
        &self.data.metadata
    }

    fn resolve(&self, context: &FileIconContext) -> IconAssetId {
        self.data.resolve(context)
    }

    fn asset(&self, id: &IconAssetId) -> Result<IconAsset, IconThemeError> {
        self.data.asset(id)
    }
}
