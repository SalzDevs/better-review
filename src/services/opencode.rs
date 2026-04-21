use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use rusqlite::Connection;
use serde_json::Value;
use tokio::process::Command;
use tokio::time::timeout;

use crate::domain::diff::{DiffLine, DiffLineKind, FileDiff, Hunk};

const RUN_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpencodeSession {
    pub id: String,
    pub title: String,
    pub directory: PathBuf,
    pub time_updated: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhyAnswer {
    pub content: String,
    pub fork_session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WhyTarget {
    File {
        path: String,
        status: String,
        diff: String,
    },
    Hunk {
        path: String,
        header: String,
        diff: String,
    },
    Line {
        path: String,
        header: String,
        line_kind: DiffLineKind,
        old_line: Option<u32>,
        new_line: Option<u32>,
        line_content: String,
        hunk_diff: String,
    },
}

impl WhyTarget {
    pub fn label(&self) -> String {
        match self {
            Self::File { path, .. } => format!("file {path}"),
            Self::Hunk { path, header, .. } => format!("hunk {path} {header}"),
            Self::Line {
                path,
                old_line,
                new_line,
                ..
            } => {
                let locator = match (old_line, new_line) {
                    (Some(old), Some(new)) => format!("old {old}, new {new}"),
                    (Some(old), None) => format!("old {old}"),
                    (None, Some(new)) => format!("new {new}"),
                    (None, None) => "selected line".to_string(),
                };
                format!("line {path} ({locator})")
            }
        }
    }

    pub fn cache_key(&self, session_id: &str) -> String {
        match self {
            Self::File { path, .. } => format!("{session_id}:file:{path}"),
            Self::Hunk { path, header, .. } => format!("{session_id}:hunk:{path}:{header}"),
            Self::Line {
                path,
                old_line,
                new_line,
                line_content,
                ..
            } => format!(
                "{session_id}:line:{path}:{}:{}:{line_content}",
                old_line.map(|value| value.to_string()).unwrap_or_default(),
                new_line.map(|value| value.to_string()).unwrap_or_default(),
            ),
        }
    }

    fn prompt(&self) -> String {
        let instruction = concat!(
            "You are explaining code that was produced in this exact opencode session context. ",
            "Reply in plain text with 3 short sections: Intent, Change, Risk. ",
            "Be specific to the selected scope. If something is uncertain, say so explicitly."
        );

        match self {
            Self::File { path, status, diff } => format!(
                "{instruction}\n\nScope: file\nPath: {path}\nStatus: {status}\n\nDiff:\n{diff}"
            ),
            Self::Hunk { path, header, diff } => format!(
                "{instruction}\n\nScope: hunk\nPath: {path}\nHeader: {header}\n\nHunk diff:\n{diff}"
            ),
            Self::Line {
                path,
                header,
                line_kind,
                old_line,
                new_line,
                line_content,
                hunk_diff,
            } => format!(
                "{instruction}\n\nScope: line\nPath: {path}\nHunk: {header}\nLine kind: {}\nOld line: {}\nNew line: {}\nSelected line: {}\n\nFull hunk diff:\n{hunk_diff}",
                line_kind_label(*line_kind),
                old_line
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "none".to_string()),
                new_line
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "none".to_string()),
                line_content,
            ),
        }
    }
}

#[derive(Clone)]
pub struct OpencodeService {
    repo_path: PathBuf,
    db_path: PathBuf,
}

impl OpencodeService {
    pub fn new(repo_path: impl AsRef<Path>) -> Result<Self> {
        let home = std::env::var("HOME").context("HOME is not set")?;
        Ok(Self {
            repo_path: repo_path.as_ref().to_path_buf(),
            db_path: PathBuf::from(home).join(".local/share/opencode/opencode.db"),
        })
    }

