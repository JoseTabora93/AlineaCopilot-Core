-- Migration 017: Proyectos nativos (Alinea Fase 2 #2 — slice 3).
--
-- Un PROYECTO es el contenedor de conversaciones (ya ligadas vía
-- conversations.project_id, migración 016), documentos y — en slices 4-7 — un
-- pipeline de tareas humano/agente.
--
-- Membresía: NO hay tabla nueva. Vive en `resource_acl` (migración 014) con
-- resource_type='project'. El creador obtiene perm='owner'; miembros 'read'/'write'.
-- El gate fail-closed de `ConversationService::update` consulta esta tabla.
--
-- pipeline_templates: playbooks reusables por tipo de proyecto que Hermes
-- instancia y adapta (slice 6). created_at=0 = filas de sistema (seeds).

CREATE TABLE IF NOT EXISTS projects (
    id           TEXT PRIMARY KEY NOT NULL,
    name         TEXT NOT NULL,
    description  TEXT,
    project_type TEXT NOT NULL DEFAULT 'generico',  -- 'preventa_mep' | 'diseno_mep' | 'generico'
    status       TEXT NOT NULL DEFAULT 'active',     -- 'active' | 'archived'
    created_by   TEXT NOT NULL,
    created_at   INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_projects_status  ON projects(status);
CREATE INDEX IF NOT EXISTS idx_projects_creator ON projects(created_by);

CREATE TABLE IF NOT EXISTS pipeline_templates (
    id           TEXT PRIMARY KEY NOT NULL,
    name         TEXT NOT NULL,
    project_type TEXT NOT NULL,
    version      INTEGER NOT NULL DEFAULT 1,
    definition   TEXT NOT NULL,                      -- JSON: árbol de task-specs (slice 6)
    is_active    INTEGER NOT NULL DEFAULT 1,
    created_at   INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_pipeline_templates_type ON pipeline_templates(project_type);

-- Seeds de playbooks base. La PREVENTA termina en BOM (explosión de materiales con
-- desperdicio/rastro) + Alcances de obra (scope) — entregables DISTINTOS.
-- requires_human_review es POR-TAREA (decisión de José). assignee_kind = human|agent.
INSERT OR IGNORE INTO pipeline_templates (id, name, project_type, version, definition, is_active, created_at) VALUES
  ('tpl-preventa-mep', 'Preventa MEP', 'preventa_mep', 1,
   '{"tasks":[{"key":"levantamiento","title":"Levantamiento en sitio","assignee_kind":"human","requires_human_review":false,"depends_on":[]},{"key":"analisis","title":"Análisis técnico de requerimientos","assignee_kind":"human","requires_human_review":false,"depends_on":["levantamiento"]},{"key":"calculo","title":"Cálculo / dimensionamiento HVAC","assignee_kind":"agent","agent":"openclaw","requires_human_review":true,"depends_on":["analisis"]},{"key":"bom","title":"Explosión de materiales (BOM)","assignee_kind":"agent","agent":"openclaw","requires_human_review":true,"produces_artifact":"bom","depends_on":["calculo"]},{"key":"alcances","title":"Alcances de obra","assignee_kind":"agent","agent":"openclaw","requires_human_review":true,"produces_artifact":"alcances_obra","depends_on":["calculo"]},{"key":"propuesta","title":"Propuesta comercial","assignee_kind":"human","requires_human_review":false,"depends_on":["bom","alcances"]}]}',
   1, 0),
  ('tpl-diseno-mep', 'Diseño MEP', 'diseno_mep', 1,
   '{"tasks":[{"key":"brief","title":"Brief y alcance del diseño","assignee_kind":"human","requires_human_review":false,"depends_on":[]},{"key":"calculo","title":"Cálculos de ingeniería","assignee_kind":"agent","agent":"openclaw","requires_human_review":true,"depends_on":["brief"]},{"key":"planos","title":"Planos / DXF","assignee_kind":"agent","agent":"openclaw","requires_human_review":true,"produces_artifact":"doc","depends_on":["calculo"]},{"key":"revision","title":"Revisión y ajustes","assignee_kind":"human","requires_human_review":false,"depends_on":["planos"]}]}',
   1, 0);
