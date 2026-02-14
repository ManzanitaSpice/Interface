// ─── Launch Task ───
// Spawns the Minecraft game process with the correct arguments.

use std::process::Stdio;

use tracing::{debug, info};

use crate::core::error::{LauncherError, LauncherResult};
use crate::core::instance::Instance;
use crate::core::java;

use super::classpath::safe_path_str;

/// Launch the game as a child process.
///
/// Returns immediately after spawning. The caller is responsible for monitoring
/// the child process and setting state back to `Ready` when it exits.
pub async fn launch(instance: &Instance, classpath: &str) -> LauncherResult<std::process::Child> {
    let main_class = instance
        .main_class
        .as_deref()
        .ok_or_else(|| LauncherError::Other("Main class not set on instance".into()))?;

    // Determine Java binary
    let java_bin = match &instance.java_path {
        Some(p) if p.exists() => p.clone(),
        _ => {
            // Auto-detect based on version JSON requirements
            let java_major = instance
                .required_java_major
                .unwrap_or_else(|| determine_java_major(&instance.minecraft_version));
            java::resolve_java_binary(java_major).await?
        }
    };

    let natives_dir = instance.natives_dir();
    let game_dir = instance.game_dir();
    let assets_dir = game_dir.join("assets");

    let mut cmd = std::process::Command::new(&java_bin);

    // ── JVM Arguments ──
    cmd.arg(format!("-Xmx{}M", instance.max_memory_mb));
    cmd.arg("-Xms512M");
    cmd.arg(format!(
        "-Djava.library.path={}",
        safe_path_str(&natives_dir)
    ));
    cmd.arg("-Dminecraft.launcher.brand=InterfaceOficial");
    cmd.arg("-Dminecraft.launcher.version=0.1.0");

    // Extra JVM args from instance config or loader
    for arg in &instance.jvm_args {
        if arg.contains("${") {
            continue;
        }
        cmd.arg(arg);
    }

    // Classpath
    if classpath.trim().is_empty() {
        return Err(LauncherError::Other(
            "Classpath vacío: se cancela el arranque para evitar 'java -cp' inválido".into(),
        ));
    }
    debug!("Classpath len={} value={:?}", classpath.len(), classpath);
    cmd.arg("-cp").arg(classpath);

    // Main class
    cmd.arg(main_class);

    // ── Game Arguments ──
    cmd.arg("--gameDir").arg(safe_path_str(&game_dir));
    cmd.arg("--assetsDir").arg(safe_path_str(&assets_dir));

    if let Some(ref asset_index) = instance.asset_index {
        cmd.arg("--assetIndex").arg(asset_index);
    }

    // Extra game args from loader (replace known placeholders)
    for arg in &instance.game_args {
        let resolved = arg
            .replace("${auth_player_name}", "Player")
            .replace("${version_name}", &instance.minecraft_version)
            .replace("${game_directory}", &safe_path_str(&game_dir))
            .replace("${assets_root}", &safe_path_str(&assets_dir))
            .replace(
                "${assets_index_name}",
                instance.asset_index.as_deref().unwrap_or("legacy"),
            )
            .replace("${auth_uuid}", "00000000-0000-0000-0000-000000000000")
            .replace("${auth_access_token}", "0")
            .replace("${user_type}", "legacy")
            .replace("${version_type}", "release");
        cmd.arg(resolved);
    }

    // Placeholder auth (offline mode)
    cmd.arg("--username").arg("Player");
    cmd.arg("--uuid")
        .arg("00000000-0000-0000-0000-000000000000");
    cmd.arg("--version").arg(&instance.minecraft_version);
    cmd.arg("--accessToken").arg("0");
    cmd.arg("--userType").arg("legacy");
    cmd.arg("--versionType").arg("release");

    cmd.current_dir(&game_dir);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    info!("Launching Minecraft with Java: {:?}", java_bin);
    debug!("Command: {:?}", cmd);

    let child = cmd
        .spawn()
        .map_err(|e| LauncherError::JavaExecution(e.to_string()))?;

    Ok(child)
}

/// Determine the required Java major version based on Minecraft version string.
fn determine_java_major(minecraft_version: &str) -> u32 {
    let parts: Vec<&str> = minecraft_version.split('.').collect();
    if parts.len() < 2 {
        return 17;
    }

    let major = parts[0].parse::<u32>().unwrap_or(1);
    let minor = parts[1].parse::<u32>().unwrap_or(20);

    if major > 1 || minor >= 21 {
        return 21;
    }

    if minor >= 17 {
        return 17;
    }

    8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn java_major_detection() {
        assert_eq!(determine_java_major("1.21.4"), 21);
        assert_eq!(determine_java_major("1.21"), 21);
        assert_eq!(determine_java_major("1.20.4"), 17);
        assert_eq!(determine_java_major("1.16.5"), 8);
        assert_eq!(determine_java_major("1.8.9"), 8);
    }
}
