use crate::app::{AppState, Pane, RequestField, Screen, SettingsFocus, SyncSettingsField};
use crossterm::cursor::{Hide, Show};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use std::io::{self, Stdout};

use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};

pub type AppTerminal = Terminal<CrosstermBackend<Stdout>>;

pub fn setup_terminal() -> io::Result<AppTerminal> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableBracketedPaste, Hide)?;
    Terminal::new(CrosstermBackend::new(stdout))
}

pub fn restore_terminal() -> io::Result<()> {
    disable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, DisableBracketedPaste, LeaveAlternateScreen, Show)?;
    Ok(())
}

pub fn draw(frame: &mut Frame<'_>, app: &AppState) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(frame.area());

    render_top_bar(frame, layout[0], app);
    match app.screen {
        Screen::Main => render_main_screen(frame, layout[1], app),
        Screen::Settings => render_settings_screen(frame, layout[1], app),
    }
}

fn render_top_bar(frame: &mut Frame<'_>, area: Rect, app: &AppState) {
    let mut spans = vec![
        Span::styled(
            "hurl",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" | "),
        Span::raw(format!("Sync: {}", app.sync_status_label())),
    ];

    for control in controls_for_screen(app.screen) {
        spans.push(Span::raw(" | "));
        spans.push(Span::styled(*control, Style::default().fg(Color::DarkGray)));
    }

    let line = Line::from(spans);

    frame.render_widget(Paragraph::new(line), area);
}

fn render_main_screen(frame: &mut Frame<'_>, area: Rect, app: &AppState) {
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(area);

    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(56), Constraint::Percentage(44)])
        .split(body[1]);

    render_library(frame, body[0], app);
    render_request_editor(frame, right[0], app);
    render_response(frame, right[1], app);
}

fn render_settings_screen(frame: &mut Frame<'_>, area: Rect, app: &AppState) {
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(24), Constraint::Min(0)])
        .split(area);

    render_settings_nav(frame, body[0], app);
    render_settings_detail(frame, body[1], app);
}

fn render_settings_nav(frame: &mut Frame<'_>, area: Rect, app: &AppState) {
    let block = Block::default()
        .title("Settings")
        .borders(Borders::ALL)
        .border_style(pane_style(app.settings.focus == SettingsFocus::Nav));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let items = vec![ListItem::new(app.settings.section.label())];
    let list = List::new(items)
        .highlight_style(Style::default().fg(Color::Black).bg(Color::Cyan))
        .highlight_symbol("> ");

    let mut state = ListState::default();
    state.select(Some(0));
    frame.render_stateful_widget(list, inner, &mut state);
}

fn render_settings_detail(frame: &mut Frame<'_>, area: Rect, app: &AppState) {
    let detail = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(7),
            Constraint::Min(12),
            Constraint::Length(6),
        ])
        .split(area);

    render_sync_summary(frame, detail[0], app);
    render_sync_fields(frame, detail[1], app);
    render_settings_help(frame, detail[2], app);
}

