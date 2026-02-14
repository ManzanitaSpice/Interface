use std::fs::File;
use std::io::Cursor;
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};
use zip::ZipArchive;

use crate::core::error::{LauncherError, LauncherResult};

const ADOPTIUM_JRE17_X64_URL: &str =
    "https://github.com/adoptium/temurin17-binaries/releases/latest/download/OpenJDK17U-jre_x64_windows_hotspot.zip";

/// A detected Java installation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JavaInstallation {
    pub path: PathBuf,
    pub version: String,
    pub major: u32,
    pub is_64bit: bool,
}

fn preferred_embedded_runtime_dir() -> Option<PathBuf> {
    let exe_path = std::env::current_exe().ok()?;
    let exe_dir = exe_path.parent()?;
    Some(exe_dir.join("resources").join("runtime"))
}

/// Resolve the embedded Java path from executable location.
///
/// Build (Windows/Tauri): `<exe_dir>/resources/runtime/bin/java.exe`
/// Dev fallback: `<repo>/src-tauri/resources/runtime/bin/java.exe`
pub fn detect_embedded_java_binary() -> Option<PathBuf> {
    let exe_path = std::env::current_exe().ok()?;
    let exe_dir = exe_path.parent()?;

    let mut candidates = vec![
        exe_dir
            .join("resources")
            .join("runtime")
            .join("bin")
            .join(java_exe()),
        exe_dir
            .join("..")
            .join("Resources")
            .join("runtime")
            .join("bin")
            .join(java_exe()),
    ];

    for ancestor in exe_dir.ancestors() {
        candidates.push(
            ancestor
                .join("src-tauri")
                .join("resources")
                .join("runtime")
                .join("bin")
                .join(java_exe()),
        );
    }

    for candidate in candidates {
        if candidate.exists() {
            let canonical = std::fs::canonicalize(&candidate).unwrap_or(candidate);
            info!("Embedded Java detected at {:?}", canonical);
            return Some(canonical);
        }
    }

    warn!("Embedded Java binary not found next to current executable");
    None
}

/// Returns embedded Java if available; otherwise fallback to system Java.
pub async fn resolve_java_binary(required_major: u32) -> LauncherResult<PathBuf> {
    if let Some(embedded) = detect_embedded_java_binary() {
        if let Some(probed) = probe_java(&embedded) {
            if probed.major >= required_major {
                info!("Using embedded Java {} at {:?}", probed.major, probed.path);
                return Ok(probed.path);
            }

            warn!(
                "Embedded Java {} is lower than required {}. Falling back to system Java.",
                probed.major, required_major
            );
        } else {
            warn!(
                "Embedded Java exists but is not usable (failed `java -version`): {:?}",
                embedded
            );
        }
    }

    if let Some(runtime_dir) = preferred_embedded_runtime_dir() {
        if let Ok(downloaded) = ensure_embedded_jre17(&runtime_dir).await {
            if let Some(probed) = probe_java(&downloaded) {
                if probed.major >= required_major {
                    info!(
                        "Using freshly downloaded embedded Java {} at {:?}",
                        probed.major, probed.path
                    );
                    return Ok(probed.path);
                }
            }
        }
    }

    find_java_binary(required_major).await
}

