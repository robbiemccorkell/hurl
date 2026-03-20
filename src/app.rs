use crate::config;
use crate::events::{AppEvent, AppEventReceiver, AppEventSender, SyncOperation, event_channel};
use crate::model::{
    HeaderEntry, HttpMethod, LibraryFile, RequestInput, ResponseData, SavedRequest,
    headers_to_text, parse_header_lines, validate_json_body, validate_url,
};
use crate::sync::{
    self, DeviceCodePrompt, SecretPersistence, SyncConfig, SyncFile,
    SyncStatus as SyncConnectionStatus,
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

const DISABLED_SYNC_FIELDS: [SyncSettingsField; 6] = [
    SyncSettingsField::ConnectGitHub,
    SyncSettingsField::Owner,
    SyncSettingsField::Repo,
    SyncSettingsField::Password,
    SyncSettingsField::ConfirmPassword,
    SyncSettingsField::EnableSync,
];

const ENABLED_SYNC_FIELDS: [SyncSettingsField; 2] =
    [SyncSettingsField::SyncNow, SyncSettingsField::Disconnect];

fn sync_operation_updates_status(operation: SyncOperation) -> bool {
    matches!(operation, SyncOperation::Manual | SyncOperation::Enable)
}

pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let path = storage::library_path()?;
    let sync_path = storage::sync_path()?;
    let library = storage::load_library(&path)?;
    let sync_file = storage::load_sync_file(&sync_path)?;
    let (sender, receiver) = event_channel();
    let mut app = AppState::new(path, sync_path, library, sync_file);
    let mut terminal = ui::setup_terminal()?;

    app.schedule_startup_sync();

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
            app.handle_app_event(event, &sender);
        }

        terminal.draw(|frame| ui::draw(frame, app))?;
        app.start_pending_startup_sync(&sender);

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
pub enum Screen {
    Main,
    Settings,
}

