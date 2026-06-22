//! E2E tests del ledger de consumos $ (Alinea Fase 2 #3).
//!
//! Cubren: "mi consumo" vacío, reflejo de eventos registrados, fijar límite
//! (admin) + verlo en /me, y el gate admin (no-admin → 403).

mod common;

use axum::http::StatusCode;
use serde_json::json;
use tower::ServiceExt;

use aionui_db::models::NewUsageEvent;

use common::{body_json, build_app, get_with_token, json_with_token, setup_and_login};

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
