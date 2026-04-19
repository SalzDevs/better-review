use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use tokio::time::{Duration, timeout};
use tokio::process::Command;

use crate::domain::diff::{FileDiff, ReviewStatus};
use crate::services::parser::parse_git_diff;

const EMPTY_TREE_HASH: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
const GIT_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);

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
        let worktree_tree = self.write_worktree_tree().await?;
        let base_tree = self.base_tree().await?;
        self.diff_between_trees(&base_tree, &worktree_tree).await
    }

    pub async fn accept_file(&self, file: &mut FileDiff) -> Result<()> {
        let path = display_path(file);
        self.run_git(&["add", "--", path]).await?;
        file.set_all_hunks_status(ReviewStatus::Accepted);
        Ok(())
    }

    pub async fn reject_file_in_place(&self, file: &mut FileDiff) -> Result<()> {
        let path = display_path(file);
        self.run_git(&["restore", "--staged", "--", path]).await?;
        file.set_all_hunks_status(ReviewStatus::Rejected);
        Ok(())
    }

    pub async fn unstage_file_in_place(&self, file: &mut FileDiff) -> Result<()> {
        let path = display_path(file);
        self.run_git(&["restore", "--staged", "--", path]).await?;
        file.set_all_hunks_status(ReviewStatus::Unreviewed);
        Ok(())
    }

    pub async fn apply_patch_to_index(&self, patch: &str) -> Result<()> {
        self.run_git_apply(&["apply", "--cached", "-"], patch)
            .await
            .context("apply patch to index")
    }

    pub async fn sync_file_hunks_to_index(&self, file: &FileDiff) -> Result<()> {
        let path = display_path(file);
        self.run_git(&["restore", "--staged", "--", path]).await?;

        let accepted_patch = file
            .hunks
            .iter()
            .filter(|hunk| hunk.review_status == ReviewStatus::Accepted)
            .map(|hunk| patch_from_hunk(file, hunk))
            .collect::<Vec<_>>()
            .join("");
        if !accepted_patch.is_empty() {
            self.apply_patch_to_index(&accepted_patch).await?;
        }

        Ok(())
    }

    pub async fn has_staged_changes(&self) -> Result<bool> {
        let output = self.output_git(&["diff", "--cached", "--quiet"]).await?;
        Ok(!output.status.success())
    }

    pub async fn commit_staged(&self, message: &str) -> Result<()> {
        self.run_git(&["commit", "-m", message]).await?;
        Ok(())
    }

    async fn diff_between_trees(&self, old_tree: &str, new_tree: &str) -> Result<(String, Vec<FileDiff>)> {
        if old_tree == new_tree {
            return Ok((String::new(), Vec::new()));
        }

        let diff = self
            .run_git(&["diff", "--no-color", "--no-ext-diff", "--find-renames", old_tree, new_tree])
            .await
            .with_context(|| format!("diff trees {old_tree}..{new_tree}"))?;
        let files = if diff.trim().is_empty() {
            Vec::new()
        } else {
            parse_git_diff(&diff)?
        };
        Ok((diff, files))
    }

    async fn base_tree(&self) -> Result<String> {
        let output = self.output_git(&["rev-parse", "--verify", "HEAD^{tree}"]).await?;
        if output.status.success() {
            return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
        }

        Ok(EMPTY_TREE_HASH.to_string())
    }

    async fn write_worktree_tree(&self) -> Result<String> {
        let temp_index_path = temp_git_index_path();
        let temp_index = temp_index_path.to_string_lossy().into_owned();

        let result = async {
            self.run_git_with_env(&["add", "-A"], &[("GIT_INDEX_FILE", temp_index.as_str())])
                .await?;
            self.run_git_with_env(&["write-tree"], &[("GIT_INDEX_FILE", temp_index.as_str())])
                .await
        }
        .await;

        cleanup_temp_index(&temp_index_path).await;
        Ok(result?.trim().to_string())
    }

    async fn run_git_apply(&self, args: &[&str], patch: &str) -> Result<()> {
        let child = Command::new("git")
            .args(args)
            .current_dir(&self.repo_path)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .with_context(|| format!("spawn git {}", args.join(" ")))?;

        let output = feed_stdin_and_wait(child, patch).await?;
        if !output.status.success() {
            bail!(String::from_utf8_lossy(&output.stderr).to_string());
        }
        Ok(())
    }

    async fn run_git(&self, args: &[&str]) -> Result<String> {
        let output = self.output_git(args).await?;
        if !output.status.success() {
            bail!(String::from_utf8_lossy(&output.stderr).to_string());
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    async fn run_git_with_env(&self, args: &[&str], envs: &[(&str, &str)]) -> Result<String> {
        let output = self.output_git_with_env(args, envs).await?;
        if !output.status.success() {
            bail!(String::from_utf8_lossy(&output.stderr).to_string());
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    async fn output_git(&self, args: &[&str]) -> Result<std::process::Output> {
        self.output_git_with_env(args, &[]).await
    }

    async fn output_git_with_env(
        &self,
        args: &[&str],
        envs: &[(&str, &str)],
    ) -> Result<std::process::Output> {
        let mut command = Command::new("git");
        command.args(args).current_dir(&self.repo_path);
        for (key, value) in envs {
            command.env(key, value);
        }

        timeout(GIT_COMMAND_TIMEOUT, command.output())
            .await
            .with_context(|| format!("git command timed out {:?}", args))?
            .with_context(|| format!("run git {:?}", args))
    }
}

fn display_path(file: &FileDiff) -> &str {
    if !file.new_path.is_empty() {
        &file.new_path
    } else {
        &file.old_path
    }
}

fn temp_git_index_path() -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!("better-review-{}-{unique}.index", std::process::id()))
}

async fn cleanup_temp_index(path: &Path) {
    let _ = tokio::fs::remove_file(path).await;
    let lock_path = format!("{}.lock", path.display());
    let _ = tokio::fs::remove_file(lock_path).await;
}

async fn feed_stdin_and_wait(
    mut child: tokio::process::Child,
    patch: &str,
) -> Result<std::process::Output> {
    use tokio::io::AsyncWriteExt;

    let mut stdin = child.stdin.take().context("child stdin unavailable")?;
    stdin.write_all(patch.as_bytes()).await?;
    stdin.shutdown().await?;
    drop(stdin);
    let output = timeout(GIT_COMMAND_TIMEOUT, child.wait_with_output())
        .await
        .context("git apply timed out")??;
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
    use std::path::Path;
    use tokio::process::Command;

    use crate::domain::diff::ReviewStatus;

    #[tokio::test]
    async fn collect_diff_handles_empty_repo() -> Result<()> {
        let temp = tempfile::tempdir()?;
        init_repo(temp.path()).await?;

        let service = GitService::new(temp.path());
        let (_, files) = service.collect_diff().await?;
        assert!(files.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn commit_staged_creates_commit_for_accepted_changes() -> Result<()> {
        let temp = tempfile::tempdir()?;
        init_repo(temp.path()).await?;
        write_file(temp.path(), "tracked.txt", "base\n").await?;
        git(temp.path(), &["add", "tracked.txt"]).await?;
        git(temp.path(), &["commit", "-m", "init"]).await?;

        let service = GitService::new(temp.path());
        write_file(temp.path(), "tracked.txt", "accepted\n").await?;
        let (_, mut files) = service.collect_diff().await?;
        let file = files
            .iter_mut()
            .find(|file| file.display_path() == "tracked.txt")
            .expect("tracked file in session diff");

        service.accept_file(file).await?;
        assert!(service.has_staged_changes().await?);
        service.commit_staged("commit accepted changes").await?;

        let head = git_stdout(temp.path(), &["log", "-1", "--pretty=%s"]).await?;
        assert_eq!(head.trim(), "commit accepted changes");
        let content = tokio::fs::read_to_string(temp.path().join("tracked.txt")).await?;
        assert_eq!(content, "accepted\n");
        Ok(())
    }

    #[tokio::test]
    async fn sync_file_hunks_keeps_index_clean_for_rejected_hunks() -> Result<()> {
        let temp = tempfile::tempdir()?;
        init_repo(temp.path()).await?;
        write_file(temp.path(), "tracked.txt", "base\nkeep\n").await?;
        git(temp.path(), &["add", "tracked.txt"]).await?;
        git(temp.path(), &["commit", "-m", "init"]).await?;

        let service = GitService::new(temp.path());
        write_file(temp.path(), "tracked.txt", "changed\nkeep\n").await?;
        let (_, mut files) = service.collect_diff().await?;
        let file = files
            .iter_mut()
            .find(|file| file.display_path() == "tracked.txt")
            .expect("tracked file in session diff");

        file.hunks[0].review_status = ReviewStatus::Rejected;
        file.sync_review_status();
        service.sync_file_hunks_to_index(file).await?;
        assert!(!service.has_staged_changes().await?);

        let worktree = tokio::fs::read_to_string(temp.path().join("tracked.txt")).await?;
        assert_eq!(worktree, "changed\nkeep\n");
        Ok(())
    }

    #[tokio::test]
    async fn sync_file_hunks_stages_only_accepted_hunks() -> Result<()> {
        let temp = tempfile::tempdir()?;
        init_repo(temp.path()).await?;
        write_file(temp.path(), "tracked.txt", "zero\none\ntwo\nthree\nfour\nfive\n").await?;
        git(temp.path(), &["add", "tracked.txt"]).await?;
        git(temp.path(), &["commit", "-m", "init"]).await?;

        let service = GitService::new(temp.path());
        write_file(temp.path(), "tracked.txt", "ZERO\none\ntwo\nthree\nFOUR\nfive\n").await?;
        let (_, mut files) = service.collect_diff().await?;
        let file = files
            .iter_mut()
            .find(|file| file.display_path() == "tracked.txt")
            .expect("tracked file in session diff");

        assert_eq!(file.hunks.len(), 1);
        file.hunks[0].review_status = ReviewStatus::Accepted;
        file.sync_review_status();
        service.sync_file_hunks_to_index(file).await?;

        let staged_patch = git_stdout(temp.path(), &["diff", "--cached", "--", "tracked.txt"]).await?;
        assert!(staged_patch.contains("+ZERO"));
        assert!(staged_patch.contains("+FOUR"));
        let worktree = tokio::fs::read_to_string(temp.path().join("tracked.txt")).await?;
        assert_eq!(worktree, "ZERO\none\ntwo\nthree\nFOUR\nfive\n");
        Ok(())
    }

    #[tokio::test]
    async fn sync_file_hunks_rewrites_partially_staged_file_from_review_state() -> Result<()> {
        let temp = tempfile::tempdir()?;
        init_repo(temp.path()).await?;
        write_file(
            temp.path(),
            "tracked.txt",
            "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\neleven\ntwelve\n",
        )
        .await?;
        git(temp.path(), &["add", "tracked.txt"]).await?;
        git(temp.path(), &["commit", "-m", "init"]).await?;

        let service = GitService::new(temp.path());
        write_file(
            temp.path(),
            "tracked.txt",
            "ONE\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\neleven\nTWELVE\n",
        )
        .await?;

        let first_hunk_patch = r#"--- a/tracked.txt
+++ b/tracked.txt
@@ -1,4 +1,4 @@
-one
+ONE
 two
 three
 four
"#;
        service.apply_patch_to_index(first_hunk_patch).await?;

        let staged_before = git_stdout(temp.path(), &["diff", "--cached", "--", "tracked.txt"]).await?;
        assert!(staged_before.contains("+ONE"));
        assert!(!staged_before.contains("+TWELVE"));

        let (_, mut files) = service.collect_diff().await?;
        let file = files
            .iter_mut()
            .find(|file| file.display_path() == "tracked.txt")
            .expect("tracked file in diff");
        assert!(file.hunks.len() >= 2, "expected separate hunks for first/last line edits");

        for hunk in &mut file.hunks {
            hunk.review_status = if hunk.old_start == 1 {
                ReviewStatus::Accepted
            } else {
                ReviewStatus::Rejected
            };
        }
        file.sync_review_status();
        service.sync_file_hunks_to_index(file).await?;

        let staged_after = git_stdout(temp.path(), &["diff", "--cached", "--", "tracked.txt"]).await?;
        assert!(staged_after.contains("+ONE"));
        assert!(!staged_after.contains("+TWELVE"));

        let unstaged_after = git_stdout(temp.path(), &["diff", "--", "tracked.txt"]).await?;
        assert!(unstaged_after.contains("+TWELVE"));
        Ok(())
    }

    #[tokio::test]
    async fn commit_staged_fails_with_unmerged_conflicts() -> Result<()> {
        let temp = tempfile::tempdir()?;
        init_repo(temp.path()).await?;
        create_merge_conflict(temp.path()).await?;

        write_file(temp.path(), "ready.txt", "stage me\n").await?;
        git(temp.path(), &["add", "ready.txt"]).await?;

        let service = GitService::new(temp.path());
        let err = service
            .commit_staged("this should fail")
            .await
            .expect_err("commit should fail when index has unresolved conflicts");
        let message = format!("{err:#}");
        assert!(
            message.contains("unmerged")
                || message.contains("unresolved conflict")
                || message.contains("resolve"),
            "unexpected commit error: {message}"
        );
        Ok(())
    }

    async fn create_merge_conflict(path: &Path) -> Result<()> {
        write_file(path, "conflict.txt", "base\n").await?;
        git(path, &["add", "conflict.txt"]).await?;
        git(path, &["commit", "-m", "base"]).await?;

        let base_branch = git_stdout(path, &["rev-parse", "--abbrev-ref", "HEAD"]).await?;
        let base_branch = base_branch.trim().to_string();

        git(path, &["checkout", "-b", "feature/conflict"]).await?;
        write_file(path, "conflict.txt", "feature\n").await?;
        git(path, &["add", "conflict.txt"]).await?;
        git(path, &["commit", "-m", "feature change"]).await?;

        git(path, &["checkout", &base_branch]).await?;
        write_file(path, "conflict.txt", "main\n").await?;
        git(path, &["add", "conflict.txt"]).await?;
        git(path, &["commit", "-m", "main change"]).await?;

        let merge = Command::new("git")
            .args(["merge", "feature/conflict"])
            .current_dir(path)
            .output()
            .await?;
        if merge.status.success() {
            anyhow::bail!("expected merge conflict");
        }

        Ok(())
    }

    async fn init_repo(path: &Path) -> Result<()> {
        Command::new("git").args(["init"]).current_dir(path).output().await?;
        git(path, &["config", "user.email", "test@example.com"]).await?;
        git(path, &["config", "user.name", "Test User"]).await?;
        Ok(())
    }

    async fn git(path: &Path, args: &[&str]) -> Result<()> {
        let output = Command::new("git").args(args).current_dir(path).output().await?;
        if !output.status.success() {
            anyhow::bail!(String::from_utf8_lossy(&output.stderr).to_string());
        }
        Ok(())
    }

    async fn git_stdout(path: &Path, args: &[&str]) -> Result<String> {
        let output = Command::new("git").args(args).current_dir(path).output().await?;
        if !output.status.success() {
            anyhow::bail!(String::from_utf8_lossy(&output.stderr).to_string());
        }
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    async fn write_file(root: &Path, path: &str, contents: &str) -> Result<()> {
        let file_path = root.join(path);
        if let Some(parent) = file_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(file_path, contents).await?;
        Ok(())
    }
}
