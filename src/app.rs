use std::path::PathBuf;
use std::time::Duration;

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

use crate::domain::diff::{DiffLineKind, FileDiff, ReviewStatus};
use crate::services::git::GitService;
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum Overlay {
    None,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ReviewFocus {
    #[default]
    Files,
    Hunks,
}

enum Message {
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
        let (tx, rx) = mpsc::unbounded_channel();
        let mut app = Self {
            repo_path,
            git,
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
                        if app.review_busy {
                            app.status =
                                "Wait for the current review update to finish.".to_string();
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
        KeyCode::Char('u') => {
            if let Some(file) = app.review.files.get_mut(app.review.cursor_file) {
                let result = app.git.unstage_file_in_place(file).await;

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

    let action_line = Line::from(vec![
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
        .constraints([Constraint::Length(28), Constraint::Min(30)])
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
    let mut line_hunks = vec![None];

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
            line_hunks.push(Some(index));
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
                line_hunks.push(Some(index));
            }
            diff_lines.push(Line::from(Span::raw("")));
            line_hunks.push(Some(index));
        }
    }

    let diff_scroll = diff_scroll_offset(app, sections[1], &diff_lines);
    let diff = Paragraph::new(diff_lines).scroll((diff_scroll, 0));
    frame.render_widget(diff, sections[1]);
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
        let mut app = App {
            repo_path: PathBuf::from("."),
            git: GitService::new("."),
            status: String::new(),
            screen: Screen::Home,
            review: ReviewUiState {
                files: vec![
                    sample_file(),
                    FileDiff {
                        new_path: "README.md".to_string(),
                        review_status: ReviewStatus::Rejected,
                        ..FileDiff::default()
                    },
                ],
                ..ReviewUiState::default()
            },
            overlay: Overlay::None,
            had_staged_changes_on_open: false,
            review_busy: false,
            logo_animation: AnimatedTextState::with_interval(120),
            tx: mpsc::unbounded_channel().0,
            rx: mpsc::unbounded_channel().1,
        };

        let counts = app.review_counts();
        assert_eq!(counts.unreviewed, 1);
        assert_eq!(counts.accepted, 1);
        assert_eq!(counts.rejected, 1);

        app.review.files[0].set_all_hunks_status(ReviewStatus::Accepted);
        let counts = app.review_counts();
        assert_eq!(counts.unreviewed, 0);
        assert_eq!(counts.accepted, 2);
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
        let (tx, rx) = mpsc::unbounded_channel();
        let mut app = App {
            repo_path: PathBuf::from("."),
            git: GitService::new("."),
            status: String::new(),
            screen: Screen::Review,
            review: ReviewUiState {
                files: vec![sample_file()],
                cursor_file: 0,
                cursor_hunk: 0,
                cursor_line: 1,
                focus: ReviewFocus::Hunks,
            },
            overlay: Overlay::None,
            had_staged_changes_on_open: false,
            review_busy: false,
            logo_animation: AnimatedTextState::with_interval(120),
            tx,
            rx,
        };

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
        let (tx, rx) = mpsc::unbounded_channel();
        let app = App {
            repo_path: PathBuf::from("."),
            git: GitService::new("."),
            status: String::new(),
            screen: Screen::Review,
            review: ReviewUiState {
                files: vec![sample_file()],
                cursor_file: 0,
                cursor_hunk: 1,
                cursor_line: 6,
                focus: ReviewFocus::Hunks,
            },
            overlay: Overlay::None,
            had_staged_changes_on_open: false,
            review_busy: false,
            logo_animation: AnimatedTextState::with_interval(120),
            tx,
            rx,
        };

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
}
