// ─── Classpath Builder ───
// Constructs the dynamic classpath string for launching Minecraft.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tracing::{debug, warn};

use crate::core::error::{LauncherError, LauncherResult};
use crate::core::instance::{Instance, LoaderType};
use crate::core::maven::MavenArtifact;

fn parse_numeric_version_parts(raw: &str) -> Vec<u32> {
    raw.split(|c: char| !c.is_ascii_digit())
        .filter(|segment| !segment.is_empty())
        .filter_map(|segment| segment.parse::<u32>().ok())
        .collect()
}

fn compare_versions(a: &str, b: &str) -> Ordering {
    let a_parts = parse_numeric_version_parts(a);
    let b_parts = parse_numeric_version_parts(b);

    let max_len = a_parts.len().max(b_parts.len());
    for idx in 0..max_len {
        let a_val = a_parts.get(idx).copied().unwrap_or(0);
        let b_val = b_parts.get(idx).copied().unwrap_or(0);
        match a_val.cmp(&b_val) {
            Ordering::Equal => continue,
            non_eq => return non_eq,
        }
    }

    // Deterministic tiebreaker for versions with identical numeric parts.
    a.cmp(b)
}

fn uses_module_bootstrap(instance: &Instance) -> bool {
    instance.jvm_args.iter().any(|arg| {
        let trimmed = arg.trim();
        trimmed == "--module-path"
            || trimmed == "-p"
            || trimmed.starts_with("--module-path=")
            || trimmed == "--add-modules"
            || trimmed.starts_with("--add-modules=")
    })
}

fn is_bootstraplauncher_main(instance: &Instance) -> bool {
    instance
        .main_class
        .as_deref()
        .is_some_and(|main| main == "cpw.mods.bootstraplauncher.BootstrapLauncher")
}