/// Ensure a JRE 17 exists in `runtime_root`, downloading and extracting it when missing.
pub async fn ensure_embedded_jre17(runtime_root: &Path) -> LauncherResult<PathBuf> {
    let java_bin = runtime_root.join("bin").join(java_exe());
    if java_bin.exists() && is_usable_java_binary(&java_bin) {
        info!("Embedded runtime already available at {:?}", java_bin);
        return Ok(java_bin);
    }

    warn!(
        "Embedded runtime missing or invalid at {:?}. Downloading JRE 17...",
        runtime_root
    );

    tokio::fs::create_dir_all(runtime_root)
        .await
        .map_err(|source| LauncherError::Io {
            path: runtime_root.to_path_buf(),
            source,
        })?;

    let response = reqwest::get(ADOPTIUM_JRE17_X64_URL).await?;
    let status = response.status();
    if !status.is_success() {
        return Err(LauncherError::DownloadFailed {
            url: ADOPTIUM_JRE17_X64_URL.to_string(),
            status: status.as_u16(),
        });
    }

    let bytes = response.bytes().await?;
    let runtime_root = runtime_root.to_path_buf();

    tauri::async_runtime::spawn_blocking(move || extract_jre_zip(&bytes, &runtime_root))
        .await
        .map_err(|e| LauncherError::Other(format!("Join error extracting JRE: {e}")))??;

    if !java_bin.exists() {
        return Err(LauncherError::Other(format!(
            "JRE extracted but {} was not found",
            java_bin.display()
        )));
    }

    if !is_usable_java_binary(&java_bin) {
        return Err(LauncherError::Other(format!(
            "Downloaded embedded Java at {} failed validation",
            java_bin.display()
        )));
    }

    info!("Embedded JRE 17 installed successfully at {:?}", java_bin);
    Ok(java_bin)
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

/// Detect all Java installations available on the system.
pub async fn detect_java_installations() -> Vec<JavaInstallation> {
    detect_java_installations_sync()
}

/// Blocking implementation used by async wrappers.
pub fn detect_java_installations_sync() -> Vec<JavaInstallation> {
    let mut installations = Vec::new();
    let mut seen = HashSet::new();

    let mut push_candidate = |candidate: PathBuf| {
        if !candidate.exists() {
            return;
        }
        if let Some(info) = probe_java(&candidate) {
            let key = info.path.to_string_lossy().to_string();
            if seen.insert(key) {
                installations.push(info);
            }
        }
    };

    if let Ok(java_home) = std::env::var("JAVA_HOME") {
        push_candidate(PathBuf::from(&java_home).join("bin").join(java_exe()));
    }

    if let Ok(path_var) = std::env::var("PATH") {
        let separator = if cfg!(windows) { ';' } else { ':' };
        for dir in path_var.split(separator) {
            push_candidate(PathBuf::from(dir).join(java_exe()));
        }
    }

    #[cfg(target_os = "windows")]
    {
        let search_roots = vec![
            r"C:\Program Files\Java",
            r"C:\Program Files\Eclipse Adoptium",
            r"C:\Program Files\Microsoft",
            r"C:\Program Files\Zulu",
            r"C:\Program Files\BellSoft",
        ];

        for root in search_roots {
            let root_path = PathBuf::from(root);
            if root_path.is_dir() {
                if let Ok(entries) = std::fs::read_dir(&root_path) {
                    for entry in entries.flatten() {
                        push_candidate(entry.path().join("bin").join("java.exe"));
                    }
                }
            }
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        let search_roots = vec![
            "/usr/lib/jvm",
            "/usr/local/lib/jvm",
            "/Library/Java/JavaVirtualMachines",
        ];

        for root in search_roots {
            let root_path = PathBuf::from(root);
            if root_path.is_dir() {
                if let Ok(entries) = std::fs::read_dir(&root_path) {
                    for entry in entries.flatten() {
                        push_candidate(entry.path().join("bin").join("java"));
                        push_candidate(entry.path().join("Contents/Home/bin/java"));
                    }
                }
            }
        }
    }

    installations.sort_by(|a, b| {
        b.major
            .cmp(&a.major)
            .then_with(|| b.is_64bit.cmp(&a.is_64bit))
            .then_with(|| b.version.cmp(&a.version))
            .then_with(|| a.path.cmp(&b.path))
    });

    info!("Detected {} Java installations", installations.len());
    installations
}


/// Find a suitable Java binary for a given major version (e.g. 17, 21).
pub async fn find_java_binary(major: u32) -> LauncherResult<PathBuf> {
    let installations = detect_java_installations().await;

    if let Some(exact_64) = installations.iter().find(|i| i.major == major && i.is_64bit) {
        return Ok(exact_64.path.clone());
    }

    if let Some(exact) = installations.iter().find(|i| i.major == major) {
        return Ok(exact.path.clone());
    }

    if let Some(compat_64) = installations.iter().find(|i| i.major > major && i.is_64bit) {
        warn!(
            "Exact Java {} not found, using Java {} at {:?}",
            major, compat_64.major, compat_64.path
        );
        return Ok(compat_64.path.clone());
    }

    if let Some(compat) = installations.iter().find(|i| i.major > major) {
        warn!(
            "Exact Java {} not found, using Java {} at {:?}",
            major, compat.major, compat.path
        );
        return Ok(compat.path.clone());
    }

    Err(LauncherError::JavaNotFound(major))
}

pub fn is_usable_java_binary(path: &Path) -> bool {
    let path_buf = path.to_path_buf();
    probe_java(&path_buf).is_some()
}

fn probe_java(path: &PathBuf) -> Option<JavaInstallation> {
    let output = Command::new(path).arg("-version").output().ok()?;

    let stderr = String::from_utf8_lossy(&output.stderr);
    debug!(
        "Probing {:?}: {}",
        path,
        stderr.lines().next().unwrap_or("")
    );

    let version_str = parse_version_string(&stderr)?;
    let major = parse_major_version(&version_str);
    let is_64bit = stderr.contains("64-Bit");

    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());

    Some(JavaInstallation {
        path: canonical,
        version: version_str,
        major,
        is_64bit,
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
}
