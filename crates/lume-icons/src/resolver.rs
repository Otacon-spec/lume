use std::collections::HashMap;

use crate::{FileIconContext, IconAssetId, IconThemeProvider};

#[derive(Debug, Clone)]
pub struct IconThemeAssociations {
    file: IconAssetId,
    folder: IconAssetId,
    folder_open: IconAssetId,
    root_folder: IconAssetId,
    root_folder_open: IconAssetId,
    file_names: HashMap<String, IconAssetId>,
    file_extensions: HashMap<String, IconAssetId>,
    language_ids: HashMap<String, IconAssetId>,
    folder_names: HashMap<String, IconAssetId>,
    open_folder_names: HashMap<String, IconAssetId>,
    root_folder_names: HashMap<String, IconAssetId>,
    open_root_folder_names: HashMap<String, IconAssetId>,
}

impl IconThemeAssociations {
    pub fn new(
        file: IconAssetId,
        folder: IconAssetId,
        folder_open: IconAssetId,
        root_folder: IconAssetId,
        root_folder_open: IconAssetId,
    ) -> Self {
        Self {
            file,
            folder,
            folder_open,
            root_folder,
            root_folder_open,
            file_names: HashMap::new(),
            file_extensions: HashMap::new(),
            language_ids: HashMap::new(),
            folder_names: HashMap::new(),
            open_folder_names: HashMap::new(),
            root_folder_names: HashMap::new(),
            open_root_folder_names: HashMap::new(),
        }
    }

    pub fn generic_file(&self) -> &IconAssetId {
        &self.file
    }

    pub fn generic_folder(&self, is_root: bool, is_open: bool) -> &IconAssetId {
        match (is_root, is_open) {
            (true, true) => &self.root_folder_open,
            (true, false) => &self.root_folder,
            (false, true) => &self.folder_open,
            (false, false) => &self.folder,
        }
    }

    pub fn insert_file_name(&mut self, name: impl AsRef<str>, asset: IconAssetId) {
        self.file_names.insert(normalize(name.as_ref()), asset);
    }

    pub fn insert_file_extension(&mut self, extension: impl AsRef<str>, asset: IconAssetId) {
        let key = extension.as_ref().trim_start_matches('.');
        self.file_extensions.insert(normalize(key), asset);
    }

    pub fn insert_language_id(&mut self, language_id: impl AsRef<str>, asset: IconAssetId) {
        self.language_ids
            .insert(normalize(language_id.as_ref()), asset);
    }

    pub fn insert_folder_name(&mut self, name: impl AsRef<str>, asset: IconAssetId) {
        self.folder_names.insert(normalize(name.as_ref()), asset);
    }

    pub fn insert_open_folder_name(&mut self, name: impl AsRef<str>, asset: IconAssetId) {
        self.open_folder_names
            .insert(normalize(name.as_ref()), asset);
    }

    pub fn insert_root_folder_name(&mut self, name: impl AsRef<str>, asset: IconAssetId) {
        self.root_folder_names
            .insert(normalize(name.as_ref()), asset);
    }

    pub fn insert_open_root_folder_name(&mut self, name: impl AsRef<str>, asset: IconAssetId) {
        self.open_root_folder_names
            .insert(normalize(name.as_ref()), asset);
    }

    pub fn set_generic_file(&mut self, asset: IconAssetId) {
        self.file = asset;
    }

    pub fn set_generic_folder(&mut self, is_root: bool, is_open: bool, asset: IconAssetId) {
        match (is_root, is_open) {
            (true, true) => self.root_folder_open = asset,
            (true, false) => self.root_folder = asset,
            (false, true) => self.folder_open = asset,
            (false, false) => self.folder = asset,
        }
    }

    pub fn resolve(&self, context: &FileIconContext) -> IconAssetId {
        if context.is_directory {
            return self.resolve_folder(context);
        }

        let name = normalize(&context.display_name);
        if let Some(asset) = self.file_names.get(&name) {
            return asset.clone();
        }
        if let Some(asset) =
            compound_extensions(&name).find_map(|extension| self.file_extensions.get(extension))
        {
            return asset.clone();
        }
        if let Some(asset) = context
            .language_id
            .as_deref()
            .map(normalize)
            .as_deref()
            .and_then(|language_id| self.language_ids.get(language_id))
        {
            return asset.clone();
        }
        self.file.clone()
    }

    pub fn referenced_asset_ids(&self) -> Vec<IconAssetId> {
        let mut ids = vec![
            self.file.clone(),
            self.folder.clone(),
            self.folder_open.clone(),
            self.root_folder.clone(),
            self.root_folder_open.clone(),
        ];
        ids.extend(self.file_names.values().cloned());
        ids.extend(self.file_extensions.values().cloned());
        ids.extend(self.language_ids.values().cloned());
        ids.extend(self.folder_names.values().cloned());
        ids.extend(self.open_folder_names.values().cloned());
        ids.extend(self.root_folder_names.values().cloned());
        ids.extend(self.open_root_folder_names.values().cloned());
        ids.sort();
        ids.dedup();
        ids
    }

    fn resolve_folder(&self, context: &FileIconContext) -> IconAssetId {
        let name = normalize(&context.display_name);
        let (closed_names, open_names) = if context.is_root {
            (&self.root_folder_names, &self.open_root_folder_names)
        } else {
            (&self.folder_names, &self.open_folder_names)
        };
        if context.is_open
            && let Some(asset) = open_names.get(&name)
        {
            return asset.clone();
        }
        if let Some(asset) = closed_names.get(&name) {
            return asset.clone();
        }
        self.generic_folder(context.is_root, context.is_open)
            .clone()
    }
}

pub struct FileIconResolver<'a> {
    provider: &'a dyn IconThemeProvider,
}

impl<'a> FileIconResolver<'a> {
    pub fn new(provider: &'a dyn IconThemeProvider) -> Self {
        Self { provider }
    }

    pub fn resolve(&self, context: &FileIconContext) -> IconAssetId {
        self.provider.resolve(context)
    }
}

fn normalize(value: &str) -> String {
    value.to_lowercase()
}

fn compound_extensions(name: &str) -> impl Iterator<Item = &str> {
    name.char_indices()
        .filter(|(_, character)| *character == '.')
        .map(|(index, _)| &name[index + 1..])
        .filter(|extension| !extension.is_empty())
}
