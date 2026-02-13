use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::core::error::LauncherResult;
use crate::core::instance::LoaderType;

use super::{
    context::InstallContext, fabric::FabricInstaller, forge::ForgeInstaller,
    neoforge::NeoForgeInstaller, quilt::QuiltInstaller, vanilla::VanillaInstaller,
};

/// Resultado unificado de instalación.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoaderInstallResult {
    pub main_class: String,
    pub extra_jvm_args: Vec<String>,
    pub extra_game_args: Vec<String>,
    pub libraries: Vec<String>,
    pub asset_index_id: Option<String>,
    pub asset_index_url: Option<String>,
    pub java_major: Option<u32>,
}

#[async_trait]
pub trait LoaderInstaller: Send + Sync {
    async fn install(&self, ctx: InstallContext<'_>) -> LauncherResult<LoaderInstallResult>;
}

/// Dispatcher sin Box<dyn>
pub enum Installer {
    Vanilla(VanillaInstaller),
    Fabric(FabricInstaller),
    Quilt(QuiltInstaller),
    Forge(ForgeInstaller),
    NeoForge(NeoForgeInstaller),
}

impl Installer {
    pub fn new(loader: &LoaderType, client: reqwest::Client) -> Self {
        match loader {
            LoaderType::Vanilla => Self::Vanilla(VanillaInstaller::new(client)),
            LoaderType::Fabric => Self::Fabric(FabricInstaller::new(client)),
            LoaderType::Quilt => Self::Quilt(QuiltInstaller::new(client)),
            LoaderType::Forge => Self::Forge(ForgeInstaller::new(client)),
            LoaderType::NeoForge => Self::NeoForge(NeoForgeInstaller::new(client)),
        }
    }

    pub async fn install(&self, ctx: InstallContext<'_>) -> LauncherResult<LoaderInstallResult> {
        match self {
            Installer::Vanilla(i) => i.install(ctx).await,
            Installer::Fabric(i) => i.install(ctx).await,
            Installer::Quilt(i) => i.install(ctx).await,
            Installer::Forge(i) => i.install(ctx).await,
            Installer::NeoForge(i) => i.install(ctx).await,
        }
    }
}
