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
            java::find_java_binary(java_major).await?
        }
    };

    let natives_dir = instance.natives_dir();
    let game_dir = instance.game_dir();

    let mut cmd = std::process::Command::new(&java_bin);

    // ── JVM Arguments ──
    cmd.arg(format!("-Xmx{}M", instance.max_memory_mb));
    cmd.arg(format!(
        "-Djava.library.path={}",
        safe_path_str(&natives_dir)
    ));
    cmd.arg("-Dminecraft.launcher.brand=InterfaceOficial");
    cmd.arg("-Dminecraft.launcher.version=0.1.0");

    // Extra JVM args from instance config or loader
    for arg in &instance.jvm_args {
        cmd.arg(arg);
    }

    // Classpath
    cmd.arg("-cp").arg(classpath);

    // Main class
    cmd.arg(main_class);

    // ── Game Arguments ──
    cmd.arg("--gameDir").arg(safe_path_str(&game_dir));
    cmd.arg("--assetsDir")
        .arg(safe_path_str(&game_dir.join("assets")));

    if let Some(ref asset_index) = instance.asset_index {
        cmd.arg("--assetIndex").arg(asset_index);
    }

    // Extra game args from loader
    for arg in &instance.game_args {
        cmd.arg(arg);
    }

    // Placeholder auth (offline mode)
    cmd.arg("--username").arg("Player");
    cmd.arg("--version").arg(&instance.minecraft_version);
    cmd.arg("--accessToken").arg("0");

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
    // 1.21+ requires Java 21
    // 1.17+ requires Java 17
    // Older versions use Java 8 (but we target 17 minimum)
    let parts: Vec<&str> = minecraft_version.split('.').collect();
    if parts.len() >= 2 {
        if let (Ok(major), Ok(minor)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>()) {
            if major >= 1 && minor >= 21 {
                return 21;
            }
        }
    }
    17 // Safe default
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn java_major_detection() {
        assert_eq!(determine_java_major("1.21.4"), 21);
        assert_eq!(determine_java_major("1.21"), 21);
        assert_eq!(determine_java_major("1.20.4"), 17);
        assert_eq!(determine_java_major("1.16.5"), 17);
    }
}
