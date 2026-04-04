use crate::model::LibraryFile;
use crate::sync::SyncFile;
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

const DEMO_LIBRARY_JSON: &str = include_str!("../assets/demo-library.json");
pub const DEMO_DEFAULT_REQUEST_ID: &str = "5f8d0f16-1c8d-4d17-8b5a-b0b58bf02b52";

pub struct DemoSession {
    pub storage_path: PathBuf,
    pub sync_path: PathBuf,
    pub library: LibraryFile,
    pub sync_file: SyncFile,
    pub default_request_id: Uuid,
    _workspace: DemoWorkspace,
}

impl DemoSession {
    pub fn start() -> Result<Self> {
        let library = load_demo_library()?;
        let sync_file = SyncFile::default();
        let workspace = DemoWorkspace::new()?;
        let storage_path = workspace.root().join("library.json");
        let sync_path = workspace.root().join("sync.json");

        Ok(Self {
            storage_path,
            sync_path,
            library,
            sync_file,
            default_request_id: Uuid::parse_str(DEMO_DEFAULT_REQUEST_ID)
                .expect("demo default request id should be valid"),
            _workspace: workspace,
        })
    }
}

fn load_demo_library() -> Result<LibraryFile> {
    serde_json::from_str(DEMO_LIBRARY_JSON).context("Failed to parse the embedded demo library.")
}

struct DemoWorkspace {
    root: PathBuf,
}

impl DemoWorkspace {
    fn new() -> Result<Self> {
        let root = std::env::temp_dir().join(format!("hurl-demo-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).context("Failed to create the demo workspace directory.")?;
        Ok(Self { root })
    }

    fn root(&self) -> &Path {
        &self.root
    }
}

impl Drop for DemoWorkspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_embedded_demo_library_with_expected_default_request() {
        let library = load_demo_library().unwrap();

        assert!(
            library
                .requests
                .iter()
                .any(|request| request.id == Uuid::parse_str(DEMO_DEFAULT_REQUEST_ID).unwrap())
        );
    }

    #[test]
    fn demo_library_uses_public_https_test_endpoints() {
        let library = load_demo_library().unwrap();

        assert!(library.requests.iter().all(|request| {
            request.url.starts_with("https://")
                && (request.url.contains("postman-echo.com") || request.url.contains("httpbin.org"))
        }));
    }
}
