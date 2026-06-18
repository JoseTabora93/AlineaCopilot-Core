//! E2E tests for the admin RBAC endpoints (Alinea Fase 2 #5).
//!
//! Cubren el gate `require_admin` (no-admin → 403), el listado de usuarios con
//! `roles[]`, el catálogo de roles, asignar/quitar roles, y los guardas:
//! rol desconocido (400), usuario inexistente (404) e invariante anti-lockout
//! "nunca cero administradores" (409).

mod common;

use axum::http::StatusCode;
use serde_json::json;
use tower::ServiceExt;

use aionui_app::AppServices;

use common::{body_json, build_app, delete_with_token, get_with_token, json_with_token, setup_and_login};

const PW: &str = "StrongP@ss1";

/// Crea+loguea un usuario y le asigna el rol `admin`.
/// Devuelve `(session_token, csrf_token, user_id)`.
async fn login_as_admin(
    app: &mut axum::Router,
    services: &AppServices,
    username: &str,
) -> (String, String, String) {
    let (token, csrf) = setup_and_login(app, services, username, PW).await;
    let user = services.user_repo.find_by_username(username).await.unwrap().unwrap();
    // auth_middleware recarga los roles por request, así que asignar aquí
    // (post-login) ya se refleja en la siguiente petición.
    services.user_repo.assign_role(&user.id, "admin").await.unwrap();
    (token, csrf, user.id)
}

async fn create_user(services: &AppServices, username: &str) -> String {
    let hash = aionui_auth::hash_password(PW).unwrap();
    services.user_repo.create_user(username, &hash).await.unwrap().id
}

#[tokio::test]
async fn non_admin_user_gets_403_on_admin_routes() {
    let (mut app, services) = build_app().await;
    let (token, _csrf) = setup_and_login(&mut app, &services, "worker", PW).await;

    let resp = app
        .clone()
        .oneshot(get_with_token("/api/admin/users", &token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN, "sin rol admin → 403");
}

#[tokio::test]
async fn admin_can_list_users_and_roles() {
    let (mut app, services) = build_app().await;
    let (token, _csrf, _id) = login_as_admin(&mut app, &services, "boss").await;

    // /api/admin/users incluye roles[]
    let resp = app
        .clone()
        .oneshot(get_with_token("/api/admin/users", &token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    let users = json["data"].as_array().expect("data array");
    let boss = users.iter().find(|u| u["username"] == "boss").expect("boss listado");
    assert!(
        boss["roles"].as_array().unwrap().iter().any(|r| r == "admin"),
        "boss debe traer el rol admin en roles[]"
    );

    // /api/admin/roles = catálogo de 6 roles del seed
    let resp = app
        .clone()
        .oneshot(get_with_token("/api/admin/roles", &token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    let roles = json["data"].as_array().expect("data array");
    assert_eq!(roles.len(), 6, "los 6 roles del seed");
    for expected in ["admin", "gerencia", "tecnica", "comercial", "financiera", "ingenieria"] {
        assert!(
            roles.iter().any(|r| r["id"] == expected),
            "falta el rol '{expected}' en el catálogo"
        );
    }
}

#[tokio::test]
async fn admin_can_assign_then_remove_role() {
    let (mut app, services) = build_app().await;
    let (token, csrf, _id) = login_as_admin(&mut app, &services, "boss").await;
    let target_id = create_user(&services, "target").await;

    // Asignar 'gerencia' → 200, roles incluye gerencia
    let resp = app
        .clone()
        .oneshot(json_with_token(
            "POST",
            &format!("/api/admin/users/{target_id}/roles"),
            json!({ "role": "gerencia" }),
            &token,
            &csrf,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert!(
        json["data"]["roles"].as_array().unwrap().iter().any(|r| r == "gerencia"),
        "tras asignar, roles[] debe incluir gerencia"
    );

    // Quitar 'gerencia' → 200, roles ya no la incluye
    let resp = app
        .clone()
        .oneshot(delete_with_token(
            &format!("/api/admin/users/{target_id}/roles/gerencia"),
            &token,
            &csrf,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert!(
        !json["data"]["roles"].as_array().unwrap().iter().any(|r| r == "gerencia"),
        "tras quitar, roles[] no debe incluir gerencia"
    );
}

#[tokio::test]
async fn assign_unknown_role_returns_400() {
    let (mut app, services) = build_app().await;
    let (token, csrf, id) = login_as_admin(&mut app, &services, "boss").await;

    let resp = app
        .clone()
        .oneshot(json_with_token(
            "POST",
            &format!("/api/admin/users/{id}/roles"),
            json!({ "role": "bogus" }),
            &token,
            &csrf,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "rol fuera del catálogo → 400");
}

#[tokio::test]
async fn assign_to_unknown_user_returns_404() {
    let (mut app, services) = build_app().await;
    let (token, csrf, _id) = login_as_admin(&mut app, &services, "boss").await;

    let resp = app
        .clone()
        .oneshot(json_with_token(
            "POST",
            "/api/admin/users/does_not_exist/roles",
            json!({ "role": "gerencia" }),
            &token,
            &csrf,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND, "usuario inexistente → 404");
}

#[tokio::test]
async fn cannot_remove_last_admin_returns_409() {
    let (mut app, services) = build_app().await;
    let (token, csrf, id) = login_as_admin(&mut app, &services, "boss").await;

    // boss es el ÚNICO admin: quitarle el rol dejaría el sistema sin admins.
    let resp = app
        .clone()
        .oneshot(delete_with_token(
            &format!("/api/admin/users/{id}/roles/admin"),
            &token,
            &csrf,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT, "último admin → 409");
}

#[tokio::test]
async fn can_remove_admin_when_another_admin_exists() {
    let (mut app, services) = build_app().await;
    let (token, csrf, _id) = login_as_admin(&mut app, &services, "boss").await;

    // Segundo admin: ahora hay 2, así que quitarle el rol a uno está permitido.
    let second_id = create_user(&services, "second").await;
    services.user_repo.assign_role(&second_id, "admin").await.unwrap();

    let resp = app
        .clone()
        .oneshot(delete_with_token(
            &format!("/api/admin/users/{second_id}/roles/admin"),
            &token,
            &csrf,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert!(
        !json["data"]["roles"].as_array().unwrap().iter().any(|r| r == "admin"),
        "al segundo admin se le quitó el rol"
    );
}
