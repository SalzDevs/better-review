use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{
    Clear as TerminalClear, ClearType, EnterAlternateScreen, LeaveAlternateScreen,
    disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui_core::style::{Modifier, Style};
use ratatui_core::widgets::Widget;
use ratatui_interact::components::{AnimatedText, AnimatedTextState, AnimatedTextStyle};
use ratatui_textarea::{TextArea, WrapMode};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::domain::diff::{DiffLineKind, FileDiff, ReviewStatus};
use crate::services::git::GitService;
use crate::services::opencode::{
    OpencodeService, OpencodeSession, WhyAnswer, WhyRiskLevel, WhyTarget, why_target_for_file,
    why_target_for_hunk,
};
use crate::settings::{AppSettings, SettingsStore};
use crate::ui::styles;

pub async fn run() -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        TerminalClear(ClearType::All),
        TerminalClear(ClearType::Purge)
    )?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_app(&mut terminal).await;

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;

    result
}

struct App {
    repo_path: PathBuf,
    git: GitService,
    opencode: Option<OpencodeService>,
    settings: AppSettings,
    settings_store: SettingsStore,
    settings_cursor: usize,
    saved_model_cursor: usize,
    session_state: SessionUiState,
    why_this: WhyThisUiState,
    status: String,
    screen: Screen,
    review: ReviewUiState,
    overlay: Overlay,
    had_staged_changes_on_open: bool,
    review_busy: bool,
    logo_animation: AnimatedTextState,
    tx: mpsc::UnboundedSender<Message>,
    rx: mpsc::UnboundedReceiver<Message>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Overlay {
    None,
    CommitPrompt,
    Settings,
    SettingsModelPicker,
    ExplainMenu,
    SessionPicker,
    ModelPicker,
    ExplainHistory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Home,
    Review,
}

#[derive(Default)]
struct ReviewUiState {
    files: Vec<FileDiff>,
    cursor_file: usize,
    cursor_hunk: usize,
    cursor_line: usize,
    focus: ReviewFocus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ReviewFocus {
    #[default]
    Files,
    Hunks,
}

enum Message {
    HunkSync {
        file_index: usize,
        original_file: FileDiff,
        updated_file: FileDiff,
        success_status: String,
        result: Result<(), String>,
    },
    WhyThis {
        job_id: u64,
        cache_key: String,
        label: String,
        result: Result<WhyAnswer, String>,
    },
    ModelList {
        result: Result<Vec<String>, String>,
    },
}

#[derive(Default)]
struct SessionUiState {
    sessions: Vec<OpencodeSession>,
    selected: Option<usize>,
    cursor: usize,
}

#[derive(Default)]
struct WhyThisUiState {
    cache: HashMap<String, WhyAnswer>,
    runs: Vec<ExplainRun>,
    current_run_id: Option<u64>,
    history_cursor: usize,
    next_run_id: u64,
    model: WhyModelState,
    model_override: Option<WhyModelChoice>,
    return_to_menu: bool,
}

struct ExplainRun {
    id: u64,
    label: String,
    target: WhyTarget,
    context_source_id: String,
    context_source_label: String,
    requested_model: Option<String>,
    model_label: String,
    cache_key: String,
    status: ExplainRunStatus,
    result: Option<WhyAnswer>,
    error: Option<String>,
    handle: Option<JoinHandle<()>>,
}

enum ExplainRunStatus {
    Running,
    Ready,
    Failed,
    Cancelled,
}

#[derive(Default)]
struct WhyModelState {
    available: Vec<String>,
    cursor: usize,
    auto_session_model: Option<String>,
    loading: bool,
    last_loaded_at: Option<Instant>,
    last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
enum WhyModelChoice {
    #[default]
    Auto,
    Explicit(String),
}

impl App {
    async fn new() -> Result<Self> {
        let repo_path = std::env::current_dir()?;
        let git = GitService::new(&repo_path);
        let opencode = OpencodeService::new(&repo_path).ok();
        let settings_store = SettingsStore::new()?;
        let settings = settings_store
            .load()
            .unwrap_or_else(|_| AppSettings::default());
        let (tx, rx) = mpsc::unbounded_channel();
        let mut app = Self {
            repo_path,
            git,
            opencode,
            settings,
            settings_store,
            settings_cursor: 0,
            saved_model_cursor: 0,
            session_state: SessionUiState::default(),
            why_this: WhyThisUiState::default(),
            status: "Run your coding agent elsewhere, then open better-review to review changes."
                .to_string(),
            screen: Screen::Home,
            review: ReviewUiState::default(),
            overlay: Overlay::None,
            had_staged_changes_on_open: false,
            review_busy: false,
            logo_animation: AnimatedTextState::with_interval(120),
            tx,
            rx,
        };
        app.load_initial_state().await?;
        Ok(app)
    }

    async fn load_initial_state(&mut self) -> Result<()> {
        let (_, files) = self.git.collect_diff().await?;
        self.review.files = files;
        self.had_staged_changes_on_open = self.git.has_staged_changes().await?;
        self.apply_saved_settings();
        self.load_sessions()?;
        self.refresh_auto_model();

        Ok(())
    }

    fn apply_saved_settings(&mut self) {
        self.saved_model_cursor = saved_model_picker_cursor(
            self.settings.explain.default_model.as_deref(),
            &self.why_this.model.available,
        );
    }

    fn load_sessions(&mut self) -> Result<()> {
        let Some(opencode) = &self.opencode else {
            return Ok(());
        };

        let sessions = opencode.list_repo_sessions()?;
        let selected = if sessions.is_empty() { None } else { Some(0) };
        self.session_state = SessionUiState {
            sessions,
            selected,
            cursor: 0,
        };
        Ok(())
    }

    fn review_counts(&self) -> ReviewCounts {
        let mut counts = ReviewCounts::default();

        for file in &self.review.files {
            if file.hunks.is_empty() {
                counts.bump(&file.review_status);
            } else {
                for hunk in &file.hunks {
                    counts.bump(&hunk.review_status);
                }
            }
        }

        counts
    }

    fn open_commit_prompt(&mut self) -> TextArea<'static> {
        self.overlay = Overlay::CommitPrompt;
        self.status = "Write a commit message for the accepted changes.".to_string();

        new_commit_message_input()
    }

    fn active_session(&self) -> Option<&OpencodeSession> {
        self.session_state
            .selected
            .and_then(|index| self.session_state.sessions.get(index))
    }

    fn refresh_auto_model(&mut self) {
        self.why_this.model.auto_session_model = None;
        let session_id = self.active_session().map(|session| session.id.clone());
        let Some(opencode) = &self.opencode else {
            return;
        };
        let Some(session_id) = session_id else {
            return;
        };

        if let Ok(model) = opencode.session_model(&session_id) {
            self.why_this.model.auto_session_model = model;
        }
    }
}

#[derive(Default)]
struct ReviewCounts {
    unreviewed: usize,
    accepted: usize,
    rejected: usize,
}

const BRAND_ICON: &str = "⌕";
const BRAND_ICON_ALT: &str = "✓";
const BRAND_WORDMARK: &str = "better-review";
const MODEL_CACHE_TTL: Duration = Duration::from_secs(180);
fn brand_lockup_width() -> u16 {
    BRAND_ICON.chars().count() as u16 + 2 + BRAND_WORDMARK.chars().count() as u16
}

fn current_brand_icon(animation: &AnimatedTextState) -> &'static str {
    if animation.frame < 128 {
        BRAND_ICON
    } else {
        BRAND_ICON_ALT
    }
}

impl ReviewCounts {
    fn bump(&mut self, status: &ReviewStatus) {
        match status {
            ReviewStatus::Unreviewed => self.unreviewed += 1,
            ReviewStatus::Accepted => self.accepted += 1,
            ReviewStatus::Rejected => self.rejected += 1,
        }
    }
}

fn new_commit_message_input() -> TextArea<'static> {
    let mut commit_message = TextArea::default();
    commit_message.set_placeholder_text("Write the commit message for accepted changes");
    commit_message.set_wrap_mode(WrapMode::WordOrGlyph);
    commit_message
}

async fn submit_commit_message(
    app: &mut App,
    commit_message: &mut TextArea<'static>,
) -> Result<()> {
    let message = commit_message.lines().join("\n").trim().to_string();
    if message.is_empty() {
        app.status = "Write a commit message first.".to_string();
        return Ok(());
    }

    if !app.git.has_staged_changes().await? {
        app.status = "No accepted changes are staged yet.".to_string();
        return Ok(());
    }

    if app.had_staged_changes_on_open {
        app.status =
            "Cannot commit from better-review because the app opened with unrelated staged changes."
                .to_string();
        return Ok(());
    }

    app.git.commit_staged(&message).await?;
    refresh_review_files(app).await?;
    app.overlay = Overlay::None;
    app.status = "Committed accepted changes.".to_string();
    *commit_message = new_commit_message_input();

    Ok(())
}

async fn refresh_review_files(app: &mut App) -> Result<()> {
    let (_, files) = app.git.collect_diff().await?;
    app.review.files = files;
    app.review.cursor_file = 0;
    app.review.cursor_hunk = 0;
    app.review.cursor_line = 0;
    app.review.focus = ReviewFocus::Files;
    app.screen = if app.review.files.is_empty() {
        Screen::Home
    } else {
        Screen::Review
    };
    Ok(())
}

async fn run_app(terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>) -> Result<()> {
    let mut app = App::new().await?;
    let mut commit_message = new_commit_message_input();

    loop {
        app.logo_animation
            .tick_with_text_width(usize::from(brand_lockup_width()));

        while let Ok(message) = app.rx.try_recv() {
            match message {
                Message::HunkSync {
                    file_index,
                    original_file,
                    updated_file,
                    success_status,
                    result,
                } => {
                    app.review_busy = false;
                    if let Some(file) = app.review.files.get_mut(file_index) {
                        match result {
                            Ok(()) => {
                                *file = updated_file;
                                sync_cursor_line_to_hunk(&mut app.review);
                                app.status = success_status;
                            }
                            Err(err) => {
                                *file = original_file;
                                app.status = err;
                            }
                        }
                    }
                }
                Message::WhyThis {
                    job_id,
                    cache_key,
                    label,
                    result,
                } => {
                    if let Some(index) = find_explain_run_index_by_id(&app.why_this, job_id) {
                        let is_running = matches!(
                            app.why_this.runs.get(index).map(|run| &run.status),
                            Some(ExplainRunStatus::Running)
                        );
                        if !is_running {
                            continue;
                        }

                        if let Some(run) = app.why_this.runs.get_mut(index) {
                            run.handle = None;
                            match result {
                                Ok(answer) => {
                                    app.status = format!("Loaded explanation for {label}.");
                                    app.why_this.cache.insert(cache_key, answer.clone());
                                    run.status = ExplainRunStatus::Ready;
                                    run.result = Some(answer);
                                    run.error = None;
                                }
                                Err(error) => {
                                    app.status =
                                        format!("Explain failed: {error}. Press r to retry.");
                                    run.status = ExplainRunStatus::Failed;
                                    run.error = Some(error);
                                    run.result = None;
                                }
                            }
                        }
                        app.why_this.current_run_id = Some(job_id);
                        app.why_this.history_cursor = index;
                    }
                }
                Message::ModelList { result } => {
                    app.why_this.model.loading = false;
                    match result {
                        Ok(mut models) => {
                            ensure_model_present(
                                &mut models,
                                explicit_model_choice(&current_model_choice(&app)),
                            );
                            ensure_model_present(
                                &mut models,
                                app.settings.explain.default_model.as_deref(),
                            );

                            app.why_this.model.available = models;
                            sync_model_picker_cursors(&mut app);
                            app.why_this.model.last_loaded_at = Some(Instant::now());
                            app.why_this.model.last_error = None;
                            if app.overlay == Overlay::ModelPicker
                                || app.overlay == Overlay::SettingsModelPicker
                            {
                                app.status = model_picker_status_message(app.overlay).to_string();
                            }
                        }
                        Err(error) => {
                            app.why_this.model.last_error = Some(error.clone());
                            if app.overlay == Overlay::ModelPicker
                                || app.overlay == Overlay::SettingsModelPicker
                            {
                                app.status = format!(
                                    "Could not load Explain models: {error}. Close and reopen the picker to retry."
                                );
                            }
                        }
                    }
                }
            }
        }

        terminal.draw(|frame| draw(frame, &app, &commit_message))?;

        if event::poll(Duration::from_millis(16))?
            && let Event::Key(key) = event::read()?
        {
            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                break;
            }

            match app.overlay {
                Overlay::CommitPrompt => match key.code {
                    KeyCode::Esc => {
                        app.overlay = Overlay::None;
                        app.status = "Commit cancelled. Review remains active.".to_string();
                    }
                    KeyCode::Enter => {
                        submit_commit_message(&mut app, &mut commit_message).await?;
                    }
                    _ => {
                        commit_message.input(to_textarea_input(key));
                    }
                },
                Overlay::Settings => handle_settings_key(&mut app, key),
                Overlay::SettingsModelPicker => handle_saved_model_picker_key(&mut app, key),
                Overlay::ExplainMenu => handle_explain_menu_key(&mut app, key).await?,
                Overlay::SessionPicker => handle_session_picker_key(&mut app, key),
                Overlay::ModelPicker => handle_model_picker_key(&mut app, key),
                Overlay::ExplainHistory => handle_explain_history_key(&mut app, key),
                Overlay::None => {
                    if key.code == KeyCode::Enter && app.screen == Screen::Home {
                        if app.review.files.is_empty() {
                            app.status =
                                "No reviewable changes yet. Run your coding agent, then reopen better-review."
                                    .to_string();
                        } else {
                            app.screen = Screen::Review;
                            app.status = "Review workspace ready.".to_string();
                        }
                        continue;
                    }

                    if key.code == KeyCode::Char('c') {
                        if app.review.files.is_empty() {
                            app.status =
                                "Cannot commit yet because there are no reviewable changes in this repository."
                                    .to_string();
                        } else if app.review_busy {
                            app.status =
                                "Wait for the current review update to finish.".to_string();
                        } else {
                            commit_message = app.open_commit_prompt();
                        }
                        continue;
                    }

                    if key.code == KeyCode::Char('s') {
                        open_settings(&mut app);
                        continue;
                    }

                    handle_review_key(&mut app, key).await?;
                }
            }
        }
    }

    Ok(())
}

