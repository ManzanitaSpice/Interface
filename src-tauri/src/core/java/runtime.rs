use std::cmp::Ordering;
use std::fs::File;
use std::io::Cursor;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{debug, info, warn};
use zip::ZipArchive;

use crate::core::error::{LauncherError, LauncherResult};

const APP_DIR_NAME: &str = "InterfaceOficial";
const ADOPTIUM_API_BASE: &str = "https://api.adoptium.net/v3/assets/latest";

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
    identifier: String,
    major: u32,
    vendor: String,
    version: String,
    arch: String,
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

#[derive(Debug, Clone)]
struct DownloadRuntimeSpec {
    major: u32,
    arch: String,
    vendor: String,
    version: String,
    url: String,
    sha256: String,
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

pub async fn managed_runtime_info(
    required_major: u32,
) -> LauncherResult<Option<ManagedRuntimeInfo>> {
    let base_dir = launcher_base_dir();
    managed_runtime_info_in_dir(&base_dir, required_major).await
}

pub async fn managed_runtime_info_in_dir(
    data_dir: &Path,
    required_major: u32,
) -> LauncherResult<Option<ManagedRuntimeInfo>> {
    let runtime_major = runtime_track(required_major);
    let arch = platform_arch();
    let runtimes_root = data_dir.join("runtimes");

    let Some(candidate) = best_compatible_runtime(&runtimes_root, runtime_major, &arch).await?
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

    let arch = platform_arch();

    if let Some(existing) = best_compatible_runtime(&runtimes_root, runtime_major, &arch).await? {
        info!(
            "Reusing managed runtime {} at {:?}",
            existing.metadata.identifier, existing.java_bin
        );
        return Ok(existing.java_bin);
    }

    let lock_path = runtimes_root.join(format!(".downloading_java{}_{}.lock", runtime_major, arch));
    let _lock = acquire_runtime_lock(&lock_path).await?;

    // Re-check after lock to avoid duplicate downloads in concurrent launches.
    if let Some(existing) = best_compatible_runtime(&runtimes_root, runtime_major, &arch).await? {
        return Ok(existing.java_bin);
    }

    install_runtime(&runtimes_root, runtime_major, &arch).await
}

pub async fn detect_java_installations() -> Vec<JavaInstallation> {
    detect_java_installations_sync()
}

pub fn detect_java_installations_sync() -> Vec<JavaInstallation> {
    let data_dir = launcher_base_dir();
    let runtimes_root = data_dir.join("runtimes");
    let Ok(entries) = std::fs::read_dir(&runtimes_root) else {
        return Vec::new();
    };

    entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .filter_map(|runtime_root| {
            let java_bin = runtime_root.join("bin").join(java_exe());
            probe_java(&java_bin)
        })
        .collect()
}

async fn install_runtime(
    runtimes_root: &Path,
    required_major: u32,
    arch: &str,
) -> LauncherResult<PathBuf> {
    let spec = fetch_runtime_spec(required_major, arch).await?;
    let identifier = format!(
        "java{}-{}-{}-{}",
        spec.major,
        spec.vendor.to_lowercase(),
        normalize_version_for_id(&spec.version),
        spec.arch
    );

    let runtime_root = runtimes_root.join(&identifier);
    let temp_root = runtimes_root
        .join("temp")
        .join(format!("{}_tmp", identifier));
    let zip_path = runtimes_root
        .join("temp")
        .join(format!("{}.zip", identifier));

    if runtime_root.exists() {
        tokio::fs::remove_dir_all(&runtime_root)
            .await
            .map_err(|source| LauncherError::Io {
                path: runtime_root.clone(),
                source,
            })?;
    }

    if temp_root.exists() {
        tokio::fs::remove_dir_all(&temp_root)
            .await
            .map_err(|source| LauncherError::Io {
                path: temp_root.clone(),
                source,
            })?;
    }

    if let Some(parent) = zip_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|source| LauncherError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
    }

