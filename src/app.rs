use crate::events::{AppEvent, AppEventReceiver, AppEventSender, event_channel};
use crate::model::{
    HeaderEntry, HttpMethod, LibraryFile, RequestInput, ResponseData, SavedRequest,
    headers_to_text, parse_header_lines, validate_json_body, validate_url,
};
use crate::{network, storage, ui};
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::{Color, Style};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;
use tui_textarea::{Input, Key, TextArea};
use uuid::Uuid;

pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let path = storage::library_path()?;
    let library = storage::load_library(&path)?;
    let (sender, receiver) = event_channel();
    let mut app = AppState::new(path, library);
    let mut terminal = ui::setup_terminal()?;

    let result = run_loop(&mut terminal, &mut app, sender, receiver).await;
    ui::restore_terminal()?;
    result
}

async fn run_loop(
    terminal: &mut ui::AppTerminal,
    app: &mut AppState,
    sender: AppEventSender,
    mut receiver: AppEventReceiver,
) -> Result<(), Box<dyn std::error::Error>> {
    loop {
        while let Ok(event) = receiver.try_recv() {
            app.handle_app_event(event);
        }

        terminal.draw(|frame| ui::draw(frame, app))?;

        if app.should_quit {
            break;
        }

        if event::poll(Duration::from_millis(50))? {
            match event::read()? {
                Event::Key(key) => app.handle_key_event(key, &sender),
                Event::Resize(_, _) | Event::Mouse(_) | Event::FocusGained | Event::FocusLost => {}
                Event::Paste(text) => app.handle_paste(text),
            }
        }
    }

    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Pane {
    Library,
    Request,
    Response,
}

impl Pane {
    fn next(self) -> Self {
        match self {
            Self::Library => Self::Request,
            Self::Request => Self::Response,
            Self::Response => Self::Library,
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::Library => Self::Response,
            Self::Request => Self::Library,
            Self::Response => Self::Request,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestField {
    Title,
    Method,
    Url,
    Headers,
    Body,
}

impl RequestField {
    fn next(self) -> Self {
        match self {
            Self::Title => Self::Method,
            Self::Method => Self::Url,
            Self::Url => Self::Headers,
            Self::Headers => Self::Body,
            Self::Body => Self::Title,
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::Title => Self::Body,
            Self::Method => Self::Title,
            Self::Url => Self::Method,
            Self::Headers => Self::Url,
            Self::Body => Self::Headers,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatusTone {
    Info,
    Success,
    Error,
}

#[derive(Clone, Debug)]
pub struct StatusMessage {
    pub tone: StatusTone,
    pub message: String,
}

impl StatusMessage {
    fn info(message: impl Into<String>) -> Self {
        Self {
            tone: StatusTone::Info,
            message: message.into(),
        }
    }

    fn success(message: impl Into<String>) -> Self {
        Self {
            tone: StatusTone::Success,
            message: message.into(),
        }
    }

    fn error(message: impl Into<String>) -> Self {
        Self {
            tone: StatusTone::Error,
            message: message.into(),
        }
    }
}

#[derive(Debug)]
pub struct AppState {
    pub library: Vec<SavedRequest>,
    pub selected_index: Option<usize>,
    pub draft: RequestEditor,
    pub response: Option<ResponseData>,
    pub focus: Pane,
    pub request_field: RequestField,
    pub request_editing: bool,
    pub response_scroll: u16,
    pub status: StatusMessage,
    pub request_in_flight: bool,
    pub should_quit: bool,
    storage_path: PathBuf,
}

impl AppState {
    pub fn new(storage_path: PathBuf, library: LibraryFile) -> Self {
        let selected_index = (!library.requests.is_empty()).then_some(0);
        let mut state = Self {
            library: library.requests,
            selected_index,
            draft: RequestEditor::blank(),
            response: None,
            focus: if selected_index.is_some() {
                Pane::Library
            } else {
                Pane::Request
            },
            request_field: RequestField::Title,
            request_editing: false,
            response_scroll: 0,
            status: StatusMessage::info("Ready."),
            request_in_flight: false,
            should_quit: false,
            storage_path,
        };

        if selected_index.is_some() {
            state.load_selected_request();
        }

        state
    }
    pub fn handle_app_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::NetworkResponse(result) => {
                self.request_in_flight = false;
                match result {
                    Ok(response) => {
                        self.response_scroll = 0;
                        self.status = StatusMessage::success(format!(
                            "Received {} in {} ms.",
                            response.status_code, response.elapsed_ms
                        ));
                        self.response = Some(response);
                    }
                    Err(error) => {
                        self.status = StatusMessage::error(error);
                    }
                }
            }
        }
    }

    pub fn handle_paste(&mut self, text: String) {
        if self.focus != Pane::Request || !self.request_editing {
            return;
        }

        match self.request_field {
            RequestField::Title => {
                self.draft
                    .title
                    .insert_str(normalize_single_line_paste(&text));
            }
            RequestField::Url => {
                self.draft
                    .url
                    .insert_str(normalize_single_line_paste(&text));
            }
            RequestField::Headers => {
                self.draft
                    .headers
                    .insert_str(normalize_multiline_paste(&text));
            }
            RequestField::Body => {
                self.draft.body.insert_str(normalize_multiline_paste(&text));
            }
            RequestField::Method => {}
        };
    }

    pub fn handle_key_event(&mut self, key: KeyEvent, sender: &AppEventSender) {
        if self.try_paste_from_clipboard_shortcut(key) {
            return;
        }

        if self.handle_global_shortcuts(key, sender) {
            return;
        }

        match self.focus {
            Pane::Library => self.handle_library_key(key),
            Pane::Request => self.handle_request_key(key),
            Pane::Response => self.handle_response_key(key),
        }
    }

    fn try_paste_from_clipboard_shortcut(&mut self, key: KeyEvent) -> bool {
        if !is_clipboard_paste_shortcut(key) {
            return false;
        }

        if self.focus != Pane::Request || self.request_field == RequestField::Method {
            self.status =
                StatusMessage::error("Clipboard paste only works in request text fields.");
            return true;
        }

        self.request_editing = true;
        match read_system_clipboard_text() {
            Ok(text) => {
                self.handle_paste(text);
                self.status = StatusMessage::success("Pasted from clipboard.");
            }
            Err(error) => {
                self.status = StatusMessage::error(error);
            }
        }

        true
    }

    fn handle_global_shortcuts(&mut self, key: KeyEvent, sender: &AppEventSender) -> bool {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('s') {
            match self.save_current_request() {
                Ok(created_new) => {
                    let message = if created_new {
                        "Saved request to the library."
                    } else {
                        "Updated saved request."
                    };
                    self.status = StatusMessage::success(message);
                }
                Err(error) => self.status = StatusMessage::error(error),
            }
            return true;
        }

        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('r') {
            if let Err(error) = self.send_current_request(sender) {
                self.status = StatusMessage::error(error);
            }
            return true;
        }

        match key.code {
            KeyCode::Tab => {
                self.request_editing = false;
                self.focus = self.focus.next();
                self.status = StatusMessage::info("Switched focus.");
                true
            }
            KeyCode::BackTab => {
                self.request_editing = false;
                self.focus = self.focus.previous();
                self.status = StatusMessage::info("Switched focus.");
                true
            }
            KeyCode::Esc if self.request_editing => {
                self.request_editing = false;
                self.status = StatusMessage::info("Exited request editing.");
                true
            }
            KeyCode::Char('n') if !self.request_editing => {
                self.new_request();
                true
            }
            KeyCode::Char('q') if !self.request_editing => {
                self.should_quit = true;
                true
            }
            _ => false,
        }
    }

    fn handle_library_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up => self.select_previous_request(),
            KeyCode::Down => self.select_next_request(),
            KeyCode::Enter => {
                if self.load_selected_request() {
                    self.status = StatusMessage::info("Loaded request into the editor.");
                }
            }
            _ => {}
        }
    }

    fn handle_request_key(&mut self, key: KeyEvent) {
        if self.request_editing {
            self.handle_request_edit_input(key);
            return;
        }

        match key.code {
            KeyCode::Up => self.request_field = self.request_field.previous(),
            KeyCode::Down => self.request_field = self.request_field.next(),
            KeyCode::Left if self.request_field == RequestField::Method => {
                self.draft.method = self.draft.method.previous();
            }
            KeyCode::Right if self.request_field == RequestField::Method => {
                self.draft.method = self.draft.method.next();
            }
            KeyCode::Enter => {
                self.request_editing = true;
                self.status = StatusMessage::info(match self.request_field {
                    RequestField::Method => "Editing method. Use Left/Right to change it.",
                    _ => "Editing request field. Press Esc to stop editing.",
                });
            }
            _ => {}
        }
    }

    fn handle_request_edit_input(&mut self, key: KeyEvent) {
        match self.request_field {
            RequestField::Method => match key.code {
                KeyCode::Left | KeyCode::Up => self.draft.method = self.draft.method.previous(),
                KeyCode::Right | KeyCode::Down => self.draft.method = self.draft.method.next(),
                KeyCode::Enter => self.request_editing = false,
                _ => {}
            },
            RequestField::Title => handle_single_line_key(&mut self.draft.title, key),
            RequestField::Url => handle_single_line_key(&mut self.draft.url, key),
            RequestField::Headers => {
                self.draft.headers.input(Input::from(key));
            }
            RequestField::Body => {
                self.draft.body.input(Input::from(key));
            }
        }
    }

    fn handle_response_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up => self.response_scroll = self.response_scroll.saturating_sub(1),
            KeyCode::Down => self.response_scroll = self.response_scroll.saturating_add(1),
            KeyCode::PageUp => self.response_scroll = self.response_scroll.saturating_sub(10),
            KeyCode::PageDown => self.response_scroll = self.response_scroll.saturating_add(10),
            _ => {}
        }
    }

    fn select_next_request(&mut self) {
        let Some(current) = self.selected_index else {
            return;
        };

        if current + 1 < self.library.len() {
            self.selected_index = Some(current + 1);
        }
    }

    fn select_previous_request(&mut self) {
        let Some(current) = self.selected_index else {
            return;
        };

        if current > 0 {
            self.selected_index = Some(current - 1);
        }
    }

    fn load_selected_request(&mut self) -> bool {
        let Some(index) = self.selected_index else {
            return false;
        };
        let Some(request) = self.library.get(index).cloned() else {
            return false;
        };

        self.draft = RequestEditor::from_saved_request(&request);
        true
    }

    fn new_request(&mut self) {
        self.focus = Pane::Request;
        self.request_field = RequestField::Title;
        self.request_editing = false;
        self.draft = RequestEditor::blank();
        self.status = StatusMessage::info("Created a new request draft.");
    }

    pub fn save_current_request(&mut self) -> Result<bool, String> {
        let title = self.draft.optional_title();
        let headers = self.draft.parsed_headers()?;
        validate_json_body(&self.draft.body_text())?;

        let saved = SavedRequest {
            id: self.draft.loaded_request_id.unwrap_or_else(Uuid::new_v4),
            title,
            method: self.draft.method,
            url: self.draft.url_text(),
            headers,
            json_body: self.draft.body_text(),
        };

        let created_new = match self
            .draft
            .loaded_request_id
            .and_then(|id| self.library.iter().position(|request| request.id == id))
        {
            Some(index) => {
                self.library[index] = saved.clone();
                self.selected_index = Some(index);
                false
            }
            None => {
                self.library.push(saved.clone());
                self.selected_index = Some(self.library.len() - 1);
                true
            }
        };

        self.draft = RequestEditor::from_saved_request(&saved);
        storage::save_library(
            &self.storage_path,
            &LibraryFile {
                version: crate::model::CURRENT_LIBRARY_VERSION,
                requests: self.library.clone(),
            },
        )
        .map_err(|error| error.to_string())?;

        Ok(created_new)
    }

    fn send_current_request(&mut self, sender: &AppEventSender) -> Result<(), String> {
        if self.request_in_flight {
            return Err("A request is already in flight.".to_string());
        }

        if self.focus == Pane::Library && !self.load_selected_request() {
            return Err("Select a saved request before sending.".to_string());
        }

        let request = self.draft.to_request_input()?;
        self.request_in_flight = true;
        self.status = StatusMessage::info("Sending request...");
        let sender = sender.clone();

        tokio::spawn(async move {
            let result = network::send_request(request).await;
            let _ = sender.send(AppEvent::NetworkResponse(result));
        });

        Ok(())
    }
}

#[derive(Debug)]
pub struct RequestEditor {
    pub loaded_request_id: Option<Uuid>,
    pub method: HttpMethod,
    pub title: TextArea<'static>,
    pub url: TextArea<'static>,
    pub headers: TextArea<'static>,
    pub body: TextArea<'static>,
}

impl RequestEditor {
    pub fn blank() -> Self {
        Self {
            loaded_request_id: None,
            method: HttpMethod::Get,
            title: single_line_area(""),
            url: single_line_area(""),
            headers: multi_line_area("Accept: application/json"),
            body: multi_line_area(""),
        }
    }

    pub fn from_saved_request(request: &SavedRequest) -> Self {
        Self {
            loaded_request_id: Some(request.id),
            method: request.method,
            title: single_line_area(request.title.as_deref().unwrap_or("")),
            url: single_line_area(&request.url),
            headers: multi_line_area(&headers_to_text(&request.headers)),
            body: multi_line_area(&request.json_body),
        }
    }

    pub fn title_text(&self) -> String {
        sanitize_single_line(&self.title)
    }

    pub fn url_text(&self) -> String {
        sanitize_single_line(&self.url)
    }

    pub fn headers_text(&self) -> String {
        self.headers.lines().join("\n")
    }

    pub fn body_text(&self) -> String {
        self.body.lines().join("\n")
    }

    pub fn optional_title(&self) -> Option<String> {
        let title = self.title_text();
        (!title.trim().is_empty()).then_some(title)
    }

    pub fn parsed_headers(&self) -> Result<Vec<HeaderEntry>, String> {
        parse_header_lines(&self.headers_text())
    }

    pub fn to_request_input(&self) -> Result<RequestInput, String> {
        let request = RequestInput {
            title: self.optional_title(),
            method: self.method,
            url: self.url_text(),
            headers: self.parsed_headers()?,
            json_body: self.body_text(),
        };

        validate_url(&request.url)?;
        validate_json_body(&request.json_body)?;

        Ok(request)
    }

    #[cfg(test)]
    fn set_title(&mut self, value: &str) {
        self.title = single_line_area(value);
    }

    #[cfg(test)]
    fn set_url(&mut self, value: &str) {
        self.url = single_line_area(value);
    }

    #[cfg(test)]
    fn set_headers(&mut self, value: &str) {
        self.headers = multi_line_area(value);
    }

    #[cfg(test)]
    fn set_body(&mut self, value: &str) {
        self.body = multi_line_area(value);
    }
}

fn single_line_area(value: &str) -> TextArea<'static> {
    let sanitized = value.replace(['\n', '\r'], " ");
    let mut textarea = TextArea::new(vec![sanitized]);
    textarea.set_cursor_line_style(Style::default().fg(Color::White));
    textarea
}

