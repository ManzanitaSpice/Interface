use std::path::Path;

use serde::Deserialize;
use tracing::{info, warn};

use super::{LoaderInstallResult, LoaderInstaller};
use crate::core::downloader::Downloader;
use crate::core::error::{LauncherError, LauncherResult};
use crate::core::maven::MavenArtifact;

/// Installs Forge by downloading the installer JAR, extracting `install_profile.json`,
/// and running processors via `std::process::Command`.
pub struct ForgeInstaller;

const FORGE_MAVEN: &str = "https://maven.minecraftforge.net";

/// Subset of Forge's `install_profile.json`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForgeInstallProfile {
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub minecraft: Option<String>,
    #[serde(default)]
    pub libraries: Vec<ForgeLibrary>,
    #[serde(default)]
    pub processors: Vec<ForgeProcessor>,
    #[serde(default)]
    pub data: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct ForgeLibrary {
    pub name: String,
    #[serde(default)]
    pub downloads: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct ForgeProcessor {
    #[serde(default)]
    pub sides: Option<Vec<String>>,
    pub jar: String,
    #[serde(default)]
    pub classpath: Vec<String>,
    #[serde(default)]
    pub args: Vec<String>,
}

/// Subset of the Forge version JSON (inside the installer as `version.json`).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForgeVersionJson {
    pub main_class: String,
    #[serde(default)]
    pub libraries: Vec<ForgeLibrary>,
    #[serde(default)]
    pub arguments: Option<ForgeArguments>,
    #[serde(default)]
    pub minecraft_arguments: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ForgeArguments {
    #[serde(default)]
    pub game: Vec<serde_json::Value>,
    #[serde(default)]
    pub jvm: Vec<serde_json::Value>,
}

#[async_trait::async_trait]
impl LoaderInstaller for ForgeInstaller {
    async fn install(
        &self,
        minecraft_version: &str,
        loader_version: &str,
        instance_dir: &Path,
        libs_dir: &Path,
        downloader: &Downloader,
    ) -> LauncherResult<LoaderInstallResult> {
        info!(
            "Installing Forge {} for MC {}",
            loader_version, minecraft_version
        );

        let forge_id = format!("{}-{}", minecraft_version, loader_version);
        let installer_name = format!("forge-{}-installer.jar", forge_id);

        // 1. Download the Forge installer JAR
        let installer_url = format!(
            "{}/net/minecraftforge/forge/{}/{}",
            FORGE_MAVEN, forge_id, installer_name
        );
        let installer_path = instance_dir.join(&installer_name);
        downloader
            .download_file(&installer_url, &installer_path, None)
            .await?;

        // 2. Extract install_profile.json and version.json from the installer JAR
        let installer_bytes =
            tokio::fs::read(&installer_path)
                .await
                .map_err(|e| LauncherError::Io {
                    path: installer_path.clone(),
                    source: e,
                })?;

        let cursor = std::io::Cursor::new(&installer_bytes);
        let mut archive = zip::ZipArchive::new(cursor)?;

        // Extract install_profile.json
        let install_profile: ForgeInstallProfile = {
            let file = archive
                .by_name("install_profile.json")
                .map_err(|e| LauncherError::Loader(format!("Missing install_profile.json: {}", e)))?;
            serde_json::from_reader(file)?
        };

        // Extract version.json
        let version_json: ForgeVersionJson = {
            let file = archive
                .by_name("version.json")
                .map_err(|e| LauncherError::Loader(format!("Missing version.json: {}", e)))?;
            serde_json::from_reader(file)?
        };

        // 3. Download all libraries declared in install_profile
        let mut lib_names = Vec::new();
        for lib in &install_profile.libraries {
            let artifact = MavenArtifact::parse(&lib.name)?;
            let dest = libs_dir.join(artifact.local_path());
            if !dest.exists() {
                let url = artifact.url(FORGE_MAVEN);
                match downloader.download_file(&url, &dest, None).await {
                    Ok(()) => {}
                    Err(e) => {
                        // Try Mojang libraries as fallback
                        let mojang_url =
                            artifact.url(crate::core::maven::MOJANG_LIBRARIES);
                        match downloader
                            .download_file(&mojang_url, &dest, None)
                            .await
                        {
                            Ok(()) => {}
                            Err(_) => {
                                warn!(
                                    "Failed to download Forge lib {}: {}",
                                    lib.name, e
                                );
                            }
                        }
                    }
                }
            }
        }

        // 4. Download libraries from version.json
        for lib in &version_json.libraries {
            let artifact = MavenArtifact::parse(&lib.name)?;
            let dest = libs_dir.join(artifact.local_path());
            if !dest.exists() {
                let url = artifact.url(FORGE_MAVEN);
                let _ = downloader.download_file(&url, &dest, None).await;
            }
            lib_names.push(lib.name.clone());
        }

        // 5. Run processors (client-side only)
        for processor in &install_profile.processors {
            // Skip server-side processors
            if let Some(sides) = &processor.sides {
                if !sides.iter().any(|s| s == "client") {
                    continue;
                }
            }

            let jar_artifact = MavenArtifact::parse(&processor.jar)?;
            let jar_path = libs_dir.join(jar_artifact.local_path());

            // Build classpath for the processor
            let mut cp_entries = vec![jar_path.to_string_lossy().to_string()];
            for cp_coord in &processor.classpath {
                let cp_artifact = MavenArtifact::parse(cp_coord)?;
                let cp_path = libs_dir.join(cp_artifact.local_path());
                cp_entries.push(cp_path.to_string_lossy().to_string());
            }
            let classpath = cp_entries.join(if cfg!(windows) { ";" } else { ":" });

            // Resolve processor args — substitute known variables
            let client_jar = instance_dir.join("client.jar");
            let resolved_args: Vec<String> = processor
                .args
                .iter()
                .map(|arg| {
                    arg.replace("{SIDE}", "client")
                        .replace("{MINECRAFT_JAR}", &client_jar.to_string_lossy())
                        .replace("{ROOT}", &instance_dir.to_string_lossy())
                        .replace("{INSTALLER}", &installer_path.to_string_lossy())
                        .replace("{LIBRARY_DIR}", &libs_dir.to_string_lossy())
                })
                .collect();

            // Execute processor
            info!("Running Forge processor: {}", processor.jar);
            let java_bin = match crate::core::java::find_java_binary(17).await {
                Ok(bin) => bin,
                Err(_) => std::path::PathBuf::from("java"),
            };

            let status = std::process::Command::new(&java_bin)
                .arg("-cp")
                .arg(&classpath)
                .arg("net.minecraftforge.installertools.ConsoleTool") // common entry point
                .args(&resolved_args)
                .current_dir(instance_dir)
                .status()
                .map_err(|e| LauncherError::JavaExecution(e.to_string()))?;

            if !status.success() {
                warn!(
                    "Forge processor {} exited with code {:?}",
                    processor.jar,
                    status.code()
                );
            }
        }

        // Cleanup installer JAR
        let _ = tokio::fs::remove_file(&installer_path).await;

        info!("Forge {} installed successfully", forge_id);

        Ok(LoaderInstallResult {
            main_class: version_json.main_class,
            extra_jvm_args: vec![],
            extra_game_args: vec![],
            libraries: lib_names,
            asset_index_id: None,
            asset_index_url: None,
            java_major: None,
        })
    }
}
