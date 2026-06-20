//! Repositorio de proyectos (Alinea Fase 2 #2 — slice 3).
//!
//! Single-responsibility: solo la fila `projects` + lectura de
//! `pipeline_templates`. La membresía (otorgar owner al crear, listar miembros)
//! la maneja `IResourceAclRepository` desde el handler — así el repo no acopla
//! dos tablas.

use crate::DbError;
use crate::models::{PipelineTemplateRow, ProjectRow};

/// Parámetros para crear un proyecto.
pub struct NewProject {
    pub name: String,
    pub description: Option<String>,
    pub project_type: String,
    pub created_by: String,
}

/// Actualización parcial de un proyecto. `description: Some(None)` lo limpia.
#[derive(Default)]
pub struct ProjectUpdate {
    pub name: Option<String>,
    pub description: Option<Option<String>>,
    pub status: Option<String>,
}

#[async_trait::async_trait]
pub trait IProjectRepository: Send + Sync {
    /// Inserta un proyecto y devuelve la fila creada. NO crea la membresía
    /// (el handler otorga 'owner' vía `IResourceAclRepository`).
    async fn create(&self, params: NewProject) -> Result<ProjectRow, DbError>;

    async fn get(&self, id: &str) -> Result<Option<ProjectRow>, DbError>;

    /// Proyectos donde `user_id` es miembro (vía resource_acl). Si
    /// `include_archived` es false, solo `status='active'`.
    async fn list_for_user(&self, user_id: &str, include_archived: bool) -> Result<Vec<ProjectRow>, DbError>;

    /// Actualización parcial. Error `NotFound` si el id no existe.
    async fn update(&self, id: &str, update: ProjectUpdate) -> Result<(), DbError>;

    /// Plantillas activas, opcionalmente filtradas por `project_type`.
    async fn list_templates(&self, project_type: Option<&str>) -> Result<Vec<PipelineTemplateRow>, DbError>;

    async fn get_template(&self, id: &str) -> Result<Option<PipelineTemplateRow>, DbError>;
}
