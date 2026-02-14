use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use reqwest::Client;
use serde::{Deserialize, Serialize};
use tauri::Manager;

use crate::core::downloader::Downloader;
use crate::core::http::build_http_client;
use crate::core::instance::InstanceManager;

const APP_DIR_NAME: &str = "InterfaceOficial";
const BOOTSTRAP_FILE: &str = "launcher_bootstrap.json";
const INSTALL_MARKER_FILE: &str = "launcher_installation.json";

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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BootstrapConfig {
    data_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct InstallationMarker {
    initialized: bool,
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
        let embedded_runtime = data_dir.join("runtime");
        if !embedded_runtime.exists() {
            if let Some(resource_dir) = app_handle.path().resource_dir().ok() {
                let bundled_runtime = resource_dir.join("runtime");
                if bundled_runtime.exists() {
                    let _ = std::fs::create_dir_all(&embedded_runtime);
                    let _ = copy_dir_recursive(&bundled_runtime, &embedded_runtime);
                }
            }
        }
        let instances_dir = data_dir.join("instances");
        let instance_manager = InstanceManager::new(instances_dir);

        let http_client = build_http_client().expect("Failed to build HTTP client");

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

    pub fn is_first_launch(&self) -> bool {
        !self.data_dir.join(INSTALL_MARKER_FILE).exists()
    }

    pub fn initialize_launcher_installation(
        &mut self,
        app_handle: &tauri::AppHandle,
        target_dir: PathBuf,
        create_shortcut: bool,
    ) -> std::io::Result<PathBuf> {
        let destination = normalize_install_dir(target_dir);

        std::fs::create_dir_all(&destination)?;
        std::fs::create_dir_all(destination.join("instances"))?;
        std::fs::create_dir_all(destination.join("libraries"))?;
        std::fs::create_dir_all(destination.join("assets"))?;

        self.data_dir = destination.clone();
        self.instance_manager = InstanceManager::new(self.instances_dir());
        self.launcher_settings = load_settings_from_disk(&self.data_dir).unwrap_or_default();

        self.install_embedded_runtime(app_handle)?;
        self.save_settings()?;
        self.save_install_marker()?;
        save_bootstrap_config(&self.data_dir)?;

        if create_shortcut {
            let _ = create_desktop_shortcut(app_handle, &self.data_dir);
        }

        Ok(destination)
    }

    pub fn reinstall_launcher(&mut self, app_handle: &tauri::AppHandle) -> std::io::Result<()> {
        if self.data_dir.exists() {
            std::fs::remove_dir_all(&self.data_dir)?;
        }

        std::fs::create_dir_all(&self.data_dir)?;
        std::fs::create_dir_all(self.instances_dir())?;
        std::fs::create_dir_all(self.libraries_dir())?;
        std::fs::create_dir_all(self.assets_dir())?;

        self.running_instances.clear();
        self.launcher_settings = LauncherSettings::default();
        self.instance_manager = InstanceManager::new(self.instances_dir());

        self.install_embedded_runtime(app_handle)?;
        self.save_settings()?;
        self.save_install_marker()?;
        save_bootstrap_config(&self.data_dir)?;
        Ok(())
    }

    pub fn migrate_data_dir(&mut self, target_dir: PathBuf) -> std::io::Result<PathBuf> {
        let destination = if target_dir
            .file_name()
            .map(|n| n.to_string_lossy() == APP_DIR_NAME)
            .unwrap_or(false)
        {
            target_dir
        } else {
            target_dir.join(APP_DIR_NAME)
        };

        if destination == self.data_dir {
            return Ok(destination);
        }

        std::fs::create_dir_all(&destination)?;
        copy_dir_recursive(&self.data_dir, &destination)?;

        let bootstrap = BootstrapConfig {
            data_dir: destination.clone(),
        };
        let bootstrap_json = serde_json::to_string_pretty(&bootstrap)?;
        std::fs::write(default_base_dir().join(BOOTSTRAP_FILE), bootstrap_json)?;

        self.data_dir = destination.clone();
        self.instance_manager = InstanceManager::new(self.instances_dir());
        self.launcher_settings = load_settings_from_disk(&self.data_dir).unwrap_or_default();
        self.save_settings()?;

        Ok(destination)
    }
}

impl AppState {
    fn install_embedded_runtime(&self, app_handle: &tauri::AppHandle) -> std::io::Result<()> {
        let embedded_runtime = self.data_dir.join("runtime");
        if embedded_runtime.exists() {
            std::fs::remove_dir_all(&embedded_runtime)?;
        }

        if let Some(resource_dir) = app_handle.path().resource_dir().ok() {
            let bundled_runtime = resource_dir.join("runtime");
            if bundled_runtime.exists() {
                std::fs::create_dir_all(&embedded_runtime)?;
                copy_dir_recursive(&bundled_runtime, &embedded_runtime)?;
            }
        }

        Ok(())
    }

    fn save_install_marker(&self) -> std::io::Result<()> {
        let marker_path = self.data_dir.join(INSTALL_MARKER_FILE);
        let marker = InstallationMarker { initialized: true };
        let marker_json = serde_json::to_string_pretty(&marker)?;
        std::fs::write(marker_path, marker_json)
    }
}

fn load_settings_from_disk(data_dir: &PathBuf) -> Option<LauncherSettings> {
    let path = data_dir.join("launcher_settings.json");
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn default_base_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(|| PathBuf::from("."))
}

fn normalize_install_dir(target_dir: PathBuf) -> PathBuf {
    if target_dir
        .file_name()
        .map(|n| n.to_string_lossy() == APP_DIR_NAME)
        .unwrap_or(false)
    {
        target_dir
    } else {
        target_dir.join(APP_DIR_NAME)
    }
}

fn save_bootstrap_config(data_dir: &PathBuf) -> std::io::Result<()> {
    let bootstrap = BootstrapConfig {
        data_dir: data_dir.clone(),
    };
    let bootstrap_json = serde_json::to_string_pretty(&bootstrap)?;
    std::fs::write(default_base_dir().join(BOOTSTRAP_FILE), bootstrap_json)
}

fn create_desktop_shortcut(
    _app_handle: &tauri::AppHandle,
    _data_dir: &PathBuf,
) -> std::io::Result<()> {
    let desktop_dir = dirs::desktop_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."));

    #[cfg(target_os = "windows")]
    {
        let launcher_exe = std::env::current_exe()?;
        let shortcut_path = desktop_dir.join("Interface Launcher.bat");
        let content = format!("@echo off\r\nstart \"\" \"{}\"\r\n", launcher_exe.display());
        std::fs::write(shortcut_path, content)?;
    }

    #[cfg(target_os = "linux")]
    {
        let launcher_exe = std::env::current_exe()?;
        let shortcut_path = desktop_dir.join("interface-launcher.desktop");
        let content = format!(
            "[Desktop Entry]\nType=Application\nName=Interface Launcher\nExec={}\nPath={}\nTerminal=false\n",
            launcher_exe.display(),
            _data_dir.display()
        );
        std::fs::write(&shortcut_path, content)?;
    }

    #[cfg(target_os = "macos")]
    {
        let launcher_exe = std::env::current_exe()?;
        let shortcut_path = desktop_dir.join("Interface Launcher.command");
        let content = format!("#!/bin/bash\n\"{}\"\n", launcher_exe.display());
        std::fs::write(&shortcut_path, content)?;
    }

    Ok(())
}

fn default_data_dir() -> PathBuf {
    let base = default_base_dir();
    let bootstrap_path = base.join(BOOTSTRAP_FILE);

    if let Ok(raw) = std::fs::read_to_string(&bootstrap_path) {
        if let Ok(cfg) = serde_json::from_str::<BootstrapConfig>(&raw) {
            if !cfg.data_dir.exists() {
                let _ = std::fs::create_dir_all(&cfg.data_dir);
            }
            return cfg.data_dir;
        }
    }

    let dir = base.join(APP_DIR_NAME);

    if !dir.exists() {
        let _ = std::fs::create_dir_all(&dir);
    }

    dir
}

fn copy_dir_recursive(
    source: &std::path::Path,
    destination: &std::path::Path,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = destination.join(entry.file_name());
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            std::fs::create_dir_all(&dst_path)?;
            copy_dir_recursive(&src_path, &dst_path)?;
        } else if file_type.is_file() {
            if dst_path.exists() {
                std::fs::remove_file(&dst_path)?;
            }
            std::fs::copy(&src_path, &dst_path)?;
        }
    }

    Ok(())
}
