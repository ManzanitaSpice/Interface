use std::process::Command;
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
    pub path: String,
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
            path: inst.path.to_string_lossy().to_string(),
            minecraft_version: inst.minecraft_version.clone(),
            loader_type: inst.loader.clone(),
            loader_version: inst.loader_version.clone(),
            state: inst.state.clone(),
        }
    }
}

#[tauri::command]
pub async fn open_instance_folder(
    state: tauri::State<'_, Arc<Mutex<AppState>>>,
    id: String,
) -> Result<(), LauncherError> {
    let state = state.lock().await;
    let instance = state.instance_manager.load(&id).await?;
    let folder = instance.path;

    #[cfg(target_os = "windows")]
    let mut command = {
        let mut cmd = Command::new("explorer");
        cmd.arg(&folder);
        cmd
    };

    #[cfg(target_os = "macos")]
    let mut command = {
        let mut cmd = Command::new("open");
        cmd.arg(&folder);
        cmd
    };

    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        let mut cmd = Command::new("xdg-open");
        cmd.arg(&folder);
        cmd
    };

    let status = command.status().map_err(|source| LauncherError::Io {
        path: folder.clone(),
        source,
    })?;

    if !status.success() {
        return Err(LauncherError::Other(format!(
            "No se pudo abrir el explorador para {:?}",
            folder
        )));
    }

    Ok(())
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

    let mut versions: Vec<String> = manifest
        .versions
        .iter()
        .filter(|entry| entry.version_type == "release")
        .map(|entry| entry.id.clone())
        .collect();

    if versions.is_empty() {
        versions = manifest
            .versions
            .iter()
            .map(|entry| entry.id.clone())
            .collect();
    }

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
                #[serde(default)]
                stable: bool,
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

            let entries = response.json::<Vec<FabricLoaderEntry>>().await?;

            let stable_exists = entries.iter().any(|entry| entry.stable);

            entries
                .into_iter()
                .filter(|entry| !stable_exists || entry.stable)
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
    let mut state = state.lock().await;
    if let Some(pid) = state.running_instances.remove(&id) {
        kill_process(pid)?;
    }
    state.instance_manager.delete(&id).await?;
    info!("Deleted instance {}", id);
    Ok(())
}

#[tauri::command]
pub async fn launch_instance(
    state: tauri::State<'_, Arc<Mutex<AppState>>>,
    id: String,
) -> Result<(), LauncherError> {
    let mut state = state.lock().await;
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
    let child = launch::launch(&instance, &classpath).await?;
    let pid = child.id();
    state.running_instances.insert(id.clone(), pid);

    info!("Launched instance {}", instance.name);

    // Note: In production, you'd monitor the child process and
    // set state back to Ready when it exits.

    Ok(())
}

#[tauri::command]
pub async fn force_close_instance(
    state: tauri::State<'_, Arc<Mutex<AppState>>>,
    id: String,
) -> Result<(), LauncherError> {
    let mut state = state.lock().await;
    let mut instance = state.instance_manager.load(&id).await?;

    let pid = state.running_instances.remove(&id).ok_or_else(|| {
        LauncherError::Other(format!("No hay proceso activo para la instancia {id}"))
    })?;

    kill_process(pid)?;
    instance.state = InstanceState::Ready;
    state.instance_manager.save(&instance).await?;

    info!("Force closed instance {} (pid {})", id, pid);
    Ok(())
}

fn kill_process(pid: u32) -> Result<(), LauncherError> {
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut cmd = Command::new("taskkill");
        cmd.args(["/PID", &pid.to_string(), "/T", "/F"]);
        cmd
    };

    #[cfg(not(target_os = "windows"))]
    let mut command = {
        let mut cmd = Command::new("kill");
        cmd.args(["-9", &pid.to_string()]);
        cmd
    };

    let status = command
        .status()
        .map_err(|e| LauncherError::Other(format!("No se pudo finalizar proceso {pid}: {e}")))?;

    if !status.success() {
        return Err(LauncherError::Other(format!(
            "El comando para cerrar el proceso {pid} devolvió código {:?}",
            status.code()
        )));
    }

    Ok(())
}

#[tauri::command]
pub async fn get_java_installations() -> Result<Vec<JavaInstallation>, LauncherError> {
    let installations = java::detect_java_installations().await;
    Ok(installations)
}
