use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::core::error::{LauncherError, LauncherResult};

const APP_DIR_NAME: &str = "InterfaceOficial";

#[derive(Debug, Clone)]
pub struct RuntimePaths {
    app_data_dir: PathBuf,
    resource_dir: PathBuf,
    temp_dir: PathBuf,
}

impl RuntimePaths {
    pub fn app_data_dir(&self) -> &Path {
        &self.app_data_dir
    }

    pub fn resource_dir(&self) -> &Path {
        &self.resource_dir
    }

    pub fn temp_dir(&self) -> &Path {
        &self.temp_dir
    }
}

static RUNTIME_PATHS: OnceLock<RuntimePaths> = OnceLock::new();

pub fn runtime_paths() -> LauncherResult<&'static RuntimePaths> {
    if let Some(paths) = RUNTIME_PATHS.get() {
        return Ok(paths);
    }

    let data_dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(APP_DIR_NAME);

    let temp_dir = std::env::temp_dir().join(APP_DIR_NAME);
    let resource_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources");

    let canonical_data = canonical_or_create_dir(&data_dir)?;
    let canonical_temp = canonical_or_create_dir(&temp_dir)?;
    let canonical_resource = canonical_or_create_dir(&resource_dir)?;

    let paths = RuntimePaths {
        app_data_dir: canonical_data,
        resource_dir: canonical_resource,
        temp_dir: canonical_temp,
    };

    let _ = RUNTIME_PATHS.set(paths);
    Ok(RUNTIME_PATHS.get().expect("runtime paths set"))
}

fn canonical_or_create_dir(path: &Path) -> LauncherResult<PathBuf> {
    std::fs::create_dir_all(path).map_err(|source| LauncherError::Io {
        path: path.to_path_buf(),
        source,
    })?;

    std::fs::canonicalize(path).map_err(|source| LauncherError::Io {
        path: path.to_path_buf(),
        source,
    })
}
