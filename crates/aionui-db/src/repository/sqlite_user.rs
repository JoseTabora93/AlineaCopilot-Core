use sqlx::SqlitePool;

use crate::error::DbError;
use crate::models::{Role, User};
use crate::repository::IUserRepository;
use crate::repository::user::RoleRemoval;

/// Id del rol con privilegios de administración (seed de la migración 013).
/// La invariante anti-lockout de `remove_role` se ancla a este valor.
const ADMIN_ROLE: &str = "admin";

/// SQLite-backed implementation of [`IUserRepository`].
#[derive(Clone, Debug)]
pub struct SqliteUserRepository {
    pool: SqlitePool,
}

impl SqliteUserRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl IUserRepository for SqliteUserRepository {
    async fn has_users(&self) -> Result<bool, DbError> {
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users WHERE password_hash != ''")
            .fetch_one(&self.pool)
            .await?;

        Ok(row.0 > 0)
    }

    async fn get_system_user(&self) -> Result<Option<User>, DbError> {
        let user = sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = 'system_default_user'")
            .fetch_optional(&self.pool)
            .await?;

        Ok(user)
    }

    async fn get_primary_webui_user(&self) -> Result<Option<User>, DbError> {
        // Priority: system default user first
        if let Some(user) = self.get_system_user().await? {
            return Ok(Some(user));
        }

        // Fallback: user named "admin"
        let user = sqlx::query_as::<_, User>("SELECT * FROM users WHERE username = 'admin'")
            .fetch_optional(&self.pool)
            .await?;

        Ok(user)
    }