fn should_skip_cpw_mods_bootstrap_on_classpath(instance: &Instance) -> bool {
    // When launching BootstrapLauncher from the classpath, ModLauncher will load
    // securejarhandler/modlauncher/jarhandling in a separate MC-BOOTSTRAP layer.
    // If we also include them on -cp, they can get initialized twice with different
    // classloaders, causing `java.net.URL.setURLStreamHandlerFactory` to be invoked
    // more than once -> `java.lang.Error: factory already defined`.
    is_bootstraplauncher_main(instance) && matches!(instance.loader, LoaderType::Forge | LoaderType::NeoForge)
}

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
    let separator = get_classpath_separator();
    let mut entries: Vec<String> = Vec::new();
    let mut non_asm_entries: Vec<String> = Vec::new();
    let module_bootstrap = uses_module_bootstrap(instance);
    let skip_cpw_mods_bootstrap = should_skip_cpw_mods_bootstrap_on_classpath(instance);

    // ASM is extremely order-sensitive for Forge/NeoForge bootstrap.
    // If multiple ASM versions exist, the first one on the classpath wins.
    // Ensure the newest ASM jars appear first and older duplicates are ignored.
    // Key: artifactId + classifier (to keep e.g. asm-tree separate).
    let mut best_asm_by_key: HashMap<String, (String, String)> = HashMap::new();

    // 1. All declared libraries (Vanilla + loader)
    for coord in extra_lib_coords {
        let trimmed = coord.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Ok(artifact) = MavenArtifact::parse(trimmed) {
            // When launching Forge/NeoForge with module-path, putting these on the classpath
            // can load them twice (module layer + classpath) and crash with:
            // `java.lang.Error: factory already defined`.
            if module_bootstrap
                && artifact.group_id == "cpw.mods"
                && matches!(
                    artifact.artifact_id.as_str(),
                    "securejarhandler" | "modlauncher" | "jarhandling"
                )
            {
                continue;
            }

            // Even without explicit module-path JVM args, BootstrapLauncher/ModLauncher
            // will construct its own MC-BOOTSTRAP layer. Keep these off the -cp to
            // prevent double initialization.
            if skip_cpw_mods_bootstrap
                && artifact.group_id == "cpw.mods"
                && matches!(
                    artifact.artifact_id.as_str(),
                    "securejarhandler" | "modlauncher" | "jarhandling"
                )
            {
                continue;
            }

            if artifact.group_id == "org.ow2.asm" {
                let classifier = artifact.classifier.clone().unwrap_or_default();
                let key = format!("{}:{}", artifact.artifact_id, classifier);

                match best_asm_by_key.get(&key) {
                    None => {
                        best_asm_by_key.insert(key, (trimmed.to_string(), artifact.version));
                    }
                    Some((_, existing_version)) => {
                        if compare_versions(&artifact.version, existing_version) == Ordering::Greater {
                            best_asm_by_key.insert(key, (trimmed.to_string(), artifact.version));
                        }
                    }
                }

                continue;
            }
        }

        if let Some(entry) = resolve_library_entry(instance, libs_dir, trimmed) {
            non_asm_entries.push(entry);
        } else {
            debug!("Library not found on disk (skipping): {}", trimmed);
        }
    }

    // 1.0 Insert best ASM jars first (newest version wins per artifact).
    let mut asm_candidates: Vec<(String, String)> = best_asm_by_key.into_values().collect();
    asm_candidates.sort_by(|(_, a_version), (_, b_version)| {
        compare_versions(b_version, a_version).then_with(|| b_version.cmp(a_version))
    });
    for (coord, _version) in asm_candidates {
        if let Some(entry) = resolve_library_entry(instance, libs_dir, &coord) {
            entries.push(entry);
        }
    }

    // 1.0b Then append the rest of libraries.
    entries.extend(non_asm_entries);

    // 1.1 Fallback: include every local JAR generated by installer-based loaders.
    // Forge/NeoForge installers can materialize additional launch-critical artifacts
    // under instance-local repositories that are not always declared in metadata.
    let local_jars = collect_local_library_jars(instance);
    if !local_jars.is_empty() {
        debug!("Found {} local library JARs", local_jars.len());
    }

    // Avoid poisoning the classpath with duplicates (same jar in different roots)
    // and with older bootstrap artifacts. Duplicate securejarhandler/modlauncher jars
    // can trigger `java.net.URL.setURLStreamHandlerFactory` twice -> "factory already defined".
    let mut included_basenames = std::collections::HashSet::<String>::new();
    for entry in &entries {
        if let Some(name) = std::path::Path::new(entry)
            .file_name()
            .and_then(|n| n.to_str())
        {
            let key = if cfg!(target_os = "windows") {
                name.to_lowercase()
            } else {
                name.to_string()
            };
            included_basenames.insert(key);
        }
    }

    let sensitive_prefixes: [&str; 6] = [
        "securejarhandler-",
        "modlauncher-",
        "jarhandling-",
        "bootstraplauncher-",
        "fmlloader-",
        "fmlcore-",
    ];

    let mut newest_sensitive: HashMap<String, (PathBuf, String)> = HashMap::new();
    let mut other_local = Vec::<PathBuf>::new();

    for discovered_jar in local_jars {
        let Some(file_name) = discovered_jar.file_name().and_then(|n| n.to_str()) else {
            continue;
        };

        let file_key = if cfg!(target_os = "windows") {
            file_name.to_lowercase()
        } else {
            file_name.to_string()
        };

        if included_basenames.contains(&file_key) {
            continue;
        }

        // Do not inject bootstrap artifacts from local jar scanning.
        // They must not appear on the JVM -cp for BootstrapLauncher runs.
        if (module_bootstrap || skip_cpw_mods_bootstrap)
            && (file_key.starts_with("securejarhandler-")
                || file_key.starts_with("modlauncher-")
                || file_key.starts_with("jarhandling-"))
        {
            continue;
        }

        // Prefer the newest version for sensitive bootstrap artifacts.
        let mut captured = false;
        for prefix in sensitive_prefixes {
            if let Some(rest) = file_key.strip_prefix(prefix) {
                if let Some(rest) = rest.strip_suffix(".jar") {
                    // rest is like "11.0.5" or "2.1.10_7" etc.
                    // Keep only the newest by numeric compare.
                    let artifact_name = prefix.trim_end_matches('-').to_string();
                    let version = rest.to_string();
                    match newest_sensitive.get(&artifact_name) {
                        None => {
                            newest_sensitive.insert(artifact_name, (discovered_jar.clone(), version));
                        }
                        Some((_, existing_version)) => {
                            if compare_versions(&version, existing_version) == Ordering::Greater {
                                newest_sensitive.insert(
                                    artifact_name,
                                    (discovered_jar.clone(), version),
                                );
                            }
                        }
                    }
                    captured = true;
                    break;
                }
            }
        }
        if captured {
            continue;
        }

        other_local.push(discovered_jar);
    }

    // Insert newest sensitive jars first.
    let mut newest_sensitive_values: Vec<(PathBuf, String)> = newest_sensitive
        .into_values()
        .collect();
    newest_sensitive_values.sort_by(|(_, a), (_, b)| compare_versions(b, a));
    for (path, _) in newest_sensitive_values {
        entries.push(safe_path_str(&path));
    }

    // Then include other local jars.
    for path in other_local {
        entries.push(safe_path_str(&path));
    }

    // 1.2 Loader/vanilla version JARs generated under `minecraft/versions`.
    // Forge/NeoForge bootstrap classes are provided by these jars, not by Maven libs.
    let version_jars = collect_required_version_jars(instance);
    if version_jars.is_empty() && instance.loader != LoaderType::Vanilla {
        warn!(
            "No version JARs found for loader {:?}. The game might not start.",
            instance.loader
        );
    }
    for version_jar in version_jars {
        entries.push(safe_path_str(&version_jar));
    }

    // 2. Minecraft base client JAR
    let client_jar = instance.path.join("client.jar");
    if client_jar.exists() {
        entries.push(safe_path_str(&client_jar));
    } else {
        let global_client = instance
            .game_dir()
            .join("versions")
            .join(&instance.minecraft_version)
            .join(format!("{}.jar", instance.minecraft_version));
        if global_client.exists() {
            entries.push(safe_path_str(&global_client));
        }
    }

    if entries.is_empty() {
        return Err(LauncherError::Other(
            "Classpath is empty — no libraries or client.jar found".into(),
        ));
    }

    entries.retain(|entry| !entry.trim().is_empty());
    dedup_preserving_order(&mut entries);
    prioritize_bootstrap_entries(&mut entries);
    if entries.is_empty() {
        return Err(LauncherError::Other(
            "Classpath is empty after filtering invalid entries".into(),
        ));
    }

    Ok(entries.join(separator))
}

