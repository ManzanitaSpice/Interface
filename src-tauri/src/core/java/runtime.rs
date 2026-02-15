use std::cmp::Ordering;
use std::collections::HashMap;
use std::io::{Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use chrono::Utc;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tracing::{debug, info, instrument, warn};
use uuid::Uuid;

use crate::core::error::{LauncherError, LauncherResult};

use super::paths::{runtime_paths, RuntimePaths};

const ADOPTIUM_API_BASE: &str = "https://api.adoptium.net/v3/assets/latest";
const RESOLVED_CACHE_FILE: &str = "resolved_java.json";
const RUNTIME_SCHEMA_VERSION: u32 = 3;
const RUNTIME_LOCK_STALE_SECS: i64 = 60 * 10;
const RUNTIME_KEEP_PER_MAJOR: usize = 2;
const RUNTIME_USER_AGENT: &str = "InterfaceOficial-RuntimeManager/1.0";
const ADOPTIUM_CACHE_FILE: &str = "adoptium_cache.json";
const ADOPTIUM_CACHE_TTL_SECS: i64 = 60 * 30;
const GLOBAL_BACKOFF_429_FILE: &str = "adoptium_backoff_429.json";
const GLOBAL_BACKOFF_429_SECS: i64 = 30;
const MIN_FREE_DISK_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("io error at {path:?}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("zip error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid runtime: {0}")]
    InvalidRuntime(String),
}

impl From<RuntimeError> for LauncherError {
    fn from(value: RuntimeError) -> Self {
        match value {
            RuntimeError::Io { path, source } => LauncherError::Io { path, source },
            RuntimeError::Http(source) => LauncherError::Http(source),
            RuntimeError::Zip(source) => LauncherError::Zip(source),
            RuntimeError::Json(source) => LauncherError::Json(source),
            RuntimeError::InvalidRuntime(message) => LauncherError::Other(message),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JavaInstallation {
    pub path: PathBuf,
    pub version: String,
    pub major: u32,
    pub is_64bit: bool,
    pub vendor: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedRuntimeInfo {
    pub identifier: String,
    pub major: u32,
    pub vendor: String,
    pub version: String,
    pub arch: String,
    pub root: PathBuf,
    pub java_bin: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RuntimeMetadata {
    schema_version: u32,
    identifier: String,
    major: u32,
    vendor: String,
    version: String,
    arch: String,
    sha256_zip: String,
    sha256_java: String,
    installed_at: String,
    source_url: String,
    launcher_version: String,
    chmod_applied: bool,
    java_bin_rel: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct RuntimeIndex {
    runtimes: Vec<RuntimeMetadata>,
}

#[derive(Debug, Clone)]
struct RuntimeCandidate {
    metadata: RuntimeMetadata,
    root: PathBuf,
    java_bin: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
struct AdoptiumRelease {
    binary: AdoptiumBinary,
    version: AdoptiumVersion,
}

#[derive(Debug, Clone, Deserialize)]
struct AdoptiumBinary {
    package: AdoptiumPackage,
}

#[derive(Debug, Clone, Deserialize)]
struct AdoptiumPackage {
    checksum: String,
    link: String,
}

#[derive(Debug, Clone, Deserialize)]
struct AdoptiumVersion {
    openjdk_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DownloadRuntimeSpec {
    major: u32,
    arch: String,
    vendor: String,
    version: String,
    url: String,
    sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ResolutionCache {
    by_major: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct AdoptiumCache {
    entries: HashMap<String, CachedRuntimeSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedRuntimeSpec {
    stored_at: i64,
    spec: DownloadRuntimeSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DownloadCheckpoint {
    downloaded_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Backoff429State {
    until_ts: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RuntimeDiagnostic {
    pub app_data_dir: String,
    pub resource_dir: String,
    pub temp_dir: String,
    pub runtimes_root: String,
    pub indexed_runtimes: usize,
}

#[derive(Debug, Clone)]
pub struct RuntimeManager {
    paths: RuntimePaths,
    client: reqwest::Client,
}

impl RuntimeManager {
    pub fn new(paths: RuntimePaths) -> LauncherResult<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .user_agent(RUNTIME_USER_AGENT)
            .build()?;
        Ok(Self { paths, client })
    }

    pub fn from_global_paths() -> LauncherResult<Self> {
        Self::new(runtime_paths()?.clone())
    }

    pub async fn list_runtimes(&self) -> LauncherResult<Vec<ManagedRuntimeInfo>> {
        let runtimes_root = self.paths.app_data_dir().join("runtimes");
        let mut out = Vec::new();
        let candidates =
            select::scan_runtime_candidates(&runtimes_root, &platform::platform_arch()).await?;
        for candidate in candidates {
            out.push(ManagedRuntimeInfo {
                identifier: candidate.metadata.identifier,
                major: candidate.metadata.major,
                vendor: candidate.metadata.vendor,
                version: candidate.metadata.version,
                arch: candidate.metadata.arch,
                root: candidate.root,
                java_bin: candidate.java_bin,
            });
        }
        Ok(out)
    }

    pub async fn resolve_java(&self, required_major: u32) -> LauncherResult<PathBuf> {
        resolve_java_binary_in_dir(self.paths.app_data_dir(), required_major).await
    }

    pub fn validate_java(&self, path: &Path, required_major: u32) -> bool {
        runtime_is_valid(path, required_major)
    }

    pub async fn clear_runtimes(&self) -> LauncherResult<()> {
        let runtimes_root = self.paths.app_data_dir().join("runtimes");
        if runtimes_root.exists() {
            tokio::fs::remove_dir_all(&runtimes_root)
                .await
                .map_err(|source| LauncherError::Io {
                    path: runtimes_root.clone(),
                    source,
                })?;
        }
        tokio::fs::create_dir_all(&runtimes_root)
            .await
            .map_err(|source| LauncherError::Io {
                path: runtimes_root,
                source,
            })
    }

    pub async fn diagnostics(&self) -> LauncherResult<RuntimeDiagnostic> {
        let runtimes_root = self.paths.app_data_dir().join("runtimes");
        let indexed = read_runtime_index(&runtimes_root).await?.runtimes.len();
        Ok(RuntimeDiagnostic {
            app_data_dir: self.paths.app_data_dir().to_string_lossy().to_string(),
            resource_dir: self.paths.resource_dir().to_string_lossy().to_string(),
            temp_dir: self.paths.temp_dir().to_string_lossy().to_string(),
            runtimes_root: runtimes_root.to_string_lossy().to_string(),
            indexed_runtimes: indexed,
        })
    }

    pub fn http_client(&self) -> &reqwest::Client {
        &self.client
    }
}

pub fn managed_runtime_dir(data_dir: &Path, major: u32) -> PathBuf {
    data_dir
        .join("runtimes")
        .join(format!("java{}", runtime_track(major)))
}

pub async fn resolve_java_binary(required_major: u32) -> LauncherResult<PathBuf> {
    let base_dir = launcher_base_dir();
    resolve_java_binary_in_dir(&base_dir, required_major).await
}

pub async fn ensure_embedded_runtime_registered(data_dir: &Path) -> LauncherResult<()> {
    let embedded_root = data_dir.join("runtime");
    let embedded_java = locate_java_binary(&embedded_root);
    let Some(info) = probe::probe_java(&embedded_java) else {
        return Ok(());
    };

    if !info.is_64bit {
        return Ok(());
    }

    let runtime_major = runtime_track(info.major);
    let java_sha256 = sha256_file(&embedded_java)?;
    let metadata = RuntimeMetadata {
        schema_version: RUNTIME_SCHEMA_VERSION,
        identifier: format!(
            "java{}-embedded-{}-{}",
            runtime_major,
            normalize_version_for_id(&info.version),
            platform::platform_arch()
        ),
        major: runtime_major,
        vendor: if info.vendor == "unknown" {
            "Embedded".to_string()
        } else {
            info.vendor
        },
        version: info.version,
        arch: platform::platform_arch(),
        sha256_zip: String::new(),
        sha256_java: java_sha256,
        installed_at: Utc::now().to_rfc3339(),
        source_url: "embedded://bundled".to_string(),
        launcher_version: env!("CARGO_PKG_VERSION").to_string(),
        chmod_applied: true,
        java_bin_rel: None,
    };

    let runtimes_root = data_dir.join("runtimes");
    tokio::fs::create_dir_all(&runtimes_root)
        .await
        .map_err(|source| LauncherError::Io {
            path: runtimes_root.clone(),
            source,
        })?;

    let runtime_root = runtimes_root.join(&metadata.identifier);
    if !runtime_root.exists() {
        copy_dir_recursive(&embedded_root, &runtime_root)?;
    }
    ensure_java_executable_once(&runtime_root, &metadata).await?;
    write_runtime_metadata(&runtime_root, &metadata).await?;
    update_runtime_index(&runtimes_root, &metadata).await?;
    Ok(())
}

pub async fn managed_runtime_info_in_dir(
    data_dir: &Path,
    required_major: u32,
) -> LauncherResult<Option<ManagedRuntimeInfo>> {
    info!(
        "java runtime startup platform_os={} platform_arch={}",
        platform::platform_os(),
        platform::platform_arch()
    );
    let runtime_major = runtime_track(required_major);
    let arch = platform::platform_arch();
    let runtimes_root = data_dir.join("runtimes");

    let Some(candidate) =
        select::best_compatible_runtime(&runtimes_root, runtime_major, &arch).await?
    else {
        return Ok(None);
    };

    Ok(Some(ManagedRuntimeInfo {
        identifier: candidate.metadata.identifier,
        major: candidate.metadata.major,
        vendor: candidate.metadata.vendor,
        version: candidate.metadata.version,
        arch: candidate.metadata.arch,
        root: candidate.root,
        java_bin: candidate.java_bin,
    }))
}

pub async fn managed_runtime_info(
    required_major: u32,
) -> LauncherResult<Option<ManagedRuntimeInfo>> {
    let base_dir = launcher_base_dir();
    managed_runtime_info_in_dir(&base_dir, required_major).await
}

#[instrument(skip(data_dir))]
pub async fn resolve_java_binary_in_dir(
    data_dir: &Path,
    required_major: u32,
) -> LauncherResult<PathBuf> {
    let runtime_major = runtime_track(required_major);
    let runtimes_root = data_dir.join("runtimes");
    tokio::fs::create_dir_all(&runtimes_root)
        .await
        .map_err(|source| LauncherError::Io {
            path: runtimes_root.clone(),
            source,
        })?;
    cleanup_abandoned_runtime_locks(&runtimes_root).await;

    let arch = platform::platform_arch();

    if let Some(cached) = read_resolution_cache(data_dir, runtime_major)?
        && runtime_is_valid(&cached, runtime_major)
    {
        return Ok(cached);
    }

    if let Some(existing) =
        select::best_compatible_runtime(&runtimes_root, runtime_major, &arch).await?
    {
        write_resolution_cache(data_dir, runtime_major, &existing.java_bin).await?;
        return Ok(existing.java_bin);
    }

    let lock_path = runtimes_root.join(format!(".downloading_java{}_{}.lock", runtime_major, arch));
    let _lock = acquire_runtime_lock(&lock_path).await?;

    if let Some(existing) =
        select::best_compatible_runtime(&runtimes_root, runtime_major, &arch).await?
    {
        write_resolution_cache(data_dir, runtime_major, &existing.java_bin).await?;
        return Ok(existing.java_bin);
    }

    match install_runtime(&runtimes_root, runtime_major, &arch).await {
        Ok(installed) => {
            write_resolution_cache(data_dir, runtime_major, &installed).await?;
            Ok(installed)
        }
        Err(err) => {
            if let Some(existing) =
                select::any_compatible_runtime(&runtimes_root, runtime_major, &arch).await?
            {
                warn!(
                    "Runtime install failed, using cached runtime {}: {}",
                    existing.metadata.identifier, err
                );
                write_resolution_cache(data_dir, runtime_major, &existing.java_bin).await?;
                return Ok(existing.java_bin);
            }
            Err(err)
        }
    }
}

pub async fn detect_java_installations() -> Vec<JavaInstallation> {
    detect_java_installations_sync()
}

pub fn detect_java_installations_sync() -> Vec<JavaInstallation> {
    let data_dir = launcher_base_dir();
    let runtimes_root = data_dir.join("runtimes");
    let mut detected: Vec<JavaInstallation> = Vec::new();

    if let Ok(entries) = std::fs::read_dir(&runtimes_root) {
        detected.extend(
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.is_dir())
                .filter_map(|runtime_root| {
                    let java_bin = locate_java_binary(&runtime_root);
                    probe::probe_java(&java_bin)
                }),
        );
    }

    if let Ok(output) = Command::new("java").arg("-version").output() {
        let temp_path = if cfg!(windows) {
            PathBuf::from("java.exe")
        } else {
            PathBuf::from("java")
        };
        let version_output = format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        if let Some(version) = version_output.lines().find_map(|line| {
            let start = line.find('"')?;
            let end = line[start + 1..].find('"')?;
            Some(line[start + 1..start + 1 + end].to_string())
        }) {
            let major = parse_major_version(&version);
            let lower = version_output.to_ascii_lowercase();
            let is_64bit = lower.contains("sun.arch.data.model = 64")
                || lower.contains("os.arch = amd64")
                || lower.contains("os.arch = x86_64")
                || lower.contains("os.arch = aarch64");
            detected.push(JavaInstallation {
                path: temp_path,
                version,
                major,
                is_64bit,
                vendor: "system".to_string(),
            });
        }
    }

    detected.sort_by(|a, b| a.path.cmp(&b.path));
    detected.dedup_by(|a, b| a.path == b.path);
    detected
}

#[instrument(skip(runtimes_root))]
async fn install_runtime(
    runtimes_root: &Path,
    required_major: u32,
    arch: &str,
) -> LauncherResult<PathBuf> {
    let spec = download::fetch_runtime_spec(required_major, arch).await?;
    let identifier = format!(
        "java{}-{}-{}-{}",
        spec.major,
        spec.vendor.to_lowercase(),
        normalize_version_for_id(&spec.version),
        spec.arch
    );

    let runtime_root = runtimes_root.join(&identifier);
    let staging_id = Uuid::new_v4().to_string();
    let temp_root = runtimes_root.join("temp").join(format!("{staging_id}_dir"));
    let zip_path = runtimes_root.join("temp").join(format!("{staging_id}.zip"));

    if temp_root.exists() {
        let _ = tokio::fs::remove_dir_all(&temp_root).await;
    }

    if let Some(parent) = zip_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|source| LauncherError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
    }

    let download_start = Instant::now();
    info!("Downloading runtime {} from {}", identifier, spec.url);
    ensure_min_disk_space(runtimes_root, MIN_FREE_DISK_BYTES)?;
    download::download_to_file_with_hash(&spec.url, &zip_path, &spec.sha256).await?;
    info!(
        "Runtime download finished in {:?}",
        download_start.elapsed()
    );

    let extract_start = Instant::now();
    ensure_min_disk_space(runtimes_root, MIN_FREE_DISK_BYTES)?;
    extract::extract_zip_file(&zip_path, &temp_root)?;
    info!(
        "Runtime extraction finished in {:?}",
        extract_start.elapsed()
    );

    let mut metadata = RuntimeMetadata {
        schema_version: RUNTIME_SCHEMA_VERSION,
        identifier: identifier.clone(),
        major: required_major,
        vendor: spec.vendor,
        version: spec.version,
        arch: spec.arch,
        sha256_zip: spec.sha256.clone(),
        sha256_java: String::new(),
        installed_at: Utc::now().to_rfc3339(),
        source_url: spec.url.clone(),
        launcher_version: env!("CARGO_PKG_VERSION").to_string(),
        chmod_applied: false,
        java_bin_rel: None,
    };

    ensure_java_executable_once(&temp_root, &metadata).await?;
    metadata.chmod_applied = true;

    let java_bin = locate_java_binary(&temp_root);
    if !runtime_is_valid(&java_bin, required_major) {
        let _ = tokio::fs::remove_file(&zip_path).await;
        let _ = tokio::fs::remove_dir_all(&temp_root).await;
        return Err(LauncherError::Other(format!(
            "Downloaded runtime failed validation: {}",
            java_bin.display()
        )));
    }

    metadata.sha256_java = sha256_file(&java_bin)?;
    metadata.java_bin_rel = java_bin
        .strip_prefix(&temp_root)
        .ok()
        .map(|p| p.to_string_lossy().to_string());
    write_runtime_metadata(&temp_root, &metadata).await?;

    let backup_root = runtime_root.with_extension("backup");
    if backup_root.exists() {
        let _ = tokio::fs::remove_dir_all(&backup_root).await;
    }
    if runtime_root.exists() {
        tokio::fs::rename(&runtime_root, &backup_root)
            .await
            .map_err(|source| LauncherError::Io {
                path: backup_root.clone(),
                source,
            })?;
    }

    if let Err(source) = tokio::fs::rename(&temp_root, &runtime_root).await {
        if backup_root.exists() {
            let _ = tokio::fs::rename(&backup_root, &runtime_root).await;
        }
        return Err(LauncherError::Io {
            path: runtime_root.clone(),
            source,
        });
    }

    let _ = tokio::fs::remove_file(&zip_path).await;
    let _ = tokio::fs::remove_dir_all(&backup_root).await;
    update_runtime_index(runtimes_root, &metadata).await?;
    cleanup_old_runtimes(runtimes_root, required_major, arch).await?;

    let final_java = locate_java_binary(&runtime_root);
    if probe::probe_java(&final_java).is_none() {
        return Err(LauncherError::Other(format!(
            "Final java binary no arranca con -version: {}",
            final_java.display()
        )));
    }

    Ok(final_java)
}

async fn write_runtime_metadata(
    runtime_root: &Path,
    metadata: &RuntimeMetadata,
) -> LauncherResult<()> {
    let metadata_path = runtime_root.join("runtime.json");
    let payload = serde_json::to_vec_pretty(metadata)?;
    tokio::fs::write(&metadata_path, payload)
        .await
        .map_err(|source| LauncherError::Io {
            path: metadata_path,
            source,
        })
}

async fn update_runtime_index(
    runtimes_root: &Path,
    new_metadata: &RuntimeMetadata,
) -> LauncherResult<()> {
    let index_path = runtimes_root.join("index.json");
    let mut index = match tokio::fs::read(&index_path).await {
        Ok(bytes) => serde_json::from_slice::<RuntimeIndex>(&bytes).unwrap_or_default(),
        Err(_) => RuntimeIndex::default(),
    };

    index.runtimes.retain(|rt| {
        !(rt.major == new_metadata.major
            && rt.arch == new_metadata.arch
            && rt.identifier == new_metadata.identifier)
    });
    index.runtimes.push(new_metadata.clone());

    let payload = serde_json::to_vec_pretty(&index)?;
    tokio::fs::write(&index_path, payload)
        .await
        .map_err(|source| LauncherError::Io {
            path: index_path,
            source,
        })
}

async fn read_runtime_index(runtimes_root: &Path) -> LauncherResult<RuntimeIndex> {
    let index_path = runtimes_root.join("index.json");
    let bytes = match tokio::fs::read(&index_path).await {
        Ok(bytes) => bytes,
        Err(_) => return Ok(RuntimeIndex::default()),
    };
    Ok(serde_json::from_slice::<RuntimeIndex>(&bytes).unwrap_or_default())
}

#[instrument(skip(runtimes_root))]
async fn cleanup_old_runtimes(runtimes_root: &Path, major: u32, arch: &str) -> LauncherResult<()> {
    let mut index = read_runtime_index(runtimes_root).await?;
    let mut same_major = index
        .runtimes
        .iter()
        .filter(|rt| rt.major == major && rt.arch == arch)
        .cloned()
        .collect::<Vec<_>>();

    same_major.sort_by(|a, b| a.installed_at.cmp(&b.installed_at).reverse());
    for stale in same_major.into_iter().skip(RUNTIME_KEEP_PER_MAJOR) {
        let path = runtimes_root.join(&stale.identifier);
        if path.exists() {
            let _ = tokio::fs::remove_dir_all(&path).await;
        }
        index
            .runtimes
            .retain(|rt| rt.identifier != stale.identifier);
    }

    let index_path = runtimes_root.join("index.json");
    let payload = serde_json::to_vec_pretty(&index)?;
    tokio::fs::write(index_path, payload).await?;
    Ok(())
}

async fn acquire_runtime_lock(lock_path: &Path) -> LauncherResult<RuntimeLockGuard> {
    let mut attempts = 0_u32;
    loop {
        attempts += 1;
        match tokio::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(lock_path)
            .await
        {
            Ok(mut file) => {
                let pid = std::process::id();
                let payload = serde_json::json!({
                    "pid": pid,
                    "timestamp": Utc::now().timestamp(),
                });
                file.write_all(payload.to_string().as_bytes())
                    .await
                    .map_err(|source| LauncherError::Io {
                        path: lock_path.to_path_buf(),
                        source,
                    })?;
                return Ok(RuntimeLockGuard {
                    path: lock_path.to_path_buf(),
                });
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                cleanup_stale_lock(lock_path).await;
                if attempts % 20 == 0 {
                    info!("Waiting for runtime lock at {:?}", lock_path);
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            Err(source) => {
                return Err(LauncherError::Io {
                    path: lock_path.to_path_buf(),
                    source,
                })
            }
        }
    }
}

async fn cleanup_stale_lock(lock_path: &Path) {
    if let Ok(content) = tokio::fs::read_to_string(lock_path).await
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(&content)
    {
        let pid = value
            .get("pid")
            .and_then(|v| v.as_u64())
            .unwrap_or_default() as u32;
        let timestamp = value
            .get("timestamp")
            .and_then(|v| v.as_i64())
            .unwrap_or_default();
        let expired = Utc::now().timestamp().saturating_sub(timestamp) > RUNTIME_LOCK_STALE_SECS;

        #[cfg(target_os = "linux")]
        let dead = !PathBuf::from(format!("/proc/{pid}")).exists();
        #[cfg(not(target_os = "linux"))]
        let dead = false;

        if expired || dead {
            let _ = tokio::fs::remove_file(lock_path).await;
        }
    }
}

struct RuntimeLockGuard {
    path: PathBuf,
}

impl Drop for RuntimeLockGuard {
    fn drop(&mut self) {
        if let Err(source) = std::fs::remove_file(&self.path) {
            warn!("Failed to remove lock {:?}: {}", self.path, source);
        }
    }
}

fn runtime_is_valid(java_bin: &Path, required_major: u32) -> bool {
    let Some(info) = probe::probe_java(java_bin) else {
        return false;
    };

    info.major == required_major && info.is_64bit
}

fn runtime_hash_matches(candidate: &RuntimeCandidate) -> bool {
    let expected = candidate.metadata.sha256_java.trim();
    if expected.is_empty() {
        return true;
    }
    match sha256_file(&candidate.java_bin) {
        Ok(actual) => actual.eq_ignore_ascii_case(expected),
        Err(_) => false,
    }
}

fn ensure_min_disk_space(path: &Path, minimum_bytes: u64) -> LauncherResult<()> {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut best_len = 0usize;
    let mut available = None;
    for disk in disks.list() {
        let mount = disk.mount_point();
        if canonical.starts_with(mount) {
            let len = mount.as_os_str().len();
            if len >= best_len {
                best_len = len;
                available = Some(disk.available_space());
            }
        }
    }
    if let Some(bytes) = available
        && bytes < minimum_bytes
    {
        return Err(LauncherError::Other(format!(
            "Espacio insuficiente para instalar runtime: disponible={} requerido={}",
            bytes, minimum_bytes
        )));
    }
    Ok(())
}

async fn cleanup_abandoned_runtime_locks(runtimes_root: &Path) {
    let mut entries = match tokio::fs::read_dir(runtimes_root).await {
        Ok(entries) => entries,
        Err(_) => return,
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("lock") {
            cleanup_stale_lock(&path).await;
        }
    }
}

fn runtime_track(required_major: u32) -> u32 {
    if required_major <= 8 {
        8
    } else if required_major >= 21 {
        21
    } else {
        17
    }
}

pub fn required_java_for_minecraft_version(minecraft_version: &str) -> u32 {
    let lower = minecraft_version.to_ascii_lowercase();
    if let Some(week_pos) = lower.find('w') {
        let year_hint = &lower[..week_pos];
        if year_hint.len() >= 2 {
            let year_suffix = &year_hint[year_hint.len() - 2..];
            if let Ok(snapshot_year) = year_suffix.parse::<u32>() {
                if snapshot_year >= 24 {
                    return 21;
                }
                return 17;
            }
        }
    }

    let mut parts = minecraft_version.split('.');
    let major = parts
        .next()
        .and_then(|p| p.parse::<u32>().ok())
        .unwrap_or(1);
    let minor = parts
        .next()
        .and_then(|p| p.parse::<u32>().ok())
        .unwrap_or(20);
    let patch = parts
        .next()
        .and_then(|p| p.parse::<u32>().ok())
        .unwrap_or(0);

    if major > 1 || minor >= 21 || (minor == 20 && patch >= 5) {
        21
    } else if minor >= 17 {
        17
    } else {
        8
    }
}

pub fn is_java_compatible_major(installed_major: u32, required_major: u32) -> bool {
    installed_major >= required_major
        && runtime_track(installed_major) == runtime_track(required_major)
}

pub fn is_usable_java_binary(path: &Path) -> bool {
    probe::probe_java(path).is_some()
}

pub fn inspect_java_binary(path: &Path) -> Option<JavaInstallation> {
    probe::probe_java(path)
}

fn parse_major_version(version: &str) -> u32 {
    let first_part = version.split('.').next().unwrap_or("0");
    let major: u32 = first_part.parse().unwrap_or(0);

    if major == 1 {
        version
            .split('.')
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(major)
    } else {
        major
    }
}

fn parse_java_version(version: &str) -> Option<(u32, u32, u32, u32)> {
    let cleaned = clean_openjdk_version(version);
    let (core, build) = cleaned.split_once('+').unwrap_or((cleaned.as_str(), "0"));
    let mut nums = core
        .split('.')
        .filter_map(|part| part.parse::<u32>().ok())
        .collect::<Vec<_>>();
    while nums.len() < 3 {
        nums.push(0);
    }
    let build = build
        .split('.')
        .next()
        .and_then(|b| b.parse::<u32>().ok())
        .unwrap_or(0);
    Some((nums[0], nums[1], nums[2], build))
}

fn compare_java_versions(left: &str, right: &str) -> Option<Ordering> {
    let l = parse_java_version(left)?;
    let r = parse_java_version(right)?;
    Some(l.cmp(&r))
}

fn clean_openjdk_version(raw: &str) -> String {
    raw.split('-').next().unwrap_or(raw).to_string()
}

fn normalize_version_for_id(version: &str) -> String {
    version.replace('+', "_").replace(' ', "")
}

fn java_exe() -> &'static str {
    if cfg!(windows) {
        "java.exe"
    } else {
        "java"
    }
}

fn locate_java_binary(runtime_root: &Path) -> PathBuf {
    let primary = runtime_root.join("bin").join(java_exe());
    if primary.exists() {
        return primary;
    }

    let mac_layout = runtime_root
        .join("Contents")
        .join("Home")
        .join("bin")
        .join(java_exe());
    if mac_layout.exists() {
        return mac_layout;
    }

    find_java_binary_recursive(runtime_root).unwrap_or(primary)
}

fn find_java_binary_recursive(root: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(root).ok()?;
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let file_type = entry.file_type().ok()?;

        if file_type.is_file() {
            if path.file_name().and_then(|n| n.to_str()) == Some(java_exe()) {
                return Some(path);
            }
        } else if file_type.is_dir()
            && let Some(found) = find_java_binary_recursive(&path)
        {
            return Some(found);
        }
    }
    None
}

async fn ensure_java_executable_once(
    runtime_root: &Path,
    metadata: &RuntimeMetadata,
) -> LauncherResult<()> {
    if metadata.chmod_applied {
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let java_bin = locate_java_binary(runtime_root);
        if java_bin.exists() {
            let mut perms = std::fs::metadata(&java_bin)
                .map_err(|source| LauncherError::Io {
                    path: java_bin.clone(),
                    source,
                })?
                .permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&java_bin, perms).map_err(|source| LauncherError::Io {
                path: java_bin,
                source,
            })?;
        }
    }
    Ok(())
}

fn copy_dir_recursive(source: &Path, destination: &Path) -> LauncherResult<()> {
    std::fs::create_dir_all(destination).map_err(|source_err| LauncherError::Io {
        path: destination.to_path_buf(),
        source: source_err,
    })?;

    for entry in std::fs::read_dir(source).map_err(|source_err| LauncherError::Io {
        path: source.to_path_buf(),
        source: source_err,
    })? {
        let entry = entry.map_err(|source_err| LauncherError::Io {
            path: source.to_path_buf(),
            source: source_err,
        })?;
        let src_path = entry.path();
        let dst_path = destination.join(entry.file_name());
        let file_type = entry.file_type().map_err(|source_err| LauncherError::Io {
            path: src_path.clone(),
            source: source_err,
        })?;

        if file_type.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else if file_type.is_file() {
            std::fs::copy(&src_path, &dst_path).map_err(|source_err| LauncherError::Io {
                path: dst_path,
                source: source_err,
            })?;
        }
    }

    Ok(())
}

fn sha256_file(path: &Path) -> LauncherResult<String> {
    let bytes = std::fs::read(path).map_err(|source| LauncherError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(format!("{:x}", hasher.finalize()))
}

fn resolved_cache_path(data_dir: &Path) -> PathBuf {
    data_dir.join(RESOLVED_CACHE_FILE)
}

fn launcher_base_dir() -> PathBuf {
    runtime_paths()
        .map(|paths| paths.app_data_dir().to_path_buf())
        .unwrap_or_else(|_| PathBuf::from("."))
}

fn read_resolution_cache(data_dir: &Path, major: u32) -> LauncherResult<Option<PathBuf>> {
    let path = resolved_cache_path(data_dir);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(_) => return Ok(None),
    };
    let cache: ResolutionCache = serde_json::from_slice(&bytes).unwrap_or_default();
    let Some(stored) = cache.by_major.get(&major.to_string()) else {
        return Ok(None);
    };
    let path = PathBuf::from(stored);
    Ok(std::fs::canonicalize(path).ok())
}

async fn write_resolution_cache(
    data_dir: &Path,
    major: u32,
    java_bin: &Path,
) -> LauncherResult<()> {
    let path = resolved_cache_path(data_dir);
    let mut cache = match tokio::fs::read(&path).await {
        Ok(bytes) => serde_json::from_slice::<ResolutionCache>(&bytes).unwrap_or_default(),
        Err(_) => ResolutionCache::default(),
    };
    let canonical = std::fs::canonicalize(java_bin).unwrap_or_else(|_| java_bin.to_path_buf());
    cache
        .by_major
        .insert(major.to_string(), canonical.to_string_lossy().to_string());
    let payload = serde_json::to_vec_pretty(&cache)?;
    tokio::fs::write(path, payload).await?;
    Ok(())
}

mod platform {
    pub fn platform_arch() -> String {
        match std::env::consts::ARCH {
            "x86_64" => "x64".to_string(),
            "aarch64" => "arm64".to_string(),
            other => other.to_string(),
        }
    }

    pub fn platform_os() -> &'static str {
        match std::env::consts::OS {
            "windows" => "windows",
            "linux" => "linux",
            "macos" => "mac",
            _ => "windows",
        }
    }
}

mod probe {
    use super::*;

    #[instrument]
    pub fn probe_java(path: &Path) -> Option<JavaInstallation> {
        let output = Command::new(path)
            .args(["-XshowSettings:properties", "-version"])
            .output()
            .ok()?;

        parse_output(path, output)
    }

    fn parse_output(path: &Path, output: std::process::Output) -> Option<JavaInstallation> {
        let version_output = format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        debug!(
            "Probing {:?}: {}",
            path,
            version_output.lines().next().unwrap_or("")
        );

        let version_str = parse_version_string(&version_output)?;
        let major = parse_major_version(&version_str);
        let lower_output = version_output.to_ascii_lowercase();
        let is_64bit = lower_output.contains("sun.arch.data.model = 64")
            || lower_output.contains("os.arch = amd64")
            || lower_output.contains("os.arch = x86_64")
            || lower_output.contains("os.arch = aarch64");
        let vendor = parse_vendor(&version_output);

        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());

        Some(JavaInstallation {
            path: canonical,
            version: version_str,
            major,
            is_64bit,
            vendor,
        })
    }

    fn parse_version_string(output: &str) -> Option<String> {
        for line in output.lines() {
            if let Some(start) = line.find('"')
                && let Some(end) = line[start + 1..].find('"')
            {
                return Some(line[start + 1..start + 1 + end].to_string());
            }
        }
        None
    }

    fn parse_vendor(output: &str) -> String {
        for line in output.lines() {
            if line.contains("Temurin") {
                return "Temurin".to_string();
            }
            if line.contains("Adoptium") {
                return "Adoptium".to_string();
            }
            if line.contains("OpenJDK") {
                return "OpenJDK".to_string();
            }
        }
        "unknown".to_string()
    }
}

mod download {
    use super::*;

    pub async fn fetch_runtime_spec(
        required_major: u32,
        arch: &str,
    ) -> LauncherResult<DownloadRuntimeSpec> {
        let cache_key = format!("{}:{}:{}", required_major, arch, platform::platform_os());
        if let Some(spec) = read_cached_spec(&cache_key)? {
            return Ok(spec);
        }

        let client = http_client()?;
        let mut last_download_error: Option<LauncherError> = None;
        let mut resolved_spec: Option<DownloadRuntimeSpec> = None;

        for image_type in ["jre", "jdk"] {
            let api_url = format!(
                "{}/{}/hotspot?architecture={}&image_type={}&os={}",
                ADOPTIUM_API_BASE,
                required_major,
                arch,
                image_type,
                platform::platform_os()
            );

            match get_with_retry(&client, &api_url, 3, 0).await {
                Ok(response) => {
                    let status = response.status();
                    if !status.is_success() {
                        last_download_error = Some(LauncherError::DownloadFailed {
                            url: api_url,
                            status: status.as_u16(),
                        });
                        continue;
                    }

                    let releases: Vec<AdoptiumRelease> = response.json().await?;
                    if let Some(found) = releases.into_iter().next() {
                        resolved_spec = Some(DownloadRuntimeSpec {
                            major: required_major,
                            arch: arch.to_string(),
                            vendor: "Temurin".to_string(),
                            version: clean_openjdk_version(&found.version.openjdk_version),
                            url: found.binary.package.link,
                            sha256: found.binary.package.checksum,
                        });
                        break;
                    }
                }
                Err(source) => last_download_error = Some(source),
            }
        }

        let Some(spec) = resolved_spec else {
            if let Some(error) = last_download_error {
                return Err(error);
            }
            return Err(LauncherError::Other(format!(
                "No runtime release found for Java {} ({arch})",
                required_major
            )));
        };

        write_cached_spec(&cache_key, &spec)?;
        Ok(spec)
    }

    pub async fn download_to_file_with_hash(
        url: &str,
        output_path: &Path,
        expected_sha256: &str,
    ) -> LauncherResult<()> {
        let checkpoint_path = output_path.with_extension("checkpoint.json");
        let mut start_offset = 0_u64;
        if output_path.exists() {
            start_offset = tokio::fs::metadata(output_path)
                .await
                .map(|m| m.len())
                .unwrap_or_default();
        }

        if checkpoint_path.exists()
            && let Ok(bytes) = tokio::fs::read(&checkpoint_path).await
            && let Ok(checkpoint) = serde_json::from_slice::<DownloadCheckpoint>(&bytes)
            && checkpoint.downloaded_bytes > start_offset
        {
            start_offset = checkpoint.downloaded_bytes;
        }

        let client = http_client()?;
        let response = get_with_retry(&client, url, 3, start_offset).await?;
        let status = response.status();
        if !(status.is_success() || status.as_u16() == 206) {
            return Err(LauncherError::DownloadFailed {
                url: url.to_string(),
                status: status.as_u16(),
            });
        }

        let output = output_path.to_path_buf();
        let checkpoint = checkpoint_path.clone();
        let mut file = tokio::task::spawn_blocking(move || -> LauncherResult<std::fs::File> {
            let mut options = std::fs::OpenOptions::new();
            options.create(true).write(true);
            if start_offset > 0 && status.as_u16() == 206 {
                options.read(true);
                let mut file = options.open(&output).map_err(|source| LauncherError::Io {
                    path: output.clone(),
                    source,
                })?;
                file.seek(SeekFrom::Start(start_offset))
                    .map_err(|source| LauncherError::Io {
                        path: output.clone(),
                        source,
                    })?;
                Ok(file)
            } else {
                options.truncate(true);
                let file = options.open(&output).map_err(|source| LauncherError::Io {
                    path: output.clone(),
                    source,
                })?;
                let _ = std::fs::remove_file(&checkpoint);
                Ok(file)
            }
        })
        .await
        .map_err(|e| LauncherError::Other(format!("Task join error: {e}")))??;

        let mut stream = response.bytes_stream();
        let mut downloaded = start_offset;
        let output_for_write = output_path.to_path_buf();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            let write_buf = chunk.to_vec();
            let out = output_for_write.clone();
            file = tokio::task::spawn_blocking(move || -> LauncherResult<std::fs::File> {
                use std::io::Write;
                let mut f = file;
                f.write_all(&write_buf)
                    .map_err(|source| LauncherError::Io { path: out, source })?;
                Ok(f)
            })
            .await
            .map_err(|e| LauncherError::Other(format!("Task join error: {e}")))??;

            downloaded = downloaded.saturating_add(chunk.len() as u64);
            if downloaded % (4 * 1024 * 1024) < chunk.len() as u64 {
                let payload = serde_json::to_vec(&DownloadCheckpoint {
                    downloaded_bytes: downloaded,
                })?;
                tokio::fs::write(&checkpoint_path, payload)
                    .await
                    .map_err(|source| LauncherError::Io {
                        path: checkpoint_path.clone(),
                        source,
                    })?;
            }
        }

        let actual = sha256_file(output_path)?;
        if !actual.eq_ignore_ascii_case(expected_sha256) {
            return Err(LauncherError::Other(format!(
                "SHA-256 mismatch for {:?}: expected {}, got {}",
                output_path, expected_sha256, actual
            )));
        }
        let _ = tokio::fs::remove_file(&checkpoint_path).await;
        Ok(())
    }

    fn read_cached_spec(cache_key: &str) -> LauncherResult<Option<DownloadRuntimeSpec>> {
        let path = cache_path();
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(_) => return Ok(None),
        };
        let cache: AdoptiumCache = serde_json::from_slice(&bytes).unwrap_or_default();
        let Some(entry) = cache.entries.get(cache_key) else {
            return Ok(None);
        };
        if Utc::now().timestamp().saturating_sub(entry.stored_at) > ADOPTIUM_CACHE_TTL_SECS {
            return Ok(None);
        }
        Ok(Some(entry.spec.clone()))
    }

    fn write_cached_spec(cache_key: &str, spec: &DownloadRuntimeSpec) -> LauncherResult<()> {
        let path = cache_path();
        let mut cache = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<AdoptiumCache>(&bytes).unwrap_or_default(),
            Err(_) => AdoptiumCache::default(),
        };
        cache.entries.insert(
            cache_key.to_string(),
            CachedRuntimeSpec {
                stored_at: Utc::now().timestamp(),
                spec: spec.clone(),
            },
        );
        let payload = serde_json::to_vec_pretty(&cache)?;
        std::fs::write(path, payload)?;
        Ok(())
    }

    fn cache_path() -> PathBuf {
        launcher_base_dir().join(ADOPTIUM_CACHE_FILE)
    }

    fn backoff_path() -> PathBuf {
        launcher_base_dir().join(GLOBAL_BACKOFF_429_FILE)
    }

    fn windows_retry_multiplier() -> u64 {
        if !cfg!(windows) {
            return 1;
        }
        if detect_windows_av_or_sandbox() {
            2
        } else {
            1
        }
    }

    fn detect_windows_av_or_sandbox() -> bool {
        if !cfg!(windows) {
            return false;
        }
        let names = [
            "MsMpEng",
            "avg",
            "avast",
            "kaspersky",
            "sandboxie",
            "vboxservice",
            "vmtoolsd",
        ];
        let mut system = sysinfo::System::new_all();
        system.refresh_all();
        system.processes().values().any(|process| {
            let proc_name = process.name().to_string_lossy().to_ascii_lowercase();
            names
                .iter()
                .any(|n| proc_name.contains(&n.to_ascii_lowercase()))
        })
    }

    fn http_client() -> LauncherResult<&'static reqwest::Client> {
        use std::sync::OnceLock;
        static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
        if let Some(client) = CLIENT.get() {
            return Ok(client);
        }
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .user_agent(RUNTIME_USER_AGENT)
            .build()?;
        let _ = CLIENT.set(client);
        Ok(CLIENT.get().expect("http client set"))
    }

    async fn enforce_global_backoff_if_needed() {
        let path = backoff_path();
        let Ok(bytes) = tokio::fs::read(path).await else {
            return;
        };
        let Ok(state) = serde_json::from_slice::<Backoff429State>(&bytes) else {
            return;
        };
        let now = Utc::now().timestamp();
        if state.until_ts > now {
            tokio::time::sleep(Duration::from_secs((state.until_ts - now) as u64)).await;
        }
    }

    async fn persist_global_backoff_429() {
        let state = Backoff429State {
            until_ts: Utc::now().timestamp() + GLOBAL_BACKOFF_429_SECS,
        };
        if let Ok(payload) = serde_json::to_vec(&state) {
            let _ = tokio::fs::write(backoff_path(), payload).await;
        }
    }

    async fn get_with_retry(
        client: &reqwest::Client,
        url: &str,
        retries: u32,
        start_offset: u64,
    ) -> LauncherResult<reqwest::Response> {
        enforce_global_backoff_if_needed().await;
        let mut last_error: Option<LauncherError> = None;
        for attempt in 0..=retries {
            let mut req = client.get(url);
            if start_offset > 0 {
                req = req.header(reqwest::header::RANGE, format!("bytes={start_offset}-"));
            }
            match req.send().await {
                Ok(response) => {
                    if response.status().as_u16() == 429 {
                        persist_global_backoff_429().await;
                    }
                    return Ok(response);
                }
                Err(err) => {
                    last_error = Some(err.into());
                    if attempt < retries {
                        let backoff_ms = 2_u64.pow(attempt + 1) * 250 * windows_retry_multiplier();
                        tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                    }
                }
            }
        }
        Err(last_error.unwrap_or_else(|| LauncherError::Other(format!("failed request to {url}"))))
    }
}

mod extract {
    use super::*;

    pub fn extract_zip_file(zip_path: &Path, runtime_root: &Path) -> LauncherResult<()> {
        let zip_file = std::fs::File::open(zip_path).map_err(|source| LauncherError::Io {
            path: zip_path.to_path_buf(),
            source,
        })?;
        let mut archive = zip::ZipArchive::new(zip_file)?;

        if runtime_root.exists() {
            std::fs::remove_dir_all(runtime_root).map_err(|source| LauncherError::Io {
                path: runtime_root.to_path_buf(),
                source,
            })?;
        }

        std::fs::create_dir_all(runtime_root).map_err(|source| LauncherError::Io {
            path: runtime_root.to_path_buf(),
            source,
        })?;

        for index in 0..archive.len() {
            let mut zipped = archive.by_index(index)?;
            let mut rel_path = PathBuf::new();

            let enclosed_name = zipped
                .enclosed_name()
                .ok_or_else(|| LauncherError::Other("Invalid zip entry path".into()))?;
            let mut components = enclosed_name.components();
            let _ = components.next();
            for component in components {
                if let Component::Normal(part) = component {
                    rel_path.push(part);
                }
            }

            if rel_path.as_os_str().is_empty() {
                continue;
            }

            let out_path = runtime_root.join(rel_path);
            if zipped.name().ends_with('/') {
                std::fs::create_dir_all(&out_path).map_err(|source| LauncherError::Io {
                    path: out_path,
                    source,
                })?;
                continue;
            }

            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent).map_err(|source| LauncherError::Io {
                    path: parent.to_path_buf(),
                    source,
                })?;
            }

            let mut out = std::fs::File::create(&out_path).map_err(|source| LauncherError::Io {
                path: out_path.clone(),
                source,
            })?;
            std::io::copy(&mut zipped, &mut out).map_err(|source| LauncherError::Io {
                path: out_path,
                source,
            })?;
        }

        Ok(())
    }
}

mod select {
    use super::*;

    pub async fn best_compatible_runtime(
        runtimes_root: &Path,
        required_major: u32,
        arch: &str,
    ) -> LauncherResult<Option<RuntimeCandidate>> {
        let mut candidates = scan_runtime_candidates(runtimes_root, arch).await?;
        candidates.retain(|candidate| {
            candidate.metadata.major == required_major
                && runtime_is_valid(&candidate.java_bin, required_major)
                && runtime_hash_matches(candidate)
                && parse_java_version(&candidate.metadata.version).is_some()
        });

        candidates.sort_by(|a, b| {
            compare_java_versions(&a.metadata.version, &b.metadata.version)
                .unwrap_or(Ordering::Equal)
                .reverse()
        });

        Ok(candidates.into_iter().next())
    }

    pub async fn any_compatible_runtime(
        runtimes_root: &Path,
        required_major: u32,
        arch: &str,
    ) -> LauncherResult<Option<RuntimeCandidate>> {
        let mut candidates = scan_runtime_candidates(runtimes_root, arch).await?;
        candidates.retain(|candidate| {
            candidate.metadata.major == required_major
                && runtime_is_valid(&candidate.java_bin, required_major)
        });
        candidates.sort_by(|a, b| {
            a.metadata
                .installed_at
                .cmp(&b.metadata.installed_at)
                .reverse()
        });
        Ok(candidates.into_iter().next())
    }

    pub async fn scan_runtime_candidates(
        runtimes_root: &Path,
        arch: &str,
    ) -> LauncherResult<Vec<RuntimeCandidate>> {
        let mut candidates = Vec::new();
        let mut entries =
            tokio::fs::read_dir(runtimes_root)
                .await
                .map_err(|source| LauncherError::Io {
                    path: runtimes_root.to_path_buf(),
                    source,
                })?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|source| LauncherError::Io {
                path: runtimes_root.to_path_buf(),
                source,
            })?
        {
            let root = entry.path();
            if !root.is_dir() || root.file_name().and_then(|n| n.to_str()) == Some("temp") {
                continue;
            }

            let metadata_path = root.join("runtime.json");
            let metadata_bytes = match tokio::fs::read(&metadata_path).await {
                Ok(bytes) => bytes,
                Err(_) => continue,
            };

            let metadata: RuntimeMetadata = match serde_json::from_slice(&metadata_bytes) {
                Ok(metadata) => metadata,
                Err(_) => continue,
            };

            if metadata.arch != arch {
                continue;
            }

            let java_bin = metadata
                .java_bin_rel
                .as_ref()
                .map(|relative| root.join(relative))
                .filter(|p| p.exists())
                .unwrap_or_else(|| locate_java_binary(&root));
            candidates.push(RuntimeCandidate {
                metadata,
                root,
                java_bin,
            });
        }

        Ok(candidates)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_major_modern() {
        assert_eq!(parse_major_version("17.0.8"), 17);
        assert_eq!(parse_major_version("21.0.1"), 21);
    }

    #[test]
    fn test_parse_major_legacy() {
        assert_eq!(parse_major_version("1.8.0_392"), 8);
    }

    #[test]
    fn java_required_by_minecraft_version() {
        assert_eq!(required_java_for_minecraft_version("1.16.5"), 8);
        assert_eq!(required_java_for_minecraft_version("1.20.4"), 17);
        assert_eq!(required_java_for_minecraft_version("1.20.5"), 21);
    }

    #[test]
    fn java_runtime_track_mapping() {
        assert_eq!(runtime_track(8), 8);
        assert_eq!(runtime_track(11), 17);
        assert_eq!(runtime_track(17), 17);
        assert_eq!(runtime_track(21), 21);
    }

    #[test]
    fn java_version_comparison_prefers_newer() {
        assert_eq!(
            compare_java_versions("21.0.2+13", "21.0.3+7"),
            Some(Ordering::Less)
        );
    }
}
