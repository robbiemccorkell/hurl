use crate::model::{LibraryData, SavedFolder, SavedRequest};
use anyhow::Context;
use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use keyring::{Entry, Error as KeyringError};
use rand::RngCore;
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue, USER_AGENT};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::time::Duration;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

pub const SYNC_FILE_VERSION: u32 = 1;
pub const DEFAULT_REPO_NAME: &str = "hurl-sync";
const GITHUB_API_BASE: &str = "https://api.github.com";
const GITHUB_DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
const GITHUB_ACCESS_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const TOKEN_SERVICE: &str = "hurl GitHub Sync Token";
const TOKEN_ACCOUNT: &str = "github-sync";
const PASSWORD_SERVICE: &str = "hurl Sync Password";
const PASSWORD_ACCOUNT: &str = "library-encryption";
const LEGACY_TOKEN_SERVICE: &str = "hurl.github.sync.token";
const LEGACY_PASSWORD_SERVICE: &str = "hurl.github.sync.password";
const LEGACY_SECRET_ACCOUNT: &str = "default";
const MANIFEST_PATH: &str = "manifest.json";
const FOLDERS_DIR: &str = "folders";
const REQUESTS_DIR: &str = "requests";
const ENVELOPE_VERSION: u32 = 1;
const ARGON2_MEMORY_KIB: u32 = 65_536;
const ARGON2_ITERATIONS: u32 = 3;
const ARGON2_PARALLELISM: u32 = 1;
const NONCE_BYTES: usize = 24;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct SyncFile {
    pub version: u32,
    pub config: Option<SyncConfig>,
    pub state: SyncState,
}

