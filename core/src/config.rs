use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Default, Deserialize)]
pub struct FocalConfig {
    #[serde(default)]
    pub manifests: ManifestConfig,
}

#[derive(Debug, Default, Deserialize)]
pub struct ManifestConfig {
    #[serde(default)]
    pub auto_import: Vec<String>,
    #[serde(default)]
    pub auto_import_git: Vec<String>,
}

impl FocalConfig {
    pub fn load() -> Self {
        let path = Self::config_path();
        match std::fs::read_to_string(&path) {
            Ok(content) => toml::from_str(&content).unwrap_or_else(|e| {
                tracing::warn!(error = %e, "failed to parse config, using defaults");
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    fn config_path() -> PathBuf {
        dirs::home_dir()
            .expect("failed to determine home directory")
            .join(".focal")
            .join("config.toml")
    }
}
