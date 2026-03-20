use crate::model::ResponseData;
use crate::sync::{DeviceCodePrompt, GitHubIdentity, SyncRunOutput};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyncOperation {
    Startup,
    Save,
    Manual,
    Enable,
}

#[derive(Debug)]
pub enum AppEvent {
    NetworkResponse(Result<ResponseData, String>),
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