    info!("Downloading runtime {} from {}", identifier, spec.url);
    let bytes = download_bytes(&spec.url).await?;
    verify_sha256(&zip_path, &bytes, &spec.sha256)?;

    tokio::fs::write(&zip_path, &bytes)
        .await
        .map_err(|source| LauncherError::Io {
            path: zip_path.clone(),
            source,
        })?;

    extract_jre_zip_from_file_async(&zip_path, &temp_root).await?;

    let java_bin = temp_root.join("bin").join(java_exe());
    if !runtime_is_valid(&java_bin, required_major) {
        let _ = tokio::fs::remove_file(&zip_path).await;
        let _ = tokio::fs::remove_dir_all(&temp_root).await;
        return Err(LauncherError::Other(format!(
            "Downloaded runtime failed validation: {}",
            java_bin.display()
        )));
    }

    let metadata = RuntimeMetadata {
        identifier: identifier.clone(),
        major: required_major,
        vendor: spec.vendor,
        version: spec.version,
        arch: spec.arch,
    };
    write_runtime_metadata(&temp_root, &metadata).await?;

    tokio::fs::rename(&temp_root, &runtime_root)
        .await
        .map_err(|source| LauncherError::Io {
            path: runtime_root.clone(),
            source,
        })?;

    let _ = tokio::fs::remove_file(&zip_path).await;
    update_runtime_index(runtimes_root, &metadata).await?;

    Ok(runtime_root.join("bin").join(java_exe()))
}

async fn fetch_runtime_spec(
    required_major: u32,
    arch: &str,
) -> LauncherResult<DownloadRuntimeSpec> {
    let api_url = format!(
        "{}/{}/hotspot?architecture={}&image_type=jre&os=windows&vendor=eclipse",
        ADOPTIUM_API_BASE, required_major, arch
    );
    let response = reqwest::get(&api_url).await?;
    let status = response.status();
    if !status.is_success() {
        return Err(LauncherError::DownloadFailed {
            url: api_url,
            status: status.as_u16(),
        });
    }

    let releases: Vec<AdoptiumRelease> = response.json().await?;
    let Some(release) = releases.into_iter().next() else {
        return Err(LauncherError::Other(format!(
            "No runtime release found for Java {} ({arch})",
            required_major
        )));
    };

    Ok(DownloadRuntimeSpec {
        major: required_major,
        arch: arch.to_string(),
        vendor: "Temurin".to_string(),
        version: clean_openjdk_version(&release.version.openjdk_version),
        url: release.binary.package.link,
        sha256: release.binary.package.checksum,
    })
}

async fn download_bytes(url: &str) -> LauncherResult<Vec<u8>> {
    let response = reqwest::get(url).await?;
    let status = response.status();
    if !status.is_success() {
        return Err(LauncherError::DownloadFailed {
            url: url.to_string(),
            status: status.as_u16(),
        });
    }

    Ok(response.bytes().await?.to_vec())
}

