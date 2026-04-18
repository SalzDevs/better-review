use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use tokio::time::{Duration, timeout};
use tokio::process::Command;

use crate::domain::diff::{FileDiff, ReviewStatus};
use crate::domain::session::WorkspaceSnapshot;
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
        let snapshot = self.snapshot_workspace().await?;
        let base_tree = self.base_tree().await?;
        self.diff_between_trees(&base_tree, &snapshot.worktree_tree).await
    }

    pub async fn snapshot_workspace(&self) -> Result<WorkspaceSnapshot> {
        let index_tree = self.write_index_tree().await?;
        let worktree_tree = self.write_worktree_tree().await?;
        let base_tree = self.base_tree().await?;
        let protected_paths = self.diff_names_between_trees(&base_tree, &worktree_tree).await?;
        let unstaged_paths = self.diff_names_between_trees(&index_tree, &worktree_tree).await?;
        let had_staged_changes = index_tree != base_tree;

        Ok(WorkspaceSnapshot {
            index_tree,
            worktree_tree,
            protected_paths,
            unstaged_paths,
            had_staged_changes,
        })
    }

    pub async fn collect_session_diff(
        &self,
        snapshot: &WorkspaceSnapshot,
    ) -> Result<(String, Vec<FileDiff>)> {
        let current_worktree = self.write_worktree_tree().await?;
        self.diff_between_trees(&snapshot.worktree_tree, &current_worktree)
            .await
    }

    pub async fn accept_file(&self, file: &mut FileDiff) -> Result<()> {
        let path = display_path(file);
        self.run_git(&["add", "--", path]).await?;
        file.set_all_hunks_status(ReviewStatus::Accepted);
        Ok(())
    }

    pub async fn reject_file(
        &self,
        file: &mut FileDiff,
        snapshot: &WorkspaceSnapshot,
    ) -> Result<()> {
        let path = display_path(file);
        self.restore_path_in_index(path, &snapshot.index_tree).await?;
        file.set_all_hunks_status(ReviewStatus::Rejected);
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

    pub async fn unstage_file(
        &self,
        file: &mut FileDiff,
        snapshot: &WorkspaceSnapshot,
    ) -> Result<()> {
        let path = display_path(file);
        self.restore_path_in_index(path, &snapshot.index_tree).await?;
        file.set_all_hunks_status(ReviewStatus::Unreviewed);
        Ok(())
    }

    pub async fn apply_patch_to_index(&self, patch: &str) -> Result<()> {
        self.run_git_apply(&["apply", "--cached", "-"], patch)
            .await
            .context("apply patch to index")
    }

    pub async fn sync_file_hunks_to_index(
        &self,
        file: &FileDiff,
        snapshot: Option<&WorkspaceSnapshot>,
    ) -> Result<()> {
        let path = display_path(file);

        if let Some(snapshot) = snapshot {
            self.restore_path_in_index(path, &snapshot.index_tree).await?;
        } else {
            self.run_git(&["restore", "--staged", "--", path]).await?;
        }

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

    async fn restore_path_in_index(&self, path: &str, tree: &str) -> Result<()> {
        if self.tree_contains_path(tree, path).await? {
            self.run_git(&["restore", "--source", tree, "--staged", "--", path])
                .await?;
        } else {
            self.run_git(&["rm", "--cached", "-r", "--ignore-unmatch", "--", path])
                .await?;
        }
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

    async fn diff_names_between_trees(&self, old_tree: &str, new_tree: &str) -> Result<Vec<String>> {
        if old_tree == new_tree {
            return Ok(Vec::new());
        }

        let output = self
            .run_git(&["diff", "--name-only", "--find-renames", old_tree, new_tree])
            .await
            .with_context(|| format!("diff tree names {old_tree}..{new_tree}"))?;
        let mut paths = output
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        paths.sort();
        paths.dedup();
        Ok(paths)
    }

    async fn base_tree(&self) -> Result<String> {
        let output = self.output_git(&["rev-parse", "--verify", "HEAD^{tree}"]).await?;
        if output.status.success() {
            return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
        }

        Ok(EMPTY_TREE_HASH.to_string())
    }

    async fn write_index_tree(&self) -> Result<String> {
        Ok(self.run_git(&["write-tree"]).await?.trim().to_string())
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

    async fn tree_contains_path(&self, tree: &str, path: &str) -> Result<bool> {
        let object = format!("{tree}:{path}");
        let output = self.output_git(&["cat-file", "-e", object.as_str()]).await?;
        Ok(output.status.success())
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
    async fn collect_session_diff_excludes_preexisting_dirty_paths() -> Result<()> {
        let temp = tempfile::tempdir()?;
        init_repo(temp.path()).await?;
        write_file(temp.path(), "tracked.txt", "base\n").await?;
        git(temp.path(), &["add", "tracked.txt"]).await?;
        git(temp.path(), &["commit", "-m", "init"]).await?;

        write_file(temp.path(), "tracked.txt", "user\n").await?;
        write_file(temp.path(), "notes.md", "keep me\n").await?;

        let service = GitService::new(temp.path());
        let snapshot = service.snapshot_workspace().await?;
        assert_eq!(snapshot.protected_paths.len(), 2);

        write_file(temp.path(), "tracked.txt", "user\nagent\n").await?;
        write_file(temp.path(), "generated.rs", "fn main() {}\n").await?;

        let (_, files) = service.collect_session_diff(&snapshot).await?;
        let paths = files
            .iter()
            .map(|file| file.display_path().to_string())
            .collect::<Vec<_>>();
        assert_eq!(paths, vec!["generated.rs".to_string(), "tracked.txt".to_string()]);
        Ok(())
    }

    #[tokio::test]
    async fn reject_file_restores_preexisting_staged_index_state() -> Result<()> {
        let temp = tempfile::tempdir()?;
        init_repo(temp.path()).await?;
        write_file(temp.path(), "tracked.txt", "base\n").await?;
        git(temp.path(), &["add", "tracked.txt"]).await?;
        git(temp.path(), &["commit", "-m", "init"]).await?;

        write_file(temp.path(), "tracked.txt", "preexisting\n").await?;
        git(temp.path(), &["add", "tracked.txt"]).await?;

        let service = GitService::new(temp.path());
        let snapshot = service.snapshot_workspace().await?;

        write_file(temp.path(), "tracked.txt", "preexisting\nagent\n").await?;
        let (_, mut files) = service.collect_session_diff(&snapshot).await?;
        let file = files
            .iter_mut()
            .find(|file| file.display_path() == "tracked.txt")
            .expect("tracked session diff");

        service.reject_file(file, &snapshot).await?;

        let worktree = tokio::fs::read_to_string(temp.path().join("tracked.txt")).await?;
        assert_eq!(worktree, "preexisting\nagent\n");

        let staged = git_stdout(temp.path(), &["show", ":tracked.txt"]).await?;
        assert_eq!(staged, "preexisting\n");
        Ok(())
    }

    #[tokio::test]
    async fn reject_file_keeps_agent_changes_in_worktree() -> Result<()> {
        let temp = tempfile::tempdir()?;
        init_repo(temp.path()).await?;
        write_file(temp.path(), "tracked.txt", "base\n").await?;
        git(temp.path(), &["add", "tracked.txt"]).await?;
        git(temp.path(), &["commit", "-m", "init"]).await?;

        write_file(temp.path(), "tracked.txt", "preexisting\n").await?;
        git(temp.path(), &["add", "tracked.txt"]).await?;

        let service = GitService::new(temp.path());
        let snapshot = service.snapshot_workspace().await?;
        write_file(temp.path(), "tracked.txt", "preexisting\nagent\n").await?;
        let (_, mut files) = service.collect_session_diff(&snapshot).await?;
        let file = files
            .iter_mut()
            .find(|file| file.display_path() == "tracked.txt")
            .expect("tracked session diff");

        service.reject_file(file, &snapshot).await?;

        let worktree = tokio::fs::read_to_string(temp.path().join("tracked.txt")).await?;
        assert_eq!(worktree, "preexisting\nagent\n");
        let staged = git_stdout(temp.path(), &["show", ":tracked.txt"]).await?;
        assert_eq!(staged, "preexisting\n");
        Ok(())
    }

    #[tokio::test]
    async fn snapshot_marks_preexisting_staged_changes() -> Result<()> {
        let temp = tempfile::tempdir()?;
        init_repo(temp.path()).await?;
        write_file(temp.path(), "tracked.txt", "base\n").await?;
        git(temp.path(), &["add", "tracked.txt"]).await?;
        git(temp.path(), &["commit", "-m", "init"]).await?;

        write_file(temp.path(), "tracked.txt", "staged\n").await?;
        git(temp.path(), &["add", "tracked.txt"]).await?;

        let service = GitService::new(temp.path());
        let snapshot = service.snapshot_workspace().await?;
        assert!(snapshot.had_staged_changes);
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
        let snapshot = service.snapshot_workspace().await?;
        write_file(temp.path(), "tracked.txt", "accepted\n").await?;
        let (_, mut files) = service.collect_session_diff(&snapshot).await?;
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
        let snapshot = service.snapshot_workspace().await?;
        write_file(temp.path(), "tracked.txt", "changed\nkeep\n").await?;
        let (_, mut files) = service.collect_session_diff(&snapshot).await?;
        let file = files
            .iter_mut()
            .find(|file| file.display_path() == "tracked.txt")
            .expect("tracked file in session diff");

        file.hunks[0].review_status = ReviewStatus::Rejected;
        file.sync_review_status();
        service.sync_file_hunks_to_index(file, Some(&snapshot)).await?;
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
        let snapshot = service.snapshot_workspace().await?;
        write_file(temp.path(), "tracked.txt", "ZERO\none\ntwo\nthree\nFOUR\nfive\n").await?;
        let (_, mut files) = service.collect_session_diff(&snapshot).await?;
        let file = files
            .iter_mut()
            .find(|file| file.display_path() == "tracked.txt")
            .expect("tracked file in session diff");

        assert_eq!(file.hunks.len(), 1);
        file.hunks[0].review_status = ReviewStatus::Accepted;
        file.sync_review_status();
        service.sync_file_hunks_to_index(file, Some(&snapshot)).await?;

        let staged_patch = git_stdout(temp.path(), &["diff", "--cached", "--", "tracked.txt"]).await?;
        assert!(staged_patch.contains("+ZERO"));
        assert!(staged_patch.contains("+FOUR"));
        let worktree = tokio::fs::read_to_string(temp.path().join("tracked.txt")).await?;
        assert_eq!(worktree, "ZERO\none\ntwo\nthree\nFOUR\nfive\n");
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
