use aionui_common::TimestampMs;
use serde::{Deserialize, Serialize};

/// Row mapping for the `roles` table (RBAC eje 1 — Alinea Fase 2 #5).
///
/// Catálogo de roles del negocio (admin, gerencia, técnica, comercial,
/// financiera, ingeniería). Los seeds usan `created_at = 0` (filas de sistema).
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Role {
    /// Identificador estable usado en `user_roles.role_id` y `acl_policy.role_id`.
    pub id: String,
    /// Nombre interno (igual al id para los 6 roles base).
    pub name: String,
    /// Etiqueta legible para la UI ("Administrador", "Gerencia", …).
    pub label: String,
    pub created_at: TimestampMs,
}
