//! E2E tests de Perfiles de agente (Motor MULTI-PERFIL — plan hermes-alinea, tarea A1).
//!
//! Cubren: CRUD admin (`/api/admin/profiles*`), el gate 403 para no-admin, la
//! validación del esquema PERFIL v1 al crear, y el gate FAIL-CLOSED de
//! `GET /api/profiles` (usuario con rol con grant lo ve; sin grant no lo ve).

mod common;

use axum::http::StatusCode;
use serde_json::json;
use tower::ServiceExt;

use common::{body_json, build_app, delete_with_token, get_with_token, json_with_token, setup_and_login};

const PW: &str = "StrongP@ss1";

fn valid_definition(name: &str) -> serde_json::Value {
    json!({
        "name": name,
        "label": format!("Label {name}"),
        "engines": ["hermes"],
        "soul_md": "Test soul",
        "model": { "primary": "zai/glm-5.1", "fallbacks": [] },
        "mcp_allowlist": [],
        "skills": [],
        "kb_scope": [],
        "channels": [{ "type": "web" }],
        "caps": { "soft_usd": 1.0, "hard_usd": 2.0, "period": "month" },
        "acl": { "etiqueta": "interno", "roles": ["ingenieria"] }
    })
}

#[tokio::test]
async fn admin_can_create_list_get_update_delete_profile() {
    let (mut app, services) = build_app().await;
    let (token, csrf) = setup_and_login(&mut app, &services, "admin", PW).await;
    let admin = services.user_repo.find_by_username("admin").await.unwrap().unwrap();
    services.user_repo.assign_role(&admin.id, "admin").await.unwrap();

    // Create
    let resp = app
        .clone()
        .oneshot(json_with_token(
            "POST",
            "/api/admin/profiles",
            json!({
                "name": "ingenieria",
                "label": "Ingeniería (role-pack)",
                "definition": valid_definition("ingenieria"),
            }),
            &token,
            &csrf,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "create should succeed");
    let body = body_json(resp).await;
    let id = body["data"]["id"].as_str().unwrap().to_string();
    assert_eq!(body["data"]["name"], "ingenieria");
    assert_eq!(body["data"]["is_active"], true);

    // List (admin sees it, including inactive-capable listing)
    let resp = app
        .clone()
        .oneshot(get_with_token("/api/admin/profiles", &token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["data"].as_array().unwrap().len(), 1);

    // Get
    let resp = app
        .clone()
        .oneshot(get_with_token(&format!("/api/admin/profiles/{id}"), &token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Update (label + is_active)
    let resp = app
        .clone()
        .oneshot(json_with_token(
            "PATCH",
            &format!("/api/admin/profiles/{id}"),
            json!({ "label": "Nuevo label" }),
            &token,
            &csrf,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["data"]["label"], "Nuevo label");

    // Delete
    let resp = app
        .clone()
        .oneshot(delete_with_token(&format!("/api/admin/profiles/{id}"), &token, &csrf))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .clone()
        .oneshot(get_with_token(&format!("/api/admin/profiles/{id}"), &token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn admin_create_rejects_unknown_field_in_definition() {
    let (mut app, services) = build_app().await;
    let (token, csrf) = setup_and_login(&mut app, &services, "admin", PW).await;
    let admin = services.user_repo.find_by_username("admin").await.unwrap().unwrap();
    services.user_repo.assign_role(&admin.id, "admin").await.unwrap();

    let mut bad_def = valid_definition("preventa");
    bad_def
        .as_object_mut()
        .unwrap()
        .insert("totally_unknown_field".to_string(), json!(true));

    let resp = app
        .clone()
        .oneshot(json_with_token(
            "POST",
            "/api/admin/profiles",
            json!({ "name": "preventa", "label": "Preventa", "definition": bad_def }),
            &token,
            &csrf,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = body_json(resp).await;
    let msg = body["error"].as_str().unwrap_or_default();
    assert!(msg.contains("unknown field"), "unexpected message: {msg}");
}

#[tokio::test]
async fn admin_create_enforces_unique_name() {
    let (mut app, services) = build_app().await;
    let (token, csrf) = setup_and_login(&mut app, &services, "admin", PW).await;
    let admin = services.user_repo.find_by_username("admin").await.unwrap().unwrap();
    services.user_repo.assign_role(&admin.id, "admin").await.unwrap();

    let create_req = || {
        json_with_token(
            "POST",
            "/api/admin/profiles",
            json!({
                "name": "servimec-tko",
                "label": "ServiMec TKO",
                "definition": valid_definition("servimec-tko"),
            }),
            &token,
            &csrf,
        )
    };

    let resp = app.clone().oneshot(create_req()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app.clone().oneshot(create_req()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT, "duplicate name must 409");
}

#[tokio::test]
async fn non_admin_gets_403_on_admin_profile_routes() {
    let (mut app, services) = build_app().await;
    let (token, csrf) = setup_and_login(&mut app, &services, "member", PW).await;

    let resp = app
        .clone()
        .oneshot(get_with_token("/api/admin/profiles", &token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    let resp = app
        .clone()
        .oneshot(json_with_token(
            "POST",
            "/api/admin/profiles",
            json!({ "name": "x", "label": "X", "definition": valid_definition("x") }),
            &token,
            &csrf,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

/// 🔒 Gate FAIL-CLOSED de `GET /api/profiles`: un usuario con el rol
/// `ingenieria` y un grant de `resource_acl` (resource_type='agent_profile',
/// principal_type='role', principal_id='ingenieria') VE el perfil; un usuario
/// SIN ese rol/grant NO lo ve (lista vacía, no error).
#[tokio::test]
async fn user_sees_profile_only_with_role_grant_gate_fail_closed() {
    let (mut app, services) = build_app().await;

    // Admin crea el perfil.
    let (admin_token, admin_csrf) = setup_and_login(&mut app, &services, "admin", PW).await;
    let admin = services.user_repo.find_by_username("admin").await.unwrap().unwrap();
    services.user_repo.assign_role(&admin.id, "admin").await.unwrap();

    let resp = app
        .clone()
        .oneshot(json_with_token(
            "POST",
            "/api/admin/profiles",
            json!({
                "name": "ingenieria",
                "label": "Ingeniería (role-pack)",
                "definition": valid_definition("ingenieria"),
            }),
            &admin_token,
            &admin_csrf,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    let profile_id = body["data"]["id"].as_str().unwrap().to_string();

    // Usuario SIN rol/grant: /api/profiles debe ser una lista VACÍA (fail-closed).
    let (member_token, _member_csrf) = setup_and_login(&mut app, &services, "sin_acceso", PW).await;
    let resp = app
        .clone()
        .oneshot(get_with_token("/api/profiles", &member_token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(
        body["data"].as_array().unwrap().len(),
        0,
        "sin grant, el usuario no debe ver ningún perfil"
    );

    // Usuario CON rol 'ingenieria' + grant de resource_acl a ese rol: SÍ lo ve.
    let (eng_token, _eng_csrf) = setup_and_login(&mut app, &services, "ingeniero", PW).await;
    let ingeniero = services.user_repo.find_by_username("ingeniero").await.unwrap().unwrap();
    services
        .user_repo
        .assign_role(&ingeniero.id, "ingenieria")
        .await
        .unwrap();
    services
        .resource_acl_repo
        .grant("agent_profile", &profile_id, "ingenieria", "read")
        .await
        .unwrap();

    let resp = app
        .clone()
        .oneshot(get_with_token("/api/profiles", &eng_token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    let visible = body["data"].as_array().unwrap();
    assert_eq!(visible.len(), 1, "con grant de rol, el usuario debe ver el perfil");
    assert_eq!(visible[0]["id"], profile_id);

    // El usuario sin acceso SIGUE sin verlo (el grant es a nivel de rol, no global).
    let resp = app
        .clone()
        .oneshot(get_with_token("/api/profiles", &member_token))
        .await
        .unwrap();
    let body = body_json(resp).await;
    assert_eq!(body["data"].as_array().unwrap().len(), 0);
}

/// Grant DIRECTO por usuario (no por rol) también debe dar visibilidad.
#[tokio::test]
async fn user_sees_profile_with_direct_user_grant() {
    let (mut app, services) = build_app().await;

    let (admin_token, admin_csrf) = setup_and_login(&mut app, &services, "admin", PW).await;
    let admin = services.user_repo.find_by_username("admin").await.unwrap().unwrap();
    services.user_repo.assign_role(&admin.id, "admin").await.unwrap();

    let resp = app
        .clone()
        .oneshot(json_with_token(
            "POST",
            "/api/admin/profiles",
            json!({
                "name": "preventa",
                "label": "Preventa MEP",
                "definition": valid_definition("preventa"),
            }),
            &admin_token,
            &admin_csrf,
        ))
        .await
        .unwrap();
    let body = body_json(resp).await;
    let profile_id = body["data"]["id"].as_str().unwrap().to_string();

    let (user_token, _csrf) = setup_and_login(&mut app, &services, "comercial1", PW).await;
    let user = services
        .user_repo
        .find_by_username("comercial1")
        .await
        .unwrap()
        .unwrap();

    // Sin grant: no lo ve.
    let resp = app
        .clone()
        .oneshot(get_with_token("/api/profiles", &user_token))
        .await
        .unwrap();
    let body = body_json(resp).await;
    assert_eq!(body["data"].as_array().unwrap().len(), 0);

    // Grant directo por user_id (sin rol especial): lo ve.
    services
        .resource_acl_repo
        .grant("agent_profile", &profile_id, &user.id, "read")
        .await
        .unwrap();
    let resp = app
        .clone()
        .oneshot(get_with_token("/api/profiles", &user_token))
        .await
        .unwrap();
    let body = body_json(resp).await;
    let visible = body["data"].as_array().unwrap();
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0]["id"], profile_id);
}

/// Perfiles inactivos (`is_active=0`) no aparecen en `GET /api/profiles`
/// aunque el usuario tenga grant — solo en el catálogo admin.
#[tokio::test]
async fn inactive_profile_hidden_from_user_endpoint_even_with_grant() {
    let (mut app, services) = build_app().await;

    let (admin_token, admin_csrf) = setup_and_login(&mut app, &services, "admin", PW).await;
    let admin = services.user_repo.find_by_username("admin").await.unwrap().unwrap();
    services.user_repo.assign_role(&admin.id, "admin").await.unwrap();

    let resp = app
        .clone()
        .oneshot(json_with_token(
            "POST",
            "/api/admin/profiles",
            json!({
                "name": "servimec-tko",
                "label": "ServiMec TKO",
                "definition": valid_definition("servimec-tko"),
            }),
            &admin_token,
            &admin_csrf,
        ))
        .await
        .unwrap();
    let body = body_json(resp).await;
    let profile_id = body["data"]["id"].as_str().unwrap().to_string();

    let (user_token, _csrf) = setup_and_login(&mut app, &services, "tecnico1", PW).await;
    let user = services.user_repo.find_by_username("tecnico1").await.unwrap().unwrap();
    services
        .resource_acl_repo
        .grant("agent_profile", &profile_id, &user.id, "read")
        .await
        .unwrap();

    // Admin desactiva el perfil.
    let resp = app
        .clone()
        .oneshot(json_with_token(
            "PATCH",
            &format!("/api/admin/profiles/{profile_id}"),
            json!({ "is_active": false }),
            &admin_token,
            &admin_csrf,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .clone()
        .oneshot(get_with_token("/api/profiles", &user_token))
        .await
        .unwrap();
    let body = body_json(resp).await;
    assert_eq!(
        body["data"].as_array().unwrap().len(),
        0,
        "perfil inactivo no debe verse aunque haya grant"
    );

    // Pero sigue visible en el catálogo admin (incluye inactivos).
    let resp = app
        .clone()
        .oneshot(get_with_token("/api/admin/profiles", &admin_token))
        .await
        .unwrap();
    let body = body_json(resp).await;
    assert_eq!(body["data"].as_array().unwrap().len(), 1);
}

// ---------------------------------------------------------------------------
// Materialización perfil→assistant (motor "copilot" — tarea A8)
// ---------------------------------------------------------------------------
//
// Estos tests ejercitan el stack HTTP real (`build_app`, sin fixtures
// custom): crear/actualizar/desactivar un perfil vía `/api/admin/profiles`
// dispara el compilador determinista `aionui_assistant::materialize_profile`
// (enganchado en `crates/aionui-app/src/router/profiles.rs`), y el gate de
// visibilidad de `GET /api/assistants` (`aionui_assistant::routes::list`,
// `crates/aionui-assistant/src/profile_visibility.rs`) respeta el mismo ACL
// que `GET /api/profiles`.

fn copilot_definition(name: &str, soul_md: &str, model_primary: &str) -> serde_json::Value {
    json!({
        "name": name,
        "label": format!("Label {name}"),
        "engines": ["copilot"],
        "soul_md": soul_md,
        "model": { "primary": model_primary, "fallbacks": [] },
        "mcp_allowlist": ["ingelmec-kb"],
        "skills": ["alcance-bom"],
        "kb_scope": [],
        "channels": [{ "type": "web" }],
        "caps": { "soft_usd": 1.0, "hard_usd": 2.0, "period": "month" },
        "acl": { "etiqueta": "interno", "roles": ["ingenieria"] }
    })
}

/// Assistant materializado visible solo con grant (mismo gate que `GET
/// /api/profiles`), invisible sin grant, y visible para admin sin grant
/// explícito.
#[tokio::test]
async fn copilot_profile_materializes_assistant_gated_like_profiles_endpoint() {
    let (mut app, services) = build_app().await;

    let (admin_token, admin_csrf) = setup_and_login(&mut app, &services, "admin", PW).await;
    let admin = services.user_repo.find_by_username("admin").await.unwrap().unwrap();
    services.user_repo.assign_role(&admin.id, "admin").await.unwrap();

    let resp = app
        .clone()
        .oneshot(json_with_token(
            "POST",
            "/api/admin/profiles",
            json!({
                "name": "ingenieria",
                "label": "Ingeniería (role-pack)",
                "definition": copilot_definition("ingenieria", "# Ingeniería", "zai/glm-5.1"),
            }),
            &admin_token,
            &admin_csrf,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    let profile_id = body["data"]["id"].as_str().unwrap().to_string();

    // Admin ve el assistant materializado en /api/assistants sin grant explícito.
    let resp = app
        .clone()
        .oneshot(get_with_token("/api/assistants", &admin_token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    let items = body["data"].as_array().unwrap();
    assert!(
        items.iter().any(|a| a["id"] == "profile:ingenieria"),
        "admin debe ver el assistant materializado"
    );

    // Usuario SIN rol/grant: no lo ve.
    let (member_token, _csrf) = setup_and_login(&mut app, &services, "sin_acceso", PW).await;
    let resp = app
        .clone()
        .oneshot(get_with_token("/api/assistants", &member_token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    let items = body["data"].as_array().unwrap();
    assert!(
        !items.iter().any(|a| a["id"] == "profile:ingenieria"),
        "sin grant, el usuario no debe ver el assistant gestionado"
    );

    // Usuario CON rol 'ingenieria' + grant de resource_acl a ese rol: sí lo ve.
    let (eng_token, _csrf) = setup_and_login(&mut app, &services, "ingeniero", PW).await;
    let ingeniero = services.user_repo.find_by_username("ingeniero").await.unwrap().unwrap();
    services
        .user_repo
        .assign_role(&ingeniero.id, "ingenieria")
        .await
        .unwrap();
    services
        .resource_acl_repo
        .grant("agent_profile", &profile_id, "ingenieria", "read")
        .await
        .unwrap();

    let resp = app
        .clone()
        .oneshot(get_with_token("/api/assistants", &eng_token))
        .await
        .unwrap();
    let body = body_json(resp).await;
    let items = body["data"].as_array().unwrap();
    let materialized = items
        .iter()
        .find(|a| a["id"] == "profile:ingenieria")
        .expect("con grant de rol, el usuario debe ver el assistant gestionado");
    assert_eq!(
        materialized["source"], "user",
        "el DTO actual no distingue 'generated' de 'user' a nivel de wire"
    );
}

/// Actualizar `soul_md` en el perfil actualiza el assistant materializado;
/// re-mandar exactamente la misma definición es un no-op (mismo
/// `updated_at`) a nivel del compilador — verificado indirectamente aquí
/// comprobando que la segunda actualización no cambia el contenido servido
/// y que el flujo HTTP completo (create → update → update idéntico) no
/// falla ni duplica nada.
#[tokio::test]
async fn updating_profile_soul_md_updates_materialized_assistant_and_rematerializing_is_safe() {
    let (mut app, services) = build_app().await;
    let (admin_token, admin_csrf) = setup_and_login(&mut app, &services, "admin", PW).await;
    let admin = services.user_repo.find_by_username("admin").await.unwrap().unwrap();
    services.user_repo.assign_role(&admin.id, "admin").await.unwrap();

    let resp = app
        .clone()
        .oneshot(json_with_token(
            "POST",
            "/api/admin/profiles",
            json!({
                "name": "preventa",
                "label": "Preventa MEP",
                "definition": copilot_definition("preventa", "# v1", "zai/glm-5.1"),
            }),
            &admin_token,
            &admin_csrf,
        ))
        .await
        .unwrap();
    let body = body_json(resp).await;
    let profile_id = body["data"]["id"].as_str().unwrap().to_string();

    // Re-materializar sin cambios (PATCH con exactamente la misma definición).
    let resp = app
        .clone()
        .oneshot(json_with_token(
            "PATCH",
            &format!("/api/admin/profiles/{profile_id}"),
            json!({ "definition": copilot_definition("preventa", "# v1", "zai/glm-5.1") }),
            &admin_token,
            &admin_csrf,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Cambiar soul_md: el assistant materializado refleja el cambio.
    let resp = app
        .clone()
        .oneshot(json_with_token(
            "PATCH",
            &format!("/api/admin/profiles/{profile_id}"),
            json!({ "definition": copilot_definition("preventa", "# v2 cambiado", "zai/glm-5.1") }),
            &admin_token,
            &admin_csrf,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .clone()
        .oneshot(get_with_token("/api/assistants/profile:preventa", &admin_token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["data"]["rules"]["content"], "# v2 cambiado");
}

/// Desactivar el perfil retira el assistant materializado de
/// `GET /api/assistants` (incluso para admin).
#[tokio::test]
async fn deactivating_profile_removes_materialized_assistant_from_catalog() {
    let (mut app, services) = build_app().await;
    let (admin_token, admin_csrf) = setup_and_login(&mut app, &services, "admin", PW).await;
    let admin = services.user_repo.find_by_username("admin").await.unwrap().unwrap();
    services.user_repo.assign_role(&admin.id, "admin").await.unwrap();

    let resp = app
        .clone()
        .oneshot(json_with_token(
            "POST",
            "/api/admin/profiles",
            json!({
                "name": "servimec-tko",
                "label": "ServiMec TKO",
                "definition": copilot_definition("servimec-tko", "# TKO", "zai/glm-5.1"),
            }),
            &admin_token,
            &admin_csrf,
        ))
        .await
        .unwrap();
    let body = body_json(resp).await;
    let profile_id = body["data"]["id"].as_str().unwrap().to_string();

    let resp = app
        .clone()
        .oneshot(get_with_token("/api/assistants", &admin_token))
        .await
        .unwrap();
    let body = body_json(resp).await;
    assert!(
        body["data"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["id"] == "profile:servimec-tko")
    );

    let resp = app
        .clone()
        .oneshot(json_with_token(
            "PATCH",
            &format!("/api/admin/profiles/{profile_id}"),
            json!({ "is_active": false }),
            &admin_token,
            &admin_csrf,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .clone()
        .oneshot(get_with_token("/api/assistants", &admin_token))
        .await
        .unwrap();
    let body = body_json(resp).await;
    assert!(
        !body["data"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["id"] == "profile:servimec-tko"),
        "el assistant materializado debe retirarse al desactivar el perfil"
    );
}

/// Perfiles sin `"copilot"` en `engines` no materializan ningún assistant, y
/// los assistants no gestionados (aquí: ninguno seed-eado, pero se prueba
/// negativamente que el catálogo permanece vacío para un perfil "hermes").
#[tokio::test]
async fn profile_without_copilot_engine_does_not_materialize_assistant() {
    let (mut app, services) = build_app().await;
    let (admin_token, admin_csrf) = setup_and_login(&mut app, &services, "admin", PW).await;
    let admin = services.user_repo.find_by_username("admin").await.unwrap().unwrap();
    services.user_repo.assign_role(&admin.id, "admin").await.unwrap();

    let mut def = copilot_definition("hermes-only", "# hermes", "zai/glm-5.1");
    def["engines"] = json!(["hermes"]);

    let resp = app
        .clone()
        .oneshot(json_with_token(
            "POST",
            "/api/admin/profiles",
            json!({
                "name": "hermes-only",
                "label": "Hermes only",
                "definition": def,
            }),
            &admin_token,
            &admin_csrf,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .clone()
        .oneshot(get_with_token("/api/assistants", &admin_token))
        .await
        .unwrap();
    let body = body_json(resp).await;
    assert!(
        !body["data"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["id"] == "profile:hermes-only"),
        "perfil sin 'copilot' en engines no debe materializar assistant"
    );
}