/// Platform-specific Java classpath separator.
pub fn get_classpath_separator() -> &'static str {
    if cfg!(target_os = "windows") {
        ";"
    } else {
        ":"
    }
}

fn resolve_library_entry(instance: &Instance, libs_dir: &Path, raw: &str) -> Option<String> {
    let direct_path = Path::new(raw);

    // Absolute path as-is.
    if direct_path.is_absolute() && direct_path.exists() && is_allowed_classpath_path(direct_path) {
        return Some(safe_path_str(direct_path));
    }

    // Relative path candidates (for loader metadata that references local jars).
    let relative_candidates = [
        libs_dir.join(raw),
        instance.path.join(raw),
        instance.game_dir().join("libraries").join(raw),
        instance.path.join("libraries").join(raw),
    ];
    for candidate in relative_candidates {
        if candidate.exists() && is_allowed_classpath_path(&candidate) {
            return Some(safe_path_str(&candidate));
        }
    }

    // Maven coordinate candidates in global and instance-local repositories.
    if let Ok(artifact) = MavenArtifact::parse(raw) {
        let repo_candidates = [
            libs_dir.join(artifact.local_path()),
            instance.path.join("libraries").join(artifact.local_path()),
            instance
                .game_dir()
                .join("libraries")
                .join(artifact.local_path()),
        ];
        for candidate in repo_candidates {
            if candidate.exists() && is_allowed_classpath_path(&candidate) {
                return Some(safe_path_str(&candidate));
            }
        }
    }

    None
}

fn is_allowed_classpath_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("jar") || ext.eq_ignore_ascii_case("zip"))
}

