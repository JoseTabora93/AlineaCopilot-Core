//! Identidad firmada por request (Alinea Fase 2 #5 — §5.1).
//!
//! El Core emite, por **cada** request a un agente, un token Ed25519 con
//! `{user_id, roles, project_id?, scopes, exp, jti}`. El agente (OpenClaw/Hermes)
//! valida la firma con la clave pública del Core y opera **solo dentro del scope**.
//! `exp` corto + `jti` único permiten revocación por denylist.

use base64::Engine;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier};
use serde::{Deserialize, Serialize};

/// base64 url-safe sin padding (compacto, apto para headers).
const B64: base64::engine::general_purpose::GeneralPurpose = base64::engine::general_purpose::URL_SAFE_NO_PAD;

/// Claims firmados que viajan en cada request a un agente.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IdentityClaims {
    pub user_id: String,
    /// Roles del usuario (RBAC eje 1).
    pub roles: Vec<String>,
    /// Proyecto activo (RBAC eje 3); `None` fuera de un proyecto.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// Scopes otorgados a este request.
    pub scopes: Vec<String>,
    /// Expiración en unix-ms.
    pub exp: i64,
    /// Identificador único del token (revocación por denylist).
    pub jti: String,
}

#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    #[error("malformed identity token")]
    Malformed,
    #[error("invalid identity signature")]
    BadSignature,
    #[error("identity token expired")]
    Expired,
    #[error("identity serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("entropy error")]
    Entropy,
}

/// Firma y verifica tokens de identidad por request con Ed25519.
///
/// La clave privada vive en el Core; la pública (`public_key_base64`) se distribuye
/// a OpenClaw/Hermes para que validen la firma.
#[derive(Clone)]
pub struct RequestIdentityService {
    signing_key: SigningKey,
}

impl RequestIdentityService {
    /// Construye desde una semilla de 32 bytes (persistida en el Core).
    pub fn from_seed(seed: [u8; 32]) -> Self {
        Self {
            signing_key: SigningKey::from_bytes(&seed),
        }
    }

    /// Genera un keypair nuevo (primer arranque). La semilla devuelta DEBE persistirse
    /// para que los tokens sigan siendo verificables tras reinicios.
    pub fn generate() -> Result<(Self, [u8; 32]), IdentityError> {
        let mut seed = [0u8; 32];
        getrandom::getrandom(&mut seed).map_err(|_| IdentityError::Entropy)?;
        Ok((Self::from_seed(seed), seed))
    }

    /// Clave pública en base64 (para los agentes).
    pub fn public_key_base64(&self) -> String {
        B64.encode(self.signing_key.verifying_key().to_bytes())
    }

    /// Emite un token firmado para `claims`. Formato compacto: `<b64(json)>.<b64(sig)>`.
    pub fn issue(&self, claims: &IdentityClaims) -> Result<String, IdentityError> {
        let payload = serde_json::to_vec(claims)?;
        let sig = self.signing_key.sign(&payload);
        Ok(format!("{}.{}", B64.encode(&payload), B64.encode(sig.to_bytes())))
    }

    /// Atajo: emite con `exp = now_ms + ttl_ms` y un `jti` nuevo (uuid v7).
    pub fn issue_for(
        &self,
        user_id: impl Into<String>,
        roles: Vec<String>,
        project_id: Option<String>,
        scopes: Vec<String>,
        now_ms: i64,
        ttl_ms: i64,
    ) -> Result<String, IdentityError> {
        let claims = IdentityClaims {
            user_id: user_id.into(),
            roles,
            project_id,
            scopes,
            exp: now_ms + ttl_ms,
            jti: uuid::Uuid::now_v7().to_string(),
        };
        self.issue(&claims)
    }

    /// Verifica firma + expiración (`now_ms`) y devuelve los claims.
    pub fn verify(&self, token: &str, now_ms: i64) -> Result<IdentityClaims, IdentityError> {
        let (p_b64, s_b64) = token.split_once('.').ok_or(IdentityError::Malformed)?;
        let payload = B64.decode(p_b64).map_err(|_| IdentityError::Malformed)?;
        let sig_bytes = B64.decode(s_b64).map_err(|_| IdentityError::Malformed)?;
        let sig = Signature::from_slice(&sig_bytes).map_err(|_| IdentityError::Malformed)?;
        self.signing_key
            .verifying_key()
            .verify(&payload, &sig)
            .map_err(|_| IdentityError::BadSignature)?;
        let claims: IdentityClaims = serde_json::from_slice(&payload)?;
        if claims.exp <= now_ms {
            return Err(IdentityError::Expired);
        }
        Ok(claims)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn svc() -> RequestIdentityService {
        RequestIdentityService::from_seed([7u8; 32])
    }

    #[test]
    fn round_trip_valid() {
        let s = svc();
        let token = s
            .issue_for(
                "u1",
                vec!["ingenieria".into()],
                Some("p1".into()),
                vec!["read".into()],
                1_000,
                60_000,
            )
            .unwrap();
        let claims = s.verify(&token, 1_500).unwrap();
        assert_eq!(claims.user_id, "u1");
        assert_eq!(claims.roles, vec!["ingenieria".to_string()]);
        assert_eq!(claims.project_id.as_deref(), Some("p1"));
        assert!(!claims.jti.is_empty());
    }

    #[test]
    fn rejects_tampered_signature() {
        let s = svc();
        let token = s.issue_for("u1", vec![], None, vec![], 1_000, 60_000).unwrap();
        let (p, _sig) = token.split_once('.').unwrap();
        let forged = format!("{p}.{}", B64.encode([0u8; 64]));
        assert!(matches!(s.verify(&forged, 1_500), Err(IdentityError::BadSignature)));
    }

    #[test]
    fn rejects_expired() {
        let s = svc();
        let token = s.issue_for("u1", vec![], None, vec![], 1_000, 10).unwrap();
        assert!(matches!(s.verify(&token, 2_000), Err(IdentityError::Expired)));
    }

    #[test]
    fn wrong_key_fails() {
        let a = RequestIdentityService::from_seed([1u8; 32]);
        let b = RequestIdentityService::from_seed([2u8; 32]);
        let token = a.issue_for("u1", vec![], None, vec![], 1_000, 60_000).unwrap();
        assert!(b.verify(&token, 1_500).is_err());
    }
}
