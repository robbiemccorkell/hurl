use crate::model::{CURRENT_LIBRARY_VERSION, LibraryData, LibraryFile};
use crate::sync::{SYNC_FILE_VERSION, SyncFile};
use directories::ProjectDirs;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum StorageError {
    DirectoryUnavailable,
    Io(io::Error),
    Json(serde_json::Error),
    UnsupportedVersion(u32),
    UnsupportedSyncVersion(u32),
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DirectoryUnavailable => {
                f.write_str("Unable to determine the app config directory for hurl.")
            }
            Self::Io(error) => write!(f, "File system error: {error}"),
            Self::Json(error) => write!(f, "Failed to read library JSON: {error}"),
            Self::UnsupportedVersion(version) => {
                write!(f, "Library version {version} is not supported.")
            }
            Self::UnsupportedSyncVersion(version) => {
                write!(f, "Sync file version {version} is not supported.")
            }
        }
    }
}

impl std::error::Error for StorageError {}

impl From<io::Error> for StorageError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for StorageError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

pub fn library_path() -> Result<PathBuf, StorageError> {
    Ok(config_dir()?.join("library.json"))
}

pub fn sync_path() -> Result<PathBuf, StorageError> {
    Ok(config_dir()?.join("sync.json"))
}

fn config_dir() -> Result<PathBuf, StorageError> {
    let project_dirs =
        ProjectDirs::from("dev", "hurl", "hurl").ok_or(StorageError::DirectoryUnavailable)?;
    Ok(project_dirs.config_dir().to_path_buf())
}

pub fn load_library(path: &Path) -> Result<LibraryFile, StorageError> {
    if !path.exists() {
        return Ok(LibraryFile::default());
    }

    let content = fs::read_to_string(path)?;
    if content.trim().is_empty() {
        return Ok(LibraryFile::default());
    }

    let file: LibraryFile = serde_json::from_str(&content)?;
    match file.version {
        1 | CURRENT_LIBRARY_VERSION => {
            let library = LibraryData::from(file).normalized();
            Ok(LibraryFile::from(library))
        }
        version => Err(StorageError::UnsupportedVersion(version)),
    }
}

pub fn save_library(path: &Path, file: &LibraryFile) -> Result<(), StorageError> {
    save_json(path, file)
}

pub fn load_sync_file(path: &Path) -> Result<SyncFile, StorageError> {
    if !path.exists() {
        return Ok(SyncFile::default());
    }

    let content = fs::read_to_string(path)?;
    if content.trim().is_empty() {
        return Ok(SyncFile::default());
    }

    let file: SyncFile = serde_json::from_str(&content)?;
    if file.version != SYNC_FILE_VERSION {
        return Err(StorageError::UnsupportedSyncVersion(file.version));
    }

    Ok(file)
}

pub fn save_sync_file(path: &Path, file: &SyncFile) -> Result<(), StorageError> {
    save_json(path, file)
}

fn save_json<T: serde::Serialize>(path: &Path, file: &T) -> Result<(), StorageError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let content = serde_json::to_string_pretty(file)?;
    fs::write(path, content)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{HeaderEntry, HttpMethod, SavedRequest};
    use crate::sync::{SyncConfig, SyncState};
    use tempfile::tempdir;
    use uuid::Uuid;

    #[test]
    fn round_trips_library_json() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("library.json");
        let file = LibraryFile {
            version: CURRENT_LIBRARY_VERSION,
            folders: vec![],
            requests: vec![SavedRequest {
                id: Uuid::new_v4(),
                folder_id: None,
                title: Some("Example".to_string()),
                method: HttpMethod::Post,
                url: "https://example.com".to_string(),
                headers: vec![HeaderEntry {
                    name: "Accept".to_string(),
                    value: "application/json".to_string(),
                }],
                json_body: r#"{"hello":"world"}"#.to_string(),
            }],
        };

        save_library(&path, &file).unwrap();
        let loaded = load_library(&path).unwrap();

        assert_eq!(loaded, file);
    }

    #[test]
    fn errors_on_unsupported_version() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("library.json");
        fs::write(&path, r#"{"version":999,"requests":[]}"#).unwrap();

        let error = load_library(&path).unwrap_err();
        assert!(matches!(error, StorageError::UnsupportedVersion(999)));
    }

    #[test]
    fn missing_library_file_returns_empty_default() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("missing-library.json");

        let file = load_library(&path).unwrap();
        assert_eq!(file, LibraryFile::default());
    }

    #[test]
    fn migrates_flat_v1_library_files() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("library.json");
        fs::write(
            &path,
            r#"{
  "version": 1,
  "requests": [
    {
      "id": "00000000-0000-0000-0000-000000000001",
      "title": "Example",
      "method": "Get",
      "url": "https://example.com",
      "headers": [],
      "json_body": "{}"
    }
  ]
}"#,
        )
        .unwrap();

        let file = load_library(&path).unwrap();

        assert_eq!(file.version, CURRENT_LIBRARY_VERSION);
        assert!(file.folders.is_empty());
        assert_eq!(file.requests.len(), 1);
        assert_eq!(file.requests[0].folder_id, None);
    }

    #[test]
    fn round_trips_sync_json() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("sync.json");
        let file = SyncFile {
            version: SYNC_FILE_VERSION,
            config: Some(SyncConfig {
                enabled: true,
                owner: "robbie".to_string(),
                repo: "hurl-sync".to_string(),
                branch: "main".to_string(),
                github_user: "robbie".to_string(),
                device_id: Uuid::new_v4(),
            }),
            state: SyncState {
                last_head_sha: Some("abc123".to_string()),
                last_synced_hash: Default::default(),
                last_synced_folder_hash: Default::default(),
                last_success_at: Some("2026-03-20T12:00:00Z".to_string()),
                dirty: false,
            },
        };

        save_sync_file(&path, &file).unwrap();
        let loaded = load_sync_file(&path).unwrap();

        assert_eq!(loaded, file);
    }
}
