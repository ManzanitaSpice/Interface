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

/// Detect all Java installations available on the system.
pub async fn detect_java_installations() -> Vec<JavaInstallation> {
    let mut installations = Vec::new();

    // 1. Check JAVA_HOME
    if let Ok(java_home) = std::env::var("JAVA_HOME") {
        let bin = PathBuf::from(&java_home).join("bin").join(java_exe());
        if bin.exists() {
            if let Some(info) = probe_java(&bin) {
                installations.push(info);
            }
        }
    }

    // 2. Check PATH
    if let Ok(path_var) = std::env::var("PATH") {
        let separator = if cfg!(windows) { ';' } else { ':' };
        for dir in path_var.split(separator) {
            let bin = PathBuf::from(dir).join(java_exe());
            if bin.exists() {
                if let Some(info) = probe_java(&bin) {
                    // Avoid duplicates
                    if !installations.iter().any(|i| i.path == info.path) {
                        installations.push(info);
                    }
                }
            }
        }
    }

    // 3. Scan well-known directories (Windows)
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

    // 4. Scan well-known directories (Linux/macOS)
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
                        let bin_alt = entry.path().join("Contents/Home/bin/java"); // macOS
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

    info!("Detected {} Java installations", installations.len());
    installations
}

/// Find a suitable Java binary for a given major version (e.g. 17, 21).
pub async fn find_java_binary(major: u32) -> LauncherResult<PathBuf> {
    let installations = detect_java_installations().await;

    // Prefer exact major match
    if let Some(exact) = installations.iter().find(|i| i.major == major) {
        return Ok(exact.path.clone());
    }

    // Fallback: any version >= requested major
    if let Some(compat) = installations.iter().find(|i| i.major >= major) {
        warn!(
            "Exact Java {} not found, using Java {} at {:?}",
            major, compat.major, compat.path
        );
        return Ok(compat.path.clone());
    }

    Err(LauncherError::JavaNotFound(major))
}

/// Probe a `java` binary to determine its version.
fn probe_java(path: &PathBuf) -> Option<JavaInstallation> {
    let output = Command::new(path)
        .arg("-version")
        .output()
        .ok()?;

    // `java -version` writes to stderr
    let stderr = String::from_utf8_lossy(&output.stderr);
    debug!("Probing {:?}: {}", path, stderr.lines().next().unwrap_or(""));

    // Parse version string: "17.0.8" or "1.8.0_392"
    let version_str = parse_version_string(&stderr)?;
    let major = parse_major_version(&version_str);
    let is_64bit = stderr.contains("64-Bit");

    // Canonicalize path for consistency
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());

    Some(JavaInstallation {
        path: canonical,
        version: version_str,
        major,
        is_64bit,
    })
}

/// Extract the version number from `java -version` output.
fn parse_version_string(output: &str) -> Option<String> {
    // Matches: "17.0.8", "21.0.1", "1.8.0_392"
    for line in output.lines() {
        if let Some(start) = line.find('"') {
            if let Some(end) = line[start + 1..].find('"') {
                return Some(line[start + 1..start + 1 + end].to_string());
            }
        }
    }
    None
}

/// From "17.0.8" → 17, from "1.8.0_392" → 8.
fn parse_major_version(version: &str) -> u32 {
    let first_part = version.split('.').next().unwrap_or("0");
    let major: u32 = first_part.parse().unwrap_or(0);

    // Legacy format: "1.8.x" → major is 8
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
        let output = r#"openjdk version "17.0.8" 2023-07-18"#;
        assert_eq!(parse_version_string(output), Some("17.0.8".to_string()));
    }
}
