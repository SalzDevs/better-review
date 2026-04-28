use crate::domain::diff::{DiffLine, DiffLineKind, FileDiff, Hunk, ReviewStatus};

#[derive(Debug, Clone)]
pub struct ReviewRenderSideLine {
    pub kind: DiffLineKind,
    pub content: String,
    pub line_number: Option<u32>,
}

#[derive(Debug, Clone)]
pub enum ReviewRenderRow {
    HunkHeader {
        hunk_index: usize,
        header: String,
        status: ReviewStatus,
    },
    Diff {
        hunk_index: usize,
        old: Option<ReviewRenderSideLine>,
        new: Option<ReviewRenderSideLine>,
    },
    Spacer,
}

pub fn build_review_render_rows(file: &FileDiff) -> Vec<ReviewRenderRow> {
    let mut rows = Vec::new();

    for (hunk_index, hunk) in file.hunks.iter().enumerate() {
        rows.push(ReviewRenderRow::HunkHeader {
            hunk_index,
            header: hunk.header.clone(),
            status: hunk.review_status.clone(),
        });
        rows.extend(build_hunk_diff_rows(hunk_index, hunk));
        rows.push(ReviewRenderRow::Spacer);
    }

    rows
}

pub fn review_render_line_count(file: &FileDiff) -> usize {
    1 + file
        .hunks
        .iter()
        .map(|hunk| 2 + rendered_hunk_row_count(hunk))
        .sum::<usize>()
}

pub fn hunk_line_start(file: &FileDiff, hunk_index: usize) -> usize {
    let mut line = 1;
    for (index, hunk) in file.hunks.iter().enumerate() {
        if index == hunk_index {
            return line;
        }
        line += 2 + rendered_hunk_row_count(hunk);
    }
    0
}

pub fn hunk_index_for_line(file: &FileDiff, line_index: usize) -> usize {
    if file.hunks.is_empty() {
        return 0;
    }

    let mut current_line = 1;
    let mut current_hunk = 0;
    for (index, hunk) in file.hunks.iter().enumerate() {
        let hunk_end = current_line + rendered_hunk_row_count(hunk);
        if line_index <= hunk_end {
            return index;
        }
        current_line = hunk_end + 1;
        current_hunk = index;
    }
    current_hunk
}

fn build_hunk_diff_rows(hunk_index: usize, hunk: &Hunk) -> Vec<ReviewRenderRow> {
    let mut rows = Vec::new();
    let mut cursor = 0;

    while cursor < hunk.lines.len() {
        let line = &hunk.lines[cursor];
        match line.kind {
            DiffLineKind::Context => {
                let side_line = as_side_line(line);
                rows.push(ReviewRenderRow::Diff {
                    hunk_index,
                    old: Some(side_line.clone()),
                    new: Some(side_line),
                });
                cursor += 1;
            }
            DiffLineKind::Remove | DiffLineKind::Add => {
                let mut removed = Vec::new();
                let mut added = Vec::new();

                while cursor < hunk.lines.len() {
                    let next = &hunk.lines[cursor];
                    match next.kind {
                        DiffLineKind::Context => break,
                        DiffLineKind::Remove => removed.push(as_side_line(next)),
                        DiffLineKind::Add => added.push(as_side_line(next)),
                    }
                    cursor += 1;
                }

                let pair_count = removed.len().max(added.len());
                for index in 0..pair_count {
                    rows.push(ReviewRenderRow::Diff {
                        hunk_index,
                        old: removed.get(index).cloned(),
                        new: added.get(index).cloned(),
                    });
                }
            }
        }
    }

    rows
}

fn rendered_hunk_row_count(hunk: &Hunk) -> usize {
    build_hunk_diff_rows(0, hunk).len()
}

fn as_side_line(line: &DiffLine) -> ReviewRenderSideLine {
    ReviewRenderSideLine {
        kind: line.kind,
        content: line.content.clone(),
        line_number: line.old_line.or(line.new_line),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::diff::{FileStatus, ReviewStatus};

    fn sample_file() -> FileDiff {
        FileDiff {
            new_path: "src/lib.rs".to_string(),
            status: FileStatus::Modified,
            hunks: vec![
                Hunk {
                    header: "@@ -1,2 +1,2 @@".to_string(),
                    old_start: 1,
                    old_count: 2,
                    new_start: 1,
                    new_count: 2,
                    lines: vec![
                        DiffLine {
                            kind: DiffLineKind::Remove,
                            content: "old".to_string(),
                            old_line: Some(1),
                            new_line: None,
                        },
                        DiffLine {
                            kind: DiffLineKind::Add,
                            content: "new".to_string(),
                            old_line: None,
                            new_line: Some(1),
                        },
                    ],
                    review_status: ReviewStatus::Unreviewed,
                },
                Hunk {
                    header: "@@ -10,2 +10,3 @@".to_string(),
                    old_start: 10,
                    old_count: 2,
                    new_start: 10,
                    new_count: 3,
                    lines: vec![
                        DiffLine {
                            kind: DiffLineKind::Context,
                            content: "ctx".to_string(),
                            old_line: Some(10),
                            new_line: Some(10),
                        },
                        DiffLine {
                            kind: DiffLineKind::Add,
                            content: "extra".to_string(),
                            old_line: None,
                            new_line: Some(11),
                        },
                    ],
                    review_status: ReviewStatus::Accepted,
                },
            ],
            review_status: ReviewStatus::Unreviewed,
            ..FileDiff::default()
        }
    }

    #[test]
    fn render_rows_pair_old_and_new_lines() {
        let rows = build_review_render_rows(&sample_file());
        assert!(matches!(rows[0], ReviewRenderRow::HunkHeader { .. }));
        assert!(matches!(rows[1], ReviewRenderRow::Diff { .. }));
        match &rows[1] {
            ReviewRenderRow::Diff { old, new, .. } => {
                assert_eq!(old.as_ref().and_then(|line| line.line_number), Some(1));
                assert_eq!(new.as_ref().and_then(|line| line.line_number), Some(1));
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn render_helpers_track_hunk_positions() {
        let file = sample_file();
        assert_eq!(review_render_line_count(&file), 8);
        assert_eq!(hunk_line_start(&file, 0), 1);
        assert_eq!(hunk_line_start(&file, 1), 4);
        assert_eq!(hunk_index_for_line(&file, 0), 0);
        assert_eq!(hunk_index_for_line(&file, 2), 0);
        assert_eq!(hunk_index_for_line(&file, 4), 1);
        assert_eq!(hunk_index_for_line(&file, 99), 1);
    }
}
