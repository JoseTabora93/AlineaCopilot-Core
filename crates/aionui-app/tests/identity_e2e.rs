//! E2E: endpoint público de la clave pública de identidad (Fase 2 #5).
//!
//! Verifica que `GET /api/identity/pubkey` responde sin auth, devuelve un
//! Ed25519 pubkey no vacío, y que es estable entre requests (misma semilla).

mod common;

use axum::http::StatusCode;
use common::{body_json, build_app, get_request};
use tower::ServiceExt;

#[tokio::test]
async fn identity_pubkey_is_public_and_stable() {
    let (app, _services) = build_app().await;

    let resp = app
        .clone()
        .oneshot(get_request("/api/identity/pubkey"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "pubkey debe ser accesible sin auth");

    let json = body_json(resp).await;
    assert_eq!(json["algorithm"], "ed25519");
    let pk = json["public_key"].as_str().expect("public_key string").to_owned();
    assert!(!pk.is_empty(), "pubkey no vacío");

    // Estable: un segundo request devuelve la misma clave (misma semilla en disco).
    let resp2 = app.oneshot(get_request("/api/identity/pubkey")).await.unwrap();
    let json2 = body_json(resp2).await;
    assert_eq!(json2["public_key"].as_str().unwrap(), pk, "pubkey estable entre requests");
}
