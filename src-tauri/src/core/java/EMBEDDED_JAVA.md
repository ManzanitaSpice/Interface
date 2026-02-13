# Guía de Java embebido para launcher Rust + Tauri

## Prioridad de Java

1. Java embebido detectado junto al ejecutable.
2. Si falta o no cumple versión, descarga automática de JRE 17 embebido.
3. Java del sistema (`JAVA_HOME`, `PATH`, ubicaciones comunes).
4. Error final si no existe ninguno compatible.

## Diferencia: Java del sistema vs Java embebido

- **Sistema**: depende de lo instalado por el usuario (puede romperse por versión, arquitectura o desinstalación).
- **Embebido**: runtime controlado por el launcher, consistente y reproducible.

## Versiones de Minecraft y Java

- Minecraft moderno (1.17+) requiere Java 17.
- Versiones nuevas como 1.21+ usan Java 21 para máxima compatibilidad.
- Launchers profesionales detectan requisito por versión y eligen runtime automáticamente.

## Buenas prácticas implementadas

- Validación real del runtime con `java -version`.
- Logs explícitos para detección, fallback y errores.
- Descarga bajo demanda si falta runtime embebido.
- Estructura modular para separar detección, descarga y lanzamiento.
