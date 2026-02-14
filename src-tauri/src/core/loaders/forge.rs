use std::collections::{BTreeSet, HashMap};
use std::io::Read;
use std::path::Path;

use serde::Deserialize;
use tracing::info;

use super::context::InstallContext;
use super::installer::{LoaderInstallResult, LoaderInstaller};
use crate::core::error::{LauncherError, LauncherResult};
use crate::core::maven::MavenArtifact;
use crate::core::version::VersionJson;

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
    #[serde(default)]
    pub processors: Vec<ForgeProcessor>,
    #[serde(default)]
    pub data: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct ForgeLibrary {
    pub name: String,
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

        let required_java =
            crate::core::java::required_java_for_minecraft_version(ctx.minecraft_version);
        let java_bin = crate::core::java::resolve_java_binary(required_java).await?;

        let minecraft_dir = ctx.instance_dir.join("minecraft");
        tokio::fs::create_dir_all(&minecraft_dir)
            .await
            .map_err(|e| LauncherError::Io {
                path: minecraft_dir.clone(),
                source: e,
            })?;

        let launcher_profiles_path = minecraft_dir.join("launcher_profiles.json");
        if !launcher_profiles_path.exists() {
            tokio::fs::write(
                &launcher_profiles_path,
                br#"{"profiles":{},"selectedProfile":null}"#,
            )
            .await
            .map_err(|e| LauncherError::Io {
                path: launcher_profiles_path.clone(),
                source: e,
            })?;
        }

        let output = std::process::Command::new(&java_bin)
            .arg("-jar")
            .arg(&installer_path)
            .arg("--installClient")
            .arg(&minecraft_dir)
            .current_dir(&minecraft_dir)
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

        let installed_version_path = minecraft_dir
            .join("versions")
            .join(&forge_id)
            .join(format!("{}.json", forge_id));
        let mut resolved_main_class = version_json.main_class.clone();
        let mut extra_jvm_args = Vec::new();
        let mut extra_game_args = Vec::new();
        let mut java_major = None;

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
            java_major = Some(installed_version.required_java_major());

            for lib in installed_version
                .download_libraries(ctx.libs_dir, ctx.downloader)
                .await?
            {
                libraries.insert(lib);
            }
        }

        for lib_name in &libraries {
            let Ok(artifact) = MavenArtifact::parse(lib_name) else {
                // Some metadata entries are direct artifact paths already resolved
                // from `downloads.artifact.path`; those are handled by classpath
                // resolution and do not need Maven coordinate downloads.
                continue;
            };

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

        run_processors(
            ctx,
            &java_bin,
            &installer_bytes,
            &installer_path,
            &install_profile,
        )?;

        let _ = tokio::fs::remove_file(&installer_path).await;

        info!("Forge {} installed successfully", forge_id);

        Ok(LoaderInstallResult {
            main_class: resolved_main_class,
            extra_jvm_args,
            extra_game_args,
            libraries: libraries.into_iter().collect(),
            asset_index_id: None,
            asset_index_url: None,
            java_major,
        })
    }
}

fn run_processors(
    ctx: InstallContext<'_>,
    java_bin: &Path,
    installer_bytes: &[u8],
    installer_path: &Path,
    install_profile: &ForgeInstallProfile,
) -> LauncherResult<()> {
    let mut variables = HashMap::new();
    merge_profile_data_variables(&mut variables, &install_profile.data);
    merge_runtime_processor_variables(
        &mut variables,
        &build_processor_variables(&ctx, installer_path, installer_bytes)?,
    );

    for processor in &install_profile.processors {
        if let Some(sides) = &processor.sides {
            if !sides.iter().any(|s| s == "client") {
                continue;
            }
        }

        let processor_artifact = MavenArtifact::parse(&processor.jar)?;
        let processor_jar_path = ctx.libs_dir.join(processor_artifact.local_path());
        if !processor_jar_path.exists() {
            return Err(LauncherError::Loader(format!(
                "Missing Forge processor JAR: {}",
                processor_jar_path.display()
            )));
        }

        let mut classpath_entries = vec![processor_jar_path.to_string_lossy().to_string()];
        for cp in &processor.classpath {
            let cp_artifact = MavenArtifact::parse(cp)?;
            let cp_path = ctx.libs_dir.join(cp_artifact.local_path());
            if cp_path.exists() {
                classpath_entries.push(cp_path.to_string_lossy().to_string());
            }
        }

        let classpath = classpath_entries.join(if cfg!(windows) { ";" } else { ":" });
        let main_class = read_main_class_from_jar(&processor_jar_path)?;
        let args = processor
            .args
            .iter()
            .map(|arg| resolve_processor_arg(arg, &variables, ctx.libs_dir))
            .collect::<LauncherResult<Vec<_>>>()?;

        info!(
            "Running Forge processor {} with main class {}",
            processor.jar, main_class
        );

        let output = std::process::Command::new(java_bin)
            .arg("-cp")
            .arg(&classpath)
            .arg(&main_class)
            .args(&args)
            .current_dir(ctx.libs_dir)
            .output()
            .map_err(|e| LauncherError::JavaExecution(e.to_string()))?;

        if !output.status.success() {
            return Err(LauncherError::Loader(format!(
                "Forge processor {} failed (code {:?})\nSTDOUT:\n{}\nSTDERR:\n{}",
                processor.jar,
                output.status.code(),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )));
        }
    }

    Ok(())
}

fn merge_runtime_processor_variables(
    vars: &mut HashMap<String, String>,
    runtime_vars: &HashMap<String, String>,
) {
    for (key, value) in runtime_vars {
        vars.insert(key.clone(), value.clone());
    }
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

fn extract_client_binpatch(
    ctx: &InstallContext<'_>,
    installer_bytes: &[u8],
) -> LauncherResult<Option<std::path::PathBuf>> {
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
            "Failed to access Forge binpatch from installer: {}",
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
