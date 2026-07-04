//! E2E test del seed de role-packs (Alinea plan hermes-alinea, tarea A6).
//!
//! Migración `022_role_pack_profiles.sql` fusiona los 6 roles sembrados del
//! Core (`roles` de `014_rbac_roles.sql`) con los 6 agentes-stub de OpenClaw
//! (`workspace/agents/openclaw-*.md`, tarea A5) en `agent_profiles`, más los
//! grants rol→perfil en `resource_acl`.
//!
//! Test obligatorio (gate FAIL-CLOSED, mismo mecanismo que
//! `profiles_e2e.rs::user_sees_profile_only_with_role_grant_gate_fail_closed`):
//! un usuario con rol `tecnica` puede resolver el perfil `servimec-tko`
//! (grant sembrado por la migración) y NO puede resolver `financiera`.

mod common;

use axum::http::StatusCode;
use tower::ServiceExt;

use common::{body_json, get_with_token, setup_and_login};

const PW: &str = "StrongP@ss1";

/// Los 6 role-packs sembrados por la migración 022 ya existen en el catálogo
/// admin sin necesidad de crearlos vía API (idempotente, corre con las demás
/// migraciones al iniciar la DB in-memory de test).
#[tokio::test]
async fn seed_creates_six_role_packs_visible_to_admin() {
    let (mut app, services) = common::build_app().await;
    let (admin_token, _csrf) = setup_and_login(&mut app, &services, "admin", PW).await;
    let admin = services.user_repo.find_by_username("admin").await.unwrap().unwrap();
    services.user_repo.assign_role(&admin.id, "admin").await.unwrap();

    let resp = app
        .clone()
        .oneshot(get_with_token("/api/admin/profiles", &admin_token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    let mut names: Vec<String> = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap().to_string())
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            "admin",
            "comercial",
            "financiera",
            "gerencia",
            "ingenieria",
            "servimec-tko"
        ],
        "los 6 role-packs (fusión rol Core + stub openclaw-*) deben existir tras el seed"
    );

    // Cada perfil está activo (is_active=1) — el seed no crea perfiles apagados.
    for profile in body["data"].as_array().unwrap() {
        assert_eq!(
            profile["is_active"], true,
            "profile {:?} debe estar activo",
            profile["name"]
        );
    }
}

/// 🔒 Gate FAIL-CLOSED — caso obligatorio de la tarea A6:
/// un usuario con rol `tecnica` SÍ resuelve `servimec-tko` (grant sembrado
/// por la migración) y NO resuelve `financiera` (sin grant para 'tecnica'
/// sobre ese perfil).
#[tokio::test]
async fn tecnica_role_resolves_servimec_tko_but_not_financiera() {
    let (mut app, services) = common::build_app().await;

    // Usuario de prueba con rol 'tecnica' (uno de los 6 roles seed de
    // 014_rbac_roles.sql) — NO admin, para ejercitar el gate real.
    let (tecnica_token, _csrf) = setup_and_login(&mut app, &services, "tecnico_campo", PW).await;
    let tecnico = services
        .user_repo
        .find_by_username("tecnico_campo")
        .await
        .unwrap()
        .unwrap();
    services.user_repo.assign_role(&tecnico.id, "tecnica").await.unwrap();

    let resp = app
        .clone()
        .oneshot(get_with_token("/api/profiles", &tecnica_token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    let visible: Vec<String> = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap().to_string())
        .collect();

    assert!(
        visible.contains(&"servimec-tko".to_string()),
        "un usuario con rol 'tecnica' debe poder resolver el perfil 'servimec-tko'; visibles: {visible:?}"
    );
    assert!(
        !visible.contains(&"financiera".to_string()),
        "un usuario con rol 'tecnica' NO debe poder resolver 'financiera' (fail-closed); visibles: {visible:?}"
    );
    // Tampoco debe ver el resto de role-packs ajenos a su rol.
    for other in ["comercial", "gerencia", "admin"] {
        assert!(
            !visible.contains(&other.to_string()),
            "un usuario con rol 'tecnica' no debe ver el role-pack '{other}'"
        );
    }
}

/// Simétrico: un usuario con rol `financiera` resuelve `financiera` pero NO
/// `servimec-tko` — confirma que el gate no es un accidente de orden de
/// inserción sino un ACL real por rol.
#[tokio::test]
async fn financiera_role_resolves_financiera_but_not_servimec_tko() {
    let (mut app, services) = common::build_app().await;

    let (fin_token, _csrf) = setup_and_login(&mut app, &services, "contador1", PW).await;
    let contador = services.user_repo.find_by_username("contador1").await.unwrap().unwrap();
    services
        .user_repo
        .assign_role(&contador.id, "financiera")
        .await
        .unwrap();

    let resp = app
        .clone()
        .oneshot(get_with_token("/api/profiles", &fin_token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    let visible: Vec<String> = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap().to_string())
        .collect();

    assert!(visible.contains(&"financiera".to_string()));
    assert!(!visible.contains(&"servimec-tko".to_string()));
}

/// Un usuario sin ningún rol de negocio asignado no resuelve ningún
/// role-pack — fail-closed también para el caso "sin rol".
#[tokio::test]
async fn user_without_business_role_resolves_no_role_pack() {
    let (mut app, services) = common::build_app().await;
    let (token, _csrf) = setup_and_login(&mut app, &services, "sin_rol", PW).await;

    let resp = app
        .clone()
        .oneshot(get_with_token("/api/profiles", &token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(
        body["data"].as_array().unwrap().len(),
        0,
        "sin rol de negocio asignado, el usuario no debe ver ningún role-pack"
    );
}

/// El `soul_md` de cada role-pack conserva el contenido fusionado del stub
/// `openclaw-<rol>.md` original (verifica que la fusión A5+A6 no se perdió
/// en el seed — spot-check sobre 'servimec-tko' e 'ingenieria').
#[tokio::test]
async fn role_pack_definition_preserves_openclaw_stub_content() {
    let (mut app, services) = common::build_app().await;
    let (admin_token, _csrf) = setup_and_login(&mut app, &services, "admin", PW).await;
    let admin = services.user_repo.find_by_username("admin").await.unwrap().unwrap();
    services.user_repo.assign_role(&admin.id, "admin").await.unwrap();

    let servimec = services
        .profile_repo
        .get_by_name("servimec-tko")
        .await
        .unwrap()
        .expect("servimec-tko debe existir tras el seed");
    assert!(
        servimec.definition.contains("Vertiv/Liebert"),
        "soul_md de servimec-tko debe conservar el contenido del stub openclaw-tecnica.md"
    );
    assert!(
        servimec.definition.contains("\"tecnica\""),
        "acl.roles debe incluir 'tecnica'"
    );

    let ingenieria = services
        .profile_repo
        .get_by_name("ingenieria")
        .await
        .unwrap()
        .expect("ingenieria debe existir tras el seed");
    assert!(
        ingenieria.definition.contains("dxf-takeoff"),
        "mcp_allowlist de ingenieria debe conservar dxf-takeoff del stub openclaw-ingenieria.md"
    );

    // Sanity extra: la respuesta admin también expone el campo (no solo el repo).
    let resp = app
        .clone()
        .oneshot(get_with_token("/api/admin/profiles", &admin_token))
        .await
        .unwrap();
    let body = body_json(resp).await;
    let names: Vec<&str> = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"servimec-tko"));
}