fn collect_local_library_jars(instance: &Instance) -> Vec<PathBuf> {
    let mut jars = Vec::new();
    let mut scan_dirs = vec![
        instance.path.join("libraries"),
        instance.game_dir().join("libraries"),
    ];
    scan_dirs.dedup();

    for dir in scan_dirs {
        if !dir.exists() {
            continue;
        }

        let mut stack = vec![dir];
        while let Some(current_dir) = stack.pop() {
            let read_dir = match std::fs::read_dir(&current_dir) {
                Ok(read_dir) => read_dir,
                Err(_) => continue,
            };

            for entry in read_dir.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if is_allowed_classpath_path(&path) {
                    jars.push(path);
                }
            }
        }
    }

    jars
}

fn collect_required_version_jars(instance: &Instance) -> Vec<PathBuf> {
    let versions_dir = instance.game_dir().join("versions");
    let mut potential_ids = vec![instance.minecraft_version.clone()];

    if let Some(loader_version) = &instance.loader_version {
        if !loader_version.trim().is_empty() {
            match instance.loader {
                LoaderType::Forge => {
                    potential_ids
                        .push(format!("{}-{}", instance.minecraft_version, loader_version));
                    potential_ids.push(format!(
                        "forge-{}-{}",
                        instance.minecraft_version, loader_version
                    ));
                }
                LoaderType::NeoForge => {
                    potential_ids
                        .push(format!("{}-{}", instance.minecraft_version, loader_version));
                    potential_ids.push(format!("neoforge-{}", loader_version));
                    potential_ids.push(format!(
                        "{}-neoforge-{}",
                        instance.minecraft_version, loader_version
                    ));
                    potential_ids.push(loader_version.clone());
                }
                _ => {}
            }
        }
    }

    let mut jars = Vec::new();
    for version_id in potential_ids {
        let jar = versions_dir
            .join(&version_id)
            .join(format!("{}.jar", version_id));
        if jar.exists() {
            debug!("Found version JAR: {:?}", jar);
            jars.push(jar);
        }
    }

    jars
}

fn dedup_preserving_order(entries: &mut Vec<String>) {
    let mut seen = std::collections::HashSet::new();
    entries.retain(|entry| {
        let key = if cfg!(target_os = "windows") {
            entry.to_lowercase()
        } else {
            entry.clone()
        };
        seen.insert(key)
    });
}

/// ModLauncher-based stacks (Forge/NeoForge) are sensitive to classpath order.
/// Ensure bootstrap artifacts are loaded first before the rest of the runtime jars.
fn prioritize_bootstrap_entries(entries: &mut Vec<String>) {
    fn score(entry: &str) -> usize {
        let lower = entry.to_ascii_lowercase();
        if lower.contains("bootstraplauncher") {
            0
        } else if lower.contains("modlauncher") {
            1
        } else if lower.contains("securejarhandler") {
            2
        } else {
            10
        }
    }

    let mut indexed: Vec<(usize, usize, String)> = entries
        .drain(..)
        .enumerate()
        .map(|(idx, entry)| (score(&entry), idx, entry))
        .collect();

    indexed.sort_by_key(|(priority, idx, _)| (*priority, *idx));
    entries.extend(indexed.into_iter().map(|(_, _, entry)| entry));
}

#[cfg(test)]
mod tests {
    use super::prioritize_bootstrap_entries;

    #[test]
    fn bootstrapping_jars_are_moved_to_the_front_in_required_order() {
        let mut entries = vec![
            "/tmp/other-lib.jar".to_string(),
            "/tmp/modlauncher-10.0.jar".to_string(),
            "/tmp/securejarhandler-3.0.jar".to_string(),
            "/tmp/bootstraplauncher-2.0.jar".to_string(),
            "/tmp/another-lib.jar".to_string(),
        ];

        prioritize_bootstrap_entries(&mut entries);

        assert_eq!(entries[0], "/tmp/bootstraplauncher-2.0.jar");
        assert_eq!(entries[1], "/tmp/modlauncher-10.0.jar");
        assert_eq!(entries[2], "/tmp/securejarhandler-3.0.jar");
    }
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
        let effective_path = if jar_path.exists() {
            jar_path
        } else {
            let local_path = instance
                .game_dir()
                .join("libraries")
                .join(artifact.local_path());
            if !local_path.exists() {
                continue;
            }
            local_path
        };

