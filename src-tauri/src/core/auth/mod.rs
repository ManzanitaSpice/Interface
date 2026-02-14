use serde::{Deserialize, Serialize};

pub const AZURE_CLIENT_ID_FALLBACK: &str = "00000000402B5328";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AccountMode {
    Offline,
    Microsoft,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaunchAccountProfile {
    pub mode: AccountMode,
    pub username: String,
    pub uuid: String,
    pub access_token: String,
    pub xuid: String,
    pub user_type: String,
    pub client_id: String,
}

impl Default for LaunchAccountProfile {
    fn default() -> Self {
        Self::offline("Player")
    }
}

impl LaunchAccountProfile {
    pub fn offline(username: &str) -> Self {
        Self {
            mode: AccountMode::Offline,
            username: username.trim().to_string(),
            uuid: "00000000-0000-0000-0000-000000000000".into(),
            access_token: "offline_access_token".into(),
            xuid: "0".into(),
            user_type: "legacy".into(),
            client_id: AZURE_CLIENT_ID_FALLBACK.into(),
        }
    }

    pub fn sanitized(mut self) -> Self {
        if self.username.trim().is_empty() {
            self.username = "Player".into();
        }
        if self.uuid.trim().is_empty() {
            self.uuid = "00000000-0000-0000-0000-000000000000".into();
        }
        if self.access_token.trim().is_empty() {
            self.access_token = "offline_access_token".into();
        }
        if self.xuid.trim().is_empty() {
            self.xuid = "0".into();
        }
        if self.user_type.trim().is_empty() {
            self.user_type = match self.mode {
                AccountMode::Offline => "legacy".into(),
                AccountMode::Microsoft => "msa".into(),
            };
        }
        if self.client_id.trim().is_empty() {
            self.client_id = AZURE_CLIENT_ID_FALLBACK.into();
        }
        self
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AuthResearchInfo {
    pub official_version_manifest: &'static str,
    pub official_libraries_base: &'static str,
    pub official_assets_base: &'static str,
    pub official_client_jar_hint: &'static str,
    pub oauth_guidance: &'static str,
    pub fallback_public_client_id: &'static str,
}

impl Default for AuthResearchInfo {
    fn default() -> Self {
        Self {
            official_version_manifest: "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json",
            official_libraries_base: "https://libraries.minecraft.net",
            official_assets_base: "https://resources.download.minecraft.net",
            official_client_jar_hint: "El campo downloads.client.url de cada version JSON apunta al JAR oficial firmado por Mojang.",
            oauth_guidance: "Para launcher premium en producción debes registrar tu propia app de Azure y usar PKCE. Un client_id público de terceros puede fallar, cambiar sin aviso o violar TOS.",
            fallback_public_client_id: AZURE_CLIENT_ID_FALLBACK,
        }
    }
}
