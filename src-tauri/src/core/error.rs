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
        serializer.serialize_str(&self.to_string())
    }
}
