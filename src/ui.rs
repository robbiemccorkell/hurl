use crate::app::{
    AppState, Pane, RequestField, ResponseView, Screen, SettingsFocus, SyncSettingsField,
};
use crate::model::ResponseTrace;
use crossterm::cursor::{Hide, Show};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use std::io::{self, Stdout};
use std::time::{SystemTime, UNIX_EPOCH};

use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};

pub type AppTerminal = Terminal<CrosstermBackend<Stdout>>;

const ACCENT_BLUE: Color = Color::Rgb(108, 174, 255);
const ACCENT_BLUE_BG: Color = Color::Rgb(32, 58, 88);
const ACCENT_AMBER: Color = Color::Rgb(233, 181, 88);
const ACCENT_GREEN: Color = Color::Rgb(124, 196, 162);
const ACCENT_RED: Color = Color::Rgb(227, 112, 112);
const ACCENT_ORANGE: Color = Color::Rgb(240, 143, 82);
const ACCENT_CYAN: Color = Color::Rgb(95, 210, 255);
const TEXT_MUTED: Color = Color::Rgb(118, 132, 156);
const BORDER_MUTED: Color = Color::Rgb(72, 86, 108);
const SELECT_FG: Color = Color::Rgb(236, 243, 255);

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
                .fg(ACCENT_BLUE)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" | "),
        Span::styled("Sync:", Style::default().fg(TEXT_MUTED)),
        Span::raw(" "),
        Span::styled(
            app.sync_status_label(),
            sync_status_style(app.sync_status_label()),
        ),
    ];

    for control in controls_for_screen(app.screen) {
        spans.push(Span::raw(" | "));
        spans.push(Span::styled(*control, Style::default().fg(TEXT_MUTED)));
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
        .constraints([Constraint::Percentage(52), Constraint::Percentage(48)])
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
    let block = pane_block("Settings", app.settings.focus == SettingsFocus::Nav);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let items = vec![ListItem::new(app.settings.section.label())];
    let list = List::new(items)
        .highlight_style(Style::default().fg(SELECT_FG).bg(ACCENT_BLUE_BG))
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
    let block = pane_block(
        "Sync Status",
        app.settings.focus == SettingsFocus::Detail && !app.settings.editing,
    );
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
    let block = pane_block("Sync", app.settings.focus == SettingsFocus::Detail);
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
        Style::default().fg(if is_editing {
            ACCENT_AMBER
        } else {
            ACCENT_BLUE
        })
    } else {
        Style::default()
    };
    frame.render_widget(Paragraph::new(text).style(style), inner);
}

fn render_settings_help(frame: &mut Frame<'_>, area: Rect, app: &AppState) {
    let block = pane_block("Help", false);
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
    let block = pane_block("Library", app.focus == Pane::Library);
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
        .highlight_style(Style::default().fg(SELECT_FG).bg(ACCENT_BLUE_BG))
        .highlight_symbol("> ");

    let mut state = ListState::default();
    state.select(app.selected_index);
    frame.render_stateful_widget(list, inner, &mut state);
}

fn render_request_editor(frame: &mut Frame<'_>, area: Rect, app: &AppState) {
    let block = pane_block("Request", app.focus == Pane::Request);
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
        Style::default().fg(if app.request_editing {
            ACCENT_AMBER
        } else {
            ACCENT_BLUE
        })
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
    let block = pane_block("Response", app.focus == Pane::Response);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    render_response_tabs(frame, rows[0], app);
    match app.response_view {
        ResponseView::Trace => render_trace_view(frame, rows[1], app),
        ResponseView::Body => render_response_text_view(
            frame,
            rows[1],
            app,
            app.response
                .as_ref()
                .map(|response| response.body_text())
                .unwrap_or_else(|| {
                    if app.request_in_flight {
                        "Waiting for the response body...".to_string()
                    } else {
                        "Submit a request to see the response body here.".to_string()
                    }
                }),
        ),
        ResponseView::Headers => render_response_text_view(
            frame,
            rows[1],
            app,
            app.response
                .as_ref()
                .map(|response| response.headers_text())
                .unwrap_or_else(|| {
                    if app.request_in_flight {
                        "Waiting for response headers...".to_string()
                    } else {
                        "Submit a request to inspect response headers here.".to_string()
                    }
                }),
        ),
    }
}

