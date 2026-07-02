//! Repositorio de `agent_profiles` (Motor MULTI-PERFIL — tarea A1).
//!
//! Single-responsibility: solo la fila `agent_profiles`. La visibilidad/gate
//! por usuario vive en `IResourceAclRepository` (resource_type='agent_profile'),
//! consultado desde el handler `GET /api/profiles` — mismo patrón que
//! Proyectos (`crates/aionui-db/src/repository/project.rs`).
//!
//! La validación del JSON `definition` contra el esquema PERFIL v1
//! (`crate::profile_schema::validate_profile_definition`) es responsabilidad
//! del LLAMADOR (handler de rutas), no de este repo — así el repo permanece
//! agnóstico del contenido del JSON y el error de validación puede mapearse a
//! un 400 claro sin pasar por `DbError`.

use crate::DbError;
use crate::models::AgentProfileRow;

/// Parámetros para crear un perfil.
pub struct NewAgentProfile {
    pub name: String,
    pub label: String,
    /// JSON crudo — YA validado por el llamador.
    pub definition: String,
}

/// Actualización parcial de un perfil.
#[derive(Default)]
pub struct AgentProfileUpdate {
    pub label: Option<String>,
    /// JSON crudo — YA validado por el llamador.
    pub definition: Option<String>,
    pub is_active: Option<bool>,
}

#[async_trait::async_trait]
pub trait IAgentProfileRepository: Send + Sync {
    /// Inserta un perfil. `DbError::Conflict` si `name` ya existe (UNIQUE).
    async fn create(&self, params: NewAgentProfile) -> Result<AgentProfileRow, DbError>;

    async fn get(&self, id: &str) -> Result<Option<AgentProfileRow>, DbError>;

    async fn get_by_name(&self, name: &str) -> Result<Option<AgentProfileRow>, DbError>;

    /// Todos los perfiles (panel admin). `include_inactive=false` filtra a
    /// `is_active=1`.
    async fn list_all(&self, include_inactive: bool) -> Result<Vec<AgentProfileRow>, DbError>;

    /// Perfiles cuyo `id` está en `ids`, preservando solo los activos salvo
    /// `include_inactive`. Usado por `GET /api/profiles` tras resolver el
    /// gate de membresía (evita N+1 `get()` por id).
    async fn list_by_ids(&self, ids: &[String], include_inactive: bool) -> Result<Vec<AgentProfileRow>, DbError>;

    /// Actualización parcial. Error `NotFound` si el id no existe;
    /// `Conflict` si el `label`/`definition` no cambian el name pero la fila
    /// desaparece entre lectura y escritura no aplica aquí (no se permite
    /// cambiar `name` — es la clave estable para los compiladores).
    async fn update(&self, id: &str, update: AgentProfileUpdate) -> Result<(), DbError>;

    /// Borrado físico. Error `NotFound` si el id no existe.
    async fn delete(&self, id: &str) -> Result<(), DbError>;
}
