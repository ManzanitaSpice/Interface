use std::fs::File;
use std::io::Cursor;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};
use zip::ZipArchive;

use crate::core::error::{LauncherError, LauncherResult};

const APP_DIR_NAME: &str = "InterfaceOficial";
const ADOPTIUM_JRE8_X64_URL: &str =
    "https://github.com/adoptium/temurin8-binaries/releases/latest/download/OpenJDK8U-jre_x64_windows_hotspot.zip";
const ADOPTIUM_JRE17_X64_URL: &str =
    "https://github.com/adoptium/temurin17-binaries/releases/latest/download/OpenJDK17U-jre_x64_windows_hotspot.zip";
const ADOPTIUM_JRE21_X64_URL: &str =
    "https://api.adoptium.net/v3/binary/latest/21/ga/windows/x64/jdk/hotspot/normal/eclipse";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JavaInstallation {
    pub path: PathBuf,
    pub version: String,
    pub major: u32,
    pub is_64bit: bool,
    pub vendor: String,
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

pub async fn resolve_java_binary_in_dir(
    data_dir: &Path,
    required_major: u32,
) -> LauncherResult<PathBuf> {
    let runtime_major = runtime_track(required_major);
    let runtime_root = managed_runtime_dir(data_dir, runtime_major);
    ensure_embedded_runtime(&runtime_root, runtime_major).await
}

pub async fn detect_java_installations() -> Vec<JavaInstallation> {
    detect_java_installations_sync()
}

pub fn detect_java_installations_sync() -> Vec<JavaInstallation> {
    let data_dir = launcher_base_dir();

    [8_u32, 17_u32, 21_u32]
        .into_iter()
        .filter_map(|major| {
            let candidate = managed_runtime_dir(&data_dir, major)
                .join("bin")
                .join(java_exe());
            probe_java(&candidate).filter(|info| info.major == major && info.is_64bit)
        })
        .collect()
}

async fn ensure_embedded_runtime(
    runtime_root: &Path,
    required_major: u32,
) -> LauncherResult<PathBuf> {
    let java_bin = runtime_root.join("bin").join(java_exe());
    if runtime_is_valid(&java_bin, required_major) {
        info!(
            "Managed runtime java{} already available at {:?}",
            required_major, java_bin
        );
        return Ok(java_bin);
    }

    if runtime_root.exists() {
        warn!(
            "Runtime {:?} is invalid, removing it before re-download",
            runtime_root
        );
        tokio::fs::remove_dir_all(runtime_root)
            .await
            .map_err(|source| LauncherError::Io {
                path: runtime_root.to_path_buf(),
                source,
            })?;
    }

    tokio::fs::create_dir_all(runtime_root)
        .await
        .map_err(|source| LauncherError::Io {
            path: runtime_root.to_path_buf(),
            source,
        })?;

    let runtime_url = runtime_url(required_major);
    info!(
        "Downloading managed runtime java{} from {}",
        required_major, runtime_url
    );

    let response = reqwest::get(runtime_url).await?;
    let status = response.status();
    if !status.is_success() {
        return Err(LauncherError::DownloadFailed {
            url: runtime_url.to_string(),
            status: status.as_u16(),
        });
    }

    let bytes = response.bytes().await?;
    let runtime_root = runtime_root.to_path_buf();
    let zip_path = runtime_zip_path(&runtime_root, required_major);

    if let Some(parent) = zip_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|source| LauncherError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
    }

    tokio::fs::write(&zip_path, &bytes)
        .await
        .map_err(|source| LauncherError::Io {
            path: zip_path.clone(),
            source,
        })?;

    let runtime_root_for_extract = runtime_root.clone();
    let zip_path_for_extract = zip_path.clone();
    tauri::async_runtime::spawn_blocking(move || {
        extract_jre_zip_from_file(&zip_path_for_extract, &runtime_root_for_extract)
    })
    .await
    .map_err(|e| LauncherError::Other(format!("Join error extracting JRE: {e}")))??;

    let _ = tokio::fs::remove_file(&zip_path).await;

    if !runtime_is_valid(&java_bin, required_major) {
        let _ = tokio::fs::remove_dir_all(runtime_root).await;
        return Err(LauncherError::Other(format!(
            "Downloaded runtime {} failed validation",
            java_bin.display()
        )));
    }

    info!(
        "Managed runtime java{} installed successfully at {:?}",
        required_major, java_bin
    );
    Ok(java_bin)
}

fn runtime_is_valid(java_bin: &Path, required_major: u32) -> bool {
    let Some(info) = probe_java(&java_bin.to_path_buf()) else {
        return false;
    };

    info.major == required_major
        && info.is_64bit
        && (info.vendor.contains("Temurin") || info.vendor.contains("Adoptium"))
}

fn runtime_url(required_major: u32) -> &'static str {
    match runtime_track(required_major) {
        8 => ADOPTIUM_JRE8_X64_URL,
        17 => ADOPTIUM_JRE17_X64_URL,
        _ => ADOPTIUM_JRE21_X64_URL,
    }
}

fn runtime_zip_path(runtime_root: &Path, required_major: u32) -> PathBuf {
    let base_dir = runtime_root
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| launcher_base_dir());

    let file_name = if required_major >= 21 {
        "java21.zip"
    } else if required_major >= 17 {
        "java17.zip"
    } else {
        "java8.zip"
    };

    base_dir.join("temp").join(file_name)
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
}