impl Screen {
    pub fn label(self) -> &'static str {
        match self {
            Self::Main => "Main",
            Self::Settings => "Settings",
        }
    }
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

    pub fn label(self) -> &'static str {
        match self {
            Self::Library => "Library",
            Self::Request => "Request",
            Self::Response => "Response",
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
    fn up(self) -> Self {
        match self {
            Self::Title => Self::Title,
            Self::Method | Self::Url => Self::Title,
            Self::Headers => Self::Method,
            Self::Body => Self::Headers,
        }
    }

    fn down(self) -> Self {
        match self {
            Self::Title => Self::Method,
            Self::Method | Self::Url => Self::Headers,
            Self::Headers => Self::Body,
            Self::Body => Self::Body,
        }
    }

    fn left(self) -> Self {
        match self {
            Self::Url => Self::Method,
            field => field,
        }
    }

    fn right(self) -> Self {
        match self {
            Self::Method => Self::Url,
            field => field,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Title => "Title",
            Self::Method => "Method",
            Self::Url => "URL",
            Self::Headers => "Headers",
            Self::Body => "Body",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SettingsFocus {
    Nav,
    Detail,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SettingsSection {
    Sync,
}

impl SettingsSection {
    pub fn label(self) -> &'static str {
        match self {
            Self::Sync => "Sync",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyncSettingsField {
    ConnectGitHub,
    Owner,
    Repo,
    Password,
    ConfirmPassword,
    EnableSync,
    SyncNow,
    Disconnect,
}

impl SyncSettingsField {
    pub fn label(self) -> &'static str {
        match self {
            Self::ConnectGitHub => "Connect GitHub",
            Self::Owner => "Repo Owner",
            Self::Repo => "Repo Name",
            Self::Password => "Sync Password",
            Self::ConfirmPassword => "Confirm Password",
            Self::EnableSync => "Enable Sync",
            Self::SyncNow => "Sync Now",
            Self::Disconnect => "Disconnect",
        }
    }

    fn is_text_input(self) -> bool {
        matches!(
            self,
            Self::Owner | Self::Repo | Self::Password | Self::ConfirmPassword
        )
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
pub struct SettingsState {
    pub focus: SettingsFocus,
    pub section: SettingsSection,
    pub sync_field: SyncSettingsField,
    pub editing: bool,
}

impl Default for SettingsState {
    fn default() -> Self {
        Self {
            focus: SettingsFocus::Nav,
            section: SettingsSection::Sync,
            sync_field: SyncSettingsField::ConnectGitHub,
            editing: false,
        }
    }
}

#[derive(Debug)]
pub struct SyncSettingsForm {
    pub owner: TextArea<'static>,
    pub repo: TextArea<'static>,
    pub password: TextArea<'static>,
    pub confirm_password: TextArea<'static>,
}

impl SyncSettingsForm {
    fn new(owner: &str, repo: &str) -> Self {
        Self {
            owner: single_line_area(owner),
            repo: single_line_area(repo),
            password: single_line_area(""),
            confirm_password: single_line_area(""),
        }
    }

    fn owner_text(&self) -> String {
        sanitize_single_line(&self.owner)
    }

    fn repo_text(&self) -> String {
        sanitize_single_line(&self.repo)
    }

    fn password_text(&self) -> String {
        sanitize_single_line(&self.password)
    }

    fn confirm_password_text(&self) -> String {
        sanitize_single_line(&self.confirm_password)
    }

    fn clear_passwords(&mut self) {
        self.password = single_line_area("");
        self.confirm_password = single_line_area("");
    }
}

#[derive(Debug)]
pub struct SyncRuntime {
    pub file: SyncFile,
    pub status: SyncConnectionStatus,
    pub access_token: Option<String>,
    pub sync_password: Option<String>,
    pub github_user: Option<String>,
    pub last_error: Option<String>,
    pub last_warning: Option<String>,
    pub pending_device_code: Option<DeviceCodePrompt>,
    pub in_flight: bool,
}

#[derive(Debug)]
pub struct AppState {
    pub screen: Screen,
    pub library: Vec<SavedRequest>,
    pub selected_index: Option<usize>,
    pub draft: RequestEditor,
    pub response: Option<ResponseData>,
    pub focus: Pane,
    pub request_field: RequestField,
    pub request_editing: bool,
    pub response_scroll: u16,
    pub settings: SettingsState,
    pub sync_form: SyncSettingsForm,
    pub sync: SyncRuntime,
    pub status: StatusMessage,
    pub request_in_flight: bool,
    pub should_quit: bool,
    storage_path: PathBuf,
    sync_path: PathBuf,
    library_revision: u64,
    startup_sync_pending: bool,
}

impl AppState {
    pub fn new(
        storage_path: PathBuf,
        sync_path: PathBuf,
        library: LibraryFile,
        sync_file: SyncFile,
    ) -> Self {
        let selected_index = (!library.requests.is_empty()).then_some(0);
        let focus = if selected_index.is_some() {
            Pane::Library
        } else {
            Pane::Request
        };

        let github_user = sync_file
            .config
            .as_ref()
            .map(|config| config.github_user.clone())
            .filter(|user| !user.trim().is_empty());
        let owner = sync_file
            .config
            .as_ref()
            .map(|config| config.owner.as_str())
            .filter(|owner| !owner.trim().is_empty())
            .or(github_user.as_deref())
            .unwrap_or("");
        let repo = sync_file
            .config
            .as_ref()
            .map(|config| config.repo.as_str())
            .filter(|repo| !repo.trim().is_empty())
            .unwrap_or(sync::default_repo_name());

        let mut state = Self {
            screen: Screen::Main,
            library: library.requests,
            selected_index,
            draft: RequestEditor::blank(),
            response: None,
            focus,
            request_field: RequestField::Title,
            request_editing: false,
            response_scroll: 0,
            settings: SettingsState::default(),
            sync_form: SyncSettingsForm::new(owner, repo),
            sync: SyncRuntime {
                file: sync_file,
                status: SyncConnectionStatus::Off,
                access_token: None,
                sync_password: None,
                github_user,
                last_error: None,
                last_warning: None,
                pending_device_code: None,
                in_flight: false,
            },
            status: StatusMessage::info("Ready."),
            request_in_flight: false,
            should_quit: false,
            storage_path,
            sync_path,
            library_revision: 0,
            startup_sync_pending: false,
        };

        if state.selected_index.is_some() {
            state.load_selected_request();
        }

        state.recalculate_sync_status();
        state.sync_field_sanitize();
        state
    }

    pub fn handle_app_event(&mut self, event: AppEvent, sender: &AppEventSender) {
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
            AppEvent::GitHubDeviceCode(result) => match result {
                Ok(prompt) => {
                    self.sync.pending_device_code = Some(prompt);
                    self.sync.last_error = None;
                    self.status = StatusMessage::info(
                        "Authorize hurl in GitHub using the device code shown in Settings.",
                    );
                    self.recalculate_sync_status();
                }
                Err(error) => {
                    self.sync.pending_device_code = None;
                    self.sync.last_error = Some(error.clone());
                    self.status = StatusMessage::error(error);
                    self.recalculate_sync_status();
                }
            },
            AppEvent::GitHubAuthComplete(result) => {
                self.sync.pending_device_code = None;
                match result {
                    Ok(identity) => {
                        let persistence = sync::store_access_token(&identity.access_token);
                        self.sync.access_token = Some(identity.access_token);
                        self.sync.github_user = Some(identity.username.clone());
                        if self.sync_form.owner_text().trim().is_empty() {
                            self.sync_form.owner = single_line_area(&identity.username);
                        }
                        let message = match persistence {
                            SecretPersistence::Persisted => {
                                format!("Connected GitHub account `{}`.", identity.username)
                            }
                            SecretPersistence::SessionOnly => format!(
                                "Connected GitHub account `{}`. Token will only persist for this session.",
                                identity.username
                            ),
                            SecretPersistence::Deleted => {
                                format!("Connected GitHub account `{}`.", identity.username)
                            }
                        };
                        self.sync.last_error = None;
                        self.status = StatusMessage::success(message);
                    }
                    Err(error) => {
                        self.sync.last_error = Some(error.clone());
                        self.status = StatusMessage::error(error);
                    }
                }
                self.recalculate_sync_status();
            }
            AppEvent::SyncFinished {
                operation,
                base_revision,
                result,
            } => {
                self.sync.in_flight = false;
                match result {
                    Ok(output) => {
                        if base_revision != self.library_revision {
                            self.sync.file.state.dirty = true;
                            self.sync.last_warning = Some(
                                "Local changes were made during sync. Sync will run again."
                                    .to_string(),
                            );
                            let _ = self.persist_sync_file();
                            self.recalculate_sync_status();
                            if sync_operation_updates_status(operation) {
                                self.status = StatusMessage::info(
                                    "Local changes were made during sync. Queued another sync.",
                                );
                            }
                            self.start_sync_if_possible(sender, SyncOperation::Save);
                            return;
                        }

                        if operation == SyncOperation::Enable {
                            let password = self.sync_form.password_text();
                            let persistence = sync::store_sync_password(&password);
                            self.sync.sync_password = Some(password);
                            self.sync_form.clear_passwords();
                            self.settings.sync_field = SyncSettingsField::SyncNow;
                            if persistence == SecretPersistence::SessionOnly {
                                self.sync.last_warning = Some(
                                    "The sync password could not be stored in the OS keychain and will only persist for this session."
                                        .to_string(),
                                );
                            }
                        }

                        self.sync.file.config = Some(output.config.clone());
                        self.sync.file.state = output.state.clone();
                        self.sync.last_error = None;
                        self.sync.last_warning =
                            output.warning.clone().or(self.sync.last_warning.clone());
                        self.apply_synced_library(output.library);
                        if let Err(error) = self.persist_library() {
                            self.sync.last_error = Some(error.clone());
                            if sync_operation_updates_status(operation) {
                                self.status = StatusMessage::error(error);
                            }
                        } else if let Err(error) = self.persist_sync_file() {
                            self.sync.last_error = Some(error.clone());
                            if sync_operation_updates_status(operation) {
                                self.status = StatusMessage::error(error);
                            }
                        } else {
                            let mut message = match operation {
                                SyncOperation::Enable => "Sync enabled.".to_string(),
                                SyncOperation::Startup => "Startup sync completed.".to_string(),
                                SyncOperation::Save => {
                                    "Saved request and synced library.".to_string()
                                }
                                SyncOperation::Manual => "Sync completed.".to_string(),
                            };
                            if output.imported_count > 0 || output.uploaded_count > 0 {
                                message.push_str(&format!(
                                    " Imported {} and uploaded {} request(s).",
                                    output.imported_count, output.uploaded_count
                                ));
                            }
                            if output.conflict_count > 0 {
                                message.push_str(&format!(
                                    " Created {} conflict copy/copies.",
                                    output.conflict_count
                                ));
                            }
                            if sync_operation_updates_status(operation) {
                                self.status = if output.warning.is_some() {
                                    StatusMessage::info(message)
                                } else {
                                    StatusMessage::success(message)
                                };
                            }
                        }
                    }
                    Err(error) => {
                        self.sync.last_error = Some(error.clone());
                        if self.is_sync_enabled() {
                            self.sync.file.state.dirty = true;
                            let _ = self.persist_sync_file();
                        }
                        if sync_operation_updates_status(operation) {
                            self.status = StatusMessage::error(error);
                        }
                    }
                }
                self.recalculate_sync_status();
            }
        }
    }

    pub fn handle_paste(&mut self, text: String) {
        match self.screen {
            Screen::Main => {
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
            Screen::Settings => {
                if !self.settings.editing || self.settings.focus != SettingsFocus::Detail {
                    return;
                }
                match self.settings.sync_field {
                    SyncSettingsField::Owner => {
                        self.sync_form
                            .owner
                            .insert_str(normalize_single_line_paste(&text));
                    }
                    SyncSettingsField::Repo => {
                        self.sync_form
                            .repo
                            .insert_str(normalize_single_line_paste(&text));
                    }
                    SyncSettingsField::Password => {
                        self.sync_form
                            .password
                            .insert_str(normalize_single_line_paste(&text));
                    }
                    SyncSettingsField::ConfirmPassword => {
                        self.sync_form
                            .confirm_password
                            .insert_str(normalize_single_line_paste(&text));
                    }
                    _ => {}
                }
            }
        }
    }

    pub fn handle_key_event(&mut self, key: KeyEvent, sender: &AppEventSender) {
        if self.try_paste_from_clipboard_shortcut(key) {
            return;
        }

        if self.handle_global_shortcuts(key, sender) {
            return;
        }

        match self.screen {
            Screen::Main => match self.focus {
                Pane::Library => self.handle_library_key(key),
                Pane::Request => self.handle_request_key(key),
                Pane::Response => self.handle_response_key(key),
            },
            Screen::Settings => self.handle_settings_key(key, sender),
        }
    }

    fn try_paste_from_clipboard_shortcut(&mut self, key: KeyEvent) -> bool {
        if !is_clipboard_paste_shortcut(key) {
            return false;
        }

        if !self.is_editable_text_field_active_for_paste() {
            self.status =
                StatusMessage::error("Clipboard paste only works in editable text fields.");
            return true;
        }

        match self.screen {
            Screen::Main => self.request_editing = true,
            Screen::Settings => self.settings.editing = true,
        }

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
        if matches!(key.code, KeyCode::Char('g')) && !self.any_text_editing() {
            self.toggle_settings_screen();
            return true;
        }

        if matches!(key.code, KeyCode::Char('q')) && !self.any_text_editing() {
            self.should_quit = true;
            return true;
        }

        match self.screen {
            Screen::Main => {
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('s') {
                    match self.save_current_request() {
                        Ok(created_new) => {
                            let message = if created_new {
                                "Saved request to the library."
                            } else {
                                "Updated saved request."
                            };
                            self.status = StatusMessage::success(message);
                            self.start_sync_if_possible(sender, SyncOperation::Save);
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
                    _ => false,
                }
            }
            Screen::Settings => match key.code {
                KeyCode::Tab if !self.settings.editing => {
                    self.settings.focus = match self.settings.focus {
                        SettingsFocus::Nav => SettingsFocus::Detail,
                        SettingsFocus::Detail => SettingsFocus::Nav,
                    };
                    self.status = StatusMessage::info("Switched settings focus.");
                    true
                }
                KeyCode::BackTab if !self.settings.editing => {
                    self.settings.focus = match self.settings.focus {
                        SettingsFocus::Nav => SettingsFocus::Detail,
                        SettingsFocus::Detail => SettingsFocus::Nav,
                    };
                    self.status = StatusMessage::info("Switched settings focus.");
                    true
                }
                KeyCode::Esc if self.settings.editing => {
                    self.settings.editing = false;
                    self.status = StatusMessage::info("Exited settings editing.");
                    true
                }
                KeyCode::Esc => {
                    self.screen = Screen::Main;
                    self.status = StatusMessage::info("Closed Settings.");
                    true
                }
                _ => false,
            },
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
            KeyCode::Up => self.request_field = self.request_field.up(),
            KeyCode::Down => self.request_field = self.request_field.down(),
            KeyCode::Left => self.request_field = self.request_field.left(),
            KeyCode::Right => self.request_field = self.request_field.right(),
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

    fn handle_settings_key(&mut self, key: KeyEvent, sender: &AppEventSender) {
        if self.settings.editing {
            self.handle_settings_edit_input(key);
            return;
        }

        match self.settings.focus {
            SettingsFocus::Nav => match key.code {
                KeyCode::Enter | KeyCode::Right => {
                    self.settings.focus = SettingsFocus::Detail;
                    self.status = StatusMessage::info("Opened Sync settings.");
                }
                KeyCode::Up | KeyCode::Down => {}
                _ => {}
            },
            SettingsFocus::Detail => match key.code {
                KeyCode::Up => self.move_sync_field(-1),
                KeyCode::Down => self.move_sync_field(1),
                KeyCode::Left => {
                    self.settings.focus = SettingsFocus::Nav;
                    self.status = StatusMessage::info("Moved to settings navigation.");
                }
                KeyCode::Enter => {
                    if self.settings.sync_field.is_text_input() && !self.is_sync_enabled() {
                        self.settings.editing = true;
                        self.status = StatusMessage::info(
                            "Editing settings field. Press Esc to stop editing.",
                        );
                    } else if let Err(error) = self.activate_sync_field(sender) {
                        self.status = StatusMessage::error(error);
                    }
                }
                _ => {}
            },
        }
    }

    fn handle_settings_edit_input(&mut self, key: KeyEvent) {
        match self.settings.sync_field {
            SyncSettingsField::Owner => handle_single_line_key(&mut self.sync_form.owner, key),
            SyncSettingsField::Repo => handle_single_line_key(&mut self.sync_form.repo, key),
            SyncSettingsField::Password => {
                handle_single_line_key(&mut self.sync_form.password, key)
            }
            SyncSettingsField::ConfirmPassword => {
                handle_single_line_key(&mut self.sync_form.confirm_password, key)
            }
            _ => {}
        }
    }

    fn activate_sync_field(&mut self, sender: &AppEventSender) -> Result<(), String> {
        match self.settings.sync_field {
            SyncSettingsField::ConnectGitHub => self.start_github_auth(sender),
            SyncSettingsField::EnableSync => self.begin_enable_sync(sender),
            SyncSettingsField::SyncNow => {
                self.start_sync_if_possible(sender, SyncOperation::Manual);
                Ok(())
            }
            SyncSettingsField::Disconnect => self.disconnect_sync(),
            _ => Ok(()),
        }
    }

    fn start_github_auth(&mut self, sender: &AppEventSender) -> Result<(), String> {
        if self.sync.pending_device_code.is_some() {
            return Err("GitHub authorization is already waiting for approval.".to_string());
        }
        let client_id = config::github_client_id().ok_or_else(|| {
            "GitHub sync is not configured. Add your GitHub OAuth app client ID in src/config.rs.".to_string()
        })?;
        self.status = StatusMessage::info("Starting GitHub device authorization...");
        let sender = sender.clone();
        tokio::spawn(async move {
            let prompt_result = sync::request_device_code(&client_id).await;
            let prompt = match prompt_result {
                Ok(prompt) => {
                    let _ = webbrowser::open(&prompt.verification_uri);
                    let _ = sender.send(AppEvent::GitHubDeviceCode(Ok(prompt.clone())));
                    prompt
                }
                Err(error) => {
                    let _ = sender.send(AppEvent::GitHubDeviceCode(Err(error)));
                    return;
                }
            };

            let completion = sync::complete_device_flow(&client_id, &prompt).await;
            let _ = sender.send(AppEvent::GitHubAuthComplete(completion));
        });
        Ok(())
    }

    fn begin_enable_sync(&mut self, sender: &AppEventSender) -> Result<(), String> {
        if self.sync.in_flight {
            return Err("A sync operation is already in flight.".to_string());
        }
        let access_token = self
            .sync
            .access_token
            .clone()
            .ok_or_else(|| "Connect GitHub before enabling sync.".to_string())?;
        let github_user = self
            .sync
            .github_user
            .clone()
            .ok_or_else(|| "Connect GitHub before enabling sync.".to_string())?;
        let owner = self.sync_form.owner_text();
        let repo = self.sync_form.repo_text();
        let password = self.sync_form.password_text();
        let confirm = self.sync_form.confirm_password_text();

        if owner.trim().is_empty() {
            return Err("A GitHub repo owner is required.".to_string());
        }
        if repo.trim().is_empty() {
            return Err("A GitHub repo name is required.".to_string());
        }
        if password.is_empty() {
            return Err("Enter a sync password before enabling sync.".to_string());
        }
        if password != confirm {
            return Err("Sync password confirmation does not match.".to_string());
        }

        let device_id = self
            .sync
            .file
            .config
            .as_ref()
            .map(|config| config.device_id)
            .unwrap_or_else(Uuid::new_v4);
        let config = SyncConfig {
            enabled: true,
            owner,
            repo,
            branch: "main".to_string(),
            github_user,
            device_id,
        };
        let state = self.sync.file.state.clone();
        let library = self.library.clone();
        let base_revision = self.library_revision;

        self.sync.in_flight = true;
        self.sync.last_error = None;
        self.sync.last_warning = None;
        self.recalculate_sync_status();
        self.status = StatusMessage::info("Enabling sync...");
        let sender = sender.clone();
        tokio::spawn(async move {
            let result = sync::enable_sync(config, state, library, &access_token, &password).await;
            let _ = sender.send(AppEvent::SyncFinished {
                operation: SyncOperation::Enable,
                base_revision,
                result,
            });
        });
        Ok(())
    }

    fn start_sync_if_possible(&mut self, sender: &AppEventSender, operation: SyncOperation) {
        if !self.is_sync_enabled() {
            self.recalculate_sync_status();
            return;
        }
        if self.sync.in_flight {
            self.sync.file.state.dirty = true;
            self.sync.last_warning =
                Some("Another sync is already running. Changes were queued.".to_string());
            let _ = self.persist_sync_file();
            self.recalculate_sync_status();
            return;
        }

        let Some(config) = self.sync.file.config.clone() else {
            self.recalculate_sync_status();
            return;
        };
        if let Err(error) = self.ensure_sync_secrets_loaded() {
            self.sync.last_error = Some(error.clone());
            if matches!(operation, SyncOperation::Save) {
                self.sync.file.state.dirty = true;
                let _ = self.persist_sync_file();
            }
            self.recalculate_sync_status();
            if sync_operation_updates_status(operation) {
                self.status = StatusMessage::error(error);
            }
            return;
        }
        let Some(access_token) = self.sync.access_token.clone() else {
            self.sync.last_error = Some(
                "Sync is enabled but the stored GitHub token is missing. Reconnect in Settings."
                    .to_string(),
            );
            if matches!(operation, SyncOperation::Save) {
                self.sync.file.state.dirty = true;
                let _ = self.persist_sync_file();
            }
            self.recalculate_sync_status();
            return;
        };
        let Some(password) = self.sync.sync_password.clone() else {
            self.sync.last_error = Some(
                "Sync is enabled but the sync password is missing on this machine. Reconnect in Settings.".to_string(),
            );
            if matches!(operation, SyncOperation::Save) {
                self.sync.file.state.dirty = true;
                let _ = self.persist_sync_file();
            }
            self.recalculate_sync_status();
            return;
        };

        let state = self.sync.file.state.clone();
        let library = self.library.clone();
        let base_revision = self.library_revision;
        self.sync.in_flight = true;
        self.sync.last_error = None;
        self.sync.last_warning = None;
        self.recalculate_sync_status();
        if sync_operation_updates_status(operation) {
            self.status = StatusMessage::info(match operation {
                SyncOperation::Startup => "Running startup sync...",
                SyncOperation::Save => "Syncing saved changes...",
                SyncOperation::Manual => "Syncing now...",
                SyncOperation::Enable => "Syncing...",
            });
        }

        let sender = sender.clone();
        tokio::spawn(async move {
            let result = sync::sync_library(config, state, library, &access_token, &password).await;
            let _ = sender.send(AppEvent::SyncFinished {
                operation,
                base_revision,
                result,
            });
        });
    }

    fn ensure_sync_secrets_loaded(&mut self) -> Result<(), String> {
        if self.sync.access_token.is_none() {
            self.sync.access_token = sync::load_access_token();
        }
        if self.sync.sync_password.is_none() {
            self.sync.sync_password = sync::load_sync_password();
        }
        if self.sync.access_token.is_none() {
            return Err(
                "Sync is enabled but the stored GitHub token is missing. Reconnect in Settings."
                    .to_string(),
            );
        }
        if self.sync.sync_password.is_none() {
            return Err(
                "Sync is enabled but the sync password is missing on this machine. Reconnect in Settings."
                    .to_string(),
            );
        }
        Ok(())
    }

    pub fn schedule_startup_sync(&mut self) {
        if self.is_sync_enabled() {
            self.startup_sync_pending = true;
            self.status = StatusMessage::info(
                "Startup sync pending. hurl may ask for access to your saved GitHub sync credentials.",
            );
        }
    }

    pub fn start_pending_startup_sync(&mut self, sender: &AppEventSender) {
        if self.startup_sync_pending {
            self.startup_sync_pending = false;
            self.start_sync_if_possible(sender, SyncOperation::Startup);
        }
    }

    fn disconnect_sync(&mut self) -> Result<(), String> {
        sync::delete_access_token();
        sync::delete_sync_password();
        self.sync.access_token = None;
        self.sync.sync_password = None;
        self.sync.github_user = None;
        self.sync.pending_device_code = None;
        self.sync.last_error = None;
        self.sync.last_warning = None;
        self.sync.in_flight = false;
        self.sync.file = sync::default_sync_file();
        self.sync_form = SyncSettingsForm::new("", sync::default_repo_name());
        self.settings.focus = SettingsFocus::Nav;
        self.settings.sync_field = SyncSettingsField::ConnectGitHub;
        self.settings.editing = false;
        self.persist_sync_file()?;
        self.recalculate_sync_status();
        self.status = StatusMessage::success("Disconnected sync settings for this machine.");
        Ok(())
    }

    fn is_sync_enabled(&self) -> bool {
        self.sync
            .file
            .config
            .as_ref()
            .map(|config| config.enabled)
            .unwrap_or(false)
    }

    fn recalculate_sync_status(&mut self) {
        self.sync.status = if self.sync.in_flight {
            SyncConnectionStatus::Syncing
        } else if !self.is_sync_enabled() {
            SyncConnectionStatus::Off
        } else if self.sync.last_error.is_some() {
            SyncConnectionStatus::Error
        } else if self.sync.file.state.dirty {
            SyncConnectionStatus::Dirty
        } else {
            SyncConnectionStatus::Ready
        };
        self.sync_field_sanitize();
    }

    fn sync_field_sanitize(&mut self) {
        let fields = self.current_sync_fields();
        if !fields.contains(&self.settings.sync_field) {
            self.settings.sync_field = fields[0];
        }
    }

    fn current_sync_fields(&self) -> &'static [SyncSettingsField] {
        if self.is_sync_enabled() {
            &ENABLED_SYNC_FIELDS
        } else {
            &DISABLED_SYNC_FIELDS
        }
    }

    fn move_sync_field(&mut self, delta: isize) {
        let fields = self.current_sync_fields();
        let current = fields
            .iter()
            .position(|field| *field == self.settings.sync_field)
            .unwrap_or(0) as isize;
        let next = (current + delta).clamp(0, (fields.len() - 1) as isize) as usize;
        self.settings.sync_field = fields[next];
    }

    fn toggle_settings_screen(&mut self) {
        match self.screen {
            Screen::Main => {
                self.screen = Screen::Settings;
                self.settings.focus = SettingsFocus::Nav;
                self.settings.editing = false;
                self.status = StatusMessage::info("Opened Settings.");
            }
            Screen::Settings => {
                self.screen = Screen::Main;
                self.settings.editing = false;
                self.status = StatusMessage::info("Closed Settings.");
            }
        }
    }

    fn any_text_editing(&self) -> bool {
        match self.screen {
            Screen::Main => self.request_editing,
            Screen::Settings => self.settings.editing,
        }
    }

    fn is_editable_text_field_active_for_paste(&self) -> bool {
        match self.screen {
            Screen::Main => {
                self.focus == Pane::Request && self.request_field != RequestField::Method
            }
            Screen::Settings => {
                self.settings.focus == SettingsFocus::Detail
                    && !self.is_sync_enabled()
                    && self.settings.sync_field.is_text_input()
            }
        }
    }

    fn persist_library(&self) -> Result<(), String> {
        storage::save_library(
            &self.storage_path,
            &LibraryFile {
                version: crate::model::CURRENT_LIBRARY_VERSION,
                requests: self.library.clone(),
            },
        )
        .map_err(|error| error.to_string())
    }

    fn persist_sync_file(&self) -> Result<(), String> {
        storage::save_sync_file(&self.sync_path, &self.sync.file).map_err(|error| error.to_string())
    }

    fn apply_synced_library(&mut self, new_library: Vec<SavedRequest>) {
        let current_loaded_id = self.draft.loaded_request_id;
        let request_was_editing = self.request_editing;
        self.library = new_library;

        self.selected_index = current_loaded_id
            .and_then(|id| self.library.iter().position(|request| request.id == id))
            .or_else(|| (!self.library.is_empty()).then_some(0));

        if request_was_editing {
            return;
        }

        if let Some(id) = current_loaded_id {
            if let Some(request) = self
                .library
                .iter()
                .find(|request| request.id == id)
                .cloned()
            {
                self.draft = RequestEditor::from_saved_request(&request);
                return;
            }
        }

        if let Some(index) = self.selected_index {
            if let Some(request) = self.library.get(index).cloned() {
                self.draft = RequestEditor::from_saved_request(&request);
                return;
            }
        }

        self.draft = RequestEditor::blank();
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

        self.library_revision = self.library_revision.saturating_add(1);
        self.draft = RequestEditor::from_saved_request(&saved);
        self.persist_library()?;
        if self.is_sync_enabled() {
            self.sync.file.state.dirty = true;
            let _ = self.persist_sync_file();
            self.recalculate_sync_status();
        }

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

    pub fn sync_status_label(&self) -> &'static str {
        self.sync.status.label()
    }

    pub fn sync_enabled(&self) -> bool {
        self.is_sync_enabled()
    }

    pub fn visible_sync_fields(&self) -> &'static [SyncSettingsField] {
        self.current_sync_fields()
    }

    pub fn sync_summary_lines(&self) -> Vec<String> {
        let mut lines = vec![format!("Status: {}", self.sync.status.label())];
        if let Some(user) = &self.sync.github_user {
            lines.push(format!("GitHub: {user}"));
        } else {
            lines.push("GitHub: not connected".to_string());
        }
        if self.is_sync_enabled() {
            if let Some(config) = &self.sync.file.config {
                lines.push(format!("Repo: {}/{}", config.owner, config.repo));
            }
            if let Some(last_success_at) = &self.sync.file.state.last_success_at {
                lines.push(format!("Last Sync: {last_success_at}"));
            }
        } else {
            lines.push("Repo: sync disabled".to_string());
        }
        if let Some(warning) = &self.sync.last_warning {
            lines.push(format!("Warning: {warning}"));
        }
        if let Some(error) = &self.sync.last_error {
            lines.push(format!("Error: {error}"));
        }
        lines
    }

    pub fn settings_help_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if self.is_sync_enabled() {
            lines.push("Sync is enabled for this machine.".to_string());
        } else {
            if config::github_client_id().is_none() {
                lines.push(
                    "GitHub sync is not configured. Add your GitHub OAuth app client ID in src/config.rs."
                        .to_string(),
                );
            }
            lines.push(
                "Connect GitHub, choose a private repo, and enter a sync password.".to_string(),
            );
        }
        if let Some(prompt) = &self.sync.pending_device_code {
            lines.push(format!("Visit: {}", prompt.verification_uri));
            lines.push(format!("Code: {}", prompt.user_code));
        }
        lines
    }

    pub fn masked_sync_value(&self, field: SyncSettingsField) -> String {
        match field {
            SyncSettingsField::Password => mask_secret(&self.sync_form.password_text()),
            SyncSettingsField::ConfirmPassword => {
                mask_secret(&self.sync_form.confirm_password_text())
            }
            SyncSettingsField::Owner => self.sync_form.owner_text(),
            SyncSettingsField::Repo => self.sync_form.repo_text(),
            SyncSettingsField::ConnectGitHub
            | SyncSettingsField::EnableSync
            | SyncSettingsField::SyncNow
            | SyncSettingsField::Disconnect => field.label().to_string(),
        }
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

fn mask_secret(value: &str) -> String {
    if value.is_empty() {
        String::new()
    } else {
        "*".repeat(value.chars().count().max(8))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    fn app_with_library() -> AppState {
        let dir = tempdir().unwrap();
        AppState::new(
            dir.path().join("library.json"),
            dir.path().join("sync.json"),
            LibraryFile::default(),
            SyncFile::default(),
        )
    }

    #[test]
    fn loads_selected_request_into_draft() {
        let dir = tempdir().unwrap();
        let request = sample_request();
        let app = AppState::new(
            dir.path().join("library.json"),
            dir.path().join("sync.json"),
            LibraryFile {
                version: crate::model::CURRENT_LIBRARY_VERSION,
                requests: vec![request.clone()],
            },
            SyncFile::default(),
        );

        assert_eq!(app.draft.url_text(), request.url);
        assert_eq!(app.draft.optional_title(), request.title);
    }

    #[test]
    fn saves_new_requests_to_library() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("library.json");
        let mut app = AppState::new(
            path.clone(),
            dir.path().join("sync.json"),
            LibraryFile::default(),
            SyncFile::default(),
        );

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
            dir.path().join("sync.json"),
            LibraryFile {
                version: crate::model::CURRENT_LIBRARY_VERSION,
                requests: vec![request],
            },
            SyncFile::default(),
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
        let (sender, _receiver) = event_channel();
        let mut app = app_with_library();

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
    fn arrow_keys_follow_request_layout_outside_edit_mode() {
        let (sender, _receiver) = event_channel();
        let mut app = app_with_library();

        app.request_field = RequestField::Method;
        app.handle_key_event(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE), &sender);
        assert_eq!(app.request_field, RequestField::Url);

        app.handle_key_event(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE), &sender);
        assert_eq!(app.request_field, RequestField::Method);

        app.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &sender);
        assert_eq!(app.request_field, RequestField::Headers);

        app.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE), &sender);
        assert_eq!(app.request_field, RequestField::Method);
    }

    #[test]
    fn method_changes_only_while_editing() {
        let (sender, _receiver) = event_channel();
        let mut app = app_with_library();

        app.request_field = RequestField::Method;
        let original = app.draft.method;

        app.handle_key_event(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE), &sender);
        assert_eq!(app.draft.method, original);
        assert_eq!(app.request_field, RequestField::Url);

        app.request_field = RequestField::Method;
        app.request_editing = true;
        app.handle_key_event(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE), &sender);
        assert_eq!(app.draft.method, original.next());
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
        let mut app = app_with_library();
        app.focus = Pane::Request;
        app.request_editing = true;
        app.request_field = RequestField::Url;

        app.handle_paste("https://example.com/api\nusers".to_string());

        assert_eq!(app.draft.url_text(), "https://example.com/api users");
        assert_eq!(app.draft.url.lines().len(), 1);
    }

    #[test]
    fn pastes_multiline_body_preserving_newlines() {
        let mut app = app_with_library();
        app.focus = Pane::Request;
        app.request_editing = true;
        app.request_field = RequestField::Body;

        app.handle_paste("{\r\n  \"ok\": true\r\n}".to_string());

        assert_eq!(app.draft.body_text(), "{\n  \"ok\": true\n}");
        assert_eq!(app.draft.body.lines().len(), 3);
    }

    #[test]
    fn g_opens_settings_from_main_screen() {
        let (sender, _receiver) = event_channel();
        let mut app = app_with_library();

        app.handle_key_event(
            KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE),
            &sender,
        );

        assert_eq!(app.screen, Screen::Settings);
        assert_eq!(app.settings.focus, SettingsFocus::Nav);
    }

    #[test]
    fn esc_closes_settings_after_exiting_edit_mode() {
        let (sender, _receiver) = event_channel();
        let mut app = app_with_library();
        app.screen = Screen::Settings;
        app.settings.focus = SettingsFocus::Detail;
        app.settings.sync_field = SyncSettingsField::Owner;
        app.settings.editing = true;

        app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &sender);
        assert!(!app.settings.editing);
        assert_eq!(app.screen, Screen::Settings);

        app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &sender);
        assert_eq!(app.screen, Screen::Main);
    }

    #[test]
    fn settings_tab_switches_focus() {
        let (sender, _receiver) = event_channel();
        let mut app = app_with_library();
        app.screen = Screen::Settings;

        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE), &sender);
        assert_eq!(app.settings.focus, SettingsFocus::Detail);

        app.handle_key_event(
            KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
            &sender,
        );
        assert_eq!(app.settings.focus, SettingsFocus::Nav);
    }

    #[test]
    fn settings_detail_navigation_moves_between_sync_fields() {
        let (sender, _receiver) = event_channel();
        let mut app = app_with_library();
        app.screen = Screen::Settings;
        app.settings.focus = SettingsFocus::Detail;

        app.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &sender);
        assert_eq!(app.settings.sync_field, SyncSettingsField::Owner);

        app.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &sender);
        assert_eq!(app.settings.sync_field, SyncSettingsField::Repo);

        app.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE), &sender);
        assert_eq!(app.settings.sync_field, SyncSettingsField::Owner);
    }

    #[test]
    fn g_does_not_open_settings_while_editing_request_text() {
        let (sender, _receiver) = event_channel();
        let mut app = app_with_library();
        app.request_editing = true;

        app.handle_key_event(
            KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE),
            &sender,
        );

        assert_eq!(app.screen, Screen::Main);
    }
}