async fn handle_review_key(app: &mut App, key: KeyEvent) -> Result<()> {
    if app.screen != Screen::Review {
        return Ok(());
    }

    if app.review.files.is_empty() {
        return Ok(());
    }

    if app.review_busy {
        match key.code {
            KeyCode::Esc => app.review.focus = ReviewFocus::Files,
            _ => app.status = "Updating review state...".to_string(),
        }
        return Ok(());
    }

    match key.code {
        KeyCode::Enter => {
            app.review.focus = ReviewFocus::Hunks;
            sync_cursor_line_to_hunk(&mut app.review);
        }
        KeyCode::Esc => {
            if app.review.focus == ReviewFocus::Hunks {
                app.review.focus = ReviewFocus::Files;
            } else {
                app.screen = Screen::Home;
                app.status = "Back on the better-review home screen.".to_string();
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if app.review.focus == ReviewFocus::Files {
                app.review.cursor_file = app.review.cursor_file.saturating_sub(1);
                app.review.cursor_hunk = 0;
                app.review.cursor_line = 0;
            } else {
                move_review_cursor_by_line(app, -1);
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if app.review.focus == ReviewFocus::Files {
                if app.review.cursor_file + 1 < app.review.files.len() {
                    app.review.cursor_file += 1;
                    app.review.cursor_hunk = 0;
                    app.review.cursor_line = 0;
                }
            } else {
                move_review_cursor_by_line(app, 1);
            }
        }
        KeyCode::Tab if app.review.focus == ReviewFocus::Hunks => {
            if let Some(file) = app.review.files.get(app.review.cursor_file)
                && !file.hunks.is_empty()
            {
                app.review.cursor_hunk = (app.review.cursor_hunk + 1) % file.hunks.len();
                sync_cursor_line_to_hunk(&mut app.review);
            }
        }
        KeyCode::Char('y') => {
            if app.review.focus == ReviewFocus::Files {
                if let Some(file) = app.review.files.get_mut(app.review.cursor_file) {
                    match app.git.accept_file(file).await {
                        Ok(()) => app.status = "Accepted file changes.".to_string(),
                        Err(err) => app.status = format!("Could not accept file: {err}"),
                    }
                }
            } else if let Some(file) = app.review.files.get_mut(app.review.cursor_file)
                && file.hunks.get(app.review.cursor_hunk).is_some()
            {
                let file_index = app.review.cursor_file;
                let original_file = file.clone();
                let mut updated_file = file.clone();
                updated_file.hunks[app.review.cursor_hunk].review_status = ReviewStatus::Accepted;
                updated_file.sync_review_status();

                let tx = app.tx.clone();
                let git = app.git.clone();
                app.review_busy = true;
                app.status = "Applying accepted hunk...".to_string();

                tokio::spawn(async move {
                    let result = git
                        .sync_file_hunks_to_index(&updated_file)
                        .await
                        .map_err(|err| format!("Could not accept hunk: {err}"));
                    let _ = tx.send(Message::HunkSync {
                        file_index,
                        original_file,
                        updated_file,
                        success_status: "Accepted hunk.".to_string(),
                        result,
                    });
                });
            }
        }
        KeyCode::Char('x') => {
            if app.review.focus == ReviewFocus::Files {
                if let Some(file) = app.review.files.get_mut(app.review.cursor_file) {
                    let result = app.git.reject_file_in_place(file).await;

                    match result {
                        Ok(()) => app.status = "Rejected file changes.".to_string(),
                        Err(err) => app.status = format!("Could not reject file: {err}"),
                    }
                }
            } else if let Some(file) = app.review.files.get_mut(app.review.cursor_file)
                && file.hunks.get(app.review.cursor_hunk).is_some()
            {
                let file_index = app.review.cursor_file;
                let original_file = file.clone();
                let mut updated_file = file.clone();
                updated_file.hunks[app.review.cursor_hunk].review_status = ReviewStatus::Rejected;
                updated_file.sync_review_status();

                let tx = app.tx.clone();
                let git = app.git.clone();
                app.review_busy = true;
                app.status = "Rejecting hunk...".to_string();

                tokio::spawn(async move {
                    let result = git
                        .sync_file_hunks_to_index(&updated_file)
                        .await
                        .map_err(|err| format!("Could not reject hunk: {err}"));
                    let _ = tx.send(Message::HunkSync {
                        file_index,
                        original_file,
                        updated_file,
                        success_status: "Rejected hunk.".to_string(),
                        result,
                    });
                });
            }
        }
        KeyCode::Char('u') => {
            if let Some(file) = app.review.files.get_mut(app.review.cursor_file) {
                let result = app.git.unstage_file_in_place(file).await;

                match result {
                    Ok(()) => app.status = "Moved file back to unreviewed.".to_string(),
                    Err(err) => app.status = format!("Could not unstage file: {err}"),
                }
            }
        }
        KeyCode::Char('s') => open_settings(app),
        KeyCode::Char('e') => open_explain_menu(app),
        KeyCode::Char('h') => {
            app.why_this.return_to_menu = false;
            open_explain_history(app)
        }
        KeyCode::Char('r') => retry_current_explain(app).await?,
        KeyCode::Char('z') => cancel_current_explain(app),
        KeyCode::Char('m') => {
            app.why_this.return_to_menu = false;
            open_model_picker(app).await
        }
        _ => {}
    }

    Ok(())
}

async fn handle_explain_menu_key(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Esc => {
            app.overlay = Overlay::None;
            app.why_this.return_to_menu = false;
            app.status = "Closed the Explain menu.".to_string();
        }
        KeyCode::Enter => {
            if app.opencode.is_none() {
                app.status = "Explain is unavailable because opencode could not start.".to_string();
                return Ok(());
            }
            if app.active_session().is_none() {
                app.status =
                    "No context source is linked to this repository. Press c to choose one."
                        .to_string();
                return Ok(());
            }
            if current_why_target(&app.review).is_none() {
                app.status = "Nothing is selected to explain.".to_string();
                return Ok(());
            }

            app.overlay = Overlay::None;
            app.why_this.return_to_menu = false;
            request_explain(app).await?;
        }
        KeyCode::Char('c') => {
            app.why_this.return_to_menu = true;
            open_session_picker(app)
        }
        KeyCode::Char('m') => {
            app.why_this.return_to_menu = true;
            open_model_picker(app).await
        }
        KeyCode::Char('h') => {
            app.why_this.return_to_menu = true;
            open_explain_history(app)
        }
        KeyCode::Char('r') => retry_current_explain(app).await?,
        KeyCode::Char('z') => cancel_current_explain(app),
        _ => {}
    }

    Ok(())
}

fn handle_session_picker_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => {
            close_explain_submenu(app, "Session picker closed.");
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.session_state.cursor = app.session_state.cursor.saturating_sub(1);
        }
        KeyCode::Down | KeyCode::Char('j')
            if app.session_state.cursor + 1 < app.session_state.sessions.len() =>
        {
            app.session_state.cursor += 1;
        }
        KeyCode::Enter => {
            app.session_state.selected = Some(app.session_state.cursor);
            app.refresh_auto_model();
            close_explain_submenu(app, "Choose a file or hunk, then run Explain.");
            if let Some(session) = app.active_session() {
                app.status = format!("Explain will use context source {}.", session.title);
            }
        }
        _ => {}
    }
}

fn handle_explain_history_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => {
            close_explain_submenu(app, "Closed Explain history.");
        }
        KeyCode::Up | KeyCode::Char('k') => move_explain_history_cursor(app, -1),
        KeyCode::Down | KeyCode::Char('j') => move_explain_history_cursor(app, 1),
        KeyCode::Enter => focus_history_run(app),
        KeyCode::Char('r') => retry_history_run(app),
        KeyCode::Char('z') => cancel_history_run(app),
        KeyCode::Backspace | KeyCode::Delete => clear_history_run(app),
        _ => {}
    }
}

fn handle_model_picker_key(app: &mut App, key: KeyEvent) {
    let max_index = app.why_this.model.available.len();
    match key.code {
        KeyCode::Esc => {
            close_explain_submenu(app, "Closed the Explain model picker.");
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.why_this.model.cursor = app.why_this.model.cursor.saturating_sub(1);
        }
        KeyCode::Down | KeyCode::Char('j') if app.why_this.model.cursor < max_index => {
            app.why_this.model.cursor += 1;
        }
        KeyCode::Enter => {
            if app.why_this.model.cursor == 0 {
                app.why_this.model_override = Some(WhyModelChoice::Auto);
                app.status = format!("Explain model set to {}.", why_model_display_label(app));
            } else if let Some(model) = app
                .why_this
                .model
                .available
                .get(app.why_this.model.cursor - 1)
                .cloned()
            {
                app.why_this.model_override = Some(WhyModelChoice::Explicit(model.clone()));
                app.status = format!("Explain model set to {model}.");
            }
            if app.why_this.return_to_menu {
                app.overlay = Overlay::ExplainMenu;
            } else {
                app.overlay = Overlay::None;
            }
        }
        _ => {}
    }
}

fn handle_saved_model_picker_key(app: &mut App, key: KeyEvent) {
    let max_index = app.why_this.model.available.len();
    match key.code {
        KeyCode::Esc => {
            app.overlay = Overlay::Settings;
            app.status = "Back to settings.".to_string();
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.saved_model_cursor = app.saved_model_cursor.saturating_sub(1);
        }
        KeyCode::Down | KeyCode::Char('j') if app.saved_model_cursor < max_index => {
            app.saved_model_cursor += 1;
        }
        KeyCode::Enter => {
            app.settings.explain.default_model = if app.saved_model_cursor == 0 {
                None
            } else {
                app.why_this
                    .model
                    .available
                    .get(app.saved_model_cursor - 1)
                    .cloned()
            };
            save_settings(app);
            sync_model_picker_cursors(app);
            app.overlay = Overlay::Settings;
            app.status = format!(
                "Default Explain model set to {}.",
                saved_model_label(&app.settings.explain.default_model)
            );
        }
        _ => {}
    }
}

fn handle_settings_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => {
            app.overlay = Overlay::None;
            app.status = format!(
                "Closed settings. Config: {}",
                app.settings_store.path().display()
            );
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.settings_cursor = app.settings_cursor.saturating_sub(1);
        }
        KeyCode::Down | KeyCode::Char('j') if app.settings_cursor + 1 < settings_row_count() => {
            app.settings_cursor += 1;
        }
        KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
            open_saved_model_picker(app);
        }
        _ => {}
    }
}

async fn open_model_picker(app: &mut App) {
    let Some(opencode) = app.opencode.clone() else {
        app.status =
            "Explain model selection is unavailable because opencode is not ready.".to_string();
        return;
    };

    app.overlay = Overlay::ModelPicker;
    app.why_this.model.cursor =
        model_picker_cursor(&current_model_choice(app), &app.why_this.model.available);

    let is_cache_fresh = app
        .why_this
        .model
        .last_loaded_at
        .is_some_and(|loaded_at| loaded_at.elapsed() < MODEL_CACHE_TTL);
    if is_cache_fresh && !app.why_this.model.available.is_empty() {
        app.status = model_picker_status_message(app.overlay).to_string();
        return;
    }

    if app.why_this.model.loading {
        app.status = "Loading Explain models...".to_string();
        return;
    }

    app.why_this.model.loading = true;
    app.status = "Loading Explain models...".to_string();

    let tx = app.tx.clone();
    tokio::spawn(async move {
        let result = opencode.list_models().await.map_err(|err| err.to_string());
        let _ = tx.send(Message::ModelList { result });
    });
}

fn open_saved_model_picker(app: &mut App) {
    if app.opencode.is_none() {
        app.status =
            "Default Explain model selection is unavailable because opencode is not ready."
                .to_string();
        return;
    }

    app.overlay = Overlay::SettingsModelPicker;
    app.saved_model_cursor = saved_model_picker_cursor(
        app.settings.explain.default_model.as_deref(),
        &app.why_this.model.available,
    );

    let is_cache_fresh = app
        .why_this
        .model
        .last_loaded_at
        .is_some_and(|loaded_at| loaded_at.elapsed() < MODEL_CACHE_TTL);
    if is_cache_fresh && !app.why_this.model.available.is_empty() {
        app.status = model_picker_status_message(app.overlay).to_string();
        return;
    }

    if app.why_this.model.loading {
        app.status = "Loading Explain models...".to_string();
        return;
    }

    app.why_this.model.loading = true;
    app.status = "Loading Explain models...".to_string();

    let Some(opencode) = app.opencode.clone() else {
        return;
    };
    let tx = app.tx.clone();
    tokio::spawn(async move {
        let result = opencode.list_models().await.map_err(|err| err.to_string());
        let _ = tx.send(Message::ModelList { result });
    });
}

