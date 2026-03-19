use crate::model::ResponseData;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

#[derive(Debug)]
pub enum AppEvent {
    NetworkResponse(Result<ResponseData, String>),
}

pub type AppEventReceiver = UnboundedReceiver<AppEvent>;
pub type AppEventSender = UnboundedSender<AppEvent>;

pub fn event_channel() -> (AppEventSender, AppEventReceiver) {
    mpsc::unbounded_channel()
}
