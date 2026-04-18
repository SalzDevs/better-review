use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    Clear as TerminalClear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
    enable_raw_mode,
};
use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;
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

use crate::domain::diff::{DiffLineKind, FileDiff, ReviewStatus};
use crate::domain::model_catalog::ModelOption;
use crate::domain::session::WorkspaceSnapshot;
use crate::services::git::GitService;
use crate::services::opencode::{OpencodeService, RunResult};
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
    execute!(terminal.backend_mut(), DisableMouseCapture, LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

struct App {
    repo_path: PathBuf,
    git: GitService,
    opencode: OpencodeService,
    status: String,
    run_state: RunState,
    screen: Screen,
    review: ReviewUiState,
    overlay: Overlay,
    models: Vec<ModelOption>,
    selected_model: Option<String>,
    selected_variant: Option<String>,
    session_snapshot: Option<WorkspaceSnapshot>,
    review_busy: bool,
    logo_animation: AnimatedTextState,
    tx: mpsc::UnboundedSender<Message>,
    rx: mpsc::UnboundedReceiver<Message>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunState {
    Ready,
    Running,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Overlay {
    None,
    Composer,
    ModelPicker,
    CommitPrompt,
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
    session_only: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ReviewFocus {
    #[default]
    Files,
    Hunks,
}

enum Message {
    ModelsLoaded(Result<Vec<ModelOption>, String>),
    PromptFinished(Result<RunResult, String>),
    HunkSyncFinished {
        file_index: usize,
        original_file: FileDiff,
        updated_file: FileDiff,
        success_status: String,
        result: Result<(), String>,
    },
}

impl App {
    async fn new() -> Result<Self> {
        let repo_path = std::env::current_dir()?;
        let git = GitService::new(&repo_path);
        let opencode = OpencodeService::new(&repo_path, "opencode");
        let (tx, rx) = mpsc::unbounded_channel();
        let mut app = Self {
            repo_path,
            git,
            opencode,
            status: "Press Ctrl+O to open the composer.".to_string(),
            run_state: RunState::Ready,
            screen: Screen::Home,
            review: ReviewUiState::default(),
            overlay: Overlay::None,
            models: Vec::new(),
            selected_model: None,
            selected_variant: None,
            session_snapshot: None,
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
        self.review.session_only = false;

        let tx = self.tx.clone();
        let service = self.opencode.clone();
        tokio::spawn(async move {
            let result = service.load_models().await.map_err(|err| err.to_string());
            let _ = tx.send(Message::ModelsLoaded(result));
        });

        Ok(())
    }

    fn current_model_label(&self) -> String {
        let Some(selected_model) = self.selected_model.as_ref() else {
            return "loading...".to_string();
        };

        let Some(model) = self
            .models
            .iter()
            .find(|option| &option.id == selected_model)
        else {
            return sanitize_model_display(selected_model);
        };

        let name = sanitize_model_display(&model.name);
        match self.selected_variant.as_deref().map(sanitize_variant_display) {
            Some(variant) if !variant.is_empty() => format!("{name} [{variant}]"),
            _ => name,
        }
    }

    fn selected_model_variants(&self) -> Vec<String> {
        self.models
            .iter()
            .find(|model| Some(&model.id) == self.selected_model.as_ref())
            .map(|model| model.variants.clone())
            .unwrap_or_default()
    }

    fn current_file_path(&self) -> Option<&str> {
        self.review.files.get(self.review.cursor_file).map(FileDiff::display_path)
    }

    fn current_file_has_protected_unstaged_content(&self) -> bool {
        let Some(path) = self.current_file_path() else {
            return false;
        };
        self.session_snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.has_unstaged_path(path))
    }

    fn review_context_label(&self) -> String {
        match &self.session_snapshot {
            Some(snapshot) => {
                let protected = snapshot.protected_path_count();
                if protected == 0 {
                    "Session-only review".to_string()
                } else {
                    format!("Session-only review  |  protecting {protected} pre-run path(s)")
                }
            }
            None => "Workspace review".to_string(),
        }
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

fn new_composer_input() -> TextArea<'static> {
    let mut composer = TextArea::default();
    composer.set_placeholder_text("Describe the change you want opencode to make");
    composer.set_wrap_mode(WrapMode::WordOrGlyph);
    composer
}

fn new_commit_message_input() -> TextArea<'static> {
    let mut commit_message = TextArea::default();
    commit_message.set_placeholder_text("Write the commit message for accepted changes");
    commit_message.set_wrap_mode(WrapMode::WordOrGlyph);
    commit_message
}

async fn submit_composer_prompt(app: &mut App, composer: &mut TextArea<'static>) -> Result<()> {
    if app.run_state == RunState::Running {
        return Ok(());
    }
    let prompt = composer.lines().join("\n").trim().to_string();
    if prompt.is_empty() {
        app.status = "Write a prompt first.".to_string();
        return Ok(());
    }

    app.run_state = RunState::Running;
    app.status = format!("Running opencode with {}...", app.current_model_label());
    let tx = app.tx.clone();
    let service = app.opencode.clone();
    let git = app.git.clone();
    let model = app.selected_model.clone();
    let variant = app.selected_variant.clone();
    let snapshot = git.snapshot_workspace().await?;
    app.session_snapshot = Some(snapshot.clone());
    *composer = new_composer_input();
    tokio::spawn(async move {
        let result = service
            .run_prompt(
                &git,
                &snapshot,
                &prompt,
                model.as_deref(),
                variant.as_deref(),
            )
            .await
            .map_err(|err| err.to_string());
        let _ = tx.send(Message::PromptFinished(result));
    });

    Ok(())
}

async fn submit_commit_message(app: &mut App, commit_message: &mut TextArea<'static>) -> Result<()> {
    let message = commit_message.lines().join("\n").trim().to_string();
    if message.is_empty() {
        app.status = "Write a commit message first.".to_string();
        return Ok(());
    }

    if !app.git.has_staged_changes().await? {
        app.status = "No accepted changes are staged yet.".to_string();
        return Ok(());
    }

    if app
        .session_snapshot
        .as_ref()
        .is_some_and(|snapshot| snapshot.had_staged_changes)
    {
        app.status = "Cannot commit from better-review because the session started with unrelated staged changes."
            .to_string();
        return Ok(());
    }

    app.git.commit_staged(&message).await?;
    let (_, files) = if let Some(snapshot) = app.session_snapshot.as_ref() {
        app.git.collect_session_diff(snapshot).await?
    } else {
        app.git.collect_diff().await?
    };
    app.review.files = files;
    app.review.cursor_file = 0;
    app.review.cursor_hunk = 0;
    app.review.cursor_line = 0;
    app.review.focus = ReviewFocus::Files;
    app.overlay = Overlay::None;
    app.status = "Committed accepted changes.".to_string();
    *commit_message = new_commit_message_input();

    Ok(())
}

async fn run_app(terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>) -> Result<()> {
    let mut app = App::new().await?;
    let matcher = SkimMatcherV2::default();
    let mut composer = new_composer_input();
    let mut commit_message = new_commit_message_input();
    let mut model_search = TextArea::default();
    model_search.set_placeholder_text("Search by provider or model");
    let mut model_cursor = 0_usize;

    loop {
        app.logo_animation
            .tick_with_text_width(usize::from(brand_lockup_width()));

        while let Ok(message) = app.rx.try_recv() {
            match message {
                Message::ModelsLoaded(result) => match result {
                    Ok(models) => {
                        app.models = models;
                        if app.selected_model.is_none() {
                            if let Some(first) = app.models.first() {
                                app.selected_model = Some(first.id.clone());
                                app.selected_variant = first.variants.first().cloned();
                            }
                        }
                    }
                    Err(err) => {
                        app.status = format!("Could not load opencode models: {err}");
                        app.run_state = RunState::Failed;
                    }
                },
                Message::PromptFinished(result) => match result {
                    Ok(run) => {
                        app.review.files = run.changed_files;
                        app.review.cursor_file = 0;
                        app.review.cursor_hunk = 0;
                        app.review.cursor_line = 0;
                        app.review.focus = ReviewFocus::Files;
                        app.review.session_only = app.session_snapshot.is_some();
                        app.screen = if app.review.files.is_empty() {
                            Screen::Home
                        } else {
                            Screen::Review
                        };
                        app.status = if app.review.files.is_empty() {
                            "Run finished with no code changes.".to_string()
                        } else {
                            format!(
                                "Run finished. Review {} changed file(s).",
                                app.review.files.len()
                            )
                        };
                        app.run_state = RunState::Ready;
                        app.overlay = Overlay::None;
                    }
                    Err(err) => {
                        app.status = err;
                        app.run_state = RunState::Failed;
                        app.overlay = Overlay::None;
                    }
                },
                Message::HunkSyncFinished {
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
            }
        }

        terminal.draw(|frame| {
            draw(
                frame,
                &app,
                &composer,
                &commit_message,
                &model_search,
                &matcher,
                model_cursor,
            )
        })?;

        if event::poll(Duration::from_millis(16))? {
            if let Event::Key(key) = event::read()? {
                if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    break;
                }

                match app.overlay {
                    Overlay::Composer => match key.code {
                        KeyCode::Esc => {
                            app.overlay = Overlay::None;
                            app.status =
                                "Review remains active. Press Ctrl+O for a new prompt.".to_string();
                        }
                        KeyCode::Tab => {
                            app.overlay = Overlay::ModelPicker;
                        }
                        KeyCode::Enter => {
                            submit_composer_prompt(&mut app, &mut composer).await?;
                        }
                        KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            cycle_variant(&mut app, 1);
                        }
                        _ => {
                            composer.input(to_textarea_input(key));
                        }
                    },
                    Overlay::ModelPicker => {
                        let filtered = filtered_models(
                            &app.models,
                            &matcher,
                            &model_search.lines().join("\n"),
                        );
                        match key.code {
                            KeyCode::Esc => app.overlay = Overlay::Composer,
                            KeyCode::Up => model_cursor = model_cursor.saturating_sub(1),
                            KeyCode::Down => {
                                if model_cursor + 1 < filtered.len() {
                                    model_cursor += 1;
                                }
                            }
                            KeyCode::Enter => {
                                if let Some(selected) = filtered.get(model_cursor) {
                                    app.selected_model = Some(selected.id.clone());
                                    app.selected_variant = selected.variants.first().cloned();
                                    app.overlay = Overlay::Composer;
                                    app.status =
                                        format!("Selected model {}.", app.current_model_label());
                                }
                            }
                            _ => {
                                model_search.input(to_textarea_input(key));
                                model_cursor = 0;
                            }
                        }
                    }
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
                    Overlay::None => {
                        if key.code == KeyCode::Char('o')
                            && key.modifiers.contains(KeyModifiers::CONTROL)
                        {
                            app.overlay = Overlay::Composer;
                            app.status = "Compose a new prompt.".to_string();
                            continue;
                        }

                        if key.code == KeyCode::Enter && app.screen == Screen::Home {
                            if app.review.files.is_empty() {
                                app.status =
                                    "No reviewable changes yet. Start with Ctrl+O.".to_string();
                            } else {
                                app.screen = Screen::Review;
                                app.status = "Review workspace ready.".to_string();
                            }
                            continue;
                        }

                        if key.code == KeyCode::Char('c') {
                            if app.review_busy {
                                app.status = "Wait for the current review update to finish.".to_string();
                            } else {
                                commit_message = app.open_commit_prompt();
                            }
                            continue;
                        }

                        handle_review_key(&mut app, key).await?;
                    }
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
        KeyCode::Tab => {
            if app.review.focus == ReviewFocus::Hunks {
                if let Some(file) = app.review.files.get(app.review.cursor_file) {
                    if !file.hunks.is_empty() {
                        app.review.cursor_hunk = (app.review.cursor_hunk + 1) % file.hunks.len();
                        sync_cursor_line_to_hunk(&mut app.review);
                    }
                }
            }
        }
        KeyCode::Char('y') => {
            if app.review.focus == ReviewFocus::Files {
                if app.current_file_has_protected_unstaged_content() {
                    app.status =
                        "Cannot accept a file with pre-run unstaged changes. Review hunks or use your editor."
                            .to_string();
                    return Ok(());
                }
                if let Some(file) = app.review.files.get_mut(app.review.cursor_file) {
                    match app.git.accept_file(file).await {
                        Ok(()) => app.status = "Accepted file changes.".to_string(),
                        Err(err) => app.status = format!("Could not accept file: {err}"),
                    }
                }
            } else if let Some(file) = app.review.files.get_mut(app.review.cursor_file) {
                if file.hunks.get(app.review.cursor_hunk).is_some() {
                    let file_index = app.review.cursor_file;
                    let original_file = file.clone();
                    let mut updated_file = file.clone();
                    updated_file.hunks[app.review.cursor_hunk].review_status = ReviewStatus::Accepted;
                    updated_file.sync_review_status();

                    let tx = app.tx.clone();
                    let git = app.git.clone();
                    let snapshot = app.session_snapshot.clone();
                    app.review_busy = true;
                    app.status = "Applying accepted hunk...".to_string();

                    tokio::spawn(async move {
                        let result = git
                            .sync_file_hunks_to_index(&updated_file, snapshot.as_ref())
                            .await
                            .map_err(|err| format!("Could not accept hunk: {err}"));
                        let _ = tx.send(Message::HunkSyncFinished {
                            file_index,
                            original_file,
                            updated_file,
                            success_status: "Accepted hunk.".to_string(),
                            result,
                        });
                    });
                }
            }
        }
        KeyCode::Char('x') => {
            if app.review.focus == ReviewFocus::Files {
                if let Some(file) = app.review.files.get_mut(app.review.cursor_file) {
                    let result = if let Some(snapshot) = app.session_snapshot.as_ref() {
                        app.git.reject_file(file, snapshot).await
                    } else {
                        app.git.reject_file_in_place(file).await
                    };

                    match result {
                        Ok(()) => app.status = "Rejected file changes.".to_string(),
                        Err(err) => app.status = format!("Could not reject file: {err}"),
                    }
                }
            } else if let Some(file) = app.review.files.get_mut(app.review.cursor_file) {
                if file.hunks.get(app.review.cursor_hunk).is_some() {
                    let file_index = app.review.cursor_file;
                    let original_file = file.clone();
                    let mut updated_file = file.clone();
                    updated_file.hunks[app.review.cursor_hunk].review_status = ReviewStatus::Rejected;
                    updated_file.sync_review_status();

                    let tx = app.tx.clone();
                    let git = app.git.clone();
                    let snapshot = app.session_snapshot.clone();
                    app.review_busy = true;
                    app.status = "Rejecting hunk...".to_string();

                    tokio::spawn(async move {
                        let result = git
                            .sync_file_hunks_to_index(&updated_file, snapshot.as_ref())
                            .await
                            .map_err(|err| format!("Could not reject hunk: {err}"));
                        let _ = tx.send(Message::HunkSyncFinished {
                            file_index,
                            original_file,
                            updated_file,
                            success_status: "Rejected hunk.".to_string(),
                            result,
                        });
                    });
                }
            }
        }
        KeyCode::Char('u') => {
            if let Some(file) = app.review.files.get_mut(app.review.cursor_file) {
                let result = if let Some(snapshot) = app.session_snapshot.as_ref() {
                    app.git.unstage_file(file, snapshot).await
                } else {
                    app.git.unstage_file_in_place(file).await
                };

                match result {
                    Ok(()) => app.status = "Moved file back to unreviewed.".to_string(),
                    Err(err) => app.status = format!("Could not unstage file: {err}"),
                }
            }
        }
        _ => {}
    }

    Ok(())
}

fn draw(
    frame: &mut ratatui::Frame,
    app: &App,
    composer: &TextArea<'_>,
    commit_message: &TextArea<'_>,
    model_search: &TextArea<'_>,
    matcher: &SkimMatcherV2,
    model_cursor: usize,
) {
    let size = frame.area();
    let footer_height = if app.screen == Screen::Review { 2 } else { 0 };
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(10),
            Constraint::Length(footer_height),
        ])
        .split(size);

    draw_top_bar(frame, layout[0], app);
    match app.screen {
        Screen::Home => draw_home(frame, layout[1], app),
        Screen::Review => draw_review(frame, layout[1], app),
    }
    if app.screen == Screen::Review {
        draw_footer(frame, layout[2]);
    }

    match app.overlay {
        Overlay::Composer => draw_composer(frame, layout[1], app, composer),
        Overlay::ModelPicker => {
            draw_model_picker(frame, layout[1], app, model_search, matcher, model_cursor)
        }
        Overlay::CommitPrompt => draw_commit_prompt(frame, layout[1], app, commit_message),
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

    let content = area.inner(ratatui::layout::Margin {
        horizontal: 2,
        vertical: 0,
    });
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(content);

    render_brand_lockup(frame, area, app, Alignment::Center);

    let run_status = match app.run_state {
        RunState::Ready => "ready",
        RunState::Running => "running",
        RunState::Failed => "failed",
    };
    let sync_status = if app.review_busy { "syncing" } else { "idle" };
    let meta = Line::from(vec![
        Span::styled("repo ", styles::subtle()),
        Span::styled(
            app.repo_path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("repo"),
            styles::title(),
        ),
        Span::raw("  |  "),
        Span::styled("model ", styles::subtle()),
        Span::styled(app.current_model_label(), styles::title()),
        Span::raw("  |  "),
        Span::styled(run_status, styles::soft_accent()),
        Span::raw("  |  "),
        Span::styled(sync_status, styles::soft_accent()),
        Span::raw("  |  "),
        Span::styled(app.review_context_label(), styles::muted()),
    ]);
    frame.render_widget(Paragraph::new(meta).alignment(Alignment::Center), sections[1]);

    if area.width > 0 {
        frame.render_widget(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(styles::BORDER_MUTED)),
            area,
        );
    }
}

fn draw_footer(frame: &mut ratatui::Frame, area: Rect) {
    let line = Line::from(vec![
        Span::styled("Ctrl+O", styles::keybind()),
        Span::styled(" composer", styles::subtle()),
        Span::raw("  |  "),
        Span::styled("Enter", styles::keybind()),
        Span::styled(" continue", styles::subtle()),
        Span::raw("  |  "),
        Span::styled("Esc", styles::keybind()),
        Span::styled(" home", styles::subtle()),
        Span::raw("  |  "),
        Span::styled("y", styles::keybind()),
        Span::styled(" accept", styles::subtle()),
        Span::raw("  |  "),
        Span::styled("x", styles::keybind()),
        Span::styled(" reject", styles::subtle()),
        Span::raw("  |  "),
        Span::styled("c", styles::keybind()),
        Span::styled(" commit", styles::subtle()),
        Span::raw("    "),
        Span::styled("Ctrl+C", styles::keybind()),
        Span::styled(" quit", styles::subtle()),
    ]);
    frame.render_widget(Paragraph::new(line).style(styles::subtle()), area);
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
        Span::styled("model ", styles::subtle()),
        Span::styled(app.current_model_label(), styles::title()),
    ]);
    frame.render_widget(Paragraph::new(summary).alignment(Alignment::Center), sections[2]);

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
    frame.render_widget(Paragraph::new(queue).alignment(Alignment::Center), sections[3]);

    let action_line = Line::from(vec![
        Span::styled("Ctrl+O", styles::keybind()),
        Span::styled(" compose", styles::muted()),
        Span::raw("      "),
        Span::styled("Enter", styles::keybind()),
        Span::styled(" review", styles::muted()),
        Span::raw("      "),
        Span::styled("c", styles::keybind()),
        Span::styled(" commit", styles::muted()),
        Span::raw("      "),
        Span::styled("Ctrl+C", styles::keybind()),
        Span::styled(" quit", styles::muted()),
    ]);
    frame.render_widget(
        Paragraph::new(action_line)
            .alignment(Alignment::Center)
            .style(styles::soft_accent()),
        sections[4],
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
                "Open the composer and run a prompt to start a review session.",
                styles::muted(),
            )),
            Line::from(Span::styled(
                "Accepted and rejected changes will appear here as soon as the run finishes.",
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
        horizontal: 2,
        vertical: 1,
    });
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(styles::BORDER_MUTED))
            .style(Style::default().bg(styles::SURFACE)),
        canvas,
    );
    let content = canvas.inner(ratatui::layout::Margin {
        horizontal: 2,
        vertical: 1,
    });

    let sections = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(36), Constraint::Min(30)])
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
        Span::raw("  |  "),
        Span::styled(app.status.as_str(), styles::muted()),
    ])];
    let mut line_hunks = vec![None];

    if let Some(file) = app.review.files.get(app.review.cursor_file) {
        for (index, hunk) in file.hunks.iter().enumerate() {
            let is_current_hunk = app.review.focus == ReviewFocus::Hunks && app.review.cursor_hunk == index;
            let is_current_line = app.review.focus == ReviewFocus::Hunks && app.review.cursor_line == diff_lines.len();
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
                ReviewStatus::Accepted => Span::styled(" [accepted]", Style::default().fg(styles::SUCCESS)),
                ReviewStatus::Rejected => Span::styled(" [rejected]", Style::default().fg(styles::DANGER)),
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
            line_hunks.push(Some(index));
            for line in &hunk.lines {
                let is_current_line = app.review.focus == ReviewFocus::Hunks && app.review.cursor_line == diff_lines.len();
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
                    style.bg(styles::SURFACE_RAISED).add_modifier(Modifier::UNDERLINED)
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
                line_hunks.push(Some(index));
            }
            diff_lines.push(Line::from(Span::raw("")));
            line_hunks.push(Some(index));
        }
    }

    let diff_scroll = diff_scroll_offset(app, sections[1], &diff_lines);
    let diff = Paragraph::new(diff_lines)
        .scroll((diff_scroll, 0))
        .wrap(Wrap { trim: false });
    frame.render_widget(diff, sections[1]);

    if app.review.focus == ReviewFocus::Hunks {
        if let Some(Some(hunk_index)) = line_hunks.get(app.review.cursor_line) {
            if *hunk_index != app.review.cursor_hunk {
                // draw-time sync only reflects current position; input handlers keep state authoritative
            }
        }
    }
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
    let preferred_top = app.review.cursor_line.saturating_sub(visible_height.saturating_sub(3));
    preferred_top.min(max_scroll).min(u16::MAX as usize) as u16
}