async fn request_explain(app: &mut App) -> Result<()> {
    let Some(_opencode) = app.opencode.clone() else {
        app.status = "Explain is unavailable because opencode could not start.".to_string();
        return Ok(());
    };
    let Some(session) = app.active_session().cloned() else {
        app.status =
            "No context source is linked to this repository. Press c to choose one.".to_string();
        return Ok(());
    };

    let Some((label, target)) = current_why_target(&app.review) else {
        app.status = "Nothing is selected to explain.".to_string();
        return Ok(());
    };

    let resolved_model = resolved_why_model(app);
    let session_id = session.id.clone();
    let session_label = format!("{} ({})", session.title, session.id);
    let model_label = why_model_display_label(app);
    request_explain_with_target(
        app,
        label,
        target,
        session_id,
        session_label,
        resolved_model,
        model_label,
    )
    .await
}

async fn request_explain_with_target(
    app: &mut App,
    label: String,
    target: WhyTarget,
    context_source_id: String,
    context_source_label: String,
    requested_model: Option<String>,
    model_label: String,
) -> Result<()> {
    let Some(opencode) = app.opencode.clone() else {
        app.status = "Explain is unavailable because opencode could not start.".to_string();
        return Ok(());
    };

    let cache_key = why_cache_key(&target, &context_source_id, requested_model.as_deref());
    if let Some(index) = find_reusable_explain_run_index(&app.why_this, &cache_key) {
        if let Some(run) = app.why_this.runs.get(index) {
            app.why_this.current_run_id = Some(run.id);
            app.why_this.history_cursor = index;
        }
        app.status = "Focused the existing explanation.".to_string();
        return Ok(());
    }

    if let Some(answer) = app.why_this.cache.get(&cache_key).cloned() {
        let run_id = next_explain_run_id(&mut app.why_this);
        app.why_this.runs.push(ExplainRun {
            id: run_id,
            label: label.clone(),
            target,
            context_source_id,
            context_source_label,
            requested_model,
            model_label,
            cache_key,
            status: ExplainRunStatus::Ready,
            result: Some(answer),
            error: None,
            handle: None,
        });
        app.why_this.current_run_id = Some(run_id);
        app.why_this.history_cursor = app.why_this.runs.len().saturating_sub(1);
        app.status = "Loaded the cached explanation.".to_string();
        return Ok(());
    }

    let run_id = next_explain_run_id(&mut app.why_this);
    let cache_key_for_message = cache_key.clone();
    let target_for_run = target.clone();
    let requested_model_for_task = requested_model.clone();
    let context_source_id_for_task = context_source_id.clone();
    let tx = app.tx.clone();

    app.status = format!("Running Explain for {label} with {model_label}.");

    let handle = tokio::spawn(async move {
        let result = opencode
            .ask_why(
                &context_source_id_for_task,
                &target,
                requested_model_for_task.as_deref(),
            )
            .await
            .map_err(|err| err.to_string());
        let _ = tx.send(Message::WhyThis {
            job_id: run_id,
            cache_key: cache_key_for_message,
            label,
            result,
        });
    });

    app.why_this.runs.push(ExplainRun {
        id: run_id,
        label: target_for_run.label(),
        target: target_for_run,
        context_source_id,
        context_source_label,
        requested_model,
        model_label,
        cache_key,
        status: ExplainRunStatus::Running,
        result: None,
        error: None,
        handle: Some(handle),
    });
    app.why_this.current_run_id = Some(run_id);
    app.why_this.history_cursor = app.why_this.runs.len().saturating_sub(1);

    Ok(())
}

fn draw(frame: &mut ratatui::Frame, app: &App, commit_message: &TextArea<'_>) {
    let size = frame.area();
    let header_height = if app.screen == Screen::Review { 1 } else { 2 };
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(header_height), Constraint::Min(10)])
        .split(size);

    draw_top_bar(frame, layout[0], app);
    match app.screen {
        Screen::Home => draw_home(frame, layout[1], app),
        Screen::Review => draw_review(frame, layout[1], app),
    }

    match app.overlay {
        Overlay::CommitPrompt => draw_commit_prompt(frame, layout[1], app, commit_message),
        Overlay::Settings => draw_settings(frame, layout[1], app),
        Overlay::SettingsModelPicker => draw_saved_model_picker(frame, layout[1], app),
        Overlay::ExplainMenu => draw_explain_menu(frame, layout[1], app),
        Overlay::SessionPicker => draw_session_picker(frame, layout[1], app),
        Overlay::ModelPicker => draw_model_picker(frame, layout[1], app),
        Overlay::ExplainHistory => draw_explain_history(frame, layout[1], app),
        Overlay::None => {}
    }
}

fn draw_top_bar(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    if app.screen == Screen::Home {
        render_brand_lockup(frame, area, app, Alignment::Center);
        if area.width > 0 {
            frame.render_widget(
                Block::default()
                    .borders(Borders::BOTTOM)
                    .border_style(Style::default().fg(styles::BORDER_MUTED)),
                area,
            );
        }
        return;
    }

    render_brand_lockup(frame, area, app, Alignment::Center);
}

fn draw_home(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    frame.render_widget(
        Block::default().style(Style::default().bg(styles::BASE_BG)),
        area,
    );

    let inner = area.inner(ratatui::layout::Margin {
        horizontal: 4,
        vertical: 2,
    });
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(styles::BORDER_MUTED))
            .style(Style::default().bg(styles::SURFACE)),
        inner,
    );

    let content = inner.inner(ratatui::layout::Margin {
        horizontal: 4,
        vertical: 2,
    });
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Min(1),
        ])
        .split(content);

    let counts = app.review_counts();
    frame.render_widget(
        Paragraph::new("Review agent changes before they become commits.")
            .alignment(Alignment::Center)
            .style(styles::accent_bold()),
        sections[1],
    );

    let summary = Line::from(vec![
        Span::styled("repo ", styles::subtle()),
        Span::styled(
            app.repo_path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("repo"),
            styles::title(),
        ),
        Span::raw("  |  "),
        Span::styled("mode ", styles::subtle()),
        Span::styled("review", styles::title()),
    ]);
    frame.render_widget(
        Paragraph::new(summary).alignment(Alignment::Center),
        sections[2],
    );

    let queue = Line::from(vec![
        Span::styled("queue ", styles::subtle()),
        Span::styled(
            format!(
                "{} files  {} unreviewed  {} accepted  {} rejected",
                app.review.files.len(),
                counts.unreviewed,
                counts.accepted,
                counts.rejected
            ),
            styles::muted(),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(queue).alignment(Alignment::Center),
        sections[3],
    );

    frame.render_widget(
        Paragraph::new(app.status.as_str())
            .alignment(Alignment::Center)
            .style(styles::muted())
            .wrap(Wrap { trim: true }),
        sections[4],
    );

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Enter", styles::keybind()),
            Span::styled(" review", styles::muted()),
            Span::raw("      "),
            Span::styled("c", styles::keybind()),
            Span::styled(" commit", styles::muted()),
            Span::raw("      "),
            Span::styled("s", styles::keybind()),
            Span::styled(" settings", styles::muted()),
            Span::raw("      "),
            Span::styled("Ctrl+C", styles::keybind()),
            Span::styled(" quit", styles::muted()),
        ]))
        .alignment(Alignment::Center)
        .style(styles::soft_accent()),
        sections[5],
    );
}

fn draw_review(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    frame.render_widget(
        Block::default().style(Style::default().bg(styles::BASE_BG)),
        area,
    );

    if app.review.files.is_empty() {
        let empty = Paragraph::new(vec![
            Line::from(Span::raw("")),
            Line::from(Span::raw("")),
            Line::from(Span::styled("No code changes yet", styles::title())),
            Line::from(Span::raw("")),
            Line::from(Span::styled(
                "Run your coding agent in another pane or window, then come back here to review.",
                styles::muted(),
            )),
            Line::from(Span::styled(
                "Relaunch better-review after your agent finishes to load new changes.",
                styles::muted(),
            )),
        ])
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(styles::BORDER_MUTED)),
        )
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true });
        frame.render_widget(empty, centered_rect(78, 38, area));
        return;
    }

    let canvas = area.inner(ratatui::layout::Margin {
        horizontal: 1,
        vertical: 0,
    });
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(styles::BORDER_MUTED))
            .style(Style::default().bg(styles::SURFACE)),
        canvas,
    );
    let content = canvas.inner(ratatui::layout::Margin {
        horizontal: 1,
        vertical: 0,
    });

    let sections = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(28),
            Constraint::Min(30),
            Constraint::Length(34),
        ])
        .split(content);

    let counts = app.review_counts();

    let items = app
        .review
        .files
        .iter()
        .enumerate()
        .map(|(index, file)| {
            let style = if index == app.review.cursor_file {
                Style::default()
                    .fg(styles::TEXT_PRIMARY)
                    .bg(styles::ACCENT_DIM)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(styles::TEXT_MUTED)
            };
            let marker = review_marker(file.review_status.clone(), file.status, false);
            ListItem::new(Line::from(vec![
                Span::styled(format!(" {marker} "), style),
                Span::styled(truncate_path(file.display_path(), 28), style),
            ]))
        })
        .collect::<Vec<_>>();

    let sidebar = List::new(items).block(
        Block::default()
            .title(format!(
                "Files  {} unreviewed  {} accepted  {} rejected",
                counts.unreviewed, counts.accepted, counts.rejected
            ))
            .borders(Borders::RIGHT)
            .border_style(
                Style::default().fg(if app.review.focus == ReviewFocus::Files {
                    styles::ACCENT_BRIGHT
                } else {
                    styles::BORDER_MUTED
                }),
            ),
    );
    let mut sidebar_state = ListState::default().with_selected(Some(app.review.cursor_file));
    frame.render_stateful_widget(sidebar, sections[0], &mut sidebar_state);

    let mut diff_lines = vec![Line::from(vec![
        Span::styled(
            app.review.files[app.review.cursor_file].display_path(),
            styles::title(),
        ),
        Span::raw("  "),
        Span::styled(
            match app.review.focus {
                ReviewFocus::Files => "reviewing files",
                ReviewFocus::Hunks => "inspecting hunks",
            },
            styles::soft_accent(),
        ),
    ])];
    if let Some(file) = app.review.files.get(app.review.cursor_file) {
        for (index, hunk) in file.hunks.iter().enumerate() {
            let is_current_hunk =
                app.review.focus == ReviewFocus::Hunks && app.review.cursor_hunk == index;
            let is_current_line = app.review.focus == ReviewFocus::Hunks
                && app.review.cursor_line == diff_lines.len();
            let mut style = Style::default()
                .fg(styles::TEXT_PRIMARY)
                .bg(styles::SURFACE_RAISED);
            if is_current_hunk {
                style = Style::default()
                    .fg(styles::TEXT_PRIMARY)
                    .bg(styles::ACCENT_DIM)
                    .add_modifier(Modifier::BOLD);
            }
            if is_current_line {
                style = style.add_modifier(Modifier::UNDERLINED);
            }

            let status = match hunk.review_status {
                ReviewStatus::Accepted => {
                    Span::styled(" [accepted]", Style::default().fg(styles::SUCCESS))
                }
                ReviewStatus::Rejected => {
                    Span::styled(" [rejected]", Style::default().fg(styles::DANGER))
                }
                ReviewStatus::Unreviewed => Span::styled(" [unreviewed]", styles::muted()),
            };

            diff_lines.push(Line::from(vec![
                Span::styled(
                    format!(
                        "{} {}",
                        review_marker(hunk.review_status.clone(), file.status, true),
                        hunk.header,
                    ),
                    style,
                ),
                status,
            ]));
            for line in &hunk.lines {
                let is_current_line = app.review.focus == ReviewFocus::Hunks
                    && app.review.cursor_line == diff_lines.len();
                let prefix = match line.kind {
                    DiffLineKind::Add => "+",
                    DiffLineKind::Remove => "-",
                    DiffLineKind::Context => " ",
                };
                let style = match line.kind {
                    DiffLineKind::Add => Style::default().fg(styles::SUCCESS),
                    DiffLineKind::Remove => Style::default().fg(styles::DANGER),
                    DiffLineKind::Context => Style::default().fg(styles::TEXT_MUTED),
                };
                let style = if is_current_line {
                    style
                        .bg(styles::SURFACE_RAISED)
                        .add_modifier(Modifier::UNDERLINED)
                } else {
                    style
                };
                let old = line
                    .old_line
                    .map(|n| format!("{n:>4}"))
                    .unwrap_or_else(|| "    ".to_string());
                let new = line
                    .new_line
                    .map(|n| format!("{n:>4}"))
                    .unwrap_or_else(|| "    ".to_string());
                diff_lines.push(Line::from(vec![
                    Span::styled(format!("{old} {new} "), styles::subtle()),
                    Span::styled(prefix, style),
                    Span::styled(line.content.clone(), style),
                ]));
            }
            diff_lines.push(Line::from(Span::raw("")));
        }
    }

    let diff_scroll = diff_scroll_offset(app, sections[1], &diff_lines);
    let diff = Paragraph::new(diff_lines).scroll((diff_scroll, 0));
    frame.render_widget(diff, sections[1]);

    let why_block = Block::default()
        .title(Line::from(Span::styled("Explain", styles::title())))
        .borders(Borders::LEFT)
        .border_style(Style::default().fg(styles::BORDER_MUTED))
        .style(Style::default().bg(styles::SURFACE_RAISED));
    let why_lines = explain_panel_lines(app);
    frame.render_widget(
        Paragraph::new(why_lines)
            .block(why_block)
            .style(Style::default().bg(styles::SURFACE_RAISED))
            .wrap(Wrap { trim: true }),
        sections[2],
    );
}

