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

## Runtime contract (managed + embedded)

- Se acepta layout estándar (`bin/java` o `bin/java.exe`) y layout macOS bundle (`Contents/Home/bin/java`).
- El archivo `runtime.json` ahora usa `schema_version: 2` y guarda:
  - `java_sha256`: hash SHA-256 del binario final de Java.
  - `chmod_applied`: marca para no re-aplicar permisos ejecutables en cada inicio.
- La resolución de Java persiste cache en `resolved_java.json` dentro de `app_data_dir` para evitar escaneos completos por arranque.
- La descarga usa timeout + retry exponencial, y fallback automático `jre -> jdk` cuando no hay release de JRE.
- El runtime anterior no se elimina hasta validar al 100% el nuevo (swap atómico con backup temporal).