fn render_response_tabs(frame: &mut Frame<'_>, area: Rect, app: &AppState) {
    let mut spans = Vec::new();
    for view in [
        ResponseView::Trace,
        ResponseView::Body,
        ResponseView::Headers,
    ] {
        if !spans.is_empty() {
            spans.push(Span::raw(" "));
        }
        let active = app.response_view == view;
        spans.push(Span::styled(
            format!(" {} ", view.label()),
            if active {
                Style::default()
                    .fg(SELECT_FG)
                    .bg(ACCENT_BLUE_BG)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(TEXT_MUTED)
            },
        ));
    }
    spans.push(Span::raw("  "));
    spans.push(Span::styled("← → switch", Style::default().fg(TEXT_MUTED)));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_response_text_view(frame: &mut Frame<'_>, area: Rect, app: &AppState, text: String) {
    let response = Paragraph::new(text)
        .wrap(Wrap { trim: false })
        .scroll((app.response_scroll, 0));
    frame.render_widget(response, area);
}

fn render_trace_view(frame: &mut Frame<'_>, area: Rect, app: &AppState) {
    let Some(trace) = app.trace.as_ref() else {
        let placeholder = if app.request_in_flight {
            format!("{} Establishing connection...", spinner_frame())
        } else {
            "Submit a request to see the live waterfall here.".to_string()
        };
        frame.render_widget(Paragraph::new(placeholder), area);
        return;
    };

    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(7),
            Constraint::Min(4),
        ])
        .split(area);

    render_trace_summary(frame, sections[0], app, trace);
    render_trace_waterfall(frame, sections[1], trace);
    render_trace_scope(frame, sections[2], app, trace);
}