fn draw_session_picker(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let modal = centered_rect(58, 42, area);
    frame.render_widget(Clear, modal);
    frame.render_widget(
        Block::default().style(Style::default().bg(styles::SURFACE_RAISED)),
        modal,
    );
    let inner = modal.inner(ratatui::layout::Margin {
        horizontal: 1,
        vertical: 1,
    });
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(2)])
        .split(inner);
    let items = app
        .session_state
        .sessions
        .iter()
        .enumerate()
        .map(|(index, session)| {
            let style = if index == app.session_state.cursor {
                Style::default()
                    .fg(styles::TEXT_PRIMARY)
                    .bg(styles::ACCENT_DIM)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(styles::TEXT_MUTED)
            };
            let marker = if app.session_state.selected == Some(index) {
                "[✓]"
            } else {
                "[ ]"
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!(" {marker} "), style),
                Span::styled(session.title.clone(), style),
            ]))
        })
        .collect::<Vec<_>>();
    let mut state = ListState::default().with_selected(Some(app.session_state.cursor));
    frame.render_stateful_widget(
        List::new(items).block(
            Block::default()
                .title(Line::from(Span::styled(
                    "Choose context source",
                    styles::title(),
                )))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(styles::ACCENT_BRIGHT))
                .style(Style::default().bg(styles::SURFACE_RAISED)),
        ),
        sections[0],
        &mut state,
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Enter", styles::keybind()),
            Span::styled(" select", styles::muted()),
            Span::raw("  "),
            Span::styled("Esc", styles::keybind()),
            Span::styled(" close", styles::muted()),
        ]))
        .style(Style::default().bg(styles::SURFACE_RAISED)),
        sections[1],
    );
}

fn draw_model_picker(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    draw_model_picker_modal(
        frame,
        area,
        app,
        Overlay::ModelPicker,
        app.why_this.model.cursor,
        current_model_choice(app),
    );
}

fn draw_saved_model_picker(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    draw_model_picker_modal(
        frame,
        area,
        app,
        Overlay::SettingsModelPicker,
        app.saved_model_cursor,
        saved_model_choice(app),
    );
}

fn draw_model_picker_modal(
    frame: &mut ratatui::Frame,
    area: Rect,
    app: &App,
    overlay: Overlay,
    cursor: usize,
    selected_choice: WhyModelChoice,
) {
    let modal = centered_rect(62, 48, area);
    frame.render_widget(Clear, modal);
    frame.render_widget(
        Block::default().style(Style::default().bg(styles::SURFACE_RAISED)),
        modal,
    );
    let inner = modal.inner(ratatui::layout::Margin {
        horizontal: 1,
        vertical: 1,
    });
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(2)])
        .split(inner);

    let mut rows = Vec::with_capacity(app.why_this.model.available.len() + 1);
    let title = match overlay {
        Overlay::ModelPicker => "Choose Explain model",
        Overlay::SettingsModelPicker => "Default Explain model",
        _ => unreachable!(),
    };
    let auto_label = match overlay {
        Overlay::ModelPicker => format!(" Auto ({})", auto_model_label(app)),
        Overlay::SettingsModelPicker => format!(
            " Auto ({})",
            app.why_this
                .model
                .auto_session_model
                .clone()
                .unwrap_or_else(|| "session default".to_string())
        ),
        _ => unreachable!(),
    };
    rows.push(model_picker_item(
        0,
        &auto_label,
        cursor,
        selected_choice == WhyModelChoice::Auto,
    ));

    for (index, model) in app.why_this.model.available.iter().enumerate() {
        rows.push(model_picker_item(
            index + 1,
            model,
            cursor,
            matches!(&selected_choice, WhyModelChoice::Explicit(selected) if selected == model),
        ));
    }

    if app.why_this.model.loading && app.why_this.model.available.is_empty() {
        rows.push(ListItem::new(Line::from(Span::styled(
            " Loading models...",
            styles::muted(),
        ))));
    }

    if let Some(error) = &app.why_this.model.last_error {
        rows.push(ListItem::new(Line::from(Span::styled(
            format!(" Error: {error}"),
            Style::default().fg(styles::DANGER),
        ))));
        rows.push(ListItem::new(Line::from(Span::styled(
            " Close and reopen this picker to retry.",
            styles::muted(),
        ))));
    }

    let mut state = ListState::default().with_selected(Some(cursor));
    frame.render_stateful_widget(
        List::new(rows).block(
            Block::default()
                .title(Line::from(Span::styled(title, styles::title())))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(styles::ACCENT_BRIGHT))
                .style(Style::default().bg(styles::SURFACE_RAISED)),
        ),
        sections[0],
        &mut state,
    );

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Enter", styles::keybind()),
            Span::styled(" select", styles::muted()),
            Span::raw("  "),
            Span::styled("Esc", styles::keybind()),
            Span::styled(" close", styles::muted()),
        ]))
        .style(Style::default().bg(styles::SURFACE_RAISED)),
        sections[1],
    );
}

fn draw_explain_menu(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let modal = centered_rect(64, 46, area);
    frame.render_widget(Clear, modal);
    frame.render_widget(
        Paragraph::new(explain_menu_lines(app))
            .block(
                Block::default()
                    .title(Line::from(Span::styled("Explain", styles::title())))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(styles::ACCENT_BRIGHT))
                    .style(Style::default().bg(styles::SURFACE_RAISED)),
            )
            .style(Style::default().bg(styles::SURFACE_RAISED))
            .wrap(Wrap { trim: true }),
        modal,
    );
}

fn model_picker_item(
    index: usize,
    label: &str,
    cursor: usize,
    selected_value: bool,
) -> ListItem<'static> {
    let style = if index == cursor {
        Style::default()
            .fg(styles::TEXT_PRIMARY)
            .bg(styles::ACCENT_DIM)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(styles::TEXT_MUTED)
    };
    let marker = if selected_value { "[✓]" } else { "[ ]" };

    ListItem::new(Line::from(vec![
        Span::styled(format!(" {marker} "), style),
        Span::styled(label.to_string(), style),
    ]))
}

fn draw_explain_history(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let modal = centered_rect(70, 56, area);
    frame.render_widget(Clear, modal);
    frame.render_widget(
        Paragraph::new(explain_history_lines(app))
            .block(
                Block::default()
                    .title(Line::from(Span::styled("Explain History", styles::title())))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(styles::ACCENT_BRIGHT))
                    .style(Style::default().bg(styles::SURFACE_RAISED)),
            )
            .style(Style::default().bg(styles::SURFACE_RAISED))
            .wrap(Wrap { trim: true }),
        modal,
    );
}

fn explain_panel_lines(app: &App) -> Vec<Line<'static>> {
    let mut lines = explain_context_lines(app);

    let Some(run) = current_explain_run(app) else {
        lines.extend(explain_empty_lines());
        return lines;
    };

    lines.push(Line::from(Span::raw("")));
    lines.extend(render_explain_run_lines(run, &app.logo_animation));
    lines.push(Line::from(Span::raw("")));
    lines.extend(explain_footer_lines(app));
    lines
}

fn explain_context_lines(app: &App) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(Span::styled(
        explain_context_source_line(app),
        styles::soft_accent(),
    ))];
    lines.push(Line::from(Span::styled(
        format!("model: {}", why_model_display_label(app)),
        styles::muted(),
    )));
    if let Some(scope_preview) = explain_scope_preview(app) {
        lines.push(Line::from(Span::styled(
            format!("scope: {scope_preview}"),
            styles::muted(),
        )));
    }
    lines
}

fn explain_menu_lines(app: &App) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(Span::styled(
            "Review focus decides the scope.",
            styles::soft_accent(),
        )),
        Line::from(Span::raw("")),
        explain_menu_detail_line(
            "Scope",
            explain_scope_preview(app).unwrap_or_else(|| "nothing selected".to_string()),
        ),
        explain_menu_detail_line("Context", explain_context_source_label(app)),
        explain_menu_detail_line("Model", why_model_display_label(app)),
        explain_menu_detail_line(
            "History",
            format!("{} run(s) this session", app.why_this.runs.len()),
        ),
        Line::from(Span::raw("")),
        Line::from(vec![
            Span::styled("Enter", styles::keybind()),
            Span::styled(" run explain", styles::muted()),
        ]),
        Line::from(vec![
            Span::styled("c", styles::keybind()),
            Span::styled(" choose context", styles::muted()),
        ]),
        Line::from(vec![
            Span::styled("m", styles::keybind()),
            Span::styled(" choose model", styles::muted()),
        ]),
        Line::from(vec![
            Span::styled("h", styles::keybind()),
            Span::styled(" open history", styles::muted()),
        ]),
        Line::from(vec![
            Span::styled("r", styles::keybind()),
            Span::styled(" retry current run", styles::muted()),
        ]),
        Line::from(vec![
            Span::styled("z", styles::keybind()),
            Span::styled(" cancel current run", styles::muted()),
        ]),
        Line::from(vec![
            Span::styled("Esc", styles::keybind()),
            Span::styled(" close", styles::muted()),
        ]),
    ];

    if app.active_session().is_none() {
        lines.push(Line::from(Span::raw("")));
        lines.push(Line::from(Span::styled(
            "Choose a context source before you run Explain.",
            Style::default()
                .fg(styles::DANGER)
                .add_modifier(Modifier::BOLD),
        )));
    }

    lines
}

fn explain_menu_detail_line(label: &str, value: String) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label:<7}"), styles::title()),
        Span::styled(value, styles::muted()),
    ])
}

fn explain_empty_lines() -> Vec<Line<'static>> {
    vec![
        Line::from(Span::raw("")),
        Line::from(Span::styled("Explain the current change", styles::title())),
        Line::from(vec![
            Span::styled(" e ", styles::keybind()),
            Span::styled("open the Explain menu", styles::muted()),
        ]),
        Line::from(vec![
            Span::styled(" m ", styles::keybind()),
            Span::styled("choose model", styles::muted()),
        ]),
        Line::from(vec![
            Span::styled(" c ", styles::keybind()),
            Span::styled("choose context source", styles::muted()),
        ]),
        Line::from(vec![
            Span::styled(" h ", styles::keybind()),
            Span::styled("open explain history", styles::muted()),
        ]),
        Line::from(vec![
            Span::styled(" z ", styles::keybind()),
            Span::styled("cancel current run", styles::muted()),
        ]),
        Line::from(vec![
            Span::styled(" r ", styles::keybind()),
            Span::styled("retry current run", styles::muted()),
        ]),
        Line::from(Span::raw("")),
        Line::from(Span::styled(
            "Tip: file focus explains the file; hunk focus explains the current hunk.",
            styles::subtle(),
        )),
    ]
}

fn explain_footer_lines(app: &App) -> Vec<Line<'static>> {
    vec![Line::from(vec![
        Span::styled("e", styles::keybind()),
        Span::styled(" menu", styles::muted()),
        Span::raw("  "),
        Span::styled("s", styles::keybind()),
        Span::styled(" settings", styles::muted()),
        Span::raw("  "),
        Span::styled("h", styles::keybind()),
        Span::styled(
            format!(" history ({})", app.why_this.runs.len()),
            styles::muted(),
        ),
        Span::raw("  "),
        Span::styled("r", styles::keybind()),
        Span::styled(" retry", styles::muted()),
        Span::raw("  "),
        Span::styled("z", styles::keybind()),
        Span::styled(" cancel", styles::muted()),
    ])]
}

fn explain_history_lines(app: &App) -> Vec<Line<'static>> {
    let mut lines = explain_context_lines(app);
    lines.push(Line::from(Span::raw("")));

    if app.why_this.runs.is_empty() {
        lines.push(Line::from(Span::styled(
            "No explain runs yet.",
            styles::title(),
        )));
        return lines;
    }

    lines.push(Line::from(Span::styled(
        format!("{} run(s) this session", app.why_this.runs.len()),
        styles::title(),
    )));
    lines.extend(render_explain_history_list_lines(app));
    lines.push(Line::from(Span::raw("")));
    if let Some(run) = selected_history_run(app) {
        lines.extend(render_explain_run_lines(run, &app.logo_animation));
    }
    lines.push(Line::from(Span::raw("")));
    lines.push(Line::from(vec![
        Span::styled("j/k", styles::keybind()),
        Span::styled(" move", styles::muted()),
        Span::raw("  "),
        Span::styled("Enter", styles::keybind()),
        Span::styled(" focus", styles::muted()),
        Span::raw("  "),
        Span::styled("r", styles::keybind()),
        Span::styled(" retry", styles::muted()),
        Span::raw("  "),
        Span::styled("z", styles::keybind()),
        Span::styled(" cancel", styles::muted()),
        Span::raw("  "),
        Span::styled("Del", styles::keybind()),
        Span::styled(" clear", styles::muted()),
    ]));
    lines
}

