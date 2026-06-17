//! Endpoint público que expone la clave pública Ed25519 del Core (Alinea Fase 2 #5).
//!
//! Los agentes (OpenClaw/Hermes) la usan para verificar la firma del token de
//! identidad que viaja en cada request. Es público **a propósito**: una clave
//! pública no permite forjar tokens, y el agente —que no es un usuario— debe
//! poder leerla sin un JWT de sesión.

use std::sync::Arc;

use aionui_auth::RequestIdentityService;
use axum::Json;
use axum::extract::State;
use serde::Serialize;

#[derive(Serialize)]
pub(super) struct IdentityPubkeyResponse {
    /// Algoritmo de firma (siempre `ed25519`).
    algorithm: &'static str,
    /// Clave pública en base64 (url-safe, sin padding).
    public_key: String,
}

pub(super) async fn identity_pubkey(
    State(identity): State<Arc<RequestIdentityService>>,
) -> Json<IdentityPubkeyResponse> {
    Json(IdentityPubkeyResponse {
        algorithm: "ed25519",
        public_key: identity.public_key_base64(),
    })
}
