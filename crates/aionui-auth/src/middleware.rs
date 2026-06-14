#![allow(clippy::disallowed_types)]

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;

use aionui_common::ApiError;
use aionui_db::IUserRepository;

use crate::JwtService;
use crate::extract::extract_token_from_headers;

/// Authenticated user injected into request extensions by the auth middleware.
///
/// Route handlers extract this via `Extension<CurrentUser>` to identify
/// the current user and check their role.
#[derive(Debug, Clone)]
pub struct CurrentUser {
    /// User ID from the database.
    pub id: String,
    /// Login username.
    pub username: String,
    /// RBAC role. Known values: `"admin"`, `"member"`.
    pub role: String,
}

impl CurrentUser {
    /// Returns `true` if this user has the `"admin"` role.
    pub fn is_admin(&self) -> bool {
        self.role == "admin"
    }
}

/// Shared state for the authentication middleware.
#[derive(Clone)]
pub struct AuthState {
    pub jwt_service: Arc<JwtService>,
    pub user_repo: Arc<dyn IUserRepository>,
    /// When `true`, skip JWT verification and inject a fixed default user.
    pub local: bool,
}

/// Authentication middleware that verifies JWT tokens and injects [`CurrentUser`].
///
/// Flow:
/// 1. Extract bearer token from `Authorization` header or `aionui-session` cookie
/// 2. Verify JWT signature, expiration, and blacklist
/// 3. Look up user in the database to ensure they still exist
/// 4. Reject deactivated accounts with 403
/// 5. Insert [`CurrentUser`] into request extensions
///
/// Returns 401 for authentication failures, 403 for deactivated accounts.
///
/// Use with `axum::middleware::from_fn_with_state`.
pub async fn auth_middleware(
    State(state): State<AuthState>,
    mut request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    // In local mode, bypass JWT and inject the system default admin user.
    if state.local {
        request.extensions_mut().insert(CurrentUser {
            id: "system_default_user".to_string(),
            username: "system_default_user".to_string(),
            role: "admin".to_string(),
        });
        return Ok(next.run(request).await);
    }

    let token = extract_token_from_headers(request.headers())
        .ok_or_else(|| ApiError::Unauthorized("Authentication required".into()))?;

    let payload = state.jwt_service.verify(&token).map_err(|e| {
        tracing::debug!("Token verification failed: {e}");
        ApiError::Unauthorized("Invalid or expired token".into())
    })?;

    let user = state
        .user_repo
        .find_by_id(&payload.user_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "auth middleware user lookup failed");
            ApiError::Internal("Authentication service unavailable".into())
        })?
        .ok_or_else(|| ApiError::Unauthorized("Invalid authentication subject".into()))?;

    if !user.is_active {
        return Err(ApiError::Forbidden("Account is deactivated".into()));
    }

    request.extensions_mut().insert(CurrentUser {
        id: user.id,
        username: user.username,
        role: user.role,
    });

    Ok(next.run(request).await)
}

/// Admin-only guard middleware.
///
/// Must be stacked **after** `auth_middleware` so that [`CurrentUser`] is already
/// present in the request extensions. Returns 403 if the user does not have the
/// `"admin"` role.
///
/// Use with `axum::middleware::from_fn`.
pub async fn require_admin_middleware(request: Request, next: Next) -> Result<Response, ApiError> {
    let user = request
        .extensions()
        .get::<CurrentUser>()
        .ok_or_else(|| ApiError::Internal("CurrentUser missing — apply auth_middleware first".into()))?;

    if !user.is_admin() {
        return Err(ApiError::Forbidden("Admin access required".into()));
    }

    Ok(next.run(request).await)
}

/// Local-mode authentication middleware that skips JWT verification.
///
/// Injects a fixed `CurrentUser` with id and username `system_default_user` and
/// role `"admin"`. Used when the server runs as an embedded subprocess inside Electron.
pub async fn local_auth_middleware(mut request: Request, next: Next) -> Response {
    request.extensions_mut().insert(CurrentUser {
        id: "system_default_user".to_string(),
        username: "system_default_user".to_string(),
        role: "admin".to_string(),
    });
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use tower::ServiceExt;

    async fn echo_user(request: Request<Body>) -> String {
        let user = request.extensions().get::<CurrentUser>().unwrap();
        format!("{}:{}:{}", user.id, user.username, user.role)
    }

    #[tokio::test]
    async fn test_local_auth_middleware_injects_default_user() {
        let app = Router::new()
            .route("/test", get(echo_user))
            .route_layer(axum::middleware::from_fn(local_auth_middleware));

        let response = app
            .oneshot(Request::builder().uri("/test").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(
            std::str::from_utf8(&body).unwrap(),
            "system_default_user:system_default_user:admin"
        );
    }

    #[tokio::test]
    async fn test_require_admin_middleware_rejects_member() {
        async fn protected(_req: Request<Body>) -> &'static str {
            "ok"
        }

        // Inject a member user then apply require_admin
        let app = Router::new()
            .route("/admin", get(protected))
            .route_layer(axum::middleware::from_fn(require_admin_middleware))
            .route_layer(axum::middleware::from_fn(
                |mut req: Request<Body>, next: Next| async move {
                    req.extensions_mut().insert(CurrentUser {
                        id: "user_1".to_string(),
                        username: "member_user".to_string(),
                        role: "member".to_string(),
                    });
                    next.run(req).await
                },
            ));

        let response = app
            .oneshot(Request::builder().uri("/admin").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_require_admin_middleware_allows_admin() {
        async fn protected(_req: Request<Body>) -> &'static str {
            "ok"
        }

        let app = Router::new()
            .route("/admin", get(protected))
            .route_layer(axum::middleware::from_fn(require_admin_middleware))
            .route_layer(axum::middleware::from_fn(
                |mut req: Request<Body>, next: Next| async move {
                    req.extensions_mut().insert(CurrentUser {
                        id: "user_1".to_string(),
                        username: "admin_user".to_string(),
                        role: "admin".to_string(),
                    });
                    next.run(req).await
                },
            ));

        let response = app
            .oneshot(Request::builder().uri("/admin").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }
}