fn render_explain_history_list_lines(app: &App) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    for (index, run) in app.why_this.runs.iter().enumerate() {
        let selected = app.why_this.history_cursor == index;
        let marker = if selected { ">" } else { " " };
        let style = if selected {
            Style::default()
                .fg(styles::TEXT_PRIMARY)
                .bg(styles::ACCENT_DIM)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(styles::TEXT_MUTED)
        };

        lines.push(Line::from(vec![
            Span::styled(format!("{marker} #{} ", run.id), style),
            Span::styled(
                explain_run_status_label(&run.status),
                explain_run_status_style(&run.status),
            ),
            Span::styled(format!(" {}", run.label), style),
        ]));
    }

    lines
}

fn render_explain_run_lines(run: &ExplainRun, animation: &AnimatedTextState) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(Span::styled(run.label.clone(), styles::title())),
        Line::from(Span::styled(
            format!("status: {}", explain_run_status_label(&run.status)),
            explain_run_status_style(&run.status),
        )),
        Line::from(Span::styled(
            format!("context: {}", run.context_source_label),
            styles::muted(),
        )),
        Line::from(Span::styled(
            format!("model: {}", run.model_label),
            styles::muted(),
        )),
    ];

    match &run.status {
        ExplainRunStatus::Running => {
            lines.push(Line::from(Span::raw("")));
            lines.push(Line::from(Span::styled(
                loading_thinking_label(animation),
                styles::accent_bold(),
            )));
            lines.push(Line::from(Span::styled(
                "Using a fork of the selected context source so the live coding thread stays clean.",
                styles::muted(),
            )));
        }
        ExplainRunStatus::Ready => {
            let Some(answer) = &run.result else {
                return lines;
            };
            lines.push(Line::from(Span::raw("")));
            lines.push(Line::from(Span::styled(
                format!("forked session: {}", answer.fork_session_id),
                styles::subtle(),
            )));
            lines.push(Line::from(Span::raw("")));
            lines.extend(render_why_answer_lines(answer));
        }
        ExplainRunStatus::Failed => {
            lines.push(Line::from(Span::raw("")));
            lines.push(Line::from(Span::styled(
                "Explain could not produce a valid answer.",
                Style::default()
                    .fg(styles::DANGER)
                    .add_modifier(Modifier::BOLD),
            )));
            if let Some(error) = &run.error {
                lines.push(Line::from(Span::raw(error.clone())));
            }
            lines.push(Line::from(Span::styled(
                "Press r to retry, or press m to switch models.",
                styles::muted(),
            )));
        }
        ExplainRunStatus::Cancelled => {
            lines.push(Line::from(Span::raw("")));
            lines.push(Line::from(Span::styled(
                "This explain run was cancelled before completion.",
                styles::muted(),
            )));
        }
    }

    lines
}

fn next_explain_run_id(why_this: &mut WhyThisUiState) -> u64 {
    why_this.next_run_id = why_this.next_run_id.saturating_add(1);
    why_this.next_run_id
}

fn find_explain_run_index_by_id(why_this: &WhyThisUiState, run_id: u64) -> Option<usize> {
    why_this.runs.iter().position(|run| run.id == run_id)
}

fn find_reusable_explain_run_index(why_this: &WhyThisUiState, cache_key: &str) -> Option<usize> {
    why_this.runs.iter().position(|run| {
        run.cache_key == cache_key
            && matches!(
                run.status,
                ExplainRunStatus::Running | ExplainRunStatus::Ready
            )
    })
}

fn current_explain_run(app: &App) -> Option<&ExplainRun> {
    let run_id = app.why_this.current_run_id?;
    app.why_this.runs.iter().find(|run| run.id == run_id)
}

fn selected_history_run(app: &App) -> Option<&ExplainRun> {
    app.why_this.runs.get(app.why_this.history_cursor)
}

fn move_explain_history_cursor(app: &mut App, delta: isize) {
    if app.why_this.runs.is_empty() {
        app.status = "No explain runs yet.".to_string();
        return;
    }

    let len = app.why_this.runs.len() as isize;
    let current = app.why_this.history_cursor as isize;
    let next = (current + delta).rem_euclid(len) as usize;
    app.why_this.history_cursor = next;
    if let Some(run) = app.why_this.runs.get(next) {
        app.status = format!("Selected explain run #{}.", run.id);
    }
}

fn focus_history_run(app: &mut App) {
    let Some(run_id) = selected_history_run(app).map(|run| run.id) else {
        app.status = "No explain run selected.".to_string();
        return;
    };

    app.why_this.current_run_id = Some(run_id);
    app.overlay = Overlay::None;
    app.why_this.return_to_menu = false;
    app.status = format!("Focused explain run #{}.", run_id);
}

fn cancel_run_by_index(app: &mut App, index: usize) {
    let Some(run) = app.why_this.runs.get_mut(index) else {
        app.status = "Selected explain run no longer exists.".to_string();
        return;
    };

    if !matches!(run.status, ExplainRunStatus::Running) {
        app.status = format!("Explain run #{} is not running.", run.id);
        return;
    };

    if let Some(handle) = run.handle.take() {
        handle.abort();
    }
    run.status = ExplainRunStatus::Cancelled;
    run.error = None;
    app.status = format!("Cancelled explain run #{}.", run.id);
}

fn cancel_current_explain(app: &mut App) {
    let Some(run_id) = app.why_this.current_run_id else {
        app.status = "No current explain run.".to_string();
        return;
    };

    if let Some(index) = find_explain_run_index_by_id(&app.why_this, run_id) {
        cancel_run_by_index(app, index);
    }
}

fn cancel_history_run(app: &mut App) {
    cancel_run_by_index(app, app.why_this.history_cursor);
}

fn clear_run_by_index(app: &mut App, index: usize) {
    let Some(run) = app.why_this.runs.get(index) else {
        app.status = "Selected explain run no longer exists.".to_string();
        return;
    };

    if matches!(run.status, ExplainRunStatus::Running) {
        app.status = format!(
            "Explain run #{} is still running. Press z to cancel it.",
            run.id
        );
        return;
    }

    let removed = app.why_this.runs.remove(index);
    if app.why_this.current_run_id == Some(removed.id) {
        app.why_this.current_run_id = app.why_this.runs.last().map(|run| run.id);
    }
    if app.why_this.runs.is_empty() {
        app.why_this.history_cursor = 0;
        if app.overlay == Overlay::ExplainHistory {
            app.overlay = Overlay::None;
        }
    } else {
        app.why_this.history_cursor = index.min(app.why_this.runs.len().saturating_sub(1));
    }
    app.status = format!("Cleared explain run #{}.", removed.id);
}

fn clear_history_run(app: &mut App) {
    clear_run_by_index(app, app.why_this.history_cursor);
}

fn open_explain_menu(app: &mut App) {
    app.overlay = Overlay::ExplainMenu;
    app.why_this.return_to_menu = true;
    app.status = "Choose a file or hunk, then run Explain.".to_string();
}

fn open_session_picker(app: &mut App) {
    if app.session_state.sessions.is_empty() {
        app.status = "No opencode sessions were found for this repository.".to_string();
        return;
    }

    if let Some(selected) = app.session_state.selected {
        app.session_state.cursor = selected;
    }
    app.overlay = Overlay::SessionPicker;
    app.status = "Choose the context source for Explain.".to_string();
}

fn close_explain_submenu(app: &mut App, status: &str) {
    app.overlay = if app.why_this.return_to_menu {
        Overlay::ExplainMenu
    } else {
        Overlay::None
    };
    app.status = status.to_string();
}

fn open_explain_history(app: &mut App) {
    app.overlay = Overlay::ExplainHistory;
    app.status = "Opened Explain history.".to_string();
}

fn open_settings(app: &mut App) {
    app.overlay = Overlay::Settings;
    app.settings_cursor = 0;
    app.status = format!(
        "Settings loaded from {}.",
        app.settings_store.path().display()
    );
}

fn save_settings(app: &mut App) {
    match app.settings_store.save(&app.settings) {
        Ok(()) => {
            app.apply_saved_settings();
        }
        Err(error) => {
            app.status = format!("Could not save settings: {error}");
        }
    }
}

fn settings_row_count() -> usize {
    1
}

fn saved_model_label(model: &Option<String>) -> String {
    model.clone().unwrap_or_else(|| "Auto".to_string())
}

fn settings_lines(app: &App) -> Vec<Line<'static>> {
    let rows = [(
        "Default Explain model",
        saved_model_label(&app.settings.explain.default_model),
        "Press Enter to choose the saved default model.",
    )];

    let mut lines = vec![Line::from(Span::styled(
        format!("Config: {}", app.settings_store.path().display()),
        styles::soft_accent(),
    ))];
    lines.push(Line::from(Span::raw("")));

    for (index, (label, value, hint)) in rows.into_iter().enumerate() {
        let selected = index == app.settings_cursor;
        let row_style = if selected {
            Style::default()
                .fg(styles::TEXT_PRIMARY)
                .bg(styles::ACCENT_DIM)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(styles::TEXT_MUTED)
        };
        let marker = if selected { ">" } else { " " };
        lines.push(Line::from(vec![
            Span::styled(format!("{marker} {label:<15}"), row_style),
            Span::styled(value, row_style),
        ]));
        if selected {
            lines.push(Line::from(Span::styled(
                format!("  {hint}"),
                styles::muted(),
            )));
        }
    }

    lines
}

fn saved_model_choice(app: &App) -> WhyModelChoice {
    match &app.settings.explain.default_model {
        Some(model) => WhyModelChoice::Explicit(model.clone()),
        None => WhyModelChoice::Auto,
    }
}

fn explicit_model_choice(choice: &WhyModelChoice) -> Option<&str> {
    match choice {
        WhyModelChoice::Auto => None,
        WhyModelChoice::Explicit(model) => Some(model.as_str()),
    }
}

fn saved_model_picker_cursor(saved_model: Option<&str>, models: &[String]) -> usize {
    match saved_model {
        None => 0,
        Some(model) => models
            .iter()
            .position(|candidate| candidate == model)
            .map_or(0, |index| index + 1),
    }
}

fn ensure_model_present(models: &mut Vec<String>, model: Option<&str>) {
    let Some(model) = model else {
        return;
    };
    if !models.iter().any(|candidate| candidate == model) {
        models.insert(0, model.to_string());
    }
}

fn sync_model_picker_cursors(app: &mut App) {
    app.why_this.model.cursor =
        model_picker_cursor(&current_model_choice(app), &app.why_this.model.available);
    app.saved_model_cursor = saved_model_picker_cursor(
        app.settings.explain.default_model.as_deref(),
        &app.why_this.model.available,
    );
}

fn model_picker_status_message(overlay: Overlay) -> &'static str {
    match overlay {
        Overlay::ModelPicker => "Choose the Explain model, or keep Auto.",
        Overlay::SettingsModelPicker => "Choose the default Explain model, or keep Auto.",
        _ => "Choose a model.",
    }
}

async fn retry_current_explain(app: &mut App) -> Result<()> {
    let Some(run_id) = app.why_this.current_run_id else {
        app.status = "No current explain run.".to_string();
        return Ok(());
    };
    retry_run_by_id(app, run_id).await
}

fn retry_history_run(app: &mut App) {
    if let Some(run_id) = selected_history_run(app).map(|run| run.id) {
        app.why_this.current_run_id = Some(run_id);
        app.status = format!("Focused explain run #{} for retry.", run_id);
    }
}

async fn retry_run_by_id(app: &mut App, run_id: u64) -> Result<()> {
    let Some(index) = find_explain_run_index_by_id(&app.why_this, run_id) else {
        app.status = "Explain run no longer exists.".to_string();
        return Ok(());
    };
    let Some(run) = app.why_this.runs.get(index) else {
        app.status = "Explain run no longer exists.".to_string();
        return Ok(());
    };
    if matches!(run.status, ExplainRunStatus::Running) {
        app.status = format!("Explain run #{} is already running.", run.id);
        return Ok(());
    }

    request_explain_with_target(
        app,
        run.label.clone(),
        run.target.clone(),
        run.context_source_id.clone(),
        run.context_source_label.clone(),
        run.requested_model.clone(),
        run.model_label.clone(),
    )
    .await
}

fn explain_run_status_label(status: &ExplainRunStatus) -> &'static str {
    match status {
        ExplainRunStatus::Running => "running",
        ExplainRunStatus::Ready => "ready",
        ExplainRunStatus::Failed => "failed",
        ExplainRunStatus::Cancelled => "cancelled",
    }
}

fn explain_run_status_style(status: &ExplainRunStatus) -> Style {
    match status {
        ExplainRunStatus::Running => styles::accent_bold(),
        ExplainRunStatus::Ready => Style::default().fg(styles::SUCCESS),
        ExplainRunStatus::Failed => Style::default().fg(styles::DANGER),
        ExplainRunStatus::Cancelled => styles::muted(),
    }
}

