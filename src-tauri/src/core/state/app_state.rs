use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::core::downloader::Downloader;
use crate::core::instance::InstanceManager;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JavaRuntimePreference {
    Auto,
    Embedded,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LauncherSettings {
    pub java_runtime: JavaRuntimePreference,
    pub selected_java_path: Option<PathBuf>,
}

impl Default for LauncherSettings {
    fn default() -> Self {
        Self {
            java_runtime: JavaRuntimePreference::Auto,
            selected_java_path: None,
        }
    }
}

pub struct AppState {
    pub data_dir: PathBuf,
    pub instance_manager: InstanceManager,
    pub downloader: Arc<Downloader>,
    pub http_client: Client,
    pub running_instances: HashMap<String, u32>,
    pub launcher_settings: LauncherSettings,
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
        let launcher_settings = load_settings_from_disk(&data_dir).unwrap_or_default();

        Self {
            data_dir,
            instance_manager,
            downloader,
            http_client,
            running_instances: HashMap::new(),
            launcher_settings,
        }
    }

    pub fn libraries_dir(&self) -> PathBuf {
        self.data_dir.join("libraries")
    }

    pub fn assets_dir(&self) -> PathBuf {
        self.data_dir.join("assets")
    }

    pub fn instances_dir(&self) -> PathBuf {
        self.data_dir.join("instances")
    }

    pub fn embedded_java_path(&self) -> PathBuf {
        if cfg!(target_os = "windows") {
            self.data_dir.join("runtime").join("bin").join("java.exe")
        } else {
            self.data_dir.join("runtime").join("bin").join("java")
        }
    }

    pub fn save_settings(&self) -> std::io::Result<()> {
        let settings_path = self.data_dir.join("launcher_settings.json");
        let json = serde_json::to_string_pretty(&self.launcher_settings)?;
        std::fs::write(settings_path, json)
    }
}

fn load_settings_from_disk(data_dir: &PathBuf) -> Option<LauncherSettings> {
    let path = data_dir.join("launcher_settings.json");
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn default_data_dir() -> PathBuf {
    let base = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    let dir = base.join("InterfaceOficial");

    if !dir.exists() {
        let _ = std::fs::create_dir_all(&dir);
    }

    dir
}
