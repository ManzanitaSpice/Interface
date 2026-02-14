use std::collections::BTreeSet;

use serde::Deserialize;
use tracing::info;

use super::context::InstallContext;
use super::installer::{LoaderInstallResult, LoaderInstaller};
use crate::core::error::{LauncherError, LauncherResult};
use crate::core::maven::MavenArtifact;

/// Installs Forge by downloading and executing the official installer JAR.
pub struct ForgeInstaller;

impl ForgeInstaller {
    pub fn new(_client: reqwest::Client) -> Self {
        Self
    }
}

const FORGE_MAVEN: &str = "https://maven.minecraftforge.net";

/// Subset of Forge's `install_profile.json`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForgeInstallProfile {
    #[serde(default)]
    pub libraries: Vec<ForgeLibrary>,
}

#[derive(Debug, Deserialize)]
pub struct ForgeLibrary {
    pub name: String,
}

/// Subset of the Forge version JSON (inside the installer as `version.json`).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForgeVersionJson {
    pub main_class: String,
    #[serde(default)]
    pub libraries: Vec<ForgeLibrary>,
}

#[async_trait::async_trait]
impl LoaderInstaller for ForgeInstaller {
    async fn install(&self, ctx: InstallContext<'_>) -> LauncherResult<LoaderInstallResult> {
        info!(
            "Installing Forge {} for MC {}",
            ctx.loader_version, ctx.minecraft_version
        );

        let forge_id = format!("{}-{}", ctx.minecraft_version, ctx.loader_version);
        let installer_name = format!("forge-{}-installer.jar", forge_id);

        let installer_url = format!(
            "{}/net/minecraftforge/forge/{}/{}",
            FORGE_MAVEN, forge_id, installer_name
        );
        let installer_path = ctx.instance_dir.join(&installer_name);
        ctx.downloader
            .download_file(&installer_url, &installer_path, None)
            .await?;

        let installer_bytes =
            tokio::fs::read(&installer_path)
                .await
                .map_err(|e| LauncherError::Io {
                    path: installer_path.clone(),
                    source: e,
                })?;

        let cursor = std::io::Cursor::new(&installer_bytes);
        let mut archive = zip::ZipArchive::new(cursor)?;

        let install_profile: ForgeInstallProfile = {
            let file = archive.by_name("install_profile.json").map_err(|e| {
                LauncherError::Loader(format!("Missing install_profile.json: {}", e))
            })?;
            serde_json::from_reader(file)?
        };

        let version_json: ForgeVersionJson = {
            let file = archive
                .by_name("version.json")
                .map_err(|e| LauncherError::Loader(format!("Missing version.json: {}", e)))?;
            serde_json::from_reader(file)?
        };

        let required_java = required_java_for_minecraft(ctx.minecraft_version);
        let java_bin = crate::core::java::resolve_java_binary(required_java).await?;

        let output = std::process::Command::new(&java_bin)
            .arg("-jar")
            .arg(&installer_path)
            .arg("--installClient")
            .arg(ctx.instance_dir)
            .current_dir(ctx.instance_dir)
            .output()
            .map_err(|e| LauncherError::JavaExecution(e.to_string()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            return Err(LauncherError::Loader(format!(
                "Forge installer failed (code {:?})\nSTDOUT:\n{}\nSTDERR:\n{}",
                output.status.code(),
                stdout,
                stderr
            )));
        }

        let mut libraries = BTreeSet::new();
        for lib in &install_profile.libraries {
            libraries.insert(lib.name.clone());
        }
        for lib in &version_json.libraries {
            libraries.insert(lib.name.clone());
        }

        for lib_name in &libraries {
            let artifact = MavenArtifact::parse(lib_name)?;
            let dest = ctx.libs_dir.join(artifact.local_path());
            if !dest.exists() {
                let primary = artifact.url(FORGE_MAVEN);
                if ctx
                    .downloader
                    .download_file(&primary, &dest, None)
                    .await
                    .is_err()
                {
                    let fallback = artifact.url(crate::core::maven::MOJANG_LIBRARIES);
                    let _ = ctx.downloader.download_file(&fallback, &dest, None).await;
                }
            }
        }

        let _ = tokio::fs::remove_file(&installer_path).await;

        info!("Forge {} installed successfully", forge_id);

        Ok(LoaderInstallResult {
            main_class: version_json.main_class,
            extra_jvm_args: vec![],
            extra_game_args: vec![],
            libraries: libraries.into_iter().collect(),
            asset_index_id: None,
            asset_index_url: None,
            java_major: None,
        })
    }
}

fn required_java_for_minecraft(version: &str) -> u32 {
    let mut parts = version.split('.');
    let major = parts
        .next()
        .and_then(|p| p.parse::<u32>().ok())
        .unwrap_or(1);
    let minor = parts
        .next()
        .and_then(|p| p.parse::<u32>().ok())
        .unwrap_or(20);

    if major > 1 || minor >= 21 {
        21
    } else if minor >= 17 {
        17
    } else {
        8
    }
}
