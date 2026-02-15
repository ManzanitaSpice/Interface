use std::path::PathBuf;
use thiserror::Error;

/// Central error type for the entire launcher backend.
/// Every module returns `Result<T, LauncherError>`.
#[derive(Debug, Error)]
pub enum LauncherError {
    // ── IO ──────────────────────────────────────────────
    #[error("IO error at {path:?}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },

    // ── Network ─────────────────────────────────────────
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Download failed for {url}: HTTP {status}")]
    DownloadFailed { url: String, status: u16 },

    // ── Integrity ───────────────────────────────────────
    #[error("SHA-1 mismatch for {path:?}: expected {expected}, got {actual}")]
    Sha1Mismatch {
        path: PathBuf,
        expected: String,
        actual: String,
    },

    // ── Maven ───────────────────────────────────────────
    #[error("Invalid Maven coordinate: {0}")]
    InvalidMavenCoordinate(String),

    #[error("POM parse error: {0}")]
    PomParse(String),

    // ── XML ─────────────────────────────────────────────
    #[error("XML parse error: {0}")]
    Xml(#[from] quick_xml::DeError),

    // ── JSON ────────────────────────────────────────────
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    // ── Instance ────────────────────────────────────────
    #[error("Instance not found: {0}")]
    InstanceNotFound(String),

    #[error("Instance already exists: {0}")]
    InstanceAlreadyExists(String),

    // ── Java ────────────────────────────────────────────
    #[error("Java not found for major version {0}")]
    JavaNotFound(u32),

    #[error("Java execution failed: {0}")]
    JavaExecution(String),

    // ── Loader ──────────────────────────────────────────
    #[error("Loader error: {0}")]
    Loader(String),

    #[error("Loader API unreachable: {0}")]
    LoaderApi(String),

    // ── Archive ─────────────────────────────────────────
    #[error("Zip extraction error: {0}")]
    Zip(#[from] zip::result::ZipError),

    // ── Generic ─────────────────────────────────────────
    #[error("{0}")]
    Other(String),
}

/// Convenience alias used throughout the crate.
pub type LauncherResult<T> = Result<T, LauncherError>;

impl From<std::io::Error> for LauncherError {
    fn from(source: std::io::Error) -> Self {
        LauncherError::Io {
            path: PathBuf::new(),
            source,
        }
    }
}

// ── Serialization for Tauri IPC ─────────────────────────
// Tauri commands require the error type to implement `Serialize`.
impl serde::Serialize for LauncherError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;

        let mut map = serializer.serialize_map(Some(5))?;
        map.serialize_entry("message", &self.to_string())?;
        map.serialize_entry("i18n_key", self.i18n_key())?;
        map.serialize_entry("severity", self.severity())?;
        map.serialize_entry("recoverable", &self.is_recoverable())?;
        map.serialize_entry("kind", self.kind())?;
        map.end()
    }
}

impl LauncherError {
    pub fn i18n_key(&self) -> &'static str {
        match self {
            LauncherError::Io { .. } => "error.io",
            LauncherError::Http(_) => "error.http",
            LauncherError::DownloadFailed { .. } => "error.download_failed",
            LauncherError::Sha1Mismatch { .. } => "error.sha1_mismatch",
            LauncherError::InvalidMavenCoordinate(_) => "error.invalid_maven_coordinate",
            LauncherError::PomParse(_) => "error.pom_parse",
            LauncherError::Xml(_) => "error.xml",
            LauncherError::Json(_) => "error.json",
            LauncherError::InstanceNotFound(_) => "error.instance_not_found",
            LauncherError::InstanceAlreadyExists(_) => "error.instance_already_exists",
            LauncherError::JavaNotFound(_) => "error.java_not_found",
            LauncherError::JavaExecution(_) => "error.java_execution",
            LauncherError::Loader(_) => "error.loader",
            LauncherError::LoaderApi(_) => "error.loader_api",
            LauncherError::Zip(_) => "error.zip",
            LauncherError::Other(_) => "error.other",
        }
    }

    pub fn severity(&self) -> &'static str {
        if self.is_recoverable() {
            "recoverable"
        } else {
            "fatal"
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            LauncherError::Io { .. } => "io",
            LauncherError::Http(_) | LauncherError::DownloadFailed { .. } => "network",
            LauncherError::Sha1Mismatch { .. } => "integrity",
            LauncherError::InvalidMavenCoordinate(_) | LauncherError::PomParse(_) => "maven",
            LauncherError::Xml(_) | LauncherError::Json(_) => "parsing",
            LauncherError::InstanceNotFound(_) | LauncherError::InstanceAlreadyExists(_) => {
                "instance"
            }
            LauncherError::JavaNotFound(_) | LauncherError::JavaExecution(_) => "java",
            LauncherError::Loader(_) | LauncherError::LoaderApi(_) => "loader",
            LauncherError::Zip(_) => "archive",
            LauncherError::Other(_) => "generic",
        }
    }

    pub fn is_recoverable(&self) -> bool {
        matches!(
            self,
            LauncherError::Http(_)
                | LauncherError::DownloadFailed { .. }
                | LauncherError::LoaderApi(_)
                | LauncherError::Io { .. }
                | LauncherError::JavaNotFound(_)
        )
    }
}
