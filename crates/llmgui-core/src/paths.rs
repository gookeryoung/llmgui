use std::path::PathBuf;

use crate::error::Result;

/// Root for all mutable llmgui state. Never defaults to the repository disk.
pub fn llmgui_home() -> PathBuf {
    if let Ok(value) = std::env::var("LLMGUI_HOME")
        && !value.trim().is_empty()
    {
        return PathBuf::from(value);
    }
    directories::ProjectDirs::from("dev", "llmgui", "llmgui")
        .map(|dirs| dirs.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".llmgui"))
}

pub fn config_path() -> PathBuf {
    llmgui_home().join("config.toml")
}

pub fn cache_dir() -> PathBuf {
    std::env::var("LLMGUI_CACHE_DIR")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| llmgui_home().join("cache"))
}

pub fn models_dir() -> PathBuf {
    llmgui_home().join("models")
}

pub fn adapters_dir() -> PathBuf {
    llmgui_home().join("adapters")
}

pub fn jobs_dir() -> PathBuf {
    llmgui_home().join("jobs")
}

pub fn registry_path() -> PathBuf {
    llmgui_home().join("registry.json")
}

pub fn catalog_dir() -> PathBuf {
    llmgui_home().join("catalog")
}

pub fn catalog_cache_path() -> PathBuf {
    catalog_dir().join("cache.toml")
}

pub fn catalog_state_path() -> PathBuf {
    catalog_dir().join("state.json")
}

pub fn ensure_layout() -> Result<()> {
    for dir in [
        llmgui_home(),
        cache_dir(),
        models_dir(),
        adapters_dir(),
        jobs_dir(),
        catalog_dir(),
    ] {
        std::fs::create_dir_all(dir)?;
    }
    Ok(())
}

/// Reject paths that escape `root` after canonicalisation. Non-existent files
/// are checked lexically, so this can be used before creating a file.
pub fn ensure_within(root: &std::path::Path, candidate: &std::path::Path) -> Result<PathBuf> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        root.join(candidate)
    };
    let normalised = normalize_path(&joined);
    if !normalised.starts_with(&root) {
        return Err(crate::error::LlmguiError::PathEscape(normalised));
    }
    Ok(normalised)
}

fn normalize_path(path: &std::path::Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_parent_escape() {
        let root = std::env::temp_dir().join("llmgui-path-test");
        std::fs::create_dir_all(&root).unwrap();
        assert!(ensure_within(&root, std::path::Path::new("../evil.gguf")).is_err());
        assert!(ensure_within(&root, std::path::Path::new("ok.gguf")).is_ok());
    }
}
