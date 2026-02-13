use async_trait::async_trait;
use tracing::info;

use crate::core::error::{LauncherError, LauncherResult};
use crate::core::version::{VersionJson, VersionManifest};

use super::context::InstallContext;
use super::installer::{LoaderInstallResult, LoaderInstaller};

/// Vanilla "installer" — resolves the official Mojang version JSON,
/// downloads client.jar, libraries (with OS rules evaluation), and assets.
pub struct VanillaInstaller {
    client: reqwest::Client,
}

impl VanillaInstaller {
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }
}

#[async_trait]
impl LoaderInstaller for VanillaInstaller {
    async fn install(&self, ctx: InstallContext<'_>) -> LauncherResult<LoaderInstallResult> {
        info!("Installing Vanilla {}", ctx.minecraft_version);

        // 1. Fetch version manifest
        let manifest = VersionManifest::fetch(&self.client).await?;

        // 2. Find matching version entry
        let entry = manifest
            .find_version(ctx.minecraft_version)
            .ok_or_else(|| {
                LauncherError::Other(format!(
                    "Minecraft version {} not found in manifest",
                    ctx.minecraft_version
                ))
            })?;

        // 3. Fetch and save version JSON
        let (version_json, raw_json) = VersionJson::fetch(&self.client, &entry.url).await?;
        VersionJson::save_to(&raw_json, ctx.instance_dir, ctx.minecraft_version).await?;

        // 4. Download client.jar
        version_json
            .download_client(ctx.instance_dir, ctx.downloader)
            .await?;

        // 5. Download libraries (with OS rules evaluation)
        let lib_coords = version_json
            .download_libraries(ctx.libs_dir, ctx.downloader)
            .await?;

        // 6. Collect asset index info
        let asset_index_id = version_json.asset_index.as_ref().map(|ai| ai.id.clone());
        let asset_index_url = version_json.asset_index.as_ref().map(|ai| ai.url.clone());
        let extra_jvm_args = version_json.simple_jvm_args();
        let extra_game_args = version_json.simple_game_args();
        let java_major = Some(version_json.required_java_major());

        info!("Vanilla {} installed successfully", ctx.minecraft_version);

        Ok(LoaderInstallResult {
            main_class: version_json.main_class,
            extra_jvm_args,
            extra_game_args,
            libraries: lib_coords,
            asset_index_id,
            asset_index_url,
            java_major,
        })
    }
}
