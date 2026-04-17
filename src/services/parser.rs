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
            if let Some(hunk) = current_hunk.take() {
                if let Some(file) = current_file.as_mut() {
                    file.hunks.push(hunk);
                }
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

    if let Some(hunk) = current_hunk {
        if let Some(file) = current_file.as_mut() {
            file.hunks.push(hunk);
        }
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
    use crate::domain::diff::FileStatus;

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
}
