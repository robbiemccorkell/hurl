use crate::events::{AppEvent, AppEventSender};
use crate::model::{
    HeaderEntry, HttpMethod, RequestInput, ResponseBody, ResponseData, ResponseTrace,
    TraceMetricsSnapshot, validate_json_body, validate_url,
};
use futures_lite::io::AsyncReadExt;
use isahc::config::{Configurable, RedirectPolicy};
use isahc::http::header::{CONTENT_LENGTH, CONTENT_TYPE, HeaderName, HeaderValue};
use isahc::http::{Method, Request};
use isahc::{AsyncBody, HttpClient, Metrics, ResponseExt};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use uuid::Uuid;

static HTTP_CLIENT: OnceLock<Result<HttpClient, String>> = OnceLock::new();

pub async fn send_request(request: RequestInput, sender: AppEventSender) {
    let trace_id = Uuid::new_v4();
    let trace = ResponseTrace::new(&request, trace_id);
    let _ = sender.send(AppEvent::NetworkStarted(trace));

    let result = send_request_inner(request, trace_id, sender.clone()).await;
    let _ = sender.send(AppEvent::NetworkResponse { trace_id, result });
}

async fn send_request_inner(
    request: RequestInput,
    trace_id: Uuid,
    sender: AppEventSender,
) -> Result<ResponseData, String> {
    validate_url(&request.url)?;
    validate_json_body(&request.json_body)?;

    let client = http_client()?;
    let built_request = build_request(&request)?;
    let start = Instant::now();
    let mut response = client
        .send_async(built_request)
        .await
        .map_err(|error| format!("Request failed: {error}"))?;

    let status = response.status();
    let reason = status.canonical_reason().map(str::to_string);
    let headers = collect_headers(response.headers());
    let content_type = header_value(&headers, CONTENT_TYPE.as_str());
    let content_length = header_value(&headers, CONTENT_LENGTH.as_str())
        .and_then(|value| value.parse::<u64>().ok())
        .or_else(|| response.body().len());

    let _ = sender.send(AppEvent::NetworkHead {
        trace_id,
        status_code: status.as_u16(),
        reason: reason.clone(),
        content_length,
    });

    let metrics = response.metrics().cloned();
    let done = Arc::new(AtomicBool::new(false));
    let sampler = metrics.clone().map(|metrics| {
        tokio::spawn(sample_metrics(
            trace_id,
            metrics,
            sender.clone(),
            done.clone(),
        ))
    });

    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 8_192];
    loop {
        let read = response
            .body_mut()
            .read(&mut buffer)
            .await
            .map_err(|error| format!("Failed to read response body: {error}"))?;
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..read]);
    }

    done.store(true, Ordering::Relaxed);
    if let Some(sampler) = sampler {
        let _ = sampler.await;
    }

    let mut trace = ResponseTrace::new(&request, trace_id);
    trace.apply_head(status.as_u16(), reason.clone(), content_length);
    if let Some(metrics) = metrics.as_ref() {
        let snapshot = metrics_snapshot(metrics);
        let _ = sender.send(AppEvent::NetworkTraceSample {
            trace_id,
            snapshot: snapshot.clone(),
        });
        trace.apply_metrics_snapshot(&snapshot);
    }
    let elapsed_ms = metrics
        .as_ref()
        .map(|metrics| metrics.total_time().as_millis())
        .unwrap_or_else(|| start.elapsed().as_millis());
    trace.downloaded_bytes = bytes.len() as u64;
    trace.mark_complete(elapsed_ms.max(1));

    Ok(ResponseData {
        status_code: status.as_u16(),
        reason,
        elapsed_ms,
        content_type,
        body_bytes: bytes.len(),
        headers,
        body: format_response_body(bytes.as_ref()),
        trace,
    })
}

fn http_client() -> Result<&'static HttpClient, String> {
    HTTP_CLIENT
        .get_or_init(|| {
            HttpClient::builder()
                .redirect_policy(RedirectPolicy::Limit(10))
                .build()
                .map_err(|error| format!("Failed to build HTTP client: {error}"))
        })
        .as_ref()
        .map_err(|error| error.clone())
}