    pub fn list_repo_sessions(&self) -> Result<Vec<OpencodeSession>> {
        let connection = Connection::open(&self.db_path)
            .with_context(|| format!("failed to open {}", self.db_path.display()))?;
        let mut statement = connection.prepare(
            "select id, title, directory, time_updated from session
             where directory = ?1 and time_archived is null
             order by time_updated desc",
        )?;
        let rows = statement.query_map([self.repo_path.to_string_lossy().as_ref()], |row| {
            Ok(OpencodeSession {
                id: row.get(0)?,
                title: row.get(1)?,
                directory: PathBuf::from(row.get::<_, String>(2)?),
                time_updated: row.get(3)?,
            })
        })?;

        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub async fn ask_why(&self, session_id: &str, target: &WhyTarget) -> Result<WhyAnswer> {
        let prompt = target.prompt();
        let output = timeout(RUN_TIMEOUT, self.run_forked_session(session_id, &prompt))
            .await
            .map_err(|_| anyhow!("opencode timed out while generating a why-this explanation"))??;

        let fork_session_id = extract_session_id_from_run_output(&output)
            .context("opencode did not report a fork session id")?;
        let exported = self.export_session(&fork_session_id).await?;
        let content = extract_answer_from_export(&exported)?;

        Ok(WhyAnswer {
            content,
            fork_session_id,
        })
    }

    async fn run_forked_session(&self, session_id: &str, prompt: &str) -> Result<String> {
        let output = Command::new("opencode")
            .args([
                "run",
                "--session",
                session_id,
                "--fork",
                "--format",
                "json",
                "--dir",
                self.repo_path.to_string_lossy().as_ref(),
                prompt,
            ])
            .output()
            .await
            .context("failed to start opencode")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            bail!(
                "opencode why-this request failed{}",
                if stderr.is_empty() {
                    String::new()
                } else {
                    format!(": {stderr}")
                }
            );
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    async fn export_session(&self, session_id: &str) -> Result<String> {
        let output = Command::new("opencode")
            .args(["export", session_id])
            .output()
            .await
            .context("failed to export opencode session")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            bail!(
                "failed to export opencode fork session{}",
                if stderr.is_empty() {
                    String::new()
                } else {
                    format!(": {stderr}")
                }
            );
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }
}

pub fn why_target_for_file(file: &FileDiff) -> WhyTarget {
    WhyTarget::File {
        path: file.display_path().to_string(),
        status: file_status_label(file.status).to_string(),
        diff: diff_text_for_file(file),
    }
}

pub fn why_target_for_hunk(file: &FileDiff, hunk: &Hunk) -> WhyTarget {
    WhyTarget::Hunk {
        path: file.display_path().to_string(),
        header: hunk.header.clone(),
        diff: diff_text_for_hunk(file, hunk),
    }
}

pub fn why_target_for_line(file: &FileDiff, hunk: &Hunk, line: &DiffLine) -> WhyTarget {
    WhyTarget::Line {
        path: file.display_path().to_string(),
        header: hunk.header.clone(),
        line_kind: line.kind,
        old_line: line.old_line,
        new_line: line.new_line,
        line_content: line.content.clone(),
        hunk_diff: diff_text_for_hunk(file, hunk),
    }
}

fn diff_text_for_file(file: &FileDiff) -> String {
    let old_path = if file.old_path.is_empty() {
        "/dev/null"
    } else {
        file.old_path.as_str()
    };
    let new_path = if file.new_path.is_empty() {
        "/dev/null"
    } else {
        file.new_path.as_str()
    };

    let mut lines = vec![format!("--- {old_path}"), format!("+++ {new_path}")];
    for hunk in &file.hunks {
        lines.push(hunk.header.clone());
        for line in &hunk.lines {
            lines.push(format_diff_line(line));
        }
    }

    if file.hunks.is_empty() {
        lines.push(format!(
            "[{} change without textual hunks]",
            file_status_label(file.status)
        ));
    }

    lines.join("\n")
}

fn diff_text_for_hunk(file: &FileDiff, hunk: &Hunk) -> String {
    let old_path = if file.old_path.is_empty() {
        "/dev/null"
    } else {
        file.old_path.as_str()
    };
    let new_path = if file.new_path.is_empty() {
        "/dev/null"
    } else {
        file.new_path.as_str()
    };

    let mut lines = vec![
        format!("--- {old_path}"),
        format!("+++ {new_path}"),
        hunk.header.clone(),
    ];
    lines.extend(hunk.lines.iter().map(format_diff_line));
    lines.join("\n")
}

fn format_diff_line(line: &DiffLine) -> String {
    let prefix = match line.kind {
        DiffLineKind::Add => '+',
        DiffLineKind::Remove => '-',
        DiffLineKind::Context => ' ',
    };
    format!("{prefix}{}", line.content)
}

fn file_status_label(status: crate::domain::diff::FileStatus) -> &'static str {
    match status {
        crate::domain::diff::FileStatus::Added => "added",
        crate::domain::diff::FileStatus::Deleted => "deleted",
        crate::domain::diff::FileStatus::Modified => "modified",
    }
}

fn line_kind_label(kind: DiffLineKind) -> &'static str {
    match kind {
        DiffLineKind::Add => "added",
        DiffLineKind::Remove => "removed",
        DiffLineKind::Context => "context",
    }
}

