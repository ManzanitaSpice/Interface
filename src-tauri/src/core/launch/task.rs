// ─── Launch Task ───
// Spawns the Minecraft game process with the correct arguments.

use std::process::Stdio;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

use tracing::{debug, info};

use crate::core::auth::LaunchAccountProfile;
use crate::core::error::{LauncherError, LauncherResult};
use crate::core::instance::Instance;
use crate::core::java;

use super::classpath::safe_path_str;

/// Launch the game as a child process.
///
/// Returns immediately after spawning. The caller is responsible for monitoring
/// the child process and setting state back to `Ready` when it exits.
pub async fn launch(
    instance: &Instance,
    classpath: &str,
    libraries_dir: &std::path::Path,
) -> LauncherResult<std::process::Child> {
    let main_class = instance
        .main_class
        .as_deref()
        .ok_or_else(|| LauncherError::Other("Main class not set on instance".into()))?;

    // Determine Java binary
    let java_bin = match &instance.java_path {
        Some(p) if p.exists() => p.clone(),
        _ => {
            // Auto-detect based on version JSON requirements
            let java_major = instance.required_java_major.unwrap_or_else(|| {
                java::required_java_for_minecraft_version(&instance.minecraft_version)
            });
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
    cmd.arg(format!(
        "-DlibraryDirectory={}",
        safe_path_str(libraries_dir)
    ));
    cmd.arg("-Dminecraft.launcher.brand=InterfaceOficial");
    cmd.arg("-Dminecraft.launcher.version=0.1.0");

    // Extra JVM args from instance config or loader (normalized to avoid
    // dangling "-cp" without value and unresolved placeholders).
    let mut effective_jvm_args = sanitize_jvm_args(
        instance,
        &instance.jvm_args,
        &natives_dir,
        libraries_dir,
        classpath,
    );
    ensure_loader_jvm_workarounds(instance, &mut effective_jvm_args);

    for arg in effective_jvm_args {
        cmd.arg(arg);
    }

    // Classpath
    if classpath.trim().is_empty() {
        return Err(LauncherError::Other(
            "Classpath vacío: se cancela el arranque para evitar 'java -cp' inválido".into(),
        ));
    }
    debug!("Classpath len={} value={:?}", classpath.len(), classpath);
    info!("Classpath: {}", classpath);
    cmd.arg("-cp").arg(classpath);

    // Main class
    cmd.arg(main_class);

    // ── Game Arguments ──
    let final_game_args = sanitize_game_args(
        instance,
        &instance.game_args,
        &game_dir,
        &assets_dir,
        &instance.account,
    );

    for arg in final_game_args {
        cmd.arg(arg);
    }

    cmd.current_dir(&game_dir);
    configure_native_library_env(&mut cmd, &natives_dir);
    configure_platform_spawn(&mut cmd);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    info!("Launching Minecraft with Java: {:?}", java_bin);
    debug!("Command: {:?}", cmd);
    debug!("Command (copy/paste): {}", format_command_for_logs(&cmd));

    let child = cmd
        .spawn()
        .map_err(|e| LauncherError::JavaExecution(e.to_string()))?;

    Ok(child)
}

fn sanitize_jvm_args(
    instance: &Instance,
    raw_args: &[String],
    natives_dir: &std::path::Path,
    libraries_dir: &std::path::Path,
    classpath: &str,
) -> Vec<String> {
    let mut sanitized = Vec::new();
    let mut i = 0;
    let natives = safe_path_str(natives_dir);
    let game_dir = safe_path_str(&instance.game_dir());
    let library_dir = safe_path_str(libraries_dir);
    let classpath_separator = super::classpath::get_classpath_separator();
    let launch_version_name = launch_version_name(instance);
    let loader_version = instance.loader_version.as_deref().unwrap_or("");

    while i < raw_args.len() {
        let arg = &raw_args[i];

        // We always inject classpath ourselves later, so loader-provided
        // classpath switches must be dropped together with their value.
        if arg == "-cp" || arg == "-classpath" || arg == "--class-path" {
            i += 2;
            continue;
        }

        let resolved = arg
            .replace("${natives_directory}", &natives)
            .replace("${library_directory}", &library_dir)
            .replace("${classpath}", classpath)
            .replace("${classpath_separator}", classpath_separator)
            .replace("${game_directory}", &game_dir)
            .replace("${version_name}", &launch_version_name)
            .replace("${version}", loader_version)
            .replace("${mc_version}", &instance.minecraft_version)
            .replace("${launcher_name}", "InterfaceOficial")
            .replace("${launcher_version}", "0.1.0");

        // Any remaining placeholders indicate data we cannot currently resolve.
        // Skip to avoid passing invalid runtime arguments to Java.
        if resolved.contains("${") {
            drop_dangling_option(&mut sanitized);
            i += 1;
            continue;
        }

        sanitized.push(resolved);
        i += 1;
    }

    sanitized
}

fn sanitize_game_args(
    instance: &Instance,
    raw_args: &[String],
    game_dir: &std::path::Path,
    assets_dir: &std::path::Path,
    account: &LaunchAccountProfile,
) -> Vec<String> {
    let mut sanitized = Vec::new();
    let game_dir = safe_path_str(game_dir);
    let assets_dir = safe_path_str(assets_dir);
    let launch_version_name = launch_version_name(instance);
    let loader_version = instance.loader_version.as_deref().unwrap_or("");

    let mut i = 0;
    while i < raw_args.len() {
        let arg = &raw_args[i];

        let resolved = arg
            .replace("${auth_player_name}", &account.username)
            .replace("${version_name}", &launch_version_name)
            .replace("${version}", loader_version)
            .replace("${mc_version}", &instance.minecraft_version)
            .replace("${game_directory}", &game_dir)
            .replace("${assets_root}", &assets_dir)
            .replace(
                "${assets_index_name}",
                instance.asset_index.as_deref().unwrap_or("legacy"),
            )
            .replace("${auth_uuid}", &account.uuid)
            .replace("${auth_access_token}", &account.access_token)
            .replace("${auth_xuid}", &account.xuid)
            .replace("${clientid}", &account.client_id)
            .replace("${user_properties}", "{}")
            .replace("${user_type}", &account.user_type)
            .replace("${version_type}", "release");

        // Skip unresolved placeholders to avoid passing malformed values.
        if resolved.contains("${") {
            drop_dangling_option(&mut sanitized);
            i += 1;
            continue;
        }

        sanitized.push(resolved);
        i += 1;
    }

    let sanitized = sanitize_numeric_window_args(sanitized);
    ensure_required_fml_game_args(instance, sanitized)
}

fn ensure_required_fml_game_args(instance: &Instance, mut args: Vec<String>) -> Vec<String> {
    let needs_fml_args = matches!(
        instance.loader,
        crate::core::instance::LoaderType::Forge | crate::core::instance::LoaderType::NeoForge
    );

    if !needs_fml_args {
        return args;
    }

    if !contains_flag(&args, "--fml.mcVersion") {
        args.push("--fml.mcVersion".into());
        args.push(instance.minecraft_version.clone());
    }

    match instance.loader {
        crate::core::instance::LoaderType::Forge => {
            if let Some(loader_version) = instance.loader_version.as_deref() {
                if !loader_version.trim().is_empty() && !contains_flag(&args, "--fml.forgeVersion")
                {
                    args.push("--fml.forgeVersion".into());
                    args.push(loader_version.to_string());
                }
            }
        }
        crate::core::instance::LoaderType::NeoForge => {
            if let Some(loader_version) = instance.loader_version.as_deref() {
                if !loader_version.trim().is_empty()
                    && !contains_flag(&args, "--fml.neoForgeVersion")
                {
                    args.push("--fml.neoForgeVersion".into());
                    args.push(loader_version.to_string());
                }
            }
        }
        _ => {}
    }

    args
}

fn contains_flag(args: &[String], flag: &str) -> bool {
    args.iter().any(|arg| arg == flag)
}

fn sanitize_numeric_window_args(args: Vec<String>) -> Vec<String> {
    let mut sanitized = Vec::with_capacity(args.len());
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        if arg == "--width" || arg == "--height" {
            let Some(value) = args.get(i + 1) else {
                i += 1;
                continue;
            };

            if value.starts_with('-') || value.parse::<u32>().is_err() {
                i += 1;
                continue;
            }

            sanitized.push(arg.clone());
            sanitized.push(value.clone());
            i += 2;
            continue;
        }

        sanitized.push(arg.clone());
        i += 1;
    }

    sanitized
}

fn launch_version_name(instance: &Instance) -> String {
    match instance.loader_version.as_deref() {
        Some(loader_version) if !loader_version.trim().is_empty() => {
            format!("{}-{}", instance.minecraft_version, loader_version)
        }
        _ => instance.minecraft_version.clone(),
    }
}

fn drop_dangling_option(args: &mut Vec<String>) {
    if args.last().is_some_and(|last| last.starts_with('-')) {
        let _ = args.pop();
    }
}

fn ensure_loader_jvm_workarounds(instance: &Instance, args: &mut Vec<String>) {
    let is_forge_like = matches!(
        instance.loader,
        crate::core::instance::LoaderType::Forge | crate::core::instance::LoaderType::NeoForge
    );

    if !is_forge_like {
        return;
    }

    if java::required_java_for_minecraft_version(&instance.minecraft_version) >= 17 {
        ensure_modern_forge_jvm_args(args);
    }

    if !matches!(instance.loader, crate::core::instance::LoaderType::NeoForge) {
        return;
    }

    // NeoForge relies on Java modules that are not always enabled by default
    // in third-party launchers. Keep these flags present even when profile
    // metadata is incomplete.
    ensure_jvm_arg_present(args, "--add-modules=jdk.naming.dns");
    ensure_jvm_arg_present(args, "--add-opens=java.base/java.util.jar=ALL-UNNAMED");
    set_jvm_system_property(args, "ignoreList", "bootstraplauncher,neon-fml");

    // Workaround for crashes in NeoForge Early Display (`rendererFuture` null)
    // seen on some GPU/overlay setups. Disabling the early progress window lets
    // the game continue with the normal LWJGL window initialization path.
    set_jvm_system_property(args, "fml.earlyprogresswindow", "false");
    // Legacy namespace still appears in some Forge/NeoForge metadata.
    set_jvm_system_property(args, "forge.earlywindow", "false");

    // Newer NeoForge builds also support this namespace. Keeping both avoids
    // requiring users to manually tweak launch options per loader version.
    set_jvm_system_property(args, "neoforge.earlydisplay", "false");
}

fn modern_forge_jvm_arg_pairs() -> Vec<(&'static str, &'static str)> {
    vec![
        ("--add-modules", "ALL-SYSTEM"),
        ("--add-opens", "java.base/java.util.jar=ALL-UNNAMED"),
        ("--add-opens", "java.base/java.lang=ALL-UNNAMED"),
        ("--add-opens", "java.base/java.util=ALL-UNNAMED"),
        ("--add-opens", "java.base/java.lang.invoke=ALL-UNNAMED"),
        ("--add-opens", "java.base/java.lang.reflect=ALL-UNNAMED"),
        ("--add-opens", "java.base/java.nio.file=ALL-UNNAMED"),
        ("--add-opens", "java.base/sun.security.util=ALL-UNNAMED"),
        ("--add-exports", "java.base/sun.security.action=ALL-UNNAMED"),
        ("--add-opens", "java.base/java.io=ALL-UNNAMED"),
        ("--add-opens", "java.base/java.net=ALL-UNNAMED"),
        ("--add-opens", "java.base/sun.nio.ch=ALL-UNNAMED"),
    ]
}

fn ensure_modern_forge_jvm_args(args: &mut Vec<String>) {
    for (flag, value) in modern_forge_jvm_arg_pairs() {
        ensure_jvm_arg_pair_present(args, flag, value);
    }
}

fn ensure_jvm_arg_pair_present(args: &mut Vec<String>, flag: &str, value: &str) {
    let combined = format!("{}={}", flag, value);
    if args.iter().any(|arg| arg == &combined) {
        return;
    }

    let mut i = 0;
    while i + 1 < args.len() {
        if args[i] == flag && args[i + 1] == value {
            return;
        }
        i += 1;
    }

    args.push(flag.to_string());
    args.push(value.to_string());
}

fn ensure_jvm_arg_present(args: &mut Vec<String>, flag_with_value: &str) {
    if args.iter().any(|arg| arg == flag_with_value) {
        return;
    }

    args.push(flag_with_value.to_string());
}

fn set_jvm_system_property(args: &mut Vec<String>, property: &str, value: &str) {
    let prefix = format!("-D{}=", property);
    args.retain(|arg| !arg.starts_with(&prefix));
    args.push(format!("{}{}", prefix, value));
}

fn configure_native_library_env(cmd: &mut std::process::Command, natives_dir: &std::path::Path) {
    let native_path = safe_path_str(natives_dir);

    if cfg!(target_os = "windows") {
        let merged = append_env_path("PATH", &native_path);
        cmd.env("PATH", merged);
    } else if cfg!(target_os = "linux") {
        let merged = append_env_path("LD_LIBRARY_PATH", &native_path);
        cmd.env("LD_LIBRARY_PATH", merged);
    } else if cfg!(target_os = "macos") {
        let merged = append_env_path("DYLD_LIBRARY_PATH", &native_path);
        cmd.env("DYLD_LIBRARY_PATH", merged);
    }
}

fn configure_platform_spawn(cmd: &mut std::process::Command) {
    #[cfg(target_os = "windows")]
    {
        const CREATE_NEW_CONSOLE: u32 = 0x00000010;
        cmd.creation_flags(CREATE_NEW_CONSOLE);

        // Tauri/terminal-related vars can make Java/LWJGL treat the process as
        // a virtual terminal session. Drop the most common ones to keep the
        // child process environment closer to a standard desktop launch.
        cmd.env_remove("WT_SESSION");
        cmd.env_remove("TERM");
        cmd.env_remove("ConEmuANSI");
    }
}

fn append_env_path(var_name: &str, value: &str) -> String {
    let separator = if cfg!(target_os = "windows") {
        ";"
    } else {
        ":"
    };
    match std::env::var(var_name) {
        Ok(existing) if !existing.trim().is_empty() => {
            format!("{}{}{}", value, separator, existing)
        }
        _ => value.to_string(),
    }
}

fn format_command_for_logs(cmd: &std::process::Command) -> String {
    let program = shell_escape(&cmd.get_program().to_string_lossy());
    let args = cmd
        .get_args()
        .map(|arg| shell_escape(&arg.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(" ");

    if args.is_empty() {
        program
    } else {
        format!("{} {}", program, args)
    }
}

fn shell_escape(raw: &str) -> String {
    if raw.is_empty() {
        return "\"\"".to_string();
    }

    if raw.chars().all(|ch| {
        ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '/' | ':' | '\\' | '=')
    }) {
        return raw.to_string();
    }

    format!("\"{}\"", raw.replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn java_major_detection() {
        assert_eq!(java::required_java_for_minecraft_version("1.21.4"), 21);
        assert_eq!(java::required_java_for_minecraft_version("1.21"), 21);
        assert_eq!(java::required_java_for_minecraft_version("1.20.4"), 17);
        assert_eq!(java::required_java_for_minecraft_version("1.16.5"), 8);
        assert_eq!(java::required_java_for_minecraft_version("1.8.9"), 8);
    }

    #[test]
    fn sanitize_jvm_args_removes_external_classpath_and_unresolved_tokens() {
        let natives = std::path::PathBuf::from("/tmp/natives");
        let args = vec![
            "-XX:+UseG1GC".to_string(),
            "-cp".to_string(),
            "${classpath}".to_string(),
            "-Djava.library.path=${natives_directory}".to_string(),
            "--class-path".to_string(),
            "/tmp/wrong.jar".to_string(),
            "-Dsomething=${unknown_placeholder}".to_string(),
        ];

        let mut instance = Instance::new(
            "test".into(),
            "1.20.1".into(),
            crate::core::instance::LoaderType::Vanilla,
            None,
            2048,
            std::path::Path::new("/tmp"),
        );
        instance.path = std::path::PathBuf::from("/tmp/test-instance");

        let sanitized = sanitize_jvm_args(
            &instance,
            &args,
            &natives,
            std::path::Path::new("/tmp/libraries"),
            "/tmp/classpath.jar",
        );

        assert_eq!(sanitized.len(), 2);
        assert_eq!(sanitized[0], "-XX:+UseG1GC");
        assert_eq!(sanitized[1], "-Djava.library.path=/tmp/natives");
    }

    #[test]
    fn sanitize_game_args_resolves_known_placeholders_and_drops_unknown() {
        let mut instance = Instance::new(
            "test".into(),
            "1.20.1".into(),
            crate::core::instance::LoaderType::Vanilla,
            None,
            2048,
            std::path::Path::new("/tmp"),
        );
        instance.path = std::path::PathBuf::from("/tmp/test-instance");
        instance.asset_index = Some("17".into());

        let args = vec![
            "--username".into(),
            "${auth_player_name}".into(),
            "--accessToken".into(),
            "${auth_access_token}".into(),
            "--userType".into(),
            "${user_type}".into(),
            "--xuid".into(),
            "${auth_xuid}".into(),
            "--clientId".into(),
            "${clientid}".into(),
            "--assetIndex".into(),
            "${assets_index_name}".into(),
            "--bad".into(),
            "${unknown_placeholder}".into(),
        ];

        instance.account = LaunchAccountProfile::offline("Alex").sanitized();

        let sanitized = sanitize_game_args(
            &instance,
            &args,
            std::path::Path::new("/tmp/game"),
            std::path::Path::new("/tmp/assets"),
            &instance.account,
        );

        assert_eq!(
            sanitized,
            vec![
                "--username",
                "Alex",
                "--accessToken",
                &instance.account.access_token,
                "--userType",
                "legacy",
                "--xuid",
                "0",
                "--clientId",
                "00000000402B5328",
                "--assetIndex",
                "17",
            ]
        );
    }

    #[test]
    fn sanitize_game_args_drops_dangling_option_for_unresolved_placeholder() {
        let mut instance = Instance::new(
            "test".into(),
            "1.20.1".into(),
            crate::core::instance::LoaderType::Forge,
            Some("47.2.0".into()),
            2048,
            std::path::Path::new("/tmp"),
        );
        instance.path = std::path::PathBuf::from("/tmp/test-instance");
        instance.account = LaunchAccountProfile::offline("Alex").sanitized();

        let args = vec![
            "--fml.forgeVersion".into(),
            "${missing_forge_version}".into(),
            "--fml.mcVersion".into(),
            "${mc_version}".into(),
        ];

        let sanitized = sanitize_game_args(
            &instance,
            &args,
            std::path::Path::new("/tmp/game"),
            std::path::Path::new("/tmp/assets"),
            &instance.account,
        );

        assert_eq!(sanitized, vec!["--fml.mcVersion", "1.20.1"]);
    }

    #[test]
    fn sanitize_game_args_drops_invalid_window_size_pairs() {
        let mut instance = Instance::new(
            "test".into(),
            "1.20.1".into(),
            crate::core::instance::LoaderType::NeoForge,
            Some("20.4.1-beta".into()),
            2048,
            std::path::Path::new("/tmp"),
        );
        instance.path = std::path::PathBuf::from("/tmp/test-instance");
        instance.account = LaunchAccountProfile::offline("Alex").sanitized();

        let args = vec![
            "--width".into(),
            "--height".into(),
            "480".into(),
            "--height".into(),
            "720".into(),
        ];

        let sanitized = sanitize_game_args(
            &instance,
            &args,
            std::path::Path::new("/tmp/game"),
            std::path::Path::new("/tmp/assets"),
            &instance.account,
        );

        assert_eq!(sanitized, vec!["--height", "720"]);
    }

    #[test]
    fn sanitize_game_args_injects_required_neoforge_fml_versions() {
        let mut instance = Instance::new(
            "test".into(),
            "1.20.1".into(),
            crate::core::instance::LoaderType::NeoForge,
            Some("47.1.79".into()),
            2048,
            std::path::Path::new("/tmp"),
        );
        instance.path = std::path::PathBuf::from("/tmp/test-instance");
        instance.account = LaunchAccountProfile::offline("Alex").sanitized();

        let sanitized = sanitize_game_args(
            &instance,
            &Vec::new(),
            std::path::Path::new("/tmp/game"),
            std::path::Path::new("/tmp/assets"),
            &instance.account,
        );

        assert_eq!(
            sanitized,
            vec![
                "--fml.mcVersion",
                "1.20.1",
                "--fml.neoForgeVersion",
                "47.1.79"
            ]
        );
    }

    #[test]
    fn sanitize_game_args_does_not_duplicate_existing_fml_flags() {
        let mut instance = Instance::new(
            "test".into(),
            "1.20.1".into(),
            crate::core::instance::LoaderType::NeoForge,
            Some("47.1.79".into()),
            2048,
            std::path::Path::new("/tmp"),
        );
        instance.path = std::path::PathBuf::from("/tmp/test-instance");
        instance.account = LaunchAccountProfile::offline("Alex").sanitized();

        let args = vec![
            "--fml.mcVersion".into(),
            "${mc_version}".into(),
            "--fml.neoForgeVersion".into(),
            "${version}".into(),
        ];

        let sanitized = sanitize_game_args(
            &instance,
            &args,
            std::path::Path::new("/tmp/game"),
            std::path::Path::new("/tmp/assets"),
            &instance.account,
        );

        assert_eq!(
            sanitized,
            vec![
                "--fml.mcVersion",
                "1.20.1",
                "--fml.neoForgeVersion",
                "47.1.79"
            ]
        );
    }

    #[test]
    fn ensure_loader_jvm_workarounds_adds_neoforge_early_window_flag_once() {
        let mut instance = Instance::new(
            "test".into(),
            "1.20.1".into(),
            crate::core::instance::LoaderType::NeoForge,
            Some("47.1.79".into()),
            2048,
            std::path::Path::new("/tmp"),
        );
        instance.path = std::path::PathBuf::from("/tmp/test-instance");

        let mut args = vec!["-Xmx2048M".to_string()];
        ensure_loader_jvm_workarounds(&instance, &mut args);
        ensure_loader_jvm_workarounds(&instance, &mut args);

        assert!(args.contains(&"-Xmx2048M".to_string()));
        assert!(args.contains(&"--add-modules".to_string()));
        assert!(args.contains(&"ALL-SYSTEM".to_string()));
        assert!(args.contains(&"-DignoreList=bootstraplauncher,neon-fml".to_string()));
        assert!(args.contains(&"-Dfml.earlyprogresswindow=false".to_string()));
        assert!(args.contains(&"-Dforge.earlywindow=false".to_string()));
        assert!(args.contains(&"-Dneoforge.earlydisplay=false".to_string()));
    }

    #[test]
    fn append_env_path_prefixes_new_value() {
        let merged = append_env_path("THIS_ENV_VAR_SHOULD_NOT_EXIST", "/tmp/natives");
        assert_eq!(merged, "/tmp/natives");

        std::env::set_var("IFACE_TEST_PATH", "C:/Windows/System32");
        let merged = append_env_path("IFACE_TEST_PATH", "C:/Game/natives");
        let expected_sep = if cfg!(target_os = "windows") {
            ";"
        } else {
            ":"
        };
        assert_eq!(
            merged,
            format!("C:/Game/natives{}C:/Windows/System32", expected_sep)
        );
        std::env::remove_var("IFACE_TEST_PATH");
    }

    #[test]
    fn neoforge_workarounds_inject_module_flags_and_early_display_flags() {
        let mut instance = Instance::new(
            "test".into(),
            "1.20.1".into(),
            crate::core::instance::LoaderType::NeoForge,
            Some("47.1.79".into()),
            2048,
            std::path::Path::new("/tmp"),
        );
        instance.path = std::path::PathBuf::from("/tmp/test-instance");

        let mut args = vec!["-Xmx2G".to_string()];
        ensure_loader_jvm_workarounds(&instance, &mut args);

        assert!(args.contains(&"--add-modules".to_string()));
        assert!(args.contains(&"ALL-SYSTEM".to_string()));
        assert!(args.contains(&"--add-opens".to_string()));
        assert!(args.contains(&"java.base/java.lang.reflect=ALL-UNNAMED".to_string()));
        assert!(args.contains(&"--add-modules=jdk.naming.dns".to_string()));
        assert!(args.contains(&"-DignoreList=bootstraplauncher,neon-fml".to_string()));
        assert!(args.contains(&"-Dfml.earlyprogresswindow=false".to_string()));
        assert!(args.contains(&"-Dforge.earlywindow=false".to_string()));
        assert!(args.contains(&"-Dneoforge.earlydisplay=false".to_string()));
    }

    #[test]
    fn forge_workarounds_inject_modern_module_opens_for_java_17_plus() {
        let mut instance = Instance::new(
            "test".into(),
            "1.20.1".into(),
            crate::core::instance::LoaderType::Forge,
            Some("47.2.0".into()),
            2048,
            std::path::Path::new("/tmp"),
        );
        instance.path = std::path::PathBuf::from("/tmp/test-instance");

        let mut args = vec!["-Xmx2G".to_string()];
        ensure_loader_jvm_workarounds(&instance, &mut args);

        assert!(args.contains(&"--add-modules".to_string()));
        assert!(args.contains(&"ALL-SYSTEM".to_string()));
        assert!(args.contains(&"--add-opens".to_string()));
        assert!(args.contains(&"java.base/java.lang.reflect=ALL-UNNAMED".to_string()));
        assert!(!args.contains(&"-DignoreList=bootstraplauncher,neon-fml".to_string()));
    }

    #[test]
    fn neoforge_workarounds_override_conflicting_early_window_properties() {
        let mut instance = Instance::new(
            "test".into(),
            "1.20.1".into(),
            crate::core::instance::LoaderType::NeoForge,
            Some("47.1.79".into()),
            2048,
            std::path::Path::new("/tmp"),
        );
        instance.path = std::path::PathBuf::from("/tmp/test-instance");

        let mut args = vec![
            "-DignoreList=something,else".to_string(),
            "-Dfml.earlyprogresswindow=true".to_string(),
            "-Dforge.earlywindow=true".to_string(),
            "-Dneoforge.earlydisplay=true".to_string(),
        ];
        ensure_loader_jvm_workarounds(&instance, &mut args);

        assert_eq!(
            args.iter()
                .filter(|arg| arg.starts_with("-DignoreList="))
                .count(),
            1
        );
        assert_eq!(
            args.iter()
                .filter(|arg| arg.starts_with("-Dfml.earlyprogresswindow="))
                .count(),
            1
        );
        assert_eq!(
            args.iter()
                .filter(|arg| arg.starts_with("-Dforge.earlywindow="))
                .count(),
            1
        );
        assert_eq!(
            args.iter()
                .filter(|arg| arg.starts_with("-Dneoforge.earlydisplay="))
                .count(),
            1
        );
        assert!(args.contains(&"-DignoreList=bootstraplauncher,neon-fml".to_string()));
        assert!(args.contains(&"-Dfml.earlyprogresswindow=false".to_string()));
        assert!(args.contains(&"-Dforge.earlywindow=false".to_string()));
        assert!(args.contains(&"-Dneoforge.earlydisplay=false".to_string()));
    }

    #[test]
    fn neoforge_workarounds_keep_existing_module_values() {
        let mut instance = Instance::new(
            "test".into(),
            "1.20.1".into(),
            crate::core::instance::LoaderType::NeoForge,
            Some("47.1.79".into()),
            2048,
            std::path::Path::new("/tmp"),
        );
        instance.path = std::path::PathBuf::from("/tmp/test-instance");

        let mut args = vec![
            "--add-modules=java.naming".to_string(),
            "--add-opens=java.base/java.lang=ALL-UNNAMED".to_string(),
        ];
        ensure_loader_jvm_workarounds(&instance, &mut args);

        assert!(args.contains(&"--add-modules=java.naming".to_string()));
        assert!(args.contains(&"--add-modules".to_string()));
        assert!(args.contains(&"ALL-SYSTEM".to_string()));
        assert!(args.contains(&"--add-modules=jdk.naming.dns".to_string()));
        assert!(args.contains(&"--add-opens=java.base/java.lang=ALL-UNNAMED".to_string()));
        assert!(args.contains(&"java.base/java.util.jar=ALL-UNNAMED".to_string()));
    }
}
