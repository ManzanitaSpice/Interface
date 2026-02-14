# Minecraft oficial: premium y no premium (notas de implementación)

Este launcher ya usa infraestructura oficial para versiones y artefactos:

- Manifest oficial: `https://piston-meta.mojang.com/mc/game/version_manifest_v2.json`
- Librerías oficiales: `https://libraries.minecraft.net`
- Assets oficiales: `https://resources.download.minecraft.net`
- JAR cliente oficial: `downloads.client.url` dentro de cada `version.json`

## Premium (Microsoft)

Para una implementación premium robusta se recomienda:

1. Registrar una aplicación propia en Azure (client_id propio).
2. Usar OAuth 2.0 + PKCE.
3. Intercambiar el token para Xbox Live / XSTS / Minecraft Services.
4. Obtener `access_token`, `uuid`, `username`, `xuid` y construir placeholders de lanzamiento.

> Nota: usar un `client_id` público de terceros no es estable para producción y puede romperse sin aviso.

## No premium (offline)

El modo offline usa placeholders locales:

- `auth_player_name`
- `auth_uuid`
- `auth_access_token`
- `auth_xuid`
- `clientid`
- `user_type`

Se permite lanzar con jars/versiones oficiales, pero sin validación online de propiedad de cuenta.
