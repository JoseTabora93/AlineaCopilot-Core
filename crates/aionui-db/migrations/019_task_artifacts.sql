-- Migration 019: Artefactos de tarea + log de handoffs (Alinea Fase 2 #2 — slice 5).
--
-- task_artifacts: el ENTREGABLE que produce una tarea (BOM, alcances de obra,
-- doc, archivo…). En slice 7 OpenClaw los registra vía MCP; en slice 5 los
-- registra el humano. task_handoff_log: auditoría de cada transición/cascada
-- del modelo supervisor/ejecutor (quién disparó qué).

CREATE TABLE IF NOT EXISTS task_artifacts (
    id          TEXT PRIMARY KEY NOT NULL,
    task_id     TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    kind        TEXT NOT NULL,   -- 'bom' | 'alcances_obra' | 'doc' | 'file' | 'link' | 'conversation_ref'
    uri         TEXT NOT NULL,   -- ruta en el sandbox del usuario o URL
    title       TEXT NOT NULL,
    produced_by TEXT NOT NULL,   -- user_id | 'openclaw'
    created_at  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_task_artifacts_task ON task_artifacts(task_id);

CREATE TABLE IF NOT EXISTS task_handoff_log (
    id          TEXT PRIMARY KEY NOT NULL,
    task_id     TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    from_status TEXT,
    to_status   TEXT NOT NULL,
    actor        TEXT NOT NULL,  -- user_id | 'openclaw' | 'hermes' | 'system'
    trigger_kind TEXT NOT NULL,  -- 'manual' | 'dependency_satisfied' | 'agent_completed' | 'approval' | 'rejection'
    note         TEXT,
    created_at  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_task_handoff_task ON task_handoff_log(task_id);