        // Extract .dll/.so/.dylib from the JAR
        let jar_bytes = tokio::fs::read(&effective_path)
            .await
            .map_err(|e| LauncherError::Io {
                path: effective_path.clone(),
                source: e,
            })?;

        let dest_dir = natives_dir.clone();
        let path_debug = effective_path.clone();
        tokio::task::spawn_blocking(move || {
            let cursor = std::io::Cursor::new(jar_bytes);
            let mut archive = match zip::ZipArchive::new(cursor) {
                Ok(a) => a,
                Err(e) => {
                    warn!("Cannot open native JAR {:?}: {}", path_debug, e);
                    return;
                }
            };

            for i in 0..archive.len() {
                let file = archive.by_index(i);
                if file.is_err() {
                    continue;
                }
                let mut file = file.unwrap();
                let name = file.name().to_string();

                if name.contains("META-INF") || name.contains('/') || name.contains('\\') {
                    continue;
                }

                let is_native = name.ends_with(".dll")
                    || name.ends_with(".so")
                    || name.ends_with(".dylib")
                    || name.ends_with(".jnilib");

                if is_native {
                    let dest = dest_dir.join(&name);
                    let mut out = match std::fs::File::create(&dest) {
                        Ok(file) => file,
                        Err(_) => continue,
                    };
                    let _ = std::io::copy(&mut file, &mut out);
                    debug!("Extracted native: {}", name);
                }
            }
        })
        .await
        .map_err(|e| LauncherError::Other(format!("Task join error: {}", e)))?;
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
    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let text = resolved.to_string_lossy().to_string();

    #[cfg(target_os = "windows")]
    {
        // Java classpath handling can fail for Windows extended-length paths
        // (e.g. `\\?\C:\...`) and report `ClassNotFoundException` even when
        // jars exist. Strip the prefix before building launch arguments.
        if let Some(stripped) = text.strip_prefix(r"\\?\") {
            return stripped.to_string();
        }
    }

    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::instance::{Instance, LoaderType};

    fn test_instance(base_dir: &Path) -> Instance {
        let mut instance = Instance::new(
            "test".into(),
            "1.21.1".into(),
            LoaderType::Vanilla,
            None,
            2048,
            base_dir,
        );
        instance.path = base_dir.to_path_buf();
        instance
    }

