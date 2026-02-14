use serde::Deserialize;
use tracing::{info, warn};

use super::context::InstallContext;
use super::installer::{LoaderInstallResult, LoaderInstaller};
use crate::core::error::{LauncherError, LauncherResult};
use crate::core::maven::MavenArtifact;

/// NeoForge installer — similar to Forge but uses the NeoForge Maven and API.
pub struct NeoForgeInstaller;

impl NeoForgeInstaller {
    pub fn new(_client: reqwest::Client) -> Self {
        Self
    }
}

const NEOFORGE_MAVEN: &str = "https://maven.neoforged.net/releases";

/// Subset of NeoForge's `install_profile.json`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NeoForgeInstallProfile {
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub minecraft: Option<String>,
    #[serde(default)]
    pub libraries: Vec<NeoForgeLibrary>,
    #[serde(default)]
    pub processors: Vec<NeoForgeProcessor>,
    #[serde(default)]
    pub data: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct NeoForgeLibrary {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct NeoForgeProcessor {
    #[serde(default)]
    pub sides: Option<Vec<String>>,
    pub jar: String,
    #[serde(default)]
    pub classpath: Vec<String>,
    #[serde(default)]
    pub args: Vec<String>,
}

/// NeoForge version JSON (inside installer as `version.json`).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NeoForgeVersionJson {
    pub main_class: String,
    #[serde(default)]
    pub libraries: Vec<NeoForgeLibrary>,
    #[serde(default)]
    pub arguments: Option<NeoForgeArguments>,
}

#[derive(Debug, Deserialize)]
pub struct NeoForgeArguments {
    #[serde(default)]
    pub game: Vec<serde_json::Value>,
    #[serde(default)]
    pub jvm: Vec<serde_json::Value>,
}

