use std::collections::{BTreeSet, HashMap};
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use tracing::{info, warn};

use super::context::InstallContext;
use super::installer::{LoaderInstallResult, LoaderInstaller};
use crate::core::downloader::Downloader;
use crate::core::error::{LauncherError, LauncherResult};
use crate::core::maven::MavenArtifact;
use crate::core::version::VersionJson;

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

        // NeoForge installer naming differs by era:
        // - Modern (MC 1.21+): net.neoforged:neoforge:<ver>:installer  => neoforge-<ver>-installer.jar
        // - Legacy (MC 1.20.1): net.neoforged:forge:<mc>-<ver>:installer => forge-<mc>-<ver>-installer.jar
        // Some configs provide ctx.loader_version without the mc prefix (e.g. "47.1.82").
        let installer_path = ctx
            .instance_dir
            .join(format!("neoforge-{}-installer.jar", ctx.loader_version));

        let mc_prefixed = format!("{}-{}", ctx.minecraft_version, ctx.loader_version);
        let candidates = [
            // Modern
            (
                format!("neoforge-{}-installer.jar", ctx.loader_version),
                format!(
                    "{}/net/neoforged/neoforge/{}/neoforge-{}-installer.jar",
                    NEOFORGE_MAVEN, ctx.loader_version, ctx.loader_version
                ),
            ),
            // Legacy (no mc prefix)
            (
                format!("forge-{}-installer.jar", ctx.loader_version),
                format!(
                    "{}/net/neoforged/forge/{}/forge-{}-installer.jar",
                    NEOFORGE_MAVEN, ctx.loader_version, ctx.loader_version
                ),
            ),
            // Legacy (mc-prefixed version id)
            (
                format!("forge-{}-installer.jar", mc_prefixed),
                format!(
                    "{}/net/neoforged/forge/{}/forge-{}-installer.jar",
                    NEOFORGE_MAVEN, mc_prefixed, mc_prefixed
                ),
            ),
            // Rare: neoforge artifact but mc-prefixed version id
            (
                format!("neoforge-{}-installer.jar", mc_prefixed),
                format!(
                    "{}/net/neoforged/neoforge/{}/neoforge-{}-installer.jar",
                    NEOFORGE_MAVEN, mc_prefixed, mc_prefixed
                ),
            ),
        ];

        let mut last_err: Option<LauncherError> = None;
        let mut downloaded = false;

        for (name, url) in candidates {
            info!("Trying NeoForge installer: {}", url);
            let dest = ctx.instance_dir.join(&name);
            match download_with_archive_validation(ctx.downloader, &url, &dest).await {
                Ok(()) => {
                    // Normalize to installer_path for the rest of the pipeline.
                    if dest != installer_path {
                        let _ = tokio::fs::remove_file(&installer_path).await;
                        let _ = tokio::fs::rename(&dest, &installer_path).await;
                    }
                    downloaded = true;
                    break;
                }
                Err(e) => last_err = Some(e),
            }
        }

        if !downloaded {
            return Err(last_err.unwrap_or_else(|| {
                LauncherError::Loader("No valid NeoForge installer URL found".into())
            }));
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

        let java_bin = crate::core::java::resolve_runtime(
            crate::core::java::RuntimeRole::Delta,
            Some(ctx.minecraft_version),
        )
        .await?;
        log_runtime_role("Delta", &java_bin, ctx.instance_dir);

        // Download libraries from install_profile
        let mut libraries = BTreeSet::new();
        for lib in &install_profile.libraries {
            libraries.insert(lib.name.clone());
            let artifact = MavenArtifact::parse(&lib.name)?;
            let dest = ctx.libs_dir.join(artifact.local_path());
            if should_download_or_replace_archive(&dest) {
                let _ = tokio::fs::remove_file(&dest).await;
                let url = artifact.url(NEOFORGE_MAVEN);
                if let Err(e) = download_with_archive_validation(ctx.downloader, &url, &dest).await
                {
                    // Fallback to Mojang libs
                    let mojang_url = artifact.url(crate::core::maven::MOJANG_LIBRARIES);
                    if let Err(_) =
                        download_with_archive_validation(ctx.downloader, &mojang_url, &dest).await
                    {
                        warn!("Failed to download NeoForge lib {}: {}", lib.name, e);
                    }
                }
            }
        }

        // Download libraries from version.json
        for lib in &version_json.libraries {
            libraries.insert(lib.name.clone());
            let artifact = MavenArtifact::parse(&lib.name)?;
            let dest = ctx.libs_dir.join(artifact.local_path());
            if should_download_or_replace_archive(&dest) {
                let _ = tokio::fs::remove_file(&dest).await;
                let url = artifact.url(NEOFORGE_MAVEN);
                let _ = download_with_archive_validation(ctx.downloader, &url, &dest).await;
            }
        }

        let mut processor_vars = HashMap::new();
        merge_profile_data_variables(&mut processor_vars, &install_profile.data);
        merge_runtime_processor_variables(
            &mut processor_vars,
            &build_processor_variables(&ctx, &installer_path, &installer_bytes)?,
        );

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

            let resolved_args: Vec<String> = processor
                .args
                .iter()
                .map(|arg| resolve_processor_arg(arg, &processor_vars, ctx.libs_dir))
                .collect::<LauncherResult<Vec<_>>>()?;

            let main_class = read_main_class_from_jar(&jar_path)
                .unwrap_or_else(|_| "net.minecraftforge.installertools.ConsoleTool".to_string());

            let java_home = java_bin
                .parent()
                .and_then(|bin| bin.parent())
                .unwrap_or(ctx.instance_dir);
            let output = std::process::Command::new(&java_bin)
                .env("JAVA_HOME", java_home)
                .arg("-cp")
                .arg(&classpath)
                .arg(&main_class)
                .args(&resolved_args)
                .current_dir(ctx.instance_dir)
                .output()
                .map_err(|e| LauncherError::JavaExecution(e.to_string()))?;

            if !output.status.success() {
                return Err(LauncherError::Loader(format!(
                    "NeoForge processor {} failed (code {:?})\nSTDOUT:\n{}\nSTDERR:\n{}",
                    processor.jar,
                    output.status.code(),
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                )));
            }

            // `processor.classpath` y `processor.jar` se usan solo durante instalación.
            // Añadirlos a librerías runtime contamina el classpath final con tooling
            // (binarypatcher, jarsplitter, etc.) y puede romper el bootstrap.
        }

        let mut extra_jvm_args = Vec::new();
        let mut extra_game_args = Vec::new();
        let mut resolved_main_class = version_json.main_class.clone();

        let installed_version_path = resolve_installed_neoforge_version_path(&ctx);
        if installed_version_path.exists() {
            let raw_version = tokio::fs::read_to_string(&installed_version_path)
                .await
                .map_err(|e| LauncherError::Io {
                    path: installed_version_path.clone(),
                    source: e,
                })?;
            let installed_version = resolve_version_with_inheritance(
                &raw_version,
                installed_version_path.parent().unwrap_or(ctx.instance_dir),
            )?;

            resolved_main_class = installed_version.main_class.clone();
            extra_jvm_args = installed_version.simple_jvm_args();
            extra_game_args = installed_version.simple_game_args();

            for lib in installed_version
                .download_libraries(ctx.libs_dir, ctx.downloader)
                .await?
            {
                libraries.insert(lib);
            }
        }

        // If we couldn't resolve a loader main class (or inherited Vanilla), force the
        // modern NeoForge bootstrap entrypoint so ModLauncher targets are honored.
        if resolved_main_class.trim().is_empty()
            || resolved_main_class.as_str() == "net.minecraft.client.main.Main"
        {
            resolved_main_class = "cpw.mods.bootstraplauncher.BootstrapLauncher".to_string();
        }

        for lib in &libraries {
            let Ok(artifact) = MavenArtifact::parse(lib) else {
                continue;
            };
            let dest = ctx.libs_dir.join(artifact.local_path());
            if should_download_or_replace_archive(&dest) {
                let _ = tokio::fs::remove_file(&dest).await;
                let primary = artifact.url(NEOFORGE_MAVEN);
                if download_with_archive_validation(ctx.downloader, &primary, &dest)
                    .await
                    .is_err()
                {
                    let fallback = artifact.url(crate::core::maven::MOJANG_LIBRARIES);
                    let _ =
                        download_with_archive_validation(ctx.downloader, &fallback, &dest).await;
                }
            }
        }

        let _ = tokio::fs::remove_file(&installer_path).await;

        info!("NeoForge {} installed successfully", ctx.loader_version);

        Ok(LoaderInstallResult {
            main_class: resolved_main_class,
            extra_jvm_args,
            extra_game_args,
            libraries: libraries.into_iter().collect(),
            asset_index_id: None,
            asset_index_url: None,
            java_major: Some(crate::core::java::required_java_for_minecraft_version(
                ctx.minecraft_version,
            )),
        })
    }
}