fn verify_sha256(reference_path: &Path, bytes: &[u8], expected_sha256: &str) -> LauncherResult<()> {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let actual = format!("{:x}", hasher.finalize());
    if !actual.eq_ignore_ascii_case(expected_sha256) {
        return Err(LauncherError::Other(format!(
            "SHA-256 mismatch for {:?}: expected {}, got {}",
            reference_path, expected_sha256, actual
        )));
    }
    Ok(())
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

async fn best_compatible_runtime(
    runtimes_root: &Path,
    required_major: u32,
    arch: &str,
) -> LauncherResult<Option<RuntimeCandidate>> {
    let mut candidates = scan_runtime_candidates(runtimes_root, arch).await?;
    candidates.retain(|candidate| {
        candidate.metadata.major == required_major
            && runtime_is_valid(&candidate.java_bin, required_major)
            && parse_java_version(&candidate.metadata.version).is_some()
    });

    candidates.sort_by(|a, b| {
        compare_java_versions(&a.metadata.version, &b.metadata.version)
            .unwrap_or(Ordering::Equal)
            .reverse()
    });

    Ok(candidates.into_iter().next())
}

async fn scan_runtime_candidates(
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

        let java_bin = root.join("bin").join(java_exe());
        candidates.push(RuntimeCandidate {
            metadata,
            root,
            java_bin,
        });
    }

    Ok(candidates)
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
            Ok(_) => {
                return Ok(RuntimeLockGuard {
                    path: lock_path.to_path_buf(),
                })
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
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
    let Some(info) = probe_java(&java_bin.to_path_buf()) else {
        return false;
    };

    info.major == required_major
        && info.is_64bit
        && (info.vendor.contains("Temurin") || info.vendor.contains("Adoptium"))
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

fn extract_jre_zip(bytes: &[u8], runtime_root: &Path) -> LauncherResult<()> {
    let cursor = Cursor::new(bytes.to_vec());
    let mut archive = ZipArchive::new(cursor)?;

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

        let mut out = File::create(&out_path).map_err(|source| LauncherError::Io {
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

fn extract_jre_zip_from_file(zip_path: &Path, runtime_root: &Path) -> LauncherResult<()> {
    let bytes = std::fs::read(zip_path).map_err(|source| LauncherError::Io {
        path: zip_path.to_path_buf(),
        source,
    })?;
    extract_jre_zip(&bytes, runtime_root)
}

async fn extract_jre_zip_from_file_async(
    zip_path: &Path,
    runtime_root: &Path,
) -> LauncherResult<()> {
    let zip_path = zip_path.to_path_buf();
    let runtime_root = runtime_root.to_path_buf();

    tauri::async_runtime::spawn_blocking(move || {
        extract_jre_zip_from_file(&zip_path, &runtime_root)
    })
    .await
    .map_err(|e| LauncherError::Other(format!("Join error extracting runtime: {e}")))?
}

pub fn required_java_for_minecraft_version(minecraft_version: &str) -> u32 {
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

pub fn is_usable_java_binary(path: &Path) -> bool {
    let path_buf = path.to_path_buf();
    probe_java(&path_buf).is_some()
}

pub fn inspect_java_binary(path: &Path) -> Option<JavaInstallation> {
    probe_java(&path.to_path_buf())
}

fn probe_java(path: &PathBuf) -> Option<JavaInstallation> {
    let output = Command::new(path).arg("-version").output().ok()?;

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
    let is_64bit = version_output.contains("64-Bit");
    let vendor = parse_vendor(&version_output);

    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());

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
        if let Some(start) = line.find('"') {
            if let Some(end) = line[start + 1..].find('"') {
                return Some(line[start + 1..start + 1 + end].to_string());
            }
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
    }
    "unknown".to_string()
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

fn platform_arch() -> String {
    match std::env::consts::ARCH {
        "x86_64" => "x64".to_string(),
        "aarch64" => "arm64".to_string(),
        other => other.to_string(),
    }
}

fn java_exe() -> &'static str {
    if cfg!(windows) {
        "java.exe"
    } else {
        "java"
    }
}

fn launcher_base_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| {
            dirs::data_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(APP_DIR_NAME)
        })
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
    fn test_parse_version_string() {
        let output = "openjdk version \"17.0.8\" 2023-07-18";
        assert_eq!(parse_version_string(output), Some("17.0.8".to_string()));
    }

    #[test]
    fn java_required_by_minecraft_version() {
        assert_eq!(required_java_for_minecraft_version("1.16.5"), 8);
        assert_eq!(required_java_for_minecraft_version("1.20.4"), 17);
        assert_eq!(required_java_for_minecraft_version("1.20.5"), 21);
        assert_eq!(required_java_for_minecraft_version("1.20.6"), 21);
        assert_eq!(required_java_for_minecraft_version("1.21.1"), 21);
        assert_eq!(required_java_for_minecraft_version("25w03a"), 17);
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
        assert_eq!(
            compare_java_versions("21.0.10+7-LTS", "21.0.3+11"),
            Some(Ordering::Greater)
        );
    }
}