fn multi_line_area(value: &str) -> TextArea<'static> {
    let normalized = value.replace('\r', "");
    let lines = if normalized.is_empty() {
        vec![String::new()]
    } else {
        normalized.split('\n').map(str::to_string).collect()
    };
    let mut textarea = TextArea::new(lines);
    textarea.set_cursor_line_style(Style::default().fg(Color::White));
    textarea
}

fn sanitize_single_line(textarea: &TextArea<'static>) -> String {
    textarea.lines().join(" ").trim().to_string()
}

fn normalize_single_line_paste(text: &str) -> String {
    text.replace(['\r', '\n'], " ")
}

fn normalize_multiline_paste(text: &str) -> String {
    text.replace('\r', "")
}

fn is_clipboard_paste_shortcut(key: KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char(ch) if ch.eq_ignore_ascii_case(&'v'))
}

fn read_system_clipboard_text() -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        return run_clipboard_command("pbpaste", &[]);
    }

    #[cfg(target_os = "windows")]
    {
        return run_clipboard_command("powershell", &["-NoProfile", "-Command", "Get-Clipboard"]);
    }

    #[cfg(target_os = "linux")]
    {
        for (program, args) in [
            ("wl-paste", vec!["--no-newline"]),
            ("xclip", vec!["-selection", "clipboard", "-o"]),
            ("xsel", vec!["--clipboard", "--output"]),
        ] {
            if let Ok(text) = run_clipboard_command(program, &args) {
                return Ok(text);
            }
        }

        return Err(
            "Unable to read the clipboard. Try terminal paste or install wl-paste, xclip, or xsel."
                .to_string(),
        );
    }

    #[allow(unreachable_code)]
    Err("Clipboard paste is not supported on this platform yet.".to_string())
}