fn log_runtime_role(role: &str, java_bin: &Path, cwd: &Path) {
    let version = std::process::Command::new(java_bin)
        .arg("-version")
        .current_dir(cwd)
        .output()
        .ok()
        .map(|out| {
            let stderr = String::from_utf8_lossy(&out.stderr);
            stderr
                .lines()
                .next()
                .unwrap_or("java -version unavailable")
                .to_string()
        })
        .unwrap_or_else(|| "java -version unavailable".to_string());
    info!(
        "[RUNTIME] role={} java_bin={} version={} cwd={}",
        role,
        java_bin.display(),
        version,
        cwd.display()
    );
}

async fn download_with_archive_validation(
    downloader: &Downloader,
    url: &str,
    dest: &Path,
) -> LauncherResult<()> {
    downloader.download_file(url, dest, None).await?;

    if is_archive_path(dest) && !is_valid_archive(dest) {
        let _ = tokio::fs::remove_file(dest).await;
        return Err(LauncherError::Loader(format!(
            "Downloaded artifact is corrupt: {}",
            dest.display()
        )));
    }

    Ok(())
}

fn should_download_or_replace_archive(path: &Path) -> bool {
    !path.exists() || (is_archive_path(path) && !is_valid_archive(path))
}

fn is_archive_path(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|v| v.to_str()),
        Some("jar") | Some("zip")
    )
}

