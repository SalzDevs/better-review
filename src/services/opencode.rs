use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Deserialize;
use tokio::process::Command;

use crate::domain::diff::FileDiff;
use crate::domain::model_catalog::ModelOption;
use crate::services::git::GitService;
#[derive(Debug, Clone)]
pub struct OpencodeService {
    repo_path: PathBuf,
    binary: String,
}

#[derive(Debug, Clone)]
pub struct RunResult {
    pub changed_files: Vec<FileDiff>,
    #[allow(dead_code)]
    pub stdout: String,
    #[allow(dead_code)]
    pub stderr: String,
}

impl OpencodeService {
    pub fn new(repo_path: impl Into<PathBuf>, binary: impl Into<String>) -> Self {
        Self {
            repo_path: repo_path.into(),
            binary: binary.into(),
        }
    }

    pub async fn load_models(&self) -> Result<Vec<ModelOption>> {
        let output = Command::new(&self.binary)
            .args(["models", "--verbose"])
            .current_dir(&self.repo_path)
            .output()
            .await
            .context("load opencode models")?;

        if !output.status.success() {
            anyhow::bail!(String::from_utf8_lossy(&output.stderr).to_string());
        }

        Ok(parse_model_options(&String::from_utf8_lossy(
            &output.stdout,
        )))
    }

    pub async fn run_prompt(
        &self,
        git: &GitService,
        prompt: &str,
        model: Option<&str>,
        variant: Option<&str>,
    ) -> Result<RunResult> {
        let _before = git.collect_diff().await?;

        let repo_path = self.repo_path.to_string_lossy().to_string();
        let mut args = vec![
            "run".to_string(),
            "--dir".to_string(),
            repo_path,
            "--format".to_string(),
            "json".to_string(),
        ];
        if let Some(model) = model.filter(|value| !value.is_empty()) {
            args.push("--model".to_string());
            args.push(model.to_string());
        }
        if let Some(variant) = variant.filter(|value| !value.is_empty()) {
            args.push("--variant".to_string());
            args.push(variant.to_string());
        }
        args.push(prompt.to_string());

        let output = Command::new(&self.binary)
            .args(&args)
            .current_dir(&self.repo_path)
            .output()
            .await
            .context("run opencode prompt")?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if !output.status.success() {
            anyhow::bail!(if stderr.is_empty() {
                stdout.clone()
            } else {
                stderr.clone()
            });
        }

        let (diff, files) = git.collect_diff().await?;
        let changed_files = if diff.trim().is_empty() {
            Vec::new()
        } else {
            files
        };

        Ok(RunResult {
            changed_files,
            stdout,
            stderr,
        })
    }
}

pub fn parse_model_options(raw: &str) -> Vec<ModelOption> {
    let mut options = Vec::new();

    for chunk in raw.trim().split("\n\n") {
        let chunk = chunk.trim();
        if chunk.is_empty() {
            continue;
        }

        let mut lines = chunk.lines();
        let Some(id) = lines
            .next()
            .map(str::trim)
            .filter(|line| line.contains('/'))
        else {
            continue;
        };
        let body = lines.collect::<Vec<_>>().join("\n");
        let payload: ModelPayload = serde_json::from_str(&body).unwrap_or_default();
        let (provider, name) = split_model_id(id);

        let mut variants = payload.variants.into_keys().collect::<Vec<_>>();
        variants.sort();
        options.push(ModelOption {
            id: id.to_string(),
            provider,
            name,
            variants,
        });
    }

    options.sort_by(|a, b| a.provider.cmp(&b.provider).then(a.name.cmp(&b.name)));
    options
}

fn split_model_id(id: &str) -> (String, String) {
    let mut parts = id.splitn(2, '/');
    let provider = parts.next().unwrap_or_default().to_string();
    let name = parts.next().unwrap_or(id).to_string();
    (provider, name)
}

#[derive(Debug, Default, Deserialize)]
struct ModelPayload {
    #[serde(default)]
    variants: std::collections::BTreeMap<String, serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::parse_model_options;

    #[test]
    fn parses_model_catalog_with_variants() {
        let raw = r#"openai/gpt-5.4
{
  "variants": {
    "low": {},
    "high": {}
  }
}

github-copilot/gpt-5.1-codex
{
  "variants": {}
}"#;

        let models = parse_model_options(raw);
        assert_eq!(models.len(), 2);
        assert_eq!(models[1].id, "openai/gpt-5.4");
        assert_eq!(models[1].variants, vec!["high", "low"]);
    }
}
