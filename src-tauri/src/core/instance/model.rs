use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

/// Supported mod loaders — strongly typed, no magic strings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LoaderType {
    Vanilla,
    Forge,
    Fabric,
    NeoForge,
    Quilt,
}

impl std::fmt::Display for LoaderType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoaderType::Vanilla => write!(f, "vanilla"),
            LoaderType::Forge => write!(f, "forge"),
            LoaderType::Fabric => write!(f, "fabric"),
            LoaderType::NeoForge => write!(f, "neoforge"),
            LoaderType::Quilt => write!(f, "quilt"),
        }
    }
}

/// Lifecycle state of an instance.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum InstanceState {
    /// Instance metadata exists but files haven't been downloaded.
    Created,
    /// Currently downloading / installing.
    Installing,
    /// Ready to launch.
    Ready,
    /// Game is running.
    Running,
    /// Something went wrong during install.
    Error,
}

/// Full instance representation persisted to disk as `instance.json`.
///
/// Each instance has its own folder under `instances/<uuid>/` with:
/// - `minecraft/`  — game working directory (.minecraft equivalent)
/// - `mods/`       — mod JARs
/// - `config/`     — mod configuration files
/// - `instance.json` — this serialized struct
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Instance {
    pub name: String,
    pub path: PathBuf,
    pub minecraft_version: String,
    pub loader: LoaderType,
    pub loader_version: Option<String>,
    pub java_path: Option<PathBuf>,
    pub max_memory_mb: u32,

    // ── Internal state ──
    pub id: String,
    pub state: InstanceState,
    pub created_at: DateTime<Utc>,
    pub last_played: Option<DateTime<Utc>>,
    /// Main class resolved from version JSON / loader.
    pub main_class: Option<String>,
    /// Asset index ID (e.g. "17" for 1.21.x).
    pub asset_index: Option<String>,
    /// Library coordinates saved during installation.
    pub libraries: Vec<String>,
    /// Extra JVM arguments from config or loader.
    pub jvm_args: Vec<String>,
    /// Extra game arguments from loader.
    pub game_args: Vec<String>,
}

impl Instance {
    /// Create a new instance with initial state.
    pub fn new(
        name: String,
        minecraft_version: String,
        loader: LoaderType,
        loader_version: Option<String>,
        max_memory_mb: u32,
        base_dir: &std::path::Path,
    ) -> Self {
        let id = Uuid::new_v4().to_string();
        let instance_dir = base_dir.join(&id);

        Self {
            name,
            path: instance_dir,
            minecraft_version,
            loader,
            loader_version,
            java_path: None,
            max_memory_mb,
            id,
            state: InstanceState::Created,
            created_at: Utc::now(),
            last_played: None,
            main_class: None,
            asset_index: None,
            libraries: Vec::new(),
            jvm_args: Vec::new(),
            game_args: Vec::new(),
        }
    }

    /// Path to the instance's `minecraft/` game working directory.
    pub fn game_dir(&self) -> PathBuf {
        self.path.join("minecraft")
    }

    /// Path to the `mods/` directory.
    pub fn mods_dir(&self) -> PathBuf {
        self.path.join("mods")
    }

    /// Path to the `config/` directory.
    pub fn config_dir(&self) -> PathBuf {
        self.path.join("config")
    }

    /// Path to the `natives` folder (extracted per launch session).
    pub fn natives_dir(&self) -> PathBuf {
        self.path.join("natives")
    }

    /// Path to this instance's config file.
    pub fn config_path(&self) -> PathBuf {
        self.path.join("instance.json")
    }
}
