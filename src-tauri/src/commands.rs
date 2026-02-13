use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::info;

use crate::core::version::VersionManifest;

use crate::core::error::LauncherError;
use crate::core::instance::{Instance, InstanceState, LoaderType};
use crate::core::java::{self, JavaInstallation};
use crate::core::launch;
use crate::core::loaders;
use crate::core::state::AppState;

/// Payload sent from the frontend to create an instance.
#[derive(Debug, Deserialize)]
pub struct CreateInstancePayload {
    pub name: String,
    pub minecraft_version: String,
    pub loader_type: LoaderType,
    pub loader_version: Option<String>,
    pub memory_max_mb: Option<u32>,
}

/// Lightweight instance info returned to the frontend.
#[derive(Debug, Serialize)]
pub struct InstanceInfo {
    pub id: String,
    pub name: String,
    pub minecraft_version: String,
    pub loader_type: LoaderType,
    pub loader_version: Option<String>,
    pub state: InstanceState,
}

impl From<&Instance> for InstanceInfo {
    fn from(inst: &Instance) -> Self {
        Self {
            id: inst.id.clone(),
            name: inst.name.clone(),
            minecraft_version: inst.minecraft_version.clone(),
            loader_type: inst.loader.clone(),
            loader_version: inst.loader_version.clone(),
            state: inst.state.clone(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct MavenMetadata {
    versioning: MavenVersioning,
}

#[derive(Debug, Deserialize)]
struct MavenVersioning {
    versions: MavenVersions,
}

#[derive(Debug, Deserialize)]
struct MavenVersions {
    #[serde(rename = "version", default)]
    version: Vec<String>,
}

#[tauri::command]
pub async fn get_minecraft_versions(
    state: tauri::State<'_, Arc<Mutex<AppState>>>,
) -> Result<Vec<String>, LauncherError> {
    let state = state.lock().await;
    let manifest = VersionManifest::fetch(&state.http_client).await?;

    let versions = manifest
        .versions
        .iter()
        .map(|entry| entry.id.clone())
        .collect();

    Ok(versions)
}

#[tauri::command]
pub async fn get_loader_versions(
    state: tauri::State<'_, Arc<Mutex<AppState>>>,
    loader_type: LoaderType,
    minecraft_version: String,
) -> Result<Vec<String>, LauncherError> {
    let state = state.lock().await;
    let client = state.http_client.clone();

    let mut versions = match loader_type {
        LoaderType::Vanilla => vec![],
        LoaderType::Fabric => {
            #[derive(Deserialize)]
            struct FabricLoaderEntry {
                loader: FabricLoaderVersion,
            }
            #[derive(Deserialize)]
            struct FabricLoaderVersion {
                version: String,
            }

            let url = format!(
                "https://meta.fabricmc.net/v2/versions/loader/{}",
                minecraft_version
            );

            let response = client.get(url).send().await?;
            if !response.status().is_success() {
                return Err(LauncherError::LoaderApi(format!(
                    "Fabric API returned {}",
                    response.status()
                )));
            }

            response
                .json::<Vec<FabricLoaderEntry>>()
                .await?
                .into_iter()
                .map(|entry| entry.loader.version)
                .collect()
        }
        LoaderType::Quilt => loaders::quilt::list_loader_versions(&minecraft_version).await?,
        LoaderType::Forge => {
            let xml = client
                .get("https://maven.minecraftforge.net/net/minecraftforge/forge/maven-metadata.xml")
                .send()
                .await?
                .text()
                .await?;

            let metadata: MavenMetadata = quick_xml::de::from_str(&xml).map_err(|e| {
                LauncherError::LoaderApi(format!("Unable to parse Forge metadata: {e}"))
            })?;

            metadata
                .versioning
                .versions
                .version
                .into_iter()
                .filter_map(|v| {
                    v.strip_prefix(&format!("{}-", minecraft_version))
                        .map(str::to_owned)
                })
                .collect()
        }
        LoaderType::NeoForge => {
            let xml = client
                .get("https://maven.neoforged.net/releases/net/neoforged/neoforge/maven-metadata.xml")
                .send()
                .await?
                .text()
                .await?;

            let metadata: MavenMetadata = quick_xml::de::from_str(&xml).map_err(|e| {
                LauncherError::LoaderApi(format!("Unable to parse NeoForge metadata: {e}"))
            })?;

            let version_prefix = minecraft_version
                .trim_start_matches("1.")
                .split('.')
                .take(2)
                .collect::<Vec<_>>()
                .join(".");

            let mut resolved: Vec<String> = metadata
                .versioning
                .versions
                .version
                .into_iter()
                .filter(|v| v.starts_with(&version_prefix))
                .collect();

            // Legacy NeoForge builds for MC 1.20.1 were published as net.neoforged:forge.
            if minecraft_version == "1.20.1" {
                let legacy_xml = client
                    .get("https://maven.neoforged.net/releases/net/neoforged/forge/maven-metadata.xml")
                    .send()
                    .await?
                    .text()
                    .await?;

                let legacy_metadata: MavenMetadata =
                    quick_xml::de::from_str(&legacy_xml).map_err(|e| {
                        LauncherError::LoaderApi(format!(
                            "Unable to parse legacy NeoForge metadata: {e}"
                        ))
                    })?;

                resolved.extend(legacy_metadata.versioning.versions.version);
            }

            resolved
        }
    };

    versions.sort();
    versions.dedup();
    versions.sort();
    versions.reverse();
    versions.truncate(200);

    Ok(versions)
}

// ── Tauri Commands ──────────────────────────────────────

#[tauri::command]
pub async fn create_instance(
    state: tauri::State<'_, Arc<Mutex<AppState>>>,
    payload: CreateInstancePayload,
) -> Result<InstanceInfo, LauncherError> {
    let state = state.lock().await;

    let _java_major = if payload.minecraft_version.starts_with("1.20")
        || payload.minecraft_version.starts_with("1.21")
    {
        17
    } else {
        17 // Safe default; can be refined per version later
    };

    let mut instance = state
        .instance_manager
        .create(Instance::new(
            payload.name,
            payload.minecraft_version,
            payload.loader_type,
            payload.loader_version,
            payload.memory_max_mb.unwrap_or(2048),
            &state.instances_dir(),
        ))
        .await?;

    // Install vanilla base first
    let libs_dir = state.libraries_dir();
    let client = state.http_client.clone();
    let vanilla_installer = loaders::Installer::new(&LoaderType::Vanilla, client.clone());

    let vanilla_result = vanilla_installer
        .install(loaders::InstallContext {
            minecraft_version: &instance.minecraft_version,
            loader_version: "",
            instance_dir: &instance.path,
            libs_dir: &libs_dir,
            downloader: state.downloader.as_ref(),
            http_client: &client,
        })
        .await?;

    instance.main_class = Some(vanilla_result.main_class.clone());

    // Install loader if not vanilla
    if instance.loader != LoaderType::Vanilla {
        if let Some(ref loader_version) = instance.loader_version {
            let installer = loaders::Installer::new(&instance.loader, client.clone());
            let loader_result = installer
                .install(loaders::InstallContext {
                    minecraft_version: &instance.minecraft_version,
                    loader_version,
                    instance_dir: &instance.path,
                    libs_dir: &libs_dir,
                    downloader: state.downloader.as_ref(),
                    http_client: &client,
                })
                .await?;

            // Loader's main class overrides vanilla's
            instance.main_class = Some(loader_result.main_class);
        }
    }

    instance.state = InstanceState::Ready;
    state.instance_manager.save(&instance).await?;

    info!("Instance '{}' created and ready", instance.name);
    Ok(InstanceInfo::from(&instance))
}

#[tauri::command]
pub async fn list_instances(
    state: tauri::State<'_, Arc<Mutex<AppState>>>,
) -> Result<Vec<InstanceInfo>, LauncherError> {
    let state = state.lock().await;
    let instances = state.instance_manager.list().await?;
    Ok(instances.iter().map(InstanceInfo::from).collect())
}

#[tauri::command]
pub async fn delete_instance(
    state: tauri::State<'_, Arc<Mutex<AppState>>>,
    id: String,
) -> Result<(), LauncherError> {
    let state = state.lock().await;
    state.instance_manager.delete(&id).await?;
    info!("Deleted instance {}", id);
    Ok(())
}

#[tauri::command]
pub async fn launch_instance(
    state: tauri::State<'_, Arc<Mutex<AppState>>>,
    id: String,
) -> Result<(), LauncherError> {
    let state = state.lock().await;
    let mut instance = state.instance_manager.load(&id).await?;

    if instance.state != InstanceState::Ready {
        return Err(LauncherError::Other(format!(
            "Instance {} is not in Ready state (current: {:?})",
            id, instance.state
        )));
    }

    let libs_dir = state.libraries_dir();

    // TODO: Collect actual library coordinates from saved version data
    let lib_coords: Vec<String> = vec![];

    let classpath = launch::build_classpath(&instance, &libs_dir, &lib_coords)?;

    // Extract natives
    let _natives_dir = launch::extract_natives(&instance, &libs_dir, &[]).await?;

    // Update state
    instance.state = InstanceState::Running;
    state.instance_manager.save(&instance).await?;

    // Launch (non-blocking spawn)
    let _child = launch::launch(&instance, &classpath).await?;

    info!("Launched instance {}", instance.name);

    // Note: In production, you'd monitor the child process and
    // set state back to Ready when it exits.

    Ok(())
}

#[tauri::command]
pub async fn get_java_installations() -> Result<Vec<JavaInstallation>, LauncherError> {
    let installations = java::detect_java_installations().await;
    Ok(installations)
}
