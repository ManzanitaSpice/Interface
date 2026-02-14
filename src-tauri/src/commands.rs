use std::process::Command;
use std::sync::Arc;
use std::{fs, path::Path};
use std::{io::BufRead, io::BufReader as StdBufReader};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tauri::Emitter;
use tokio::sync::Mutex;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::core::assets::AssetManager;
use crate::core::auth::{AccountMode, AuthResearchInfo, LaunchAccountProfile};
use crate::core::error::LauncherError;
use crate::core::instance::{Instance, InstanceState, LoaderType};
use crate::core::java::{self, JavaInstallation};
use crate::core::launch;
use crate::core::loaders;
use crate::core::state::{AppState, JavaRuntimePreference, LauncherSettings};
use crate::core::version::VersionManifest;

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum DeleteInstanceResponse {
    Deleted,
    NeedsElevation,
    ElevationRequested,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LaunchDiagnostic {
    NeoForgeEarlyDisplayRendererFuture,
    CorruptedLibraryArchive,
}

fn detect_launch_diagnostic(line: &str) -> Option<LaunchDiagnostic> {
    if line.contains("rendererFuture") || line.contains("DisplayWindow.takeOverGlfwWindow") {
        return Some(LaunchDiagnostic::NeoForgeEarlyDisplayRendererFuture);
    }

    if line.contains("ZipException: zip END header not found") {
        return Some(LaunchDiagnostic::CorruptedLibraryArchive);
    }

    None
}

fn diagnostic_message(diagnostic: LaunchDiagnostic) -> &'static str {
    match diagnostic {
        LaunchDiagnostic::NeoForgeEarlyDisplayRendererFuture => {
            "[DIAGNÓSTICO] NeoForge falló en early display (rendererFuture nulo). Prueba actualizar NeoForge, usar Java 17/21 x64 limpia y desactivar overlays de GPU antes de reiniciar la instancia."
        }
        LaunchDiagnostic::CorruptedLibraryArchive => {
            "[DIAGNÓSTICO] Se detectó una librería dañada (zip END header not found). Cierra la instancia, borra la ruta `libraries/net/neoforged/neoform/...` indicada en el log y reinicia para forzar una descarga limpia."
        }
    }
}

