use std::collections::HashMap;

use crate::{
    CatppuccinIconTheme, IconThemeError, IconThemeProvider, LumeDotIconTheme, LumeMonoIconTheme,
    MaterialIconTheme,
};

pub struct IconThemeRegistry {
    providers: HashMap<String, Box<dyn IconThemeProvider>>,
    display_order: Vec<String>,
    recovery_id: String,
    active_id: String,
}

impl IconThemeRegistry {
    pub fn with_builtins() -> Self {
        let providers: HashMap<String, Box<dyn IconThemeProvider>> = [
            Box::new(MaterialIconTheme::new()) as Box<dyn IconThemeProvider>,
            Box::new(CatppuccinIconTheme::new()) as Box<dyn IconThemeProvider>,
            Box::new(LumeMonoIconTheme::new()) as Box<dyn IconThemeProvider>,
            Box::new(LumeDotIconTheme::new()) as Box<dyn IconThemeProvider>,
        ]
        .into_iter()
        .map(|provider| (provider.metadata().id.clone(), provider))
        .collect();
        let mut registry = Self {
            providers,
            display_order: Vec::new(),
            recovery_id: "lume-mono".to_owned(),
            active_id: "material".to_owned(),
        };
        registry.rebuild_display_order();
        registry
    }

    pub fn with_recovery(
        recovery: impl IconThemeProvider + 'static,
    ) -> Result<Self, IconThemeError> {
        let recovery_id = recovery.metadata().id.clone();
        if recovery_id.trim().is_empty() {
            return Err(IconThemeError::EmptyProviderId);
        }
        let mut registry = Self {
            providers: HashMap::new(),
            display_order: Vec::new(),
            recovery_id: recovery_id.clone(),
            active_id: recovery_id,
        };
        registry.register(recovery)?;
        Ok(registry)
    }

    pub fn register(
        &mut self,
        provider: impl IconThemeProvider + 'static,
    ) -> Result<(), IconThemeError> {
        let id = provider.metadata().id.clone();
        if id.trim().is_empty() {
            return Err(IconThemeError::EmptyProviderId);
        }
        if self.providers.contains_key(&id) {
            return Err(IconThemeError::DuplicateId(id));
        }
        self.providers.insert(id, Box::new(provider));
        self.rebuild_display_order();
        Ok(())
    }

    pub fn activate(&mut self, id: &str) -> Result<(), IconThemeError> {
        if !self.providers.contains_key(id) {
            return Err(IconThemeError::UnknownProvider(id.to_owned()));
        }
        self.active_id = id.to_owned();
        Ok(())
    }

    pub fn recover(&mut self) {
        self.active_id.clone_from(&self.recovery_id);
    }

    pub fn active_id(&self) -> &str {
        &self.active_id
    }

    #[allow(clippy::indexing_slicing)] // active_id è sempre una chiave di providers (invariante del registry)
    pub fn active(&self) -> &dyn IconThemeProvider {
        self.providers[&self.active_id].as_ref()
    }

    pub fn provider(&self, id: &str) -> Option<&dyn IconThemeProvider> {
        self.providers.get(id).map(Box::as_ref)
    }

    pub fn providers(&self) -> impl Iterator<Item = &dyn IconThemeProvider> {
        self.display_order
            .iter()
            .filter_map(|id| self.providers.get(id).map(Box::as_ref))
    }

    #[allow(clippy::indexing_slicing)] // le chiavi vengono da providers.keys()
    fn rebuild_display_order(&mut self) {
        let mut order = self.providers.keys().cloned().collect::<Vec<_>>();
        order.sort_by(|left, right| {
            let left = self.providers[left].metadata();
            let right = self.providers[right].metadata();
            left.built_in_rank
                .unwrap_or(u16::MAX)
                .cmp(&right.built_in_rank.unwrap_or(u16::MAX))
                .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
                .then_with(|| left.id.cmp(&right.id))
        });
        self.display_order = order;
    }
}
