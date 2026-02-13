use std::path::PathBuf;
use std::process::Command;

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::core::error::{LauncherError, LauncherResult};

/// A detected Java installation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JavaInstallation {
    pub path: PathBuf,
    pub version: String,
    pub major: u32,
    pub is_64bit: bool,
}

/// Blocking implementation used by async wrappers.
pub fn detect_java_installations_sync() -> Vec<JavaInstallation> {
    let mut installations = Vec::new();

    if let Ok(java_home) = std::env::var("JAVA_HOME") {
        let bin = PathBuf::from(&java_home).join("bin").join(java_exe());
        if bin.exists() {
            if let Some(info) = probe_java(&bin) {
                installations.push(info);
            }
        }
    }

    if let Ok(path_var) = std::env::var("PATH") {
        let separator = if cfg!(windows) { ';' } else { ':' };
        for dir in path_var.split(separator) {
            let bin = PathBuf::from(dir).join(java_exe());
            if bin.exists() {
                if let Some(info) = probe_java(&bin) {
                    if !installations.iter().any(|i| i.path == info.path) {
                        installations.push(info);
                    }
                }
            }
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
                        let bin = entry.path().join("bin").join("java.exe");
                        if bin.exists() {
                            if let Some(info) = probe_java(&bin) {
                                if !installations.iter().any(|i| i.path == info.path) {
                                    installations.push(info);
                                }
                            }
                        }
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
                        let bin = entry.path().join("bin").join("java");
                        let bin_alt = entry.path().join("Contents/Home/bin/java");
                        for candidate in [&bin, &bin_alt] {
                            if candidate.exists() {
                                if let Some(info) = probe_java(candidate) {
                                    if !installations.iter().any(|i| i.path == info.path) {
                                        installations.push(info);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    if installations.is_empty() {
        warn!("No Java installations detected");
    } else {
        info!("Detected {} Java installations", installations.len());
    }

    installations
}

/// Detect all Java installations available on the system.
pub async fn detect_java_installations() -> Vec<JavaInstallation> {
    tauri::async_runtime::spawn_blocking(detect_java_installations_sync)
        .await
        .unwrap_or_else(|e| {
            warn!("Java detection task failed: {}", e);
            Vec::new()
        })
}

/// Find a suitable Java binary for a given major version (e.g. 17, 21).
pub async fn find_java_binary(major: u32) -> LauncherResult<PathBuf> {
    let installations = detect_java_installations().await;

    if let Some(exact) = installations
        .iter()
        .find(|i| i.major == major && i.is_64bit)
    {
        info!("Using Java {} at {:?}", exact.major, exact.path);
        return Ok(exact.path.clone());
    }

    if let Some(compat) = installations
        .iter()
        .find(|i| i.major >= major && i.is_64bit)
    {
        warn!(
            "Exact Java {} not found, using Java {} at {:?}",
            major, compat.major, compat.path
        );
        return Ok(compat.path.clone());
    }

    Err(LauncherError::JavaNotFound(major))
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
    if first_part == "1" {
        version
            .split('.')
            .nth(1)
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(8)
    } else {
        first_part.parse::<u32>().unwrap_or(17)
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
    fn parses_major_versions() {
        assert_eq!(parse_major_version("1.8.0_392"), 8);
        assert_eq!(parse_major_version("17.0.8"), 17);
        assert_eq!(parse_major_version("21.0.1"), 21);
    }
}