fn render_sync_summary(frame: &mut Frame<'_>, area: Rect, app: &AppState) {
    let block = Block::default()
        .title("Sync Status")
        .borders(Borders::ALL)
        .border_style(pane_style(
            app.settings.focus == SettingsFocus::Detail && !app.settings.editing,
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let lines = app
        .sync_summary_lines()
        .into_iter()
        .map(Line::from)
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn render_sync_fields(frame: &mut Frame<'_>, area: Rect, app: &AppState) {
    let block = Block::default()
        .title("Sync")
        .borders(Borders::ALL)
        .border_style(pane_style(app.settings.focus == SettingsFocus::Detail));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let fields = app.visible_sync_fields();
    let mut constraints = fields
        .iter()
        .map(|_| Constraint::Length(3))
        .collect::<Vec<_>>();
    constraints.push(Constraint::Min(0));
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);

    for (index, field) in fields.iter().enumerate() {
        render_sync_field(frame, rows[index], app, *field);
    }
}

fn render_sync_field(frame: &mut Frame<'_>, area: Rect, app: &AppState, field: SyncSettingsField) {
    let is_selected =
        app.settings.focus == SettingsFocus::Detail && app.settings.sync_field == field;
    let is_editing = is_selected && app.settings.editing;
    let block = field_block(field.label(), is_selected, is_editing);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if is_editing {
        match field {
            SyncSettingsField::Owner => frame.render_widget(&app.sync_form.owner, inner),
            SyncSettingsField::Repo => frame.render_widget(&app.sync_form.repo, inner),
            SyncSettingsField::Password => frame.render_widget(&app.sync_form.password, inner),
            SyncSettingsField::ConfirmPassword => {
                frame.render_widget(&app.sync_form.confirm_password, inner)
            }
            _ => {}
        }
        return;
    }

    let text = app.masked_sync_value(field);
    let style = if is_selected {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };
    frame.render_widget(Paragraph::new(text).style(style), inner);
}

fn render_settings_help(frame: &mut Frame<'_>, area: Rect, app: &AppState) {
    let block = Block::default().title("Help").borders(Borders::ALL);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let lines = app
        .settings_help_lines()
        .into_iter()
        .map(Line::from)
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn render_library(frame: &mut Frame<'_>, area: Rect, app: &AppState) {
    let block = Block::default()
        .title("Library")
        .borders(Borders::ALL)
        .border_style(pane_style(app.focus == Pane::Library));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if app.library.is_empty() {
        let placeholder = Paragraph::new("No saved requests yet.\nPress `n` to create one.")
            .wrap(Wrap { trim: false });
        frame.render_widget(placeholder, inner);
        return;
    }

    let items = app
        .library
        .iter()
        .map(|request| ListItem::new(request.display_name()))
        .collect::<Vec<_>>();

    let list = List::new(items)
        .highlight_style(Style::default().fg(Color::Black).bg(Color::Cyan))
        .highlight_symbol("> ");

    let mut state = ListState::default();
    state.select(app.selected_index);
    frame.render_stateful_widget(list, inner, &mut state);
}

fn render_request_editor(frame: &mut Frame<'_>, area: Rect, app: &AppState) {
    let block = Block::default()
        .title("Request")
        .borders(Borders::ALL)
        .border_style(pane_style(app.focus == Pane::Request));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(7),
            Constraint::Min(8),
        ])
        .split(inner);

    let method_url = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(16), Constraint::Min(20)])
        .split(rows[1]);

    render_text_field(
        frame,
        rows[0],
        "Title",
        app,
        RequestField::Title,
        &app.draft.title_text(),
        Some(&app.draft.title),
    );

    render_method_field(frame, method_url[0], app);
    render_text_field(
        frame,
        method_url[1],
        "URL",
        app,
        RequestField::Url,
        &app.draft.url_text(),
        Some(&app.draft.url),
    );
    render_text_field(
        frame,
        rows[2],
        "Headers",
        app,
        RequestField::Headers,
        &app.draft.headers_text(),
        Some(&app.draft.headers),
    );
    render_text_field(
        frame,
        rows[3],
        "JSON Body",
        app,
        RequestField::Body,
        &app.draft.body_text(),
        Some(&app.draft.body),
    );
}

fn render_method_field(frame: &mut Frame<'_>, area: Rect, app: &AppState) {
    let is_selected = app.focus == Pane::Request && app.request_field == RequestField::Method;
    let block = field_block("Method", is_selected, is_selected && app.request_editing);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let hint = if is_selected && app.request_editing {
        format!("{}  <- ->", app.draft.method)
    } else {
        app.draft.method.to_string()
    };

    let style = if is_selected {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };

    frame.render_widget(Paragraph::new(hint).style(style), inner);
}

fn render_text_field(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    app: &AppState,
    field: RequestField,
    text: &str,
    textarea: Option<&tui_textarea::TextArea<'static>>,
) {
    let is_selected = app.focus == Pane::Request && app.request_field == field;
    let is_editing = is_selected && app.request_editing;
    let block = field_block(title, is_selected, is_editing);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if is_editing {
        if let Some(textarea) = textarea {
            frame.render_widget(textarea, inner);
        }
        return;
    }

    let paragraph = Paragraph::new(text.to_string()).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, inner);
}

fn render_response(frame: &mut Frame<'_>, area: Rect, app: &AppState) {
    let block = Block::default()
        .title("Response")
        .borders(Borders::ALL)
        .border_style(pane_style(app.focus == Pane::Response));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let text = match &app.response {
        Some(response) => {
            let mut output = response.display_text();
            if let Some(suffix) = response.body.detail_suffix() {
                output.push_str(&suffix);
            }
            output
        }
        None => "Submit a request to see the response here.".to_string(),
    };

    let response = Paragraph::new(text)
        .wrap(Wrap { trim: false })
        .scroll((app.response_scroll, 0));
    frame.render_widget(response, inner);
}

fn pane_style(is_focused: bool) -> Style {
    if is_focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    }
}

fn field_block<'a>(title: &'a str, is_selected: bool, is_editing: bool) -> Block<'a> {
    let border_style = if is_editing {
        Style::default().fg(Color::Yellow)
    } else if is_selected {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default()
    };

    Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(border_style)
}

fn controls_for_screen(screen: Screen) -> &'static [&'static str] {
    match screen {
        Screen::Main => &[
            "g Settings",
            "Tab Panes",
            "Enter Edit/Load",
            "Ctrl+V Paste",
            "Ctrl+S Save",
            "Ctrl+R Send",
            "n New",
            "q Quit",
        ],
        Screen::Settings => &[
            "Tab Switch",
            "Enter Edit/Run",
            "Esc Back",
            "g Close",
            "q Quit",
        ],
    }
}
