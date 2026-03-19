use serde::{Deserialize, Serialize};
use std::fmt;
use url::Url;
use uuid::Uuid;

pub const CURRENT_LIBRARY_VERSION: u32 = 1;

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestInput {
    pub title: Option<String>,
    pub method: HttpMethod,
    pub url: String,
    pub headers: Vec<HeaderEntry>,
    pub json_body: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LibraryFile {
    pub version: u32,
    pub requests: Vec<SavedRequest>,
}

impl Default for LibraryFile {
    fn default() -> Self {
        Self {
            version: CURRENT_LIBRARY_VERSION,
            requests: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResponseData {
    pub status_code: u16,
    pub reason: Option<String>,
    pub elapsed_ms: u128,
    pub headers: Vec<HeaderEntry>,
    pub body: ResponseBody,
}

impl ResponseData {
    pub fn display_text(&self) -> String {
        let reason = self.reason.as_deref().unwrap_or("Unknown");
        let header_text = if self.headers.is_empty() {
            "<none>".to_string()
        } else {
            self.headers
                .iter()
                .map(|header| format!("{}: {}", header.name, header.value))
                .collect::<Vec<_>>()
                .join("\n")
        };

        format!(
            "Status: {} {}\nTime: {} ms\n\nHeaders\n{}\n\nBody\n{}",
            self.status_code,
            reason,
            self.elapsed_ms,
            header_text,
            self.body.display_text()
        )
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
