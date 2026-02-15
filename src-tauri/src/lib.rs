mod commands;
mod core;

use std::sync::Arc;
use tauri::Manager;
use tokio::sync::Mutex;
use tracing_subscriber::EnvFilter;

use crate::core::state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Initialize structured logging
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    tracing::info!("InterfaceOficial launcher starting...");

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let handle = app.handle().clone();
            let state = AppState::new(handle);
            app.manage(Arc::new(Mutex::new(state)));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_minecraft_versions,
            commands::get_minecraft_versions_detailed,
            commands::get_loader_versions,
            commands::create_instance,
            commands::list_instances,
            commands::delete_instance,
            commands::delete_instance_with_elevation,
            commands::clone_instance,
            commands::launch_instance,
            commands::force_close_instance,
            commands::open_instance_folder,
            commands::get_java_installations,
            commands::get_java_metadata,
            commands::get_required_java_version,
            commands::install_managed_java,
            commands::get_java_info,
            commands::check_java_binary,
            commands::get_first_launch_status,
            commands::initialize_launcher_installation,
            commands::reinstall_launcher_completely,
            commands::get_launcher_settings,
            commands::update_launcher_settings,
            commands::migrate_launcher_data_dir,
            commands::update_instance_launch_config,
            commands::optimize_instance_with_real_process,
            commands::update_instance_account,
            commands::get_auth_research_info,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
