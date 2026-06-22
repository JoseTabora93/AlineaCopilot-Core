//! Application configuration parsed from CLI arguments + key derivation.

use std::path::PathBuf;

use sha2::{Digest, Sha256};

/// Application configuration parsed from CLI arguments.
#[derive(Debug, Clone)]
pub struct AppConfig {
    pub host: String,
    pub port: u16,
    pub data_dir: PathBuf,
    pub work_dir: PathBuf,
    pub app_version: String,
    /// Run in local embedded mode (skip authentication, use system_default_user).
    pub local: bool,
    /// Activa la segregación de ficheros por usuario (Fase 2 #5): cada usuario
    /// solo accede a su subárbol `{work_dir}/users/{id}`. `None` deriva de
    /// `!local` (multiusuario seguro por defecto); los tests lo fijan a
    /// `Some(false)` para ejercitar ops de ficheros con paths arbitrarios.
    pub enforce_file_scope: Option<bool>,
}

impl AppConfig {
    /// Format as `host:port` for socket binding.
    pub fn socket_addr(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    /// ¿Aplicar el guard de scope de ficheros por-usuario? Deriva de `!local`
    /// cuando no está fijado explícitamente.
    pub fn enforce_file_scope(&self) -> bool {
        self.enforce_file_scope.unwrap_or(!self.local)
    }

    /// Path to the SQLite database file.
    pub fn database_path(&self) -> PathBuf {
        self.data_dir.join("aionui-backend.db")
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            host: aionui_common::constants::DEFAULT_HOST.to_string(),
            port: aionui_common::constants::DEFAULT_PORT,
            data_dir: PathBuf::from("data"),
            work_dir: PathBuf::from("data"),
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            local: false,
            enforce_file_scope: None,
        }
    }
}

/// Derive a 32-byte encryption key from the JWT secret using SHA-256.
pub fn derive_encryption_key(jwt_secret: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"aionui-encryption-key:");
    hasher.update(jwt_secret.as_bytes());
    hasher.finalize().into()
}

/// Deriva el KEK de 32 bytes que cifra la semilla de identidad Ed25519 (Fase 2 #5).
///
/// Usa **separación de dominio** respecto a [`derive_encryption_key`]: el prefijo
/// distinto garantiza que ambos KEK sean criptográficamente independientes aunque
/// compartan secreto raíz, de modo que comprometer el cifrado de un dominio (p. ej.
/// las api-keys de providers) no ayuda a atacar el otro (la raíz de identidad).
pub fn derive_identity_kek(root_secret: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"alinea-identity-kek:");
    hasher.update(root_secret.as_bytes());
    hasher.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_config_default() {
        let config = AppConfig::default();
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 25808);
        assert_eq!(config.data_dir, PathBuf::from("data"));
        assert_eq!(config.app_version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn test_app_config_socket_addr() {
        let config = AppConfig {
            host: "0.0.0.0".to_string(),
            port: 3000,
            ..Default::default()
        };
        assert_eq!(config.socket_addr(), "0.0.0.0:3000");
    }

    #[test]
    fn test_app_config_database_path() {
        let config = AppConfig {
            data_dir: PathBuf::from("/tmp/aionui"),
            ..Default::default()
        };
        assert_eq!(config.database_path(), PathBuf::from("/tmp/aionui/aionui-backend.db"));
    }
}
