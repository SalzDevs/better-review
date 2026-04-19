use regex::Regex;

use crate::domain::diff::{DiffLine, DiffLineKind, FileDiff, FileStatus, Hunk};

pub fn parse_git_diff(diff: &str) -> anyhow::Result<Vec<FileDiff>> {
    let hunk_re = Regex::new(r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@(.*)")?;

    let mut files = Vec::new();
    let mut current_file: Option<FileDiff> = None;
    let mut current_hunk: Option<Hunk> = None;
    let mut current_old_line = 0_u32;
    let mut current_new_line = 0_u32;

    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            if let Some(hunk) = current_hunk.take()
                && let Some(file) = current_file.as_mut()
            {
                file.hunks.push(hunk);
            }
            if let Some(file) = current_file.take() {
                files.push(file);
            }

            let mut file = FileDiff::default();
            let parts: Vec<&str> = rest.splitn(3, ' ').collect();
            if parts.len() >= 2 {
                file.old_path = parts[0].trim_start_matches("a/").to_string();
                file.new_path = parts[1].trim_start_matches("b/").to_string();
                file.status = FileStatus::Modified;
            }
            current_file = Some(file);
            continue;
        }

        let Some(file) = current_file.as_mut() else {
            continue;
        };

        if line.starts_with("new file mode ") {
            file.status = FileStatus::Added;
            continue;
        }

        if line.starts_with("deleted file mode ") {
            file.status = FileStatus::Deleted;
            continue;
        }

        if line.starts_with("Binary files ") || line == "GIT binary patch" {
            file.is_binary = true;
            continue;
        }

        if let Some(rest) = line.strip_prefix("--- ") {
            let path = normalize_diff_path(rest);
            if path.is_empty() {
                file.old_path.clear();
                file.status = FileStatus::Added;
            } else {
                file.old_path = path;
            }
            continue;
        }

        if let Some(rest) = line.strip_prefix("+++ ") {
            let path = normalize_diff_path(rest);
            if path.is_empty() {
                file.new_path.clear();
                file.status = FileStatus::Deleted;
            } else {
                file.new_path = path;
                if file.old_path.is_empty() {
                    file.status = FileStatus::Added;
                }
            }
            continue;
        }

        if line.starts_with("@@ ") {
            if let Some(hunk) = current_hunk.take() {
                file.hunks.push(hunk);
            }
            if let Some(caps) = hunk_re.captures(line) {
                let old_start = caps.get(1).unwrap().as_str().parse::<u32>()?;
                let old_count = caps
                    .get(2)
                    .map(|value| value.as_str().parse::<u32>())
                    .transpose()?
                    .unwrap_or(1);
                let new_start = caps.get(3).unwrap().as_str().parse::<u32>()?;
                let new_count = caps
                    .get(4)
                    .map(|value| value.as_str().parse::<u32>())
                    .transpose()?
                    .unwrap_or(1);
                current_old_line = old_start;
                current_new_line = new_start;
                current_hunk = Some(Hunk {
                    header: line.to_string(),
                    old_start,
                    old_count,
                    new_start,
                    new_count,
                    ..Hunk::default()
                });
            }
            continue;
        }

        let Some(hunk) = current_hunk.as_mut() else {
            continue;
        };

        let Some(prefix) = line.chars().next() else {
            continue;
        };
        let content = line.chars().skip(1).collect::<String>();
        let (kind, old_line, new_line) = match prefix {
            '+' => {
                let new_line = Some(current_new_line);
                current_new_line += 1;
                (Some(DiffLineKind::Add), None, new_line)
            }
            '-' => {
                let old_line = Some(current_old_line);
                current_old_line += 1;
                (Some(DiffLineKind::Remove), old_line, None)
            }
            ' ' => {
                let old_line = Some(current_old_line);
                let new_line = Some(current_new_line);
                current_old_line += 1;
                current_new_line += 1;
                (Some(DiffLineKind::Context), old_line, new_line)
            }
            _ => (None, None, None),
        };

        if let Some(kind) = kind {
            hunk.lines.push(DiffLine {
                kind,
                content,
                old_line,
                new_line,
            });
        }
    }

    if let Some(hunk) = current_hunk
        && let Some(file) = current_file.as_mut()
    {
        file.hunks.push(hunk);
    }
    if let Some(file) = current_file {
        files.push(file);
    }

    Ok(files)
}