fn build_request(request: &RequestInput) -> Result<Request<AsyncBody>, String> {
    let mut builder = Request::builder()
        .method(to_http_method(request.method))
        .uri(&request.url)
        .metrics(true);

    for header in &request.headers {
        let name = HeaderName::from_str(header.name.trim())
            .map_err(|error| format!("Header `{}` is invalid: {error}", header.name))?;
        let value = HeaderValue::from_str(header.value.trim())
            .map_err(|error| format!("Header `{}` has an invalid value: {error}", header.name))?;
        builder = builder.header(name, value);
    }

    let body = if request.json_body.trim().is_empty() {
        AsyncBody::empty()
    } else {
        AsyncBody::from(request.json_body.clone())
    };

    builder
        .body(body)
        .map_err(|error| format!("Failed to build request: {error}"))
}

fn to_http_method(method: HttpMethod) -> Method {
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

fn collect_headers(headers: &isahc::http::HeaderMap) -> Vec<HeaderEntry> {
    headers
        .iter()
        .map(|(name, value)| HeaderEntry {
            name: name.as_str().to_string(),
            value: value.to_str().unwrap_or("<non-utf8 header>").to_string(),
        })
        .collect()
}

fn header_value(headers: &[HeaderEntry], name: &str) -> Option<String> {
    headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case(name))
        .map(|header| header.value.clone())
}

async fn sample_metrics(
    trace_id: Uuid,
    metrics: Metrics,
    sender: AppEventSender,
    done: Arc<AtomicBool>,
) {
    loop {
        let _ = sender.send(AppEvent::NetworkTraceSample {
            trace_id,
            snapshot: metrics_snapshot(&metrics),
        });
        if done.load(Ordering::Relaxed) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
}

fn metrics_snapshot(metrics: &Metrics) -> TraceMetricsSnapshot {
    let (uploaded_bytes, _) = metrics.upload_progress();
    let (downloaded_bytes, _) = metrics.download_progress();
    TraceMetricsSnapshot {
        at_ms: metrics.total_time().as_millis(),
        uploaded_bytes,
        downloaded_bytes,
        upload_speed_bytes_per_sec: metrics.upload_speed().round() as u64,
        download_speed_bytes_per_sec: metrics.download_speed().round() as u64,
        name_lookup_time_ms: Some(metrics.name_lookup_time().as_millis()),
        connect_time_ms: Some(metrics.connect_time().as_millis()),
        secure_connect_time_ms: Some(metrics.secure_connect_time().as_millis()),
        transfer_start_time_ms: Some(metrics.transfer_start_time().as_millis()),
        transfer_time_ms: Some(metrics.transfer_time().as_millis()),
        total_time_ms: Some(metrics.total_time().as_millis()),
        redirect_time_ms: Some(metrics.redirect_time().as_millis()),
    }
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
    use crate::events::{AppEvent, event_channel};
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
    async fn sends_requests_to_mock_server_and_emits_trace_events() {
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

        let (sender, mut receiver) = event_channel();
        send_request(
            RequestInput {
                title: Some("Submit".to_string()),
                method: HttpMethod::Post,
                url: format!("{}/submit", server.base_url()),
                headers: vec![HeaderEntry {
                    name: "accept".to_string(),
                    value: "application/json".to_string(),
                }],
                json_body: r#"{"hello":"world"}"#.to_string(),
            },
            sender,
        )
        .await;

        mock.assert();

        let mut saw_started = false;
        let mut saw_head = false;
        let mut saw_sample = false;
        let mut response = None;

        while let Ok(event) = receiver.try_recv() {
            match event {
                AppEvent::NetworkStarted(_) => saw_started = true,
                AppEvent::NetworkHead { .. } => saw_head = true,
                AppEvent::NetworkTraceSample { .. } => saw_sample = true,
                AppEvent::NetworkResponse { result, .. } => response = Some(result.unwrap()),
                _ => {}
            }
        }

        let response = response.expect("response event");
        assert!(saw_started);
        assert!(saw_head);
        assert!(saw_sample);
        assert_eq!(response.status_code, 201);
        assert_eq!(response.content_type.as_deref(), Some("application/json"));
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
