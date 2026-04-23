use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const SETTINGS_FILE_NAME: &str = "config.json";
const SETTINGS_DIR_NAME: &str = "better-review";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub version: u8,
    pub explain: ExplainSettings,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            version: 1,
            explain: ExplainSettings::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ExplainSettings {
    pub default_model: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SettingsStore {
    path: PathBuf,
}

impl SettingsStore {
    pub fn new() -> Result<Self> {
        Ok(Self {
            path: config_root_dir()?
                .join(SETTINGS_DIR_NAME)
                .join(SETTINGS_FILE_NAME),
        })
    }

    pub fn load(&self) -> Result<AppSettings> {
        if !self.path.exists() {
            return Ok(AppSettings::default());
        }

        let content = fs::read_to_string(&self.path)
            .with_context(|| format!("failed to read {}", self.path.display()))?;
        serde_json::from_str(&content)
            .with_context(|| format!("failed to parse {}", self.path.display()))
    }

    pub fn save(&self, settings: &AppSettings) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }

        let content =
            serde_json::to_string_pretty(settings).context("failed to serialize settings")?;
        fs::write(&self.path, format!("{content}\n"))
            .with_context(|| format!("failed to write {}", self.path.display()))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    #[cfg(test)]
    pub(crate) fn from_path(path: PathBuf) -> Self {
        Self { path }
    }
}

fn config_root_dir() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("XDG_CONFIG_HOME")
        && !path.is_empty()
    {
        return Ok(PathBuf::from(path));
    }

    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".config"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_returns_defaults_when_file_is_missing() {
        let temp = tempfile::tempdir().unwrap();
        let store = SettingsStore::from_path(temp.path().join("config.json"));

        assert_eq!(store.load().unwrap(), AppSettings::default());
    }

    #[test]
    fn save_and_load_round_trip_settings() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("better-review").join("config.json");
        let store = SettingsStore::from_path(path.clone());
        let settings = AppSettings {
            version: 1,
            explain: ExplainSettings {
                default_model: Some("openai/gpt-5.4".to_string()),
            },
        };

        store.save(&settings).unwrap();

        assert_eq!(store.load().unwrap(), settings);
        assert!(store.path().exists());
        assert_eq!(store.path(), path.as_path());
    }

    #[test]
    fn load_ignores_legacy_ui_settings_fields() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.json");
        let store = SettingsStore::from_path(path.clone());

        fs::write(
            &path,
            r#"{
  "version": 1,
  "ui": {
    "start_screen": "review_if_changes",
    "reduced_motion": true
  },
  "explain": {
    "default_model": "openai/gpt-5.4"
  }
}
"#,
        )
        .unwrap();

        assert_eq!(
            store.load().unwrap(),
            AppSettings {
                version: 1,
                explain: ExplainSettings {
                    default_model: Some("openai/gpt-5.4".to_string()),
                },
            }
        );
    }
}