fn draw_composer(frame: &mut ratatui::Frame, area: Rect, app: &App, composer: &TextArea<'_>) {
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

    let block = Block::default()
        .title("New Prompt")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(styles::BORDER_MUTED))
        .style(Style::default().bg(styles::SURFACE_RAISED));
    frame.render_widget(block, modal);
    frame.render_widget(
        Paragraph::new(app.current_model_label()).style(styles::title()),
        lines[0],
    );
    frame.render_widget(
        Paragraph::new(vec![Line::from(vec![
            Span::styled("Tab", styles::keybind()),
            Span::raw(" models   "),
            Span::styled("Ctrl+T", styles::keybind()),
            Span::raw(" variant   "),
            Span::styled("Esc", styles::keybind()),
            Span::raw(" close"),
        ])])
        .style(styles::muted()),
        lines[1],
    );
    frame.render_widget(composer, lines[2]);
    frame.render_widget(
        Paragraph::new(vec![Line::from(vec![
            Span::styled("Enter", styles::keybind()),
            Span::raw(" run   "),
            Span::styled("Up/Down", styles::keybind()),
            Span::raw(" move"),
        ])])
        .style(styles::muted()),
        lines[3],
    );
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

fn draw_model_picker(
    frame: &mut ratatui::Frame,
    area: Rect,
    app: &App,
    search: &TextArea<'_>,
    matcher: &SkimMatcherV2,
    model_cursor: usize,
) {
    let modal = centered_rect(70, 65, area);
    frame.render_widget(Clear, modal);
    let block = Block::default()
        .title("Choose Model")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(styles::BORDER_MUTED))
        .style(Style::default().bg(styles::SURFACE_RAISED));
    frame.render_widget(block, modal);

    let inner = modal.inner(ratatui::layout::Margin {
        horizontal: 1,
        vertical: 1,
    });
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(1),
        ])
        .split(inner);

    frame.render_widget(search, sections[0]);

    let models = filtered_models(&app.models, matcher, &search.lines().join("\n"));
    let mut rows = Vec::new();
    let mut selected_row = None;
    let mut last_provider = String::new();
    for (index, model) in models.iter().enumerate() {
        if model.provider != last_provider {
            if !last_provider.is_empty() {
                rows.push(ListItem::new(Line::from(Span::raw(""))));
            }
            rows.push(ListItem::new(Line::from(Span::styled(
                model.provider.to_uppercase(),
                styles::subtle(),
            ))));
            last_provider = model.provider.clone();
        }
        let row_index = rows.len();
        if index == model_cursor {
            selected_row = Some(row_index);
        }
        let style = if index == model_cursor {
            Style::default()
                .fg(styles::TEXT_PRIMARY)
                .bg(styles::ACCENT_DIM)
        } else {
            Style::default().fg(styles::TEXT_MUTED)
        };
        let mut spans = vec![Span::styled(
            sanitize_model_display(&model.name),
            style.add_modifier(Modifier::BOLD),
        )];
        spans.push(Span::raw("  "));
        spans.push(Span::styled(model.id.clone(), styles::subtle()));
        let display_variants = model
            .variants
            .iter()
            .map(|variant| sanitize_variant_display(variant))
            .filter(|variant| !variant.is_empty())
            .collect::<Vec<_>>();
        if !display_variants.is_empty() {
            spans.push(Span::raw("  "));
            spans.push(Span::styled(display_variants.join(", "), styles::subtle()));
        }
        if Some(&model.id) == app.selected_model.as_ref() {
            spans.push(Span::raw("  "));
            spans.push(Span::styled(
                "selected",
                Style::default()
                    .fg(styles::BASE_BG)
                    .bg(styles::SUCCESS)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        rows.push(ListItem::new(Line::from(spans)));
    }

    let mut model_list_state = ListState::default().with_selected(selected_row);
    frame.render_stateful_widget(List::new(rows), sections[1], &mut model_list_state);
    frame.render_widget(
        Paragraph::new(vec![Line::from(vec![
            Span::styled("Enter", styles::keybind()),
            Span::raw(" select   "),
            Span::styled("Esc", styles::keybind()),
            Span::raw(" back"),
        ])])
        .style(styles::muted()),
        sections[2],
    );
}

fn filtered_models<'a>(
    models: &'a [ModelOption],
    matcher: &SkimMatcherV2,
    query: &str,
) -> Vec<&'a ModelOption> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return models.iter().collect();
    }

    let mut scored = models
        .iter()
        .filter_map(|model| {
            matcher
                .fuzzy_match(
                    &format!("{} {} {}", model.provider, model.name, model.id),
                    trimmed,
                )
                .map(|score| (score, model))
        })
        .collect::<Vec<_>>();
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then(a.1.provider.cmp(&b.1.provider))
            .then(a.1.name.cmp(&b.1.name))
    });
    scored.into_iter().map(|(_, model)| model).collect()
}

