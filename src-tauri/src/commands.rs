use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::process::Command;
use std::sync::Arc;
use std::{fs, path::Path};
use std::{io::BufRead, io::BufReader as StdBufReader};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sysinfo::System;
use tauri::Emitter;
use tokio::sync::Mutex;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::core::assets::AssetManager;
use crate::core::auth::{AccountMode, AuthResearchInfo, LaunchAccountProfile};
use crate::core::error::LauncherError;
use crate::core::instance::{Instance, InstanceState, LoaderType};
use crate::core::java::{self, JavaInstallation, RuntimeRole};
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
    NeoForgeEarlyDisplayStillEnabled,
    CorruptedLibraryArchive,
    LoaderAsmTooOldForJava21,
}

fn detect_launch_diagnostic(line: &str) -> Option<LaunchDiagnostic> {
    if line.contains("rendererFuture") || line.contains("DisplayWindow.takeOverGlfwWindow") {
        return Some(LaunchDiagnostic::NeoForgeEarlyDisplayRendererFuture);
    }

    if line.contains("Loading ImmediateWindowProvider fmlearlywindow") {
        return Some(LaunchDiagnostic::NeoForgeEarlyDisplayStillEnabled);
    }

    if line.contains("ZipException: zip END header not found") {
        return Some(LaunchDiagnostic::CorruptedLibraryArchive);
    }

    if line.contains("Unsupported class file major version 65")
        || line.contains("org.objectweb.asm.ClassReader")
    {
        return Some(LaunchDiagnostic::LoaderAsmTooOldForJava21);
    }

    None
}

fn diagnostic_message(diagnostic: LaunchDiagnostic) -> &'static str {
    match diagnostic {
        LaunchDiagnostic::NeoForgeEarlyDisplayRendererFuture => {
            "[DIAGNÓSTICO] NeoForge falló en early display (rendererFuture nulo). Usa JVM args (antes de -cp): -Dfml.earlyprogresswindow=false. Si el log muestra 'Loading ImmediateWindowProvider fmlearlywindow', el flag no está entrando."
        }
        LaunchDiagnostic::NeoForgeEarlyDisplayStillEnabled => {
            "[DIAGNÓSTICO] El early window sigue activo ('Loading ImmediateWindowProvider fmlearlywindow'). Revisa que el JVM arg sea exactamente -Dfml.earlyprogresswindow=false y que se inyecte antes de -cp."
        }
        LaunchDiagnostic::CorruptedLibraryArchive => {
            "[DIAGNÓSTICO] Se detectó una librería dañada (zip END header not found). Cierra la instancia, borra la ruta `libraries/net/neoforged/neoform/...` indicada en el log y reinicia para forzar una descarga limpia."
        }
        LaunchDiagnostic::LoaderAsmTooOldForJava21 => {
            "[DIAGNÓSTICO] El loader usa ASM antiguo y no soporta bytecode Java 21 (major 65). Actualiza Forge/NeoForge de esta línea de Minecraft a una build más reciente (ASM 9.7+)."
        }
    }
}

fn parse_numeric_version_parts(raw: &str) -> Vec<u32> {
    raw.split(|c: char| !c.is_ascii_digit())
        .filter(|segment| !segment.is_empty())
        .filter_map(|segment| segment.parse::<u32>().ok())
        .collect()
}

fn asm_version_supports_java_21(version: &str) -> bool {
    let parts = parse_numeric_version_parts(version);
    let major = parts.first().copied().unwrap_or(0);
    let minor = parts.get(1).copied().unwrap_or(0);
    major > 9 || (major == 9 && minor >= 7)
}

