# Runtime roles contract (Gamma / Delta)

## Roles

- **Gamma**: runtime de ejecución final del juego (Minecraft JVM principal).
- **Delta**: runtime de tooling/bootstrap/análisis (fases internas del launcher).

## Resolución

- `Gamma` se resuelve por versión de Minecraft:
  - `<= 1.20.4` -> Java 17
  - `>= 1.20.5` -> Java 21
- `Delta` se mantiene en Java 17 de forma conservadora.

## Estructura física

Los runtimes se guardan en rutas independientes para evitar reemplazos cruzados:

- `runtimes/v1/java-gamma/`
- `runtimes/v1/java-delta/`

## Fases de ejecución

- **Preparación**: usa Delta para tareas de verificación interna.
- **Bootstrap**: usa Delta.
- **Análisis de JARs / manifests / detección de mods / ASM / validaciones previas**: usa Delta.
- **Launch del juego**: usa Gamma exclusivamente.

## Invariantes

- Nunca compartir el mismo `Command` entre Gamma y Delta.
- Nunca depender de `java` global para fases internas.
- Cada proceso recibe binario absoluto y `JAVA_HOME` explícito.
- Sin fallback silencioso: si falla Delta o Gamma se aborta.

## Debug

Se puede forzar rol global con:

- `INTERFACE_RUNTIME_DEBUG_FORCE_ROLE=delta`
- `INTERFACE_RUNTIME_DEBUG_FORCE_ROLE=gamma`
