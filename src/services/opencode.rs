use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use rusqlite::Connection;
use serde_json::Value;
use tokio::process::Command;
use tokio::time::{sleep, timeout};

use crate::domain::diff::{DiffLine, DiffLineKind, FileDiff, Hunk};

const RUN_TIMEOUT: Duration = Duration::from_secs(120);
const ANSWER_LOOKUP_ATTEMPTS: usize = 12;
const ANSWER_LOOKUP_DELAY: Duration = Duration::from_millis(250);

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
pub struct SelectedHunkTarget {
    pub header: String,
    pub diff: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedLineTarget {
    pub header: String,
    pub line_kind: DiffLineKind,
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    pub line_content: String,
    pub hunk_diff: String,
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
    SelectedHunks {
        path: String,
        hunks: Vec<SelectedHunkTarget>,
    },
    SelectedLines {
        path: String,
        lines: Vec<SelectedLineTarget>,
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
            Self::SelectedHunks { path, hunks } => {
                format!("{} selected hunk(s) in {path}", hunks.len())
            }
            Self::SelectedLines { path, lines } => {
                format!("{} selected line(s) in {path}", lines.len())
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
            Self::SelectedHunks { path, hunks } => format!(
                "{session_id}:selected-hunks:{path}:{}",
                hunks
                    .iter()
                    .map(|hunk| hunk.header.replace('|', "/"))
                    .collect::<Vec<_>>()
                    .join("|")
            ),
            Self::SelectedLines { path, lines } => format!(
                "{session_id}:selected-lines:{path}:{}",
                lines
                    .iter()
                    .map(|line| format!(
                        "{}:{}:{}:{}",
                        line.header.replace('|', "/"),
                        line.old_line
                            .map(|value| value.to_string())
                            .unwrap_or_default(),
                        line.new_line
                            .map(|value| value.to_string())
                            .unwrap_or_default(),
                        line.line_content.replace('|', "/")
                    ))
                    .collect::<Vec<_>>()
                    .join("|")
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
            Self::SelectedHunks { path, hunks } => format!(
                "{instruction}\n\nScope: selected hunks\nPath: {path}\nSelection count: {}\n\nExplain why these selected hunks exist together in this file. Describe the shared intent first, then mention any notable differences or risk.\n\n{}",
                hunks.len(),
                hunks
                    .iter()
                    .enumerate()
                    .map(|(index, hunk)| format!(
                        "Selected hunk {}\nHeader: {}\n{}",
                        index + 1,
                        hunk.header,
                        hunk.diff
                    ))
                    .collect::<Vec<_>>()
                    .join("\n\n")
            ),
            Self::SelectedLines { path, lines } => format!(
                "{instruction}\n\nScope: selected lines\nPath: {path}\nSelection count: {}\n\nExplain why these selected lines exist. Describe any shared intent first, then briefly cover each selected line with its role and risk.\n\n{}",
                lines.len(),
                lines
                    .iter()
                    .enumerate()
                    .map(|(index, line)| format!(
                        "Selected line {}\nHunk: {}\nLine kind: {}\nOld line: {}\nNew line: {}\nSelected line: {}\n\nFull hunk diff:\n{}",
                        index + 1,
                        line.header,
                        line_kind_label(line.line_kind),
                        line.old_line
                            .map(|value| value.to_string())
                            .unwrap_or_else(|| "none".to_string()),
                        line.new_line
                            .map(|value| value.to_string())
                            .unwrap_or_else(|| "none".to_string()),
                        line.line_content,
                        line.hunk_diff,
                    ))
                    .collect::<Vec<_>>()
                    .join("\n\n")
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
        let content = self.wait_for_answer(&fork_session_id).await?;

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

    async fn wait_for_answer(&self, session_id: &str) -> Result<String> {
        let mut last_error = None;

        for attempt in 0..ANSWER_LOOKUP_ATTEMPTS {
            match self.latest_assistant_text(session_id) {
                Ok(answer) => return Ok(answer),
                Err(err) => last_error = Some(err),
            }

            if attempt + 1 < ANSWER_LOOKUP_ATTEMPTS {
                sleep(ANSWER_LOOKUP_DELAY).await;
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow!("opencode did not persist a forked answer")))
    }

    fn latest_assistant_text(&self, session_id: &str) -> Result<String> {
        let connection = Connection::open(&self.db_path)
            .with_context(|| format!("failed to open {}", self.db_path.display()))?;
        let mut statement = connection.prepare(
            "select id, data from message
             where session_id = ?1
             order by time_created desc",
        )?;
        let rows = statement.query_map([session_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;

        for row in rows {
            let (message_id, message_data) = row?;
            let message: Value = serde_json::from_str(&message_data)
                .context("failed to parse opencode message JSON")?;
            if message_role(&message) != Some("assistant") {
                continue;
            }

            let text_parts = message_text_parts(&connection, session_id, &message_id)?;
            let text_refs = text_parts.iter().map(String::as_str).collect::<Vec<_>>();
            if let Some(answer) = sanitize_candidate_answer(&text_refs) {
                return Ok(answer);
            }
        }

        bail!("opencode did not return a text explanation")
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

pub fn why_target_for_selected_hunks(file: &FileDiff, hunks: Vec<&Hunk>) -> WhyTarget {
    WhyTarget::SelectedHunks {
        path: file.display_path().to_string(),
        hunks: hunks
            .into_iter()
            .map(|hunk| SelectedHunkTarget {
                header: hunk.header.clone(),
                diff: diff_text_for_hunk(file, hunk),
            })
            .collect(),
    }
}

pub fn why_target_for_selected_lines(file: &FileDiff, lines: Vec<(&Hunk, &DiffLine)>) -> WhyTarget {
    WhyTarget::SelectedLines {
        path: file.display_path().to_string(),
        lines: lines
            .into_iter()
            .map(|(hunk, line)| SelectedLineTarget {
                header: hunk.header.clone(),
                line_kind: line.kind,
                old_line: line.old_line,
                new_line: line.new_line,
                line_content: line.content.clone(),
                hunk_diff: diff_text_for_hunk(file, hunk),
            })
            .collect(),
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

fn sanitize_candidate_answer(parts: &[&str]) -> Option<String> {
    let cleaned = parts
        .iter()
        .filter_map(|part| sanitize_candidate_part(part))
        .collect::<Vec<_>>()
        .join("\n\n");
    let cleaned = cleaned.trim().to_string();

    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

fn sanitize_candidate_part(part: &str) -> Option<String> {
    let mut cleaned = part.trim().to_string();
    if let Some(reminder_index) = cleaned.find("<system-reminder>") {
        cleaned.truncate(reminder_index);
        cleaned = cleaned.trim().to_string();
    }

    if cleaned.is_empty() || looks_like_prompt_echo(&cleaned) {
        None
    } else {
        Some(cleaned)
    }
}

fn message_role(message: &Value) -> Option<&str> {
    message
        .get("role")
        .and_then(Value::as_str)
        .or_else(|| message.get("info")?.get("role")?.as_str())
}

fn message_text_parts(
    connection: &Connection,
    session_id: &str,
    message_id: &str,
) -> Result<Vec<String>> {
    let mut statement = connection.prepare(
        "select data from part
         where session_id = ?1 and message_id = ?2
         order by time_created asc",
    )?;
    let rows = statement.query_map(rusqlite::params![session_id, message_id], |row| {
        row.get::<_, String>(0)
    })?;

    let mut text_parts = Vec::new();
    for row in rows {
        let data = row?;
        let part: Value =
            serde_json::from_str(&data).context("failed to parse opencode part JSON")?;
        if part.get("type").and_then(Value::as_str) == Some("text")
            && let Some(text) = part.get("text").and_then(Value::as_str)
        {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                text_parts.push(trimmed.to_string());
            }
        }
    }

    Ok(text_parts)
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
    use serde_json::json;
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
    fn sanitize_candidate_answer_keeps_non_empty_text_parts() {
        let answer = sanitize_candidate_answer(&["Intent: update parser", "Risk: low"]).unwrap();
        assert_eq!(answer, "Intent: update parser\n\nRisk: low");
    }

    #[test]
    fn sanitize_candidate_answer_ignores_prompt_echo_and_system_reminders() {
        let answer = sanitize_candidate_answer(&[
            "You are explaining code that was produced in this exact opencode session context.\n\nScope: hunk\nPath: Cargo.lock",
            "<system-reminder>\nYour operational mode has changed from plan to build.\n</system-reminder>",
            "Intent: add SQLite-backed session discovery\n\nChange: add rusqlite and a Why This panel\n\nRisk: the export parser must ignore prompt echoes.",
        ])
        .unwrap();

        assert_eq!(
            answer,
            "Intent: add SQLite-backed session discovery\n\nChange: add rusqlite and a Why This panel\n\nRisk: the export parser must ignore prompt echoes."
        );
    }

    #[test]
    fn sanitize_candidate_answer_rejects_prompt_echo_only_payloads() {
        let prompt_echo = [
            "You are explaining code that was produced in this exact opencode session context.",
            "Scope: line",
        ];

        assert_eq!(sanitize_candidate_answer(&prompt_echo), None);
    }

    #[test]
    fn latest_assistant_text_reads_parts_from_db() {
        let temp = tempdir().unwrap();
        let db_path = temp.path().join("opencode.db");
        let repo_path = temp.path().join("repo");
        std::fs::create_dir_all(&repo_path).unwrap();

        let connection = Connection::open(&db_path).unwrap();
        create_message_tables(&connection);
        insert_message(
            &connection,
            "ses_1",
            "msg_user",
            1,
            json!({ "role": "user" }),
        );
        insert_part(
            &connection,
            "ses_1",
            "msg_user",
            1,
            json!({ "type": "text", "text": "why?" }),
        );
        insert_message(
            &connection,
            "ses_1",
            "msg_assistant",
            2,
            json!({ "role": "assistant" }),
        );
        insert_part(
            &connection,
            "ses_1",
            "msg_assistant",
            2,
            json!({ "type": "text", "text": "Intent: explain change" }),
        );
        insert_part(
            &connection,
            "ses_1",
            "msg_assistant",
            3,
            json!({ "type": "text", "text": "Risk: low" }),
        );

        let service = OpencodeService { repo_path, db_path };
        let answer = service.latest_assistant_text("ses_1").unwrap();
        assert_eq!(answer, "Intent: explain change\n\nRisk: low");
    }

    #[test]
    fn latest_assistant_text_skips_prompt_echo_only_messages() {
        let temp = tempdir().unwrap();
        let db_path = temp.path().join("opencode.db");
        let repo_path = temp.path().join("repo");
        std::fs::create_dir_all(&repo_path).unwrap();

        let connection = Connection::open(&db_path).unwrap();
        create_message_tables(&connection);
        insert_message(
            &connection,
            "ses_1",
            "msg_answer",
            1,
            json!({ "role": "assistant" }),
        );
        insert_part(
            &connection,
            "ses_1",
            "msg_answer",
            1,
            json!({ "type": "text", "text": "Intent: useful answer" }),
        );
        insert_message(
            &connection,
            "ses_1",
            "msg_echo",
            2,
            json!({ "role": "assistant" }),
        );
        insert_part(
            &connection,
            "ses_1",
            "msg_echo",
            2,
            json!({
                "type": "text",
                "text": "You are explaining code that was produced in this exact opencode session context.\n\nScope: file"
            }),
        );

        let service = OpencodeService { repo_path, db_path };
        let answer = service.latest_assistant_text("ses_1").unwrap();
        assert_eq!(answer, "Intent: useful answer");
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

    fn create_message_tables(connection: &Connection) {
        connection
            .execute_batch(
                "
                create table message (
                    id text primary key,
                    session_id text not null,
                    data text not null,
                    time_created integer not null
                );
                create table part (
                    id integer primary key,
                    session_id text not null,
                    message_id text not null,
                    data text not null,
                    time_created integer not null
                );
                ",
            )
            .unwrap();
    }

    fn insert_message(
        connection: &Connection,
        session_id: &str,
        message_id: &str,
        time_created: i64,
        data: Value,
    ) {
        connection
            .execute(
                "insert into message (id, session_id, data, time_created) values (?1, ?2, ?3, ?4)",
                rusqlite::params![message_id, session_id, data.to_string(), time_created],
            )
            .unwrap();
    }

    fn insert_part(
        connection: &Connection,
        session_id: &str,
        message_id: &str,
        time_created: i64,
        data: Value,
    ) {
        connection
            .execute(
                "insert into part (session_id, message_id, data, time_created) values (?1, ?2, ?3, ?4)",
                rusqlite::params![session_id, message_id, data.to_string(), time_created],
            )
            .unwrap();
    }
}
