use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const SETTINGS_FILE_NAME: &str = "config.json";
const SETTINGS_DIR_NAME: &str = "better-review";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub version: u8,
    pub explain: ExplainSettings,
    pub theme: ThemePreset,
    pub github: GitHubSettings,
    pub keybindings: KeybindingsSettings,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            version: 1,
            explain: ExplainSettings::default(),
            theme: ThemePreset::default(),
            github: GitHubSettings::default(),
            keybindings: KeybindingsSettings::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ThemePreset {
    #[default]
    Default,
    OneDarkPro,
    Dracula,
    TokyoNight,
    NightOwl,
}

impl std::fmt::Display for ThemePreset {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.label())
    }
}

impl ThemePreset {
    pub const ALL: [Self; 5] = [
        Self::Default,
        Self::OneDarkPro,
        Self::Dracula,
        Self::TokyoNight,
        Self::NightOwl,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "Default",
            Self::OneDarkPro => "One Dark Pro",
            Self::Dracula => "Dracula",
            Self::TokyoNight => "Tokyo Night",
            Self::NightOwl => "Night Owl",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ExplainSettings {
    pub default_model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct GitHubSettings {
    pub token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct KeybindingsSettings {
    pub refresh: String,
    pub commit: String,
    pub settings: String,
    pub accept: String,
    pub reject: String,
    pub unreview: String,
    pub explain: String,
    pub explain_context: String,
    pub explain_model: String,
    pub explain_history: String,
    pub explain_retry: String,
    pub explain_cancel: String,
    pub move_down: String,
    pub move_up: String,
}

impl Default for KeybindingsSettings {
    fn default() -> Self {
        Self {
            refresh: "r".to_string(),
            commit: "c".to_string(),
            settings: "s".to_string(),
            accept: "y".to_string(),
            reject: "x".to_string(),
            unreview: "u".to_string(),
            explain: "e".to_string(),
            explain_context: "o".to_string(),
            explain_model: "m".to_string(),
            explain_history: "h".to_string(),
            explain_retry: "t".to_string(),
            explain_cancel: "z".to_string(),
            move_down: "j".to_string(),
            move_up: "k".to_string(),
        }
    }
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
            .with_context(|| format!("failed to write {}", self.path.display()))?;
        restrict_settings_permissions(&self.path)?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn from_path(path: PathBuf) -> Self {
        Self { path }
    }
}

fn restrict_settings_permissions(path: &PathBuf) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .with_context(|| format!("failed to restrict permissions for {}", path.display()))?;
    }
    Ok(())
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
            theme: ThemePreset::TokyoNight,
            github: GitHubSettings::default(),
            keybindings: KeybindingsSettings::default(),
        };

        store.save(&settings).unwrap();

        assert_eq!(store.load().unwrap(), settings);
        assert!(path.exists());
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
                theme: ThemePreset::default(),
                github: GitHubSettings::default(),
                keybindings: KeybindingsSettings::default(),
            }
        );
    }

    #[test]
    fn load_fills_missing_keybindings_with_defaults() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.json");
        let store = SettingsStore::from_path(path.clone());
        fs::write(
            &path,
            r#"{
  "version": 1,
  "explain": {
    "default_model": "openai/gpt-5.4"
  }
}
"#,
        )
        .unwrap();

        let loaded = store.load().unwrap();
        assert_eq!(loaded.keybindings.refresh, "r");
        assert_eq!(loaded.keybindings.explain_context, "o");
        assert_eq!(loaded.theme, ThemePreset::default());
        assert_eq!(loaded.github, GitHubSettings::default());
    }
}
