use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt;
use url::Url;
use uuid::Uuid;

pub const CURRENT_LIBRARY_VERSION: u32 = 2;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum HttpMethod {
    #[default]
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Head,
    Options,
}

impl HttpMethod {
    pub const ALL: [Self; 7] = [
        Self::Get,
        Self::Post,
        Self::Put,
        Self::Patch,
        Self::Delete,
        Self::Head,
        Self::Options,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
            Self::Head => "HEAD",
            Self::Options => "OPTIONS",
        }
    }

    pub fn next(self) -> Self {
        let index = Self::ALL
            .iter()
            .position(|method| *method == self)
            .unwrap_or(0);
        Self::ALL[(index + 1) % Self::ALL.len()]
    }

    pub fn previous(self) -> Self {
        let index = Self::ALL
            .iter()
            .position(|method| *method == self)
            .unwrap_or(0);
        let previous = if index == 0 {
            Self::ALL.len() - 1
        } else {
            index - 1
        };
        Self::ALL[previous]
    }
}

impl fmt::Display for HttpMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HeaderEntry {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SavedRequest {
    pub id: Uuid,
    #[serde(default)]
    pub folder_id: Option<Uuid>,
    pub title: Option<String>,
    pub method: HttpMethod,
    pub url: String,
    pub headers: Vec<HeaderEntry>,
    pub json_body: String,
}

impl SavedRequest {
    pub fn display_name(&self) -> String {
        match self
            .title
            .as_deref()
            .map(str::trim)
            .filter(|title| !title.is_empty())
        {
            Some(title) => format!("{title} [{}]", self.method),
            None => format!("{} {}", self.method, self.url),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SavedFolder {
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub parent_id: Option<Uuid>,
}

impl SavedFolder {
    pub fn display_name(&self) -> String {
        self.name.trim().to_string()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestInput {
    pub title: Option<String>,
    pub method: HttpMethod,
    pub url: String,
    pub headers: Vec<HeaderEntry>,
    pub json_body: String,
}

impl RequestInput {
    pub fn display_label(&self) -> String {
        match self
            .title
            .as_deref()
            .map(str::trim)
            .filter(|title| !title.is_empty())
        {
            Some(title) => format!("{title} [{}]", self.method),
            None => format!("{} {}", self.method, self.url),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct LibraryData {
    #[serde(default)]
    pub folders: Vec<SavedFolder>,
    #[serde(default)]
    pub requests: Vec<SavedRequest>,
}

impl LibraryData {
    pub fn normalized(mut self) -> Self {
        self.normalize();
        self
    }

    pub fn normalize(&mut self) {
        let parent_lookup = self
            .folders
            .iter()
            .map(|folder| (folder.id, folder.parent_id))
            .collect::<HashMap<_, _>>();
        let folder_ids = parent_lookup.keys().copied().collect::<HashSet<_>>();

        for folder in &mut self.folders {
            if !folder_parent_is_valid(folder.id, folder.parent_id, &parent_lookup, &folder_ids) {
                folder.parent_id = None;
            }
            folder.name = folder.display_name();
        }

        for request in &mut self.requests {
            if !request
                .folder_id
                .is_none_or(|folder_id| folder_ids.contains(&folder_id))
            {
                request.folder_id = None;
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LibraryFile {
    pub version: u32,
    #[serde(default)]
    pub folders: Vec<SavedFolder>,
    #[serde(default)]
    pub requests: Vec<SavedRequest>,
}

impl Default for LibraryFile {
    fn default() -> Self {
        Self {
            version: CURRENT_LIBRARY_VERSION,
            folders: Vec::new(),
            requests: Vec::new(),
        }
    }
}

impl From<LibraryFile> for LibraryData {
    fn from(file: LibraryFile) -> Self {
        Self {
            folders: file.folders,
            requests: file.requests,
        }
    }
}

impl From<LibraryData> for LibraryFile {
    fn from(library: LibraryData) -> Self {
        Self {
            version: CURRENT_LIBRARY_VERSION,
            folders: library.folders,
            requests: library.requests,
        }
    }
}

fn folder_parent_is_valid(
    folder_id: Uuid,
    parent_id: Option<Uuid>,
    parent_lookup: &HashMap<Uuid, Option<Uuid>>,
    folder_ids: &HashSet<Uuid>,
) -> bool {
    let mut seen = HashSet::new();
    let mut current = parent_id;

    while let Some(id) = current {
        if id == folder_id || !folder_ids.contains(&id) || !seen.insert(id) {
            return false;
        }
        current = parent_lookup.get(&id).copied().flatten();
    }

    true
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResponseData {
    pub status_code: u16,
    pub reason: Option<String>,
    pub elapsed_ms: u128,
    pub content_type: Option<String>,
    pub body_bytes: usize,
    pub headers: Vec<HeaderEntry>,
    pub body: ResponseBody,
    pub trace: ResponseTrace,
}

impl ResponseData {
    pub fn headers_text(&self) -> String {
        if self.headers.is_empty() {
            "<none>".to_string()
        } else {
            self.headers
                .iter()
                .map(|header| format!("{}: {}", header.name, header.value))
                .collect::<Vec<_>>()
                .join("\n")
        }
    }

    pub fn body_text(&self) -> String {
        let mut output = self.body.display_text().to_string();
        if let Some(suffix) = self.body.detail_suffix() {
            output.push_str(&suffix);
        }
        output
    }

    pub fn display_text(&self) -> String {
        let reason = self.reason.as_deref().unwrap_or("Unknown");
        let header_text = self.headers_text();
        let content_type = self.content_type.as_deref().unwrap_or("unknown");

        format!(
            "Status: {} {}\nTime: {} ms\nType: {}\nSize: {} bytes\n\nHeaders\n{}\n\nBody\n{}",
            self.status_code,
            reason,
            self.elapsed_ms,
            content_type,
            self.body_bytes,
            header_text,
            self.body_text()
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResponseTrace {
    pub trace_id: Uuid,
    pub label: String,
    pub method: HttpMethod,
    pub url: String,
    pub state: TraceState,
    pub status_code: Option<u16>,
    pub reason: Option<String>,
    pub content_length: Option<u64>,
    pub uploaded_bytes: u64,
    pub downloaded_bytes: u64,
    pub upload_speed_bytes_per_sec: u64,
    pub download_speed_bytes_per_sec: u64,
    pub name_lookup_time_ms: Option<u128>,
    pub connect_time_ms: Option<u128>,
    pub secure_connect_time_ms: Option<u128>,
    pub transfer_start_time_ms: Option<u128>,
    pub transfer_time_ms: Option<u128>,
    pub total_time_ms: Option<u128>,
    pub redirect_time_ms: Option<u128>,
    pub samples: Vec<TraceSample>,
    pub error: Option<String>,
}

impl ResponseTrace {
    pub fn new(request: &RequestInput, trace_id: Uuid) -> Self {
        Self {
            trace_id,
            label: request.display_label(),
            method: request.method,
            url: request.url.clone(),
            state: TraceState::Pending,
            status_code: None,
            reason: None,
            content_length: None,
            uploaded_bytes: 0,
            downloaded_bytes: 0,
            upload_speed_bytes_per_sec: 0,
            download_speed_bytes_per_sec: 0,
            name_lookup_time_ms: None,
            connect_time_ms: None,
            secure_connect_time_ms: None,
            transfer_start_time_ms: None,
            transfer_time_ms: None,
            total_time_ms: None,
            redirect_time_ms: None,
            samples: Vec::new(),
            error: None,
        }
    }

    pub fn apply_head(
        &mut self,
        status_code: u16,
        reason: Option<String>,
        content_length: Option<u64>,
    ) {
        self.state = TraceState::Receiving;
        self.status_code = Some(status_code);
        self.reason = reason;
        self.content_length = content_length;
    }

    pub fn apply_metrics_snapshot(&mut self, snapshot: &TraceMetricsSnapshot) {
        self.state = if matches!(self.state, TraceState::Pending) {
            TraceState::Receiving
        } else {
            self.state
        };
        self.uploaded_bytes = snapshot.uploaded_bytes;
        self.downloaded_bytes = snapshot.downloaded_bytes;
        self.upload_speed_bytes_per_sec = snapshot.upload_speed_bytes_per_sec;
        self.download_speed_bytes_per_sec = snapshot.download_speed_bytes_per_sec;
        self.name_lookup_time_ms = snapshot.name_lookup_time_ms;
        self.connect_time_ms = snapshot.connect_time_ms;
        self.secure_connect_time_ms = snapshot.secure_connect_time_ms;
        self.transfer_start_time_ms = snapshot.transfer_start_time_ms;
        self.transfer_time_ms = snapshot.transfer_time_ms;
        self.total_time_ms = snapshot.total_time_ms;
        self.redirect_time_ms = snapshot.redirect_time_ms;
        self.samples.push(snapshot.sample());
        if self.samples.len() > 240 {
            let overflow = self.samples.len() - 240;
            self.samples.drain(0..overflow);
        }
    }

    pub fn mark_complete(&mut self, elapsed_ms: u128) {
        self.state = TraceState::Complete;
        self.total_time_ms = Some(elapsed_ms);
    }

    pub fn mark_failed(&mut self, error: String) {
        self.state = TraceState::Failed;
        self.error = Some(error);
    }

    pub fn total_time_ms(&self) -> u128 {
        self.total_time_ms
            .or(self
                .transfer_start_time_ms
                .map(|start| start + self.transfer_time_ms.unwrap_or_default()))
            .unwrap_or(0)
    }

    pub fn max_sample_speed_bytes_per_sec(&self) -> u64 {
        self.samples
            .iter()
            .map(TraceSample::peak_speed_bytes_per_sec)
            .max()
            .unwrap_or_else(|| {
                self.download_speed_bytes_per_sec
                    .max(self.upload_speed_bytes_per_sec)
            })
    }

    pub fn waterfall_phases(&self) -> Vec<TracePhaseBar> {
        let redirect_end = self.redirect_time_ms.unwrap_or(0);
        let name_lookup_end = self.name_lookup_time_ms.unwrap_or(redirect_end);
        let connect_end = self.connect_time_ms.unwrap_or(name_lookup_end);
        let secure_connect_end = self.secure_connect_time_ms.unwrap_or(connect_end);
        let transfer_start_end = self.transfer_start_time_ms.unwrap_or(secure_connect_end);
        let total_end = self.total_time_ms().max(transfer_start_end);

        [
            ("Redirect", 0, redirect_end),
            ("DNS", redirect_end, name_lookup_end),
            ("TCP", name_lookup_end, connect_end),
            ("TLS", connect_end, secure_connect_end),
            ("Wait", secure_connect_end, transfer_start_end),
            ("Recv", transfer_start_end, total_end),
        ]
        .into_iter()
        .map(|(label, start_ms, end_ms)| TracePhaseBar {
            label,
            start_ms,
            end_ms,
        })
        .collect()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TraceState {
    Pending,
    Receiving,
    Complete,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TraceSample {
    pub at_ms: u128,
    pub uploaded_bytes: u64,
    pub downloaded_bytes: u64,
    pub upload_speed_bytes_per_sec: u64,
    pub download_speed_bytes_per_sec: u64,
}

impl TraceSample {
    pub fn peak_speed_bytes_per_sec(&self) -> u64 {
        self.upload_speed_bytes_per_sec
            .max(self.download_speed_bytes_per_sec)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraceMetricsSnapshot {
    pub at_ms: u128,
    pub uploaded_bytes: u64,
    pub downloaded_bytes: u64,
    pub upload_speed_bytes_per_sec: u64,
    pub download_speed_bytes_per_sec: u64,
    pub name_lookup_time_ms: Option<u128>,
    pub connect_time_ms: Option<u128>,
    pub secure_connect_time_ms: Option<u128>,
    pub transfer_start_time_ms: Option<u128>,
    pub transfer_time_ms: Option<u128>,
    pub total_time_ms: Option<u128>,
    pub redirect_time_ms: Option<u128>,
}

impl TraceMetricsSnapshot {
    pub fn sample(&self) -> TraceSample {
        TraceSample {
            at_ms: self.at_ms,
            uploaded_bytes: self.uploaded_bytes,
            downloaded_bytes: self.downloaded_bytes,
            upload_speed_bytes_per_sec: self.upload_speed_bytes_per_sec,
            download_speed_bytes_per_sec: self.download_speed_bytes_per_sec,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TracePhaseBar {
    pub label: &'static str,
    pub start_ms: u128,
    pub end_ms: u128,
}

impl TracePhaseBar {
    pub fn duration_ms(self) -> u128 {
        self.end_ms.saturating_sub(self.start_ms)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResponseBody {
    PrettyJson(String),
    Text(String),
    BinarySummary { bytes: usize },
}

impl ResponseBody {
    pub fn display_text(&self) -> &str {
        match self {
            Self::PrettyJson(text) | Self::Text(text) => text.as_str(),
            Self::BinarySummary { .. } => "<binary response body not rendered>",
        }
    }

    pub fn detail_suffix(&self) -> Option<String> {
        match self {
            Self::BinarySummary { bytes } => Some(format!(" ({} bytes)", bytes)),
            _ => None,
        }
    }
}

pub fn headers_to_text(headers: &[HeaderEntry]) -> String {
    headers
        .iter()
        .map(|header| format!("{}: {}", header.name, header.value))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn parse_header_lines(input: &str) -> Result<Vec<HeaderEntry>, String> {
    let mut headers = Vec::new();

    for (index, line) in input.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let (name, value) = trimmed
            .split_once(':')
            .ok_or_else(|| format!("Header line {} must use `Key: Value` format.", index + 1))?;

        let name = name.trim();
        if name.is_empty() {
            return Err(format!(
                "Header line {} is missing a header name.",
                index + 1
            ));
        }

        headers.push(HeaderEntry {
            name: name.to_string(),
            value: value.trim().to_string(),
        });
    }

    Ok(headers)
}

pub fn validate_json_body(input: &str) -> Result<(), String> {
    if input.trim().is_empty() {
        return Ok(());
    }

    serde_json::from_str::<serde_json::Value>(input)
        .map(|_| ())
        .map_err(|error| format!("JSON body is invalid: {error}"))
}

pub fn validate_url(input: &str) -> Result<(), String> {
    if input.trim().is_empty() {
        return Err("URL is required before sending a request.".to_string());
    }

    let parsed = Url::parse(input).map_err(|error| format!("URL is invalid: {error}"))?;
    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        return Err("Only http:// and https:// URLs are supported.".to_string());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_headers_with_blank_lines() {
        let headers =
            parse_header_lines("Accept: application/json\n\nAuthorization: Bearer token").unwrap();
        assert_eq!(
            headers,
            vec![
                HeaderEntry {
                    name: "Accept".to_string(),
                    value: "application/json".to_string(),
                },
                HeaderEntry {
                    name: "Authorization".to_string(),
                    value: "Bearer token".to_string(),
                },
            ]
        );
    }

    #[test]
    fn rejects_header_without_separator() {
        let error = parse_header_lines("Accept application/json").unwrap_err();
        assert!(error.contains("Key: Value"));
    }

    #[test]
    fn allows_duplicate_headers() {
        let headers = parse_header_lines("Set-Cookie: a=1\nSet-Cookie: b=2").unwrap();
        assert_eq!(headers.len(), 2);
    }

    #[test]
    fn validates_json_body() {
        validate_json_body(r#"{"ok":true}"#).unwrap();
        let error = validate_json_body("{oops").unwrap_err();
        assert!(error.contains("JSON body is invalid"));
    }

    #[test]
    fn validates_http_urls() {
        validate_url("https://example.com").unwrap();
        assert!(validate_url("").is_err());
        assert!(validate_url("ftp://example.com").is_err());
    }
}
