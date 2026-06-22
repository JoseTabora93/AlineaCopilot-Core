//! Token de identidad por-conversación para el bridge HTTP del Guide MCP
//! (Alinea Fase 2 #5). El Guide server **emite y verifica en el mismo proceso**,
//! así que basta un cifrado autenticado (AES-256-GCM, reusando `crypto`) con
//! clave derivada del secreto del servidor: liga el `conversation_id` al token de
//! forma infalsificable.
//!
//! Cierra el IDOR donde un agente comprometido enviaba al bridge el
//! `conversation_id` de otro usuario para operar sobre su team: ahora el bridge
//! toma el `conversation_id` del token verificado, no del body del llamador, y un
//! agente solo posee el token ligado a su propia conversación.

use sha2::{Digest, Sha256};

use crate::crypto::{decrypt_string, encrypt_string};

/// Separador del payload `conversation_id`\n`user_id`. Ningún id contiene `\n`.
const SEP: char = '\n';

/// Deriva la clave AES de 32 bytes desde el secreto del Guide server (con dominio
/// separado de otros usos del mismo secreto).
fn key_from_secret(secret: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"aionui-guide-conversation-token:");
    hasher.update(secret.as_bytes());
    hasher.finalize().into()
}

/// Emite un token que liga `(conversation_id, user_id)` al `secret` del Guide
/// server. El agente lo recibe por env (`AION_MCP_TOKEN`) y lo presenta como
/// Bearer. El bridge usa estos valores del token (no del body del llamador), así
/// un agente no puede suplantar la conversación ni el usuario de otro.
pub fn scope(secret: &str, conversation_id: &str, user_id: &str) -> String {
    // Invariante: ningún id contiene el separador. Si lo contuviera, `verify`
    // partiría en el `\n` equivocado y mis-bindearía (confused deputy); fail-closed
    // devolviendo "" → el bridge responde 401.
    if conversation_id.contains(SEP) || user_id.contains(SEP) {
        return String::new();
    }
    let payload = format!("{conversation_id}{SEP}{user_id}");
    // `encrypt_string` solo falla si la clave no mide 32 bytes; SHA-256 siempre
    // los da, así que en la práctica es infalible. Ante el fallo imposible
    // devolvemos un token vacío → el bridge responde 401 (fail-closed).
    encrypt_string(&payload, &key_from_secret(secret)).unwrap_or_default()
}

/// Verifica un token y devuelve `(conversation_id, user_id)` ligados, o `None`
/// si es inválido o forjado.
pub fn verify(secret: &str, token: &str) -> Option<(String, String)> {
    if token.is_empty() {
        return None;
    }
    let payload = decrypt_string(token, &key_from_secret(secret)).ok()?;
    let (conversation_id, user_id) = payload.split_once(SEP)?;
    Some((conversation_id.to_owned(), user_id.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_binds_conversation_and_user() {
        let secret = "guide-secret-xyz";
        let token = scope(secret, "conv-A", "user-1");
        assert_eq!(verify(secret, &token), Some(("conv-A".to_owned(), "user-1".to_owned())));
    }

    #[test]
    fn wrong_secret_fails() {
        let token = scope("secret-1", "conv-A", "user-1");
        assert!(verify("secret-2", &token).is_none());
    }

    #[test]
    fn forged_or_empty_token_fails() {
        assert!(verify("s", "").is_none());
        assert!(verify("s", "not-a-valid-token").is_none());
    }

    #[test]
    fn token_hides_plaintext() {
        let token = scope("s", "conv-secret-123", "user-secret");
        assert!(!token.contains("conv-secret-123"));
        assert!(!token.contains("user-secret"));
    }

    #[test]
    fn id_with_separator_is_rejected_fail_closed() {
        // Un id con el separador no produce token válido (evita mis-binding).
        assert!(scope("s", "conv\nevil", "user").is_empty());
        assert!(scope("s", "conv", "user\nevil").is_empty());
        assert!(verify("s", &scope("s", "conv\nevil", "user")).is_none());
    }
}
