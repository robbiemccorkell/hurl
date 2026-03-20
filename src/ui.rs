use crate::app::{AppState, Pane, RequestField, StatusTone};
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

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(layout[1]);

    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(56), Constraint::Percentage(44)])
        .split(body[1]);

    render_library(frame, body[0], app);
    render_request_editor(frame, right[0], app);
    render_response(frame, right[1], app);
}

fn render_top_bar(frame: &mut Frame<'_>, area: Rect, app: &AppState) {
    let focus = match app.focus {
        Pane::Library => "Library",
        Pane::Request => "Request",
        Pane::Response => "Response",
    };

    let field = match app.request_field {
        RequestField::Title => "Title",
        RequestField::Method => "Method",
        RequestField::Url => "URL",
        RequestField::Headers => "Headers",
        RequestField::Body => "Body",
    };

    let tone_style = match app.status.tone {
        StatusTone::Info => Style::default().fg(Color::Gray),
        StatusTone::Success => Style::default().fg(Color::Green),
        StatusTone::Error => Style::default().fg(Color::Red),
    };

    let line = Line::from(vec![
        Span::styled(
            "hurl",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" | "),
        Span::raw(format!("Focus: {focus}")),
        Span::raw(" | "),
        Span::raw(format!("Field: {field}")),
        Span::raw(" | "),
        Span::styled(app.status.message.as_str(), tone_style),
        Span::raw(" | Tab panes Enter edit/load Ctrl+V paste Ctrl+S save Ctrl+R send n new q quit"),
    ]);

    frame.render_widget(Paragraph::new(line), area);
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
