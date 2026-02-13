pub mod fabric;
pub mod forge;
pub mod neoforge;
pub mod quilt;
pub mod vanilla;

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::core::downloader::Downloader;
use crate::core::error::LauncherResult;
use crate::core::instance::LoaderType;

/// Result of a loader installation: the resolved main class, libraries, and metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoaderInstallResult {
    /// The main class to launch (e.g. `net.minecraft.client.main.Main`
    /// or `net.fabricmc.loader.impl.launch.knot.KnotClient`).
    pub main_class: String,
    /// Extra JVM arguments injected by the loader.
    pub extra_jvm_args: Vec<String>,
    /// Extra game arguments.
    pub extra_game_args: Vec<String>,
    /// Library coordinates that were added by the loader.
    pub libraries: Vec<String>,
    /// Asset index ID (only set by Vanilla installer).
    pub asset_index_id: Option<String>,
    /// Asset index URL (only set by Vanilla installer).
    pub asset_index_url: Option<String>,
    /// Required Java major version (only set by Vanilla installer).
    pub java_major: Option<u32>,
}

/// Unified trait for all loader installers.
#[async_trait::async_trait]
pub trait LoaderInstaller: Send + Sync {
    /// Install the loader for a given minecraft + loader version into the instance directory.
    async fn install(
        &self,
        minecraft_version: &str,
        loader_version: &str,
        instance_dir: &Path,
        libs_dir: &Path,
        downloader: &Downloader,
    ) -> LauncherResult<LoaderInstallResult>;
}

/// Factory: create the right installer for a given `LoaderType`.
pub fn create_installer(loader: &LoaderType) -> Box<dyn LoaderInstaller> {
    match loader {
        LoaderType::Vanilla => Box::new(vanilla::VanillaInstaller),
        LoaderType::Fabric => Box::new(fabric::FabricInstaller),
        LoaderType::Quilt => Box::new(quilt::QuiltInstaller),
        LoaderType::Forge => Box::new(forge::ForgeInstaller),
        LoaderType::NeoForge => Box::new(neoforge::NeoForgeInstaller),
    }
}