    #[test]
    fn build_classpath_rejects_empty_entries() {
        let temp =
            std::env::temp_dir().join(format!("classpath-test-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();
        let instance = test_instance(&temp);
        let libs_dir = temp.join("libraries");
        std::fs::create_dir_all(&libs_dir).unwrap();

        let err = build_classpath(&instance, &libs_dir, &["   ".into()]).unwrap_err();
        assert!(err.to_string().contains("Classpath is empty"));

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn build_classpath_resolves_instance_local_libraries_for_installer_loaders() {
        let temp =
            std::env::temp_dir().join(format!("classpath-test-local-libs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        let instance_dir = temp.join("instance");
        let game_libs = instance_dir.join("minecraft").join("libraries");
        std::fs::create_dir_all(&game_libs).unwrap();
        std::fs::write(instance_dir.join("client.jar"), b"client").unwrap();

        let coord = "net.minecraftforge:bootstrap:2.1.7";
        let artifact = MavenArtifact::parse(coord).unwrap();
        let local_jar = game_libs.join(artifact.local_path());
        std::fs::create_dir_all(local_jar.parent().unwrap()).unwrap();
        std::fs::write(&local_jar, b"bootstrap").unwrap();

        let instance = test_instance(&instance_dir);
        let classpath =
            build_classpath(&instance, &temp.join("libraries"), &[coord.into()]).unwrap();

        assert!(classpath.contains("bootstrap-2.1.7.jar"));
        assert!(classpath.contains("client.jar"));

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn build_classpath_collects_discovered_local_jars_even_without_declared_coordinate() {
        let temp = std::env::temp_dir().join(format!(
            "classpath-test-discovered-local-jars-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp);
        let instance_dir = temp.join("instance");
        let local_repo = instance_dir.join("libraries").join("custom");
        std::fs::create_dir_all(&local_repo).unwrap();
        std::fs::write(instance_dir.join("client.jar"), b"client").unwrap();
        std::fs::write(local_repo.join("installer-generated.jar"), b"local").unwrap();

        let instance = test_instance(&instance_dir);
        let classpath = build_classpath(&instance, &temp.join("libraries"), &[]).unwrap();

        assert!(classpath.contains("installer-generated.jar"));

        let _ = std::fs::remove_dir_all(&temp);
    }
    #[test]
    fn build_classpath_accepts_direct_library_paths() {
        let temp =
            std::env::temp_dir().join(format!("classpath-test-direct-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();
        let instance_dir = temp.join("instance");
        std::fs::create_dir_all(&instance_dir).unwrap();
        std::fs::write(instance_dir.join("client.jar"), b"client").unwrap();
        let instance = test_instance(&instance_dir);

        let external_jar = temp.join("external-lib.jar");
        std::fs::write(&external_jar, b"lib").unwrap();

        let classpath = build_classpath(
            &instance,
            &temp.join("libraries"),
            &[external_jar.to_string_lossy().to_string()],
        )
        .unwrap();

        assert!(classpath.contains("external-lib.jar"));
        assert!(classpath.contains("client.jar"));

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn build_classpath_ignores_non_jar_entries() {
        let temp = std::env::temp_dir().join(format!(
            "classpath-test-ignore-non-jar-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();
        let instance_dir = temp.join("instance");
        std::fs::create_dir_all(&instance_dir).unwrap();
        std::fs::write(instance_dir.join("client.jar"), b"client").unwrap();
        let instance = test_instance(&instance_dir);

        let mappings = temp.join("neoform-mappings.tsrg.lzma");
        std::fs::write(&mappings, b"not-a-jar").unwrap();

        let classpath = build_classpath(
            &instance,
            &temp.join("libraries"),
            &[mappings.to_string_lossy().to_string()],
        )
        .unwrap();

        assert!(!classpath.contains("neoform-mappings.tsrg.lzma"));
        assert!(classpath.contains("client.jar"));

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn build_classpath_falls_back_to_global_client_jar() {
        let temp = std::env::temp_dir().join(format!(
            "classpath-test-global-client-fallback-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp);
        let instance_dir = temp.join("instance");
        std::fs::create_dir_all(&instance_dir).unwrap();

        let mut instance = test_instance(&instance_dir);
        instance.minecraft_version = "1.20.4".into();

        let global_client = instance
            .game_dir()
            .join("versions")
            .join("1.20.4")
            .join("1.20.4.jar");
        std::fs::create_dir_all(global_client.parent().unwrap()).unwrap();
        std::fs::write(&global_client, b"global-client").unwrap();

        let classpath = build_classpath(&instance, &temp.join("libraries"), &[]).unwrap();

        assert!(classpath.contains("1.20.4/1.20.4.jar"));

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn build_classpath_includes_neoforge_variant_version_jars() {
        let temp = std::env::temp_dir().join(format!(
            "classpath-test-neoforge-version-jars-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp);
        let instance_dir = temp.join("instance");
        std::fs::create_dir_all(&instance_dir).unwrap();
        std::fs::write(instance_dir.join("client.jar"), b"client").unwrap();

        let mut instance = test_instance(&instance_dir);
        instance.loader = LoaderType::NeoForge;
        instance.loader_version = Some("20.4.80".into());
        instance.minecraft_version = "1.20.4".into();

        let variant = instance
            .game_dir()
            .join("versions")
            .join("1.20.4-neoforge-20.4.80")
            .join("1.20.4-neoforge-20.4.80.jar");
        std::fs::create_dir_all(variant.parent().unwrap()).unwrap();
        std::fs::write(&variant, b"neoforge").unwrap();

        let classpath = build_classpath(&instance, &temp.join("libraries"), &[]).unwrap();

        assert!(classpath.contains("1.20.4-neoforge-20.4.80/1.20.4-neoforge-20.4.80.jar"));

        let _ = std::fs::remove_dir_all(&temp);
    }
    #[test]
    fn build_classpath_includes_forge_and_vanilla_version_jars() {
        let temp = std::env::temp_dir().join(format!(
            "classpath-test-version-jars-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp);
        let instance_dir = temp.join("instance");
        std::fs::create_dir_all(&instance_dir).unwrap();
        std::fs::write(instance_dir.join("client.jar"), b"client").unwrap();

        let mut instance = test_instance(&instance_dir);
        instance.loader = LoaderType::Forge;
        instance.loader_version = Some("47.2.0".into());
        instance.minecraft_version = "1.20.1".into();

        let vanilla_jar = instance
            .game_dir()
            .join("versions")
            .join("1.20.1")
            .join("1.20.1.jar");
        let forge_jar = instance
            .game_dir()
            .join("versions")
            .join("1.20.1-47.2.0")
            .join("1.20.1-47.2.0.jar");

        std::fs::create_dir_all(vanilla_jar.parent().unwrap()).unwrap();
        std::fs::create_dir_all(forge_jar.parent().unwrap()).unwrap();
        std::fs::write(&vanilla_jar, b"vanilla").unwrap();
        std::fs::write(&forge_jar, b"forge").unwrap();

        let classpath = build_classpath(&instance, &temp.join("libraries"), &[]).unwrap();

        assert!(classpath.contains("1.20.1/1.20.1.jar"));
        assert!(classpath.contains("1.20.1-47.2.0/1.20.1-47.2.0.jar"));

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn build_classpath_prioritizes_newest_asm_first() {
        let temp = std::env::temp_dir().join(format!(
            "classpath-test-asm-order-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp);

        let instance_dir = temp.join("instance");
        std::fs::create_dir_all(&instance_dir).unwrap();
        std::fs::write(instance_dir.join("client.jar"), b"client").unwrap();

        let instance = test_instance(&instance_dir);
        let libs_dir = temp.join("libraries");

        // Create both ASM 9.3 and 9.8 jars on disk.
        let asm_old = MavenArtifact::parse("org.ow2.asm:asm:9.3").unwrap();
        let asm_new = MavenArtifact::parse("org.ow2.asm:asm:9.8").unwrap();
        let old_path = libs_dir.join(asm_old.local_path());
        let new_path = libs_dir.join(asm_new.local_path());
        std::fs::create_dir_all(old_path.parent().unwrap()).unwrap();
        std::fs::create_dir_all(new_path.parent().unwrap()).unwrap();
        std::fs::write(&old_path, b"old").unwrap();
        std::fs::write(&new_path, b"new").unwrap();

        let classpath = build_classpath(
            &instance,
            &libs_dir,
            &[
                "org.ow2.asm:asm:9.3".into(),
                "org.ow2.asm:asm:9.8".into(),
            ],
        )
        .unwrap();

        // We keep only the newest ASM per artifact to prevent old ASM from being selected.
        assert!(classpath.contains("asm-9.8.jar"));
        assert!(!classpath.contains("asm-9.3.jar"));

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn build_classpath_dedupes_sensitive_bootstrap_jars_to_newest() {
        let temp = std::env::temp_dir().join(format!(
            "classpath-test-sensitive-dedupe-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp);

        let instance_dir = temp.join("instance");
        std::fs::create_dir_all(&instance_dir).unwrap();
        std::fs::write(instance_dir.join("client.jar"), b"client").unwrap();

        let instance = test_instance(&instance_dir);
        let libs_dir = temp.join("libraries");
        std::fs::create_dir_all(&libs_dir).unwrap();

        // Put two versions of the same sensitive artifact in instance-local repo.
        // They should not both end up on the classpath.
        let local_repo = instance_dir.join("libraries").join("cpw").join("mods");
        std::fs::create_dir_all(&local_repo).unwrap();
        std::fs::write(local_repo.join("securejarhandler-2.1.6.jar"), b"old").unwrap();
        std::fs::write(local_repo.join("securejarhandler-2.1.8.jar"), b"new").unwrap();

        let classpath = build_classpath(&instance, &libs_dir, &[]).unwrap();

        assert!(classpath.contains("securejarhandler-2.1.8.jar"));
        assert!(!classpath.contains("securejarhandler-2.1.6.jar"));

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn build_classpath_skips_cpw_mods_bootstrap_when_module_path_present() {
        let temp = std::env::temp_dir().join(format!(
            "classpath-test-module-skip-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp);

        let instance_dir = temp.join("instance");
        std::fs::create_dir_all(&instance_dir).unwrap();
        std::fs::write(instance_dir.join("client.jar"), b"client").unwrap();

        let mut instance = test_instance(&instance_dir);
        // Simulate Forge/NeoForge modular launch metadata.
        instance.jvm_args = vec![
            "--module-path".into(),
            "${library_directory}".into(),
            "--add-modules".into(),
            "ALL-MODULE-PATH".into(),
        ];

        let libs_dir = temp.join("libraries");
        std::fs::create_dir_all(&libs_dir).unwrap();

        // Materialize jars so `resolve_library_entry` can find them.
        let sjh = MavenArtifact::parse("cpw.mods:securejarhandler:2.1.8").unwrap();
        let ml = MavenArtifact::parse("cpw.mods:modlauncher:11.0.5").unwrap();
        let jh = MavenArtifact::parse("cpw.mods:jarhandling:0.5.5").unwrap();
        for art in [&sjh, &ml, &jh] {
            let p = libs_dir.join(art.local_path());
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, b"x").unwrap();
        }

        let classpath = build_classpath(
            &instance,
            &libs_dir,
            &[
                "cpw.mods:securejarhandler:2.1.8".into(),
                "cpw.mods:modlauncher:11.0.5".into(),
                "cpw.mods:jarhandling:0.5.5".into(),
            ],
        )
        .unwrap();

        assert!(!classpath.contains("securejarhandler-2.1.8.jar"));
        assert!(!classpath.contains("modlauncher-11.0.5.jar"));
        assert!(!classpath.contains("jarhandling-0.5.5.jar"));

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn build_classpath_skips_cpw_mods_bootstrap_when_using_bootstraplauncher_main() {
        let temp = std::env::temp_dir().join(format!(
            "classpath-test-bootstraplauncher-skip-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp);

        let instance_dir = temp.join("instance");
        std::fs::create_dir_all(&instance_dir).unwrap();
        std::fs::write(instance_dir.join("client.jar"), b"client").unwrap();

        let mut instance = test_instance(&instance_dir);
        instance.loader = LoaderType::NeoForge;
        instance.main_class = Some("cpw.mods.bootstraplauncher.BootstrapLauncher".into());

        let libs_dir = temp.join("libraries");
        std::fs::create_dir_all(&libs_dir).unwrap();

        let sjh = MavenArtifact::parse("cpw.mods:securejarhandler:3.0.8").unwrap();
        let ml = MavenArtifact::parse("cpw.mods:modlauncher:11.0.5").unwrap();
        let jh = MavenArtifact::parse("cpw.mods:jarhandling:0.5.5").unwrap();
        for art in [&sjh, &ml, &jh] {
            let p = libs_dir.join(art.local_path());
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, b"x").unwrap();
        }

        let classpath = build_classpath(
            &instance,
            &libs_dir,
            &[
                "cpw.mods:securejarhandler:3.0.8".into(),
                "cpw.mods:modlauncher:11.0.5".into(),
                "cpw.mods:jarhandling:0.5.5".into(),
            ],
        )
        .unwrap();

        assert!(!classpath.contains("securejarhandler-3.0.8.jar"));
        assert!(!classpath.contains("modlauncher-11.0.5.jar"));
        assert!(!classpath.contains("jarhandling-0.5.5.jar"));

        let _ = std::fs::remove_dir_all(&temp);
    }
}
