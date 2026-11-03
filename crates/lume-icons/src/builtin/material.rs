use crate::builtin::data::{BuiltinThemeData, StandardAssetIds, standard_associations};
use crate::{
    FileIconContext, IconAsset, IconAssetId, IconThemeError, IconThemeMetadata, IconThemeProvider,
};

pub const MATERIAL_ID: &str = "material";

pub struct MaterialIconTheme {
    data: BuiltinThemeData,
}

impl MaterialIconTheme {
    pub fn new() -> Self {
        let ids = StandardAssetIds {
            file: "file",
            folder: "folder",
            folder_open: "folder-open",
            root: "root",
            root_open: "root-open",
            java: "java",
            java_build: "maven",
            gradle: "gradle",
            spring_config: "yaml",
            rust: "rust",
            rust_manifest: "rust",
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
            folder_git: "root",
            folder_git_open: "root-open",
        };
        let assets: [(&str, &[u8]); 22] = [
            ("file", include_bytes!("../../assets/material/file.svg")),
            ("folder", include_bytes!("../../assets/material/folder.svg")),
            (
                "folder-open",
                include_bytes!("../../assets/material/folder-open.svg"),
            ),
            ("root", include_bytes!("../../assets/material/root.svg")),
            (
                "root-open",
                include_bytes!("../../assets/material/root-open.svg"),
            ),
            ("java", include_bytes!("../../assets/material/java.svg")),
            ("maven", include_bytes!("../../assets/material/maven.svg")),
            ("gradle", include_bytes!("../../assets/material/gradle.svg")),
            ("yaml", include_bytes!("../../assets/material/yaml.svg")),
            ("rust", include_bytes!("../../assets/material/rust.svg")),
            ("toml", include_bytes!("../../assets/material/toml.svg")),
            ("c", include_bytes!("../../assets/material/c.svg")),
            (
                "c-header",
                include_bytes!("../../assets/material/c-header.svg"),
            ),
            ("cpp", include_bytes!("../../assets/material/cpp.svg")),
            (
                "cpp-header",
                include_bytes!("../../assets/material/cpp-header.svg"),
            ),
            ("csharp", include_bytes!("../../assets/material/csharp.svg")),
            (
                "markdown",
                include_bytes!("../../assets/material/markdown.svg"),
            ),
            ("git", include_bytes!("../../assets/material/git.svg")),
            ("docker", include_bytes!("../../assets/material/docker.svg")),
            (
                "license",
                include_bytes!("../../assets/material/license.svg"),
            ),
            (
                "folder-src",
                include_bytes!("../../assets/material/folder-src.svg"),
            ),
            (
                "folder-src-open",
                include_bytes!("../../assets/material/folder-src-open.svg"),
            ),
        ];
        Self {
            data: BuiltinThemeData::new(
                IconThemeMetadata::built_in(MATERIAL_ID, "Material", 0),
                standard_associations(&ids),
                assets,
            ),
        }
    }

    pub fn referenced_asset_ids(&self) -> Vec<IconAssetId> {
        self.data.associations.referenced_asset_ids()
    }
}

impl Default for MaterialIconTheme {
    fn default() -> Self {
        Self::new()
    }
}

impl IconThemeProvider for MaterialIconTheme {
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
