use std::collections::HashSet;
use std::path::Path;

use tracing::{debug, warn};

use super::artifact::MavenArtifact;
use super::pom::PomDocument;
use crate::core::downloader::Downloader;
use crate::core::error::{LauncherError, LauncherResult};

/// Resolves Maven artifacts transitively, downloading JARs and parsing POMs.
pub struct MavenResolver {
    /// Ordered list of repository base URLs to search.
    pub repositories: Vec<String>,
    /// Artifacts already resolved in this session (avoid cycles).
    resolved: HashSet<String>,
}

impl MavenResolver {
    pub fn new(repositories: Vec<String>) -> Self {
        Self {
            repositories,
            resolved: HashSet::new(),
        }
    }

    /// Resolve a single artifact coordinate.
    ///
    /// If the artifact is a POM, it will be downloaded, parsed, and its
    /// compile-scope dependencies will be resolved recursively.
    ///
    /// Returns the list of local file paths written (JARs only).
    pub async fn resolve(
        &mut self,
        coord: &str,
        libs_dir: &Path,
        downloader: &Downloader,
    ) -> LauncherResult<Vec<std::path::PathBuf>> {
        let artifact = MavenArtifact::parse(coord)?;
        self.resolve_artifact(&artifact, libs_dir, downloader).await
    }

    /// Internal recursive resolver.
    fn resolve_artifact<'a>(
        &'a mut self,
        artifact: &'a MavenArtifact,
        libs_dir: &'a Path,
        downloader: &'a Downloader,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = LauncherResult<Vec<std::path::PathBuf>>> + Send + 'a>> {
        Box::pin(async move {
        let key = artifact.to_string();
        if self.resolved.contains(&key) {
            return Ok(vec![]);
        }
        self.resolved.insert(key.clone());

        let mut collected = Vec::new();

        // 1. Try to download the JAR (skip for pom-only packaging)
        if !artifact.is_pom() {
            let dest = libs_dir.join(artifact.local_path());
            if !dest.exists() {
                let downloaded = self
                    .try_download(artifact, &dest, downloader)
                    .await;
                match downloaded {
                    Ok(()) => {
                        debug!("Downloaded JAR: {}", artifact);
                        collected.push(dest);
                    }
                    Err(e) => {
                        warn!("JAR download failed for {}: {}", artifact, e);
                        // It might be a POM-only artifact. Fall through to POM resolution.
                    }
                }
            } else {
                collected.push(dest);
            }
        }

        // 2. Download and parse POM for transitive dependencies
        let pom_artifact = artifact.with_packaging("pom");
        let pom_dest = libs_dir.join(pom_artifact.local_path());

        if !pom_dest.exists() {
            if let Err(e) = self.try_download(&pom_artifact, &pom_dest, downloader).await {
                // POM not available is non-fatal for many Mojang libs
                debug!("POM not available for {}: {}", artifact, e);
                return Ok(collected);
            }
        }

        // Read and parse POM
        let pom_content = tokio::fs::read_to_string(&pom_dest).await.map_err(|e| {
            LauncherError::Io {
                path: pom_dest.clone(),
                source: e,
            }
        })?;

        let pom = match PomDocument::parse(&pom_content) {
            Ok(p) => p,
            Err(e) => {
                warn!("Failed to parse POM for {}: {}", artifact, e);
                return Ok(collected);
            }
        };

        // 3. Resolve compile-scope transitive dependencies
        for dep in pom.compile_dependencies() {
            let version = match pom.resolve_version(&dep) {
                Some(v) => v,
                None => {
                    warn!(
                        "Cannot resolve version for {}:{} (skipping)",
                        dep.group_id, dep.artifact_id
                    );
                    continue;
                }
            };

            let dep_packaging = dep.dep_type.as_deref().unwrap_or("jar");
            let coord = match &dep.classifier {
                Some(c) => format!(
                    "{}:{}:{}:{}@{}",
                    dep.group_id, dep.artifact_id, version, c, dep_packaging
                ),
                None => format!(
                    "{}:{}:{}@{}",
                    dep.group_id, dep.artifact_id, version, dep_packaging
                ),
            };

            let child = MavenArtifact::parse(&coord)?;
            let child_paths = self
                .resolve_artifact(&child, libs_dir, downloader)
                .await?;
            collected.extend(child_paths);
        }

        Ok(collected)
        }) // end Box::pin
    }

    /// Try each repository until a successful download occurs.
    async fn try_download(
        &self,
        artifact: &MavenArtifact,
        dest: &Path,
        downloader: &Downloader,
    ) -> LauncherResult<()> {
        let mut last_err: Option<LauncherError> = None;

        for repo in &self.repositories {
            let url = artifact.url(repo);
            match downloader.download_file(&url, dest, None).await {
                Ok(()) => return Ok(()),
                Err(e) => {
                    debug!("Repository {} failed for {}: {}", repo, artifact, e);
                    last_err = Some(e);
                }
            }
        }

        Err(last_err.unwrap_or_else(|| {
            LauncherError::Other(format!("No repositories configured for {}", artifact))
        }))
    }
}
