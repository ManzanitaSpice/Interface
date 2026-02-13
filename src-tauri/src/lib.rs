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
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,interface_lib=debug")),
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
            commands::get_loader_versions,
            commands::create_instance,
            commands::list_instances,
            commands::delete_instance,
            commands::launch_instance,
            commands::open_instance_folder,
            commands::get_java_installations,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