impl Default for SyncFile {
    fn default() -> Self {
        Self {
            version: SYNC_FILE_VERSION,
            config: None,
            state: SyncState::default(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct SyncConfig {
    pub enabled: bool,
    pub owner: String,
    pub repo: String,
    pub branch: String,
    pub github_user: String,
    pub device_id: Uuid,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            owner: String::new(),
            repo: DEFAULT_REPO_NAME.to_string(),
            branch: String::new(),
            github_user: String::new(),
            device_id: Uuid::new_v4(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct SyncState {
    pub last_head_sha: Option<String>,
    pub last_synced_hash: BTreeMap<Uuid, String>,
    #[serde(default)]
    pub last_synced_folder_hash: BTreeMap<Uuid, String>,
    pub last_success_at: Option<String>,
    pub dirty: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyncStatus {
    Off,
    Ready,
    Syncing,
    Dirty,
    Error,
}

impl SyncStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Ready => "Ready",
            Self::Syncing => "Syncing",
            Self::Dirty => "Dirty",
            Self::Error => "Error",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct RepoManifest {
    pub app: String,
    pub format_version: u32,
    pub kdf: String,
    pub kdf_params: KdfParams,
    pub cipher: String,
    pub salt: String,
    pub created_at: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct KdfParams {
    pub memory_kib: u32,
    pub iterations: u32,
    pub parallelism: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct EncryptedEnvelope {
    pub version: u32,
    pub nonce: String,
    pub ciphertext: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceCodePrompt {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in_seconds: u64,
    pub interval_seconds: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitHubIdentity {
    pub username: String,
    pub access_token: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SecretPersistence {
    Persisted,
    SessionOnly,
    Deleted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncRunOutput {
    pub config: SyncConfig,
    pub state: SyncState,
    pub library: LibraryData,
    pub imported_count: usize,
    pub uploaded_count: usize,
    pub conflict_count: usize,
    pub warning: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestMergeResult {
    pub requests: Vec<SavedRequest>,
    pub hashes: BTreeMap<Uuid, String>,
    pub upload_ids: Vec<Uuid>,
    pub imported_count: usize,
    pub conflict_count: usize,
    pub warning: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderMergeResult {
    pub folders: Vec<SavedFolder>,
    pub hashes: BTreeMap<Uuid, String>,
    pub upload_ids: Vec<Uuid>,
    pub imported_count: usize,
    pub warning: Option<String>,
}

#[derive(Clone, Debug)]
struct RemoteSnapshot {
    manifest: RepoManifest,
    requests: BTreeMap<Uuid, SavedRequest>,
    folders: BTreeMap<Uuid, SavedFolder>,
    request_shas: HashMap<Uuid, String>,
    folder_shas: HashMap<Uuid, String>,
    head_sha: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct RepoInfo {
    owner: RepoOwner,
    default_branch: String,
}

#[derive(Clone, Debug, Deserialize)]
struct RepoOwner {
    login: String,
}

#[derive(Clone, Debug, Deserialize)]
struct ContentFile {
    sha: String,
    content: Option<String>,
    encoding: Option<String>,
    path: String,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Clone, Debug, Deserialize)]
struct ContentListItem {
    path: String,
    name: String,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Clone, Debug, Serialize)]
struct CreateRepoBody<'a> {
    name: &'a str,
    private: bool,
    auto_init: bool,
}

#[derive(Clone, Debug, Serialize)]
struct DeviceCodeRequest<'a> {
    client_id: &'a str,
    scope: &'a str,
}

#[derive(Clone, Debug, Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    expires_in: u64,
    interval: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
struct AccessTokenRequest<'a> {
    client_id: &'a str,
    device_code: &'a str,
    grant_type: &'a str,
}

#[derive(Clone, Debug, Deserialize)]
struct AccessTokenResponse {
    access_token: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
    interval: Option<u64>,
}

#[derive(Clone, Debug, Deserialize)]
struct GitHubUser {
    login: String,
}

#[derive(Clone, Debug, Serialize)]
struct PutContentBody<'a> {
    message: &'a str,
    content: String,
    branch: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    sha: Option<&'a str>,
}

pub fn default_sync_file() -> SyncFile {
    SyncFile::default()
}

pub fn default_repo_name() -> &'static str {
    DEFAULT_REPO_NAME
}

pub fn load_access_token() -> Option<String> {
    load_secret(TOKEN_SERVICE, TOKEN_ACCOUNT).or_else(|| {
        let secret = load_secret(LEGACY_TOKEN_SERVICE, LEGACY_SECRET_ACCOUNT)?;
        let _ = store_secret(TOKEN_SERVICE, TOKEN_ACCOUNT, &secret);
        Some(secret)
    })
}

pub fn load_sync_password() -> Option<String> {
    load_secret(PASSWORD_SERVICE, PASSWORD_ACCOUNT).or_else(|| {
        let secret = load_secret(LEGACY_PASSWORD_SERVICE, LEGACY_SECRET_ACCOUNT)?;
        let _ = store_secret(PASSWORD_SERVICE, PASSWORD_ACCOUNT, &secret);
        Some(secret)
    })
}

pub fn store_access_token(token: &str) -> SecretPersistence {
    store_secret(TOKEN_SERVICE, TOKEN_ACCOUNT, token)
}

pub fn store_sync_password(password: &str) -> SecretPersistence {
    store_secret(PASSWORD_SERVICE, PASSWORD_ACCOUNT, password)
}

pub fn delete_access_token() -> SecretPersistence {
    let _ = delete_secret(LEGACY_TOKEN_SERVICE, LEGACY_SECRET_ACCOUNT);
    delete_secret(TOKEN_SERVICE, TOKEN_ACCOUNT)
}

pub fn delete_sync_password() -> SecretPersistence {
    let _ = delete_secret(LEGACY_PASSWORD_SERVICE, LEGACY_SECRET_ACCOUNT);
    delete_secret(PASSWORD_SERVICE, PASSWORD_ACCOUNT)
}

pub fn create_repo_manifest() -> Result<RepoManifest, String> {
    let mut salt = [0_u8; 16];
    rand::thread_rng().fill_bytes(&mut salt);
    Ok(RepoManifest {
        app: "hurl".to_string(),
        format_version: 1,
        kdf: "argon2id".to_string(),
        kdf_params: default_kdf_params(),
        cipher: "xchacha20poly1305".to_string(),
        salt: BASE64.encode(salt),
        created_at: now_rfc3339()?,
    })
}

pub fn derive_repo_key(password: &str, manifest: &RepoManifest) -> Result<[u8; 32], String> {
    validate_manifest(manifest)?;
    let salt = BASE64
        .decode(manifest.salt.as_bytes())
        .map_err(|error| format!("Sync manifest salt is invalid: {error}"))?;
    let params = Params::new(
        manifest.kdf_params.memory_kib,
        manifest.kdf_params.iterations,
        manifest.kdf_params.parallelism,
        Some(32),
    )
    .map_err(|error| format!("Sync manifest KDF parameters are invalid: {error}"))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut output = [0_u8; 32];
    argon2
        .hash_password_into(password.as_bytes(), &salt, &mut output)
        .map_err(|error| format!("Failed to derive sync key: {error}"))?;
    Ok(output)
}

pub fn encrypt_request(
    request: &SavedRequest,
    key_bytes: &[u8; 32],
) -> Result<EncryptedEnvelope, String> {
    let plaintext = serde_json::to_vec(request)
        .map_err(|error| format!("Failed to serialize request for sync: {error}"))?;
    let cipher = XChaCha20Poly1305::new(Key::from_slice(key_bytes));
    let mut nonce_bytes = [0_u8; NONCE_BYTES];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let ciphertext = cipher
        .encrypt(XNonce::from_slice(&nonce_bytes), plaintext.as_ref())
        .map_err(|error| format!("Failed to encrypt request for sync: {error}"))?;
    Ok(EncryptedEnvelope {
        version: ENVELOPE_VERSION,
        nonce: BASE64.encode(nonce_bytes),
        ciphertext: BASE64.encode(ciphertext),
    })
}

pub fn decrypt_request(envelope_json: &str, key_bytes: &[u8; 32]) -> Result<SavedRequest, String> {
    let envelope: EncryptedEnvelope = serde_json::from_str(envelope_json)
        .map_err(|error| format!("Failed to parse encrypted request envelope: {error}"))?;
    if envelope.version != ENVELOPE_VERSION {
        return Err(format!(
            "Encrypted request envelope version {} is not supported.",
            envelope.version
        ));
    }
    let nonce = BASE64
        .decode(envelope.nonce.as_bytes())
        .map_err(|error| format!("Encrypted request nonce is invalid: {error}"))?;
    if nonce.len() != NONCE_BYTES {
        return Err("Encrypted request nonce has the wrong length.".to_string());
    }
    let ciphertext = BASE64
        .decode(envelope.ciphertext.as_bytes())
        .map_err(|error| format!("Encrypted request payload is invalid: {error}"))?;
    let cipher = XChaCha20Poly1305::new(Key::from_slice(key_bytes));
    let plaintext = cipher
        .decrypt(XNonce::from_slice(&nonce), ciphertext.as_ref())
        .map_err(|_| "Sync password is incorrect or the sync data is corrupted.".to_string())?;
    serde_json::from_slice(&plaintext)
        .map_err(|error| format!("Failed to deserialize synced request: {error}"))
}

pub fn encrypt_folder(
    folder: &SavedFolder,
    key_bytes: &[u8; 32],
) -> Result<EncryptedEnvelope, String> {
    let plaintext = serde_json::to_vec(folder)
        .map_err(|error| format!("Failed to serialize folder for sync: {error}"))?;
    let cipher = XChaCha20Poly1305::new(Key::from_slice(key_bytes));
    let mut nonce_bytes = [0_u8; NONCE_BYTES];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let ciphertext = cipher
        .encrypt(XNonce::from_slice(&nonce_bytes), plaintext.as_ref())
        .map_err(|error| format!("Failed to encrypt folder for sync: {error}"))?;
    Ok(EncryptedEnvelope {
        version: ENVELOPE_VERSION,
        nonce: BASE64.encode(nonce_bytes),
        ciphertext: BASE64.encode(ciphertext),
    })
}

pub fn decrypt_folder(envelope_json: &str, key_bytes: &[u8; 32]) -> Result<SavedFolder, String> {
    let envelope: EncryptedEnvelope = serde_json::from_str(envelope_json)
        .map_err(|error| format!("Failed to parse encrypted folder envelope: {error}"))?;
    if envelope.version != ENVELOPE_VERSION {
        return Err(format!(
            "Encrypted folder envelope version {} is not supported.",
            envelope.version
        ));
    }
    let nonce = BASE64
        .decode(envelope.nonce.as_bytes())
        .map_err(|error| format!("Encrypted folder nonce is invalid: {error}"))?;
    if nonce.len() != NONCE_BYTES {
        return Err("Encrypted folder nonce has the wrong length.".to_string());
    }
    let ciphertext = BASE64
        .decode(envelope.ciphertext.as_bytes())
        .map_err(|error| format!("Encrypted folder payload is invalid: {error}"))?;
    let cipher = XChaCha20Poly1305::new(Key::from_slice(key_bytes));
    let plaintext = cipher
        .decrypt(XNonce::from_slice(&nonce), ciphertext.as_ref())
        .map_err(|_| "Sync password is incorrect or the sync data is corrupted.".to_string())?;
    serde_json::from_slice(&plaintext)
        .map_err(|error| format!("Failed to deserialize synced folder: {error}"))
}

pub fn request_hash(request: &SavedRequest) -> Result<String, String> {
    let bytes = serde_json::to_vec(request)
        .map_err(|error| format!("Failed to hash synced request: {error}"))?;
    let digest = Sha256::digest(bytes);
    Ok(BASE64.encode(digest))
}

pub fn folder_hash(folder: &SavedFolder) -> Result<String, String> {
    let bytes = serde_json::to_vec(folder)
        .map_err(|error| format!("Failed to hash synced folder: {error}"))?;
    let digest = Sha256::digest(bytes);
    Ok(BASE64.encode(digest))
}

pub fn merge_requests(
    local_library: &[SavedRequest],
    remote_requests: &BTreeMap<Uuid, SavedRequest>,
    last_synced_hash: &BTreeMap<Uuid, String>,
) -> Result<RequestMergeResult, String> {
    let local_by_id = local_library
        .iter()
        .cloned()
        .map(|request| (request.id, request))
        .collect::<BTreeMap<_, _>>();
    let remote_hashes = request_hash_map(remote_requests.values())?;
    let local_hashes = request_hash_map(local_library.iter())?;

    let mut merged = local_library.to_vec();
    let mut merged_index = merged
        .iter()
        .enumerate()
        .map(|(index, request)| (request.id, index))
        .collect::<HashMap<_, _>>();

    let mut imported_count = 0;
    let mut conflict_count = 0;
    let mut missing_remote_count = 0;
    let mut remote_only_appended = HashSet::new();

    let mut all_ids = local_by_id.keys().copied().collect::<BTreeSet<_>>();
    for id in remote_requests.keys().copied() {
        all_ids.insert(id);
    }

    for id in all_ids {
        let local = local_by_id.get(&id);
        let remote = remote_requests.get(&id);
        let base_hash = last_synced_hash.get(&id);

        match (local, remote) {
            (Some(_local_request), Some(remote_request)) => {
                let local_hash = local_hashes
                    .get(&id)
                    .ok_or_else(|| "Missing local sync hash during merge.".to_string())?;
                let remote_hash = remote_hashes
                    .get(&id)
                    .ok_or_else(|| "Missing remote sync hash during merge.".to_string())?;

                if local_hash == remote_hash {
                    continue;
                }

                let local_changed = base_hash.map(|hash| hash != local_hash).unwrap_or(true);
                let remote_changed = base_hash.map(|hash| hash != remote_hash).unwrap_or(true);

                match (local_changed, remote_changed) {
                    (true, false) => {}
                    (false, true) => {
                        replace_request(&mut merged, &mut merged_index, remote_request.clone());
                        imported_count += 1;
                    }
                    (true, true) => {
                        let conflict_request = conflict_copy(remote_request);
                        merged_index.insert(conflict_request.id, merged.len());
                        merged.push(conflict_request);
                        conflict_count += 1;
                        imported_count += 1;
                    }
                    (false, false) => {}
                }
            }
            (Some(_local_request), None) => {
                if base_hash.is_some() {
                    missing_remote_count += 1;
                }
            }
            (None, Some(remote_request)) => {
                if !remote_only_appended.insert(id) {
                    continue;
                }
                merged_index.insert(id, merged.len());
                merged.push(remote_request.clone());
                imported_count += 1;
            }
            (None, None) => {}
        }
    }

    let merged_hashes = request_hash_map(merged.iter())?;
    let mut upload_ids = Vec::new();
    for request in &merged {
        let merged_hash = merged_hashes
            .get(&request.id)
            .ok_or_else(|| "Missing merged sync hash after merge.".to_string())?;
        match remote_hashes.get(&request.id) {
            Some(remote_hash) if remote_hash == merged_hash => {}
            _ => upload_ids.push(request.id),
        }
    }

    let warning = if missing_remote_count > 0 {
        Some(format!(
            "Remote sync data was missing {} request file(s); local copies were preserved.",
            missing_remote_count
        ))
    } else if conflict_count > 0 {
        Some(format!(
            "Sync created {} conflict copy/copies.",
            conflict_count
        ))
    } else {
        None
    };

    Ok(RequestMergeResult {
        requests: merged,
        hashes: merged_hashes,
        upload_ids,
        imported_count,
        conflict_count,
        warning,
    })
}

pub fn merge_folders(
    local_folders: &[SavedFolder],
    remote_folders: &BTreeMap<Uuid, SavedFolder>,
    last_synced_hash: &BTreeMap<Uuid, String>,
) -> Result<FolderMergeResult, String> {
    let local_by_id = local_folders
        .iter()
        .cloned()
        .map(|folder| (folder.id, folder))
        .collect::<BTreeMap<_, _>>();
    let remote_hashes = folder_hash_map(remote_folders.values())?;
    let local_hashes = folder_hash_map(local_folders.iter())?;

    let mut merged = local_folders.to_vec();
    let mut merged_index = merged
        .iter()
        .enumerate()
        .map(|(index, folder)| (folder.id, index))
        .collect::<HashMap<_, _>>();

    let mut imported_count = 0;
    let mut conflict_count = 0;
    let mut missing_remote_count = 0;

    let mut all_ids = local_by_id.keys().copied().collect::<BTreeSet<_>>();
    for id in remote_folders.keys().copied() {
        all_ids.insert(id);
    }

    for id in all_ids {
        let local = local_by_id.get(&id);
        let remote = remote_folders.get(&id);
        let base_hash = last_synced_hash.get(&id);

        match (local, remote) {
            (Some(_local_folder), Some(remote_folder)) => {
                let local_hash = local_hashes
                    .get(&id)
                    .ok_or_else(|| "Missing local folder sync hash during merge.".to_string())?;
                let remote_hash = remote_hashes
                    .get(&id)
                    .ok_or_else(|| "Missing remote folder sync hash during merge.".to_string())?;

                if local_hash == remote_hash {
                    continue;
                }

                let local_changed = base_hash.map(|hash| hash != local_hash).unwrap_or(true);
                let remote_changed = base_hash.map(|hash| hash != remote_hash).unwrap_or(true);

                match (local_changed, remote_changed) {
                    (true, false) => {}
                    (false, true) => {
                        replace_folder(&mut merged, &mut merged_index, remote_folder.clone());
                        imported_count += 1;
                    }
                    (true, true) => {
                        conflict_count += 1;
                    }
                    (false, false) => {}
                }
            }
            (Some(_local_folder), None) => {
                if base_hash.is_some() {
                    missing_remote_count += 1;
                }
            }
            (None, Some(remote_folder)) => {
                merged_index.insert(id, merged.len());
                merged.push(remote_folder.clone());
                imported_count += 1;
            }
            (None, None) => {}
        }
    }

    let mut merged_library = LibraryData {
        folders: merged,
        requests: Vec::new(),
    };
    merged_library.normalize();
    let merged_folders = merged_library.folders;
    let merged_hashes = folder_hash_map(merged_folders.iter())?;
    let mut upload_ids = Vec::new();
    for folder in &merged_folders {
        let merged_hash = merged_hashes
            .get(&folder.id)
            .ok_or_else(|| "Missing merged folder sync hash after merge.".to_string())?;
        match remote_hashes.get(&folder.id) {
            Some(remote_hash) if remote_hash == merged_hash => {}
            _ => upload_ids.push(folder.id),
        }
    }

    let warning = if missing_remote_count > 0 {
        Some(format!(
            "Remote sync data was missing {} folder file(s); local copies were preserved.",
            missing_remote_count
        ))
    } else if conflict_count > 0 {
        Some(format!(
            "Folder metadata conflicted on {} folder(s); local versions were kept.",
            conflict_count
        ))
    } else {
        None
    };

    Ok(FolderMergeResult {
        folders: merged_folders,
        hashes: merged_hashes,
        upload_ids,
        imported_count,
        warning,
    })
}

pub async fn request_device_code(client_id: &str) -> Result<DeviceCodePrompt, String> {
    let client = github_http_client()?;
    let response = client
        .post(GITHUB_DEVICE_CODE_URL)
        .header(ACCEPT, HeaderValue::from_static("application/json"))
        .form(&DeviceCodeRequest {
            client_id,
            scope: "repo",
        })
        .send()
        .await
        .map_err(|error| {
            format!("Failed to contact GitHub device authorization endpoint: {error}")
        })?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!(
            "GitHub device authorization failed with status {}: {}",
            status,
            body.trim()
        ));
    }

    let payload: DeviceCodeResponse = response.json().await.map_err(|error| {
        format!("Failed to parse GitHub device authorization response: {error}")
    })?;

    Ok(DeviceCodePrompt {
        device_code: payload.device_code,
        user_code: payload.user_code,
        verification_uri: payload.verification_uri,
        expires_in_seconds: payload.expires_in,
        interval_seconds: payload.interval.unwrap_or(5),
    })
}

pub async fn complete_device_flow(
    client_id: &str,
    prompt: &DeviceCodePrompt,
) -> Result<GitHubIdentity, String> {
    let client = github_http_client()?;
    let mut interval = prompt.interval_seconds.max(1);
    let started_at = std::time::Instant::now();

    loop {
        if started_at.elapsed() >= Duration::from_secs(prompt.expires_in_seconds) {
            return Err("GitHub device authorization expired before it was completed.".to_string());
        }

        tokio::time::sleep(Duration::from_secs(interval)).await;

        let response = client
            .post(GITHUB_ACCESS_TOKEN_URL)
            .header(ACCEPT, HeaderValue::from_static("application/json"))
            .form(&AccessTokenRequest {
                client_id,
                device_code: &prompt.device_code,
                grant_type: "urn:ietf:params:oauth:grant-type:device_code",
            })
            .send()
            .await
            .map_err(|error| format!("Failed to poll GitHub access token endpoint: {error}"))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(format!(
                "GitHub access token polling failed with status {}: {}",
                status,
                body.trim()
            ));
        }

        let payload: AccessTokenResponse = response
            .json()
            .await
            .map_err(|error| format!("Failed to parse GitHub access token response: {error}"))?;

        if let Some(token) = payload.access_token {
            let username = GitHubApi::new()?.fetch_authenticated_user(&token).await?;
            return Ok(GitHubIdentity {
                username,
                access_token: token,
            });
        }

        match payload.error.as_deref() {
            Some("authorization_pending") => continue,
            Some("slow_down") => {
                interval = payload.interval.unwrap_or(interval + 5).max(interval + 1);
                continue;
            }
            Some("access_denied") => {
                return Err("GitHub authorization was denied.".to_string());
            }
            Some("expired_token") => {
                return Err(
                    "GitHub device authorization expired before it was completed.".to_string(),
                );
            }
            Some(error) => {
                return Err(payload
                    .error_description
                    .unwrap_or_else(|| format!("GitHub device authorization failed: {error}")));
            }
            None => {
                return Err("GitHub device authorization returned an empty response.".to_string());
            }
        }
    }
}

pub async fn enable_sync(
    mut config: SyncConfig,
    mut state: SyncState,
    mut library: LibraryData,
    access_token: &str,
    password: &str,
) -> Result<SyncRunOutput, String> {
    library.normalize();
    let api = GitHubApi::new()?;
    let repo_info = match api
        .get_repo(access_token, &config.owner, &config.repo)
        .await?
    {
        Some(info) => info,
        None => {
            if config.owner != config.github_user {
                return Err(
                    "The configured sync repository does not exist and can only be auto-created under your own GitHub account."
                        .to_string(),
                );
            }
            api.create_private_repo(access_token, &config.repo).await?
        }
    };

    config.owner = repo_info.owner.login;
    config.branch = repo_info.default_branch;
    config.enabled = true;

    let root_contents = api
        .list_directory(
            access_token,
            &config.owner,
            &config.repo,
            "",
            &config.branch,
        )
        .await?;
    let manifest_file = api
        .get_file(
            access_token,
            &config.owner,
            &config.repo,
            MANIFEST_PATH,
            &config.branch,
        )
        .await?;

    if manifest_file.is_none() {
        if !root_contents.is_empty() && !root_contents.iter().any(|item| item.path == MANIFEST_PATH)
        {
            if root_contents.len() > 1
                || root_contents.first().map(|item| item.name.as_str()) != Some("README.md")
            {
                return Err(
                    "The configured GitHub repository already contains files but is not a valid hurl sync repo. Use a dedicated empty repo."
                        .to_string(),
                );
            }
        }

        let manifest = create_repo_manifest()?;
        let repo_key = derive_repo_key(password, &manifest)?;
        api.put_file(
            access_token,
            &config.owner,
            &config.repo,
            MANIFEST_PATH,
            serde_json::to_vec_pretty(&manifest)
                .map_err(|error| format!("Failed to serialize sync manifest: {error}"))?,
            None,
            &config.branch,
            "hurl sync: initialize manifest",
        )
        .await?;

        let mut uploaded_count = 0;
        for folder in &library.folders {
            let envelope = encrypt_folder(folder, &repo_key)?;
            let path = folder_file_path(folder.id);
            api.put_file(
                access_token,
                &config.owner,
                &config.repo,
                &path,
                serde_json::to_vec(&envelope)
                    .map_err(|error| format!("Failed to serialize encrypted folder: {error}"))?,
                None,
                &config.branch,
                &format!("hurl sync: save folder {}", folder.id),
            )
            .await?;
            uploaded_count += 1;
        }

        for request in &library.requests {
            let envelope = encrypt_request(request, &repo_key)?;
            let path = request_file_path(request.id);
            api.put_file(
                access_token,
                &config.owner,
                &config.repo,
                &path,
                serde_json::to_vec(&envelope)
                    .map_err(|error| format!("Failed to serialize encrypted request: {error}"))?,
                None,
                &config.branch,
                &format!("hurl sync: save request {}", request.id),
            )
            .await?;
            uploaded_count += 1;
        }

        state.last_synced_hash = request_hash_map(library.requests.iter())?;
        state.last_synced_folder_hash = folder_hash_map(library.folders.iter())?;
        state.last_success_at = Some(now_rfc3339()?);
        state.dirty = false;
        state.last_head_sha = api
            .branch_head_sha(access_token, &config.owner, &config.repo, &config.branch)
            .await?;

        return Ok(SyncRunOutput {
            config,
            state,
            library,
            imported_count: 0,
            uploaded_count,
            conflict_count: 0,
            warning: None,
        });
    }

    sync_library(config, state, library, access_token, password).await
}

pub async fn sync_library(
    config: SyncConfig,
    mut state: SyncState,
    mut library: LibraryData,
    access_token: &str,
    password: &str,
) -> Result<SyncRunOutput, String> {
    library.normalize();
    let api = GitHubApi::new()?;
    let snapshot = api
        .read_remote_snapshot(
            access_token,
            &config.owner,
            &config.repo,
            &config.branch,
            password,
        )
        .await?;
    let request_merge = merge_requests(
        &library.requests,
        &snapshot.requests,
        &state.last_synced_hash,
    )?;
    let folder_merge = merge_folders(
        &library.folders,
        &snapshot.folders,
        &state.last_synced_folder_hash,
    )?;
    let repo_key = derive_repo_key(password, &snapshot.manifest)?;

    let merged_requests_by_id = request_merge
        .requests
        .iter()
        .cloned()
        .map(|request| (request.id, request))
        .collect::<BTreeMap<_, _>>();

    let mut uploaded_count = 0;
    for folder_id in &folder_merge.upload_ids {
        let folder = folder_merge
            .folders
            .iter()
            .find(|folder| &folder.id == folder_id)
            .ok_or_else(|| "A merged folder was missing during upload preparation.".to_string())?;
        let envelope = encrypt_folder(folder, &repo_key)?;
        let path = folder_file_path(*folder_id);
        let sha = snapshot.folder_shas.get(folder_id).map(String::as_str);
        api.put_file(
            access_token,
            &config.owner,
            &config.repo,
            &path,
            serde_json::to_vec(&envelope)
                .map_err(|error| format!("Failed to serialize encrypted folder: {error}"))?,
            sha,
            &config.branch,
            &format!("hurl sync: save folder {}", folder_id),
        )
        .await?;
        uploaded_count += 1;
    }

    for request_id in &request_merge.upload_ids {
        let request = merged_requests_by_id
            .get(request_id)
            .ok_or_else(|| "A merged request was missing during upload preparation.".to_string())?;
        let envelope = encrypt_request(request, &repo_key)?;
        let path = request_file_path(*request_id);
        let sha = snapshot.request_shas.get(request_id).map(String::as_str);
        api.put_file(
            access_token,
            &config.owner,
            &config.repo,
            &path,
            serde_json::to_vec(&envelope)
                .map_err(|error| format!("Failed to serialize encrypted request: {error}"))?,
            sha,
            &config.branch,
            &format!("hurl sync: save request {}", request_id),
        )
        .await?;
        uploaded_count += 1;
    }

    let merged_library = LibraryData {
        folders: folder_merge.folders,
        requests: request_merge.requests,
    }
    .normalized();

    state.last_synced_hash = request_hash_map(merged_library.requests.iter())?;
    state.last_synced_folder_hash = folder_hash_map(merged_library.folders.iter())?;
    state.last_success_at = Some(now_rfc3339()?);
    state.dirty = false;
    state.last_head_sha = api
        .branch_head_sha(access_token, &config.owner, &config.repo, &config.branch)
        .await?
        .or(snapshot.head_sha);

    Ok(SyncRunOutput {
        config,
        state,
        library: merged_library,
        imported_count: request_merge.imported_count + folder_merge.imported_count,
        uploaded_count,
        conflict_count: request_merge.conflict_count,
        warning: combine_warnings([request_merge.warning, folder_merge.warning]),
    })
}

fn default_kdf_params() -> KdfParams {
    KdfParams {
        memory_kib: ARGON2_MEMORY_KIB,
        iterations: ARGON2_ITERATIONS,
        parallelism: ARGON2_PARALLELISM,
    }
}

fn validate_manifest(manifest: &RepoManifest) -> Result<(), String> {
    if manifest.app != "hurl" {
        return Err("Sync manifest is not for hurl.".to_string());
    }
    if manifest.format_version != 1 {
        return Err(format!(
            "Sync manifest version {} is not supported.",
            manifest.format_version
        ));
    }
    if manifest.kdf != "argon2id" {
        return Err(format!(
            "Sync manifest KDF `{}` is not supported.",
            manifest.kdf
        ));
    }
    if manifest.cipher != "xchacha20poly1305" {
        return Err(format!(
            "Sync manifest cipher `{}` is not supported.",
            manifest.cipher
        ));
    }
    Ok(())
}

fn request_hash_map<'a, I>(requests: I) -> Result<BTreeMap<Uuid, String>, String>
where
    I: IntoIterator<Item = &'a SavedRequest>,
{
    let mut hashes = BTreeMap::new();
    for request in requests {
        hashes.insert(request.id, request_hash(request)?);
    }
    Ok(hashes)
}

fn folder_hash_map<'a, I>(folders: I) -> Result<BTreeMap<Uuid, String>, String>
where
    I: IntoIterator<Item = &'a SavedFolder>,
{
    let mut hashes = BTreeMap::new();
    for folder in folders {
        hashes.insert(folder.id, folder_hash(folder)?);
    }
    Ok(hashes)
}

fn folder_file_path(id: Uuid) -> String {
    format!("{FOLDERS_DIR}/{id}.json.enc")
}

fn request_file_path(id: Uuid) -> String {
    format!("{REQUESTS_DIR}/{id}.json.enc")
}

fn conflict_copy(request: &SavedRequest) -> SavedRequest {
    let title_base = request
        .title
        .clone()
        .unwrap_or_else(|| request.display_name());
    SavedRequest {
        id: Uuid::new_v4(),
        folder_id: request.folder_id,
        title: Some(format!("CONFLICT {} - {}", timestamp_slug(), title_base)),
        method: request.method,
        url: request.url.clone(),
        headers: request.headers.clone(),
        json_body: request.json_body.clone(),
    }
}

fn timestamp_slug() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "unknown-time".to_string())
}

fn replace_request(
    merged: &mut [SavedRequest],
    merged_index: &mut HashMap<Uuid, usize>,
    new_request: SavedRequest,
) {
    if let Some(index) = merged_index.get(&new_request.id).copied() {
        merged[index] = new_request;
    }
}

fn replace_folder(
    merged: &mut [SavedFolder],
    merged_index: &mut HashMap<Uuid, usize>,
    new_folder: SavedFolder,
) {
    if let Some(index) = merged_index.get(&new_folder.id).copied() {
        merged[index] = new_folder;
    }
}

fn combine_warnings<const N: usize>(warnings: [Option<String>; N]) -> Option<String> {
    let warnings = warnings.into_iter().flatten().collect::<Vec<_>>();
    if warnings.is_empty() {
        None
    } else {
        Some(warnings.join(" "))
    }
}

fn now_rfc3339() -> Result<String, String> {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|error| format!("Failed to format time: {error}"))
}

fn load_secret(service: &str, account: &str) -> Option<String> {
    let entry = Entry::new(service, account).ok()?;
    match entry.get_password() {
        Ok(secret) => Some(secret),
        Err(KeyringError::NoEntry) => None,
        Err(_) => None,
    }
}

fn store_secret(service: &str, account: &str, value: &str) -> SecretPersistence {
    match Entry::new(service, account) {
        Ok(entry) => match entry.set_password(value) {
            Ok(()) => SecretPersistence::Persisted,
            Err(_) => SecretPersistence::SessionOnly,
        },
        Err(_) => SecretPersistence::SessionOnly,
    }
}

fn delete_secret(service: &str, account: &str) -> SecretPersistence {
    match Entry::new(service, account) {
        Ok(entry) => match entry.delete_credential() {
            Ok(()) | Err(KeyringError::NoEntry) => SecretPersistence::Deleted,
            Err(_) => SecretPersistence::Deleted,
        },
        Err(_) => SecretPersistence::Deleted,
    }
}

fn github_http_client() -> Result<Client, String> {
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_static("hurl"));
    headers.insert(
        ACCEPT,
        HeaderValue::from_static("application/vnd.github+json"),
    );
    Client::builder()
        .default_headers(headers)
        .build()
        .map_err(|error| format!("Failed to build GitHub HTTP client: {error}"))
}

#[derive(Clone)]
struct GitHubApi {
    client: Client,
}

impl GitHubApi {
    fn new() -> Result<Self, String> {
        Ok(Self {
            client: github_http_client()?,
        })
    }

    async fn fetch_authenticated_user(&self, access_token: &str) -> Result<String, String> {
        let response = self
            .authed_get(&format!("{GITHUB_API_BASE}/user"), access_token)
            .await?;
        let user: GitHubUser = parse_json_response(response).await?;
        Ok(user.login)
    }

    async fn get_repo(
        &self,
        access_token: &str,
        owner: &str,
        repo: &str,
    ) -> Result<Option<RepoInfo>, String> {
        let response = self
            .authed_get(
                &format!("{GITHUB_API_BASE}/repos/{owner}/{repo}"),
                access_token,
            )
            .await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let repo_info: RepoInfo = parse_json_response(response).await?;
        Ok(Some(repo_info))
    }

    async fn create_private_repo(
        &self,
        access_token: &str,
        repo: &str,
    ) -> Result<RepoInfo, String> {
        let response = self
            .client
            .post(format!("{GITHUB_API_BASE}/user/repos"))
            .header(AUTHORIZATION, bearer(access_token)?)
            .json(&CreateRepoBody {
                name: repo,
                private: true,
                auto_init: true,
            })
            .send()
            .await
            .map_err(|error| format!("Failed to create GitHub sync repository: {error}"))?;

        let repo_info: RepoInfo = parse_json_response(response).await?;
        Ok(repo_info)
    }

    async fn branch_head_sha(
        &self,
        access_token: &str,
        owner: &str,
        repo: &str,
        branch: &str,
    ) -> Result<Option<String>, String> {
        let response = self
            .authed_get(
                &format!("{GITHUB_API_BASE}/repos/{owner}/{repo}/branches/{branch}"),
                access_token,
            )
            .await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let value: serde_json::Value = parse_json_response(response).await?;
        Ok(value
            .get("commit")
            .and_then(|commit| commit.get("sha"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string))
    }

    async fn list_directory(
        &self,
        access_token: &str,
        owner: &str,
        repo: &str,
        path: &str,
        branch: &str,
    ) -> Result<Vec<ContentListItem>, String> {
        let path_component = if path.is_empty() {
            String::new()
        } else {
            format!("/{path}")
        };
        let response = self
            .authed_get(
                &format!(
                    "{GITHUB_API_BASE}/repos/{owner}/{repo}/contents{path_component}?ref={branch}"
                ),
                access_token,
            )
            .await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(Vec::new());
        }
        parse_json_response(response).await
    }

    async fn get_file(
        &self,
        access_token: &str,
        owner: &str,
        repo: &str,
        path: &str,
        branch: &str,
    ) -> Result<Option<ContentFile>, String> {
        let response = self
            .authed_get(
                &format!("{GITHUB_API_BASE}/repos/{owner}/{repo}/contents/{path}?ref={branch}"),
                access_token,
            )
            .await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let file: ContentFile = parse_json_response(response).await?;
        Ok(Some(file))
    }

    async fn put_file(
        &self,
        access_token: &str,
        owner: &str,
        repo: &str,
        path: &str,
        bytes: Vec<u8>,
        sha: Option<&str>,
        branch: &str,
        message: &str,
    ) -> Result<(), String> {
        let response = self
            .client
            .put(format!(
                "{GITHUB_API_BASE}/repos/{owner}/{repo}/contents/{path}"
            ))
            .header(AUTHORIZATION, bearer(access_token)?)
            .json(&PutContentBody {
                message,
                content: BASE64.encode(bytes),
                branch,
                sha,
            })
            .send()
            .await
            .map_err(|error| format!("Failed to upload synced file `{path}` to GitHub: {error}"))?;

        parse_json_response::<serde_json::Value>(response)
            .await
            .map(|_| ())
            .map_err(|error| format!("Failed to upload synced file `{path}`: {error}"))
    }

    async fn read_remote_snapshot(
        &self,
        access_token: &str,
        owner: &str,
        repo: &str,
        branch: &str,
        password: &str,
    ) -> Result<RemoteSnapshot, String> {
        let manifest_file = self
            .get_file(access_token, owner, repo, MANIFEST_PATH, branch)
            .await?
            .ok_or_else(|| {
                "The configured GitHub sync repository is missing manifest.json.".to_string()
            })?;
        let manifest_bytes = decode_contents_file(&manifest_file)?;
        let manifest: RepoManifest = serde_json::from_slice(&manifest_bytes)
            .map_err(|error| format!("Failed to parse sync manifest: {error}"))?;
        let repo_key = derive_repo_key(password, &manifest)?;

        let mut requests = BTreeMap::new();
        let mut request_shas = HashMap::new();
        let mut folders = BTreeMap::new();
        let mut folder_shas = HashMap::new();

        for entry in self
            .list_directory(access_token, owner, repo, FOLDERS_DIR, branch)
            .await?
            .into_iter()
            .filter(|entry| entry.kind == "file" && entry.name.ends_with(".json.enc"))
        {
            let file = self
                .get_file(access_token, owner, repo, &entry.path, branch)
                .await?
                .ok_or_else(|| {
                    format!(
                        "Sync folder file `{}` disappeared during fetch.",
                        entry.path
                    )
                })?;
            let bytes = decode_contents_file(&file)?;
            let folder = decrypt_folder(
                std::str::from_utf8(&bytes).map_err(|error| {
                    format!("Failed to read synced folder `{}`: {error}", entry.path)
                })?,
                &repo_key,
            )?;
            folder_shas.insert(folder.id, file.sha);
            folders.insert(folder.id, folder);
        }

        for entry in self
            .list_directory(access_token, owner, repo, REQUESTS_DIR, branch)
            .await?
            .into_iter()
            .filter(|entry| entry.kind == "file" && entry.name.ends_with(".json.enc"))
        {
            let file = self
                .get_file(access_token, owner, repo, &entry.path, branch)
                .await?
                .ok_or_else(|| {
                    format!(
                        "Sync request file `{}` disappeared during fetch.",
                        entry.path
                    )
                })?;
            let bytes = decode_contents_file(&file)?;
            let request = decrypt_request(
                std::str::from_utf8(&bytes).map_err(|error| {
                    format!("Failed to read synced request `{}`: {error}", entry.path)
                })?,
                &repo_key,
            )?;
            request_shas.insert(request.id, file.sha);
            requests.insert(request.id, request);
        }

        Ok(RemoteSnapshot {
            manifest,
            requests,
            folders,
            request_shas,
            folder_shas,
            head_sha: self
                .branch_head_sha(access_token, owner, repo, branch)
                .await?,
        })
    }

    async fn authed_get(&self, url: &str, access_token: &str) -> Result<reqwest::Response, String> {
        self.client
            .get(url)
            .header(AUTHORIZATION, bearer(access_token)?)
            .send()
            .await
            .map_err(|error| format!("GitHub API request failed: {error}"))
    }
}

fn bearer(access_token: &str) -> Result<HeaderValue, String> {
    HeaderValue::from_str(&format!("Bearer {access_token}"))
        .map_err(|error| format!("Failed to build GitHub authorization header: {error}"))
}

async fn parse_json_response<T>(response: reqwest::Response) -> Result<T, String>
where
    T: for<'de> Deserialize<'de>,
{
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|error| format!("Failed to read GitHub API response body: {error}"))?;
    if !status.is_success() {
        let message = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|value| {
                value
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| text.trim().to_string());
        return Err(format!(
            "GitHub API returned status {}: {}",
            status, message
        ));
    }
    serde_json::from_str(&text)
        .with_context(|| format!("Failed to parse GitHub API JSON: {text}"))
        .map_err(|error| error.to_string())
}

fn decode_contents_file(file: &ContentFile) -> Result<Vec<u8>, String> {
    if file.kind != "file" {
        return Err(format!(
            "GitHub contents item `{}` is not a file.",
            file.path
        ));
    }
    let content = file.content.as_deref().ok_or_else(|| {
        format!(
            "GitHub contents response for `{}` was missing file data.",
            file.path
        )
    })?;
    let encoding = file.encoding.as_deref().unwrap_or("base64");
    if encoding != "base64" {
        return Err(format!(
            "GitHub contents response for `{}` used unsupported encoding `{encoding}`.",
            file.path
        ));
    }
    BASE64
        .decode(content.replace('\n', ""))
        .map_err(|error| format!("Failed to decode GitHub file `{}`: {error}", file.path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{HeaderEntry, HttpMethod, SavedFolder};

    fn sample_request(id: Uuid, title: &str, body: &str) -> SavedRequest {
        SavedRequest {
            id,
            folder_id: None,
            title: Some(title.to_string()),
            method: HttpMethod::Post,
            url: format!("https://example.com/{title}"),
            headers: vec![HeaderEntry {
                name: "Accept".to_string(),
                value: "application/json".to_string(),
            }],
            json_body: body.to_string(),
        }
    }

    fn sample_folder(id: Uuid, name: &str, parent_id: Option<Uuid>) -> SavedFolder {
        SavedFolder {
            id,
            name: name.to_string(),
            parent_id,
        }
    }

    #[test]
    fn encrypts_and_decrypts_requests() {
        let manifest = create_repo_manifest().unwrap();
        let key = derive_repo_key("super-secret", &manifest).unwrap();
        let request = sample_request(Uuid::new_v4(), "Create", r#"{"ok":true}"#);

        let encrypted = encrypt_request(&request, &key).unwrap();
        let decrypted = decrypt_request(&serde_json::to_string(&encrypted).unwrap(), &key).unwrap();

        assert_eq!(decrypted, request);
    }

    #[test]
    fn rejects_wrong_password() {
        let manifest = create_repo_manifest().unwrap();
        let right_key = derive_repo_key("correct", &manifest).unwrap();
        let wrong_key = derive_repo_key("wrong", &manifest).unwrap();
        let request = sample_request(Uuid::new_v4(), "Create", r#"{"ok":true}"#);

        let encrypted = encrypt_request(&request, &right_key).unwrap();
        let error =
            decrypt_request(&serde_json::to_string(&encrypted).unwrap(), &wrong_key).unwrap_err();

        assert!(error.contains("incorrect") || error.contains("corrupted"));
    }

    #[test]
    fn validates_manifest_fields() {
        let mut manifest = create_repo_manifest().unwrap();
        manifest.format_version = 9;

        let error = derive_repo_key("password", &manifest).unwrap_err();
        assert!(error.contains("not supported"));
    }

    #[test]
    fn request_hash_is_stable() {
        let request = sample_request(Uuid::new_v4(), "Create", r#"{"ok":true}"#);
        assert_eq!(
            request_hash(&request).unwrap(),
            request_hash(&request).unwrap()
        );
    }

    #[test]
    fn encrypts_and_decrypts_folders() {
        let manifest = create_repo_manifest().unwrap();
        let key = derive_repo_key("super-secret", &manifest).unwrap();
        let folder = sample_folder(Uuid::new_v4(), "Auth", None);

        let encrypted = encrypt_folder(&folder, &key).unwrap();
        let decrypted = decrypt_folder(&serde_json::to_string(&encrypted).unwrap(), &key).unwrap();

        assert_eq!(decrypted, folder);
    }

    #[test]
    fn merges_non_conflicting_changes() {
        let local_id = Uuid::new_v4();
        let remote_id = Uuid::new_v4();
        let local_request = sample_request(local_id, "Local", "{}");
        let remote_request = sample_request(remote_id, "Remote", "{}");
        let last_synced = BTreeMap::new();
        let remote = BTreeMap::from([(remote_id, remote_request.clone())]);

        let merge = merge_requests(&[local_request.clone()], &remote, &last_synced).unwrap();

        assert_eq!(merge.requests.len(), 2);
        assert!(merge.requests.iter().any(|request| request.id == local_id));
        assert!(merge.requests.iter().any(|request| request.id == remote_id));
        assert_eq!(merge.imported_count, 1);
        assert_eq!(merge.conflict_count, 0);
    }

    #[test]
    fn merges_remote_only_folders() {
        let remote_folder = sample_folder(Uuid::new_v4(), "Auth", None);
        let remote = BTreeMap::from([(remote_folder.id, remote_folder.clone())]);

        let merge = merge_folders(&[], &remote, &BTreeMap::new()).unwrap();

        assert_eq!(merge.folders, vec![remote_folder]);
        assert_eq!(merge.imported_count, 1);
        assert!(merge.upload_ids.is_empty());
    }

    #[test]
    fn creates_conflict_copy_for_dueling_changes() {
        let id = Uuid::new_v4();
        let original = sample_request(id, "Original", r#"{"ok":true}"#);
        let mut local = original.clone();
        local.json_body = r#"{"side":"local"}"#.to_string();
        let mut remote_request = original.clone();
        remote_request.json_body = r#"{"side":"remote"}"#.to_string();
        let last_synced = BTreeMap::from([(id, request_hash(&original).unwrap())]);
        let remote = BTreeMap::from([(id, remote_request.clone())]);

        let merge = merge_requests(&[local.clone()], &remote, &last_synced).unwrap();

        assert_eq!(merge.requests.len(), 2);
        assert!(
            merge
                .requests
                .iter()
                .any(|request| request.id == id && request.json_body == local.json_body)
        );
        assert!(
            merge
                .requests
                .iter()
                .any(|request| request.id != id && request.json_body == remote_request.json_body)
        );
        assert_eq!(merge.conflict_count, 1);
        assert!(merge.warning.unwrap().contains("conflict"));
    }

    #[test]
    fn preserves_local_when_remote_request_disappears() {
        let id = Uuid::new_v4();
        let local = sample_request(id, "Local", r#"{"ok":true}"#);
        let last_synced = BTreeMap::from([(id, request_hash(&local).unwrap())]);

        let merge = merge_requests(&[local.clone()], &BTreeMap::new(), &last_synced).unwrap();

        assert_eq!(merge.requests, vec![local]);
        assert!(merge.upload_ids.contains(&id));
        assert!(merge.warning.unwrap().contains("missing"));
    }
}