fn sanitize_model_display(value: &str) -> String {
    value
        .replace("-thinking", "")
        .replace(" thinking", "")
        .trim()
        .to_string()
}

fn sanitize_variant_display(value: &str) -> String {
    if value.eq_ignore_ascii_case("thinking") {
        String::new()
    } else {
        sanitize_model_display(value)
    }
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

fn cycle_variant(app: &mut App, direction: isize) {
    let variants = app.selected_model_variants();
    if variants.is_empty() {
        app.selected_variant = None;
        return;
    }

    let current_index = app
        .selected_variant
        .as_ref()
        .and_then(|variant| variants.iter().position(|candidate| candidate == variant))
        .unwrap_or(0) as isize;
    let next_index = (current_index + direction).rem_euclid(variants.len() as isize) as usize;
    app.selected_variant = variants.get(next_index).cloned();
}

fn render_brand_lockup(
    frame: &mut ratatui::Frame,
    area: Rect,
    app: &App,
    alignment: Alignment,
) {
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
        AnimatedTextStyle::pulse(styles::SUCCESS, styles::ACCENT_BRIGHT)
            .modifiers(Modifier::BOLD)
    } else {
        AnimatedTextStyle::pulse(styles::ACCENT, styles::ACCENT_BRIGHT)
            .modifiers(Modifier::BOLD)
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
