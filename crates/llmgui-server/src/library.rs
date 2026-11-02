//! Server-side wrapper around the model registry: scan roots, import files and
//! manage presets. All writes go through [`RegistryStore`] (lock + atomic).

use std::path::{Path, PathBuf};

use llmgui_core::paths;
use llmgui_core::{LlmguiError, Result};
use llmgui_models::library::{
    ModelEntry, PresetStore, Registry, RegistryStore, ScanReport, register_derived, scan,
};

pub struct LibraryState {
    pub store: RegistryStore,
    pub roots: Vec<PathBuf>,
}

impl LibraryState {
    pub fn new(home: PathBuf, extra_dirs: &[PathBuf]) -> Self {
        let mut roots = vec![home.join("models")];
        for dir in extra_dirs {
            if !roots.contains(dir) {
                roots.push(dir.clone());
            }
        }
        Self {
            store: RegistryStore::new(home),
            roots,
        }
    }

    pub fn from_env(extra_dirs: &[PathBuf]) -> Self {
        Self::new(paths::llmgui_home(), extra_dirs)
    }

    pub fn registry(&self) -> Result<Registry> {
        self.store.load_or_rebuild(&self.roots)
    }

    /// Force a fresh scan of all roots and persist the merged registry.
    pub fn refresh(&self) -> Result<(Registry, ScanReport)> {
        let mut report = ScanReport::default();
        let registry = self.store.update(|registry| {
            report = scan(registry, &self.roots)?;
            Ok(())
        })?;
        Ok((registry, report))
    }

    pub fn import(&self, path: &Path) -> Result<ModelEntry> {
        let mut entry = None;
        self.store.update(|registry| {
            entry = Some(llmgui_models::library::import_path(
                registry,
                &self.roots[0],
                path,
            )?);
            Ok(())
        })?;
        entry.ok_or_else(|| LlmguiError::Model("import failed".to_string()))
    }

    pub fn register_derived(
        &self,
        gguf: &Path,
        base_id: Option<&str>,
        adapter_id: Option<&str>,
    ) -> Result<ModelEntry> {
        let mut entry = None;
        self.store.update(|registry| {
            entry = Some(register_derived(registry, gguf, base_id, adapter_id)?);
            Ok(())
        })?;
        entry.ok_or_else(|| LlmguiError::Model("registration failed".to_string()))
    }

    pub fn set_alias(&self, id: &str, alias: Option<String>) -> Result<Registry> {
        self.store.update(|registry| registry.set_alias(id, alias))
    }

    pub fn set_defaults(
        &self,
        id: &str,
        defaults: llmgui_models::library::LoadDefaults,
    ) -> Result<Registry> {
        self.store.update(|registry| {
            if registry.get(id).is_none() {
                return Err(LlmguiError::NotFound(format!("model {id}")));
            }
            registry
                .prefs
                .entry(id.to_string())
                .or_default()
                .load_defaults = defaults;
            Ok(())
        })
    }

    pub fn remove(&self, id: &str, purge: bool) -> Result<Removal> {
        let mut removal = None;
        self.store.update(|registry| {
            let entry = registry
                .get(id)
                .cloned()
                .ok_or_else(|| LlmguiError::NotFound(format!("model {id}")))?;
            let dependents = registry.dependents(id);
            if !dependents.is_empty() {
                return Err(LlmguiError::Conflict(format!(
                    "model {id} is referenced by: {}",
                    dependents.join(", ")
                )));
            }
            registry.models.retain(|model| model.id != id);
            registry.prefs.remove(id);
            removal = Some((entry, purge));
            Ok(())
        })?;
        let (entry, purge) = removal.expect("removal set");
        let mut files_removed = false;
        if matches!(entry.location, llmgui_models::library::Location::Managed) {
            let models_root = self.roots[0].clone();
            let target = entry.path.clone();
            if target.starts_with(&models_root) && target != models_root && target.exists() {
                std::fs::remove_dir_all(&target)?;
                files_removed = true;
            }
        }
        let _ = std::fs::remove_file(
            paths::llmgui_home()
                .join("presets")
                .join(format!("{id}.json")),
        );
        Ok(Removal {
            entry,
            files_removed,
            purge,
        })
    }

    pub fn presets(&self) -> PresetStore {
        PresetStore::new(self.store.home())
    }
}

pub struct Removal {
    pub entry: ModelEntry,
    pub files_removed: bool,
    pub purge: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_scans_and_persists() {
        let home = tempfile::tempdir().unwrap();
        let models = home.path().join("models").join("m");
        std::fs::create_dir_all(&models).unwrap();
        llmgui_models::write_test_gguf(&models.join("Tiny-Q4_K_M.gguf"), "qwen2", 4, 4, 2, 512)
            .unwrap();
        let library = LibraryState::new(home.path().to_path_buf(), &[]);
        let (registry, report) = library.refresh().unwrap();
        assert_eq!(report.added, 1);
        assert_eq!(registry.models.len(), 1);
        let (registry, _) = library.refresh().unwrap();
        assert_eq!(registry.models.len(), 1);
        let id = registry.models[0].id.clone();
        library.set_alias(&id, Some("tiny".into())).unwrap();
        assert_eq!(
            library.registry().unwrap().resolve("tiny"),
            llmgui_models::library::Resolve::One(id.clone())
        );
        let removal = library.remove(&id, false).unwrap();
        assert!(removal.files_removed);
        assert!(library.registry().unwrap().models.is_empty());
    }
}
