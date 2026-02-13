use std::path::{Path, PathBuf};

use async_trait::async_trait;
use futures::stream::{self, StreamExt};
use serde::Deserialize;
use tokio::fs;
use tracing::{info, warn};

use super::{LoaderInstallResult, LoaderInstaller};
use crate::core::downloader::Downloader;
use crate::core::error::{LauncherError, LauncherResult};
use crate::core::maven::{MavenArtifact, FABRIC_MAVEN};

pub struct FabricInstaller {
    client: reqwest::Client,
}

impl FabricInstaller {
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }

    async fn fetch_profile(
        &self,
        minecraft_version: &str,
        loader_version: &str,
    ) -> LauncherResult<FabricProfile> {
        let url = format!(
            "{}/versions/loader/{}/{}/profile/json",
            FABRIC_META_BASE, minecraft_version, loader_version
        );

        let resp = self.client.get(&url).send().await?;

        if !resp.status().is_success() {
            return Err(LauncherError::LoaderApi(format!(
                "Fabric Meta returned {} for {}",
                resp.status(),
                url
            )));
        }

        let profile = resp.json::<FabricProfile>().await?;

        if profile.main_class.is_empty() {
            return Err(LauncherError::LoaderApi(
                "Fabric profile missing main_class".into(),
            ));
        }

        Ok(profile)
    }

    async fn install_libraries(
        &self,
        profile: &FabricProfile,
        libs_dir: &Path,
        downloader: &Downloader,
    ) -> LauncherResult<Vec<String>> {
        fs::create_dir_all(libs_dir).await?;

        let tasks = stream::iter(profile.libraries.iter().cloned())
            .map(|lib| {
                let libs_dir = libs_dir.to_path_buf();
                let downloader = downloader.clone();

                async move {
                    let repo = lib.url.as_deref().unwrap_or(FABRIC_MAVEN);

                    let artifact = MavenArtifact::parse(&lib.name)?;
                    let dest = libs_dir.join(artifact.local_path());

                    if !dest.try_exists().unwrap_or(false) {
                        let url = artifact.url(repo);
                        downloader.download_file(&url, &dest, None).await?;
                    }

                    Ok::<_, LauncherError>(lib.name)
                }
            })
            .buffer_unordered(8) // Descarga 8 en paralelo
            .collect::<Vec<_>>()
            .await;

        let mut installed = Vec::new();
        for result in tasks {
            installed.push(result?);
        }

        Ok(installed)
    }
}

const FABRIC_META_BASE: &str = "https://meta.fabricmc.net/v2";

#[async_trait]
impl LoaderInstaller for FabricInstaller {
    async fn install(
        &self,
        minecraft_version: &str,
        loader_version: &str,
        instance_dir: &Path,
        libs_dir: &Path,
        downloader: &Downloader,
    ) -> LauncherResult<LoaderInstallResult> {
        info!(
            "Installing Fabric {} for Minecraft {}",
            loader_version, minecraft_version
        );

        // 1️⃣ Fetch profile
        let profile = self
            .fetch_profile(minecraft_version, loader_version)
            .await?;

        // 2️⃣ Guardar profile local
        fs::create_dir_all(instance_dir).await?;

        let profile_path = instance_dir.join(format!(
            "fabric-{}-{}.json",
            minecraft_version, loader_version
        ));

        let profile_json = serde_json::to_string_pretty(&profile)?;
        fs::write(&profile_path, profile_json).await?;

        // 3️⃣ Instalar librerías en paralelo
        let libraries = self
            .install_libraries(&profile, libs_dir, downloader)
            .await?;

        // 4️⃣ Argumentos
        let (jvm_args, game_args) = match profile.arguments {
            Some(args) => (args.jvm, args.game),
            None => (vec![], vec![]),
        };

        info!("Fabric installed successfully");

        Ok(LoaderInstallResult {
            main_class: profile.main_class,
            extra_jvm_args: jvm_args,
            extra_game_args: game_args,
            libraries,
            asset_index_id: None,
            asset_index_url: None,
            java_major: None,
        })
    }
}
