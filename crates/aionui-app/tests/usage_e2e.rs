//! E2E tests del ledger de consumos $ (Alinea Fase 2 #3; ingest + profile_id —
//! Fase ledger tarea C1).
//!
//! Cubren: "mi consumo" vacío, reflejo de eventos registrados, fijar límite
//! (admin) + verlo en /me, el gate admin (no-admin → 403), y el ingest externo
//! (token válido graba, sin/con token inválido → 401, idempotencia, filtro por
//! profile_id en el panel admin).

mod common;

use axum::http::StatusCode;
use serde_json::json;
use tower::ServiceExt;

use aionui_db::models::NewUsageEvent;

use common::{
    body_json, build_app, get_with_token, json_with_token, post_no_auth, post_with_bearer_only, setup_and_login,
};

const PW: &str = "StrongP@ss1";

#[tokio::test]
async fn my_usage_starts_empty() {
    let (mut app, services) = build_app().await;
    let (token, _csrf) = setup_and_login(&mut app, &services, "ana", PW).await;

    let resp = app
        .clone()
        .oneshot(get_with_token("/api/usage/me", &token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json["data"]["usage"]["events"], 0);
    assert_eq!(json["data"]["usage"]["cost_usd"], 0.0);
    assert!(json["data"]["limit"].is_null(), "sin límite configurado");
}

#[tokio::test]
async fn my_usage_reflects_recorded_events() {
    let (mut app, services) = build_app().await;
    let (token, _csrf) = setup_and_login(&mut app, &services, "ana", PW).await;
    let user = services.user_repo.find_by_username("ana").await.unwrap().unwrap();

    services
        .usage_repo
        .record_event(NewUsageEvent {
            user_id: user.id.clone(),
            engine: "copilot".into(),
            model: Some("claude-sonnet".into()),
            tokens_in: 1_000_000,
            tokens_out: 1_000_000,
            ..Default::default()
        })
        .await
        .unwrap();

    let resp = app
        .clone()
        .oneshot(get_with_token("/api/usage/me", &token))
        .await
        .unwrap();
    let json = body_json(resp).await;
    assert_eq!(json["data"]["usage"]["events"], 1);
    // 1M in @$3 + 1M out @$15 = $18 (tabla de precios sonnet).
    let cost = json["data"]["usage"]["cost_usd"].as_f64().unwrap();
    assert!((cost - 18.0).abs() < 1e-6, "cost {cost}");
}

#[tokio::test]
async fn admin_sets_limit_and_user_sees_it() {
    let (mut app, services) = build_app().await;
    let (token, csrf) = setup_and_login(&mut app, &services, "boss", PW).await;
    let boss = services.user_repo.find_by_username("boss").await.unwrap().unwrap();
    services.user_repo.assign_role(&boss.id, "admin").await.unwrap();

    // PUT límite
    let resp = app
        .clone()
        .oneshot(json_with_token(
            "PUT",
            &format!("/api/admin/users/{}/limit", boss.id),
            json!({ "soft_usd": 5.0, "hard_usd": 20.0 }),
            &token,
            &csrf,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(body_json(resp).await["data"]["hard_usd"], 20.0);

    // /me refleja el límite
    let resp = app
        .clone()
        .oneshot(get_with_token("/api/usage/me", &token))
        .await
        .unwrap();
    assert_eq!(body_json(resp).await["data"]["limit"]["hard_usd"], 20.0);

    // panel admin lista consumo (200)
    let resp = app
        .clone()
        .oneshot(get_with_token("/api/admin/usage", &token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // lectura dedicada del límite (sirve para CUALQUIER usuario, tenga gasto o no):
    // el editor la usa para prellenar el umbral actual.
    let resp = app
        .clone()
        .oneshot(get_with_token(&format!("/api/admin/users/{}/limit", boss.id), &token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        body_json(resp).await["data"]["hard_usd"],
        20.0,
        "lectura del límite activo"
    );
}

#[tokio::test]
async fn non_admin_cannot_access_admin_usage() {
    let (mut app, services) = build_app().await;
    let (token, _csrf) = setup_and_login(&mut app, &services, "worker", PW).await;

    let resp = app
        .clone()
        .oneshot(get_with_token("/api/admin/usage", &token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN, "no-admin → 403");
}

/// Crea un service token vía el endpoint admin real (`POST
/// /api/admin/service-tokens`) y devuelve el token en claro. Ejercita el mismo
/// camino que un operador humano usaría para aprovisionar un emisor externo.
async fn create_service_token(app: &mut axum::Router, token: &str, csrf: &str, name: &str) -> String {
    let resp = app
        .clone()
        .oneshot(json_with_token(
            "POST",
            "/api/admin/service-tokens",
            json!({ "name": name }),
            token,
            csrf,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "service token creation should succeed");
    let json = body_json(resp).await;
    json["data"]["token"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn ingest_with_valid_service_token_records_event() {
    let (mut app, services) = build_app().await;
    let (admin_token, csrf) = setup_and_login(&mut app, &services, "boss", PW).await;
    let boss = services.user_repo.find_by_username("boss").await.unwrap().unwrap();
    services.user_repo.assign_role(&boss.id, "admin").await.unwrap();

    let svc_token = create_service_token(&mut app, &admin_token, &csrf, "preventa-pipeline").await;

    let resp = app
        .clone()
        .oneshot(post_with_bearer_only(
            "/api/usage/ingest",
            json!({
                "engine": "openclaw",
                "model": "glm-4.6",
                "provider": "zai",
                "profile_id": "preventa",
                "tokens_in": 10000,
                "tokens_out": 2000,
                "cache_read": 0,
                "cache_write": 0,
                "cost_usd": 0.42,
                "ts_ms": aionui_common::now_ms(),
                "source": "preventa_cost"
            }),
            &svc_token,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "ingest with valid service token should succeed"
    );
    let json = body_json(resp).await;
    assert_eq!(json["data"]["deduped"], false);
    assert!(json["data"]["event_id"].as_str().unwrap().starts_with("usage_"));

    // Visible en el panel admin (agregado bajo el usuario por defecto ya que
    // el emisor no mandó user_id).
    let resp = app
        .clone()
        .oneshot(get_with_token("/api/admin/usage", &admin_token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    let rows = json["data"].as_array().unwrap();
    let total_events: i64 = rows.iter().map(|r| r["events"].as_i64().unwrap()).sum();
    assert!(total_events >= 1, "ingest event should be visible in admin panel");
}

#[tokio::test]
async fn ingest_without_token_is_unauthorized() {
    let (app, _services) = build_app().await;

    let resp = app
        .clone()
        .oneshot(post_no_auth(
            "/api/usage/ingest",
            json!({
                "engine": "hermes",
                "cost_usd": 0.1,
                "ts_ms": aionui_common::now_ms(),
                "source": "hermes_usage_export"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "sin token → 401");
}

#[tokio::test]
async fn ingest_with_invalid_token_is_unauthorized() {
    let (app, _services) = build_app().await;

    let resp = app
        .clone()
        .oneshot(post_with_bearer_only(
            "/api/usage/ingest",
            json!({
                "engine": "hermes",
                "cost_usd": 0.1,
                "ts_ms": aionui_common::now_ms(),
                "source": "hermes_usage_export"
            }),
            "not-a-real-token",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "token inválido → 401");
}

#[tokio::test]
async fn ingest_with_repeated_idempotency_key_dedupes() {
    let (mut app, services) = build_app().await;
    let (admin_token, csrf) = setup_and_login(&mut app, &services, "boss", PW).await;
    let boss = services.user_repo.find_by_username("boss").await.unwrap().unwrap();
    services.user_repo.assign_role(&boss.id, "admin").await.unwrap();

    let svc_token = create_service_token(&mut app, &admin_token, &csrf, "hermes-export").await;

    let payload = json!({
        "engine": "hermes",
        "cost_usd": 0.5,
        "ts_ms": aionui_common::now_ms(),
        "source": "hermes_usage_export",
        "idempotency_key": "cron-run-2026-07-02T09:00"
    });

    let resp1 = app
        .clone()
        .oneshot(post_with_bearer_only("/api/usage/ingest", payload.clone(), &svc_token))
        .await
        .unwrap();
    assert_eq!(resp1.status(), StatusCode::OK);
    let json1 = body_json(resp1).await;
    assert_eq!(json1["data"]["deduped"], false);
    let event_id_1 = json1["data"]["event_id"].as_str().unwrap().to_owned();

    // Reintento con la misma idempotency_key (fallback JSONL del emisor tras
    // una caída del Core) → 200 con deduped:true, sin duplicar el evento.
    let resp2 = app
        .clone()
        .oneshot(post_with_bearer_only("/api/usage/ingest", payload, &svc_token))
        .await
        .unwrap();
    assert_eq!(resp2.status(), StatusCode::OK);
    let json2 = body_json(resp2).await;
    assert_eq!(json2["data"]["deduped"], true, "reintento debe deduplicar");
    assert_eq!(json2["data"]["event_id"], event_id_1);
}

#[tokio::test]
async fn admin_usage_filters_by_profile_id() {
    let (mut app, services) = build_app().await;
    let (admin_token, csrf) = setup_and_login(&mut app, &services, "boss", PW).await;
    let boss = services.user_repo.find_by_username("boss").await.unwrap().unwrap();
    services.user_repo.assign_role(&boss.id, "admin").await.unwrap();

    let svc_token = create_service_token(&mut app, &admin_token, &csrf, "mixed-emitter").await;

    for (profile, key) in [("servimec-tko", "k1"), ("preventa", "k2")] {
        let resp = app
            .clone()
            .oneshot(post_with_bearer_only(
                "/api/usage/ingest",
                json!({
                    "engine": "hermes",
                    "profile_id": profile,
                    "cost_usd": 1.0,
                    "ts_ms": aionui_common::now_ms(),
                    "source": "hermes_usage_export",
                    "idempotency_key": key
                }),
                &svc_token,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    let resp = app
        .clone()
        .oneshot(get_with_token("/api/admin/usage?profile_id=servimec-tko", &admin_token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    let rows = json["data"].as_array().unwrap();
    let total_events: i64 = rows.iter().map(|r| r["events"].as_i64().unwrap()).sum();
    assert_eq!(
        total_events, 1,
        "filtro por profile_id debe ver solo el evento de servimec-tko"
    );
}
