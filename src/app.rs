use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use tokio::sync::mpsc;
use tui_textarea::TextArea;

use crate::domain::diff::{DiffLineKind, FileDiff, ReviewStatus};
use crate::domain::model_catalog::ModelOption;
use crate::services::git::{GitService, patch_from_hunk};
use crate::services::opencode::{OpencodeService, RunResult};
use crate::ui::styles;

pub async fn run() -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?; 

    let result = run_app(&mut terminal).await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

struct App {
    repo_path: PathBuf,
    git: GitService,
    opencode: OpencodeService,
    status: String,
    run_state: RunState,
    review: ReviewUiState,
    overlay: Overlay,
    models: Vec<ModelOption>,
    selected_model: Option<String>,
    selected_variant: Option<String>,
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
}

#[derive(Default)]
struct ReviewUiState {
    files: Vec<FileDiff>,
    cursor_file: usize,
    cursor_hunk: usize,
    focus: ReviewFocus,
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
            review: ReviewUiState::default(),
            overlay: Overlay::None,
            models: Vec::new(),
            selected_model: None,
            selected_variant: None,
            tx,
            rx,
        };
        app.load_initial_state().await?;
        Ok(app)
    }

    async fn load_initial_state(&mut self) -> Result<()> {
        let (_, files) = self.git.collect_diff().await?;
        self.review.files = files;

        let tx = self.tx.clone();
        let service = self.opencode.clone();
        tokio::spawn(async move {
            let result = service.load_models().await.map_err(|err| err.to_string());
            let _ = tx.send(Message::ModelsLoaded(result));
        });

        Ok(())
    }

    fn current_model_label(&self) -> String {
        match (&self.selected_model, &self.selected_variant) {
            (Some(model), Some(variant)) if !variant.is_empty() => format!("{model} [{variant}]"),
            (Some(model), _) => model.clone(),
            _ => "loading...".to_string(),
        }
    }

    fn selected_model_variants(&self) -> Vec<String> {
        self.models
            .iter()
            .find(|model| Some(&model.id) == self.selected_model.as_ref())
            .map(|model| model.variants.clone())
            .unwrap_or_default()
    }
}

