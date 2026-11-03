use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};

use crate::limits::{
    MAX_ASSETS, MAX_ASSOCIATIONS, MAX_MANIFEST_BYTES, MAX_PACK_BYTES, MAX_SVG_BYTES,
};
use crate::{
    FileIconContext, IconAsset, IconAssetId, IconColorMode, IconThemeAssociations, IconThemeError,
    IconThemeMetadata, IconThemeProvider,
};

#[derive(Debug)]
pub struct ManifestIconTheme {
    metadata: IconThemeMetadata,
    dark: IconThemeAssociations,
    light: IconThemeAssociations,
    assets: HashMap<IconAssetId, IconAsset>,
}

impl ManifestIconTheme {
    pub fn load(
        root: impl AsRef<Path>,
        manifest: impl AsRef<Path>,
    ) -> Result<Self, IconThemeError> {
        let root = canonicalize(root.as_ref())?;
        let manifest_path = resolve_manifest_path(&root, manifest.as_ref())?;
        let manifest_bytes = read_bounded(&manifest_path, MAX_MANIFEST_BYTES, ReadKind::Manifest)?;
        let manifest_text =
            std::str::from_utf8(&manifest_bytes).map_err(|_| IconThemeError::InvalidUtf8 {
                path: manifest_path.clone(),
            })?;
        let raw: RawManifest = serde_json::from_str(manifest_text)
            .map_err(|error| IconThemeError::InvalidManifest(error.to_string()))?;

        enforce_counts(raw.icon_definitions.len(), raw.association_count())?;
        let mut pack_bytes = manifest_bytes.len();
        let mut assets = HashMap::with_capacity(raw.icon_definitions.len());
        for (definition_id, definition) in &raw.icon_definitions {
            let asset_path = resolve_asset_path(&root, &definition.icon_path)?;
            let bytes = read_bounded(&asset_path, MAX_SVG_BYTES, ReadKind::Svg)?;
            pack_bytes =
                pack_bytes
                    .checked_add(bytes.len())
                    .ok_or(IconThemeError::PackTooLarge {
                        actual: usize::MAX,
                        max: MAX_PACK_BYTES,
                    })?;
            enforce_pack_bytes(pack_bytes)?;
            validate_svg(&asset_path, &bytes)?;
            assets.insert(
                IconAssetId::from(definition_id.clone()),
                IconAsset::new(bytes, false),
            );
        }

        let dark = raw.dark_associations()?;
        let mut light = dark.clone();
        if let Some(overrides) = &raw.light {
            overrides.apply(&mut light);
        }
        validate_references(&dark, &raw)?;
        if let Some(overrides) = &raw.light {
            validate_association_fields(overrides, &raw.icon_definitions)?;
        }

        let folder_name = root
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("icon-pack");
        let stable_name = folder_name
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                    character.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect::<String>();

        Ok(Self {
            metadata: IconThemeMetadata::external(
                format!("external:{stable_name}"),
                folder_name.to_owned(),
            ),
            dark,
            light,
            assets,
        })
    }
}

impl IconThemeProvider for ManifestIconTheme {
    fn metadata(&self) -> &IconThemeMetadata {
        &self.metadata
    }

    fn resolve(&self, context: &FileIconContext) -> IconAssetId {
        match context.color_mode {
            IconColorMode::Dark => self.dark.resolve(context),
            IconColorMode::Light => self.light.resolve(context),
        }
    }