fn is_valid_archive(path: &Path) -> bool {
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };

    zip::ZipArchive::new(file).is_ok()
}

fn resolve_installed_neoforge_version_path(ctx: &InstallContext<'_>) -> PathBuf {
    let versions_dir = ctx.instance_dir.join("minecraft").join("versions");
    let candidates = [
        // Modern NeoForge uses a version id like: "<mc>-neoforge-<loader>".
        format!("{}-neoforge-{}", ctx.minecraft_version, ctx.loader_version),
        // Some setups keep only the "neoforge-<loader>" variant.
        format!("neoforge-{}", ctx.loader_version),
        // Legacy / fallback patterns.
        format!("{}-{}", ctx.minecraft_version, ctx.loader_version),
        ctx.loader_version.to_string(),
    ];

    for candidate in candidates {
        let path = versions_dir
            .join(&candidate)
            .join(format!("{}.json", candidate));
        if path.exists() {
            return path;
        }
    }

    versions_dir
        .join(ctx.loader_version)
        .join(format!("{}.json", ctx.loader_version))
}

fn build_processor_variables(
    ctx: &InstallContext<'_>,
    installer_path: &Path,
    installer_bytes: &[u8],
) -> LauncherResult<HashMap<String, String>> {
    let mut vars = HashMap::new();
    vars.insert("SIDE".to_string(), "client".to_string());
    vars.insert(
        "MINECRAFT_JAR".to_string(),
        ctx.instance_dir
            .join("client.jar")
            .to_string_lossy()
            .to_string(),
    );
    vars.insert(
        "LIBRARY_DIR".to_string(),
        ctx.libs_dir.to_string_lossy().to_string(),
    );
    vars.insert(
        "INSTALLER".to_string(),
        installer_path.to_string_lossy().to_string(),
    );
    vars.insert(
        "ROOT".to_string(),
        ctx.instance_dir.to_string_lossy().to_string(),
    );

    if let Some(binpatch_path) = extract_client_binpatch(ctx, installer_bytes)? {
        vars.insert(
            "BINPATCH".to_string(),
            binpatch_path.to_string_lossy().to_string(),
        );
    }

    Ok(vars)
}

fn merge_runtime_processor_variables(
    vars: &mut HashMap<String, String>,
    runtime_vars: &HashMap<String, String>,
) {
    for (key, value) in runtime_vars {
        vars.insert(key.clone(), value.clone());
    }
}

