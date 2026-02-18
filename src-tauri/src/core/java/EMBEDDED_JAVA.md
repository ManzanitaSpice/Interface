# Guía de Java embebido para launcher Rust + Tauri

## Contrato de runtime gestionado

Cada runtime instalado debe incluir `runtime.json` con:

- `schema_version`
- `sha256_zip`
- `sha256_java`
- `installed_at`
- `source_url`
- `java_bin_rel`
- `major`, `arch`, `identifier`, `version`

La validación **no depende del vendor**: solo exige major compatible, arquitectura 64-bit y ejecución exitosa de `java -version`.

## Layout esperado

El ejecutable Java puede estar en layouts alternativos:

- `bin/java` (`bin/java.exe` en Windows)
- `Contents/Home/bin/java` (macOS)
- detección recursiva como fallback

## Flujo de instalación

1. Resolver release en Adoptium (fallback `jre` → `jdk`).
2. Descargar ZIP en streaming con retry exponencial y User-Agent propio.
3. Extraer en staging temporal (`UUID`).
4. Aplicar permisos ejecutables solo en Unix.
5. Escribir metadata versionada.
6. `rename` atómico al destino final.
7. Validar runtime final con `java -version`.
8. Limpiar runtimes viejos por política (límite por major).

## Locks y recuperación

El lock de descarga guarda `pid + timestamp`. Si el proceso murió o el lock expira, se recupera automáticamente.

## Índice incremental

Se utiliza `runtimes/index.json` para evitar escaneo completo de FS en cada arranque.
