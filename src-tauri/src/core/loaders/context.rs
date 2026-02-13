use std::path::Path;

use crate::core::downloader::Downloader;

/// Contexto completo de instalación.
/// Permite escalar sin romper la API.
pub struct InstallContext<'a> {
    pub minecraft_version: &'a str,
    pub loader_version: &'a str,
    pub instance_dir: &'a Path,
    pub libs_dir: &'a Path,
    pub downloader: &'a Downloader,
    pub http_client: &'a reqwest::Client,
}