fn detect_loader_asm_incompatibility(
    instance: &Instance,
    required_java_major: u32,
) -> Option<String> {
    if required_java_major < 21 {
        return None;
    }

    if !matches!(instance.loader, LoaderType::Forge | LoaderType::NeoForge) {
        return None;
    }

    let asm_versions: Vec<String> = instance
        .libraries
        .iter()
        .filter_map(|coord| crate::core::maven::MavenArtifact::parse(coord).ok())
        .filter(|artifact| artifact.group_id == "org.ow2.asm")
        .map(|artifact| artifact.version)
        .collect();

    let has_old_asm = asm_versions
        .iter()
        .any(|version| !asm_version_supports_java_21(version));

    if !has_old_asm {
        return None;
    }

    let versions = asm_versions.join(", ");
    Some(format!(
        "El loader seleccionado requiere Java de herramientas diferente al de ejecución. Loader incompatible con Java 21 detectado: ASM antiguo en librerías [{versions}]. Actualiza la versión de {:?} para {}.",
        instance.loader, instance.minecraft_version,
    ))
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OptimizationModePayload {
    Balanced,
    MaxPerformance,
    LowPower,
}

#[derive(Debug, Deserialize)]
pub struct OptimizeInstancePayload {
    pub id: String,
    pub mode: Option<OptimizationModePayload>,
}

#[derive(Debug, Serialize)]
pub struct OptimizationReport {
    pub instance: InstanceInfo,
    pub recommended_xmx_mb: u32,
    pub recommended_xms_mb: u32,
    pub detected_mods: usize,
    pub duplicate_mods: Vec<String>,
    pub potentially_conflicting_mods: Vec<String>,
    pub missing_recommended_mods: Vec<String>,
    pub removed_logs: usize,
    pub freed_log_bytes: u64,
    pub mode: String,
    pub notes: Vec<String>,
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

#[derive(Debug, Serialize)]
pub struct JavaVersionReport {
    pub requested_minecraft_version: String,
    pub required_java_major: u32,
}

#[derive(Debug, Serialize)]
pub struct JavaRuntimeMetadataPayload {
    pub required_java_major: u32,
    pub runtime_dir: String,
    pub managed_runtime: Option<java::ManagedRuntimeInfo>,
}

#[derive(Debug, Serialize)]
pub struct RuntimeListPayload {
    pub runtimes: Vec<java::ManagedRuntimeInfo>,
}

#[derive(Debug, Serialize)]
pub struct RuntimeResolvePayload {
    pub role: java::RuntimeRole,
    pub required_java_major: u32,
    pub java_path: String,
}

#[derive(Debug, Serialize)]
pub struct RuntimeValidatePayload {
    pub role: java::RuntimeRole,
    pub path: String,
    pub required_java_major: u32,
    pub valid: bool,
}

#[derive(Debug, Serialize)]
pub struct JavaCheckReport {
    pub path: String,
    pub usable: bool,
    pub details: Option<JavaInstallation>,
}

#[derive(Debug, Deserialize)]
pub struct JavaPathPayload {
    pub path: String,
}

#[derive(Debug, Deserialize)]
pub struct MinecraftVersionPayload {
    pub minecraft_version: String,
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

    Ok(())
}

async fn validate_or_resolve_java(
    state: &crate::core::state::AppState,
    instance: &mut Instance,
) -> Result<(), LauncherError> {
    let required_major = instance
        .required_java_major
        .unwrap_or_else(|| java::required_java_for_minecraft_version(&instance.minecraft_version));

    let is_valid = |candidate: &std::path::PathBuf| {
        java::runtime::inspect_java_binary(candidate).is_some_and(|info| {
            java::is_java_compatible_major(info.major, required_major) && info.is_64bit
        })
    };

    if let Some(custom_path) = state.launcher_settings.selected_java_path.as_ref() {
        if is_valid(custom_path) {
            instance.java_path = Some(custom_path.clone());
            if !instance.loader_requires_delta {
                instance.bootstrap_runtime = RuntimeRole::Gamma;
            }
            instance.game_runtime = RuntimeRole::Gamma;
            return Ok(());
        }
    }

    match state.launcher_settings.java_runtime {
        JavaRuntimePreference::System => {
            let system_java = std::path::PathBuf::from("java");
            if is_valid(&system_java) {
                instance.java_path = Some(system_java);
                if !instance.loader_requires_delta {
                    instance.bootstrap_runtime = RuntimeRole::Gamma;
                }
                instance.game_runtime = RuntimeRole::Gamma;
                return Ok(());
            }
            return Err(LauncherError::Other(
                "Preferencia Java=System configurada pero no se encontró una Java compatible en PATH."
                    .into(),
            ));
        }
        JavaRuntimePreference::Embedded => {
            let embedded_java = state.embedded_java_path();
            if is_valid(&embedded_java) {
                instance.java_path = Some(embedded_java);
                if !instance.loader_requires_delta {
                    instance.bootstrap_runtime = RuntimeRole::Gamma;
                }
                instance.game_runtime = RuntimeRole::Gamma;
                return Ok(());
            }
        }
        JavaRuntimePreference::Auto => {}
    }

    let resolved = java::resolve_runtime_in_dir(
        &state.data_dir,
        java::RuntimeRole::Gamma,
        required_major,
        Some(&instance.minecraft_version),
    )
    .await?;
    instance.java_path = Some(resolved);
    if !instance.loader_requires_delta {
        instance.bootstrap_runtime = RuntimeRole::Gamma;
    }
    instance.game_runtime = RuntimeRole::Gamma;
    Ok(())
}

fn log_preflight_check(
    app: &tauri::AppHandle,
    instance_id: &str,
    ok: bool,
    detail: impl Into<String>,
) {
    let prefix = if ok { "✅" } else { "❌" };
    let level = if ok { "info" } else { "error" };
    emit_launch_log(
        app,
        instance_id,
        level,
        format!("[CHECK] {prefix} {}", detail.into()),
    );
}

fn collect_placeholders(arg: &str) -> Vec<String> {
    let mut placeholders = Vec::new();
    let mut rest = arg;

    while let Some(start) = rest.find("${") {
        let after_start = &rest[start + 2..];
        if let Some(end) = after_start.find('}') {
            placeholders.push(format!("${{{}}}", &after_start[..end]));
            rest = &after_start[end + 1..];
        } else {
            break;
        }
    }

    placeholders
}

fn unresolved_placeholders(args: &[String], known: &HashSet<&'static str>) -> Vec<String> {
    let mut unresolved = HashSet::new();

    for arg in args {
        for token in collect_placeholders(arg) {
            if !known.contains(token.as_str()) {
                unresolved.insert(token);
            }
        }
    }

    let mut unresolved: Vec<_> = unresolved.into_iter().collect();
    unresolved.sort();
    unresolved
}

fn verify_instance_runtime_readiness(
    app: &tauri::AppHandle,
    instance: &Instance,
    libs_dir: &Path,
) -> Result<Vec<PreflightFailure>, LauncherError> {
    let instance_id = instance.id.as_str();

    let instance_dir_ok = instance.path.is_dir();
    log_preflight_check(
        app,
        instance_id,
        instance_dir_ok,
        format!("Estructura base de instancia: {}", instance.path.display()),
    );

    let game_dir = instance.game_dir();
    let game_dir_ok = game_dir.is_dir();
    log_preflight_check(
        app,
        instance_id,
        game_dir_ok,
        format!("Carpeta minecraft disponible: {}", game_dir.display()),
    );

    let assets_dir = game_dir.join("assets");
    let assets_ok = assets_dir.is_dir();
    log_preflight_check(
        app,
        instance_id,
        assets_ok,
        format!("Carpeta assets disponible: {}", assets_dir.display()),
    );

    let client_jar = instance.path.join("client.jar");
    let client_jar_ok = client_jar.is_file();
    let client_jar_corrupted = client_jar_ok
        && fs::metadata(&client_jar)
            .map(|metadata| metadata.len() == 0)
            .unwrap_or(false);
    log_preflight_check(
        app,
        instance_id,
        client_jar_ok && !client_jar_corrupted,
        format!("Bootstrap client.jar presente: {}", client_jar.display()),
    );

    let loader_ok = matches!(instance.loader, LoaderType::Vanilla)
        || instance
            .loader_version
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty());
    log_preflight_check(
        app,
        instance_id,
        loader_ok,
        format!(
            "Loader configurado ({:?} {:?})",
            instance.loader, instance.loader_version
        ),
    );

    let main_class_ok = instance.main_class.is_some();
    log_preflight_check(
        app,
        instance_id,
        main_class_ok,
        format!("Main class resuelta: {:?}", instance.main_class),
    );

    let java_path = instance
        .java_path
        .as_ref()
        .ok_or_else(|| LauncherError::Other("No hay Java asignada a la instancia".into()))?;
    let java_exists = java_path.is_file();
    log_preflight_check(
        app,
        instance_id,
        java_exists,
        format!(
            "Binario Java inyectado en instancia: {}",
            java_path.display()
        ),
    );

    let required_major = instance
        .required_java_major
        .unwrap_or_else(|| java::required_java_for_minecraft_version(&instance.minecraft_version));

    let loader_java_compat_issue = detect_loader_asm_incompatibility(instance, required_major);
    let loader_java_ok = loader_java_compat_issue.is_none();
    log_preflight_check(
        app,
        instance_id,
        loader_java_ok,
        format!("Compatibilidad loader↔Java validada (requerida Java {required_major})"),
    );
    if let Some(issue) = loader_java_compat_issue {
        emit_launch_log(app, instance_id, "error", format!("[CHECK] {issue}"));
        emit_launch_log(
            app,
            instance_id,
            "warn",
            "[CHECK] Loader marcado como requiere Delta para fases sensibles (ASM/JAR analysis)."
                .into(),
        );
    }

    // Verificar el binario asignado directamente evita falsos negativos cuando
    // la ruta no coincide exactamente con entradas indexadas/canonizadas.
    let java_info = java::runtime::inspect_java_binary(java_path);
    let detected_java_major = java_info.as_ref().map(|candidate| candidate.major);
    let java_major_ok = detected_java_major
        .is_some_and(|major| java::is_java_compatible_major(major, required_major));
    log_preflight_check(
        app,
        instance_id,
        java_major_ok,
        format!(
            "Java compatible con Minecraft {} (requerida {}, actual {:?})",
            instance.minecraft_version, required_major, detected_java_major
        ),
    );

    let java_64_ok = java_info
        .as_ref()
        .is_some_and(|candidate| candidate.is_64bit);
    log_preflight_check(app, instance_id, java_64_ok, "Java de 64 bits validada");

    let known_jvm_placeholders = HashSet::from([
        "${natives_directory}",
        "${library_directory}",
        "${classpath}",
        "${classpath_separator}",
        "${game_directory}",
        "${version_name}",
        "${version}",
        "${mc_version}",
        "${launcher_name}",
        "${launcher_version}",
    ]);
    let known_game_placeholders = HashSet::from([
        "${auth_player_name}",
        "${version_name}",
        "${version}",
        "${mc_version}",
        "${game_directory}",
        "${assets_root}",
        "${assets_index_name}",
        "${auth_uuid}",
        "${auth_access_token}",
        "${auth_xuid}",
        "${clientid}",
        "${user_properties}",
        "${user_type}",
        "${version_type}",
        "${quickPlayMultiplayer}",
        "${quickPlaySingleplayer}",
        "${quickPlayRealms}",
        "${quickPlayPath}",
        "${resolution_width}",
        "${resolution_height}",
    ]);

    let unresolved_jvm = unresolved_placeholders(&instance.jvm_args, &known_jvm_placeholders);
    let unresolved_game = unresolved_placeholders(&instance.game_args, &known_game_placeholders);
    let args_ok = unresolved_jvm.is_empty() && unresolved_game.is_empty();
    log_preflight_check(
        app,
        instance_id,
        args_ok,
        format!(
            "Argumentos listos (JVM placeholders soportados: {}, Game placeholders soportados: {})",
            unresolved_jvm.is_empty(),
            unresolved_game.is_empty()
        ),
    );
    if !unresolved_jvm.is_empty() || !unresolved_game.is_empty() {
        emit_launch_log(
            app,
            instance_id,
            "error",
            format!(
                "[CHECK] Placeholders no reemplazados detectados -> JVM: {:?} | Game: {:?}",
                unresolved_jvm, unresolved_game
            ),
        );
    }

    let mut missing_maven_artifacts = 0usize;
    for coord in &instance.libraries {
        if let Ok(artifact) = crate::core::maven::MavenArtifact::parse(coord) {
            if !libs_dir.join(artifact.local_path()).exists() {
                missing_maven_artifacts += 1;
            }
        }
    }
    let maven_ok = missing_maven_artifacts == 0;
    log_preflight_check(
        app,
        instance_id,
        maven_ok,
        format!("Dependencias Maven listas (faltantes: {missing_maven_artifacts})"),
    );

    let external_mod_jars = fs::read_dir(instance.mods_dir())
        .ok()
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.path().extension() == Some(OsStr::new("jar")))
                .count()
        })
        .unwrap_or(0);
    log_preflight_check(
        app,
        instance_id,
        true,
        format!("JARs extra en mods detectados: {external_mod_jars}"),
    );

    let mut failures = Vec::new();

    if !java_exists {
        failures.push(PreflightFailure::MissingJava);
    }
    if java_exists && (!java_major_ok || !java_64_ok) {
        failures.push(PreflightFailure::WrongJavaVersion);
    }
    if !instance_dir_ok || !game_dir_ok || !assets_ok || !client_jar_ok {
        failures.push(PreflightFailure::MissingStructure);
    }
    if !maven_ok {
        failures.push(PreflightFailure::MissingLibraries);
    }
    if client_jar_corrupted {
        failures.push(PreflightFailure::CorruptedFiles);
    }
    if !loader_ok || !main_class_ok {
        failures.push(PreflightFailure::InvalidLoader);
    }
    if !loader_java_ok {
        failures.push(PreflightFailure::IncompatibleLoaderJava);
    }
    if !args_ok {
        failures.push(PreflightFailure::Unknown);
    }

    Ok(failures)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreflightFailure {
    MissingJava,
    WrongJavaVersion,
    MissingStructure,
    MissingLibraries,
    CorruptedFiles,
    InvalidLoader,
    IncompatibleLoaderJava,
    Unknown,
}

