use crate::{
    FileIconContext, IconAsset, IconAssetId, IconThemeAssociations, IconThemeError,
    IconThemeMetadata, IconThemeProvider,
};

pub const LUME_MONO_ID: &str = "lume-mono";

const FILE: &str = "file";
const FOLDER: &str = "folder";
const FOLDER_OPEN: &str = "folder-open";
const ROOT_FOLDER: &str = "root-folder";
const ROOT_FOLDER_OPEN: &str = "root-folder-open";

const FILE_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path fill="currentColor" d="M3 1h6l4 4v10H3zm6 1.5V6h3.5z"/></svg>"#;
const FOLDER_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path fill="currentColor" d="M1 3h5l1.5 2H15v9H1z"/></svg>"#;
const FOLDER_OPEN_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path fill="currentColor" d="M1 3h5l1.5 2H15v2H4l-2 7H0zM4.7 8H16l-2 6H3z"/></svg>"#;
const ROOT_FOLDER_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path fill="currentColor" d="M1 2h6l1.5 2H15v10H1zM7 6v2H5v2h2v2h2v-2h2V8H9V6z"/></svg>"#;
const ROOT_FOLDER_OPEN_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path fill="currentColor" d="M1 2h6l1.5 2H15v2H4l-2 8H0zm3.7 5H16l-2 7H3zm2.8 1v2h-2v1h2v2h1v-2h2v-1h-2V8z"/></svg>"#;

pub struct LumeMonoIconTheme {
    metadata: IconThemeMetadata,
    associations: IconThemeAssociations,
}

impl LumeMonoIconTheme {
    pub fn new() -> Self {
        Self {
            metadata: IconThemeMetadata::built_in(LUME_MONO_ID, "Lume Monochrome", 2),
            associations: IconThemeAssociations::new(
                FILE.into(),
                FOLDER.into(),
                FOLDER_OPEN.into(),
                ROOT_FOLDER.into(),
                ROOT_FOLDER_OPEN.into(),
            ),
        }
    }

    pub fn generic_file(&self) -> &IconAssetId {
        self.associations.generic_file()
    }
}

impl Default for LumeMonoIconTheme {
    fn default() -> Self {
        Self::new()
    }
}

impl IconThemeProvider for LumeMonoIconTheme {
    fn metadata(&self) -> &IconThemeMetadata {
        &self.metadata
    }

    fn resolve(&self, context: &FileIconContext) -> IconAssetId {
        self.associations.resolve(context)
    }

    fn asset(&self, id: &IconAssetId) -> Result<IconAsset, IconThemeError> {
        let bytes = match id.as_str() {
            FILE => FILE_SVG,
            FOLDER => FOLDER_SVG,
            FOLDER_OPEN => FOLDER_OPEN_SVG,
            ROOT_FOLDER => ROOT_FOLDER_SVG,
            ROOT_FOLDER_OPEN => ROOT_FOLDER_OPEN_SVG,
            _ => return Err(IconThemeError::MissingAsset(id.clone())),
        };
        Ok(IconAsset::from_static(bytes, true))
    }
}
