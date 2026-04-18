#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSnapshot {
    pub index_tree: String,
    pub worktree_tree: String,
    pub protected_paths: Vec<String>,
    pub unstaged_paths: Vec<String>,
    pub had_staged_changes: bool,
}

impl WorkspaceSnapshot {
    pub fn has_unstaged_path(&self, path: &str) -> bool {
        self.unstaged_paths
            .iter()
            .any(|candidate| candidate == path)
    }
}