impl PreflightFailure {
    fn label(self) -> &'static str {
        match self {
            PreflightFailure::MissingJava => "MissingJava",
            PreflightFailure::WrongJavaVersion => "WrongJavaVersion",
            PreflightFailure::MissingStructure => "MissingStructure",
            PreflightFailure::MissingLibraries => "MissingLibraries",
            PreflightFailure::CorruptedFiles => "CorruptedFiles",
            PreflightFailure::InvalidLoader => "InvalidLoader",
            PreflightFailure::IncompatibleLoaderJava => "IncompatibleLoaderJava",
            PreflightFailure::Unknown => "Unknown",
        }
    }
}

fn has_loader_java_incompatibility(failures: &[PreflightFailure]) -> bool {
    failures
        .iter()
        .any(|failure| matches!(failure, PreflightFailure::IncompatibleLoaderJava))
}

fn user_forced_gamma_only(settings: &LauncherSettings, _instance: &Instance) -> bool {
    settings
        .selected_java_path
        .as_ref()
        .and_then(|path| path.to_str())
        .is_some_and(|path| path.to_ascii_lowercase().contains("java-gamma"))
}

async fn attempt_preflight_repair(
    app: &tauri::AppHandle,
    state: &crate::core::state::AppState,
    instance: &mut Instance,
    failures: &[PreflightFailure],
) -> Result<(), LauncherError> {
    let labels = failures
        .iter()
        .map(|failure| failure.label())
        .collect::<Vec<_>>()
        .join(", ");
    emit_launch_log(
        app,
        &instance.id,
        "warn",
        format!("[REPAIR] Preflight falló. Clasificación detectada: {labels}"),
    );

    let mut needs_prepare = false;
    let mut force_full_prepare = false;

    for failure in failures {
        match failure {
            PreflightFailure::MissingJava | PreflightFailure::WrongJavaVersion => {
                emit_launch_log(
                    app,
                    &instance.id,
                    "info",
                    "[REPAIR] Resolviendo runtime de Java administrado compatible.".into(),
                );
                validate_or_resolve_java(state, instance).await?;
            }
            PreflightFailure::MissingStructure | PreflightFailure::MissingLibraries => {
                needs_prepare = true;
            }
            PreflightFailure::CorruptedFiles => {
                let client_jar = instance.path.join("client.jar");
                if client_jar.exists() {
                    let _ = tokio::fs::remove_file(&client_jar).await;
                }
                force_full_prepare = true;
            }
            PreflightFailure::InvalidLoader => {
                force_full_prepare = true;
            }
            PreflightFailure::IncompatibleLoaderJava => {
                emit_launch_log(
                    app,
                    &instance.id,
                    "info",
                    "[REPAIR] El loader requiere Java de herramientas distinto. Se ajustó automáticamente.".into(),
                );
                instance.loader_requires_delta = true;
                instance.bootstrap_runtime = RuntimeRole::Delta;
                instance.game_runtime = RuntimeRole::Gamma;
                let delta_runtime = java::resolve_runtime_in_dir(
                    &state.data_dir,
                    RuntimeRole::Delta,
                    RuntimeRole::Delta.expected_major(Some(&instance.minecraft_version)),
                    Some(&instance.minecraft_version),
                )
                .await?;
                emit_launch_log(
                    app,
                    &instance.id,
                    "info",
                    format!(
                        "[REPAIR] Reasignando runtime de fase: bootstrap=delta ({}) | game=gamma.",
                        delta_runtime.display()
                    ),
                );
            }
            PreflightFailure::Unknown => {
                needs_prepare = true;
            }
        }
    }

    if force_full_prepare {
        instance.main_class = None;
        needs_prepare = true;
    }

    if needs_prepare {
        emit_launch_log(
            app,
            &instance.id,
            "info",
            "[REPAIR] Reasignando runtime de fase y reintentando solo la fase fallida.".into(),
        );
        prepare_instance_for_launch(state, instance).await?;
    }

    Ok(())
}