#[async_trait::async_trait]
impl LoaderInstaller for NeoForgeInstaller {
    async fn install(&self, ctx: InstallContext<'_>) -> LauncherResult<LoaderInstallResult> {
        info!(
            "Installing NeoForge {} for MC {}",
            ctx.loader_version, ctx.minecraft_version
        );

        // NeoForge 1.21+ uses `neoforge` as the artifact name
        // Coordinate: net.neoforged:neoforge:<ctx.loader_version>:installer
        let installer_name = format!("neoforge-{}-installer.jar", ctx.loader_version);
        let installer_path = ctx.instance_dir.join(&installer_name);
        let installer_url = format!(
            "{}/net/neoforged/neoforge/{}/{}",
            NEOFORGE_MAVEN, ctx.loader_version, installer_name
        );

        if let Err(primary_err) = ctx
            .downloader
            .download_file(&installer_url, &installer_path, None)
            .await
        {
            // Legacy NeoForge for MC 1.20.1 was published under net.neoforged:forge
            let legacy_name = format!("forge-{}-installer.jar", ctx.loader_version);
            let legacy_url = format!(
                "{}/net/neoforged/forge/{}/{}",
                NEOFORGE_MAVEN, ctx.loader_version, legacy_name
            );
            info!(
                "Primary NeoForge route failed, trying legacy route: {}",
                legacy_url
            );
            ctx.downloader
                .download_file(&legacy_url, &installer_path, None)
                .await
                .map_err(|_| primary_err)?;
        }

        // Extract install_profile.json and version.json
        let installer_bytes =
            tokio::fs::read(&installer_path)
                .await
                .map_err(|e| LauncherError::Io {
                    path: installer_path.clone(),
                    source: e,
                })?;

        let cursor = std::io::Cursor::new(&installer_bytes);
        let mut archive = zip::ZipArchive::new(cursor)?;

        let install_profile: NeoForgeInstallProfile = {
            let file = archive.by_name("install_profile.json").map_err(|e| {
                LauncherError::Loader(format!("Missing install_profile.json: {}", e))
            })?;
            serde_json::from_reader(file)?
        };

        let version_json: NeoForgeVersionJson = {
            let file = archive
                .by_name("version.json")
                .map_err(|e| LauncherError::Loader(format!("Missing version.json: {}", e)))?;
            serde_json::from_reader(file)?
        };

        // Download libraries from install_profile
        for lib in &install_profile.libraries {
            let artifact = MavenArtifact::parse(&lib.name)?;
            let dest = ctx.libs_dir.join(artifact.local_path());
            if !dest.exists() {
                let url = artifact.url(NEOFORGE_MAVEN);
                if let Err(e) = ctx.downloader.download_file(&url, &dest, None).await {
                    // Fallback to Mojang libs
                    let mojang_url = artifact.url(crate::core::maven::MOJANG_LIBRARIES);
                    if let Err(_) = ctx.downloader.download_file(&mojang_url, &dest, None).await {
                        warn!("Failed to download NeoForge lib {}: {}", lib.name, e);
                    }
                }
            }
        }

        // Download libraries from version.json
        let mut lib_names = Vec::new();
        for lib in &version_json.libraries {
            let artifact = MavenArtifact::parse(&lib.name)?;
            let dest = ctx.libs_dir.join(artifact.local_path());
            if !dest.exists() {
                let url = artifact.url(NEOFORGE_MAVEN);
                let _ = ctx.downloader.download_file(&url, &dest, None).await;
            }
            lib_names.push(lib.name.clone());
        }

        // Run processors (client side)
        for processor in &install_profile.processors {
            if let Some(sides) = &processor.sides {
                if !sides.iter().any(|s| s == "client") {
                    continue;
                }
            }

            let jar_artifact = MavenArtifact::parse(&processor.jar)?;
            let jar_path = ctx.libs_dir.join(jar_artifact.local_path());

            let separator = if cfg!(windows) { ";" } else { ":" };
            let mut cp_entries: Vec<String> = Vec::new();

            if jar_path.exists() {
                cp_entries.push(jar_path.to_string_lossy().to_string());
            }

            for cp_coord in &processor.classpath {
                let cp_artifact = MavenArtifact::parse(cp_coord)?;
                let cp_path = ctx.libs_dir.join(cp_artifact.local_path());
                if cp_path.exists() {
                    cp_entries.push(cp_path.to_string_lossy().to_string());
                }
            }

            if cp_entries.is_empty() {
                return Err(LauncherError::Other(format!(
                    "Classpath vacío para procesador NeoForge {}",
                    processor.jar
                )));
            }

            let classpath = cp_entries.join(separator);
            info!(
                "NeoForge processor classpath len={} value={:?}",
                classpath.len(),
                classpath
            );

            let client_jar = ctx.instance_dir.join("client.jar");
            let resolved_args: Vec<String> = processor
                .args
                .iter()
                .map(|arg| {
                    arg.replace("{SIDE}", "client")
                        .replace("{MINECRAFT_JAR}", &client_jar.to_string_lossy())
                        .replace("{ROOT}", &ctx.instance_dir.to_string_lossy())
                        .replace("{INSTALLER}", &installer_path.to_string_lossy())
                        .replace("{LIBRARY_DIR}", &ctx.libs_dir.to_string_lossy())
                })
                .collect();

            info!("Running NeoForge processor: {}", processor.jar);
            let java_bin = match crate::core::java::find_java_binary(21).await {
                Ok(bin) => bin,
                Err(_) => std::path::PathBuf::from("java"),
            };

            let status = std::process::Command::new(&java_bin)
                .arg("-cp")
                .arg(&classpath)
                .arg("net.minecraftforge.installertools.ConsoleTool")
                .args(&resolved_args)
                .current_dir(ctx.instance_dir)
                .status()
                .map_err(|e| LauncherError::JavaExecution(e.to_string()))?;

            if !status.success() {
                warn!(
                    "NeoForge processor {} exited with {:?}",
                    processor.jar,
                    status.code()
                );
            }
        }

        let _ = tokio::fs::remove_file(&installer_path).await;

        info!("NeoForge {} installed successfully", ctx.loader_version);

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
