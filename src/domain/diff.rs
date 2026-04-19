#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ReviewStatus {
    #[default]
    Unreviewed,
    Accepted,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub content: String,
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    Add,
    Remove,
    Context,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Hunk {
    pub header: String,
    pub old_start: u32,
    pub old_count: u32,
    pub new_start: u32,
    pub new_count: u32,
    pub lines: Vec<DiffLine>,
    pub review_status: ReviewStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FileStatus {
    Added,
    Deleted,
    #[default]
    Modified,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FileDiff {
    pub old_path: String,
    pub new_path: String,
    pub status: FileStatus,
    pub is_binary: bool,
    pub hunks: Vec<Hunk>,
    pub review_status: ReviewStatus,
}

impl FileDiff {
    pub fn display_path(&self) -> &str {
        if !self.new_path.is_empty() {
            &self.new_path
        } else {
            &self.old_path
        }
    }

    pub fn set_all_hunks_status(&mut self, status: ReviewStatus) {
        for hunk in &mut self.hunks {
            hunk.review_status = status.clone();
        }
        self.review_status = status;
    }

    pub fn sync_review_status(&mut self) {
        if self.hunks.is_empty() {
            return;
        }

        let all_accepted = self
            .hunks
            .iter()
            .all(|hunk| hunk.review_status == ReviewStatus::Accepted);
        let all_rejected = self
            .hunks
            .iter()
            .all(|hunk| hunk.review_status == ReviewStatus::Rejected);

        self.review_status = if all_accepted {
            ReviewStatus::Accepted
        } else if all_rejected {
            ReviewStatus::Rejected
        } else {
            ReviewStatus::Unreviewed
        };
    }
}

#[cfg(test)]
mod tests {
    use super::{FileDiff, ReviewStatus};

    #[test]
    fn display_path_falls_back_to_old_path_for_deleted_file() {
        let file = FileDiff {
            old_path: "removed.txt".to_string(),
            new_path: String::new(),
            ..FileDiff::default()
        };

        assert_eq!(file.display_path(), "removed.txt");
    }

    #[test]
    fn sync_review_status_keeps_state_when_no_hunks() {
        let mut file = FileDiff {
            review_status: ReviewStatus::Accepted,
            ..FileDiff::default()
        };

        file.sync_review_status();
        assert_eq!(file.review_status, ReviewStatus::Accepted);
    }
}
