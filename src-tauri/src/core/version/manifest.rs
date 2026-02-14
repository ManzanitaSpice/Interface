// ─── Version Manifest ───
// Handles fetching and parsing the Mojang version manifest v2.

use serde::Deserialize;
use tracing::info;

use crate::core::error::LauncherResult;

const VERSION_MANIFEST_URL: &str =
    "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";

/// Top-level Mojang version manifest.
#[derive(Debug, Deserialize)]
pub struct VersionManifest {
    pub versions: Vec<VersionEntry>,
}

/// A single entry in the manifest.
#[derive(Debug, Clone, Deserialize)]
pub struct VersionEntry {
    pub id: String,
    #[serde(rename = "type")]
    pub version_type: String,
    #[serde(rename = "releaseTime")]
    pub release_time: String,
    pub url: String,
    #[serde(default)]
    pub sha1: Option<String>,
}

impl VersionManifest {
    /// Fetch the version manifest from Mojang using a shared HTTP client.
    pub async fn fetch(client: &reqwest::Client) -> LauncherResult<Self> {
        info!("Fetching Minecraft version manifest...");

        let manifest: VersionManifest = client
            .get(VERSION_MANIFEST_URL)
            .send()
            .await?
            .json()
            .await?;

        info!("Loaded {} versions from manifest", manifest.versions.len());
        Ok(manifest)
    }

    /// Find a specific version entry by ID (e.g. "1.20.4").
    pub fn find_version(&self, id: &str) -> Option<&VersionEntry> {
        self.versions.iter().find(|v| v.id == id)
    }

    /// List all official stable versions (release only).
    pub fn releases(&self) -> Vec<&VersionEntry> {
        self.versions
            .iter()
            .filter(|v| v.version_type == "release")
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_manifest_entry() {
        let json = r#"{
            "id": "1.20.4",
            "type": "release",
            "release_time": "2023-12-07T08:00:00+00:00",
            "url": "https://example.com/1.20.4.json",
            "sha1": "abc123"
        }"#;
        let entry: VersionEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.id, "1.20.4");
        assert_eq!(entry.version_type, "release");
        assert_eq!(entry.release_time, "2023-12-07T08:00:00+00:00");
    }
}
