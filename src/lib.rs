pub mod app;
pub mod cli;
pub mod config;
pub mod events;
pub mod highlight;
pub mod model;
pub mod network;
pub mod storage;
pub mod sync;
pub mod ui;

pub use app::run as run_tui;
pub use cli::run;
