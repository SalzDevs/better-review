use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use rusqlite::Connection;
use serde::Deserialize;
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
    pub summary: String,
    pub purpose: String,
    pub change: String,
    pub risk_level: WhyRiskLevel,
    pub risk_reason: String,
    pub fork_session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WhyRiskLevel {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct WhyAnswerPayload {
    version: u8,
    summary: String,
    purpose: String,
    change: String,
    risk_level: WhyRiskLevel,
    risk_reason: String,
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
}

impl WhyTarget {
    pub fn label(&self) -> String {
        match self {
            Self::File { path, .. } => format!("file {path}"),
            Self::Hunk { path, header, .. } => format!("hunk {path} {header}"),
        }
    }

    pub fn cache_key(&self, session_id: &str) -> String {
        match self {
            Self::File { path, .. } => format!("{session_id}:file:{path}"),
            Self::Hunk { path, header, .. } => format!("{session_id}:hunk:{path}:{header}"),
        }
    }

    fn prompt(&self) -> String {
        let instruction = concat!(
            "You are explaining code that was produced in this exact opencode session context. ",
            "Return ONLY valid JSON with no markdown, no code fences, and no extra text. ",
            "Schema exactly: {\"version\":1,\"summary\":string,\"purpose\":string,\"change\":string,\"risk_level\":\"low\"|\"medium\"|\"high\",\"risk_reason\":string}. ",
            "Each field must be concise plain text. Keep it specific to the selected scope. If uncertain, say so in the relevant field only."
        );

        match self {
            Self::File { path, status, diff } => format!(
                "{instruction}\n\nScope: file\nPath: {path}\nStatus: {status}\n\nDiff:\n{diff}"
            ),
            Self::Hunk { path, header, diff } => format!(
                "{instruction}\n\nScope: hunk\nPath: {path}\nHeader: {header}\n\nHunk diff:\n{diff}"
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

    pub async fn ask_why(
        &self,
        session_id: &str,
        target: &WhyTarget,
        model: Option<&str>,
    ) -> Result<WhyAnswer> {
        let prompt = target.prompt();
        let output = timeout(
            RUN_TIMEOUT,
            self.run_forked_session(session_id, &prompt, model),
        )
        .await
        .map_err(|_| anyhow!("opencode timed out while generating a why-this explanation"))??;

        if let Some(payload) = parse_answer_from_run_output(&output)? {
            let fork_session_id = extract_fork_session_id_from_run_output(&output, session_id)
                .unwrap_or_else(|| session_id.to_string());
            return Ok(WhyAnswer {
                summary: payload.summary,
                purpose: payload.purpose,
                change: payload.change,
                risk_level: payload.risk_level,
                risk_reason: payload.risk_reason,
                fork_session_id,
            });
        }

        let fork_session_id = extract_fork_session_id_from_run_output(&output, session_id)
            .context("opencode did not report a fork session id")?;
        let payload = self.wait_for_answer(&fork_session_id).await?;

        Ok(WhyAnswer {
            summary: payload.summary,
            purpose: payload.purpose,
            change: payload.change,
            risk_level: payload.risk_level,
            risk_reason: payload.risk_reason,
            fork_session_id,
        })
    }

    pub async fn list_models(&self) -> Result<Vec<String>> {
        let output = Command::new("opencode")
            .args(["models"])
            .output()
            .await
            .context("failed to list opencode models")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            bail!(
                "opencode model list failed{}",
                if stderr.is_empty() {
                    String::new()
                } else {
                    format!(": {stderr}")
                }
            );
        }

        let models = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(ToString::to_string)
            .collect::<Vec<_>>();

        Ok(models)
    }

    pub fn session_model(&self, session_id: &str) -> Result<Option<String>> {
        let connection = Connection::open(&self.db_path)
            .with_context(|| format!("failed to open {}", self.db_path.display()))?;
        let mut statement = connection.prepare(
            "select data from message
             where session_id = ?1
             order by time_created desc",
        )?;
        let rows = statement.query_map([session_id], |row| row.get::<_, String>(0))?;

        for row in rows {
            let data = row?;
            let message: Value =
                serde_json::from_str(&data).context("failed to parse opencode message JSON")?;
            if message_role(&message) != Some("assistant") {
                continue;
            }

            if let Some(model) = message.get("modelID").and_then(Value::as_str) {
                let provider = message.get("providerID").and_then(Value::as_str);
                return Ok(Some(normalize_model(provider, model)));
            }
        }

        Ok(None)
    }

    async fn run_forked_session(
        &self,
        session_id: &str,
        prompt: &str,
        model: Option<&str>,
    ) -> Result<String> {
        let repo_dir = self.repo_path.to_string_lossy().to_string();
        let mut command = Command::new("opencode");
        command
            .arg("run")
            .arg("--pure")
            .arg("--session")
            .arg(session_id)
            .arg("--fork")
            .arg("--format")
            .arg("json")
            .arg("--dir")
            .arg(repo_dir);
        if let Some(model) = model {
            command.arg("--model").arg(model);
        }
        command.arg(prompt);

        let output = command.output().await.context("failed to start opencode")?;

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

    async fn wait_for_answer(&self, session_id: &str) -> Result<WhyAnswerPayload> {
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

    fn latest_assistant_text(&self, session_id: &str) -> Result<WhyAnswerPayload> {
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
            if let Some(answer) = parse_candidate_answer(&text_refs)? {
                return Ok(answer);
            }
        }

        bail!("opencode did not return a valid why-this JSON explanation")
    }
}

fn normalize_model(provider: Option<&str>, model: &str) -> String {
    if model.contains('/') {
        model.to_string()
    } else if let Some(provider) = provider {
        format!("{provider}/{model}")
    } else {
        model.to_string()
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

#[cfg(test)]
fn extract_session_id_from_run_output(output: &str) -> Option<String> {
    session_ids_from_run_output(output).into_iter().next()
}

fn extract_fork_session_id_from_run_output(
    output: &str,
    parent_session_id: &str,
) -> Option<String> {
    let ids = session_ids_from_run_output(output);
    ids.iter()
        .find(|id| id.as_str() != parent_session_id)
        .cloned()
        .or_else(|| ids.into_iter().next())
}

fn session_ids_from_run_output(output: &str) -> Vec<String> {
    let mut ids = Vec::new();

    for line in output.lines() {
        let value = match serde_json::from_str::<Value>(line) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let event_type = value.get("type").and_then(Value::as_str);
        let nested = value
            .get("part")
            .and_then(|part| part.get("sessionID"))
            .and_then(Value::as_str);
        let top_level = value.get("sessionID").and_then(Value::as_str);

        let allow_ids = matches!(event_type, Some("step_start" | "text" | "step_finish"))
            || (event_type.is_none() && nested.is_some());
        if !allow_ids {
            continue;
        }

        let candidates = if matches!(event_type, Some("step_start" | "text" | "step_finish")) {
            vec![nested, top_level]
        } else {
            vec![nested]
        };

        for session_id in candidates.into_iter().flatten() {
            if !ids.iter().any(|existing| existing == session_id) {
                ids.push(session_id.to_string());
            }
        }
    }

    ids
}

fn parse_answer_from_run_output(output: &str) -> Result<Option<WhyAnswerPayload>> {
    let mut parts = Vec::new();

    for line in output.lines() {
        let value = match serde_json::from_str::<Value>(line) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let event_type = value.get("type").and_then(Value::as_str);
        if event_type != Some("text") {
            continue;
        }

        if let Some(text) = value
            .get("part")
            .and_then(|part| part.get("text"))
            .and_then(Value::as_str)
        {
            parts.push(text.to_string());
        }
    }

    let refs = parts.iter().map(String::as_str).collect::<Vec<_>>();
    parse_candidate_answer(&refs)
}

fn parse_candidate_answer(parts: &[&str]) -> Result<Option<WhyAnswerPayload>> {
    let cleaned = parts
        .iter()
        .filter_map(|part| sanitize_candidate_part(part))
        .collect::<Vec<_>>()
        .join("\n");
    let cleaned = cleaned.trim().to_string();

    if cleaned.is_empty() {
        return Ok(None);
    }

    let Some(json_payload) = extract_first_json_object(&cleaned) else {
        return Ok(None);
    };
    let payload: WhyAnswerPayload =
        serde_json::from_str(&json_payload).context("failed to parse why-this JSON payload")?;
    Ok(Some(normalize_payload(payload)))
}

fn extract_first_json_object(text: &str) -> Option<String> {
    let mut start_index = None;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (index, ch) in text.char_indices() {
        if let Some(start) = start_index {
            if in_string {
                if escaped {
                    escaped = false;
                    continue;
                }
                match ch {
                    '\\' => escaped = true,
                    '"' => in_string = false,
                    _ => {}
                }
                continue;
            }

            match ch {
                '"' => in_string = true,
                '{' => depth += 1,
                '}' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return Some(text[start..=index].to_string());
                    }
                }
                _ => {}
            }
        } else if ch == '{' {
            start_index = Some(index);
            depth = 1;
        }
    }

    None
}

fn normalize_payload(payload: WhyAnswerPayload) -> WhyAnswerPayload {
    WhyAnswerPayload {
        version: payload.version,
        summary: normalize_answer_field(&payload.summary),
        purpose: normalize_answer_field(&payload.purpose),
        change: normalize_answer_field(&payload.change),
        risk_level: payload.risk_level,
        risk_reason: normalize_answer_field(&payload.risk_reason),
    }
}

fn normalize_answer_field(value: &str) -> String {
    value
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(2)
        .collect::<Vec<_>>()
        .join("\n")
}

fn sanitize_candidate_part(part: &str) -> Option<String> {
    let cleaned = strip_system_reminder(part);
    let cleaned = cleaned.trim().to_string();

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
    let normalized = trimmed.trim_start_matches(['"', '\'']);
    normalized.starts_with(
        "You are explaining code that was produced in this exact opencode session context.",
    ) || normalized.starts_with("Return ONLY valid JSON")
        || normalized.starts_with("Scope: ")
        || normalized.starts_with("Path: ")
        || normalized.starts_with("Status: ")
        || normalized.starts_with("Diff:")
        || normalized.starts_with("Thinking:")
        || normalized.contains("Schema exactly: {\"version\":1")
}

fn strip_system_reminder(text: &str) -> String {
    let mut cleaned = text.to_string();
    if let Some(reminder_index) = cleaned.find("<system-reminder>") {
        cleaned.truncate(reminder_index);
    }
    cleaned
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
    fn extract_fork_session_id_prefers_non_parent_id() {
        let output = concat!(
            "{\"type\":\"step_start\",\"sessionID\":\"ses_parent\",\"part\":{\"sessionID\":\"ses_fork\"}}\n",
            "{\"type\":\"text\",\"sessionID\":\"ses_parent\",\"part\":{\"sessionID\":\"ses_fork\",\"text\":\"...\"}}\n"
        );

        assert_eq!(
            extract_fork_session_id_from_run_output(output, "ses_parent"),
            Some("ses_fork".to_string())
        );
    }

    #[test]
    fn parse_candidate_answer_parses_valid_json_payload() {
        let answer = parse_candidate_answer(&[
            r#"{"version":1,"summary":"update parser","purpose":"make explain output deterministic","change":"tighten output schema","risk_level":"low","risk_reason":"small formatting drift"}"#,
        ])
        .unwrap()
        .unwrap();

        assert_eq!(answer.version, 1);
        assert_eq!(answer.summary, "update parser");
        assert_eq!(answer.purpose, "make explain output deterministic");
        assert_eq!(answer.change, "tighten output schema");
        assert_eq!(answer.risk_level, WhyRiskLevel::Low);
        assert_eq!(answer.risk_reason, "small formatting drift");
    }

    #[test]
    fn parse_candidate_answer_ignores_prompt_echo_and_system_reminders() {
        let answer = parse_candidate_answer(&[
            "You are explaining code that was produced in this exact opencode session context.\n\nScope: hunk\nPath: Cargo.lock",
            "<system-reminder>\nYour operational mode has changed from plan to build.\n</system-reminder>",
            r#"{"version":1,"summary":"add SQLite-backed session discovery","purpose":"let Explain reuse repo-local opencode context","change":"add rusqlite and an Explain panel","risk_level":"medium","risk_reason":"the parser must ignore prompt echoes"}"#,
        ])
        .unwrap()
        .unwrap();

        assert_eq!(answer.summary, "add SQLite-backed session discovery");
        assert_eq!(
            answer.purpose,
            "let Explain reuse repo-local opencode context"
        );
        assert_eq!(answer.change, "add rusqlite and an Explain panel");
        assert_eq!(answer.risk_level, WhyRiskLevel::Medium);
        assert_eq!(answer.risk_reason, "the parser must ignore prompt echoes");
    }

    #[test]
    fn parse_candidate_answer_rejects_prompt_echo_only_payloads() {
        let prompt_echo = [
            "You are explaining code that was produced in this exact opencode session context.",
            "Scope: line",
        ];

        assert_eq!(parse_candidate_answer(&prompt_echo).unwrap(), None);
    }

    #[test]
    fn parse_candidate_answer_rejects_quoted_prompt_echo_only_payloads() {
        let prompt_echo = [
            "\"You are explaining code that was produced in this exact opencode session context.",
            "Scope: file",
        ];

        assert_eq!(parse_candidate_answer(&prompt_echo).unwrap(), None);
    }

    #[test]
    fn parse_candidate_answer_rejects_schema_only_prompt_echo() {
        let prompt_echo = [
            "\"Return ONLY valid JSON with no markdown or extra text. Schema exactly: {\\\"version\\\":1,\\\"summary\\\":string,\\\"purpose\\\":string,\\\"change\\\":string,\\\"risk_level\\\":\\\"low\\\"|\\\"medium\\\"|\\\"high\\\",\\\"risk_reason\\\":string}. Explain this tiny diff intent: +update docs, -remove old keybindings.\"",
            "<system-reminder>\nYour operational mode has changed from plan to build.\n</system-reminder>",
        ];

        assert_eq!(parse_candidate_answer(&prompt_echo).unwrap(), None);
    }

    #[test]
    fn parse_candidate_answer_normalizes_multiline_fields() {
        let payload = [
            "You are explaining code that was produced in this exact opencode session context. Scope: file Path: README.md Diff: ...",
            r#"{"version":1,"summary":"document new explain controls\nwith deterministic output","purpose":"make explain easier to understand","change":"replace Space with v/V\nand add model picker details","risk_level":"medium","risk_reason":"users may still use old keybinds initially\nuntil they relearn the flow"}"#,
            "<system-reminder>internal mode switch</system-reminder>",
        ];

        let answer = parse_candidate_answer(&payload).unwrap().unwrap();
        assert_eq!(
            answer.summary,
            "document new explain controls\nwith deterministic output"
        );
        assert_eq!(answer.purpose, "make explain easier to understand");
        assert_eq!(
            answer.change,
            "replace Space with v/V\nand add model picker details"
        );
        assert_eq!(answer.risk_level, WhyRiskLevel::Medium);
        assert_eq!(
            answer.risk_reason,
            "users may still use old keybinds initially\nuntil they relearn the flow"
        );
    }

    #[test]
    fn parse_candidate_answer_extracts_json_from_noisy_wrapper_text() {
        let answer = parse_candidate_answer(&[
            r#"Here is the explanation you asked for:
{"version":1,"summary":"explain selection","purpose":"clarify grouped edits","change":"summarize grouped edits","risk_level":"low","risk_reason":"minor chance of over-summary"}
Thanks!"#,
        ])
        .unwrap()
        .unwrap();

        assert_eq!(answer.summary, "explain selection");
        assert_eq!(answer.purpose, "clarify grouped edits");
        assert_eq!(answer.change, "summarize grouped edits");
        assert_eq!(answer.risk_level, WhyRiskLevel::Low);
        assert_eq!(answer.risk_reason, "minor chance of over-summary");
    }

    #[test]
    fn parse_candidate_answer_skips_non_json_text() {
        let answer = parse_candidate_answer(&[
            "Thinking through the diff before I answer.",
            "I need another pass.",
        ])
        .unwrap();

        assert_eq!(answer, None);
    }

    #[test]
    fn extract_first_json_object_handles_braces_inside_strings() {
        let payload = extract_first_json_object(
            "preface {\"version\":1,\"summary\":\"explain {brace}\",\"purpose\":\"keep explain stable\",\"change\":\"keep parser stable\",\"risk_level\":\"medium\",\"risk_reason\":\"parsing can fail if braces confuse extraction\"} trailing",
        );

        assert_eq!(
            payload,
            Some(
                "{\"version\":1,\"summary\":\"explain {brace}\",\"purpose\":\"keep explain stable\",\"change\":\"keep parser stable\",\"risk_level\":\"medium\",\"risk_reason\":\"parsing can fail if braces confuse extraction\"}".to_string()
            )
        );
    }

    #[test]
    fn parse_answer_from_run_output_reads_text_event_payload() {
        let output = concat!(
            "{\"type\":\"step_start\",\"sessionID\":\"ses_run\"}\n",
            "{\"type\":\"text\",\"sessionID\":\"ses_run\",\"part\":{\"text\":\"{\\\"version\\\":1,\\\"summary\\\":\\\"explain\\\",\\\"purpose\\\":\\\"clarify\\\",\\\"change\\\":\\\"summarize\\\",\\\"risk_level\\\":\\\"low\\\",\\\"risk_reason\\\":\\\"minor\\\"}\"}}\n",
            "{\"type\":\"step_finish\",\"sessionID\":\"ses_run\"}\n"
        );

        let answer = parse_answer_from_run_output(output).unwrap().unwrap();
        assert_eq!(answer.summary, "explain");
        assert_eq!(answer.purpose, "clarify");
        assert_eq!(answer.change, "summarize");
        assert_eq!(answer.risk_level, WhyRiskLevel::Low);
        assert_eq!(answer.risk_reason, "minor");
    }

    #[test]
    fn parse_answer_from_run_output_skips_prompt_echo_text() {
        let output = concat!(
            "{\"type\":\"text\",\"sessionID\":\"ses_run\",\"part\":{\"text\":\"You are explaining code that was produced in this exact opencode session context. Return ONLY valid JSON with no markdown, no code fences, and no extra text. Schema exactly: {\\\"version\\\":1,\\\"summary\\\":string,\\\"purpose\\\":string,\\\"change\\\":string,\\\"risk_level\\\":\\\"low\\\"|\\\"medium\\\"|\\\"high\\\",\\\"risk_reason\\\":string}. Scope: file Path: README.md Status: modified Diff: --- README.md +++ README.md\"}}\n",
            "{\"type\":\"step_finish\",\"sessionID\":\"ses_run\"}\n"
        );

        let answer = parse_answer_from_run_output(output).unwrap();
        assert_eq!(answer, None);
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
            json!({ "type": "text", "text": r#"{"version":1,"summary":"explain change","purpose":"clarify the current delta","change":"summarize delta","risk_level":"low","risk_reason":"minimal regression risk"}"# }),
        );

        let service = OpencodeService { repo_path, db_path };
        let answer = service.latest_assistant_text("ses_1").unwrap();
        assert_eq!(answer.summary, "explain change");
        assert_eq!(answer.purpose, "clarify the current delta");
        assert_eq!(answer.change, "summarize delta");
        assert_eq!(answer.risk_level, WhyRiskLevel::Low);
        assert_eq!(answer.risk_reason, "minimal regression risk");
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
            json!({ "type": "text", "text": r#"{"version":1,"summary":"useful answer","purpose":"clarify the current diff","change":"explain current diff","risk_level":"low","risk_reason":"small chance of over-summary"}"# }),
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
        assert_eq!(answer.summary, "useful answer");
    }

    #[test]
    fn latest_assistant_text_skips_newer_non_json_assistant_messages() {
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
            json!({ "type": "text", "text": r#"{"version":1,"summary":"useful answer","purpose":"clarify the current diff","change":"explain current diff","risk_level":"low","risk_reason":"small chance of over-summary"}"# }),
        );
        insert_message(
            &connection,
            "ses_1",
            "msg_noisy",
            2,
            json!({ "role": "assistant" }),
        );
        insert_part(
            &connection,
            "ses_1",
            "msg_noisy",
            2,
            json!({
                "type": "text",
                "text": "Thinking through the diff before I answer."
            }),
        );

        let service = OpencodeService { repo_path, db_path };
        let answer = service.latest_assistant_text("ses_1").unwrap();
        assert_eq!(answer.summary, "useful answer");
    }

    #[test]
    fn session_model_reads_provider_and_model_from_latest_assistant_message() {
        let temp = tempdir().unwrap();
        let db_path = temp.path().join("opencode.db");
        let repo_path = temp.path().join("repo");
        std::fs::create_dir_all(&repo_path).unwrap();

        let connection = Connection::open(&db_path).unwrap();
        create_message_tables(&connection);
        insert_message(
            &connection,
            "ses_1",
            "msg_assistant",
            1,
            json!({
                "role": "assistant",
                "providerID": "github-copilot",
                "modelID": "gpt-5.3-codex"
            }),
        );

        let service = OpencodeService { repo_path, db_path };
        let model = service.session_model("ses_1").unwrap();
        assert_eq!(model, Some("github-copilot/gpt-5.3-codex".to_string()));
    }

    #[test]
    fn normalize_model_keeps_fully_qualified_model_unchanged() {
        assert_eq!(
            normalize_model(Some("openai"), "openai/gpt-5"),
            "openai/gpt-5"
        );
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
