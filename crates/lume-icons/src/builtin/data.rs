use std::collections::HashMap;

use crate::{
    FileIconContext, IconAsset, IconAssetId, IconThemeAssociations, IconThemeError,
    IconThemeMetadata,
};

pub struct StandardAssetIds {
    pub file: &'static str,
    pub folder: &'static str,
    pub folder_open: &'static str,
    pub root: &'static str,
    pub root_open: &'static str,
    pub java: &'static str,
    pub java_build: &'static str,
    pub gradle: &'static str,
    pub spring_config: &'static str,
    pub rust: &'static str,
    pub rust_manifest: &'static str,
    pub toml: &'static str,
    pub c: &'static str,
    pub c_header: &'static str,
    pub cpp: &'static str,
    pub cpp_header: &'static str,
    pub csharp: &'static str,
    pub markdown: &'static str,
    pub git: &'static str,
    pub docker: &'static str,
    pub license: &'static str,
    pub folder_src: &'static str,
    pub folder_src_open: &'static str,
    pub folder_git: &'static str,
    pub folder_git_open: &'static str,
}

pub fn standard_associations(ids: &StandardAssetIds) -> IconThemeAssociations {
    let mut associations = IconThemeAssociations::new(
        ids.file.into(),
        ids.folder.into(),
        ids.folder_open.into(),
        ids.root.into(),
        ids.root_open.into(),
    );

    for name in ["pom.xml", "mvnw", "mvnw.cmd"] {
        associations.insert_file_name(name, ids.java_build.into());
    }
    for name in ["build.gradle", "settings.gradle", "gradlew", "gradlew.bat"] {
        associations.insert_file_name(name, ids.gradle.into());
    }
    for name in [
        "application.yml",
        "application.yaml",
        "application.properties",
        "bootstrap.yml",
        "bootstrap.yaml",
        "bootstrap.properties",
    ] {
        associations.insert_file_name(name, ids.spring_config.into());
    }
    for name in ["Cargo.toml", "Cargo.lock"] {
        associations.insert_file_name(name, ids.rust_manifest.into());
    }
    for name in ["README", "README.md", "CHANGELOG.md"] {
        associations.insert_file_name(name, ids.markdown.into());
    }
    for name in [".gitignore", ".gitattributes", ".gitmodules"] {
        associations.insert_file_name(name, ids.git.into());
    }
    for name in ["Dockerfile", ".dockerignore", "docker-compose.yml"] {
        associations.insert_file_name(name, ids.docker.into());
    }
    for name in ["LICENSE", "LICENSE.md", "COPYING"] {
        associations.insert_file_name(name, ids.license.into());
    }

    for (extensions, asset) in [
        (&["java", "class", "jar"][..], ids.java),
        (&["gradle"][..], ids.gradle),
        (&["yml", "yaml", "properties"][..], ids.spring_config),
        (&["rs"][..], ids.rust),
        (&["toml"][..], ids.toml),
        (&["c"][..], ids.c),
        (&["h"][..], ids.c_header),
        (&["cpp", "cc", "cxx"][..], ids.cpp),
        (&["hpp", "hh", "hxx"][..], ids.cpp_header),
        (&["cs", "csx"][..], ids.csharp),
        (&["md", "markdown", "mdx"][..], ids.markdown),
    ] {
        for extension in extensions {
            associations.insert_file_extension(extension, asset.into());
        }
    }

    for (languages, asset) in [
        (&["java"][..], ids.java),
        (&["yaml", "properties"][..], ids.spring_config),
        (&["rust"][..], ids.rust),
        (&["toml"][..], ids.toml),
        (&["c"][..], ids.c),
        (&["cpp"][..], ids.cpp),
        (&["csharp"][..], ids.csharp),
        (&["markdown"][..], ids.markdown),
    ] {
        for language in languages {
            associations.insert_language_id(language, asset.into());
        }
    }

    associations.insert_folder_name("src", ids.folder_src.into());
    associations.insert_open_folder_name("src", ids.folder_src_open.into());
    associations.insert_folder_name(".git", ids.folder_git.into());
    associations.insert_open_folder_name(".git", ids.folder_git_open.into());
    associations
}

#[derive(Debug)]
pub struct BuiltinThemeData {
    pub metadata: IconThemeMetadata,
    pub associations: IconThemeAssociations,
    pub assets: HashMap<IconAssetId, IconAsset>,
}

impl BuiltinThemeData {
    pub fn new(
        metadata: IconThemeMetadata,
        associations: IconThemeAssociations,
        assets: impl IntoIterator<Item = (&'static str, &'static [u8])>,
    ) -> Self {
        Self {
            metadata,
            associations,
            assets: assets
                .into_iter()
                .map(|(id, bytes)| (id.into(), IconAsset::from_static(bytes, false)))
                .collect(),
        }
    }

    pub fn resolve(&self, context: &FileIconContext) -> IconAssetId {
        self.associations.resolve(context)
    }

    pub fn asset(&self, id: &IconAssetId) -> Result<IconAsset, IconThemeError> {
        self.assets
            .get(id)
            .cloned()
            .ok_or_else(|| IconThemeError::MissingAsset(id.clone()))
    }
}
