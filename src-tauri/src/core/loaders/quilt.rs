use serde::Deserialize;
use tracing::info;

use super::context::InstallContext;
use super::installer::{LoaderInstallResult, LoaderInstaller};
use crate::core::error::{LauncherError, LauncherResult};
use crate::core::http::build_http_client;

/// Installs Quilt loader via the Quilt Meta API (nearly identical to Fabric's API).
pub struct QuiltInstaller {
    client: reqwest::Client,
}

impl QuiltInstaller {
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }
}

const QUILT_META_BASE: &str = "https://meta.quiltmc.org/v3";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuiltProfile {
    pub id: String,
    pub main_class: String,
    #[serde(default)]
    pub libraries: Vec<QuiltLibrary>,
    pub arguments: Option<QuiltArguments>,
}

#[derive(Debug, Deserialize)]
pub struct QuiltLibrary {
    pub name: String,
    pub url: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct QuiltArguments {
    #[serde(default)]
    pub jvm: Vec<String>,
    #[serde(default)]
    pub game: Vec<String>,
}

#[async_trait::async_trait]
impl LoaderInstaller for QuiltInstaller {
    async fn install(&self, ctx: InstallContext<'_>) -> LauncherResult<LoaderInstallResult> {
        info!(
            "Installing Quilt loader {} for MC {}",
            ctx.loader_version, ctx.minecraft_version
        );

        let client = self.client.clone();

        let profile_url = format!(
            "{}/versions/loader/{}/{}/profile/json",
            QUILT_META_BASE, ctx.minecraft_version, ctx.loader_version
        );

        let resp = client.get(&profile_url).send().await?;
        if !resp.status().is_success() {
            return Err(LauncherError::LoaderApi(format!(
                "Quilt Meta returned {} for {}",
                resp.status(),
                profile_url
            )));
        }

        let profile: QuiltProfile = resp.json().await?;

        // Save profile locally
        let profile_path = ctx.instance_dir.join(format!(
            "quilt-{}-{}.json",
            ctx.minecraft_version, ctx.loader_version
        ));
        let profile_json = serde_json::to_string_pretty(&serde_json::json!({
            "id": profile.id,
            "mainClass": profile.main_class,
        }))?;
        tokio::fs::write(&profile_path, &profile_json)
            .await
            .map_err(|e| LauncherError::Io {
                path: profile_path,
                source: e,
            })?;

        // Download libraries
        let mut lib_names = Vec::new();
        for lib in &profile.libraries {
            let repo = lib
                .url
                .as_deref()
                .unwrap_or(crate::core::maven::QUILT_MAVEN);
            let artifact = crate::core::maven::MavenArtifact::parse(&lib.name)?;
            let dest = ctx.libs_dir.join(artifact.local_path());
            if !dest.exists() {
                let url = artifact.url(repo);
                ctx.downloader.download_file(&url, &dest, None).await?;
            }
            lib_names.push(lib.name.clone());
        }

        let (jvm_args, game_args) = match &profile.arguments {
            Some(args) => (args.jvm.clone(), args.game.clone()),
            None => (vec![], vec![]),
        };

        info!("Quilt installed successfully");

        Ok(LoaderInstallResult {
            main_class: profile.main_class,
            extra_jvm_args: jvm_args,
            extra_game_args: game_args,
            libraries: lib_names,
            asset_index_id: None,
            asset_index_url: None,
            java_major: None,
        })
    }
}

/// Fetch available Quilt loader versions for a Minecraft version.
pub async fn list_loader_versions(minecraft_version: &str) -> LauncherResult<Vec<String>> {
    let url = format!("{}/versions/loader/{}", QUILT_META_BASE, minecraft_version);
    let client = build_http_client()?;
    let resp = client.get(&url).send().await?;
    if !resp.status().is_success() {
        return Err(LauncherError::LoaderApi(format!(
            "Quilt Meta returned {}",
            resp.status()
        )));
    }

    let versions: Vec<QuiltLoaderEntry> = resp.json().await?;

    Ok(versions.into_iter().map(|v| v.loader.version).collect())
}

#[derive(Deserialize)]
struct QuiltLoaderEntry {
    loader: QuiltLoaderVersion,
}

#[derive(Deserialize)]
struct QuiltLoaderVersion {
    version: String,
}
