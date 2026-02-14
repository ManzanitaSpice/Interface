// ─── Version File ───
// Parses a Mojang version JSON and evaluates OS rules for libraries.

use std::path::Path;

use serde::Deserialize;
use tracing::{debug, info};

use crate::core::downloader::Downloader;
use crate::core::error::{LauncherError, LauncherResult};

/// A fully parsed Mojang version JSON.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionJson {
    pub id: Option<String>,
    pub main_class: String,
    #[serde(default)]
    pub inherits_from: Option<String>,
    #[serde(default)]
    pub libraries: Vec<LibraryEntry>,
    pub downloads: Option<VersionDownloads>,
    #[serde(default)]
    pub asset_index: Option<AssetIndexInfo>,
    #[serde(default)]
    pub arguments: Option<Arguments>,
    /// Legacy `minecraftArguments` field (pre-1.13).
    #[serde(default)]
    pub minecraft_arguments: Option<String>,
    #[serde(default)]
    pub java_version: Option<JavaVersionInfo>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaVersionInfo {
    pub major_version: u32,
}

#[derive(Debug, Deserialize)]
pub struct VersionDownloads {
    pub client: Option<DownloadArtifact>,
    pub server: Option<DownloadArtifact>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DownloadArtifact {
    pub sha1: String,
    pub size: u64,
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AssetIndexInfo {
    pub id: String,
    pub url: String,
    #[allow(dead_code)]
    #[serde(default)]
    pub sha1: Option<String>,
    #[allow(dead_code)]
    #[serde(default)]
    pub total_size: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct Arguments {
    #[serde(default)]
    pub game: Vec<serde_json::Value>,
    #[serde(default)]
    pub jvm: Vec<serde_json::Value>,
}

// ─── Library Entry with Rules ───

#[derive(Debug, Deserialize)]
pub struct LibraryEntry {
    pub name: String,
    #[serde(default)]
    pub downloads: Option<LibraryDownloads>,
    #[serde(default)]
    pub rules: Option<Vec<LibraryRule>>,
    #[serde(default)]
    pub natives: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct LibraryDownloads {
    pub artifact: Option<LibDownloadArtifact>,
    #[serde(default)]
    pub classifiers: Option<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Debug, Deserialize)]
pub struct LibDownloadArtifact {
    pub path: String,
    pub sha1: String,
    #[allow(dead_code)]
    pub size: u64,
    pub url: String,
}

// ─── OS Rule Evaluation ───

#[derive(Debug, Deserialize)]
pub struct LibraryRule {
    pub action: RuleAction,
    #[serde(default)]
    pub os: Option<OsRule>,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RuleAction {
    Allow,
    Disallow,
}

#[derive(Debug, Deserialize)]
pub struct OsRule {
    #[serde(default)]
    pub name: Option<String>,
    #[allow(dead_code)]
    #[serde(default)]
    pub arch: Option<String>,
    #[allow(dead_code)]
    #[serde(default)]
    pub version: Option<String>,
}

impl LibraryEntry {
    /// Evaluate whether this library should be included for the current OS.
    ///
    /// Rules logic (Mojang spec):
    /// - If no rules → allowed.
    /// - Process rules top-to-bottom. Start with "disallowed".
    /// - Each rule either sets "allow" or "disallow" if the OS matches (or if no OS is specified).
    /// - Final state determines inclusion.
    pub fn is_allowed_for_current_os(&self) -> bool {
        let rules = match &self.rules {
            Some(r) => r,
            None => return true, // No rules → always allowed
        };

        let current_os = current_os_name();
        let mut allowed = false;

        for rule in rules {
            let os_matches = match &rule.os {
                None => true, // No OS constraint → rule applies universally
                Some(os) => match &os.name {
                    None => true,
                    Some(name) => name == current_os,
                },
            };

            if os_matches {
                allowed = rule.action == RuleAction::Allow;
            }
        }

        allowed
    }

    /// Check if this library has native classifiers for the current OS.
    pub fn native_classifier_for_current_os(&self) -> Option<String> {
        let natives = self.natives.as_ref()?;
        let os = current_os_name();
        natives.as_object()?.get(os)?.as_str().map(|s| {
            // Replace ${arch} with actual architecture
            let arch = if cfg!(target_arch = "x86_64") {
                "64"
            } else {
                "32"
            };
            s.replace("${arch}", arch)
        })
    }
}

/// Get the Mojang OS name for the current platform.
fn current_os_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "osx"
    } else {
        "linux"
    }
}

impl VersionJson {
    /// Fetch and parse a version JSON from the given URL using a shared client.
    pub async fn fetch(client: &reqwest::Client, url: &str) -> LauncherResult<(Self, String)> {
        let raw = client.get(url).send().await?.text().await?;
        let version_json: VersionJson = serde_json::from_str(&raw)?;
        Ok((version_json, raw))
    }

    /// Save the raw version JSON to the instance directory.
    pub async fn save_to(
        raw_json: &str,
        instance_dir: &Path,
        version_id: &str,
    ) -> LauncherResult<()> {
        let path = instance_dir.join(format!("{}.json", version_id));
        tokio::fs::write(&path, raw_json)
            .await
            .map_err(|e| LauncherError::Io { path, source: e })?;
        Ok(())
    }

    /// Download client.jar to the instance directory.
    pub async fn download_client(
        &self,
        instance_dir: &Path,
        downloader: &Downloader,
    ) -> LauncherResult<()> {
        if let Some(ref downloads) = self.downloads {
            if let Some(ref client_dl) = downloads.client {
                let client_jar_path = instance_dir.join("client.jar");
                downloader
                    .download_file(&client_dl.url, &client_jar_path, Some(&client_dl.sha1))
                    .await?;
                info!("Downloaded client.jar");
            }
        }
        Ok(())
    }

    /// Download all allowed libraries (respecting OS rules).
    pub async fn download_libraries(
        &self,
        libs_dir: &Path,
        downloader: &Downloader,
    ) -> LauncherResult<Vec<String>> {
        let mut lib_coords = Vec::new();

        for lib in &self.libraries {
            // ── Evaluate OS rules ──
            if !lib.is_allowed_for_current_os() {
                debug!("Skipping library (OS rule): {}", lib.name);
                continue;
            }

            // ── Download main artifact ──
            let mut classpath_entry = lib.name.clone();

            if let Some(ref downloads) = lib.downloads {
                if let Some(ref artifact) = downloads.artifact {
                    let dest = libs_dir.join(&artifact.path);
                    if !dest.exists() {
                        downloader
                            .download_file(&artifact.url, &dest, Some(&artifact.sha1))
                            .await?;
                    }

                    // Prefer concrete artifact path for classpath resolution.
                    classpath_entry = artifact.path.clone();
                }

                // ── Download native classifiers ──
                if let Some(classifier) = lib.native_classifier_for_current_os() {
                    if let Some(ref classifiers) = downloads.classifiers {
                        if let Some(native_info) = classifiers.get(&classifier) {
                            if let (Some(url), Some(path), Some(sha1)) = (
                                native_info.get("url").and_then(|v| v.as_str()),
                                native_info.get("path").and_then(|v| v.as_str()),
                                native_info.get("sha1").and_then(|v| v.as_str()),
                            ) {
                                let dest = libs_dir.join(path);
                                if !dest.exists() {
                                    downloader.download_file(url, &dest, Some(sha1)).await?;
                                }
                            }
                        }
                    }
                }
            }

            lib_coords.push(classpath_entry);
        }

        info!(
            "Processed {} libraries ({} allowed)",
            self.libraries.len(),
            lib_coords.len()
        );
        Ok(lib_coords)
    }

    /// Get the required Java major version from the version JSON.
    pub fn required_java_major(&self) -> u32 {
        self.java_version
            .as_ref()
            .map(|j| j.major_version)
            .unwrap_or(17)
    }

    /// Extract simple game arguments (string-only, no conditional rules).
    pub fn simple_game_args(&self) -> Vec<String> {
        match &self.arguments {
            Some(args) => args.game.iter().flat_map(extract_argument_values).collect(),
            None => {
                // Legacy minecraftArguments (space-separated)
                match &self.minecraft_arguments {
                    Some(s) => s.split_whitespace().map(|s| s.to_string()).collect(),
                    None => vec![],
                }
            }
        }
    }

    /// Extract simple JVM arguments (string-only, no conditional rules).
    pub fn simple_jvm_args(&self) -> Vec<String> {
        match &self.arguments {
            Some(args) => args.jvm.iter().flat_map(extract_argument_values).collect(),
            None => vec![],
        }
    }

    /// Build a merged version JSON with `parent_json` as base and this version
    /// overriding matching keys.
    pub fn merge_with_parent_json(
        current_json: &serde_json::Value,
        parent_json: &serde_json::Value,
    ) -> serde_json::Value {
        let mut merged = parent_json.clone();

        if let Some(obj) = current_json.as_object() {
            for (k, v) in obj {
                merged[k] = v.clone();
            }
        }

        merged
    }
}

fn extract_argument_values(value: &serde_json::Value) -> Vec<String> {
    if let Some(arg) = value.as_str() {
        return vec![arg.to_string()];
    }

    let Some(obj) = value.as_object() else {
        return vec![];
    };

    if let Some(rules) = obj.get("rules").and_then(|r| r.as_array()) {
        if !rules_allow_current_os(rules) {
            return vec![];
        }
    }

    match obj.get("value") {
        Some(serde_json::Value::String(s)) => vec![s.clone()],
        Some(serde_json::Value::Array(arr)) => arr
            .iter()
            .filter_map(|v| v.as_str().map(ToString::to_string))
            .collect(),
        _ => vec![],
    }
}

fn rules_allow_current_os(rules: &[serde_json::Value]) -> bool {
    let mut allowed = false;
    let current_os = current_os_name();

    for rule in rules {
        let action = rule
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("disallow");

        let os_matches = match rule
            .get("os")
            .and_then(|os| os.get("name"))
            .and_then(|name| name.as_str())
        {
            None => true,
            Some(name) => name == current_os,
        };

        if os_matches {
            allowed = action == "allow";
        }
    }

    allowed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_rules_means_allowed() {
        let lib = LibraryEntry {
            name: "test:lib:1.0".into(),
            downloads: None,
            rules: None,
            natives: None,
        };
        assert!(lib.is_allowed_for_current_os());
    }

    #[test]
    fn allow_only_current_os() {
        let lib = LibraryEntry {
            name: "test:lib:1.0".into(),
            downloads: None,
            rules: Some(vec![LibraryRule {
                action: RuleAction::Allow,
                os: Some(OsRule {
                    name: Some(current_os_name().to_string()),
                    arch: None,
                    version: None,
                }),
            }]),
            natives: None,
        };
        assert!(lib.is_allowed_for_current_os());
    }

    #[test]
    fn disallow_current_os() {
        let lib = LibraryEntry {
            name: "test:lib:1.0".into(),
            downloads: None,
            rules: Some(vec![
                LibraryRule {
                    action: RuleAction::Allow,
                    os: None,
                },
                LibraryRule {
                    action: RuleAction::Disallow,
                    os: Some(OsRule {
                        name: Some(current_os_name().to_string()),
                        arch: None,
                        version: None,
                    }),
                },
            ]),
            natives: None,
        };
        assert!(!lib.is_allowed_for_current_os());
    }

    #[test]
    fn argument_object_rules_apply_to_current_os() {
        let parsed: VersionJson = serde_json::from_value(serde_json::json!({
            "id": "test",
            "mainClass": "net.minecraft.client.main.Main",
            "arguments": {
                "game": [
                    "--username",
                    "Player",
                    {
                        "rules": [{"action": "allow", "os": {"name": "linux"}}],
                        "value": ["--demo"]
                    },
                    {
                        "rules": [{"action": "allow", "os": {"name": "windows"}}],
                        "value": "--should-not-appear"
                    }
                ]
            }
        }))
        .unwrap();

        let game_args = parsed.simple_game_args();
        assert!(game_args.contains(&"--username".to_string()));
        assert!(game_args.contains(&"Player".to_string()));
        if cfg!(target_os = "linux") {
            assert!(game_args.contains(&"--demo".to_string()));
            assert!(!game_args.contains(&"--should-not-appear".to_string()));
        }
    }

    #[test]
    fn merge_with_parent_json_overrides_parent_fields() {
        let parent = serde_json::json!({
            "mainClass": "parent.Main",
            "libraries": [{"name": "a:b:1.0"}],
            "arguments": { "game": ["--parent"] }
        });
        let current = serde_json::json!({
            "inheritsFrom": "1.20.1",
            "mainClass": "child.Main",
            "arguments": { "game": ["--child"] }
        });

        let merged = VersionJson::merge_with_parent_json(&current, &parent);

        assert_eq!(merged["mainClass"], "child.Main");
        assert_eq!(merged["libraries"][0]["name"], "a:b:1.0");
        assert_eq!(merged["arguments"]["game"][0], "--child");
    }
}