async fn run_bootstrap_runtime_probe(
    app: &tauri::AppHandle,
    state: &crate::core::state::AppState,
    instance: &Instance,
) -> Result<(), LauncherError> {
    let runtime_role = instance.bootstrap_runtime;
    let runtime_path = match runtime_role {
        RuntimeRole::Gamma => instance.java_path.clone().ok_or_else(|| {
            LauncherError::Other("No hay Java Gamma asignada a la instancia".into())
        })?,
        RuntimeRole::Delta => {
            java::resolve_runtime_in_dir(
                &state.data_dir,
                RuntimeRole::Delta,
                RuntimeRole::Delta.expected_major(Some(&instance.minecraft_version)),
                Some(&instance.minecraft_version),
            )
            .await?
        }
    };

    let java_home = runtime_path
        .parent()
        .and_then(|bin| bin.parent())
        .ok_or_else(|| {
            LauncherError::Other("No se pudo resolver JAVA_HOME para bootstrap".into())
        })?;

    emit_launch_log(
        app,
        &instance.id,
        "info",
        format!(
            "[BOOTSTRAP] Runtime de fase asignado: {:?} | binario: {} | JAVA_HOME: {}",
            runtime_role,
            runtime_path.display(),
            java_home.display()
        ),
    );

    let output = Command::new(&runtime_path)
        .arg("-version")
        .env("JAVA_HOME", java_home)
        .output()
        .map_err(|source| LauncherError::Io {
            path: runtime_path.clone(),
            source,
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(LauncherError::Other(format!(
            "Runtime de bootstrap inválido ({:?}): {}",
            runtime_role, stderr
        )));
    }

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

    validate_or_resolve_java(state, instance).await?;
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
        .filter(|entry| entry.version_type == "release")
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

fn is_neoforge_compatible(version: &str, minecraft_version: &str) -> bool {
    let mut mc_parts = minecraft_version
        .trim_start_matches("1.")
        .split('.')
        .filter_map(|part| part.parse::<u64>().ok());

    let Some(mc_major) = mc_parts.next() else {
        return false;
    };
    let Some(mc_minor) = mc_parts.next() else {
        return false;
    };

    let mut loader_parts = version
        .split('.')
        .filter_map(|part| part.parse::<u64>().ok());
    let Some(loader_major) = loader_parts.next() else {
        return false;
    };
    let Some(loader_minor) = loader_parts.next() else {
        return false;
    };

    loader_major == mc_major && loader_minor == mc_minor
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
                #[serde(default)]
                stable: bool,
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
                .filter(|entry| entry.loader.stable)
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

            let mut resolved: Vec<String> = metadata
                .versioning
                .versions
                .version
                .into_iter()
                .filter(|v| is_neoforge_compatible(v, &minecraft_version))
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

#[cfg(test)]
mod tests {
    use super::{
        asm_version_supports_java_21, detect_loader_asm_incompatibility, is_neoforge_compatible,
        parse_numeric_version_parts,
    };
    use crate::core::instance::{Instance, LoaderType};

    #[test]
    fn neoforge_compatibility_matches_same_minor_line() {
        assert!(is_neoforge_compatible("21.1.127", "1.21.1"));
        assert!(is_neoforge_compatible("20.6.95-beta", "1.20.6"));
    }

    #[test]
    fn neoforge_compatibility_rejects_other_minor_lines() {
        assert!(!is_neoforge_compatible("21.10.4", "1.21.1"));
        assert!(!is_neoforge_compatible("20.5.12", "1.20.1"));
        assert!(!is_neoforge_compatible("invalid", "1.20.1"));
    }

    #[test]
    fn asm_java21_threshold_requires_9_7_or_newer() {
        assert!(asm_version_supports_java_21("9.7"));
        assert!(asm_version_supports_java_21("9.7.1"));
        assert!(!asm_version_supports_java_21("9.6"));
        assert!(!asm_version_supports_java_21("9.6.1"));
    }

    #[test]
    fn parse_numeric_version_parts_ignores_suffixes() {
        assert_eq!(parse_numeric_version_parts("9.7"), vec![9, 7]);
        assert_eq!(parse_numeric_version_parts("9.6.1-neoforge"), vec![9, 6, 1]);
    }

    #[test]
    fn detects_loader_asm_issue_for_java21_modded_instance() {
        let mut instance = Instance::new(
            "Test".into(),
            "1.20.6".into(),
            LoaderType::NeoForge,
            Some("20.6.139".into()),
            2048,
            std::path::Path::new("/tmp"),
        );
        instance.libraries = vec![
            "org.ow2.asm:asm:9.6".into(),
            "org.ow2.asm:asm-tree:9.6".into(),
        ];

        let issue = detect_loader_asm_incompatibility(&instance, 21);
        assert!(issue.is_some());
    }
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

        validate_or_resolve_java(&state, &mut instance).await?;
        if let Some(java_path) = &instance.java_path {
            emit_create_log(
                &app,
                &instance.id,
                "info",
                format!(
                    "✅ Java seleccionada automáticamente para {}: {}",
                    instance.minecraft_version,
                    java_path.display()
                ),
            );
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
        emit_launch_log(&app_handle, &id, "info", "[FASE] preparación".into());

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
        emit_launch_log(&app_handle, &id, "info", "[FASE] bootstrap".into());

        let libs_dir = state_guard.libraries_dir();
        emit_launch_log(
            &app_handle,
            &id,
            "info",
            "[PREPARACIÓN] Ejecutando checklist preflight (estructura, Java, args, Maven, loader, bootstrap).".into(),
        );
        let mut preflight_failures =
            verify_instance_runtime_readiness(&app_handle, &instance, &libs_dir)?;
        if !preflight_failures.is_empty() {
            if has_loader_java_incompatibility(&preflight_failures)
                && user_forced_gamma_only(&state_guard.launcher_settings, &instance)
            {
                let err = LauncherError::Other(
                    "El loader requiere Delta para bootstrap, pero el usuario forzó solo Gamma."
                        .into(),
                );
                emit_launch_progress(&app_handle, &id, 100, "Preflight fallido", "error");
                emit_launch_log(&app_handle, &id, "error", format!("[ERROR] {err}"));
                instance.state = InstanceState::Error;
                state_guard.instance_manager.save(&instance).await?;
                return Err(err);
            }

            if has_loader_java_incompatibility(&preflight_failures) {
                emit_launch_log(
                    &app_handle,
                    &id,
                    "warn",
                    "[PREPARACIÓN] Preflight detectó incompatibilidad loader↔Java; autoreparación por reintento desactivada (0 intentos).".into(),
                );
                attempt_preflight_repair(
                    &app_handle,
                    &state_guard,
                    &mut instance,
                    &preflight_failures,
                )
                .await?;
                preflight_failures =
                    verify_instance_runtime_readiness(&app_handle, &instance, &libs_dir)?;
            } else {
                emit_launch_log(
                    &app_handle,
                    &id,
                    "warn",
                    "[PREPARACIÓN] Preflight con fallos transitorios: se iniciará autoreparación (máx. 2 intentos)."
                        .into(),
                );

                let mut repaired = false;
                for attempt in 1..=2 {
                    emit_launch_log(
                        &app_handle,
                        &id,
                        "info",
                        format!("[REPAIR] Intento automático {attempt}/2."),
                    );
                    attempt_preflight_repair(
                        &app_handle,
                        &state_guard,
                        &mut instance,
                        &preflight_failures,
                    )
                    .await?;
                    preflight_failures =
                        verify_instance_runtime_readiness(&app_handle, &instance, &libs_dir)?;
                    if preflight_failures.is_empty() {
                        repaired = true;
                        emit_launch_log(
                            &app_handle,
                            &id,
                            "info",
                            "[REPAIR] Instancia reparada y validada correctamente.".into(),
                        );
                        break;
                    }
                }

                if !repaired && !preflight_failures.is_empty() {
                    let labels = preflight_failures
                        .iter()
                        .map(|failure| failure.label())
                        .collect::<Vec<_>>()
                        .join(", ");
                    let err = LauncherError::Other(format!(
                        "Preflight fallido tras autoreparación. Fallos persistentes: {labels}"
                    ));
                    emit_launch_progress(&app_handle, &id, 100, "Preflight fallido", "error");
                    emit_launch_log(&app_handle, &id, "error", format!("[ERROR] {err}"));
                    instance.state = InstanceState::Error;
                    state_guard.instance_manager.save(&instance).await?;
                    return Err(err);
                }
            }

            if !preflight_failures.is_empty() {
                let labels = preflight_failures
                    .iter()
                    .map(|failure| failure.label())
                    .collect::<Vec<_>>()
                    .join(", ");
                let err = LauncherError::Other(format!(
                    "Preflight fallido. Fallos persistentes: {labels}"
                ));
                emit_launch_progress(&app_handle, &id, 100, "Preflight fallido", "error");
                emit_launch_log(&app_handle, &id, "error", format!("[ERROR] {err}"));
                instance.state = InstanceState::Error;
                state_guard.instance_manager.save(&instance).await?;
                return Err(err);
            }

            state_guard.instance_manager.save(&instance).await?;
        }

        run_bootstrap_runtime_probe(&app_handle, &state_guard, &instance).await?;

        let classpath = launch::build_classpath(&instance, &libs_dir, &instance.libraries)?;
        emit_launch_log(&app_handle, &id, "info", "[FASE] análisis de jars".into());
        let _natives_dir =
            launch::extract_natives(&instance, &libs_dir, &instance.libraries).await?;

        emit_launch_progress(
            &app_handle,
            &id,
            90,
            "Lanzando proceso de Minecraft",
            "running",
        );
        emit_launch_log(&app_handle, &id, "info", "[FASE] launch del juego".into());

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
                let mut asm_hint_emitted = false;
                for line in StdBufReader::new(stderr).lines().map_while(Result::ok) {
                    emit_launch_log(&app_handle, &instance_id, "warn", line.clone());
                    if let Some(diagnostic) = detect_launch_diagnostic(&line) {
                        let should_emit = match diagnostic {
                            LaunchDiagnostic::NeoForgeEarlyDisplayRendererFuture
                            | LaunchDiagnostic::NeoForgeEarlyDisplayStillEnabled => {
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
                            LaunchDiagnostic::LoaderAsmTooOldForJava21 => {
                                if asm_hint_emitted {
                                    false
                                } else {
                                    asm_hint_emitted = true;
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

fn clamp_memory_to_safe_bounds(
    total_mb: u64,
    available_mb: u64,
    suggested_mb: u32,
) -> (u32, Vec<String>) {
    let mut notes = Vec::new();
    let hard_cap_by_total = ((total_mb as f64) * 0.60).floor() as u32;
    let available_cap = available_mb.saturating_sub(if total_mb >= 32 * 1024 {
        6144
    } else if total_mb >= 16 * 1024 {
        4096
    } else {
        3072
    }) as u32;
    let mut cap = hard_cap_by_total.min(available_cap.max(2048));
    cap = cap.max(2048);

    let mut final_mb = suggested_mb.max(2048);
    if final_mb > cap {
        final_mb = cap;
        notes.push(
            "Ajustamos la RAM para evitar inestabilidad del sistema (límite dinámico aplicado)."
                .into(),
        );
    }

    (final_mb, notes)
}

fn recommended_memory_for_mod_count(mod_count: usize, mode: &OptimizationModePayload) -> u32 {
    let base = if mod_count <= 50 {
        5120
    } else if mod_count <= 150 {
        7168
    } else {
        10240
    };

    match mode {
        OptimizationModePayload::Balanced => base,
        OptimizationModePayload::MaxPerformance => base.saturating_add(1024),
        OptimizationModePayload::LowPower => base.saturating_sub(1024).max(4096),
    }
}

fn normalize_mod_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_lowercase()
}

fn collect_mod_analysis(
    instance: &Instance,
) -> (usize, Vec<String>, Vec<String>, Vec<String>, Vec<String>) {
    let mut mod_count = 0usize;
    let mut seen = HashMap::<String, usize>::new();
    let mut duplicates = Vec::new();
    let mut conflict_hits = Vec::new();
    let mut notes = Vec::new();

    let mods_dir = instance.mods_dir();
    if let Ok(entries) = fs::read_dir(&mods_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let is_jar = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("jar"))
                .unwrap_or(false);
            if !is_jar {
                continue;
            }

            mod_count += 1;
            let normalized = normalize_mod_name(&path);
            if normalized.is_empty() {
                continue;
            }

            let key = normalized
                .split(['-', '_'])
                .next()
                .unwrap_or(&normalized)
                .to_string();
            let counter = seen.entry(key.clone()).or_insert(0);
            *counter += 1;
            if *counter == 2 {
                duplicates.push(key.clone());
            }

            if normalized.contains("optifine") {
                conflict_hits.push("OptiFine puede generar conflictos en packs modernos (usa Sodium/Embeddium según loader).".into());
            }
            if normalized.contains("rubidium") && instance.loader == LoaderType::Fabric {
                conflict_hits
                    .push("Rubidium no es para Fabric; revisa compatibilidad del loader.".into());
            }
            if normalized.contains("sodium") && instance.loader == LoaderType::Forge {
                conflict_hits.push(
                    "Sodium en Forge suele indicar mod incorrecto; usa Embeddium/Rubidium.".into(),
                );
            }
        }
    } else {
        notes.push("No se pudo leer la carpeta de mods para análisis automático.".into());
    }

    let mod_names: HashSet<String> = seen.keys().cloned().collect();
    let mut missing = Vec::new();
    let recommendations = ["sodium", "lithium", "ferritecore"];
    for item in recommendations {
        if !mod_names.contains(item) {
            missing.push(item.to_string());
        }
    }

    (mod_count, duplicates, conflict_hits, missing, notes)
}

fn clean_old_logs(instance: &Instance) -> (usize, u64) {
    let mut removed = 0usize;
    let mut freed = 0u64;
    let logs_dir = instance.game_dir().join("logs");

    if let Ok(entries) = fs::read_dir(logs_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let is_log = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("log") || e.eq_ignore_ascii_case("gz"))
                .unwrap_or(false);
            if !is_log {
                continue;
            }

            if let Ok(meta) = fs::metadata(&path) {
                freed = freed.saturating_add(meta.len());
            }

            if fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
    }

    (removed, freed)
}

fn optimized_jvm_args(java_major: u32, mode: &OptimizationModePayload) -> Vec<String> {
    let mut args = vec![
        "-XX:+UseG1GC".to_string(),
        "-XX:+UnlockExperimentalVMOptions".to_string(),
        "-XX:G1NewSizePercent=20".to_string(),
        "-XX:G1MaxNewSizePercent=60".to_string(),
        "-XX:MaxGCPauseMillis=50".to_string(),
        "-XX:G1HeapRegionSize=16M".to_string(),
        "-XX:+AlwaysPreTouch".to_string(),
    ];

    if java_major < 17 {
        args.retain(|item| item != "-XX:+UnlockExperimentalVMOptions");
    }

    match mode {
        OptimizationModePayload::Balanced => {}
        OptimizationModePayload::MaxPerformance => {
            args.push("-XX:InitiatingHeapOccupancyPercent=15".into())
        }
        OptimizationModePayload::LowPower => args.push("-XX:MaxGCPauseMillis=80".into()),
    }

    args
}

#[tauri::command]
pub async fn optimize_instance_with_real_process(
    state: tauri::State<'_, Arc<Mutex<AppState>>>,
    payload: OptimizeInstancePayload,
) -> Result<OptimizationReport, LauncherError> {
    let state = state.lock().await;
    let mut instance = state.instance_manager.load(&payload.id).await?;
    let mode = payload.mode.unwrap_or(OptimizationModePayload::Balanced);

    let mut system = System::new_all();
    system.refresh_memory();
    let total_mb = system.total_memory() / (1024 * 1024);
    let available_mb = system.available_memory() / (1024 * 1024);

    let (
        detected_mods,
        duplicate_mods,
        potentially_conflicting_mods,
        missing_recommended_mods,
        mut notes,
    ) = collect_mod_analysis(&instance);

    let raw_suggested_mb = recommended_memory_for_mod_count(detected_mods, &mode);
    let (recommended_xmx_mb, mut clamp_notes) =
        clamp_memory_to_safe_bounds(total_mb, available_mb, raw_suggested_mb);
    notes.append(&mut clamp_notes);

    let recommended_xms_mb = (recommended_xmx_mb / 2).max(1024);

    let java_major = instance
        .required_java_major
        .unwrap_or_else(|| java::required_java_for_minecraft_version(&instance.minecraft_version));
    let mut merged_jvm_args = instance.jvm_args.clone();
    merged_jvm_args.extend(optimized_jvm_args(java_major, &mode));
    merged_jvm_args = merged_jvm_args
        .into_iter()
        .filter(|arg| {
            !arg.trim().is_empty() && !arg.starts_with("-Xmx") && !arg.starts_with("-Xms")
        })
        .collect::<Vec<_>>();
    merged_jvm_args.sort();
    merged_jvm_args.dedup();

    instance.max_memory_mb = recommended_xmx_mb;
    instance.jvm_args = merged_jvm_args;

    let (removed_logs, freed_log_bytes) = clean_old_logs(&instance);
    if removed_logs > 0 {
        notes.push(format!(
            "Se limpiaron {removed_logs} logs antiguos para reducir carga de disco."
        ));
    }

    state.instance_manager.save(&instance).await?;

    Ok(OptimizationReport {
        instance: InstanceInfo::from(&instance),
        recommended_xmx_mb,
        recommended_xms_mb,
        detected_mods,
        duplicate_mods,
        potentially_conflicting_mods,
        missing_recommended_mods,
        removed_logs,
        freed_log_bytes,
        mode: match mode {
            OptimizationModePayload::Balanced => "balanced".into(),
            OptimizationModePayload::MaxPerformance => "max_performance".into(),
            OptimizationModePayload::LowPower => "low_power".into(),
        },
        notes,
    })
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

        let file_type = entry.file_type().map_err(|source_err| LauncherError::Io {
            path: src_path.clone(),
            source: source_err,
        })?;

        if file_type.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else if file_type.is_symlink() {
            let target = fs::read_link(&src_path).map_err(|source_err| LauncherError::Io {
                path: src_path.clone(),
                source: source_err,
            })?;

            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(&target, &dst_path).map_err(|source_err| {
                    LauncherError::Io {
                        path: dst_path,
                        source: source_err,
                    }
                })?;
            }

            #[cfg(windows)]
            {
                let resolved_target = if target.is_absolute() {
                    target.clone()
                } else {
                    src_path.parent().unwrap_or(source).join(&target)
                };

                if resolved_target.is_dir() {
                    std::os::windows::fs::symlink_dir(&target, &dst_path).map_err(
                        |source_err| LauncherError::Io {
                            path: dst_path,
                            source: source_err,
                        },
                    )?;
                } else {
                    std::os::windows::fs::symlink_file(&target, &dst_path).map_err(
                        |source_err| LauncherError::Io {
                            path: dst_path,
                            source: source_err,
                        },
                    )?;
                }
            }
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
pub async fn get_java_metadata(
    state: tauri::State<'_, Arc<Mutex<AppState>>>,
    payload: MinecraftVersionPayload,
) -> Result<JavaRuntimeMetadataPayload, LauncherError> {
    let state = state.lock().await;
    let required_java_major = java::required_java_for_minecraft_version(&payload.minecraft_version);
    let runtime_dir = java::managed_runtime_dir(&state.data_dir, required_java_major);
    let managed_runtime =
        java::managed_runtime_info_in_dir(&state.data_dir, required_java_major).await?;

    Ok(JavaRuntimeMetadataPayload {
        required_java_major,
        runtime_dir: runtime_dir.to_string_lossy().to_string(),
        managed_runtime,
    })
}

#[tauri::command]
pub async fn get_required_java_version(
    payload: MinecraftVersionPayload,
) -> Result<JavaVersionReport, LauncherError> {
    let required_java_major = java::required_java_for_minecraft_version(&payload.minecraft_version);
    Ok(JavaVersionReport {
        requested_minecraft_version: payload.minecraft_version,
        required_java_major,
    })
}

#[tauri::command]
pub async fn install_managed_java(
    state: tauri::State<'_, Arc<Mutex<AppState>>>,
    payload: MinecraftVersionPayload,
) -> Result<JavaCheckReport, LauncherError> {
    let state = state.lock().await;
    let required_java_major = java::required_java_for_minecraft_version(&payload.minecraft_version);
    let java_path = java::resolve_java_binary_in_dir(&state.data_dir, required_java_major).await?;
    let details = java::runtime::inspect_java_binary(&java_path);

    Ok(JavaCheckReport {
        path: java_path.to_string_lossy().to_string(),
        usable: details
            .as_ref()
            .is_some_and(|info| java::is_java_compatible_major(info.major, required_java_major)),
        details,
    })
}

#[tauri::command]
pub async fn get_java_info(
    payload: JavaPathPayload,
) -> Result<Option<JavaInstallation>, LauncherError> {
    let path = std::path::PathBuf::from(&payload.path);
    Ok(java::runtime::inspect_java_binary(&path))
}

#[tauri::command]
pub async fn check_java_binary(payload: JavaPathPayload) -> Result<JavaCheckReport, LauncherError> {
    let path = std::path::PathBuf::from(&payload.path);
    let details = java::runtime::inspect_java_binary(&path);

    Ok(JavaCheckReport {
        path: payload.path,
        usable: details.is_some(),
        details,
    })
}

#[tauri::command]
pub async fn list_runtimes(
    state: tauri::State<'_, Arc<Mutex<AppState>>>,
) -> Result<RuntimeListPayload, LauncherError> {
    let state = state.lock().await;
    let manager = java::runtime::RuntimeManager::from_global_paths()?;
    let runtimes = manager.list_runtimes().await?;
    let _ = &state;
    Ok(RuntimeListPayload { runtimes })
}

#[tauri::command]
pub async fn resolve_java(
    required_java_major: u32,
) -> Result<RuntimeResolvePayload, LauncherError> {
    let manager = java::runtime::RuntimeManager::from_global_paths()?;
    let java_path = manager.resolve_java(required_java_major).await?;
    Ok(RuntimeResolvePayload {
        role: java::RuntimeRole::Gamma,
        required_java_major,
        java_path: java_path.to_string_lossy().to_string(),
    })
}

#[tauri::command]
pub async fn validate_java(
    payload: JavaPathPayload,
    required_java_major: u32,
) -> Result<RuntimeValidatePayload, LauncherError> {
    let manager = java::runtime::RuntimeManager::from_global_paths()?;
    let path = std::path::PathBuf::from(&payload.path);
    let canonical = std::fs::canonicalize(&path).unwrap_or(path);
    let valid = manager.validate_java(&canonical, required_java_major);
    Ok(RuntimeValidatePayload {
        role: java::RuntimeRole::Gamma,
        path: canonical.to_string_lossy().to_string(),
        required_java_major,
        valid,
    })
}

#[tauri::command]
pub async fn clear_runtimes() -> Result<bool, LauncherError> {
    let manager = java::runtime::RuntimeManager::from_global_paths()?;
    manager.clear_runtimes().await?;
    Ok(true)
}

#[tauri::command]
pub async fn runtime_diagnostic() -> Result<java::RuntimeDiagnostic, LauncherError> {
    let manager = java::runtime::RuntimeManager::from_global_paths()?;
    manager.diagnostics().await
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
    state.launcher_settings.selected_java_path = if let Some(custom) =
        payload.selected_java_path.as_ref()
    {
        let candidate = std::path::PathBuf::from(custom);
        let canonical = std::fs::canonicalize(&candidate).map_err(|source| LauncherError::Io {
            path: candidate.clone(),
            source,
        })?;
        if crate::core::java::runtime::inspect_java_binary(&canonical).is_none() {
            return Err(LauncherError::Other(format!(
                "Ruta Java inválida para override manual: {}",
                canonical.display()
            )));
        }
        Some(canonical)
    } else {
        None
    };

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
