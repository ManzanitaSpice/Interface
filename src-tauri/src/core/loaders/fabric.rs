use std::path::Path;

use async_trait::async_trait;
use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::fs;
use tracing::info;

use super::context::InstallContext;
use super::installer::{LoaderInstallResult, LoaderInstaller};
use crate::core::downloader::Downloader;
use crate::core::error::{LauncherError, LauncherResult};
use crate::core::maven::{MavenArtifact, FABRIC_MAVEN};

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FabricProfile {
    pub id: Option<String>,
    pub main_class: String,
    #[serde(default)]
    pub libraries: Vec<FabricLibrary>,
    pub arguments: Option<FabricArguments>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FabricLibrary {
    pub name: String,
    pub url: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct FabricArguments {
    #[serde(default)]
    pub jvm: Vec<String>,
    #[serde(default)]
    pub game: Vec<String>,
}

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

    fn ensure_loader_artifact(libraries: &mut Vec<String>, loader_version: &str) {
        let loader_coord = format!("net.fabricmc:fabric-loader:{}", loader_version);
        if libraries.iter().any(|lib| lib == &loader_coord) {
            return;
        }
        libraries.push(loader_coord);
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
                let downloader = downloader;

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
    async fn install(&self, ctx: InstallContext<'_>) -> LauncherResult<LoaderInstallResult> {
        info!(
            "Installing Fabric {} for Minecraft {}",
            ctx.loader_version, ctx.minecraft_version
        );

        // 1️⃣ Fetch profile
        let profile = self
            .fetch_profile(ctx.minecraft_version, ctx.loader_version)
            .await?;

        // 2️⃣ Guardar profile local
        fs::create_dir_all(ctx.instance_dir).await?;

        let profile_path = ctx.instance_dir.join(format!(
            "fabric-{}-{}.json",
            ctx.minecraft_version, ctx.loader_version
        ));

        let profile_json = serde_json::to_string_pretty(&profile)?;
        fs::write(&profile_path, profile_json).await?;

        // 3️⃣ Instalar librerías en paralelo
        let mut libraries = self
            .install_libraries(&profile, ctx.libs_dir, ctx.downloader)
            .await?;
        Self::ensure_loader_artifact(&mut libraries, ctx.loader_version);

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

#[cfg(test)]
mod tests {
    use super::FabricInstaller;

    #[test]
    fn ensure_loader_artifact_adds_fabric_loader_coordinate() {
        let mut libs = vec!["net.fabricmc:intermediary:1.21.1".to_string()];

        FabricInstaller::ensure_loader_artifact(&mut libs, "0.16.10");

        assert!(libs
            .iter()
            .any(|lib| lib == "net.fabricmc:fabric-loader:0.16.10"));
    }

    #[test]
    fn ensure_loader_artifact_keeps_existing_coordinate_unique() {
        let mut libs = vec!["net.fabricmc:fabric-loader:0.16.10".to_string()];

        FabricInstaller::ensure_loader_artifact(&mut libs, "0.16.10");

        assert_eq!(
            libs.iter()
                .filter(|lib| lib.as_str() == "net.fabricmc:fabric-loader:0.16.10")
                .count(),
            1
        );
    }
}