async fn run_app(terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>) -> Result<()> {
    let mut app = App::new().await?;
    let matcher = SkimMatcherV2::default();
    let mut composer = TextArea::default();
    composer.set_placeholder_text("Describe the change you want opencode to make");
    let mut model_search = TextArea::default();
    model_search.set_placeholder_text("Search by provider or model");
    let mut model_cursor = 0_usize;

    loop {
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
                        app.review.focus = ReviewFocus::Files;
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
            }
        }

        terminal.draw(|frame| {
            draw(
                frame,
                &app,
                &composer,
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
                            if app.run_state == RunState::Running {
                                continue;
                            }
                            let prompt = composer.lines().join("\n").trim().to_string();
                            if prompt.is_empty() {
                                app.status = "Write a prompt first.".to_string();
                                continue;
                            }
                            app.run_state = RunState::Running;
                            app.status =
                                format!("Running opencode with {}...", app.current_model_label());
                            let tx = app.tx.clone();
                            let service = app.opencode.clone();
                            let git = app.git.clone();
                            let model = app.selected_model.clone();
                            let variant = app.selected_variant.clone();
                            composer = TextArea::default();
                            composer.set_placeholder_text(
                                "Describe the change you want opencode to make",
                            );
                            tokio::spawn(async move {
                                let result = service
                                    .run_prompt(&git, &prompt, model.as_deref(), variant.as_deref())
                                    .await
                                    .map_err(|err| err.to_string());
                                let _ = tx.send(Message::PromptFinished(result));
                            });
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
                    Overlay::None => {
                        if key.code == KeyCode::Char('o')
                            && key.modifiers.contains(KeyModifiers::CONTROL)
                        {
                            app.overlay = Overlay::Composer;
                            app.status = "Compose a new prompt.".to_string();
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
    if app.review.files.is_empty() {
        return Ok(());
    }

    match key.code {
        KeyCode::Enter => app.review.focus = ReviewFocus::Hunks,
        KeyCode::Esc => app.review.focus = ReviewFocus::Files,
        KeyCode::Up | KeyCode::Char('k') => {
            if app.review.focus == ReviewFocus::Files {
                app.review.cursor_file = app.review.cursor_file.saturating_sub(1);
                app.review.cursor_hunk = 0;
            } else {
                app.review.cursor_hunk = app.review.cursor_hunk.saturating_sub(1);
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if app.review.focus == ReviewFocus::Files {
                if app.review.cursor_file + 1 < app.review.files.len() {
                    app.review.cursor_file += 1;
                    app.review.cursor_hunk = 0;
                }
            } else if let Some(file) = app.review.files.get(app.review.cursor_file) {
                if app.review.cursor_hunk + 1 < file.hunks.len() {
                    app.review.cursor_hunk += 1;
                }
            }
        }
        KeyCode::Tab => {
            if app.review.focus == ReviewFocus::Hunks {
                if let Some(file) = app.review.files.get(app.review.cursor_file) {
                    if !file.hunks.is_empty() {
                        app.review.cursor_hunk = (app.review.cursor_hunk + 1) % file.hunks.len();
                    }
                }
            }
        }
        KeyCode::Char('y') => {
            if app.review.focus == ReviewFocus::Files {
                if let Some(file) = app.review.files.get_mut(app.review.cursor_file) {
                    app.git.accept_file(file).await?;
                    app.status = "Accepted file changes.".to_string();
                }
            } else if let Some(file) = app.review.files.get_mut(app.review.cursor_file) {
                if let Some(hunk) = file.hunks.get(app.review.cursor_hunk).cloned() {
                    let patch = patch_from_hunk(file, &hunk);
                    app.git.apply_patch_to_index(&patch).await?;

                    let hunk = &mut file.hunks[app.review.cursor_hunk];
                    hunk.review_status = ReviewStatus::Accepted;
                    file.sync_review_status();
                    app.status = "Accepted hunk.".to_string();
                }
            }
        }
        KeyCode::Char('x') => {
            if app.review.focus == ReviewFocus::Files {
                if let Some(file) = app.review.files.get_mut(app.review.cursor_file) {
                    app.git.reject_file(file).await?;
                    app.status = "Rejected file changes.".to_string();
                }
            } else if let Some(file) = app.review.files.get_mut(app.review.cursor_file) {
                if let Some(hunk) = file.hunks.get(app.review.cursor_hunk).cloned() {
                    let patch = patch_from_hunk(file, &hunk);
                    app.git.reverse_apply_patch(&patch).await?;

                    let hunk = &mut file.hunks[app.review.cursor_hunk];
                    hunk.review_status = ReviewStatus::Rejected;
                    file.sync_review_status();
                    app.status = "Rejected hunk.".to_string();
                }
            }
        }
        KeyCode::Char('u') => {
            if let Some(file) = app.review.files.get_mut(app.review.cursor_file) {
                app.git.unstage_file(file).await?;
                app.status = "Moved file back to unreviewed.".to_string();
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
    model_search: &TextArea<'_>,
    matcher: &SkimMatcherV2,
    model_cursor: usize,
) {
    let size = frame.area();
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(10),
            Constraint::Length(2),
        ])
        .split(size);

    draw_top_bar(frame, layout[0], app);
    draw_review(frame, layout[1], app);
    draw_footer(frame, layout[2]);

    match app.overlay {
        Overlay::Composer => draw_composer(frame, layout[1], app, composer),
        Overlay::ModelPicker => {
            draw_model_picker(frame, layout[1], app, model_search, matcher, model_cursor)
        }
        Overlay::None => {}
    }
}

fn draw_top_bar(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let status = match app.run_state {
        RunState::Ready => Span::styled(
            "READY",
            Style::default()
                .fg(styles::TEXT_PRIMARY)
                .bg(styles::SURFACE_RAISED),
        ),
        RunState::Running => Span::styled(
            "RUNNING",
            Style::default()
                .fg(styles::BASE_BG)
                .bg(styles::ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        RunState::Failed => Span::styled(
            "FAILED",
            Style::default()
                .fg(styles::TEXT_PRIMARY)
                .bg(styles::DANGER)
                .add_modifier(Modifier::BOLD),
        ),
    };

    let lines = vec![
        Line::from(vec![
            Span::styled("better-review", styles::title()),
            Span::raw("  "),
            Span::styled(
                app.repo_path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or("repo"),
                styles::muted(),
            ),
            Span::raw("    "),
            Span::styled("Model ", styles::muted()),
            Span::styled(app.current_model_label(), styles::title()),
            Span::raw("   "),
            status,
        ]),
        Line::from(Span::styled(app.status.as_str(), styles::muted())),
    ];

    let paragraph = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(styles::BORDER_MUTED)),
    );
    frame.render_widget(paragraph, area);
}

fn draw_footer(frame: &mut ratatui::Frame, area: Rect) {
    let line = Line::from(vec![
        Span::styled(
            "Ctrl+O composer  |  Enter open diff  |  Esc files  |  y accept  |  x reject",
            styles::subtle(),
        ),
        Span::raw("    "),
        Span::styled("Ctrl+C quit", styles::subtle()),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn draw_review(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    if app.review.files.is_empty() {
        let empty = Paragraph::new(vec![
            Line::from(Span::styled("No code changes yet", styles::title())),
            Line::from(Span::raw("")),
            Line::from(Span::styled(
                "The review workspace stays open at all times.",
                styles::muted(),
            )),
            Line::from(Span::styled(
                "Press Ctrl+O to open the composer and send a new instruction to opencode.",
                styles::muted(),
            )),
            Line::from(Span::styled(
                "Completed runs will replace this placeholder with a real diff.",
                styles::muted(),
            )),
        ])
        .block(
            Block::default()
                .title("Review")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(styles::BORDER_MUTED)),
        )
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true });
        frame.render_widget(empty, centered_rect(80, 45, area));
        return;
    }

    let sections = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(36), Constraint::Min(30)])
        .split(area);

    let items = app
        .review
        .files
        .iter()
        .enumerate()
        .map(|(index, file)| {
            let marker = match file.review_status {
                ReviewStatus::Accepted => "A",
                ReviewStatus::Rejected => "R",
                ReviewStatus::Unreviewed => match file.status {
                    crate::domain::diff::FileStatus::Added => "+",
                    crate::domain::diff::FileStatus::Deleted => "-",
                    crate::domain::diff::FileStatus::Modified => "M",
                },
            };
            let style = if index == app.review.cursor_file {
                Style::default()
                    .fg(styles::TEXT_PRIMARY)
                    .bg(styles::SURFACE_RAISED)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(styles::TEXT_MUTED)
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!(" {marker} "), style),
                Span::styled(truncate_path(file.display_path(), 28), style),
            ]))
        })
        .collect::<Vec<_>>();

    let sidebar = List::new(items).block(
        Block::default()
            .title("Files")
            .borders(Borders::RIGHT)
            .border_style(
                Style::default().fg(if app.review.focus == ReviewFocus::Files {
                    styles::ACCENT
                } else {
                    styles::BORDER_MUTED
                }),
            ),
    );
    frame.render_widget(sidebar, sections[0]);

    let mut diff_lines = vec![Line::from(vec![
        Span::styled(
            app.review.files[app.review.cursor_file].display_path(),
            styles::title(),
        ),
        Span::raw("  "),
        Span::styled(
            match app.review.focus {
                ReviewFocus::Files => "Reviewing files",
                ReviewFocus::Hunks => "Inspecting hunks",
            },
            styles::muted(),
        ),
    ])];

    if let Some(file) = app.review.files.get(app.review.cursor_file) {
        for (index, hunk) in file.hunks.iter().enumerate() {
            let mut style = Style::default()
                .fg(styles::TEXT_PRIMARY)
                .bg(styles::SURFACE_RAISED);
            if app.review.focus == ReviewFocus::Hunks && app.review.cursor_hunk == index {
                style = Style::default()
                    .fg(styles::BASE_BG)
                    .bg(styles::ACCENT)
                    .add_modifier(Modifier::BOLD);
            }

            let status = match hunk.review_status {
                ReviewStatus::Accepted => " [accepted]",
                ReviewStatus::Rejected => " [rejected]",
                ReviewStatus::Unreviewed => "",
            };

            diff_lines.push(Line::from(Span::styled(
                format!("{}{}", hunk.header, status),
                style,
            )));
            for line in &hunk.lines {
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

    let diff = Paragraph::new(diff_lines).wrap(Wrap { trim: false });
    frame.render_widget(diff, sections[1]);
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
        .border_style(Style::default().fg(styles::ACCENT))
        .style(Style::default().bg(styles::SURFACE_RAISED));
    frame.render_widget(block, modal);
    frame.render_widget(
        Paragraph::new(app.current_model_label()).style(styles::title()),
        lines[0],
    );
    frame.render_widget(
        Paragraph::new("Tab models   Ctrl+T variant   Esc close").style(styles::muted()),
        lines[1],
    );
    frame.render_widget(composer, lines[2]);
    frame.render_widget(Paragraph::new("Enter run").style(styles::muted()), lines[3]);
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
        .border_style(Style::default().fg(styles::ACCENT))
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
        let style = if index == model_cursor {
            Style::default()
                .fg(styles::TEXT_PRIMARY)
                .bg(styles::SURFACE)
        } else {
            Style::default().fg(styles::TEXT_MUTED)
        };
        let mut spans = vec![Span::styled(
            model.name.clone(),
            style.add_modifier(Modifier::BOLD),
        )];
        spans.push(Span::raw("  "));
        spans.push(Span::styled(model.id.clone(), styles::subtle()));
        if !model.variants.is_empty() {
            spans.push(Span::raw("  "));
            spans.push(Span::styled(model.variants.join(", "), styles::subtle()));
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

    frame.render_widget(List::new(rows), sections[1]);
    frame.render_widget(
        Paragraph::new("Enter select   Esc back").style(styles::muted()),
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

fn to_textarea_input(key: KeyEvent) -> tui_textarea::Input {
    tui_textarea::Input {
        key: match key.code {
            KeyCode::Backspace => tui_textarea::Key::Backspace,
            KeyCode::Enter => tui_textarea::Key::Enter,
            KeyCode::Left => tui_textarea::Key::Left,
            KeyCode::Right => tui_textarea::Key::Right,
            KeyCode::Up => tui_textarea::Key::Up,
            KeyCode::Down => tui_textarea::Key::Down,
            KeyCode::Home => tui_textarea::Key::Home,
            KeyCode::End => tui_textarea::Key::End,
            KeyCode::PageUp => tui_textarea::Key::PageUp,
            KeyCode::PageDown => tui_textarea::Key::PageDown,
            KeyCode::Delete => tui_textarea::Key::Delete,
            KeyCode::Char(ch) => tui_textarea::Key::Char(ch),
            KeyCode::Tab => tui_textarea::Key::Tab,
            _ => tui_textarea::Key::Null,
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