    fn asset(&self, id: &IconAssetId) -> Result<IconAsset, IconThemeError> {
        self.assets
            .get(id)
            .cloned()
            .ok_or_else(|| IconThemeError::MissingAsset(id.clone()))
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawManifest {
    #[serde(deserialize_with = "deserialize_unique_map")]
    icon_definitions: HashMap<String, RawDefinition>,
    file: String,
    folder: String,
    folder_expanded: String,
    root_folder: String,
    root_folder_expanded: String,
    #[serde(default, deserialize_with = "deserialize_unique_map")]
    file_extensions: HashMap<String, String>,
    #[serde(default, deserialize_with = "deserialize_unique_map")]
    file_names: HashMap<String, String>,
    #[serde(default, deserialize_with = "deserialize_unique_map")]
    folder_names: HashMap<String, String>,
    #[serde(default, deserialize_with = "deserialize_unique_map")]
    folder_names_expanded: HashMap<String, String>,
    #[serde(default, deserialize_with = "deserialize_unique_map")]
    root_folder_names: HashMap<String, String>,
    #[serde(default, deserialize_with = "deserialize_unique_map")]
    root_folder_names_expanded: HashMap<String, String>,
    #[serde(default, deserialize_with = "deserialize_unique_map")]
    language_ids: HashMap<String, String>,
    light: Option<RawAssociationFields>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawDefinition {
    icon_path: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawAssociationFields {
    file: Option<String>,
    folder: Option<String>,
    folder_expanded: Option<String>,
    root_folder: Option<String>,
    root_folder_expanded: Option<String>,
    #[serde(default, deserialize_with = "deserialize_unique_map")]
    file_extensions: HashMap<String, String>,
    #[serde(default, deserialize_with = "deserialize_unique_map")]
    file_names: HashMap<String, String>,
    #[serde(default, deserialize_with = "deserialize_unique_map")]
    folder_names: HashMap<String, String>,
    #[serde(default, deserialize_with = "deserialize_unique_map")]
    folder_names_expanded: HashMap<String, String>,
    #[serde(default, deserialize_with = "deserialize_unique_map")]
    root_folder_names: HashMap<String, String>,
    #[serde(default, deserialize_with = "deserialize_unique_map")]
    root_folder_names_expanded: HashMap<String, String>,
    #[serde(default, deserialize_with = "deserialize_unique_map")]
    language_ids: HashMap<String, String>,
}

impl RawManifest {
    fn association_count(&self) -> usize {
        5 + self.file_extensions.len()
            + self.file_names.len()
            + self.folder_names.len()
            + self.folder_names_expanded.len()
            + self.root_folder_names.len()
            + self.root_folder_names_expanded.len()
            + self.language_ids.len()
            + self
                .light
                .as_ref()
                .map(RawAssociationFields::association_count)
                .unwrap_or(0)
    }

    fn dark_associations(&self) -> Result<IconThemeAssociations, IconThemeError> {
        validate_manifest_references(self)?;
        let mut associations = IconThemeAssociations::new(
            self.file.as_str().into(),
            self.folder.as_str().into(),
            self.folder_expanded.as_str().into(),
            self.root_folder.as_str().into(),
            self.root_folder_expanded.as_str().into(),
        );
        apply_maps(
            &mut associations,
            &self.file_extensions,
            &self.file_names,
            &self.folder_names,
            &self.folder_names_expanded,
            &self.root_folder_names,
            &self.root_folder_names_expanded,
            &self.language_ids,
        );
        Ok(associations)
    }
}

impl RawAssociationFields {
    fn association_count(&self) -> usize {
        [
            self.file.as_ref(),
            self.folder.as_ref(),
            self.folder_expanded.as_ref(),
            self.root_folder.as_ref(),
            self.root_folder_expanded.as_ref(),
        ]
        .into_iter()
        .flatten()
        .count()
            + self.file_extensions.len()
            + self.file_names.len()
            + self.folder_names.len()
            + self.folder_names_expanded.len()
            + self.root_folder_names.len()
            + self.root_folder_names_expanded.len()
            + self.language_ids.len()
    }

    fn apply(&self, associations: &mut IconThemeAssociations) {
        if let Some(id) = &self.file {
            associations.set_generic_file(id.as_str().into());
        }
        for (is_root, is_open, id) in [
            (false, false, self.folder.as_ref()),
            (false, true, self.folder_expanded.as_ref()),
            (true, false, self.root_folder.as_ref()),
            (true, true, self.root_folder_expanded.as_ref()),
        ] {
            if let Some(id) = id {
                associations.set_generic_folder(is_root, is_open, id.as_str().into());
            }
        }
        apply_maps(
            associations,
            &self.file_extensions,
            &self.file_names,
            &self.folder_names,
            &self.folder_names_expanded,
            &self.root_folder_names,
            &self.root_folder_names_expanded,
            &self.language_ids,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_maps(
    associations: &mut IconThemeAssociations,
    file_extensions: &HashMap<String, String>,
    file_names: &HashMap<String, String>,
    folder_names: &HashMap<String, String>,
    folder_names_expanded: &HashMap<String, String>,
    root_folder_names: &HashMap<String, String>,
    root_folder_names_expanded: &HashMap<String, String>,
    language_ids: &HashMap<String, String>,
) {
    for (key, value) in file_extensions {
        associations.insert_file_extension(key, value.as_str().into());
    }
    for (key, value) in file_names {
        associations.insert_file_name(key, value.as_str().into());
    }
    for (key, value) in folder_names {
        associations.insert_folder_name(key, value.as_str().into());
    }
    for (key, value) in folder_names_expanded {
        associations.insert_open_folder_name(key, value.as_str().into());
    }
    for (key, value) in root_folder_names {
        associations.insert_root_folder_name(key, value.as_str().into());
    }
    for (key, value) in root_folder_names_expanded {
        associations.insert_open_root_folder_name(key, value.as_str().into());
    }
    for (key, value) in language_ids {
        associations.insert_language_id(key, value.as_str().into());
    }
}

fn validate_manifest_references(raw: &RawManifest) -> Result<(), IconThemeError> {
    for id in [
        &raw.file,
        &raw.folder,
        &raw.folder_expanded,
        &raw.root_folder,
        &raw.root_folder_expanded,
    ] {
        ensure_definition(id, &raw.icon_definitions)?;
    }
    validate_maps(
        [
            &raw.file_extensions,
            &raw.file_names,
            &raw.folder_names,
            &raw.folder_names_expanded,
            &raw.root_folder_names,
            &raw.root_folder_names_expanded,
            &raw.language_ids,
        ],
        &raw.icon_definitions,
    )
}

fn validate_references(
    _associations: &IconThemeAssociations,
    raw: &RawManifest,
) -> Result<(), IconThemeError> {
    validate_manifest_references(raw)
}

fn validate_association_fields(
    fields: &RawAssociationFields,
    definitions: &HashMap<String, RawDefinition>,
) -> Result<(), IconThemeError> {
    for id in [
        fields.file.as_ref(),
        fields.folder.as_ref(),
        fields.folder_expanded.as_ref(),
        fields.root_folder.as_ref(),
        fields.root_folder_expanded.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        ensure_definition(id, definitions)?;
    }
    validate_maps(
        [
            &fields.file_extensions,
            &fields.file_names,
            &fields.folder_names,
            &fields.folder_names_expanded,
            &fields.root_folder_names,
            &fields.root_folder_names_expanded,
            &fields.language_ids,
        ],
        definitions,
    )
}

fn validate_maps<const N: usize>(
    maps: [&HashMap<String, String>; N],
    definitions: &HashMap<String, RawDefinition>,
) -> Result<(), IconThemeError> {
    for id in maps.into_iter().flat_map(|map| map.values()) {
        ensure_definition(id, definitions)?;
    }
    Ok(())
}

fn ensure_definition(
    id: &str,
    definitions: &HashMap<String, RawDefinition>,
) -> Result<(), IconThemeError> {
    if definitions.contains_key(id) {
        Ok(())
    } else {
        Err(IconThemeError::MissingDefinition(id.to_owned()))
    }
}

fn resolve_manifest_path(root: &Path, manifest: &Path) -> Result<PathBuf, IconThemeError> {
    let candidate = if manifest.is_absolute() {
        manifest.to_owned()
    } else {
        root.join(manifest)
    };
    let canonical = canonicalize(&candidate)?;
    ensure_contained(root, &canonical)?;
    Ok(canonical)
}

fn resolve_asset_path(root: &Path, raw: &str) -> Result<PathBuf, IconThemeError> {
    let lowercase = raw.to_ascii_lowercase();
    if lowercase.contains("://") || lowercase.starts_with("data:") || lowercase.starts_with("file:")
    {
        return Err(IconThemeError::RemoteAssetPath(raw.to_owned()));
    }
    let relative = Path::new(raw);
    if relative.is_absolute() {
        return Err(IconThemeError::AbsoluteAssetPath(raw.to_owned()));
    }
    if relative.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(IconThemeError::PathEscape {
            path: relative.to_owned(),
        });
    }
    let canonical = canonicalize(&root.join(relative))?;
    ensure_contained(root, &canonical)?;
    Ok(canonical)
}

fn canonicalize(path: &Path) -> Result<PathBuf, IconThemeError> {
    path.canonicalize().map_err(|error| IconThemeError::Io {
        path: path.to_owned(),
        message: error.to_string(),
    })
}

fn ensure_contained(root: &Path, path: &Path) -> Result<(), IconThemeError> {
    if path.starts_with(root) {
        Ok(())
    } else {
        Err(IconThemeError::PathEscape {
            path: path.to_owned(),
        })
    }
}

enum ReadKind {
    Manifest,
    Svg,
}

fn read_bounded(path: &Path, max: usize, kind: ReadKind) -> Result<Vec<u8>, IconThemeError> {
    let file = File::open(path).map_err(|error| IconThemeError::Io {
        path: path.to_owned(),
        message: error.to_string(),
    })?;
    let mut bytes = Vec::with_capacity(max.min(64 * 1024));
    file.take((max + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| IconThemeError::Io {
            path: path.to_owned(),
            message: error.to_string(),
        })?;
    if bytes.len() > max {
        return Err(match kind {
            ReadKind::Manifest => IconThemeError::ManifestTooLarge {
                actual: bytes.len(),
                max,
            },
            ReadKind::Svg => IconThemeError::SvgTooLarge {
                path: path.to_owned(),
                actual: bytes.len(),
                max,
            },
        });
    }
    Ok(bytes)
}

fn validate_svg(path: &Path, bytes: &[u8]) -> Result<(), IconThemeError> {
    let source = std::str::from_utf8(bytes).map_err(|_| IconThemeError::InvalidUtf8 {
        path: path.to_owned(),
    })?;
    let lowercase = source.to_ascii_lowercase();
    for forbidden in [
        "<!doctype",
        "<!entity",
        "<script",
        "<foreignobject",
        "@font-face",
        "@import",
        "<?xml-stylesheet",
        "file://",
        "data:",
    ] {
        if lowercase.contains(forbidden) {
            return Err(IconThemeError::ForbiddenSvg {
                path: path.to_owned(),
                reason: forbidden.to_owned(),
            });
        }
    }
    if contains_external_url(&lowercase) {
        return Err(IconThemeError::ForbiddenSvg {
            path: path.to_owned(),
            reason: "external CSS url".to_owned(),
        });
    }

    let document =
        roxmltree::Document::parse(source).map_err(|error| IconThemeError::InvalidSvg {
            path: path.to_owned(),
            message: error.to_string(),
        })?;
    if document.root_element().tag_name().name() != "svg" {
        return Err(IconThemeError::InvalidSvg {
            path: path.to_owned(),
            message: "root element is not svg".to_owned(),
        });
    }
    for node in document.descendants().filter(roxmltree::Node::is_element) {
        let tag = node.tag_name().name().to_ascii_lowercase();
        if matches!(
            tag.as_str(),
            "script"
                | "foreignobject"
                | "image"
                | "font"
                | "animate"
                | "animatemotion"
                | "animatetransform"
                | "set"
        ) {
            return Err(IconThemeError::ForbiddenSvg {
                path: path.to_owned(),
                reason: format!("element <{tag}>"),
            });
        }
        for attribute in node.attributes() {
            let name = attribute.name().to_ascii_lowercase();
            let value = attribute.value().trim().to_ascii_lowercase();
            if name.starts_with("on")
                || ((name == "href" || name.ends_with(":href")) && !value.starts_with('#'))
                || (value.contains("url(") && !value.contains("url(#"))
            {
                return Err(IconThemeError::ForbiddenSvg {
                    path: path.to_owned(),
                    reason: format!("attribute {}", attribute.name()),
                });
            }
        }
    }
    Ok(())
}

fn contains_external_url(source: &str) -> bool {
    let mut remaining = source;
    while let Some(index) = remaining.find("url(") {
        remaining = &remaining[index + 4..];
        let target = remaining.trim_start_matches(|character: char| {
            character.is_ascii_whitespace() || matches!(character, '\'' | '"')
        });
        if !target.starts_with('#') {
            return true;
        }
    }
    false
}

fn enforce_counts(asset_count: usize, association_count: usize) -> Result<(), IconThemeError> {
    if asset_count > MAX_ASSETS {
        return Err(IconThemeError::TooManyAssets {
            actual: asset_count,
            max: MAX_ASSETS,
        });
    }
    if association_count > MAX_ASSOCIATIONS {
        return Err(IconThemeError::TooManyAssociations {
            actual: association_count,
            max: MAX_ASSOCIATIONS,
        });
    }
    Ok(())
}

fn enforce_pack_bytes(bytes: usize) -> Result<(), IconThemeError> {
    if bytes > MAX_PACK_BYTES {
        Err(IconThemeError::PackTooLarge {
            actual: bytes,
            max: MAX_PACK_BYTES,
        })
    } else {
        Ok(())
    }
}

fn deserialize_unique_map<'de, D, V>(deserializer: D) -> Result<HashMap<String, V>, D::Error>
where
    D: Deserializer<'de>,
    V: Deserialize<'de>,
{
    struct UniqueMapVisitor<V>(std::marker::PhantomData<V>);

    impl<'de, V> Visitor<'de> for UniqueMapVisitor<V>
    where
        V: Deserialize<'de>,
    {
        type Value = HashMap<String, V>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("an object with unique keys")
        }

        fn visit_map<A>(self, mut access: A) -> Result<Self::Value, A::Error>
        where
            A: MapAccess<'de>,
        {
            let mut values = HashMap::with_capacity(access.size_hint().unwrap_or(0));
            while let Some((key, value)) = access.next_entry::<String, V>()? {
                if values.insert(key.clone(), value).is_some() {
                    return Err(de::Error::custom(format!("duplicate key: {key}")));
                }
            }
            Ok(values)
        }
    }

    deserializer.deserialize_map(UniqueMapVisitor(std::marker::PhantomData))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::{enforce_counts, enforce_pack_bytes};
    use crate::IconThemeError;
    use crate::limits::{MAX_ASSETS, MAX_ASSOCIATIONS, MAX_PACK_BYTES};

    #[test]
    fn exact_count_and_total_pack_bounds_fail_closed() {
        assert!(enforce_counts(MAX_ASSETS, MAX_ASSOCIATIONS).is_ok());
        assert!(matches!(
            enforce_counts(MAX_ASSETS + 1, 0),
            Err(IconThemeError::TooManyAssets { .. })
        ));
        assert!(matches!(
            enforce_counts(0, MAX_ASSOCIATIONS + 1),
            Err(IconThemeError::TooManyAssociations { .. })
        ));
        assert!(enforce_pack_bytes(MAX_PACK_BYTES).is_ok());
        assert!(matches!(
            enforce_pack_bytes(MAX_PACK_BYTES + 1),
            Err(IconThemeError::PackTooLarge { .. })
        ));
    }
}
