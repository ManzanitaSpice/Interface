use std::path::{Path, PathBuf};

use futures::stream::{self, StreamExt};
use reqwest::Client;
use sha1::{Digest, Sha1};
use tauri::{Emitter, AppHandle};
use tokio::io::AsyncWriteExt;
use tracing::{debug, info};

use crate::core::error::{LauncherError, LauncherResult};

/// Payload emitted to the frontend on download progress.
#[derive(Clone, serde::Serialize)]
pub struct DownloadProgress {
    pub url: String,
    pub bytes_downloaded: u64,
    pub total_bytes: Option<u64>,
    pub file_name: String,
}

/// A single file to download with optional SHA-1 for validation.
#[derive(Debug, Clone)]
pub struct DownloadEntry {
    pub url: String,
    pub dest: PathBuf,
    pub sha1: Option<String>,
    pub size: Option<u64>,
}

/// Concurrent, SHA-1 validated downloader.
pub struct Downloader {
    client: Client,
    /// Maximum number of parallel downloads.
    concurrency: usize,
    /// Optional Tauri app handle for emitting progress events.
    app_handle: Option<AppHandle>,
}

impl Downloader {
    pub fn new(app_handle: Option<AppHandle>) -> Self {
        let client = Client::builder()
            .user_agent("InterfaceOficial/0.1.0")
            .build()
            .expect("Failed to build HTTP client");

        Self {
            client,
            concurrency: 8,
            app_handle,
        }
    }

    pub fn with_concurrency(mut self, n: usize) -> Self {
        self.concurrency = n;
        self
    }

    // ── Single file download ────────────────────────────

    /// Download a single file to `dest`, optionally validating SHA-1.
    ///
    /// Creates parent directories as needed. Drops the file handle
    /// immediately after writing to avoid Windows OS Error 5.
    pub async fn download_file(
        &self,
        url: &str,
        dest: &Path,
        sha1_expected: Option<&str>,
    ) -> LauncherResult<()> {
        // Ensure parent dir exists
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                LauncherError::Io {
                    path: parent.to_path_buf(),
                    source: e,
                }
            })?;
        }

        let response = self.client.get(url).send().await?;

        let status = response.status();
        if !status.is_success() {
            return Err(LauncherError::DownloadFailed {
                url: url.to_string(),
                status: status.as_u16(),
            });
        }

        let total_bytes = response.content_length();
        let bytes = response.bytes().await?;

        // Validate SHA-1 before writing (compute on the in-memory buffer)
        if let Some(expected) = sha1_expected {
            let mut hasher = Sha1::new();
            hasher.update(&bytes);
            let actual = hex::encode(hasher.finalize());
            if actual != expected {
                return Err(LauncherError::Sha1Mismatch {
                    path: dest.to_path_buf(),
                    expected: expected.to_string(),
                    actual,
                });
            }
        }

        // Write to file inside a block to ensure the handle is dropped immediately
        {
            let mut file =
                tokio::fs::File::create(dest).await.map_err(|e| LauncherError::Io {
                    path: dest.to_path_buf(),
                    source: e,
                })?;
            file.write_all(&bytes).await.map_err(|e| LauncherError::Io {
                path: dest.to_path_buf(),
                source: e,
            })?;
            file.flush().await.map_err(|e| LauncherError::Io {
                path: dest.to_path_buf(),
                source: e,
            })?;
            // file is dropped here — critical on Windows
        }

        // Emit progress event if app handle is available
        if let Some(handle) = &self.app_handle {
            let file_name = dest
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let _ = handle.emit(
                "download-progress",
                DownloadProgress {
                    url: url.to_string(),
                    bytes_downloaded: bytes.len() as u64,
                    total_bytes,
                    file_name,
                },
            );
        }

        debug!("Downloaded: {} -> {:?}", url, dest);
        Ok(())
    }

    // ── Batch concurrent downloads ──────────────────────

    /// Download many files concurrently using `buffer_unordered`.
    ///
    /// Returns the list of files that failed (if any).
    pub async fn download_batch(
        &self,
        entries: Vec<DownloadEntry>,
    ) -> Vec<(DownloadEntry, LauncherError)> {
        info!(
            "Starting batch download: {} files, concurrency={}",
            entries.len(),
            self.concurrency
        );

        let results: Vec<_> = stream::iter(entries)
            .map(|entry| {
                let client = &self;
                async move {
                    let result = client
                        .download_file(&entry.url, &entry.dest, entry.sha1.as_deref())
                        .await;
                    (entry, result)
                }
            })
            .buffer_unordered(self.concurrency)
            .collect()
            .await;

        results
            .into_iter()
            .filter_map(|(entry, result)| match result {
                Ok(()) => None,
                Err(e) => Some((entry, e)),
            })
            .collect()
    }

    /// Validate an existing file's SHA-1.
    pub async fn validate_sha1(path: &Path, expected: &str) -> LauncherResult<bool> {
        let bytes = tokio::fs::read(path).await.map_err(|e| LauncherError::Io {
            path: path.to_path_buf(),
            source: e,
        })?;
        let mut hasher = Sha1::new();
        hasher.update(&bytes);
        let actual = hex::encode(hasher.finalize());
        Ok(actual == expected)
    }
}