fn render_trace_summary(frame: &mut Frame<'_>, area: Rect, app: &AppState, trace: &ResponseTrace) {
    let elapsed_ms = trace
        .total_time_ms()
        .max(trace.samples.last().map(|sample| sample.at_ms).unwrap_or(0));
    let status_label = trace
        .status_code
        .map(|status| {
            if let Some(reason) = trace.reason.as_deref() {
                format!("{status} {reason}")
            } else {
                status.to_string()
            }
        })
        .unwrap_or_else(|| format!("{} In flight", spinner_frame()));
    let state_style = match trace.state {
        crate::model::TraceState::Pending => Style::default().fg(ACCENT_AMBER),
        crate::model::TraceState::Receiving => Style::default().fg(ACCENT_BLUE),
        crate::model::TraceState::Complete => Style::default().fg(ACCENT_GREEN),
        crate::model::TraceState::Failed => Style::default().fg(ACCENT_RED),
    };
    let response_size = app
        .response
        .as_ref()
        .map(|response| response.body_bytes as u64)
        .unwrap_or(trace.downloaded_bytes);
    let content_type = app
        .response
        .as_ref()
        .and_then(|response| response.content_type.as_deref())
        .unwrap_or("pending");

    let lines = vec![
        Line::from(vec![
            Span::styled(status_label, status_style(trace.status_code)),
            Span::raw("  "),
            Span::styled(format!("{elapsed_ms} ms"), Style::default().fg(ACCENT_CYAN)),
            Span::raw("  "),
            Span::styled(content_type, Style::default().fg(TEXT_MUTED)),
        ]),
        Line::from(vec![
            Span::styled(
                trace.label.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(trace_state_label(trace), state_style),
            Span::raw("  "),
            Span::styled(
                format!("down {}", format_bytes(response_size)),
                Style::default().fg(ACCENT_GREEN),
            ),
            Span::raw("  "),
            Span::styled(
                format!("up {}", format_bytes(trace.uploaded_bytes)),
                Style::default().fg(ACCENT_ORANGE),
            ),
        ]),
    ];

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

fn render_trace_waterfall(frame: &mut Frame<'_>, area: Rect, trace: &ResponseTrace) {
    if trace.total_time_ms() == 0 {
        let placeholder = Paragraph::new(vec![
            Line::from(vec![
                Span::styled("timeline", Style::default().fg(TEXT_MUTED)),
                Span::raw("  "),
                Span::styled(
                    "waiting for timing marks...",
                    Style::default().fg(ACCENT_AMBER),
                ),
            ]),
            Line::from(Span::styled(
                live_pulse(area.width.saturating_sub(2) as usize),
                Style::default().fg(BORDER_MUTED),
            )),
        ]);
        frame.render_widget(placeholder, area);
        return;
    }

    let phases = trace.waterfall_phases();
    let total_time = trace.total_time_ms().max(1);
    let prefix_width = 31usize;
    let bar_width = area
        .width
        .saturating_sub(prefix_width as u16)
        .saturating_sub(1) as usize;
    let show_scale = area.height as usize >= phases.len() + 2;
    let header_lines = if show_scale { 2 } else { 1 };
    let visible_rows = area.height.saturating_sub(header_lines as u16) as usize;

    let mut lines = vec![Line::from(vec![
        Span::styled("waterfall", Style::default().fg(TEXT_MUTED)),
        Span::raw("  "),
        Span::styled(
            format!("total {total_time} ms"),
            Style::default().fg(ACCENT_CYAN),
        ),
        Span::raw("  "),
        Span::styled(
            "left=start  width=duration",
            Style::default().fg(TEXT_MUTED),
        ),
    ])];
    if show_scale {
        lines.push(timeline_scale_line(prefix_width, bar_width, total_time));
    }

    for phase in phases.iter().take(visible_rows) {
        let start = if bar_width == 0 {
            0
        } else {
            ((phase.start_ms * bar_width as u128) / total_time) as usize
        };
        let raw_end = if bar_width == 0 {
            0
        } else {
            ((phase.end_ms * bar_width as u128) / total_time) as usize
        };
        let has_duration = phase.duration_ms() > 0;
        let end = if has_duration {
            raw_end.max(start.saturating_add(1)).min(bar_width)
        } else {
            start.min(bar_width.saturating_sub(1))
        };
        let bar_len = if has_duration {
            end.saturating_sub(start).max(1)
        } else {
            1
        };
        let label = format!(
            "{:<11} {:>3}-{:>3} {:>3}ms",
            phase_display_label(phase.label),
            phase.start_ms,
            phase.end_ms,
            phase.duration_ms()
        );
        let row_style = if has_duration {
            Style::default().fg(TEXT_MUTED)
        } else {
            Style::default().fg(BORDER_MUTED)
        };
        let mut spans = vec![Span::styled(pad_right(&label, prefix_width), row_style)];
        if start > 0 {
            spans.push(Span::styled(
                "·".repeat(start),
                Style::default().fg(BORDER_MUTED),
            ));
        }
        spans.push(Span::styled(
            "█".repeat(bar_len.max(1)),
            Style::default()
                .fg(if has_duration {
                    phase_color(phase.label)
                } else {
                    BORDER_MUTED
                })
                .add_modifier(if has_duration {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        ));
        if end < bar_width {
            spans.push(Span::styled(
                "·".repeat(bar_width - end),
                Style::default().fg(BORDER_MUTED),
            ));
        }
        lines.push(Line::from(spans));
    }

    if phases.len() > visible_rows {
        lines.push(Line::from(Span::styled(
            format!("+{} more phase row(s)", phases.len() - visible_rows),
            Style::default().fg(TEXT_MUTED),
        )));
    }

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

fn render_trace_scope(frame: &mut Frame<'_>, area: Rect, app: &AppState, trace: &ResponseTrace) {
    let width = area.width.saturating_sub(8) as usize;
    let download_line = if trace.samples.is_empty() {
        live_pulse(width)
    } else {
        sparkline(width, trace, true)
    };
    let upload_line = if trace.samples.is_empty() {
        "·".repeat(width)
    } else {
        sparkline(width, trace, false)
    };
    let lines = vec![
        Line::from(vec![
            Span::styled("scope", Style::default().fg(TEXT_MUTED)),
            Span::raw("  "),
            Span::styled(
                if app.request_in_flight {
                    "live transfer"
                } else {
                    "frozen trace"
                },
                Style::default().fg(ACCENT_BLUE),
            ),
        ]),
        Line::from(vec![
            Span::styled("DL ", Style::default().fg(ACCENT_GREEN)),
            Span::styled(download_line, Style::default().fg(ACCENT_GREEN)),
            Span::raw(" "),
            Span::styled(
                format_speed(trace.download_speed_bytes_per_sec),
                Style::default().fg(TEXT_MUTED),
            ),
        ]),
        Line::from(vec![
            Span::styled("UL ", Style::default().fg(ACCENT_ORANGE)),
            Span::styled(upload_line, Style::default().fg(ACCENT_ORANGE)),
            Span::raw(" "),
            Span::styled(
                format_speed(trace.upload_speed_bytes_per_sec),
                Style::default().fg(TEXT_MUTED),
            ),
        ]),
    ];
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

fn spinner_frame() -> &'static str {
    let frames = ["|", "/", "-", "\\"];
    let tick = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() / 120)
        .unwrap_or(0);
    frames[(tick as usize) % frames.len()]
}

fn trace_state_label(trace: &ResponseTrace) -> &'static str {
    match trace.state {
        crate::model::TraceState::Pending => "pending",
        crate::model::TraceState::Receiving => "receiving",
        crate::model::TraceState::Complete => "complete",
        crate::model::TraceState::Failed => "failed",
    }
}

fn status_style(status_code: Option<u16>) -> Style {
    match status_code {
        Some(code) if code < 300 => Style::default()
            .fg(ACCENT_GREEN)
            .add_modifier(Modifier::BOLD),
        Some(code) if code < 400 => Style::default()
            .fg(ACCENT_BLUE)
            .add_modifier(Modifier::BOLD),
        Some(code) if code < 500 => Style::default()
            .fg(ACCENT_AMBER)
            .add_modifier(Modifier::BOLD),
        Some(_) => Style::default().fg(ACCENT_RED).add_modifier(Modifier::BOLD),
        None => Style::default()
            .fg(ACCENT_AMBER)
            .add_modifier(Modifier::BOLD),
    }
}

fn phase_color(label: &str) -> Color {
    match label {
        "Redirect" => ACCENT_RED,
        "DNS" => ACCENT_AMBER,
        "TCP" => ACCENT_BLUE,
        "TLS" => ACCENT_CYAN,
        "Wait" => ACCENT_ORANGE,
        "Recv" => ACCENT_GREEN,
        _ => TEXT_MUTED,
    }
}

fn phase_display_label(label: &str) -> &'static str {
    match label {
        "Redirect" => "redirect",
        "DNS" => "dns",
        "TCP" => "tcp",
        "TLS" => "tls",
        "Wait" => "ttfb wait",
        "Recv" => "download",
        _ => "phase",
    }
}

fn timeline_scale_line(prefix_width: usize, bar_width: usize, total_time: u128) -> Line<'static> {
    if bar_width == 0 {
        return Line::from(Span::styled(
            format!("{}0 ms", " ".repeat(prefix_width)),
            Style::default().fg(TEXT_MUTED),
        ));
    }

    let mut ruler = vec!['·'; bar_width];
    for tick in [0usize, bar_width / 2, bar_width.saturating_sub(1)] {
        if let Some(cell) = ruler.get_mut(tick) {
            *cell = '┼';
        }
    }
    let middle_ms = total_time / 2;
    let scale = format!("0 ms  {}  {} ms", middle_ms, total_time);
    Line::from(vec![
        Span::styled(" ".repeat(prefix_width), Style::default()),
        Span::styled(
            ruler.into_iter().collect::<String>(),
            Style::default().fg(BORDER_MUTED),
        ),
        Span::raw(" "),
        Span::styled(scale, Style::default().fg(TEXT_MUTED)),
    ])
}

fn pad_right(value: &str, width: usize) -> String {
    format!("{value:<width$}")
}

fn sparkline(width: usize, trace: &ResponseTrace, download: bool) -> String {
    if width == 0 {
        return String::new();
    }
    if trace.samples.is_empty() {
        return "·".repeat(width);
    }

    let mut buckets = vec![0_u64; width];
    for (index, sample) in trace.samples.iter().enumerate() {
        let bucket = index * width / trace.samples.len();
        let value = if download {
            sample.download_speed_bytes_per_sec
        } else {
            sample.upload_speed_bytes_per_sec
        };
        buckets[bucket] = buckets[bucket].max(value);
    }

    let peak = buckets.iter().copied().max().unwrap_or(0).max(1);
    let levels = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    buckets
        .into_iter()
        .map(|value| {
            let scaled = ((value as f64 / peak as f64) * (levels.len() as f64 - 1.0)).round();
            levels[scaled as usize]
        })
        .collect()
}

fn live_pulse(width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let position = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| (duration.as_millis() / 90) as usize % width.max(1))
        .unwrap_or(0);
    (0..width)
        .map(|index| if index == position { '█' } else { '·' })
        .collect()
}

fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    let bytes = bytes as f64;
    if bytes >= MIB {
        format!("{:.1} MiB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes / KIB)
    } else {
        format!("{} B", bytes as u64)
    }
}

fn format_speed(bytes_per_sec: u64) -> String {
    format!("{}/s", format_bytes(bytes_per_sec))
}

fn pane_style(is_focused: bool) -> Style {
    if is_focused {
        Style::default().fg(ACCENT_BLUE)
    } else {
        Style::default().fg(BORDER_MUTED)
    }
}

fn field_block<'a>(title: &'a str, is_selected: bool, is_editing: bool) -> Block<'a> {
    let border_style = if is_editing {
        Style::default().fg(ACCENT_AMBER)
    } else if is_selected {
        Style::default().fg(ACCENT_BLUE)
    } else {
        Style::default().fg(BORDER_MUTED)
    };

    let title_style = if is_editing {
        Style::default()
            .fg(ACCENT_AMBER)
            .add_modifier(Modifier::BOLD)
    } else if is_selected {
        Style::default()
            .fg(ACCENT_BLUE)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(TEXT_MUTED)
    };

    Block::default()
        .title(Line::from(Span::styled(title, title_style)))
        .borders(Borders::ALL)
        .border_style(border_style)
}

fn pane_block<'a>(title: &'a str, is_focused: bool) -> Block<'a> {
    let title_style = if is_focused {
        Style::default()
            .fg(ACCENT_BLUE)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(TEXT_MUTED)
    };

    Block::default()
        .title(Line::from(Span::styled(title, title_style)))
        .borders(Borders::ALL)
        .border_style(pane_style(is_focused))
}

fn sync_status_style(status: &str) -> Style {
    match status {
        "Ready" => Style::default()
            .fg(ACCENT_GREEN)
            .add_modifier(Modifier::BOLD),
        "Syncing" => Style::default()
            .fg(ACCENT_BLUE)
            .add_modifier(Modifier::BOLD),
        "Dirty" => Style::default()
            .fg(ACCENT_AMBER)
            .add_modifier(Modifier::BOLD),
        "Error" => Style::default().fg(ACCENT_RED).add_modifier(Modifier::BOLD),
        _ => Style::default().fg(TEXT_MUTED),
    }
}

fn controls_for_screen(screen: Screen) -> &'static [&'static str] {
    match screen {
        Screen::Main => &[
            "g Settings",
            "Tab Panes",
            "←→ Trace/Body/Headers",
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
