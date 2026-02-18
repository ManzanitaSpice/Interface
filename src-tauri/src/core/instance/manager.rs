use std::path::{Path, PathBuf};

use tracing::info;

use super::model::{Instance, InstanceState};
use crate::core::error::{LauncherError, LauncherResult};

/// Manages the lifecycle of instances on disk.
pub struct InstanceManager {
    /// Root directory where all instances live.
    instances_dir: PathBuf,
}

impl InstanceManager {
    pub fn new(instances_dir: PathBuf) -> Self {
        Self { instances_dir }
    }

    /// Create a new instance on disk with proper subdirectory structure.
    ///
    /// Creates:
    /// - `<instance>/minecraft/`
    /// - `<instance>/minecraft/assets/`
    /// - `<instance>/mods/`
    /// - `<instance>/config/`
    /// - `<instance>/instance.json`
    pub async fn create(&self, mut instance: Instance) -> LauncherResult<Instance> {
        // Set the path based on our instances directory
        instance.path = self.instances_dir.join(&instance.id);

        // Check for collision (extremely unlikely with UUID, but defensive)
        if instance.path.exists() {
            return Err(LauncherError::InstanceAlreadyExists(instance.id.clone()));
        }

        // Create directory structure eagerly to reduce first-launch failures.
        let minecraft_dir = instance.game_dir();
        let assets_dir = minecraft_dir.join("assets");
        let mods_dir = instance.mods_dir();
        let config_dir = instance.config_dir();
        let logs_dir = instance.logs_dir();

        tokio::try_join!(
            create_dir_safe(&minecraft_dir),
            create_dir_safe(&assets_dir),
            create_dir_safe(&mods_dir),
            create_dir_safe(&config_dir),
            create_dir_safe(&logs_dir),
        )?;

        self.verify_structure(&instance).await?;

        // Persist instance.json
        self.save(&instance).await?;

        info!("Created instance '{}' ({})", instance.name, instance.id);
        Ok(instance)
    }

    pub async fn verify_structure(&self, instance: &Instance) -> LauncherResult<()> {
        let runtime_root = instance.runtime_root_dir();
        for subdir in ["minecraft", "minecraft/assets", "mods", "config", "logs"] {
            let path = runtime_root.join(subdir);
            let metadata =
                tokio::fs::metadata(&path)
                    .await
                    .map_err(|source| LauncherError::Io {
                        path: path.clone(),
                        source,
                    })?;
            if !metadata.is_dir() {
                return Err(LauncherError::Other(format!(
                    "Estructura inválida: {:?} no es un directorio",
                    path
                )));
            }
        }

        Ok(())
    }

    /// Save instance metadata to disk.
    pub async fn save(&self, instance: &Instance) -> LauncherResult<()> {
        let json = serde_json::to_string_pretty(instance)?;
        let config_path = instance.config_path();

        if let Some(parent) = config_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| LauncherError::Io {
                    path: parent.to_path_buf(),
                    source: e,
                })?;
        }

        tokio::fs::write(&config_path, json)
            .await
            .map_err(|e| LauncherError::Io {
                path: config_path,
                source: e,
            })?;

        Ok(())
    }

    /// Load a single instance by ID.
    pub async fn load(&self, id: &str) -> LauncherResult<Instance> {
        let config_path = self.instances_dir.join(id).join("instance.json");
        if !config_path.exists() {
            return Err(LauncherError::InstanceNotFound(id.to_string()));
        }

        let json =
            tokio::fs::read_to_string(&config_path)
                .await
                .map_err(|e| LauncherError::Io {
                    path: config_path.clone(),
                    source: e,
                })?;

        let instance: Instance = serde_json::from_str(&json)?;
        Ok(instance)
    }

    /// List all instances.
    pub async fn list(&self) -> LauncherResult<Vec<Instance>> {
        let mut instances = Vec::new();

        if !self.instances_dir.exists() {
            return Ok(instances);
        }

        let mut entries = tokio::fs::read_dir(&self.instances_dir)
            .await
            .map_err(|e| LauncherError::Io {
                path: self.instances_dir.clone(),
                source: e,
            })?;

        while let Some(entry) = entries.next_entry().await.map_err(|e| LauncherError::Io {
            path: self.instances_dir.clone(),
            source: e,
        })? {
            let path = entry.path();
            if path.is_dir() {
                let config_path = path.join("instance.json");
                if config_path.exists() {
                    match tokio::fs::read_to_string(&config_path).await {
                        Ok(json) => match serde_json::from_str::<Instance>(&json) {
                            Ok(inst) => instances.push(inst),
                            Err(e) => {
                                tracing::warn!("Corrupt instance.json at {:?}: {}", config_path, e);
                            }
                        },
                        Err(e) => {
                            tracing::warn!("Cannot read {:?}: {}", config_path, e);
                        }
                    }
                }
            }
        }

        Ok(instances)
    }

    /// Delete an instance from disk.
    pub async fn delete(&self, id: &str) -> LauncherResult<()> {
        let instance_dir = self.instances_dir.join(id);
        if !instance_dir.exists() {
            return Err(LauncherError::InstanceNotFound(id.to_string()));
        }

        tokio::fs::remove_dir_all(&instance_dir)
            .await
            .map_err(|e| LauncherError::Io {
                path: instance_dir,
                source: e,
            })?;

        info!("Deleted instance {}", id);
        Ok(())
    }

    /// Update instance state and persist.
    pub async fn set_state(
        &self,
        instance: &mut Instance,
        state: InstanceState,
    ) -> LauncherResult<()> {
        instance.state = state;
        self.save(instance).await
    }

    /// Helper: canonicalize a path, adding `\\?\` prefix on Windows.
    pub fn safe_path(path: &Path) -> PathBuf {
        match std::fs::canonicalize(path) {
            Ok(p) => p,
            Err(_) => path.to_path_buf(),
        }
    }
}

async fn create_dir_safe(path: &Path) -> LauncherResult<()> {
    tokio::fs::create_dir_all(path)
        .await
        .map_err(|source| LauncherError::Io {
            path: path.to_path_buf(),
            source,
        })
}