fn normalize_diff_path(path: &str) -> String {
    let path = path.trim();
    if path.is_empty() || path == "/dev/null" {
        return String::new();
    }
    path.trim_start_matches("a/")
        .trim_start_matches("b/")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::parse_git_diff;
    use crate::domain::diff::{DiffLineKind, FileStatus};

    #[test]
    fn parses_added_and_modified_files() {
        let diff = r#"diff --git a/tracked.txt b/tracked.txt
--- a/tracked.txt
+++ b/tracked.txt
@@ -1 +1 @@
-old
+new
diff --git a/new.txt b/new.txt
new file mode 100644
--- /dev/null
+++ b/new.txt
@@ -0,0 +1 @@
+hello
"#;

        let files = parse_git_diff(diff).unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].status, FileStatus::Modified);
        assert_eq!(files[1].status, FileStatus::Added);
        assert_eq!(files[1].new_path, "new.txt");
    }

    #[test]
    fn parses_rename_and_copy_entries() {
        let diff = r#"diff --git a/old_name.txt b/new_name.txt
similarity index 100%
rename from old_name.txt
rename to new_name.txt
diff --git a/source.txt b/copied.txt
similarity index 100%
copy from source.txt
copy to copied.txt
"#;

        let files = parse_git_diff(diff).unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].old_path, "old_name.txt");
        assert_eq!(files[0].new_path, "new_name.txt");
        assert_eq!(files[0].hunks.len(), 0);
        assert_eq!(files[1].old_path, "source.txt");
        assert_eq!(files[1].new_path, "copied.txt");
        assert_eq!(files[1].hunks.len(), 0);
    }

    #[test]
    fn parses_binary_file_diffs() {
        let diff = r#"diff --git a/assets/logo.png b/assets/logo.png
index 1111111..2222222 100644
Binary files a/assets/logo.png and b/assets/logo.png differ
"#;

        let files = parse_git_diff(diff).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].display_path(), "assets/logo.png");
        assert!(files[0].is_binary);
        assert!(files[0].hunks.is_empty());
    }

    #[test]
    fn parses_deleted_file_status() {
        let diff = r#"diff --git a/obsolete.txt b/obsolete.txt
deleted file mode 100644
--- a/obsolete.txt
+++ /dev/null
@@ -1 +0,0 @@
-remove me
"#;

        let files = parse_git_diff(diff).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].status, FileStatus::Deleted);
        assert_eq!(files[0].old_path, "obsolete.txt");
        assert_eq!(files[0].new_path, "");
        assert_eq!(files[0].hunks.len(), 1);
    }

    #[test]
    fn parses_git_binary_patch_header() {
        let diff = r#"diff --git a/bin/data.bin b/bin/data.bin
new file mode 100644
index 0000000..1111111
GIT binary patch
literal 4
KcmZQz00d0D2LJ>B
"#;

        let files = parse_git_diff(diff).unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].is_binary);
        assert_eq!(files[0].status, FileStatus::Added);
    }

    #[test]
    fn parses_default_hunk_counts_without_explicit_lengths() {
        let diff = r#"diff --git a/file.txt b/file.txt
--- a/file.txt
+++ b/file.txt
@@ -3 +3 @@
-old
+new
"#;

        let files = parse_git_diff(diff).unwrap();
        let hunk = &files[0].hunks[0];
        assert_eq!(hunk.old_start, 3);
        assert_eq!(hunk.old_count, 1);
        assert_eq!(hunk.new_start, 3);
        assert_eq!(hunk.new_count, 1);
    }

    #[test]
    fn parses_no_newline_markers() {
        let diff = r#"diff --git a/readme.txt b/readme.txt
--- a/readme.txt
+++ b/readme.txt
@@ -1 +1 @@
-old
\ No newline at end of file
+new
\ No newline at end of file
"#;

        let files = parse_git_diff(diff).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].hunks.len(), 1);
        assert_eq!(files[0].hunks[0].lines.len(), 2);
        assert_eq!(files[0].hunks[0].lines[0].kind, DiffLineKind::Remove);
        assert_eq!(files[0].hunks[0].lines[0].content, "old");
        assert_eq!(files[0].hunks[0].lines[1].kind, DiffLineKind::Add);
        assert_eq!(files[0].hunks[0].lines[1].content, "new");
    }

    #[test]
    fn parses_rewrite_same_path() {
        let diff = r#"diff --git a/config.txt b/config.txt
--- a/config.txt
+++ b/config.txt
@@ -1,2 +1,2 @@
-old-a
-old-b
+new-a
+new-b
"#;

        let files = parse_git_diff(diff).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].display_path(), "config.txt");
        assert_eq!(files[0].status, FileStatus::Modified);
        assert_eq!(files[0].hunks.len(), 1);

        let removed = files[0].hunks[0]
            .lines
            .iter()
            .filter(|line| line.kind == DiffLineKind::Remove)
            .count();
        let added = files[0].hunks[0]
            .lines
            .iter()
            .filter(|line| line.kind == DiffLineKind::Add)
            .count();

        assert_eq!(removed, 2);
        assert_eq!(added, 2);
    }
}
