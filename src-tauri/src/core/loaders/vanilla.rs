use std::path::Path;

use tracing::info;

use super::{LoaderInstallResult, LoaderInstaller};
use crate::core::downloader::Downloader;
use crate::core::error::LauncherResult;
use crate::core::version::{VersionJson, VersionManifest};

/// Vanilla "installer" — resolves the official Mojang version JSON,
/// downloads client.jar, libraries (with OS rules evaluation), and assets.
pub struct VanillaInstaller;

#[async_trait::async_trait]
impl LoaderInstaller for VanillaInstaller {
    async fn install(
        &self,
        minecraft_version: &str,
        _loader_version: &str,
        instance_dir: &Path,
        libs_dir: &Path,
        downloader: &Downloader,
    ) -> LauncherResult<LoaderInstallResult> {
        info!("Installing Vanilla {}", minecraft_version);

        // 1. Fetch version manifest
        let manifest = VersionManifest::fetch().await?;

        // 2. Find matching version entry
        let entry = manifest
            .find_version(minecraft_version)
            .ok_or_else(|| {
                crate::core::error::LauncherError::Other(format!(
                    "Minecraft version {} not found in manifest",
                    minecraft_version
                ))
            })?;

        // 3. Fetch and save version JSON
        let (version_json, raw_json) = VersionJson::fetch(&entry.url).await?;
        VersionJson::save_to(&raw_json, instance_dir, minecraft_version).await?;

        // 4. Download client.jar
        version_json.download_client(instance_dir, downloader).await?;

        // 5. Download libraries (with OS rules evaluation)
        let lib_coords = version_json
            .download_libraries(libs_dir, downloader)
            .await?;

        // 6. Collect asset index info
        let asset_index_id = version_json
            .asset_index
            .as_ref()
            .map(|ai| ai.id.clone());

        info!("Vanilla {} installed successfully", minecraft_version);

        Ok(LoaderInstallResult {
            main_class: version_json.main_class,
            extra_jvm_args: version_json.simple_jvm_args(),
            extra_game_args: version_json.simple_game_args(),
            libraries: lib_coords,
            asset_index_id,
            asset_index_url: version_json
                .asset_index
                .as_ref()
                .map(|ai| ai.url.clone()),
            java_major: Some(version_json.required_java_major()),
        })
    }
}