fn extract_session_id_from_run_output(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let value = serde_json::from_str::<Value>(line).ok()?;
        value
            .get("sessionID")
            .and_then(Value::as_str)
            .or_else(|| {
                value
                    .get("part")
                    .and_then(|part| part.get("sessionID"))
                    .and_then(Value::as_str)
            })
            .map(ToString::to_string)
    })
}

fn extract_answer_from_export(export: &str) -> Result<String> {
    let json_start = export
        .find('{')
        .context("exported session did not contain JSON data")?;
    let payload = &export[json_start..];
    let value: Value =
        serde_json::from_str(payload).context("failed to parse exported session JSON")?;
    let messages = value
        .get("messages")
        .and_then(Value::as_array)
        .context("exported session did not include messages")?;

    for message in messages.iter().rev() {
        let role = message
            .get("info")
            .and_then(|info| info.get("role"))
            .and_then(Value::as_str);
        if role != Some("assistant") {
            continue;
        }

        let parts = message
            .get("parts")
            .and_then(Value::as_array)
            .into_iter()
            .flatten();
        let text_parts = parts
            .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>();

        if let Some(answer) = sanitize_exported_answer(&text_parts) {
            return Ok(answer);
        }
    }

    bail!("opencode did not return a text explanation")
}

fn sanitize_exported_answer(parts: &[&str]) -> Option<String> {
    let joined = parts.join("\n\n");
    let mut cleaned = joined.trim().to_string();

    if let Some(reminder_index) = cleaned.find("<system-reminder>") {
        cleaned.truncate(reminder_index);
        cleaned = cleaned.trim().to_string();
    }

    if looks_like_prompt_echo(&cleaned) {
        return None;
    }

    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

fn looks_like_prompt_echo(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.starts_with(
        "You are explaining code that was produced in this exact opencode session context.",
    ) || trimmed.starts_with("Scope: ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::diff::{DiffLine, DiffLineKind, FileDiff, FileStatus, Hunk};
    use tempfile::tempdir;

    fn sample_file() -> FileDiff {
        FileDiff {
            new_path: "src/lib.rs".to_string(),
            status: FileStatus::Modified,
            hunks: vec![Hunk {
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
                ..Hunk::default()
            }],
            ..FileDiff::default()
        }
    }

    #[test]
    fn extracts_fork_session_id_from_json_stream() {
        let output = concat!(
            "{\"type\":\"step_start\",\"sessionID\":\"ses_new\"}\n",
            "{\"type\":\"other\"}\n"
        );
        assert_eq!(
            extract_session_id_from_run_output(output),
            Some("ses_new".to_string())
        );
    }

    #[test]
    fn extracts_fork_session_id_from_nested_part_payload() {
        let output = concat!(
            "{\"part\":{\"sessionID\":\"ses_nested\"}}\n",
            "{\"type\":\"other\"}\n"
        );
        assert_eq!(
            extract_session_id_from_run_output(output),
            Some("ses_nested".to_string())
        );
    }

    #[test]
    fn extracts_text_answer_from_export() {
        let export = r#"Exporting session: ses_x{"messages":[{"info":{"role":"assistant"},"parts":[{"type":"text","text":"Intent\nChange\nRisk"}]}]}"#;
        let answer = extract_answer_from_export(export).unwrap();
        assert_eq!(answer, "Intent\nChange\nRisk");
    }

    #[test]
    fn extracts_last_non_empty_assistant_answer_from_export() {
        let export = r#"Exporting session: ses_x{"messages":[
            {"info":{"role":"assistant"},"parts":[{"type":"text","text":"  "}]},
            {"info":{"role":"user"},"parts":[{"type":"text","text":"ignore me"}]},
            {"info":{"role":"assistant"},"parts":[
                {"type":"text","text":"Intent: update parser"},
                {"type":"text","text":"\n\nRisk: low"}
            ]}
        ]}"#;
        let answer = extract_answer_from_export(export).unwrap();
        assert_eq!(answer, "Intent: update parser\n\nRisk: low");
    }

    #[test]
    fn ignores_prompt_echo_and_system_reminder_in_exported_answer() {
        let export = r#"Exporting session: ses_x{"messages":[
            {"info":{"role":"assistant"},"parts":[
                {"type":"text","text":"You are explaining code that was produced in this exact opencode session context.\n\nScope: hunk\nPath: Cargo.lock"},
                {"type":"text","text":"<system-reminder>\nYour operational mode has changed from plan to build.\n</system-reminder>"}
            ]},
            {"info":{"role":"assistant"},"parts":[
                {"type":"text","text":"Intent: add SQLite-backed session discovery\n\nChange: add rusqlite and a Why This panel\n\nRisk: the export parser must ignore prompt echoes."}
            ]}
        ]}"#;

        let answer = extract_answer_from_export(export).unwrap();
        assert_eq!(
            answer,
            "Intent: add SQLite-backed session discovery\n\nChange: add rusqlite and a Why This panel\n\nRisk: the export parser must ignore prompt echoes."
        );
    }

    #[test]
    fn sanitize_exported_answer_rejects_prompt_echo_only_payloads() {
        let prompt_echo = [
            "You are explaining code that was produced in this exact opencode session context.",
            "Scope: line",
        ];

        assert_eq!(sanitize_exported_answer(&prompt_echo), None);
    }

    #[test]
    fn file_targets_include_status_and_diff_in_prompt() {
        let file = sample_file();
        let target = why_target_for_file(&file);

        assert_eq!(target.label(), "file src/lib.rs");
        assert_eq!(target.cache_key("ses_1"), "ses_1:file:src/lib.rs");
        let prompt = target.prompt();
        assert!(prompt.contains("Scope: file"));
        assert!(prompt.contains("Status: modified"));
        assert!(prompt.contains("@@ -1,2 +1,2 @@"));
    }

    #[test]
    fn hunk_targets_include_header_and_diff_in_prompt() {
        let file = sample_file();
        let target = why_target_for_hunk(&file, &file.hunks[0]);

        assert_eq!(
            target.label(),
            format!("hunk src/lib.rs {}", file.hunks[0].header)
        );
        let prompt = target.prompt();
        assert!(prompt.contains("Scope: hunk"));
        assert!(prompt.contains("Header: @@ -1,2 +1,2 @@"));
        assert!(prompt.contains("+new"));
        assert!(prompt.contains("-old"));
    }

    #[test]
    fn builds_line_targets_with_hunk_context() {
        let file = sample_file();
        let hunk = &file.hunks[0];
        let line = &hunk.lines[1];
        let target = why_target_for_line(&file, hunk, line);

        match target {
            WhyTarget::Line {
                path,
                line_kind,
                line_content,
                hunk_diff,
                ..
            } => {
                assert_eq!(path, "src/lib.rs");
                assert_eq!(line_kind, DiffLineKind::Add);
                assert_eq!(line_content, "new");
                assert!(hunk_diff.contains("@@ -1,2 +1,2 @@"));
                assert!(hunk_diff.contains("+new"));
            }
            _ => panic!("expected line target"),
        }

        let prompt = why_target_for_line(&file, hunk, line).prompt();
        assert!(prompt.contains("Scope: line"));
        assert!(prompt.contains("Line kind: added"));
        assert!(prompt.contains("Selected line: new"));
    }

    #[test]
    fn cache_keys_are_scope_specific() {
        let file = sample_file();
        let file_key = why_target_for_file(&file).cache_key("ses_1");
        let hunk_key = why_target_for_hunk(&file, &file.hunks[0]).cache_key("ses_1");
        assert_ne!(file_key, hunk_key);
    }

    #[test]
    fn diff_text_for_file_describes_non_textual_change_without_hunks() {
        let file = FileDiff {
            old_path: "script.sh".to_string(),
            new_path: "script.sh".to_string(),
            status: FileStatus::Modified,
            hunks: Vec::new(),
            ..FileDiff::default()
        };

        let diff = diff_text_for_file(&file);
        assert!(diff.contains("--- script.sh"));
        assert!(diff.contains("+++ script.sh"));
        assert!(diff.contains("[modified change without textual hunks]"));
    }

    #[test]
    fn list_repo_sessions_returns_repo_matches_in_updated_order() {
        let temp = tempdir().unwrap();
        let db_path = temp.path().join("opencode.db");
        let repo_path = temp.path().join("repo");
        std::fs::create_dir_all(&repo_path).unwrap();

        let connection = Connection::open(&db_path).unwrap();
        connection
            .execute_batch(
                "
                create table session (
                    id text primary key,
                    project_id text not null,
                    parent_id text,
                    slug text not null,
                    directory text not null,
                    title text not null,
                    version text not null,
                    share_url text,
                    summary_additions integer,
                    summary_deletions integer,
                    summary_files integer,
                    summary_diffs text,
                    revert text,
                    permission text,
                    time_created integer not null,
                    time_updated integer not null,
                    time_compacting integer,
                    time_archived integer
                );
                ",
            )
            .unwrap();

        connection
            .execute(
                "insert into session (id, project_id, slug, directory, title, version, time_created, time_updated, time_archived) values (?1, 'proj', 'a', ?2, 'Older', '1.0', 1, 10, null)",
                rusqlite::params!["ses_old", repo_path.to_string_lossy().as_ref()],
            )
            .unwrap();
        connection
            .execute(
                "insert into session (id, project_id, slug, directory, title, version, time_created, time_updated, time_archived) values (?1, 'proj', 'b', ?2, 'Newest', '1.0', 1, 20, null)",
                rusqlite::params!["ses_new", repo_path.to_string_lossy().as_ref()],
            )
            .unwrap();
        connection
            .execute(
                "insert into session (id, project_id, slug, directory, title, version, time_created, time_updated, time_archived) values (?1, 'proj', 'c', '/tmp/other', 'Other repo', '1.0', 1, 30, null)",
                rusqlite::params!["ses_other"],
            )
            .unwrap();
        connection
            .execute(
                "insert into session (id, project_id, slug, directory, title, version, time_created, time_updated, time_archived) values (?1, 'proj', 'd', ?2, 'Archived', '1.0', 1, 40, 999)",
                rusqlite::params!["ses_archived", repo_path.to_string_lossy().as_ref()],
            )
            .unwrap();

        let service = OpencodeService { repo_path, db_path };
        let sessions = service.list_repo_sessions().unwrap();

        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].id, "ses_new");
        assert_eq!(sessions[0].title, "Newest");
        assert_eq!(sessions[1].id, "ses_old");
    }
}
