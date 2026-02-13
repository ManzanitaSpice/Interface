use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;

use crate::core::error::{LauncherError, LauncherResult};

/// Represents a fully parsed Maven coordinate.
///
/// Supported formats:
///   `groupId:artifactId:version`
///   `groupId:artifactId:version:classifier`
///   `groupId:artifactId:version:classifier@packaging`
///   `groupId:artifactId:version@packaging`
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct MavenArtifact {
    pub group_id: String,
    pub artifact_id: String,
    pub version: String,
    pub classifier: Option<String>,
    /// File extension / packaging type. Defaults to `"jar"`.
    pub packaging: String,
}

impl MavenArtifact {
    /// Parse a Maven coordinate string.
    ///
    /// # Examples
    /// ```
    /// let a = MavenArtifact::parse("net.sf.jopt-simple:jopt-simple:5.0.4").unwrap();
    /// assert_eq!(a.group_id, "net.sf.jopt-simple");
    /// ```
    pub fn parse(coord: &str) -> LauncherResult<Self> {
        // Split off @packaging first
        let (coord_part, packaging_override) = if let Some(idx) = coord.rfind('@') {
            (&coord[..idx], Some(&coord[idx + 1..]))
        } else {
            (coord, None)
        };

        let parts: Vec<&str> = coord_part.split(':').collect();

        match parts.len() {
            3 => Ok(Self {
                group_id: parts[0].to_string(),
                artifact_id: parts[1].to_string(),
                version: parts[2].to_string(),
                classifier: None,
                packaging: packaging_override.unwrap_or("jar").to_string(),
            }),
            4 => Ok(Self {
                group_id: parts[0].to_string(),
                artifact_id: parts[1].to_string(),
                version: parts[2].to_string(),
                classifier: Some(parts[3].to_string()),
                packaging: packaging_override.unwrap_or("jar").to_string(),
            }),
            _ => Err(LauncherError::InvalidMavenCoordinate(coord.to_string())),
        }
    }

    /// Construct the group path portion (`net/sf/jopt-simple`).
    pub fn group_path(&self) -> String {
        self.group_id.replace('.', "/")
    }

    /// Build the artifact filename.
    ///
    /// `artifactId-version[-classifier].packaging`
    pub fn filename(&self) -> String {
        match &self.classifier {
            Some(c) => format!(
                "{}-{}-{}.{}",
                self.artifact_id, self.version, c, self.packaging
            ),
            None => format!("{}-{}.{}", self.artifact_id, self.version, self.packaging),
        }
    }

    /// Construct the full URL for this artifact under the given repository base.
    ///
    /// Template:
    /// `<repo>/<group_path>/<artifact_id>/<version>/<filename>`
    pub fn url(&self, repo_base: &str) -> String {
        let base = repo_base.trim_end_matches('/');
        format!(
            "{}/{}/{}/{}/{}",
            base,
            self.group_path(),
            self.artifact_id,
            self.version,
            self.filename()
        )
    }

    /// Local path relative to the libraries directory.
    ///
    /// Mirrors Maven's local repo layout:
    /// `<group_path>/<artifact_id>/<version>/<filename>`
    pub fn local_path(&self) -> PathBuf {
        PathBuf::from(self.group_path())
            .join(&self.artifact_id)
            .join(&self.version)
            .join(self.filename())
    }

    /// Return a new artifact with packaging changed (e.g. to `"pom"`).
    pub fn with_packaging(&self, packaging: &str) -> Self {
        let mut clone = self.clone();
        clone.packaging = packaging.to_string();
        clone
    }

    /// Check if this artifact is a POM-only artifact.
    pub fn is_pom(&self) -> bool {
        self.packaging == "pom"
    }
}

impl fmt::Display for MavenArtifact {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.classifier {
            Some(c) => write!(
                f,
                "{}:{}:{}:{}@{}",
                self.group_id, self.artifact_id, self.version, c, self.packaging
            ),
            None => write!(
                f,
                "{}:{}:{}@{}",
                self.group_id, self.artifact_id, self.version, self.packaging
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_coordinate() {
        let a = MavenArtifact::parse("net.sf.jopt-simple:jopt-simple:5.0.4").unwrap();
        assert_eq!(a.group_id, "net.sf.jopt-simple");
        assert_eq!(a.artifact_id, "jopt-simple");
        assert_eq!(a.version, "5.0.4");
        assert_eq!(a.classifier, None);
        assert_eq!(a.packaging, "jar");
    }

    #[test]
    fn parse_with_classifier() {
        let a = MavenArtifact::parse("org.lwjgl:lwjgl:3.3.3:natives-windows").unwrap();
        assert_eq!(a.classifier, Some("natives-windows".to_string()));
    }

    #[test]
    fn parse_with_packaging_override() {
        let a = MavenArtifact::parse("com.example:lib:1.0@pom").unwrap();
        assert_eq!(a.packaging, "pom");
    }

    #[test]
    fn url_construction() {
        let a = MavenArtifact::parse("net.sf.jopt-simple:jopt-simple:5.0.4").unwrap();
        let url = a.url("https://libraries.minecraft.net");
        assert_eq!(
            url,
            "https://libraries.minecraft.net/net/sf/jopt-simple/jopt-simple/5.0.4/jopt-simple-5.0.4.jar"
        );
    }

    #[test]
    fn local_path_construction() {
        let a = MavenArtifact::parse("org.lwjgl:lwjgl:3.3.3:natives-windows").unwrap();
        let p = a.local_path();
        assert_eq!(
            p,
            PathBuf::from("org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3-natives-windows.jar")
        );
    }
}
