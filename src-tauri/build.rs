use std::fs;
use std::path::Path;

fn ensure_runtime_resources_placeholder() {
    let runtime_dir = Path::new("resources/runtime");
    let placeholder = runtime_dir.join(".keep");

    if let Err(error) = fs::create_dir_all(runtime_dir) {
        panic!("failed to create runtime resources directory: {error}");
    }

    if !placeholder.exists() {
        if let Err(error) = fs::write(&placeholder, b"runtime resources placeholder\n") {
            panic!("failed to create runtime resources placeholder file: {error}");
        }
    }
}

fn main() {
    ensure_runtime_resources_placeholder();
    tauri_build::build();
}