#[derive(Debug, Serialize)]
pub struct MinecraftVersionInfo {
    pub id: String,
    pub release_time: String,
    pub version_type: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateInstancePayload {
    pub name: String,
    pub minecraft_version: String,
    pub loader_type: LoaderType,
    pub loader_version: Option<String>,
    pub memory_max_mb: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountProfilePayload {
    pub mode: AccountMode,
    pub username: String,
    pub uuid: Option<String>,
    pub access_token: Option<String>,
    pub xuid: Option<String>,
    pub user_type: Option<String>,
    pub client_id: Option<String>,
}

impl AccountProfilePayload {
    fn into_profile(self) -> LaunchAccountProfile {
        match self.mode {
            AccountMode::Offline => LaunchAccountProfile::offline(&self.username).sanitized(),
            AccountMode::Microsoft => LaunchAccountProfile {
                mode: AccountMode::Microsoft,
                username: self.username,
                uuid: self.uuid.unwrap_or_default(),
                access_token: self.access_token.unwrap_or_default(),
                xuid: self.xuid.unwrap_or_default(),
                user_type: self.user_type.unwrap_or_else(|| "msa".into()),
                client_id: self.client_id.unwrap_or_default(),
            }
            .sanitized(),
        }
    }

    fn from_profile(profile: &LaunchAccountProfile) -> Self {
        Self {
            mode: profile.mode.clone(),
            username: profile.username.clone(),
            uuid: Some(profile.uuid.clone()),
            access_token: Some(profile.access_token.clone()),
            xuid: Some(profile.xuid.clone()),
            user_type: Some(profile.user_type.clone()),
            client_id: Some(profile.client_id.clone()),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct UpdateInstanceAccountPayload {
    pub id: String,
    pub account: AccountProfilePayload,
}

#[derive(Debug, Serialize)]
pub struct InstanceInfo {
    pub id: String,
    pub name: String,
    pub path: String,
    pub minecraft_version: String,
    pub loader_type: LoaderType,
    pub loader_version: Option<String>,
    pub state: InstanceState,
    pub required_java_major: Option<u32>,
    pub java_path: Option<String>,
    pub max_memory_mb: u32,
    pub account: AccountProfilePayload,
    pub jvm_args: Vec<String>,
    pub game_args: Vec<String>,
    pub total_size_bytes: u64,
    pub created_at: String,
    pub last_played: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateInstanceLaunchConfigPayload {
    pub id: String,
    pub java_path: Option<String>,
    pub max_memory_mb: u32,
    pub jvm_args: Vec<String>,
    pub game_args: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LauncherSettingsPayload {
    pub java_runtime: JavaRuntimePreference,
    pub selected_java_path: Option<String>,
    pub embedded_java_available: bool,
    pub data_dir: String,
}

#[derive(Debug, Serialize)]
pub struct FirstLaunchStatus {
    pub first_launch: bool,
    pub suggested_data_dir: String,
}

#[derive(Debug, Deserialize)]
pub struct InitializeInstallationPayload {
    pub target_dir: String,
    pub create_desktop_shortcut: bool,
}

#[derive(Debug, Deserialize)]
pub struct MigrateLauncherDataDirPayload {
    pub target_dir: String,
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
            required_java_major: inst.required_java_major,
            java_path: inst
                .java_path
                .as_ref()
                .map(|p| p.to_string_lossy().to_string()),
            max_memory_mb: inst.max_memory_mb,
            account: AccountProfilePayload::from_profile(&inst.account),
            jvm_args: inst.jvm_args.clone(),
            game_args: inst.game_args.clone(),
            total_size_bytes: directory_size_bytes(&inst.path),
            created_at: inst.created_at.to_rfc3339(),
            last_played: inst.last_played.map(|date| date.to_rfc3339()),
        }
    }
}

fn directory_size_bytes(path: &std::path::Path) -> u64 {
    let mut total_size = 0_u64;
    let mut stack = vec![path.to_path_buf()];

    while let Some(current) = stack.pop() {
        let read_dir = match std::fs::read_dir(&current) {
            Ok(read_dir) => read_dir,
            Err(_) => continue,
        };

        for entry in read_dir.flatten() {
            let entry_path = entry.path();
            if let Ok(metadata) = entry.metadata() {
                if metadata.is_file() {
                    total_size = total_size.saturating_add(metadata.len());
                } else if metadata.is_dir() {
                    stack.push(entry_path);
                }
            }
        }
    }

    total_size
}

async fn validate_instance_state_before_launch(
    _state: &crate::core::state::AppState,
    instance: &Instance,
) -> Result<(), LauncherError> {
    if instance.state != InstanceState::Ready && instance.state != InstanceState::Error {
        return Err(LauncherError::Other(format!(
            "Instance {} is not in Ready state (current: {:?})",
            instance.id, instance.state
        )));
    }

    if instance.main_class.is_none() {
        return Err(LauncherError::Other(
            "Instancia inválida: falta main_class".into(),
        ));
    }

    if instance.required_java_major.is_none() {
        return Err(LauncherError::Other(
            "Instancia inválida: falta required_java_major".into(),
        ));
    }

    Ok(())
}

async fn validate_or_resolve_java(instance: &mut Instance) -> Result<(), LauncherError> {
    let required_major = instance.required_java_major.unwrap_or(17);
    if let Some(custom_path) = &instance.java_path {
        let installations = java::runtime::detect_java_installations_sync();
        if let Some(found) = installations
            .iter()
            .find(|candidate| candidate.path == *custom_path)
        {
            if found.major != required_major {
                return Err(LauncherError::Other(format!(
                    "La Java configurada ({}) no coincide con la versión requerida {}",
                    found.version, required_major
                )));
            }
            if !found.is_64bit {
                return Err(LauncherError::Other(
                    "La Java configurada debe ser de 64 bits".into(),
                ));
            }
            return Ok(());
        }

        if !custom_path.exists() {
            instance.java_path = None;
        }
    }

    let resolved = java::resolve_java_binary(required_major).await?;
    let detected = java::runtime::detect_java_installations_sync();
    let resolved_info = detected.iter().find(|candidate| candidate.path == resolved);

    if let Some(info) = resolved_info {
        if info.major != required_major {
            return Err(LauncherError::Other(format!(
                "La Java resuelta ({}) no coincide con la versión requerida {}",
                info.version, required_major
            )));
        }
        if !info.is_64bit {
            return Err(LauncherError::Other(
                "La Java resuelta debe ser de 64 bits".into(),
            ));
        }
    }

    instance.java_path = Some(resolved);
    Ok(())
}

async fn prepare_instance_for_launch(
    state: &crate::core::state::AppState,
    instance: &mut Instance,
) -> Result<(), LauncherError> {
    let game_dir = instance.game_dir();
    tokio::fs::create_dir_all(&game_dir)
        .await
        .map_err(|source| LauncherError::Io {
            path: game_dir.clone(),
            source,
        })?;
    let assets_dir = game_dir.join("assets");
    tokio::fs::create_dir_all(&assets_dir)
        .await
        .map_err(|source| LauncherError::Io {
            path: assets_dir.clone(),
            source,
        })?;
    let libs_dir = state.libraries_dir();
    tokio::fs::create_dir_all(&libs_dir)
        .await
        .map_err(|source| LauncherError::Io {
            path: libs_dir.clone(),
            source,
        })?;

    let needs_install = instance.main_class.is_none()
        || instance.required_java_major.is_none()
        || !instance.path.join("client.jar").exists()
        || instance.libraries.iter().any(|coord| {
            crate::core::maven::MavenArtifact::parse(coord)
                .map(|artifact| !libs_dir.join(artifact.local_path()).exists())
                .unwrap_or(false)
        });

    if needs_install {
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
        instance.asset_index = vanilla_result.asset_index_id.clone();
        instance.libraries = vanilla_result.libraries.clone();
        instance.required_java_major = vanilla_result.java_major;

        if instance.loader != LoaderType::Vanilla {
            if let Some(loader_version) = &instance.loader_version {
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
                instance.main_class = Some(loader_result.main_class);
                instance.jvm_args.extend(loader_result.extra_jvm_args);
                instance.game_args.extend(loader_result.extra_game_args);
                instance.libraries.extend(loader_result.libraries);
                if loader_result.asset_index_id.is_some() {
                    instance.asset_index = loader_result.asset_index_id;
                }
            }
        }

        if let Some(url) = vanilla_result.asset_index_url {
            AssetManager::download_assets(&url, &assets_dir, state.downloader.as_ref()).await?;
        }
    }

    if instance.main_class.is_none() || instance.required_java_major.is_none() {
        return Err(LauncherError::Other(
            "Instancia inválida: main_class o required_java_major no definidos".into(),
        ));
    }

    validate_or_resolve_java(instance).await?;
    instance.libraries.sort();
    instance.libraries.dedup();
    Ok(())
}

impl LauncherSettingsPayload {
    fn from_settings(settings: &LauncherSettings, embedded_java_available: bool) -> Self {
        Self {
            java_runtime: settings.java_runtime.clone(),
            selected_java_path: settings
                .selected_java_path
                .as_ref()
                .map(|p| p.to_string_lossy().to_string()),
            embedded_java_available,
            data_dir: String::new(),
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

#[derive(Debug, Clone, Serialize)]
struct InstanceLaunchProgressEvent {
    id: String,
    value: u8,
    stage: String,
    state: String,
}

#[derive(Debug, Clone, Serialize)]
struct InstanceLaunchLogEvent {
    id: String,
    level: String,
    message: String,
}

#[derive(Debug, Clone, Serialize)]
struct InstanceCreationProgressEvent {
    id: String,
    value: u8,
    stage: String,
    state: String,
}

#[derive(Debug, Clone, Serialize)]
struct InstanceCreationLogEvent {
    id: String,
    level: String,
    message: String,
}

fn emit_launch_progress(
    app_handle: &tauri::AppHandle,
    id: &str,
    value: u8,
    stage: &str,
    state: &str,
) {
    let _ = app_handle.emit(
        "instance-launch-progress",
        InstanceLaunchProgressEvent {
            id: id.to_string(),
            value,
            stage: stage.to_string(),
            state: state.to_string(),
        },
    );
}

fn emit_launch_log(app_handle: &tauri::AppHandle, id: &str, level: &str, message: String) {
    let _ = app_handle.emit(
        "instance-launch-log",
        InstanceLaunchLogEvent {
            id: id.to_string(),
            level: level.to_string(),
            message,
        },
    );
}

fn emit_create_progress(
    app_handle: &tauri::AppHandle,
    id: &str,
    value: u8,
    stage: &str,
    state: &str,
) {
    let _ = app_handle.emit(
        "instance-create-progress",
        InstanceCreationProgressEvent {
            id: id.to_string(),
            value,
            stage: stage.to_string(),
            state: state.to_string(),
        },
    );
}

fn emit_create_log(app_handle: &tauri::AppHandle, id: &str, level: &str, message: String) {
    let _ = app_handle.emit(
        "instance-create-log",
        InstanceCreationLogEvent {
            id: id.to_string(),
            level: level.to_string(),
            message,
        },
    );
}

#[tauri::command]
pub async fn get_minecraft_versions(
    state: tauri::State<'_, Arc<Mutex<AppState>>>,
) -> Result<Vec<String>, LauncherError> {
    let state = state.lock().await;
    let manifest = VersionManifest::fetch(&state.http_client).await?;

    let versions: Vec<String> = manifest
        .versions
        .iter()
        .filter(|entry| entry.version_type == "release")
        .filter(|entry| !entry.id.to_ascii_lowercase().contains("demo"))
        .map(|entry| entry.id.clone())
        .collect();

    Ok(versions)
}

#[tauri::command]
pub async fn get_minecraft_versions_detailed(
    state: tauri::State<'_, Arc<Mutex<AppState>>>,
) -> Result<Vec<MinecraftVersionInfo>, LauncherError> {
    let state = state.lock().await;
    let manifest = VersionManifest::fetch(&state.http_client).await?;

    let versions = manifest
        .versions
        .into_iter()
        .filter(|entry| !entry.id.to_ascii_lowercase().contains("demo"))
        .map(|entry| MinecraftVersionInfo {
            id: entry.id,
            release_time: entry.release_time,
            version_type: entry.version_type,
        })
        .collect();

    Ok(versions)
}

fn version_sort_key(version: &str) -> Vec<u64> {
    version
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(|part| part.parse::<u64>().unwrap_or(0))
        .collect()
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

            let entries = response.json::<Vec<FabricLoaderEntry>>().await?;

            entries
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

    versions.sort_by(|a, b| {
        version_sort_key(b)
            .cmp(&version_sort_key(a))
            .then_with(|| b.cmp(a))
    });
    versions.dedup();

    Ok(versions)
}

#[tauri::command]
pub async fn create_instance(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<Mutex<AppState>>>,
    payload: CreateInstancePayload,
) -> Result<InstanceInfo, LauncherError> {
    let state = state.lock().await;

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

    emit_create_progress(&app, &instance.id, 8, "Estructura creada", "running");
    emit_create_log(
        &app,
        &instance.id,
        "info",
        "Instancia creada en disco, iniciando instalación base...".into(),
    );

    let libs_dir = state.libraries_dir();
    let client = state.http_client.clone();
    let vanilla_installer = loaders::Installer::new(&LoaderType::Vanilla, client.clone());

    if let Err(err) = state
        .instance_manager
        .set_state(&mut instance, InstanceState::Installing)
        .await
    {
        error!(
            "Cannot persist installing state for {}: {}",
            instance.id, err
        );
    }
    emit_create_progress(&app, &instance.id, 16, "Preparando Vanilla", "running");

    let install_result: Result<(), LauncherError> = async {
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

        emit_create_progress(&app, &instance.id, 42, "Vanilla instalado", "running");
        emit_create_log(
            &app,
            &instance.id,
            "info",
            "Runtime Vanilla preparado.".into(),
        );

        instance.main_class = Some(vanilla_result.main_class.clone());
        instance.asset_index = vanilla_result.asset_index_id.clone();
        instance.libraries = vanilla_result.libraries.clone();
        instance.jvm_args = vanilla_result.extra_jvm_args.clone();
        instance.game_args = vanilla_result.extra_game_args.clone();
        instance.required_java_major = vanilla_result.java_major;

        if instance.loader != LoaderType::Vanilla {
            if let Some(ref loader_version) = instance.loader_version {
                emit_create_progress(&app, &instance.id, 56, "Instalando loader", "running");
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

                emit_create_log(
                    &app,
                    &instance.id,
                    "info",
                    format!("Loader {} {} instalado.", instance.loader, loader_version),
                );

                instance.main_class = Some(loader_result.main_class);
                instance.jvm_args.extend(loader_result.extra_jvm_args);
                instance.game_args.extend(loader_result.extra_game_args);
                instance.libraries.extend(loader_result.libraries);
                if loader_result.asset_index_id.is_some() {
                    instance.asset_index = loader_result.asset_index_id;
                }
            }
        }

        let assets_dir = instance.game_dir().join("assets");
        tokio::fs::create_dir_all(&assets_dir)
            .await
            .map_err(|source| LauncherError::Io {
                path: assets_dir.clone(),
                source,
            })?;

        if let Some(url) = vanilla_result.asset_index_url {
            emit_create_progress(&app, &instance.id, 72, "Descargando assets", "running");
            AssetManager::download_assets(&url, &assets_dir, state.downloader.as_ref()).await?;
        }

        instance.libraries.sort();
        instance.libraries.dedup();

        match state.launcher_settings.java_runtime {
            JavaRuntimePreference::System => {
                if let Some(ref selected) = state.launcher_settings.selected_java_path {
                    instance.java_path = Some(selected.clone());
                }
            }
            JavaRuntimePreference::Embedded => {
                let embedded = state.embedded_java_path();
                if crate::core::java::runtime::is_usable_java_binary(&embedded) {
                    instance.java_path = Some(embedded);
                }
            }
            JavaRuntimePreference::Auto => {}
        }

        Ok(())
    }
    .await;

    if let Err(err) = install_result {
        emit_create_progress(&app, &instance.id, 100, "Error en creación", "error");
        emit_create_log(
            &app,
            &instance.id,
            "error",
            format!("Falló la creación: {err}"),
        );
        instance.state = InstanceState::Error;
        if let Err(save_err) = state.instance_manager.save(&instance).await {
            error!(
                "Cannot persist failed instance state for {}: {}",
                instance.id, save_err
            );
        }
        return Err(err);
    }

    instance.state = InstanceState::Ready;
    state.instance_manager.verify_structure(&instance).await?;
    state.instance_manager.save(&instance).await?;
    emit_create_progress(&app, &instance.id, 100, "Instancia lista", "done");
    emit_create_log(
        &app,
        &instance.id,
        "info",
        "Instancia creada correctamente y verificada.".into(),
    );

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

fn is_permission_error(error: &LauncherError) -> bool {
    match error {
        LauncherError::Io { source, .. } => {
            source.kind() == std::io::ErrorKind::PermissionDenied
                || matches!(source.raw_os_error(), Some(5 | 32))
        }
        _ => false,
    }
}

#[cfg(target_os = "windows")]
fn request_windows_elevated_delete(target: &Path) -> Result<(), LauncherError> {
    let escaped_target = target.display().to_string().replace('"', "`\"");
    let script = format!(
        "Start-Process -FilePath powershell -Verb RunAs -WindowStyle Hidden -ArgumentList @('-NoProfile','-Command','Remove-Item -LiteralPath \"{}\" -Recurse -Force')",
        escaped_target
    );

    let status = Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .status()
        .map_err(|source| LauncherError::Io {
            path: target.to_path_buf(),
            source,
        })?;

    if status.success() {
        Ok(())
    } else {
        Err(LauncherError::Other(
            "No se pudo solicitar permisos de administrador para eliminar la instancia.".into(),
        ))
    }
}

#[tauri::command]
pub async fn delete_instance_with_elevation(
    state: tauri::State<'_, Arc<Mutex<AppState>>>,
    id: String,
    request_elevation: bool,
) -> Result<DeleteInstanceResponse, LauncherError> {
    let mut state = state.lock().await;

    if let Some(pid) = state.running_instances.remove(&id) {
        kill_process(pid)?;
    }

    match state.instance_manager.delete(&id).await {
        Ok(_) => {
            info!("Deleted instance {}", id);
            Ok(DeleteInstanceResponse::Deleted)
        }
        Err(error) if is_permission_error(&error) => {
            if !request_elevation {
                return Ok(DeleteInstanceResponse::NeedsElevation);
            }

            #[cfg(target_os = "windows")]
            {
                let target = state.instances_dir().join(&id);
                request_windows_elevated_delete(&target)?;
                return Ok(DeleteInstanceResponse::ElevationRequested);
            }

            #[cfg(not(target_os = "windows"))]
            {
                Err(LauncherError::Other(
                    "La elevación de privilegios para eliminar instancias sólo está disponible en Windows."
                        .into(),
                ))
            }
        }
        Err(error) => Err(error),
    }
}

#[tauri::command]
pub async fn clone_instance(
    state: tauri::State<'_, Arc<Mutex<AppState>>>,
    id: String,
) -> Result<InstanceInfo, LauncherError> {
    let state = state.lock().await;
    let source = state.instance_manager.load(&id).await?;

    let mut cloned = source.clone();
    cloned.id = Uuid::new_v4().to_string();
    cloned.name = format!("{} (Copia)", source.name);
    cloned.path = state.instances_dir().join(&cloned.id);
    cloned.state = InstanceState::Ready;
    cloned.last_played = None;
    cloned.created_at = Utc::now();

    copy_dir_recursive(&source.path, &cloned.path)?;
    state.instance_manager.save(&cloned).await?;
    info!("Cloned instance {} into {}", source.id, cloned.id);
    Ok(InstanceInfo::from(&cloned))
}

#[tauri::command]
pub async fn launch_instance(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, Arc<Mutex<AppState>>>,
    id: String,
) -> Result<(), LauncherError> {
    let state_arc = state.inner().clone();
    emit_launch_progress(
        &app_handle,
        &id,
        5,
        "Iniciando validación de instancia",
        "running",
    );
    emit_launch_log(
        &app_handle,
        &id,
        "info",
        "[PREPARACIÓN] Solicitud de inicio recibida en backend.".into(),
    );

    let mut child = {
        let mut state_guard = state_arc.lock().await;
        let mut instance = state_guard.instance_manager.load(&id).await?;

        if let Err(err) = validate_instance_state_before_launch(&state_guard, &instance).await {
            emit_launch_progress(&app_handle, &id, 100, "Validación fallida", "error");
            emit_launch_log(
                &app_handle,
                &id,
                "error",
                format!("[ERROR] Validación fallida: {err}"),
            );
            instance.state = InstanceState::Error;
            state_guard.instance_manager.save(&instance).await?;
            return Err(err);
        }

        emit_launch_progress(&app_handle, &id, 15, "Validación completada", "running");
        emit_launch_log(
            &app_handle,
            &id,
            "info",
            "[PREPARACIÓN] Validación completada. Preparando archivos, Java y librerías.".into(),
        );

        instance.state = InstanceState::Installing;
        state_guard.instance_manager.save(&instance).await?;

        if let Err(err) = prepare_instance_for_launch(&state_guard, &mut instance).await {
            emit_launch_progress(&app_handle, &id, 100, "Error en preparación", "error");
            emit_launch_log(
                &app_handle,
                &id,
                "error",
                format!("[ERROR] Preparación fallida: {err}"),
            );
            instance.state = InstanceState::Error;
            state_guard.instance_manager.save(&instance).await?;
            return Err(err);
        }

        emit_launch_progress(
            &app_handle,
            &id,
            72,
            "Instalación y verificación completadas",
            "running",
        );
        emit_launch_log(
            &app_handle,
            &id,
            "info",
            "[DESCARGA] Recursos y dependencias listos. Construyendo classpath y extrayendo nativos.".into(),
        );

        let libs_dir = state_guard.libraries_dir();
        let classpath = launch::build_classpath(&instance, &libs_dir, &instance.libraries)?;
        let _natives_dir =
            launch::extract_natives(&instance, &libs_dir, &instance.libraries).await?;

        emit_launch_progress(
            &app_handle,
            &id,
            90,
            "Lanzando proceso de Minecraft",
            "running",
        );

        let child = match launch::launch(&instance, &classpath, &libs_dir).await {
            Ok(child) => child,
            Err(err) => {
                emit_launch_progress(&app_handle, &id, 100, "Error al iniciar proceso", "error");
                emit_launch_log(
                    &app_handle,
                    &id,
                    "error",
                    format!("[ERROR] No se pudo lanzar Minecraft: {err}"),
                );
                instance.state = InstanceState::Error;
                state_guard.instance_manager.save(&instance).await?;
                return Err(err);
            }
        };
        instance.state = InstanceState::Running;
        instance.last_played = Some(Utc::now());
        state_guard.instance_manager.save(&instance).await?;
        let pid = child.id();
        state_guard.running_instances.insert(id.clone(), pid);
        info!("Launched instance {}", instance.name);
        emit_launch_progress(&app_handle, &id, 100, "Instancia en ejecución", "done");
        emit_launch_log(
            &app_handle,
            &id,
            "info",
            format!("[RUNTIME] Instancia en ejecución (PID {pid})."),
        );

        child
    };

    if let Some(stdout) = child.stdout.take() {
        let instance_id = id.clone();
        let app_handle = app_handle.clone();
        tauri::async_runtime::spawn(async move {
            let _ = tauri::async_runtime::spawn_blocking(move || {
                for line in StdBufReader::new(stdout).lines().map_while(Result::ok) {
                    emit_launch_log(&app_handle, &instance_id, "info", line.clone());
                    info!("[mc:{}][stdout] {}", instance_id, line);
                }
            })
            .await;
        });
    }

    if let Some(stderr) = child.stderr.take() {
        let instance_id = id.clone();
        let app_handle = app_handle.clone();
        tauri::async_runtime::spawn(async move {
            let _ = tauri::async_runtime::spawn_blocking(move || {
                let mut neoforge_hint_emitted = false;
                let mut corrupted_zip_hint_emitted = false;
                for line in StdBufReader::new(stderr).lines().map_while(Result::ok) {
                    emit_launch_log(&app_handle, &instance_id, "warn", line.clone());
                    if let Some(diagnostic) = detect_launch_diagnostic(&line) {
                        let should_emit = match diagnostic {
                            LaunchDiagnostic::NeoForgeEarlyDisplayRendererFuture => {
                                if neoforge_hint_emitted {
                                    false
                                } else {
                                    neoforge_hint_emitted = true;
                                    true
                                }
                            }
                            LaunchDiagnostic::CorruptedLibraryArchive => {
                                if corrupted_zip_hint_emitted {
                                    false
                                } else {
                                    corrupted_zip_hint_emitted = true;
                                    true
                                }
                            }
                        };

                        if should_emit {
                            emit_launch_log(
                                &app_handle,
                                &instance_id,
                                "error",
                                diagnostic_message(diagnostic).into(),
                            );
                        }
                    }
                    warn!("[mc:{}][stderr] {}", instance_id, line);
                }
            })
            .await;
        });
    }

    let app_handle_for_wait = app_handle.clone();
    tauri::async_runtime::spawn(async move {
        let wait_result = tauri::async_runtime::spawn_blocking(move || child.wait())
            .await
            .map_err(|e| std::io::Error::other(e.to_string()))
            .and_then(|result| result);
        let mut state = state_arc.lock().await;

        state.running_instances.remove(&id);
        match state.instance_manager.load(&id).await {
            Ok(mut persisted) => {
                persisted.state = InstanceState::Ready;
                launch::cleanup_natives(&persisted).await;
                if let Err(err) = state.instance_manager.save(&persisted).await {
                    error!("Cannot persist ready state for {}: {}", id, err);
                }
            }
            Err(err) => error!("Cannot load instance {} after process exit: {}", id, err),
        }

        match wait_result {
            Ok(status) => {
                if status.success() {
                    emit_launch_progress(
                        &app_handle_for_wait,
                        &id,
                        0,
                        "Pendiente de inicio",
                        "idle",
                    );
                    emit_launch_log(
                        &app_handle_for_wait,
                        &id,
                        "info",
                        "[RUNTIME] Proceso finalizado correctamente.".into(),
                    );
                    info!(
                        "Minecraft process for {} exited with status: {:?}",
                        id, status
                    );
                } else {
                    emit_launch_progress(
                        &app_handle_for_wait,
                        &id,
                        100,
                        "Minecraft finalizó con error",
                        "error",
                    );
                    emit_launch_log(
                        &app_handle_for_wait,
                        &id,
                        "error",
                        match status.code() {
                            Some(code) => format!("[ERROR] El proceso finalizó con código {code}"),
                            None => "[ERROR] El proceso finalizó sin código de salida (terminación externa).".into(),
                        },
                    );
                    error!(
                        "Minecraft process for {} exited abnormally with status: {:?}",
                        id, status
                    );
                    if let Ok(mut persisted) = state.instance_manager.load(&id).await {
                        persisted.state = InstanceState::Error;
                        if let Err(save_err) = state.instance_manager.save(&persisted).await {
                            error!("Cannot persist error state for {}: {}", id, save_err);
                        }
                    }
                }
            }
            Err(err) => {
                emit_launch_progress(
                    &app_handle_for_wait,
                    &id,
                    100,
                    "Error de espera del proceso",
                    "error",
                );
                emit_launch_log(
                    &app_handle_for_wait,
                    &id,
                    "error",
                    format!("[ERROR] Fallo al esperar el proceso: {err}"),
                );
                error!("Minecraft process for {} failed while waiting: {}", id, err)
            }
        }
    });

    Ok(())
}

#[tauri::command]
pub async fn update_instance_launch_config(
    state: tauri::State<'_, Arc<Mutex<AppState>>>,
    payload: UpdateInstanceLaunchConfigPayload,
) -> Result<InstanceInfo, LauncherError> {
    let state = state.lock().await;
    let mut instance = state.instance_manager.load(&payload.id).await?;

    if payload.max_memory_mb < 512 {
        return Err(LauncherError::Other(
            "La memoria mínima permitida es 512 MB".into(),
        ));
    }

    instance.max_memory_mb = payload.max_memory_mb;
    instance.jvm_args = payload
        .jvm_args
        .into_iter()
        .filter(|arg| !arg.trim().is_empty())
        .collect();
    instance.game_args = payload
        .game_args
        .into_iter()
        .filter(|arg| !arg.trim().is_empty())
        .collect();
    instance.java_path = payload.java_path.map(std::path::PathBuf::from);
    state.instance_manager.save(&instance).await?;

    Ok(InstanceInfo::from(&instance))
}

#[tauri::command]
pub async fn force_close_instance(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, Arc<Mutex<AppState>>>,
    id: String,
) -> Result<(), LauncherError> {
    let mut state = state.lock().await;
    let mut instance = state.instance_manager.load(&id).await?;

    let Some(pid) = state.running_instances.remove(&id) else {
        if instance.state == InstanceState::Running {
            instance.state = InstanceState::Ready;
            state.instance_manager.save(&instance).await?;
            emit_launch_progress(&app_handle, &id, 0, "Pendiente de inicio", "idle");
            emit_launch_log(
                &app_handle,
                &id,
                "warn",
                "[RUNTIME] No había PID registrado. Estado corregido a listo.".into(),
            );
        }
        return Err(LauncherError::Other(format!(
            "No hay proceso activo para la instancia {id}"
        )));
    };

    kill_process(pid)?;
    instance.state = InstanceState::Ready;
    state.instance_manager.save(&instance).await?;
    emit_launch_progress(&app_handle, &id, 0, "Instancia detenida", "idle");
    emit_launch_log(
        &app_handle,
        &id,
        "warn",
        format!("[RUNTIME] Instancia detenida por usuario (PID {pid})."),
    );

    info!("Force closed instance {} (pid {})", id, pid);
    Ok(())
}

fn kill_process(pid: u32) -> Result<(), LauncherError> {
    #[cfg(target_os = "windows")]
    {
        let status = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .status()
            .map_err(|e| {
                LauncherError::Other(format!("No se pudo finalizar proceso {pid}: {e}"))
            })?;

        if !status.success() {
            return Err(LauncherError::Other(format!(
                "El comando para cerrar el proceso {pid} devolvió código {:?}",
                status.code()
            )));
        }

        return Ok(());
    }

    #[cfg(not(target_os = "windows"))]
    {
        let graceful = Command::new("kill")
            .args(["-15", &pid.to_string()])
            .status()
            .map_err(|e| LauncherError::Other(format!("No se pudo enviar SIGTERM a {pid}: {e}")))?;

        if graceful.success() {
            std::thread::sleep(std::time::Duration::from_millis(300));
            let check = Command::new("kill").args(["-0", &pid.to_string()]).status();
            if matches!(check, Ok(status) if !status.success()) {
                return Ok(());
            }
        }

        let force = Command::new("kill")
            .args(["-9", &pid.to_string()])
            .status()
            .map_err(|e| {
                LauncherError::Other(format!("No se pudo finalizar proceso {pid}: {e}"))
            })?;

        if !force.success() {
            return Err(LauncherError::Other(format!(
                "El comando para cerrar el proceso {pid} devolvió código {:?}",
                force.code()
            )));
        }

        Ok(())
    }
}

fn copy_dir_recursive(source: &Path, destination: &Path) -> Result<(), LauncherError> {
    if destination.exists() {
        return Err(LauncherError::InstanceAlreadyExists(
            destination.to_string_lossy().to_string(),
        ));
    }

    fs::create_dir_all(destination).map_err(|source_err| LauncherError::Io {
        path: destination.to_path_buf(),
        source: source_err,
    })?;

    for entry in fs::read_dir(source).map_err(|source_err| LauncherError::Io {
        path: source.to_path_buf(),
        source: source_err,
    })? {
        let entry = entry.map_err(|source_err| LauncherError::Io {
            path: source.to_path_buf(),
            source: source_err,
        })?;
        let src_path = entry.path();
        let dst_path = destination.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path).map_err(|source_err| LauncherError::Io {
                path: dst_path,
                source: source_err,
            })?;
        }
    }

    Ok(())
}

#[tauri::command]
pub async fn get_auth_research_info() -> Result<AuthResearchInfo, LauncherError> {
    Ok(AuthResearchInfo::default())
}

#[tauri::command]
pub async fn update_instance_account(
    state: tauri::State<'_, Arc<Mutex<AppState>>>,
    payload: UpdateInstanceAccountPayload,
) -> Result<InstanceInfo, LauncherError> {
    let state = state.lock().await;
    let mut instance = state.instance_manager.load(&payload.id).await?;
    instance.account = payload.account.into_profile();
    state.instance_manager.save(&instance).await?;
    Ok(InstanceInfo::from(&instance))
}

#[tauri::command]
pub async fn get_java_installations() -> Result<Vec<JavaInstallation>, LauncherError> {
    Ok(java::detect_java_installations().await)
}

#[tauri::command]
pub async fn get_first_launch_status(
    state: tauri::State<'_, Arc<Mutex<AppState>>>,
) -> Result<FirstLaunchStatus, LauncherError> {
    let state = state.lock().await;
    Ok(FirstLaunchStatus {
        first_launch: state.is_first_launch(),
        suggested_data_dir: state.data_dir.to_string_lossy().to_string(),
    })
}

#[tauri::command]
pub async fn initialize_launcher_installation(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, Arc<Mutex<AppState>>>,
    payload: InitializeInstallationPayload,
) -> Result<LauncherSettingsPayload, LauncherError> {
    let mut state = state.lock().await;
    let installed_dir = state
        .initialize_launcher_installation(
            &app_handle,
            std::path::PathBuf::from(payload.target_dir),
            payload.create_desktop_shortcut,
        )
        .map_err(|e| {
            LauncherError::Other(format!("No se pudo completar la instalación inicial: {e}"))
        })?;

    let embedded_available =
        crate::core::java::runtime::is_usable_java_binary(&state.embedded_java_path());
    let mut response =
        LauncherSettingsPayload::from_settings(&state.launcher_settings, embedded_available);
    response.data_dir = installed_dir.to_string_lossy().to_string();
    Ok(response)
}

#[tauri::command]
pub async fn reinstall_launcher_completely(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, Arc<Mutex<AppState>>>,
) -> Result<LauncherSettingsPayload, LauncherError> {
    let mut state = state.lock().await;
    state
        .reinstall_launcher(&app_handle)
        .map_err(|e| LauncherError::Other(format!("No se pudo reinstalar el launcher: {e}")))?;

    let embedded_available =
        crate::core::java::runtime::is_usable_java_binary(&state.embedded_java_path());
    let mut response =
        LauncherSettingsPayload::from_settings(&state.launcher_settings, embedded_available);
    response.data_dir = state.data_dir.to_string_lossy().to_string();
    Ok(response)
}

#[tauri::command]
pub async fn get_launcher_settings(
    state: tauri::State<'_, Arc<Mutex<AppState>>>,
) -> Result<LauncherSettingsPayload, LauncherError> {
    let state = state.lock().await;
    let embedded_available =
        crate::core::java::runtime::is_usable_java_binary(&state.embedded_java_path());
    let mut payload =
        LauncherSettingsPayload::from_settings(&state.launcher_settings, embedded_available);
    payload.data_dir = state.data_dir.to_string_lossy().to_string();
    Ok(payload)
}

#[tauri::command]
pub async fn update_launcher_settings(
    state: tauri::State<'_, Arc<Mutex<AppState>>>,
    payload: LauncherSettingsPayload,
) -> Result<LauncherSettingsPayload, LauncherError> {
    let mut state = state.lock().await;

    state.launcher_settings.java_runtime = payload.java_runtime;
    state.launcher_settings.selected_java_path = payload
        .selected_java_path
        .as_ref()
        .map(std::path::PathBuf::from);

    state.save_settings().map_err(|e| {
        LauncherError::Other(format!("No se pudo guardar launcher_settings.json: {e}"))
    })?;

    let embedded_available =
        crate::core::java::runtime::is_usable_java_binary(&state.embedded_java_path());
    let mut payload =
        LauncherSettingsPayload::from_settings(&state.launcher_settings, embedded_available);
    payload.data_dir = state.data_dir.to_string_lossy().to_string();
    Ok(payload)
}

#[tauri::command]
pub async fn migrate_launcher_data_dir(
    state: tauri::State<'_, Arc<Mutex<AppState>>>,
    payload: MigrateLauncherDataDirPayload,
) -> Result<LauncherSettingsPayload, LauncherError> {
    let mut state = state.lock().await;
    let target = std::path::PathBuf::from(payload.target_dir);
    let migrated_to = state
        .migrate_data_dir(target)
        .map_err(|e| LauncherError::Other(format!("No se pudo migrar el launcher: {e}")))?;

    let embedded_available =
        crate::core::java::runtime::is_usable_java_binary(&state.embedded_java_path());
    let mut response =
        LauncherSettingsPayload::from_settings(&state.launcher_settings, embedded_available);
    response.data_dir = migrated_to.to_string_lossy().to_string();
    Ok(response)
}