fn explain_context_source_label(app: &App) -> String {
    app.active_session()
        .map(|session| format!("{} ({})", session.title, session.id))
        .unwrap_or_else(|| "none selected".to_string())
}

fn explain_scope_preview(app: &App) -> Option<String> {
    let file = app.review.files.get(app.review.cursor_file)?;
    if app.review.focus == ReviewFocus::Files || file.hunks.is_empty() {
        return Some(format!("file {}", file.display_path()));
    }

    let hunk = file.hunks.get(app.review.cursor_hunk)?;
    Some(format!("hunk {} {}", file.display_path(), hunk.header))
}

fn diff_scroll_offset(app: &App, area: Rect, diff_lines: &[Line<'_>]) -> u16 {
    if app.review.focus != ReviewFocus::Hunks {
        return 0;
    }

    let visible_height = usize::from(area.height.max(1));
    if visible_height == 0 {
        return 0;
    }

    let total_lines = diff_lines.len();
    let max_scroll = total_lines.saturating_sub(visible_height);
    let preferred_top = app
        .review
        .cursor_line
        .saturating_sub(visible_height.saturating_sub(3));
    preferred_top.min(max_scroll).min(u16::MAX as usize) as u16
}

fn draw_commit_prompt(
    frame: &mut ratatui::Frame,
    area: Rect,
    app: &App,
    commit_message: &TextArea<'_>,
) {
    let modal = centered_rect(60, 35, area);
    frame.render_widget(Clear, modal);
    let inner = modal.inner(ratatui::layout::Margin {
        horizontal: 1,
        vertical: 1,
    });
    let lines = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(5),
            Constraint::Length(1),
        ])
        .split(inner);

    let counts = app.review_counts();
    let block = Block::default()
        .title("Commit Accepted Changes")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(styles::BORDER_MUTED))
        .style(Style::default().bg(styles::SURFACE_RAISED));
    frame.render_widget(block, modal);
    frame.render_widget(
        Paragraph::new(format!(
            "Accepted {}  |  Rejected {}  |  Unreviewed {}",
            counts.accepted, counts.rejected, counts.unreviewed
        ))
        .style(styles::title()),
        lines[0],
    );
    frame.render_widget(
        Paragraph::new(vec![Line::from(vec![
            Span::raw("Commit prompt active  |  "),
            Span::styled("Enter", styles::keybind()),
            Span::raw(" commit  |  "),
            Span::styled("Esc", styles::keybind()),
            Span::raw(" close"),
        ])])
        .style(styles::muted()),
        lines[1],
    );
    frame.render_widget(commit_message, lines[2]);
    frame.render_widget(
        Paragraph::new("Only accepted staged changes are committed.").style(styles::muted()),
        lines[3],
    );
}

fn draw_settings(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let modal = centered_rect(68, 52, area);
    frame.render_widget(Clear, modal);
    frame.render_widget(
        Block::default().style(Style::default().bg(styles::SURFACE_RAISED)),
        modal,
    );
    let inner = modal.inner(ratatui::layout::Margin {
        horizontal: 1,
        vertical: 1,
    });
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(4), Constraint::Length(2)])
        .split(inner);

    let rows = settings_lines(app);
    frame.render_widget(
        Paragraph::new(rows)
            .block(
                Block::default()
                    .title(Line::from(Span::styled("Settings", styles::title())))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(styles::ACCENT_BRIGHT))
                    .style(Style::default().bg(styles::SURFACE_RAISED)),
            )
            .style(Style::default().bg(styles::SURFACE_RAISED))
            .wrap(Wrap { trim: true }),
        sections[0],
    );

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("j/k", styles::keybind()),
            Span::styled(" move", styles::muted()),
            Span::raw("  "),
            Span::styled("Enter", styles::keybind()),
            Span::styled(" open", styles::muted()),
            Span::raw("  "),
            Span::styled("Esc", styles::keybind()),
            Span::styled(" close", styles::muted()),
        ]))
        .style(Style::default().bg(styles::SURFACE_RAISED)),
        sections[1],
    );
}

fn review_render_line_count(file: &FileDiff) -> usize {
    1 + file
        .hunks
        .iter()
        .map(|hunk| 2 + hunk.lines.len())
        .sum::<usize>()
}

fn hunk_line_start(file: &FileDiff, hunk_index: usize) -> usize {
    let mut line = 1;
    for (index, hunk) in file.hunks.iter().enumerate() {
        if index == hunk_index {
            return line;
        }
        line += 2 + hunk.lines.len();
    }
    0
}

fn hunk_index_for_line(file: &FileDiff, line_index: usize) -> usize {
    if file.hunks.is_empty() {
        return 0;
    }

    let mut current_line = 1;
    let mut current_hunk = 0;
    for (index, hunk) in file.hunks.iter().enumerate() {
        let hunk_end = current_line + hunk.lines.len();
        if line_index <= hunk_end {
            return index;
        }
        current_line = hunk_end + 1;
        current_hunk = index;
    }
    current_hunk
}

fn sync_cursor_line_to_hunk(review: &mut ReviewUiState) {
    let Some(file) = review.files.get(review.cursor_file) else {
        review.cursor_line = 0;
        return;
    };

    if file.hunks.is_empty() {
        review.cursor_line = 0;
        review.cursor_hunk = 0;
        return;
    }

    review.cursor_hunk = review.cursor_hunk.min(file.hunks.len().saturating_sub(1));
    review.cursor_line = hunk_line_start(file, review.cursor_hunk);
}

fn move_review_cursor_by_line(app: &mut App, delta: isize) {
    let Some(file) = app.review.files.get(app.review.cursor_file) else {
        return;
    };

    let max_line = review_render_line_count(file).saturating_sub(1) as isize;
    let next_line = (app.review.cursor_line as isize + delta).clamp(0, max_line) as usize;
    app.review.cursor_line = next_line;

    if !file.hunks.is_empty() {
        app.review.cursor_hunk = hunk_index_for_line(file, next_line);
    }
}

fn explain_context_source_line(app: &App) -> String {
    app.active_session()
        .map(|session| format!("context: {} ({})", session.title, session.id))
        .unwrap_or_else(|| "context: none selected".to_string())
}

fn model_picker_cursor(choice: &WhyModelChoice, models: &[String]) -> usize {
    match choice {
        WhyModelChoice::Auto => 0,
        WhyModelChoice::Explicit(model) => {
            models
                .iter()
                .position(|candidate| candidate == model)
                .unwrap_or(0)
                + 1
        }
    }
}

fn current_model_choice(app: &App) -> WhyModelChoice {
    app.why_this
        .model_override
        .clone()
        .unwrap_or(WhyModelChoice::Auto)
}

fn resolved_why_model(app: &App) -> Option<String> {
    match current_model_choice(app) {
        WhyModelChoice::Auto => app
            .settings
            .explain
            .default_model
            .clone()
            .or_else(|| app.why_this.model.auto_session_model.clone()),
        WhyModelChoice::Explicit(model) => Some(model.clone()),
    }
}

fn auto_model_label(app: &App) -> String {
    app.settings
        .explain
        .default_model
        .clone()
        .or_else(|| app.why_this.model.auto_session_model.clone())
        .unwrap_or_else(|| "session default".to_string())
}

fn why_model_display_label(app: &App) -> String {
    match current_model_choice(app) {
        WhyModelChoice::Auto => format!("Auto ({})", auto_model_label(app)),
        WhyModelChoice::Explicit(model) => model,
    }
}

fn why_cache_key(target: &WhyTarget, session_id: &str, model: Option<&str>) -> String {
    let base = target.cache_key(session_id);
    match model {
        Some(model) => format!("{base}:model:{model}"),
        None => format!("{base}:model:auto"),
    }
}

fn loading_thinking_label(animation: &AnimatedTextState) -> String {
    let phase = (animation.frame / 24) % 4;
    let dots = ".".repeat(phase as usize);
    format!("Thinking{dots}")
}

fn render_why_answer_lines(answer: &WhyAnswer) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.extend(render_why_section(
        "Summary:",
        styles::accent_bold(),
        &answer.summary,
    ));
    lines.extend(render_why_section(
        "Purpose:",
        styles::title(),
        &answer.purpose,
    ));
    lines.extend(render_why_section(
        "Change:",
        styles::title(),
        &answer.change,
    ));
    lines.extend(render_why_section(
        &format!("Risk ({}):", risk_level_label(answer.risk_level.clone())),
        Style::default()
            .fg(styles::DANGER)
            .add_modifier(Modifier::BOLD),
        &answer.risk_reason,
    ));
    lines
}

fn render_why_section(label: &str, label_style: Style, body: &str) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(Span::styled(label.to_string(), label_style))];
    for line in body.lines() {
        lines.push(Line::from(Span::raw(line.to_string())));
    }
    lines.push(Line::from(Span::raw("")));
    lines
}

fn risk_level_label(level: WhyRiskLevel) -> &'static str {
    match level {
        WhyRiskLevel::Low => "low",
        WhyRiskLevel::Medium => "medium",
        WhyRiskLevel::High => "high",
    }
}

fn current_why_target(review: &ReviewUiState) -> Option<(String, WhyTarget)> {
    let file = review.files.get(review.cursor_file)?;
    if review.focus == ReviewFocus::Files || file.hunks.is_empty() {
        let target = why_target_for_file(file);
        let label = target.label();
        return Some((label, target));
    }

    let hunk = file.hunks.get(review.cursor_hunk)?;
    let target = why_target_for_hunk(file, hunk);
    let label = target.label();
    Some((label, target))
}

fn review_marker(
    status: ReviewStatus,
    file_status: crate::domain::diff::FileStatus,
    is_hunk: bool,
) -> &'static str {
    match status {
        ReviewStatus::Accepted => "[✓]",
        ReviewStatus::Rejected => "[x]",
        ReviewStatus::Unreviewed if is_hunk => "[ ]",
        ReviewStatus::Unreviewed => match file_status {
            crate::domain::diff::FileStatus::Added => "[+]",
            crate::domain::diff::FileStatus::Deleted => "[-]",
            crate::domain::diff::FileStatus::Modified => "[ ]",
        },
    }
}

fn render_brand_lockup(frame: &mut ratatui::Frame, area: Rect, app: &App, alignment: Alignment) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let icon = current_brand_icon(&app.logo_animation);
    let icon_width = icon.chars().count() as u16;
    if area.width < icon_width {
        return;
    }

    let word_width = BRAND_WORDMARK.chars().count() as u16;
    let gap_width = 2;
    let show_wordmark = area.width >= icon_width + gap_width + word_width;
    let content_width = if show_wordmark {
        icon_width + gap_width + word_width
    } else {
        icon_width
    };

    let x = match alignment {
        Alignment::Center => area.x + area.width.saturating_sub(content_width) / 2,
        Alignment::Right => area.x + area.width.saturating_sub(content_width),
        Alignment::Left => area.x,
    };
    let icon_area = Rect::new(x, area.y, icon_width, 1);

    let icon_style = if icon == BRAND_ICON_ALT {
        AnimatedTextStyle::pulse(styles::SUCCESS, styles::ACCENT_BRIGHT).modifiers(Modifier::BOLD)
    } else {
        AnimatedTextStyle::pulse(styles::ACCENT, styles::ACCENT_BRIGHT).modifiers(Modifier::BOLD)
    };

    AnimatedText::new(icon, &app.logo_animation)
        .style(icon_style)
        .render(icon_area, frame.buffer_mut());

    if show_wordmark {
        let word_area = Rect::new(x + icon_width + gap_width, area.y, word_width, 1);
        AnimatedText::new(BRAND_WORDMARK, &app.logo_animation)
            .style(
                AnimatedTextStyle::wave(styles::TEXT_MUTED, styles::ACCENT_BRIGHT)
                    .modifiers(Modifier::BOLD)
                    .wave_width(4),
            )
            .render(word_area, frame.buffer_mut());
    }
}

fn to_textarea_input(key: KeyEvent) -> ratatui_textarea::Input {
    ratatui_textarea::Input {
        key: match key.code {
            KeyCode::Backspace => ratatui_textarea::Key::Backspace,
            KeyCode::Enter => ratatui_textarea::Key::Enter,
            KeyCode::Left => ratatui_textarea::Key::Left,
            KeyCode::Right => ratatui_textarea::Key::Right,
            KeyCode::Up => ratatui_textarea::Key::Up,
            KeyCode::Down => ratatui_textarea::Key::Down,
            KeyCode::Home => ratatui_textarea::Key::Home,
            KeyCode::End => ratatui_textarea::Key::End,
            KeyCode::PageUp => ratatui_textarea::Key::PageUp,
            KeyCode::PageDown => ratatui_textarea::Key::PageDown,
            KeyCode::Delete => ratatui_textarea::Key::Delete,
            KeyCode::Char(ch) => ratatui_textarea::Key::Char(ch),
            KeyCode::Tab => ratatui_textarea::Key::Tab,
            _ => ratatui_textarea::Key::Null,
        },
        ctrl: key.modifiers.contains(KeyModifiers::CONTROL),
        alt: key.modifiers.contains(KeyModifiers::ALT),
        shift: key.modifiers.contains(KeyModifiers::SHIFT),
    }
}

