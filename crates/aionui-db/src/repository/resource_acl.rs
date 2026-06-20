//! Repositorio de `resource_acl` (eje 2: ownership/membership por recurso).
//!
//! Slice 3 de Proyectos: la membresía de proyecto vive aquí (resource_type=
//! 'project'). El gate fail-closed de `ConversationService::update` consulta
//! `is_project_member` antes de asignar un proyecto a una conversación.

use crate::DbError;
use crate::models::ResourceAclRow;

// `perm` ∈ 'read' | 'write' | 'owner' (de menor a mayor). Los handlers validan
// el literal; no se exporta una constante para evitar acoplamiento innecesario.

/// Resultado de `try_revoke_project_member` (guarda anti-lockout atómica).
#[derive(Debug, PartialEq, Eq)]
pub enum MemberRevoke {
    /// Se revocó (o ya no era miembro: idempotente).
    Revoked,
    /// Era el último owner; NO se revocó (se preservó la administrabilidad).
    WouldLeaveNoOwner,
}

#[async_trait::async_trait]
pub trait IResourceAclRepository: Send + Sync {
    /// ¿`user_id` (principal de tipo 'user') tiene alguna entrada para el recurso?
    /// Cualquier `perm` cuenta como membresía.
    async fn is_member(&self, resource_type: &str, resource_id: &str, user_id: &str) -> Result<bool, DbError>;

    /// El `perm` del usuario sobre el recurso ('read'|'write'|'owner'), o `None`
    /// si no es miembro. Para decisiones owner-only en los handlers.
    async fn get_perm(&self, resource_type: &str, resource_id: &str, user_id: &str) -> Result<Option<String>, DbError>;

    /// Otorga (upsert idempotente) un permiso a un usuario sobre un recurso.
    async fn grant(&self, resource_type: &str, resource_id: &str, user_id: &str, perm: &str) -> Result<(), DbError>;

    /// Revoca el permiso de un usuario sobre un recurso (idempotente).
    async fn revoke(&self, resource_type: &str, resource_id: &str, user_id: &str) -> Result<(), DbError>;

    /// Revoca a un usuario de un proyecto de forma ATÓMICA, salvo que sea el
    /// último owner (devuelve `WouldLeaveNoOwner` sin borrar). Una sola sentencia
    /// condicional cierra el TOCTOU de un check list-then-delete.
    async fn try_revoke_project_member(&self, project_id: &str, user_id: &str) -> Result<MemberRevoke, DbError>;

    /// Lista los principales (miembros) de un recurso.
    async fn list_principals(&self, resource_type: &str, resource_id: &str) -> Result<Vec<ResourceAclRow>, DbError>;

    /// Conveniencia: membresía de proyecto (cualquier perm).
    async fn is_project_member(&self, user_id: &str, project_id: &str) -> Result<bool, DbError> {
        self.is_member("project", project_id, user_id).await
    }
}
