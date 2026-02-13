use std::path::PathBuf;
use std::sync::Arc;

use reqwest::Client;

use crate::core::downloader::Downloader;
use crate::core::instance::InstanceManager;

/// Global application state managed by Tauri.
///
/// Wrapped in `Arc<Mutex<...>>` for safe concurrent access from commands.
pub struct AppState {
    /// Root data directory for the launcher
    /// (e.g. `%APPDATA%/InterfaceOficial` on Windows).
    pub data_dir: PathBuf,
    pub instance_manager: InstanceManager,
    pub downloader: Arc<Downloader>,
    /// Shared HTTP client — reuse across all requests to leverage connection pooling.
    pub http_client: Client,
}

impl AppState {
    pub fn new(app_handle: tauri::AppHandle) -> Self {
        let data_dir = default_data_dir();

        let instances_dir = data_dir.join("instances");
        let instance_manager = InstanceManager::new(instances_dir);

        let http_client = Client::builder()
            .user_agent("InterfaceOficial/0.1.0")
            .build()
            .expect("Failed to build HTTP client");

        let downloader = Arc::new(Downloader::new(Some(app_handle)));

        Self {
            data_dir,
            instance_manager,
            downloader,
            http_client,
        }
    }

    /// Path to the shared libraries directory.
    pub fn libraries_dir(&self) -> PathBuf {
        self.data_dir.join("libraries")
    }

    /// Path to the shared assets directory.
    pub fn assets_dir(&self) -> PathBuf {
        self.data_dir.join("assets")
    }

    /// Path to the instances directory.
    pub fn instances_dir(&self) -> PathBuf {
        self.data_dir.join("instances")
    }
}

/// Determine the default data directory per platform.
fn default_data_dir() -> PathBuf {
    let base = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    let dir = base.join("InterfaceOficial");

    // Ensure the directory exists
    if !dir.exists() {
        let _ = std::fs::create_dir_all(&dir);
    }

    dir
}
