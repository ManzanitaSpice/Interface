# Embedded Java runtime layout (Tauri v2)

This launcher packages an embedded Java runtime using Tauri bundle resources.

## Expected folder structure

```text
src-tauri/
  resources/
    runtime/
      bin/
        java.exe
      conf/
      legal/
      lib/
      release
```

## Why this README exists

Tauri's glob matcher can fail when a pattern points to an empty directory. Keeping this
README guarantees `resources/runtime/**` always has at least one match.

## tauri.conf.json (v2)

Use both entries below to reduce glob edge cases:

- `resources/runtime`
- `resources/runtime/**`

The runtime is then available next to the built executable under:

- Windows: `<install_dir>/resources/runtime`
