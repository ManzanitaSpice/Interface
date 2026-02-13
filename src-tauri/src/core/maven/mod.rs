mod artifact;
mod pom;
mod resolver;

pub use artifact::MavenArtifact;
pub use pom::{PomDependency, PomDocument};
pub use resolver::MavenResolver;

/// Well-known Maven repositories used by Minecraft ecosystem.
pub const MOJANG_LIBRARIES: &str = "https://libraries.minecraft.net";
pub const MAVEN_CENTRAL: &str = "https://repo1.maven.org/maven2";
pub const FORGE_MAVEN: &str = "https://maven.minecraftforge.net";
pub const FABRIC_MAVEN: &str = "https://maven.fabricmc.net";
pub const QUILT_MAVEN: &str = "https://maven.quiltmc.org/repository/release";
pub const NEOFORGE_MAVEN: &str = "https://maven.neoforged.net/releases";
