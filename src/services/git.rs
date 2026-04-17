use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use tokio::process::Command;

use crate::domain::diff::{FileDiff, ReviewStatus};
use crate::services::parser::parse_git_diff;

#[derive(Debug, Clone)]
pub struct GitService {
    repo_path: PathBuf,
}

impl GitService {
    pub fn new(repo_path: impl Into<PathBuf>) -> Self {
        Self {
            repo_path: repo_path.into(),
        }
    }

    pub async fn collect_diff(&self) -> Result<(String, Vec<FileDiff>)> {
        let tracked = self
            .run_git(["diff", "--no-color", "--no-ext-diff"])
            .await
            .context("collect tracked diff")?;
        let untracked = self.collect_untracked_diff().await?;

        let combined = if tracked.is_empty() {
            untracked
        } else if untracked.is_empty() {
            tracked
        } else {
            format!("{tracked}\n{untracked}")
        };

        let files = parse_git_diff(&combined)?;
        Ok((combined, files))
    }

    pub async fn accept_file(&self, file: &mut FileDiff) -> Result<()> {
        let path = display_path(file);
        self.run_git(["add", "--", path]).await?;
        file.set_all_hunks_status(ReviewStatus::Accepted);
        Ok(())
    }

    pub async fn reject_file(&self, file: &mut FileDiff) -> Result<()> {
        let path = display_path(file);
        match file.status {
            crate::domain::diff::FileStatus::Added => self.reject_added_file(path).await?,
            _ => {
                self.run_git([
                    "restore",
                    "--source=HEAD",
                    "--staged",
                    "--worktree",
                    "--",
                    path,
                ])
                .await?;
            }
        }
        file.set_all_hunks_status(ReviewStatus::Rejected);
        Ok(())
    }

    pub async fn unstage_file(&self, file: &mut FileDiff) -> Result<()> {
        let path = display_path(file);
        self.run_git(["restore", "--staged", "--", path]).await?;
        file.set_all_hunks_status(ReviewStatus::Unreviewed);
        Ok(())
    }

    pub async fn apply_patch_to_index(&self, patch: &str) -> Result<()> {
        let output = Command::new("git")
            .args(["apply", "--cached", "-"])
            .current_dir(&self.repo_path)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .context("spawn git apply --cached")?;

        let output = feed_stdin_and_wait(output, patch).await?;
        if !output.status.success() {
            bail!(String::from_utf8_lossy(&output.stderr).to_string());
        }
        Ok(())
    }

    pub async fn reverse_apply_patch(&self, patch: &str) -> Result<()> {
        let output = Command::new("git")
            .args(["apply", "--reverse", "-"])
            .current_dir(&self.repo_path)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .context("spawn git apply --reverse")?;

        let output = feed_stdin_and_wait(output, patch).await?;
        if !output.status.success() {
            bail!(String::from_utf8_lossy(&output.stderr).to_string());
        }
        Ok(())
    }

    async fn collect_untracked_diff(&self) -> Result<String> {
        let raw = self
            .run_git(["ls-files", "--others", "--exclude-standard", "-z"])
            .await?;
        if raw.is_empty() {
            return Ok(String::new());
        }

        let mut chunks = Vec::new();
        for path in raw.split('\0').filter(|value| !value.is_empty()) {
            let output = Command::new("git")
                .args(["diff", "--no-index", "--no-color", "--", "/dev/null", path])
                .current_dir(&self.repo_path)
                .output()
                .await
                .with_context(|| format!("diff untracked file {path}"))?;

            if output.status.success() || output.status.code() == Some(1) {
                let diff = String::from_utf8_lossy(&output.stdout).to_string();
                if !diff.trim().is_empty() {
                    chunks.push(diff);
                }
            } else {
                bail!(String::from_utf8_lossy(&output.stderr).to_string());
            }
        }

        Ok(chunks.join("\n"))
    }

    async fn reject_added_file(&self, path: &str) -> Result<()> {
        let tracked = Command::new("git")
            .args(["ls-files", "--error-unmatch", "--", path])
            .current_dir(&self.repo_path)
            .output()
            .await
            .context("check tracked file")?;
        if tracked.status.success() {
            self.run_git(["rm", "-f", "--", path]).await?;
            return Ok(());
        }

        let full_path = self.repo_path.join(path);
        if full_path.is_dir() {
            tokio::fs::remove_dir_all(full_path).await?;
        } else if full_path.exists() {
            tokio::fs::remove_file(full_path).await?;
        }
        Ok(())
    }

    async fn run_git<const N: usize>(&self, args: [&str; N]) -> Result<String> {
        let output = Command::new("git")
            .args(args)
            .current_dir(&self.repo_path)
            .output()
            .await
            .with_context(|| format!("run git {:?}", args))?;

        if !output.status.success() {
            bail!(String::from_utf8_lossy(&output.stderr).to_string());
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }
}

fn display_path(file: &FileDiff) -> &str {
    if !file.new_path.is_empty() {
        &file.new_path
    } else {
        &file.old_path
    }
}

async fn feed_stdin_and_wait(
    mut child: tokio::process::Child,
    patch: &str,
) -> Result<std::process::Output> {
    use tokio::io::AsyncWriteExt;

    let mut stdin = child.stdin.take().context("child stdin unavailable")?;
    stdin.write_all(patch.as_bytes()).await?;
    stdin.shutdown().await?;
    let output = child.wait_with_output().await?;
    Ok(output)
}

pub fn patch_from_hunk(file: &FileDiff, hunk: &crate::domain::diff::Hunk) -> String {
    let old_path = if file.old_path.is_empty() {
        "/dev/null".to_string()
    } else {
        format!("a/{}", file.old_path)
    };
    let new_path = if file.new_path.is_empty() {
        "/dev/null".to_string()
    } else {
        format!("b/{}", file.new_path)
    };

    let mut patch = String::new();
    patch.push_str(&format!("--- {old_path}\n"));
    patch.push_str(&format!("+++ {new_path}\n"));
    patch.push_str(&format!("{}\n", hunk.header));

    for line in &hunk.lines {
        let prefix = match line.kind {
            crate::domain::diff::DiffLineKind::Add => '+',
            crate::domain::diff::DiffLineKind::Remove => '-',
            crate::domain::diff::DiffLineKind::Context => ' ',
        };
        patch.push(prefix);
        patch.push_str(&line.content);
        patch.push('\n');
    }

    patch
}

#[cfg(test)]
mod tests {
    use super::GitService;
    use anyhow::Result;

    #[tokio::test]
    async fn collect_diff_handles_empty_repo() -> Result<()> {
        let temp = tempfile::tempdir()?;
        tokio::process::Command::new("git")
            .args(["init"])
            .current_dir(temp.path())
            .output()
            .await?;
        let service = GitService::new(temp.path());
        let (_, files) = service.collect_diff().await?;
        assert!(files.is_empty());
        Ok(())
    }
}
