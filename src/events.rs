use crate::model::{ResponseData, ResponseTrace, TraceMetricsSnapshot};
use crate::sync::{DeviceCodePrompt, GitHubIdentity, SyncRunOutput};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyncOperation {
    Startup,
    Save,
    Manual,
    Enable,
}

#[derive(Debug)]
pub enum AppEvent {
    NetworkStarted(ResponseTrace),
    NetworkHead {
        trace_id: Uuid,
        status_code: u16,
        reason: Option<String>,
        content_length: Option<u64>,
    },
    NetworkTraceSample {
        trace_id: Uuid,
        snapshot: TraceMetricsSnapshot,
    },
    NetworkResponse {
        trace_id: Uuid,
        result: Result<ResponseData, String>,
    },
    GitHubDeviceCode(Result<DeviceCodePrompt, String>),
    GitHubAuthComplete(Result<GitHubIdentity, String>),
    SyncFinished {
        operation: SyncOperation,
        base_revision: u64,
        result: Result<SyncRunOutput, String>,
    },
}

pub type AppEventReceiver = UnboundedReceiver<AppEvent>;
pub type AppEventSender = UnboundedSender<AppEvent>;

pub fn event_channel() -> (AppEventSender, AppEventReceiver) {
    mpsc::unbounded_channel()
}
