use crate::error::DbError;
use crate::models::{Role, User};

/// Resultado de [`IUserRepository::remove_role`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoleRemoval {
    /// El rol se quitó, o el usuario no lo tenía (no-op idempotente).
    Removed,
    /// Bloqueado: quitar el rol `admin` dejaría el sistema sin ningún
    /// administrador. La fila NO se tocó. Invariante anti-lockout (Fase 2 #5).
    WouldLeaveNoAdmins,
}

/// User data access abstraction.
///
/// All methods return `Result<_, DbError>` so callers can map database
/// failures into their owning error type.
///
/// Object-safe via `async_trait` to support `Arc<dyn IUserRepository>`.
#[async_trait::async_trait]
pub trait IUserRepository: Send + Sync {
    /// Returns `true` if at least one user with a non-empty password exists.
    ///
    /// The system default user (empty password_hash) does not count.
    async fn has_users(&self) -> Result<bool, DbError>;

    /// Returns the system default user (`id = "system_default_user"`).
    async fn get_system_user(&self) -> Result<Option<User>, DbError>;

    /// Returns the primary WebUI user.
    ///
    /// Priority: system default user first, then falls back to a user named "admin".
    async fn get_primary_webui_user(&self) -> Result<Option<User>, DbError>;

    /// Updates the system default user's username and password hash.
    ///
    /// Used during the initial bootstrap flow.
    async fn set_system_user_credentials(&self, username: &str, password_hash: &str) -> Result<(), DbError>;

    /// Creates a new user and returns the inserted row.
    ///
    /// Returns `DbError::Conflict` if the username already exists.
    async fn create_user(&self, username: &str, password_hash: &str) -> Result<User, DbError>;

    /// Finds a user by username.
    async fn find_by_username(&self, username: &str) -> Result<Option<User>, DbError>;

    /// Finds a user by ID.
    async fn find_by_id(&self, id: &str) -> Result<Option<User>, DbError>;

    /// Lists all users.
    async fn list_users(&self) -> Result<Vec<User>, DbError>;

    /// Returns the total number of users.
    async fn count_users(&self) -> Result<i64, DbError>;

    /// Updates a user's password hash.
    async fn update_password(&self, user_id: &str, password_hash: &str) -> Result<(), DbError>;

    /// Updates a user's username.
    ///
    /// Returns `DbError::Conflict` if the new username already exists.
    async fn update_username(&self, user_id: &str, username: &str) -> Result<(), DbError>;

    /// Updates a user's last login timestamp to the current time.
    async fn update_last_login(&self, user_id: &str) -> Result<(), DbError>;

    /// Updates a user's JWT secret.
    async fn update_jwt_secret(&self, user_id: &str, jwt_secret: &str) -> Result<(), DbError>;

    /// Returns the role ids assigned to a user (from `user_roles`). Empty if none.
    ///
    /// RBAC eje 1 (rol). Usado por el auth middleware para poblar `CurrentUser.roles`.
    async fn get_user_roles(&self, user_id: &str) -> Result<Vec<String>, DbError>;

    /// Asigna `role_id` a `user_id` (idempotente). Ambos deben existir: hay FK a
    /// `users(id)` y `roles(id)`. RBAC eje 1 — habilita poblar `user_roles`.
    async fn assign_role(&self, user_id: &str, role_id: &str) -> Result<(), DbError>;

    /// Quita `role_id` de `user_id` de forma **atómica** (idempotente: no error
    /// si no lo tenía). RBAC eje 1 — usado por el panel admin.
    ///
    /// Preserva la invariante "el sistema nunca queda sin administradores": si
    /// `role_id == "admin"` y el usuario fuese el último admin, NO se quita y se
    /// devuelve [`RoleRemoval::WouldLeaveNoAdmins`]. El chequeo y el borrado
    /// ocurren en una sola sentencia SQL para evitar el race TOCTOU de dos
    /// removals concurrentes que veían `count = 2` y dejaban el sistema en 0.
    async fn remove_role(&self, user_id: &str, role_id: &str) -> Result<RoleRemoval, DbError>;

    /// Devuelve el catálogo de roles (tabla `roles`), ordenado por `id`.
    /// Fuente de verdad para los chips/checkboxes del panel admin.
    async fn list_roles(&self) -> Result<Vec<Role>, DbError>;

    /// Cuenta cuántos usuarios tienen asignado `role_id`. Sostiene invariantes
    /// como "el sistema nunca queda sin administradores".
    async fn count_users_with_role(&self, role_id: &str) -> Result<i64, DbError>;
}