    async fn set_system_user_credentials(&self, username: &str, password_hash: &str) -> Result<(), DbError> {
        let now = aionui_common::now_ms();
        let result = sqlx::query(
            "UPDATE users SET username = ?, password_hash = ?, role = 'admin', updated_at = ? \
             WHERE id = 'system_default_user'",
        )
        .bind(username)
        .bind(password_hash)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(db_err) if is_unique_violation(db_err.as_ref()) => {
                DbError::Conflict(format!("Username '{username}' already exists"))
            }
            _ => DbError::Query(e),
        })?;

        if result.rows_affected() == 0 {
            return Err(DbError::NotFound("system_default_user not found".to_string()));
        }

        // Multi-rol (Fase 2 #5): el bootstrap pone `users.role = 'admin'`, pero el
        // gate de admin lee `user_roles`. Sin esta fila, el admin recién creado por
        // el setup quedaría bloqueado (roles=[] → no admin). Materializa el rol.
        sqlx::query(
            "INSERT OR IGNORE INTO user_roles (user_id, role_id, created_at) VALUES ('system_default_user', ?, ?)",
        )
        .bind(ADMIN_ROLE)
        .bind(now)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn create_user(&self, username: &str, password_hash: &str) -> Result<User, DbError> {
        self.create_user_full(username, password_hash, None, None, "member")
            .await
    }

    async fn create_user_full(
        &self,
        username: &str,
        password_hash: &str,
        email: Option<&str>,
        display_name: Option<&str>,
        role: &str,
    ) -> Result<User, DbError> {
        let id = aionui_common::generate_prefixed_id("user");
        let now = aionui_common::now_ms();

        sqlx::query(
            "INSERT INTO users \
             (id, username, password_hash, email, display_name, role, is_active, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, 1, ?, ?)",
        )
        .bind(&id)
        .bind(username)
        .bind(password_hash)
        .bind(email)
        .bind(display_name)
        .bind(role)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(db_err) if is_unique_violation(db_err.as_ref()) => {
                DbError::Conflict(format!("Username '{username}' already exists"))
            }
            _ => DbError::Query(e),
        })?;

        Ok(User {
            id,
            username: username.to_string(),
            email: email.map(str::to_string),
            password_hash: password_hash.to_string(),
            avatar_path: None,
            jwt_secret: None,
            created_at: now,
            updated_at: now,
            last_login: None,
            role: role.to_string(),
            is_active: true,
            display_name: display_name.map(str::to_string),
        })
    }

    async fn find_by_username(&self, username: &str) -> Result<Option<User>, DbError> {
        let user = sqlx::query_as::<_, User>("SELECT * FROM users WHERE username = ?")
            .bind(username)
            .fetch_optional(&self.pool)
            .await?;

        Ok(user)
    }

    async fn find_by_id(&self, id: &str) -> Result<Option<User>, DbError> {
        let user = sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;

        Ok(user)
    }

    async fn list_users(&self) -> Result<Vec<User>, DbError> {
        let users = sqlx::query_as::<_, User>("SELECT * FROM users ORDER BY created_at ASC")
            .fetch_all(&self.pool)
            .await?;

        Ok(users)
    }

    async fn count_users(&self) -> Result<i64, DbError> {
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
            .fetch_one(&self.pool)
            .await?;

        Ok(row.0)
    }

    async fn update_password(&self, user_id: &str, password_hash: &str) -> Result<(), DbError> {
        let now = aionui_common::now_ms();
        let result = sqlx::query("UPDATE users SET password_hash = ?, updated_at = ? WHERE id = ?")
            .bind(password_hash)
            .bind(now)
            .bind(user_id)
            .execute(&self.pool)
            .await?;

        if result.rows_affected() == 0 {
            return Err(DbError::NotFound(format!("User '{user_id}' not found")));
        }

        Ok(())
    }

    async fn update_username(&self, user_id: &str, username: &str) -> Result<(), DbError> {
        let now = aionui_common::now_ms();
        let result = sqlx::query("UPDATE users SET username = ?, updated_at = ? WHERE id = ?")
            .bind(username)
            .bind(now)
            .bind(user_id)
            .execute(&self.pool)
            .await
            .map_err(|e| match &e {
                sqlx::Error::Database(db_err) if is_unique_violation(db_err.as_ref()) => {
                    DbError::Conflict(format!("Username '{username}' already exists"))
                }
                _ => DbError::Query(e),
            })?;

        if result.rows_affected() == 0 {
            return Err(DbError::NotFound(format!("User '{user_id}' not found")));
        }

        Ok(())
    }

    async fn update_last_login(&self, user_id: &str) -> Result<(), DbError> {
        let now = aionui_common::now_ms();
        let result = sqlx::query("UPDATE users SET last_login = ?, updated_at = ? WHERE id = ?")
            .bind(now)
            .bind(now)
            .bind(user_id)
            .execute(&self.pool)
            .await?;

        if result.rows_affected() == 0 {
            return Err(DbError::NotFound(format!("User '{user_id}' not found")));
        }

        Ok(())
    }

    async fn update_jwt_secret(&self, user_id: &str, jwt_secret: &str) -> Result<(), DbError> {
        let now = aionui_common::now_ms();
        let result = sqlx::query("UPDATE users SET jwt_secret = ?, updated_at = ? WHERE id = ?")
            .bind(jwt_secret)
            .bind(now)
            .bind(user_id)
            .execute(&self.pool)
            .await?;

        if result.rows_affected() == 0 {
            return Err(DbError::NotFound(format!("User '{user_id}' not found")));
        }

        Ok(())
    }

    async fn get_user_roles(&self, user_id: &str) -> Result<Vec<String>, DbError> {
        let rows: Vec<(String,)> = sqlx::query_as("SELECT role_id FROM user_roles WHERE user_id = ?")
            .bind(user_id)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(|(role_id,)| role_id).collect())
    }

    async fn assign_role(&self, user_id: &str, role_id: &str) -> Result<(), DbError> {
        let now = aionui_common::now_ms();
        sqlx::query("INSERT OR IGNORE INTO user_roles (user_id, role_id, created_at) VALUES (?, ?, ?)")
            .bind(user_id)
            .bind(role_id)
            .bind(now)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn remove_role(&self, user_id: &str, role_id: &str) -> Result<RoleRemoval, DbError> {
        // Roles no-admin: borrado simple idempotente, sin invariante.
        if role_id != ADMIN_ROLE {
            sqlx::query("DELETE FROM user_roles WHERE user_id = ? AND role_id = ?")
                .bind(user_id)
                .bind(role_id)
                .execute(&self.pool)
                .await?;
            return Ok(RoleRemoval::Removed);
        }

        // Rol admin: el chequeo "quedaría >=1 admin" y el borrado van en UNA sola
        // sentencia. SQLite serializa escrituras, así que dos removals concurrentes
        // no pueden ambos ver el conteo viejo y dejar el sistema en 0 admins.
        let deleted = sqlx::query(
            "DELETE FROM user_roles \
             WHERE user_id = ? AND role_id = ? \
               AND (SELECT COUNT(*) FROM user_roles WHERE role_id = ?) > 1",
        )
        .bind(user_id)
        .bind(role_id)
        .bind(role_id)
        .execute(&self.pool)
        .await?
        .rows_affected();
        if deleted >= 1 {
            return Ok(RoleRemoval::Removed);
        }

        // 0 filas borradas: o el usuario no tenía admin (no-op idempotente), o era
        // el último admin (bloqueado). Lo distingue una lectura post-hoc — ya no es
        // sensible al race: el borrado atómico de arriba ya ocurrió o no.
        let still_admin =
            sqlx::query_scalar::<_, i64>("SELECT 1 FROM user_roles WHERE user_id = ? AND role_id = ? LIMIT 1")
                .bind(user_id)
                .bind(role_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(if still_admin.is_some() {
            RoleRemoval::WouldLeaveNoAdmins
        } else {
            RoleRemoval::Removed
        })
    }

    async fn list_roles(&self) -> Result<Vec<Role>, DbError> {
        let roles = sqlx::query_as::<_, Role>("SELECT id, name, label, created_at FROM roles ORDER BY id")
            .fetch_all(&self.pool)
            .await?;
        Ok(roles)
    }

    async fn count_users_with_role(&self, role_id: &str) -> Result<i64, DbError> {
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM user_roles WHERE role_id = ?")
            .bind(role_id)
            .fetch_one(&self.pool)
            .await?;
        Ok(row.0)
    }

    async fn set_active(&self, user_id: &str, is_active: bool) -> Result<(), DbError> {
        let now = aionui_common::now_ms();
        let result = sqlx::query("UPDATE users SET is_active = ?, updated_at = ? WHERE id = ?")
            .bind(is_active)
            .bind(now)
            .bind(user_id)
            .execute(&self.pool)
            .await?;

        if result.rows_affected() == 0 {
            return Err(DbError::NotFound(format!("User '{user_id}' not found")));
        }

        Ok(())
    }

    async fn set_role(&self, user_id: &str, role: &str) -> Result<(), DbError> {
        let now = aionui_common::now_ms();
        let result = sqlx::query("UPDATE users SET role = ?, updated_at = ? WHERE id = ?")
            .bind(role)
            .bind(now)
            .bind(user_id)
            .execute(&self.pool)
            .await?;

        if result.rows_affected() == 0 {
            return Err(DbError::NotFound(format!("User '{user_id}' not found")));
        }

        Ok(())
    }

    async fn set_display_name(&self, user_id: &str, display_name: Option<&str>) -> Result<(), DbError> {
        let now = aionui_common::now_ms();
        let result = sqlx::query("UPDATE users SET display_name = ?, updated_at = ? WHERE id = ?")
            .bind(display_name)
            .bind(now)
            .bind(user_id)
            .execute(&self.pool)
            .await?;

        if result.rows_affected() == 0 {
            return Err(DbError::NotFound(format!("User '{user_id}' not found")));
        }

        Ok(())
    }

    async fn delete_user(&self, user_id: &str) -> Result<(), DbError> {
        // Dependent rows are removed by `ON DELETE CASCADE`:
        //   conversations.user_id -> users.id            (cascade)
        //   messages.conversation_id -> conversations.id (cascade)
        //   conversation_artifacts.conversation_id       (cascade)
        // The runtime pool enables `PRAGMA foreign_keys = ON`, so a single
        // DELETE on `users` is sufficient — no manual transaction required.
        let result = sqlx::query("DELETE FROM users WHERE id = ?")
            .bind(user_id)
            .execute(&self.pool)
            .await?;

        if result.rows_affected() == 0 {
            return Err(DbError::NotFound(format!("User '{user_id}' not found")));
        }

        Ok(())
    }
}

/// Checks if a SQLite database error is a UNIQUE constraint violation.
fn is_unique_violation(err: &dyn sqlx::error::DatabaseError) -> bool {
    // SQLite error code 2067 = SQLITE_CONSTRAINT_UNIQUE
    err.code().is_some_and(|c| c == "2067")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::init_database_memory;

    async fn setup() -> (SqliteUserRepository, crate::Database) {
        let db = init_database_memory().await.unwrap();
        let repo = SqliteUserRepository::new(db.pool().clone());
        (repo, db)
    }

    // -- Unit tests for is_unique_violation helper --

    #[test]
    fn unique_violation_code_detected() {
        assert!(is_unique_violation(&FakeDbError("2067")));
    }

    #[test]
    fn non_unique_violation_code_rejected() {
        assert!(!is_unique_violation(&FakeDbError("1555")));
    }

    struct FakeDbError(&'static str);

    impl std::fmt::Display for FakeDbError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "fake error")
        }
    }

    impl std::fmt::Debug for FakeDbError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "FakeDbError({})", self.0)
        }
    }

    impl std::error::Error for FakeDbError {}

    impl sqlx::error::DatabaseError for FakeDbError {
        fn message(&self) -> &str {
            "fake"
        }
        fn kind(&self) -> sqlx::error::ErrorKind {
            sqlx::error::ErrorKind::UniqueViolation
        }
        fn code(&self) -> Option<std::borrow::Cow<'_, str>> {
            Some(std::borrow::Cow::Borrowed(self.0))
        }
        fn as_error(&self) -> &(dyn std::error::Error + Send + Sync + 'static) {
            self
        }
        fn as_error_mut(&mut self) -> &mut (dyn std::error::Error + Send + Sync + 'static) {
            self
        }
        fn into_error(self: Box<Self>) -> Box<dyn std::error::Error + Send + Sync + 'static> {
            self
        }
    }

    // -- Integration tests against in-memory SQLite --

    #[tokio::test]
    async fn create_user_returns_populated_fields() {
        let (repo, _db) = setup().await;
        let user = repo.create_user("alice", "hash123").await.unwrap();

        assert!(user.id.starts_with("user_"));
        assert_eq!(user.username, "alice");
        assert_eq!(user.password_hash, "hash123");
        assert!(user.email.is_none());
        assert!(user.avatar_path.is_none());
        assert!(user.jwt_secret.is_none());
        assert!(user.last_login.is_none());
        assert!(user.created_at > 0);
        assert_eq!(user.created_at, user.updated_at);
        // New fields default
        assert_eq!(user.role, "member");
        assert!(user.is_active);
        assert!(user.display_name.is_none());
    }

    #[tokio::test]
    async fn create_user_duplicate_username_returns_conflict() {
        let (repo, _db) = setup().await;
        repo.create_user("bob", "h1").await.unwrap();

        let err = repo.create_user("bob", "h2").await.unwrap_err();
        assert!(matches!(err, DbError::Conflict(_)));
    }

    #[tokio::test]
    async fn has_users_false_when_only_system_user() {
        let (repo, _db) = setup().await;
        assert!(!repo.has_users().await.unwrap());
    }

    #[tokio::test]
    async fn has_users_true_after_creating_real_user() {
        let (repo, _db) = setup().await;
        repo.create_user("real", "pass").await.unwrap();
        assert!(repo.has_users().await.unwrap());
    }

    #[tokio::test]
    async fn get_system_user_returns_default() {
        let (repo, _db) = setup().await;
        let user = repo.get_system_user().await.unwrap().unwrap();
        assert_eq!(user.id, "system_default_user");
        assert_eq!(user.username, "admin");
        // Seeded with role=admin by ensure_system_user
        assert_eq!(user.role, "admin");
        assert!(user.is_active);
    }

    #[tokio::test]
    async fn get_primary_webui_user_returns_system_user_first() {
        let (repo, _db) = setup().await;
        repo.create_user("other", "hash").await.unwrap();

        let user = repo.get_primary_webui_user().await.unwrap().unwrap();
        assert_eq!(user.id, "system_default_user");
    }

    #[tokio::test]
    async fn find_by_username_existing() {
        let (repo, _db) = setup().await;
        repo.create_user("charlie", "h").await.unwrap();

        let found = repo.find_by_username("charlie").await.unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().username, "charlie");
    }

    #[tokio::test]
    async fn find_by_username_missing() {
        let (repo, _db) = setup().await;
        assert!(repo.find_by_username("ghost").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn find_by_id_existing() {
        let (repo, _db) = setup().await;
        let created = repo.create_user("dave", "h").await.unwrap();

        let found = repo.find_by_id(&created.id).await.unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, created.id);
    }

    #[tokio::test]
    async fn find_by_id_missing() {
        let (repo, _db) = setup().await;
        assert!(repo.find_by_id("nonexistent").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn list_users_includes_system_and_created() {
        let (repo, _db) = setup().await;
        repo.create_user("eve", "h").await.unwrap();
        repo.create_user("frank", "h").await.unwrap();

        let users = repo.list_users().await.unwrap();
        // system_default_user + eve + frank
        assert_eq!(users.len(), 3);
    }

    #[tokio::test]
    async fn count_users_includes_all() {
        let (repo, _db) = setup().await;
        repo.create_user("grace", "h").await.unwrap();

        // system_default_user + grace
        assert_eq!(repo.count_users().await.unwrap(), 2);
    }

    #[tokio::test]
    async fn update_password_succeeds() {
        let (repo, _db) = setup().await;
        let user = repo.create_user("hal", "old_hash").await.unwrap();

        repo.update_password(&user.id, "new_hash").await.unwrap();

        let updated = repo.find_by_id(&user.id).await.unwrap().unwrap();
        assert_eq!(updated.password_hash, "new_hash");
        assert!(updated.updated_at >= user.updated_at);
    }

    #[tokio::test]
    async fn update_password_nonexistent_user() {
        let (repo, _db) = setup().await;
        let err = repo.update_password("no_such_id", "h").await.unwrap_err();
        assert!(matches!(err, DbError::NotFound(_)));
    }

    #[tokio::test]
    async fn update_username_succeeds() {
        let (repo, _db) = setup().await;
        let user = repo.create_user("ivan", "h").await.unwrap();

        repo.update_username(&user.id, "ivan_new").await.unwrap();

        let updated = repo.find_by_id(&user.id).await.unwrap().unwrap();
        assert_eq!(updated.username, "ivan_new");
    }

    #[tokio::test]
    async fn update_username_conflict() {
        let (repo, _db) = setup().await;
        repo.create_user("jane", "h").await.unwrap();
        let other = repo.create_user("kate", "h").await.unwrap();

        let err = repo.update_username(&other.id, "jane").await.unwrap_err();
        assert!(matches!(err, DbError::Conflict(_)));
    }

    #[tokio::test]
    async fn update_last_login_sets_timestamp() {
        let (repo, _db) = setup().await;
        let user = repo.create_user("leo", "h").await.unwrap();
        assert!(user.last_login.is_none());

        repo.update_last_login(&user.id).await.unwrap();

        let updated = repo.find_by_id(&user.id).await.unwrap().unwrap();
        assert!(updated.last_login.is_some());
        assert!(updated.last_login.unwrap() > 0);
    }

    #[tokio::test]
    async fn update_jwt_secret_succeeds() {
        let (repo, _db) = setup().await;
        let user = repo.create_user("mike", "h").await.unwrap();
        assert!(user.jwt_secret.is_none());

        repo.update_jwt_secret(&user.id, "secret123").await.unwrap();

        let updated = repo.find_by_id(&user.id).await.unwrap().unwrap();
        assert_eq!(updated.jwt_secret.as_deref(), Some("secret123"));
    }

    #[tokio::test]
    async fn set_system_user_credentials_conflict_with_existing_username() {
        let (repo, _db) = setup().await;
        repo.create_user("taken", "h").await.unwrap();

        let err = repo.set_system_user_credentials("taken", "hash").await.unwrap_err();
        assert!(matches!(err, DbError::Conflict(_)));
    }

    #[tokio::test]
    async fn set_system_user_credentials_updates_fields() {
        let (repo, _db) = setup().await;

        repo.set_system_user_credentials("admin", "secure_hash").await.unwrap();

        let user = repo.get_system_user().await.unwrap().unwrap();
        assert_eq!(user.username, "admin");
        assert_eq!(user.password_hash, "secure_hash");
        // Role must remain admin (legacy column)
        assert_eq!(user.role, "admin");
        // Multi-rol (Fase 2 #5): el bootstrap también materializa el rol admin en
        // user_roles, no solo en users.role — si no, el gate multi-rol (que lee
        // user_roles) bloquearía al admin recién creado por el setup.
        assert!(
            repo.get_user_roles("system_default_user")
                .await
                .unwrap()
                .contains(&"admin".to_string()),
            "el admin del bootstrap debe tener el rol en user_roles"
        );
    }

    // -- Tests for new multiuser fields (spec section 8) --

    #[tokio::test]
    async fn create_user_full_with_admin_role() {
        let (repo, _db) = setup().await;
        let user = repo
            .create_user_full(
                "superadmin",
                "hash",
                Some("sa@example.com"),
                Some("Super Admin"),
                "admin",
            )
            .await
            .unwrap();

        assert_eq!(user.role, "admin");
        assert!(user.is_active);
        assert_eq!(user.email.as_deref(), Some("sa@example.com"));
        assert_eq!(user.display_name.as_deref(), Some("Super Admin"));
    }

    #[tokio::test]
    async fn create_user_full_defaults_member_role() {
        let (repo, _db) = setup().await;
        let user = repo
            .create_user_full("newbie", "h", None, None, "member")
            .await
            .unwrap();
        assert_eq!(user.role, "member");
        assert!(user.is_active);
        assert!(user.display_name.is_none());
    }

    #[tokio::test]
    async fn set_active_false_persists() {
        let (repo, _db) = setup().await;
        let user = repo.create_user("nina", "h").await.unwrap();
        assert!(user.is_active);

        repo.set_active(&user.id, false).await.unwrap();

        let updated = repo.find_by_id(&user.id).await.unwrap().unwrap();
        assert!(!updated.is_active);
    }

    #[tokio::test]
    async fn set_active_nonexistent_user() {
        let (repo, _db) = setup().await;
        let err = repo.set_active("no_such_id", false).await.unwrap_err();
        assert!(matches!(err, DbError::NotFound(_)));
    }

    #[tokio::test]
    async fn set_role_updates_field() {
        let (repo, _db) = setup().await;
        let user = repo.create_user("oscar", "h").await.unwrap();
        assert_eq!(user.role, "member");

        repo.set_role(&user.id, "admin").await.unwrap();

        let updated = repo.find_by_id(&user.id).await.unwrap().unwrap();
        assert_eq!(updated.role, "admin");
    }

    #[tokio::test]
    async fn set_display_name_roundtrip() {
        let (repo, _db) = setup().await;
        let user = repo.create_user("paula", "h").await.unwrap();
        assert!(user.display_name.is_none());

        repo.set_display_name(&user.id, Some("Paula Smith")).await.unwrap();
        let updated = repo.find_by_id(&user.id).await.unwrap().unwrap();
        assert_eq!(updated.display_name.as_deref(), Some("Paula Smith"));

        repo.set_display_name(&user.id, None).await.unwrap();
        let cleared = repo.find_by_id(&user.id).await.unwrap().unwrap();
        assert!(cleared.display_name.is_none());
    }

    #[tokio::test]
    async fn system_default_user_has_admin_role() {
        let (repo, _db) = setup().await;
        let user = repo.get_system_user().await.unwrap().unwrap();
        assert_eq!(user.role, "admin");
        assert!(user.is_active);
    }

    #[tokio::test]
    async fn delete_user_removes_row() {
        let (repo, _db) = setup().await;
        let user = repo.create_user("quincy", "h").await.unwrap();

        repo.delete_user(&user.id).await.unwrap();

        assert!(repo.find_by_id(&user.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn delete_user_nonexistent_returns_not_found() {
        let (repo, _db) = setup().await;
        let err = repo.delete_user("no_such_id").await.unwrap_err();
        assert!(matches!(err, DbError::NotFound(_)));
    }

    // -- RBAC eje 1: roles (Fase 2 #5) --

    #[tokio::test]
    async fn assign_get_remove_role_roundtrip() {
        let (repo, _db) = setup().await;
        let user = repo.create_user("nora", "h").await.unwrap();

        assert!(repo.get_user_roles(&user.id).await.unwrap().is_empty());

        repo.assign_role(&user.id, "gerencia").await.unwrap();
        // idempotente: reasignar no duplica
        repo.assign_role(&user.id, "gerencia").await.unwrap();
        assert_eq!(repo.get_user_roles(&user.id).await.unwrap(), vec!["gerencia"]);

        assert_eq!(
            repo.remove_role(&user.id, "gerencia").await.unwrap(),
            RoleRemoval::Removed
        );
        assert!(repo.get_user_roles(&user.id).await.unwrap().is_empty());
        // idempotente: quitar lo que no está no es error
        assert_eq!(
            repo.remove_role(&user.id, "gerencia").await.unwrap(),
            RoleRemoval::Removed
        );
    }

    #[tokio::test]
    async fn remove_role_protects_last_admin() {
        let (repo, _db) = setup().await;
        let a = repo.create_user("solo", "h").await.unwrap();
        repo.assign_role(&a.id, "admin").await.unwrap();

        // Único admin → bloqueado, fila intacta.
        assert_eq!(
            repo.remove_role(&a.id, "admin").await.unwrap(),
            RoleRemoval::WouldLeaveNoAdmins
        );
        assert_eq!(repo.get_user_roles(&a.id).await.unwrap(), vec!["admin"]);

        // Con un segundo admin, quitarle el rol a uno SÍ se permite.
        let b = repo.create_user("dos", "h").await.unwrap();
        repo.assign_role(&b.id, "admin").await.unwrap();
        assert_eq!(repo.remove_role(&a.id, "admin").await.unwrap(), RoleRemoval::Removed);
        assert_eq!(repo.count_users_with_role("admin").await.unwrap(), 1);

        // Quitar admin a quien ya no lo tiene = no-op idempotente.
        assert_eq!(repo.remove_role(&a.id, "admin").await.unwrap(), RoleRemoval::Removed);
    }

    #[tokio::test]
    async fn list_roles_returns_six_seeded() {
        let (repo, _db) = setup().await;
        let roles = repo.list_roles().await.unwrap();
        assert_eq!(roles.len(), 6);
        let ids: Vec<&str> = roles.iter().map(|r| r.id.as_str()).collect();
        for expected in ["admin", "comercial", "financiera", "gerencia", "ingenieria", "tecnica"] {
            assert!(ids.contains(&expected), "falta el rol '{expected}'");
        }
    }

    #[tokio::test]
    async fn count_users_with_role_counts_assignments() {
        let (repo, _db) = setup().await;
        let a = repo.create_user("a", "h").await.unwrap();
        let b = repo.create_user("b", "h").await.unwrap();

        assert_eq!(repo.count_users_with_role("admin").await.unwrap(), 0);
        repo.assign_role(&a.id, "admin").await.unwrap();
        repo.assign_role(&b.id, "admin").await.unwrap();
        assert_eq!(repo.count_users_with_role("admin").await.unwrap(), 2);

        repo.remove_role(&a.id, "admin").await.unwrap();
        assert_eq!(repo.count_users_with_role("admin").await.unwrap(), 1);
    }
}
