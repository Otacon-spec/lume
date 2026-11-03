//! «Lume Dot»: il tema del prototipo. Ogni file è un quadratino 8×8 con
//! raggio 2 — colore `num` per i sorgenti Rust, `faint` per il resto —
//! e le cartelle non hanno icona (solo la freccia ▸/▾ dell'albero).
//! Il colore effettivo lo sceglie la UI dal ruolo di tinta (`TintRole`).

use crate::{
    FileIconContext, IconAsset, IconAssetId, IconThemeError, IconThemeMetadata, IconThemeProvider,
    TintRole,
};

pub const LUME_DOT_ID: &str = "lume-dot";

pub const DOT_CODE: &str = "dot-code";
pub const DOT_FILE: &str = "dot-file";
pub const NONE: &str = "none";

const DOT_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><rect x="4" y="4" width="8" height="8" rx="2" fill="currentColor"/></svg>"#;
const NONE_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"/>"#;

pub struct LumeDotIconTheme {
    metadata: IconThemeMetadata,
}

impl LumeDotIconTheme {
    pub fn new() -> Self {
        Self {
            metadata: IconThemeMetadata::built_in(LUME_DOT_ID, "Lume Dot", 3),
        }
    }
}

impl Default for LumeDotIconTheme {
    fn default() -> Self {
        Self::new()
    }
}

/// Estensioni trattate come «codice» (dot `num`), come nel prototipo.
fn is_code(display_name: &str) -> bool {
    std::path::Path::new(display_name)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("rs"))
}

impl IconThemeProvider for LumeDotIconTheme {
    fn metadata(&self) -> &IconThemeMetadata {
        &self.metadata
    }

    fn resolve(&self, context: &FileIconContext) -> IconAssetId {
        if context.is_directory {
            NONE.into()
        } else if is_code(&context.display_name) {
            DOT_CODE.into()
        } else {
            DOT_FILE.into()
        }
    }

    fn asset(&self, id: &IconAssetId) -> Result<IconAsset, IconThemeError> {
        match id.as_str() {
            DOT_CODE => Ok(IconAsset::from_static(DOT_SVG, true).with_tint_role(TintRole::Num)),
            DOT_FILE => Ok(IconAsset::from_static(DOT_SVG, true).with_tint_role(TintRole::Faint)),
            NONE => Ok(IconAsset::from_static(NONE_SVG, true)),
            _ => Err(IconThemeError::MissingAsset(id.clone())),
        }
    }
}
