pub mod runtime;

pub use runtime::detect_embedded_java_binary;
pub use runtime::detect_java_installations;
pub use runtime::ensure_embedded_jre17;
pub use runtime::find_java_binary;
pub use runtime::resolve_java_binary;
pub use runtime::JavaInstallation;