fn run_clipboard_command(program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|error| format!("Clipboard read failed via `{program}`: {error}"))?;

    if !output.status.success() {
        return Err(format!(
            "Clipboard read failed via `{program}` with status {}.",
            output.status
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn handle_single_line_key(textarea: &mut TextArea<'static>, key: KeyEvent) {
    match Input::from(key) {
        Input {
            key: Key::Enter, ..
        }
        | Input {
            key: Key::Char('m'),
            ctrl: true,
            ..
        } => {}
        input => {
            textarea.input(input);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyModifiers};
    use tempfile::tempdir;

    fn sample_request() -> SavedRequest {
        SavedRequest {
            id: Uuid::new_v4(),
            title: Some("Example".to_string()),
            method: HttpMethod::Get,
            url: "https://example.com".to_string(),
            headers: vec![HeaderEntry {
                name: "Accept".to_string(),
                value: "application/json".to_string(),
            }],
            json_body: "{}".to_string(),
        }
    }

    #[test]
    fn loads_selected_request_into_draft() {
        let dir = tempdir().unwrap();
        let request = sample_request();
        let app = AppState::new(
            dir.path().join("library.json"),
            LibraryFile {
                version: crate::model::CURRENT_LIBRARY_VERSION,
                requests: vec![request.clone()],
            },
        );

        assert_eq!(app.draft.url_text(), request.url);
        assert_eq!(app.draft.optional_title(), request.title);
    }

    #[test]
    fn saves_new_requests_to_library() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("library.json");
        let mut app = AppState::new(path.clone(), LibraryFile::default());

        app.draft.set_title("Create");
        app.draft.method = HttpMethod::Post;
        app.draft.set_url("https://example.com/api");
        app.draft.set_headers("Accept: application/json");
        app.draft.set_body(r#"{"hello":"world"}"#);

        let created_new = app.save_current_request().unwrap();
        let persisted = storage::load_library(&path).unwrap();

        assert!(created_new);
        assert_eq!(app.library.len(), 1);
        assert_eq!(persisted.requests.len(), 1);
        assert_eq!(persisted.requests[0].url, "https://example.com/api");
    }

    #[test]
    fn overwrites_loaded_requests() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("library.json");
        let request = sample_request();
        let mut app = AppState::new(
            path.clone(),
            LibraryFile {
                version: crate::model::CURRENT_LIBRARY_VERSION,
                requests: vec![request],
            },
        );

        app.draft.set_title("Updated");
        app.draft.set_body(r#"{"changed":true}"#);

        let created_new = app.save_current_request().unwrap();
        let persisted = storage::load_library(&path).unwrap();

        assert!(!created_new);
        assert_eq!(persisted.requests.len(), 1);
        assert_eq!(persisted.requests[0].title.as_deref(), Some("Updated"));
    }

    #[test]
    fn cycles_focus_with_tab() {
        let dir = tempdir().unwrap();
        let (sender, _receiver) = event_channel();
        let mut app = AppState::new(dir.path().join("library.json"), LibraryFile::default());

        assert_eq!(app.focus, Pane::Request);
        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE), &sender);
        assert_eq!(app.focus, Pane::Response);
        app.handle_key_event(
            KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
            &sender,
        );
        assert_eq!(app.focus, Pane::Request);
    }

    #[test]
    fn rejects_invalid_requests_before_send() {
        let dir = tempdir().unwrap();
        let (sender, _receiver) = event_channel();
        let mut app = AppState::new(dir.path().join("library.json"), LibraryFile::default());
        app.draft.set_url("not-a-url");

        let error = app.send_current_request(&sender).unwrap_err();

        assert!(error.contains("URL is invalid") || error.contains("URL is required"));
        assert!(!app.request_in_flight);
    }

    #[test]
    fn recognizes_ctrl_v_as_clipboard_paste_shortcut() {
        assert!(is_clipboard_paste_shortcut(KeyEvent::new(
            KeyCode::Char('v'),
            KeyModifiers::CONTROL,
        )));
        assert!(is_clipboard_paste_shortcut(KeyEvent::new(
            KeyCode::Char('V'),
            KeyModifiers::CONTROL,
        )));
        assert!(!is_clipboard_paste_shortcut(KeyEvent::new(
            KeyCode::Char('v'),
            KeyModifiers::NONE,
        )));
    }

    #[test]
    fn pastes_single_line_fields_without_creating_extra_lines() {
        let dir = tempdir().unwrap();
        let mut app = AppState::new(dir.path().join("library.json"), LibraryFile::default());
        app.focus = Pane::Request;
        app.request_editing = true;
        app.request_field = RequestField::Url;

        app.handle_paste("https://example.com/api\nusers".to_string());

        assert_eq!(app.draft.url_text(), "https://example.com/api users");
        assert_eq!(app.draft.url.lines().len(), 1);
    }

    #[test]
    fn pastes_multiline_body_preserving_newlines() {
        let dir = tempdir().unwrap();
        let mut app = AppState::new(dir.path().join("library.json"), LibraryFile::default());
        app.focus = Pane::Request;
        app.request_editing = true;
        app.request_field = RequestField::Body;

        app.handle_paste("{\r\n  \"ok\": true\r\n}".to_string());

        assert_eq!(app.draft.body_text(), "{\n  \"ok\": true\n}");
        assert_eq!(app.draft.body.lines().len(), 3);
    }
}
