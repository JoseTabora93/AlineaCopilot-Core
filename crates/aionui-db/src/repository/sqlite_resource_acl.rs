//! Implementación SQLite de `IResourceAclRepository`.

use aionui_common::{generate_id, now_ms};
use sqlx::SqlitePool;

use crate::DbError;
use crate::models::ResourceAclRow;
use crate::repository::resource_acl::{IResourceAclRepository, MemberRevoke};

pub struct SqliteResourceAclRepository {
    pool: SqlitePool,
}

impl SqliteResourceAclRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl IResourceAclRepository for SqliteResourceAclRepository {
    async fn is_member(&self, resource_type: &str, resource_id: &str, user_id: &str) -> Result<bool, DbError> {
        let row: Option<(i64,)> = sqlx::query_as(
            "SELECT 1 FROM resource_acl \
             WHERE resource_type = ? AND resource_id = ? \
               AND principal_type = 'user' AND principal_id = ? LIMIT 1",
        )
        .bind(resource_type)
        .bind(resource_id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.is_some())
    }

    async fn get_perm(&self, resource_type: &str, resource_id: &str, user_id: &str) -> Result<Option<String>, DbError> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT perm FROM resource_acl \
             WHERE resource_type = ? AND resource_id = ? \
               AND principal_type = 'user' AND principal_id = ? LIMIT 1",
        )
        .bind(resource_type)
        .bind(resource_id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(perm,)| perm))
    }

    async fn grant(&self, resource_type: &str, resource_id: &str, user_id: &str, perm: &str) -> Result<(), DbError> {
        // Upsert: si ya existe la entrada (UNIQUE), actualiza el perm.
        sqlx::query(
            "INSERT INTO resource_acl \
               (id, resource_type, resource_id, principal_type, principal_id, perm, created_at) \
             VALUES (?, ?, ?, 'user', ?, ?, ?) \
             ON CONFLICT(resource_type, resource_id, principal_type, principal_id) \
             DO UPDATE SET perm = excluded.perm",
        )
        .bind(generate_id())
        .bind(resource_type)
        .bind(resource_id)
        .bind(user_id)
        .bind(perm)
        .bind(now_ms())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn revoke(&self, resource_type: &str, resource_id: &str, user_id: &str) -> Result<(), DbError> {
        sqlx::query(
            "DELETE FROM resource_acl \
             WHERE resource_type = ? AND resource_id = ? \
               AND principal_type = 'user' AND principal_id = ?",
        )
        .bind(resource_type)
        .bind(resource_id)
        .bind(user_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn try_revoke_project_member(&self, project_id: &str, user_id: &str) -> Result<MemberRevoke, DbError> {
        // Borra al miembro SALVO que sea el único owner restante. Atómico: una
        // sola sentencia condicional; SQLite serializa escrituras → sin TOCTOU.
        let res = sqlx::query(
            "DELETE FROM resource_acl \
             WHERE resource_type = 'project' AND resource_id = ? \
               AND principal_type = 'user' AND principal_id = ? \
               AND (perm != 'owner' OR (SELECT COUNT(*) FROM resource_acl \
                    WHERE resource_type = 'project' AND resource_id = ? \
                      AND principal_type = 'user' AND perm = 'owner') > 1)",
        )
        .bind(project_id)
        .bind(user_id)
        .bind(project_id)
        .execute(&self.pool)
        .await?;
        if res.rows_affected() > 0 {
            return Ok(MemberRevoke::Revoked);
        }
        // 0 filas: o no era miembro (idempotente → Revoked) o era el último owner
        // (la guarda lo preservó → WouldLeaveNoOwner). Se distingue por el perm.
        match self.get_perm("project", project_id, user_id).await? {
            Some(perm) if perm == "owner" => Ok(MemberRevoke::WouldLeaveNoOwner),
            _ => Ok(MemberRevoke::Revoked),
        }
    }

    async fn list_principals(&self, resource_type: &str, resource_id: &str) -> Result<Vec<ResourceAclRow>, DbError> {
        let rows = sqlx::query_as::<_, ResourceAclRow>(
            "SELECT id, resource_type, resource_id, principal_type, principal_id, perm, created_at \
             FROM resource_acl WHERE resource_type = ? AND resource_id = ? \
             ORDER BY created_at ASC",
        )
        .bind(resource_type)
        .bind(resource_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::init_database_memory;

    async fn setup() -> SqliteResourceAclRepository {
        let db = init_database_memory().await.unwrap();
        SqliteResourceAclRepository::new(db.pool().clone())
    }

    #[tokio::test]
    async fn grant_makes_member_with_perm() {
        let repo = setup().await;
        assert!(!repo.is_member("project", "p1", "u1").await.unwrap());
        repo.grant("project", "p1", "u1", "owner").await.unwrap();
        assert!(repo.is_member("project", "p1", "u1").await.unwrap());
        assert!(repo.is_project_member("u1", "p1").await.unwrap());
        assert_eq!(
            repo.get_perm("project", "p1", "u1").await.unwrap().as_deref(),
            Some("owner")
        );
    }

    #[tokio::test]
    async fn grant_is_idempotent_upsert() {
        let repo = setup().await;
        repo.grant("project", "p1", "u1", "read").await.unwrap();
        repo.grant("project", "p1", "u1", "owner").await.unwrap();
        assert_eq!(
            repo.get_perm("project", "p1", "u1").await.unwrap().as_deref(),
            Some("owner")
        );
        // El upsert NO duplica la fila (UNIQUE en resource_acl).
        assert_eq!(repo.list_principals("project", "p1").await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn revoke_removes_member() {
        let repo = setup().await;
        repo.grant("project", "p1", "u1", "owner").await.unwrap();
        repo.revoke("project", "p1", "u1").await.unwrap();
        assert!(!repo.is_member("project", "p1", "u1").await.unwrap());
        assert!(repo.get_perm("project", "p1", "u1").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn membership_is_scoped_per_resource_and_user() {
        let repo = setup().await;
        repo.grant("project", "p1", "u1", "owner").await.unwrap();
        // Otro usuario en el mismo proyecto: no.
        assert!(!repo.is_member("project", "p1", "u2").await.unwrap());
        // El mismo usuario en otro proyecto: no.
        assert!(!repo.is_member("project", "p2", "u1").await.unwrap());
    }

    #[tokio::test]
    async fn try_revoke_preserves_last_owner() {
        let repo = setup().await;
        repo.grant("project", "p1", "owner1", "owner").await.unwrap();
        repo.grant("project", "p1", "member1", "read").await.unwrap();
        // Quitar a un no-owner: procede.
        assert_eq!(
            repo.try_revoke_project_member("p1", "member1").await.unwrap(),
            MemberRevoke::Revoked
        );
        // Quitar al ÚNICO owner: bloqueado (preserva administrabilidad).
        assert_eq!(
            repo.try_revoke_project_member("p1", "owner1").await.unwrap(),
            MemberRevoke::WouldLeaveNoOwner
        );
        assert!(repo.is_member("project", "p1", "owner1").await.unwrap());
        // Con dos owners, se puede quitar uno.
        repo.grant("project", "p1", "owner2", "owner").await.unwrap();
        assert_eq!(
            repo.try_revoke_project_member("p1", "owner1").await.unwrap(),
            MemberRevoke::Revoked
        );
        assert!(!repo.is_member("project", "p1", "owner1").await.unwrap());
    }
}