fn truncate_path(path: &str, max_len: usize) -> String {
    if path.chars().count() <= max_len {
        return path.to_string();
    }
    let suffix = path
        .chars()
        .rev()
        .take(max_len.saturating_sub(3))
        .collect::<String>();
    format!("...{}", suffix.chars().rev().collect::<String>())
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::diff::{DiffLine, DiffLineKind, FileDiff, FileStatus, Hunk, ReviewStatus};
    use crate::settings::ExplainSettings;

    fn sample_app(review: ReviewUiState) -> App {
        let (tx, rx) = mpsc::unbounded_channel();
        App {
            repo_path: PathBuf::from("."),
            git: GitService::new("."),
            opencode: None,
            settings: AppSettings::default(),
            settings_store: SettingsStore::from_path(PathBuf::from("/tmp/better-review-test.json")),
            settings_cursor: 0,
            saved_model_cursor: 0,
            session_state: SessionUiState::default(),
            why_this: WhyThisUiState::default(),
            status: String::new(),
            screen: Screen::Review,
            review,
            overlay: Overlay::None,
            had_staged_changes_on_open: false,
            review_busy: false,
            logo_animation: AnimatedTextState::with_interval(120),
            tx,
            rx,
        }
    }

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
                    header: "@@ -10,1 +10,1 @@".to_string(),
                    old_start: 10,
                    old_count: 1,
                    new_start: 10,
                    new_count: 1,
                    lines: vec![DiffLine {
                        kind: DiffLineKind::Context,
                        content: "ctx".to_string(),
                        old_line: Some(10),
                        new_line: Some(10),
                    }],
                    review_status: ReviewStatus::Accepted,
                },
            ],
            review_status: ReviewStatus::Unreviewed,
            ..FileDiff::default()
        }
    }

    #[test]
    fn review_counts_aggregate_file_and_hunk_statuses() {
        let mut app = sample_app(ReviewUiState {
            files: vec![
                sample_file(),
                FileDiff {
                    new_path: "README.md".to_string(),
                    review_status: ReviewStatus::Rejected,
                    ..FileDiff::default()
                },
            ],
            ..ReviewUiState::default()
        });
        app.screen = Screen::Home;

        let counts = app.review_counts();
        assert_eq!(counts.unreviewed, 1);
        assert_eq!(counts.accepted, 1);
        assert_eq!(counts.rejected, 1);

        app.review.files[0].set_all_hunks_status(ReviewStatus::Accepted);
        let counts = app.review_counts();
        assert_eq!(counts.unreviewed, 0);
        assert_eq!(counts.accepted, 2);
    }

    #[tokio::test]
    async fn open_commit_from_home_without_reviewable_changes_sets_status_message() {
        let mut app = sample_app(ReviewUiState::default());
        app.screen = Screen::Home;

        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE);
        if key.code == KeyCode::Char('c') {
            if app.review.files.is_empty() {
                app.status =
                    "Cannot commit yet because there are no reviewable changes in this repository."
                        .to_string();
            } else if app.review_busy {
                app.status = "Wait for the current review update to finish.".to_string();
            } else {
                let _ = app.open_commit_prompt();
            }
        }

        assert_eq!(app.overlay, Overlay::None);
        assert!(app.status.contains("there are no reviewable changes"));
    }

    #[test]
    fn draw_home_includes_status_message() {
        let mut app = sample_app(ReviewUiState::default());
        app.screen = Screen::Home;
        app.status = "There is nothing to commit yet.".to_string();

        let text = app.status.clone();
        assert!(text.contains("nothing to commit"));
    }

    #[test]
    fn new_commit_message_input_sets_placeholder_and_wrap() {
        let input = new_commit_message_input();
        assert_eq!(input.lines(), vec![String::new()]);
    }

    #[test]
    fn review_render_helpers_track_hunk_positions() {
        let file = sample_file();
        assert_eq!(review_render_line_count(&file), 8);
        assert_eq!(hunk_line_start(&file, 0), 1);
        assert_eq!(hunk_line_start(&file, 1), 5);
        assert_eq!(hunk_index_for_line(&file, 0), 0);
        assert_eq!(hunk_index_for_line(&file, 2), 0);
        assert_eq!(hunk_index_for_line(&file, 5), 1);
        assert_eq!(hunk_index_for_line(&file, 99), 1);
    }

    #[test]
    fn sync_cursor_line_to_hunk_clamps_indices() {
        let mut review = ReviewUiState {
            files: vec![sample_file()],
            cursor_file: 0,
            cursor_hunk: 99,
            cursor_line: 0,
            focus: ReviewFocus::Files,
        };

        sync_cursor_line_to_hunk(&mut review);
        assert_eq!(review.cursor_hunk, 1);
        assert_eq!(review.cursor_line, 5);
    }

    #[test]
    fn sync_cursor_line_to_hunk_handles_empty_hunks() {
        let mut review = ReviewUiState {
            files: vec![FileDiff::default()],
            cursor_file: 0,
            cursor_hunk: 3,
            cursor_line: 7,
            focus: ReviewFocus::Files,
        };

        sync_cursor_line_to_hunk(&mut review);
        assert_eq!(review.cursor_hunk, 0);
        assert_eq!(review.cursor_line, 0);
    }

    #[test]
    fn move_review_cursor_by_line_updates_current_hunk() {
        let mut app = sample_app(ReviewUiState {
            files: vec![sample_file()],
            cursor_file: 0,
            cursor_hunk: 0,
            cursor_line: 1,
            focus: ReviewFocus::Hunks,
        });

        move_review_cursor_by_line(&mut app, 4);
        assert_eq!(app.review.cursor_line, 5);
        assert_eq!(app.review.cursor_hunk, 1);

        move_review_cursor_by_line(&mut app, -99);
        assert_eq!(app.review.cursor_line, 0);
        assert_eq!(app.review.cursor_hunk, 0);
    }

    #[test]
    fn review_marker_and_path_helpers_match_expected_output() {
        assert_eq!(
            review_marker(ReviewStatus::Accepted, FileStatus::Modified, false),
            "[✓]"
        );
        assert_eq!(
            review_marker(ReviewStatus::Rejected, FileStatus::Modified, false),
            "[x]"
        );
        assert_eq!(
            review_marker(ReviewStatus::Unreviewed, FileStatus::Added, false),
            "[+]"
        );
        assert_eq!(
            review_marker(ReviewStatus::Unreviewed, FileStatus::Deleted, false),
            "[-]"
        );
        assert_eq!(
            review_marker(ReviewStatus::Unreviewed, FileStatus::Modified, true),
            "[ ]"
        );
        assert_eq!(truncate_path("short.rs", 20), "short.rs");
        assert_eq!(truncate_path("very/long/path/file.rs", 10), "...file.rs");
    }

    #[test]
    fn brand_helpers_and_centered_rect_behave_consistently() {
        let mut animation = AnimatedTextState::with_interval(120);
        animation.frame = 0;
        assert_eq!(current_brand_icon(&animation), BRAND_ICON);
        animation.frame = 128;
        assert_eq!(current_brand_icon(&animation), BRAND_ICON_ALT);
        assert!(brand_lockup_width() > BRAND_WORDMARK.len() as u16);

        let rect = centered_rect(50, 40, Rect::new(0, 0, 100, 50));
        assert_eq!(rect.width, 50);
        assert_eq!(rect.height, 20);
        assert_eq!(rect.x, 25);
        assert_eq!(rect.y, 15);
    }

    #[test]
    fn diff_scroll_offset_respects_focus_and_window() {
        let app = sample_app(ReviewUiState {
            files: vec![sample_file()],
            cursor_file: 0,
            cursor_hunk: 1,
            cursor_line: 6,
            focus: ReviewFocus::Hunks,
        });

        let lines = vec![
            Line::raw("0"),
            Line::raw("1"),
            Line::raw("2"),
            Line::raw("3"),
            Line::raw("4"),
            Line::raw("5"),
            Line::raw("6"),
        ];
        assert_eq!(diff_scroll_offset(&app, Rect::new(0, 0, 10, 4), &lines), 3);

        let mut files_view = app;
        files_view.review.focus = ReviewFocus::Files;
        assert_eq!(
            diff_scroll_offset(&files_view, Rect::new(0, 0, 10, 4), &lines),
            0
        );
    }

    #[test]
    fn to_textarea_input_maps_keys_and_modifiers() {
        let mapped = to_textarea_input(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL));
        assert!(mapped.ctrl);
        assert!(!mapped.alt);
        assert!(!mapped.shift);

        let mapped = to_textarea_input(KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT));
        assert!(mapped.shift);
    }

    #[test]
    fn current_why_target_uses_file_scope_when_focus_is_files() {
        let review = ReviewUiState {
            files: vec![sample_file()],
            cursor_file: 0,
            cursor_hunk: 0,
            cursor_line: 0,
            focus: ReviewFocus::Files,
        };

        let (label, target) = current_why_target(&review).expect("target");
        assert_eq!(label, "file src/lib.rs");
        match target {
            WhyTarget::File { path, .. } => assert_eq!(path, "src/lib.rs"),
            _ => panic!("expected file target"),
        }
    }

    #[test]
    fn current_why_target_uses_hunk_scope_for_hunk_header() {
        let file = sample_file();
        let review = ReviewUiState {
            files: vec![file.clone()],
            cursor_file: 0,
            cursor_hunk: 1,
            cursor_line: hunk_line_start(&file, 1),
            focus: ReviewFocus::Hunks,
        };

        let (label, target) = current_why_target(&review).expect("target");
        assert!(label.starts_with("hunk src/lib.rs"));
        match target {
            WhyTarget::Hunk { header, .. } => assert_eq!(header, "@@ -10,1 +10,1 @@"),
            _ => panic!("expected hunk target"),
        }
    }

    #[test]
    fn current_why_target_uses_hunk_scope_inside_hunk_body() {
        let file = sample_file();
        let review = ReviewUiState {
            files: vec![file.clone()],
            cursor_file: 0,
            cursor_hunk: 0,
            cursor_line: hunk_line_start(&file, 0) + 2,
            focus: ReviewFocus::Hunks,
        };

        let (label, target) = current_why_target(&review).expect("target");
        assert!(label.starts_with("hunk src/lib.rs"));
        match target {
            WhyTarget::Hunk { header, .. } => assert_eq!(header, "@@ -1,2 +1,2 @@"),
            _ => panic!("expected hunk target"),
        }
    }

    #[test]
    fn explain_scope_preview_matches_review_focus() {
        let mut app = sample_app(ReviewUiState {
            files: vec![sample_file()],
            cursor_file: 0,
            cursor_hunk: 1,
            cursor_line: 0,
            focus: ReviewFocus::Files,
        });

        assert_eq!(
            explain_scope_preview(&app),
            Some("file src/lib.rs".to_string())
        );

        app.review.focus = ReviewFocus::Hunks;
        assert_eq!(
            explain_scope_preview(&app),
            Some("hunk src/lib.rs @@ -10,1 +10,1 @@".to_string())
        );
    }

    #[test]
    fn model_picker_cursor_resolves_auto_and_explicit() {
        let models = vec![
            "openai/gpt-5".to_string(),
            "github-copilot/gpt-5.3-codex".to_string(),
        ];

        assert_eq!(model_picker_cursor(&WhyModelChoice::Auto, &models), 0);
        assert_eq!(
            model_picker_cursor(
                &WhyModelChoice::Explicit("github-copilot/gpt-5.3-codex".to_string()),
                &models,
            ),
            2
        );
    }

    #[test]
    fn why_cache_key_is_model_aware() {
        let file = sample_file();
        let target = why_target_for_file(&file);
        let auto_key = why_cache_key(&target, "ses_1", None);
        let explicit_key = why_cache_key(&target, "ses_1", Some("openai/gpt-5"));

        assert_ne!(auto_key, explicit_key);
        assert!(auto_key.contains(":model:auto"));
        assert!(explicit_key.contains(":model:openai/gpt-5"));
    }

    #[test]
    fn render_why_answer_lines_styles_named_sections() {
        let lines = render_why_answer_lines(&WhyAnswer {
            summary: "explain".to_string(),
            purpose: "make the diff understandable".to_string(),
            change: "add picker".to_string(),
            risk_level: WhyRiskLevel::Medium,
            risk_reason: "medium impact".to_string(),
            fork_session_id: "ses_1".to_string(),
        });
        assert!(lines.iter().any(|line| {
            line.spans.iter().any(|span| {
                span.content.as_ref() == "Summary:" && span.style.fg == Some(styles::ACCENT_BRIGHT)
            })
        }));
        assert!(lines.iter().any(|line| {
            line.spans.iter().any(|span| {
                span.content.as_ref() == "Risk (medium):" && span.style.fg == Some(styles::DANGER)
            })
        }));
    }

    #[test]
    fn why_model_helpers_resolve_auto_and_explicit_modes() {
        let mut app = sample_app(ReviewUiState {
            files: vec![sample_file()],
            ..ReviewUiState::default()
        });

        assert_eq!(auto_model_label(&app), "session default");
        assert_eq!(why_model_display_label(&app), "Auto (session default)");
        assert_eq!(resolved_why_model(&app), None);

        app.why_this.model.auto_session_model = Some("github-copilot/gpt-5.3-codex".to_string());
        assert_eq!(
            why_model_display_label(&app),
            "Auto (github-copilot/gpt-5.3-codex)"
        );
        assert_eq!(
            resolved_why_model(&app),
            Some("github-copilot/gpt-5.3-codex".to_string())
        );

        app.why_this.model_override = Some(WhyModelChoice::Explicit("openai/gpt-5".to_string()));
        assert_eq!(why_model_display_label(&app), "openai/gpt-5");
        assert_eq!(resolved_why_model(&app), Some("openai/gpt-5".to_string()));

        app.why_this.model_override = None;
        app.settings.explain.default_model = Some("openai/gpt-5.4".to_string());
        assert_eq!(why_model_display_label(&app), "Auto (openai/gpt-5.4)");
        assert_eq!(resolved_why_model(&app), Some("openai/gpt-5.4".to_string()));
    }

    #[test]
    fn apply_saved_settings_restores_default_explain_model() {
        let mut app = sample_app(ReviewUiState {
            files: vec![sample_file()],
            ..ReviewUiState::default()
        });
        app.settings = AppSettings {
            version: 1,
            explain: ExplainSettings {
                default_model: Some("openai/gpt-5.4".to_string()),
            },
        };
        app.why_this.model.available = vec!["openai/gpt-5.4".to_string()];
        app.why_this.model_override = Some(WhyModelChoice::Explicit("openai/gpt-5".to_string()));

        app.apply_saved_settings();

        assert_eq!(app.saved_model_cursor, 1);
        assert_eq!(
            app.why_this.model_override,
            Some(WhyModelChoice::Explicit("openai/gpt-5".to_string()))
        );
    }

    #[test]
    fn open_settings_sets_overlay_and_status() {
        let mut app = sample_app(ReviewUiState {
            files: vec![sample_file()],
            ..ReviewUiState::default()
        });

        open_settings(&mut app);

        assert_eq!(app.overlay, Overlay::Settings);
        assert!(app.status.contains("Settings loaded from"));
    }

    #[test]
    fn open_saved_model_picker_requires_opencode() {
        let mut app = sample_app(ReviewUiState {
            files: vec![sample_file()],
            ..ReviewUiState::default()
        });
        open_saved_model_picker(&mut app);

        assert_eq!(app.overlay, Overlay::None);
        assert!(
            app.status
                .contains("Default Explain model selection is unavailable")
        );
    }

    #[test]
    fn handle_saved_model_picker_key_updates_persistent_default_model() {
        let mut app = sample_app(ReviewUiState {
            files: vec![sample_file()],
            ..ReviewUiState::default()
        });
        let temp = tempfile::tempdir().unwrap();
        app.settings_store = SettingsStore::from_path(temp.path().join("config.json"));
        app.overlay = Overlay::SettingsModelPicker;
        app.why_this.model.available = vec!["openai/gpt-5.4".to_string()];
        app.saved_model_cursor = 1;

        handle_saved_model_picker_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(
            app.settings.explain.default_model,
            Some("openai/gpt-5.4".to_string())
        );
        assert_eq!(app.overlay, Overlay::Settings);
        assert!(
            app.status
                .contains("Default Explain model set to openai/gpt-5.4")
        );
    }

    #[test]
    fn handle_saved_model_picker_key_supports_auto_and_escape() {
        let mut app = sample_app(ReviewUiState {
            files: vec![sample_file()],
            ..ReviewUiState::default()
        });
        app.overlay = Overlay::SettingsModelPicker;
        app.settings.explain.default_model = Some("openai/gpt-5.4".to_string());
        app.saved_model_cursor = 0;

        handle_saved_model_picker_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.settings.explain.default_model, None);
        assert_eq!(app.overlay, Overlay::Settings);

        app.overlay = Overlay::SettingsModelPicker;
        handle_saved_model_picker_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.overlay, Overlay::Settings);
    }

    #[test]
    fn explain_context_source_line_uses_selected_session_when_available() {
        let mut app = sample_app(ReviewUiState {
            files: vec![sample_file()],
            ..ReviewUiState::default()
        });
        assert_eq!(explain_context_source_line(&app), "context: none selected");

        app.session_state.sessions = vec![OpencodeSession {
            id: "ses_1".to_string(),
            title: "Main Session".to_string(),
            directory: PathBuf::from("."),
            time_updated: 1,
        }];
        app.session_state.selected = Some(0);
        assert_eq!(
            explain_context_source_line(&app),
            "context: Main Session (ses_1)".to_string()
        );
    }

    #[test]
    fn open_explain_menu_sets_overlay_and_status() {
        let mut app = sample_app(ReviewUiState {
            files: vec![sample_file()],
            ..ReviewUiState::default()
        });

        open_explain_menu(&mut app);

        assert_eq!(app.overlay, Overlay::ExplainMenu);
        assert!(app.why_this.return_to_menu);
        assert!(app.status.contains("Choose a file or hunk"));
    }

    #[test]
    fn close_explain_submenu_returns_to_menu_when_requested() {
        let mut app = sample_app(ReviewUiState {
            files: vec![sample_file()],
            ..ReviewUiState::default()
        });
        app.overlay = Overlay::ModelPicker;
        app.why_this.return_to_menu = true;

        close_explain_submenu(&mut app, "Back to explain menu.");

        assert_eq!(app.overlay, Overlay::ExplainMenu);
        assert_eq!(app.status, "Back to explain menu.");
    }

    #[test]
    fn explain_menu_lines_show_scope_and_actions() {
        let mut app = sample_app(ReviewUiState {
            files: vec![sample_file()],
            cursor_file: 0,
            cursor_hunk: 0,
            focus: ReviewFocus::Hunks,
            ..ReviewUiState::default()
        });
        app.session_state.sessions = vec![OpencodeSession {
            id: "ses_1".to_string(),
            title: "Main Session".to_string(),
            directory: PathBuf::from("."),
            time_updated: 1,
        }];
        app.session_state.selected = Some(0);

        let text = explain_menu_lines(&app)
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(text.contains("Scope  hunk src/lib.rs @@ -1,2 +1,2 @@"));
        assert!(
            text.contains("ContextMain Session (ses_1)")
                || text.contains("Context Main Session (ses_1)")
        );
        assert!(text.contains("Enter run explain"));
        assert!(text.contains("c choose context"));
        assert!(text.contains("m choose model"));
    }

    #[test]
    fn handle_model_picker_key_updates_cursor_and_selection() {
        let mut app = sample_app(ReviewUiState {
            files: vec![sample_file()],
            ..ReviewUiState::default()
        });
        app.overlay = Overlay::ModelPicker;
        app.why_this.model.available = vec!["openai/gpt-5".to_string()];

        handle_model_picker_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.why_this.model.cursor, 1);

        handle_model_picker_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(
            app.why_this.model_override,
            Some(WhyModelChoice::Explicit(ref model)) if model == "openai/gpt-5"
        ));
        assert_eq!(app.overlay, Overlay::None);
    }

    #[test]
    fn handle_model_picker_key_selects_auto_and_supports_escape() {
        let mut app = sample_app(ReviewUiState {
            files: vec![sample_file()],
            ..ReviewUiState::default()
        });
        app.overlay = Overlay::ModelPicker;
        app.why_this.model_override = Some(WhyModelChoice::Explicit("openai/gpt-5".to_string()));
        app.why_this.model.cursor = 0;

        handle_model_picker_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.why_this.model_override, Some(WhyModelChoice::Auto));

        app.overlay = Overlay::ModelPicker;
        handle_model_picker_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.overlay, Overlay::None);
    }

    #[test]
    fn explain_panel_lines_show_model_and_selection_guidance() {
        let app = sample_app(ReviewUiState {
            files: vec![sample_file()],
            cursor_file: 0,
            focus: ReviewFocus::Files,
            ..ReviewUiState::default()
        });

        let text = explain_panel_lines(&app)
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(text.contains("model:"));
        assert!(text.contains("Explain the current change"));
        assert!(text.contains("open the Explain menu"));
        assert!(text.contains("scope: file src/lib.rs"));
    }

    #[test]
    fn move_explain_history_cursor_wraps_between_runs() {
        let mut app = sample_app(ReviewUiState {
            files: vec![sample_file()],
            ..ReviewUiState::default()
        });
        app.why_this.runs = vec![
            ExplainRun {
                id: 1,
                label: "first".to_string(),
                target: why_target_for_file(&sample_file()),
                context_source_id: "ses_1".to_string(),
                context_source_label: "session".to_string(),
                requested_model: None,
                model_label: "Auto".to_string(),
                cache_key: "a".to_string(),
                status: ExplainRunStatus::Ready,
                result: Some(WhyAnswer {
                    summary: "a".to_string(),
                    purpose: "b".to_string(),
                    change: "b".to_string(),
                    risk_level: WhyRiskLevel::Low,
                    risk_reason: "c".to_string(),
                    fork_session_id: "ses_1".to_string(),
                }),
                error: None,
                handle: None,
            },
            ExplainRun {
                id: 2,
                label: "second".to_string(),
                target: why_target_for_file(&sample_file()),
                context_source_id: "ses_1".to_string(),
                context_source_label: "session".to_string(),
                requested_model: None,
                model_label: "Auto".to_string(),
                cache_key: "b".to_string(),
                status: ExplainRunStatus::Cancelled,
                result: None,
                error: None,
                handle: None,
            },
        ];
        app.why_this.history_cursor = 0;

        move_explain_history_cursor(&mut app, -1);
        assert_eq!(app.why_this.history_cursor, 1);

        move_explain_history_cursor(&mut app, 1);
        assert_eq!(app.why_this.history_cursor, 0);
    }

    #[test]
    fn cancel_current_explain_marks_running_run_cancelled() {
        let mut app = sample_app(ReviewUiState {
            files: vec![sample_file()],
            ..ReviewUiState::default()
        });
        app.why_this.runs = vec![ExplainRun {
            id: 7,
            label: "job".to_string(),
            target: why_target_for_file(&sample_file()),
            context_source_id: "ses_1".to_string(),
            context_source_label: "session".to_string(),
            requested_model: None,
            model_label: "Auto".to_string(),
            cache_key: "cache".to_string(),
            status: ExplainRunStatus::Running,
            result: None,
            error: None,
            handle: None,
        }];
        app.why_this.current_run_id = Some(7);

        cancel_current_explain(&mut app);

        assert!(matches!(
            app.why_this.runs[0].status,
            ExplainRunStatus::Cancelled
        ));
        assert!(app.status.contains("Cancelled explain run #7"));
    }

    #[test]
    fn clear_history_run_removes_finished_run() {
        let mut app = sample_app(ReviewUiState {
            files: vec![sample_file()],
            ..ReviewUiState::default()
        });
        app.overlay = Overlay::ExplainHistory;
        app.why_this.runs = vec![ExplainRun {
            id: 8,
            label: "job".to_string(),
            target: why_target_for_file(&sample_file()),
            context_source_id: "ses_1".to_string(),
            context_source_label: "session".to_string(),
            requested_model: None,
            model_label: "Auto".to_string(),
            cache_key: "cache".to_string(),
            status: ExplainRunStatus::Failed,
            result: None,
            error: Some("boom".to_string()),
            handle: None,
        }];
        app.why_this.history_cursor = 0;

        clear_history_run(&mut app);

        assert!(app.why_this.runs.is_empty());
        assert_eq!(app.why_this.current_run_id, None);
        assert_eq!(app.overlay, Overlay::None);
    }

    #[test]
    fn loading_thinking_label_cycles_states() {
        let mut animation = AnimatedTextState::with_interval(120);
        animation.frame = 0;
        assert_eq!(loading_thinking_label(&animation), "Thinking");
        animation.frame = 24;
        assert_eq!(loading_thinking_label(&animation), "Thinking.");
        animation.frame = 48;
        assert_eq!(loading_thinking_label(&animation), "Thinking..");
        animation.frame = 72;
        assert_eq!(loading_thinking_label(&animation), "Thinking...");
    }

    #[test]
    fn render_why_answer_lines_preserves_unlabeled_paragraphs() {
        let lines = render_why_answer_lines(&WhyAnswer {
            summary: "General note".to_string(),
            purpose: "Explain the purpose".to_string(),
            change: "Specific delta".to_string(),
            risk_level: WhyRiskLevel::Low,
            risk_reason: "Limited risk".to_string(),
            fork_session_id: "ses_1".to_string(),
        });
        let text = lines
            .iter()
            .flat_map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref().to_string())
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("General note"));
        assert!(text.contains("Specific delta"));
        assert!(text.contains("Limited risk"));
    }

    #[tokio::test]
    async fn open_model_picker_reports_when_opencode_is_unavailable() {
        let mut app = sample_app(ReviewUiState {
            files: vec![sample_file()],
            ..ReviewUiState::default()
        });
        app.opencode = None;

        open_model_picker(&mut app).await;
        assert!(app.status.contains("model selection is unavailable"));
    }

    #[tokio::test]
    async fn request_explain_requires_attributed_session() {
        let mut app = sample_app(ReviewUiState {
            files: vec![sample_file()],
            ..ReviewUiState::default()
        });
        app.opencode = OpencodeService::new(".").ok();
        app.session_state.selected = None;

        request_explain(&mut app).await.unwrap();
        assert!(app.status.contains("No context source is linked"));
    }
}