fn merge_profile_data_variables(vars: &mut HashMap<String, String>, data: &serde_json::Value) {
    let Some(obj) = data.as_object() else {
        return;
    };

    for (key, value) in obj {
        let resolved = value
            .get("client")
            .and_then(|v| v.as_str())
            .or_else(|| value.get("value").and_then(|v| v.as_str()))
            .or_else(|| value.as_str());

        if let Some(v) = resolved {
            vars.insert(key.clone(), v.to_string());
        }
    }
}

fn extract_client_binpatch(
    ctx: &InstallContext<'_>,
    installer_bytes: &[u8],
) -> LauncherResult<Option<PathBuf>> {
    let cursor = std::io::Cursor::new(installer_bytes);
    let mut archive = zip::ZipArchive::new(cursor)?;

    let mut source = None;
    for i in 0..archive.len() {
        let Ok(file) = archive.by_index(i) else {
            continue;
        };
        let name = file.name().to_string();
        if name.ends_with("client.lzma") || name.ends_with("client-binpatches.lzma") {
            source = Some(name);
            break;
        }
    }

    let Some(source_name) = source else {
        return Ok(None);
    };

    let mut source_file = archive.by_name(&source_name).map_err(|e| {
        LauncherError::Loader(format!(
            "Failed to access NeoForge binpatch from installer: {}",
            e
        ))
    })?;
    let mut bytes = Vec::new();
    source_file.read_to_end(&mut bytes)?;

    let target = ctx.instance_dir.join("client-binpatches.lzma");
    std::fs::write(&target, bytes).map_err(|e| LauncherError::Io {
        path: target.clone(),
        source: e,
    })?;

    Ok(Some(target))
}

fn resolve_processor_arg(
    arg: &str,
    vars: &HashMap<String, String>,
    libs_dir: &Path,
) -> LauncherResult<String> {
    let mut out = arg.to_string();

    for (key, value) in vars {
        out = out.replace(&format!("{{{}}}", key), value);
    }

    if let Some(coord) = out.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        let artifact = MavenArtifact::parse(coord)?;
        out = libs_dir
            .join(artifact.local_path())
            .to_string_lossy()
            .to_string();
    }

    Ok(out)
}

fn read_main_class_from_jar(path: &Path) -> LauncherResult<String> {
    let file = std::fs::File::open(path).map_err(|e| LauncherError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    let mut archive = zip::ZipArchive::new(file)?;
    let mut manifest = archive.by_name("META-INF/MANIFEST.MF").map_err(|e| {
        LauncherError::Loader(format!("Manifest not found in {}: {}", path.display(), e))
    })?;

    let mut text = String::new();
    manifest.read_to_string(&mut text)?;

    let mut main_class: Option<String> = None;
    let mut current_key: Option<String> = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix(' ') {
            if current_key.as_deref() == Some("Main-Class") {
                if let Some(value) = &mut main_class {
                    value.push_str(rest.trim());
                }
            }
            continue;
        }

        if let Some((key, value)) = line.split_once(':') {
            current_key = Some(key.trim().to_string());
            if key.trim() == "Main-Class" {
                main_class = Some(value.trim().to_string());
            }
        }
    }

    main_class.ok_or_else(|| {
        LauncherError::Loader(format!(
            "Main-Class missing in processor jar {}",
            path.display()
        ))
    })
}

fn resolve_version_with_inheritance(
    raw_json: &str,
    current_version_dir: &Path,
) -> LauncherResult<VersionJson> {
    let mut current_json: serde_json::Value = serde_json::from_str(raw_json)?;
    let versions_root = current_version_dir
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| current_version_dir.to_path_buf());

    for _ in 0..8 {
        let Some(parent_id) = current_json
            .get("inheritsFrom")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty())
        else {
            break;
        };

        let parent_path = versions_root
            .join(parent_id)
            .join(format!("{}.json", parent_id));
        let parent_raw = std::fs::read_to_string(&parent_path).map_err(|e| LauncherError::Io {
            path: parent_path.clone(),
            source: e,
        })?;
        let parent_json: serde_json::Value = serde_json::from_str(&parent_raw)?;
        current_json = VersionJson::merge_with_parent_json(&current_json, &parent_json);
    }

    serde_json::from_value(current_json).map_err(LauncherError::from)
}
