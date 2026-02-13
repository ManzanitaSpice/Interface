use quick_xml::de::from_str;
use serde::Deserialize;

use crate::core::error::{LauncherError, LauncherResult};

/// Minimal POM model – only the fields we care about for dependency resolution.
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PomDocument {
    #[serde(default)]
    pub group_id: Option<String>,
    #[serde(default)]
    pub artifact_id: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub packaging: Option<String>,
    #[serde(default)]
    pub dependencies: Option<PomDependencies>,
    #[serde(default)]
    pub dependency_management: Option<PomDependencyManagement>,
}

#[derive(Debug, Deserialize, Default)]
pub struct PomDependencies {
    #[serde(default, rename = "dependency")]
    pub items: Vec<PomDependency>,
}

#[derive(Debug, Deserialize, Default)]
pub struct PomDependencyManagement {
    #[serde(default)]
    pub dependencies: Option<PomDependencies>,
}

#[derive(Debug, Deserialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PomDependency {
    pub group_id: String,
    pub artifact_id: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub optional: Option<bool>,
    #[serde(rename = "type", default)]
    pub dep_type: Option<String>,
    #[serde(default)]
    pub classifier: Option<String>,
    #[serde(default)]
    pub exclusions: Option<PomExclusions>,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct PomExclusions {
    #[serde(default, rename = "exclusion")]
    pub items: Vec<PomExclusion>,
}

#[derive(Debug, Deserialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PomExclusion {
    pub group_id: String,
    pub artifact_id: String,
}

impl PomDocument {
    /// Parse a POM XML string into a `PomDocument`.
    pub fn parse(xml: &str) -> LauncherResult<Self> {
        // quick-xml's serde deserializer handles namespaces, etc.
        let doc: PomDocument = from_str(xml).map_err(|e| LauncherError::PomParse(e.to_string()))?;
        Ok(doc)
    }

    /// Resolve a dependency version using `dependencyManagement` if explicit version is absent.
    pub fn resolve_version(&self, dep: &PomDependency) -> Option<String> {
        if dep.version.is_some() {
            return dep.version.clone();
        }

        // Search dependencyManagement
        if let Some(dm) = &self.dependency_management {
            if let Some(deps) = &dm.dependencies {
                for managed in &deps.items {
                    if managed.group_id == dep.group_id && managed.artifact_id == dep.artifact_id {
                        return managed.version.clone();
                    }
                }
            }
        }

        None
    }

    /// Return compile-scope dependencies, ignoring optional and test/provided scopes.
    pub fn compile_dependencies(&self) -> Vec<PomDependency> {
        let deps = match &self.dependencies {
            Some(d) => &d.items,
            None => return vec![],
        };

        deps.iter()
            .filter(|d| {
                let scope = d.scope.as_deref().unwrap_or("compile");
                let optional = d.optional.unwrap_or(false);
                scope == "compile" && !optional
            })
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_pom() {
        let xml = r#"
        <project>
            <groupId>com.example</groupId>
            <artifactId>demo</artifactId>
            <version>1.0</version>
            <packaging>pom</packaging>
            <dependencies>
                <dependency>
                    <groupId>org.lwjgl</groupId>
                    <artifactId>lwjgl</artifactId>
                    <version>3.3.3</version>
                </dependency>
                <dependency>
                    <groupId>junit</groupId>
                    <artifactId>junit</artifactId>
                    <version>4.13</version>
                    <scope>test</scope>
                </dependency>
            </dependencies>
        </project>
        "#;
        let pom = PomDocument::parse(xml).unwrap();
        assert_eq!(pom.group_id.as_deref(), Some("com.example"));
        let compile = pom.compile_dependencies();
        assert_eq!(compile.len(), 1);
        assert_eq!(compile[0].artifact_id, "lwjgl");
    }
}
