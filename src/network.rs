use crate::model::{
    HeaderEntry, HttpMethod, RequestInput, ResponseBody, ResponseData, validate_json_body,
    validate_url,
};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::{Client, Method};
use std::str::FromStr;
use std::time::Instant;

pub async fn send_request(request: RequestInput) -> Result<ResponseData, String> {
    validate_url(&request.url)?;
    validate_json_body(&request.json_body)?;

    let client = Client::builder()
        .build()
        .map_err(|error| format!("Failed to build HTTP client: {error}"))?;

    let mut builder = client.request(to_reqwest_method(request.method), &request.url);
    builder = builder.headers(build_headers(&request.headers)?);

    if !request.json_body.trim().is_empty() {
        builder = builder.body(request.json_body.clone());
    }

    let start = Instant::now();
    let response = builder
        .send()
        .await
        .map_err(|error| format!("Request failed: {error}"))?;
    let elapsed_ms = start.elapsed().as_millis();

    let status = response.status();
    let headers = response
        .headers()
        .iter()
        .map(|(name, value)| HeaderEntry {
            name: name.as_str().to_string(),
            value: value.to_str().unwrap_or("<non-utf8 header>").to_string(),
        })
        .collect::<Vec<_>>();
    let bytes = response
        .bytes()
        .await
        .map_err(|error| format!("Failed to read response body: {error}"))?;

    Ok(ResponseData {
        status_code: status.as_u16(),
        reason: status.canonical_reason().map(str::to_string),
        elapsed_ms,
        headers,
        body: format_response_body(bytes.as_ref()),
    })
}

fn to_reqwest_method(method: HttpMethod) -> Method {
    match method {
        HttpMethod::Get => Method::GET,
        HttpMethod::Post => Method::POST,
        HttpMethod::Put => Method::PUT,
        HttpMethod::Patch => Method::PATCH,
        HttpMethod::Delete => Method::DELETE,
        HttpMethod::Head => Method::HEAD,
        HttpMethod::Options => Method::OPTIONS,
    }
}

fn build_headers(headers: &[HeaderEntry]) -> Result<HeaderMap, String> {
    let mut map = HeaderMap::new();

    for header in headers {
        let name = HeaderName::from_str(header.name.trim())
            .map_err(|error| format!("Header `{}` is invalid: {error}", header.name))?;
        let value = HeaderValue::from_str(header.value.trim())
            .map_err(|error| format!("Header `{}` has an invalid value: {error}", header.name))?;
        map.append(name, value);
    }

    Ok(map)
}

fn format_response_body(bytes: &[u8]) -> ResponseBody {
    if bytes.is_empty() {
        return ResponseBody::Text("<empty>".to_string());
    }

    let text = match String::from_utf8(bytes.to_vec()) {
        Ok(text) => text,
        Err(_) => {
            return ResponseBody::BinarySummary { bytes: bytes.len() };
        }
    };

    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(json) => serde_json::to_string_pretty(&json)
            .map(ResponseBody::PrettyJson)
            .unwrap_or(ResponseBody::Text(text)),
        Err(_) => ResponseBody::Text(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::Method::POST;
    use httpmock::MockServer;

    #[test]
    fn pretty_prints_json_bodies() {
        let body = format_response_body(br#"{"hello":"world"}"#);
        assert!(
            matches!(body, ResponseBody::PrettyJson(text) if text.contains("\"hello\": \"world\""))
        );
    }

    #[test]
    fn keeps_plain_text_bodies() {
        let body = format_response_body(b"plain text");
        assert_eq!(body, ResponseBody::Text("plain text".to_string()));
    }

    #[test]
    fn summarizes_binary_bodies() {
        let body = format_response_body(&[0xff, 0x00, 0x10]);
        assert_eq!(body, ResponseBody::BinarySummary { bytes: 3 });
    }

    #[tokio::test]
    async fn sends_requests_to_mock_server() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/submit")
                .header("accept", "application/json")
                .body(r#"{"hello":"world"}"#);
            then.status(201)
                .header("content-type", "application/json")
                .body(r#"{"ok":true}"#);
        });

        let response = send_request(RequestInput {
            title: Some("Submit".to_string()),
            method: HttpMethod::Post,
            url: format!("{}/submit", server.base_url()),
            headers: vec![HeaderEntry {
                name: "accept".to_string(),
                value: "application/json".to_string(),
            }],
            json_body: r#"{"hello":"world"}"#.to_string(),
        })
        .await
        .unwrap();

        mock.assert();
        assert_eq!(response.status_code, 201);
        assert!(response.elapsed_ms <= 5_000);
        assert!(
            response
                .headers
                .iter()
                .any(|header| header.name == "content-type")
        );
        assert!(
            matches!(response.body, ResponseBody::PrettyJson(text) if text.contains("\"ok\": true"))
        );
    }
}
