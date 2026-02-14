use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;
use tracing::info;

use crate::core::downloader::{DownloadEntry, Downloader};
use crate::core::error::{LauncherError, LauncherResult};
use crate::core::http::build_http_client;

/// Manages Minecraft asset downloads (sounds, textures referenced by asset index).
pub struct AssetManager;

/// Top-level asset index JSON structure.
#[derive(Debug, Deserialize)]
pub struct AssetIndex {
    pub objects: HashMap<String, AssetObject>,
}

#[derive(Debug, Deserialize)]
pub struct AssetObject {
    pub hash: String,
    pub size: u64,
}

const RESOURCES_URL: &str = "https://resources.download.minecraft.net";

impl AssetManager {
    /// Download the asset index JSON and all referenced assets.
    pub async fn download_assets(
        index_url: &str,
        assets_dir: &Path,
        downloader: &Downloader,
    ) -> LauncherResult<()> {
        // 1. Download asset index JSON
        let client = build_http_client()?;
        let index_resp = client.get(index_url).send().await?;
        if !index_resp.status().is_success() {
            return Err(LauncherError::DownloadFailed {
                url: index_url.to_string(),
                status: index_resp.status().as_u16(),
            });
        }
        let index_text = index_resp.text().await?;
        let index: AssetIndex = serde_json::from_str(&index_text)?;

        // Save index file
        let indexes_dir = assets_dir.join("indexes");
        tokio::fs::create_dir_all(&indexes_dir).await.map_err(|e| {
            crate::core::error::LauncherError::Io {
                path: indexes_dir.clone(),
                source: e,
            }
        })?;

        // Derive the index name from URL (e.g. "17" from ".../17.json")
        let index_name = index_url.rsplit('/').next().unwrap_or("unknown.json");
        let index_path = indexes_dir.join(index_name);
        tokio::fs::write(&index_path, &index_text)
            .await
            .map_err(|e| crate::core::error::LauncherError::Io {
                path: index_path,
                source: e,
            })?;

        // 2. Build download entries for all asset objects
        let objects_dir = assets_dir.join("objects");
        let mut entries = Vec::new();

        for (_name, obj) in &index.objects {
            let hash_prefix = &obj.hash[..2];
            let dest = objects_dir.join(hash_prefix).join(&obj.hash);

            if dest.exists() {
                continue; // Already downloaded
            }

            let url = format!("{}/{}/{}", RESOURCES_URL, hash_prefix, obj.hash);
            entries.push(DownloadEntry {
                url,
                dest,
                sha1: Some(obj.hash.clone()),
                size: Some(obj.size),
            });
        }

        info!(
            "Downloading {} asset objects ({} already cached)",
            entries.len(),
            index.objects.len() - entries.len()
        );

        // 3. Download batch
        let failures = downloader.download_batch(entries).await;
        if !failures.is_empty() {
            tracing::warn!("{} asset downloads failed", failures.len());
        }

        Ok(())
    }
}
