// ─── Classpath Builder ───
// Constructs the dynamic classpath string for launching Minecraft.

use std::path::{Path, PathBuf};

use tracing::debug;

use crate::core::error::{LauncherError, LauncherResult};
use crate::core::instance::Instance;
use crate::core::maven::MavenArtifact;

/// Builds the full classpath string for launching the game.
///
/// Includes:
/// - `client.jar`
/// - All vanilla libraries
/// - All loader libraries
///
/// Uses `;` on Windows, `:` on Linux/macOS.
pub fn build_classpath(
    instance: &Instance,
    libs_dir: &Path,
    extra_lib_coords: &[String],
) -> LauncherResult<String> {
    let separator = if cfg!(windows) { ";" } else { ":" };
    let mut entries: Vec<String> = Vec::new();

    // 1. All declared libraries (Vanilla + loader)
    for coord in extra_lib_coords {
        match MavenArtifact::parse(coord) {
            Ok(artifact) => {
                let lib_path = libs_dir.join(artifact.local_path());
                if lib_path.exists() {
                    entries.push(safe_path_str(&lib_path));
                } else {
                    debug!("Library not found on disk (skipping): {:?}", lib_path);
                }
            }
            Err(e) => {
                debug!("Invalid library coordinate '{}': {}", coord, e);
            }
        }
    }

    // 2. Minecraft base client JAR
    let client_jar = instance.path.join("client.jar");
    if client_jar.exists() {
        entries.push(safe_path_str(&client_jar));
    }

    if entries.is_empty() {
        return Err(LauncherError::Other(
            "Classpath is empty — no libraries or client.jar found".into(),
        ));
    }

    Ok(entries.join(separator))
}

/// Extract native libraries from JARs that contain `.dll`, `.so`, or `.dylib`.
///
/// Creates a temporary `natives/` directory inside the instance.
pub async fn extract_natives(
    instance: &Instance,
    libs_dir: &Path,
    native_coords: &[String],
) -> LauncherResult<PathBuf> {
    let natives_dir = instance.natives_dir();

    // Clean previous session
    if natives_dir.exists() {
        let _ = tokio::fs::remove_dir_all(&natives_dir).await;
    }
    tokio::fs::create_dir_all(&natives_dir)
        .await
        .map_err(|e| LauncherError::Io {
            path: natives_dir.clone(),
            source: e,
        })?;

    for coord in native_coords {
        let artifact = match MavenArtifact::parse(coord) {
            Ok(a) => a,
            Err(_) => continue,
        };
        let jar_path = libs_dir.join(artifact.local_path());
        if !jar_path.exists() {
            continue;
        }

        // Extract .dll/.so/.dylib from the JAR
        let jar_bytes = tokio::fs::read(&jar_path)
            .await
            .map_err(|e| LauncherError::Io {
                path: jar_path.clone(),
                source: e,
            })?;
        let cursor = std::io::Cursor::new(jar_bytes);
        let mut archive = match zip::ZipArchive::new(cursor) {
            Ok(a) => a,
            Err(e) => {
                tracing::warn!("Cannot open native JAR {:?}: {}", jar_path, e);
                continue;
            }
        };

        for i in 0..archive.len() {
            let mut file = archive.by_index(i)?;
            let name = file.name().to_string();

            let is_native = name.ends_with(".dll")
                || name.ends_with(".so")
                || name.ends_with(".dylib")
                || name.ends_with(".jnilib");

            if is_native && !name.contains('/') {
                let dest = natives_dir.join(&name);
                let mut out = std::fs::File::create(&dest).map_err(|e| LauncherError::Io {
                    path: dest.clone(),
                    source: e,
                })?;
                std::io::copy(&mut file, &mut out).map_err(|e| LauncherError::Io {
                    path: dest.clone(),
                    source: e,
                })?;
                debug!("Extracted native: {}", name);
            }
        }
    }

    Ok(natives_dir)
}

/// Clean up the temporary natives directory after the game exits.
pub async fn cleanup_natives(instance: &Instance) {
    let natives_dir = instance.natives_dir();
    if natives_dir.exists() {
        let _ = tokio::fs::remove_dir_all(&natives_dir).await;
    }
}

/// Convert path to string, using `\\?\` prefix on Windows for long path support.
pub fn safe_path_str(path: &Path) -> String {
    match std::fs::canonicalize(path) {
        Ok(p) => p.to_string_lossy().to_string(),
        Err(_) => path.to_string_lossy().to_string(),
    }
}
