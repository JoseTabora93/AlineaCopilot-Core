-- Migration 018: Tareas y subtareas (Alinea Fase 2 #2 — slice 4).
--
-- `tasks` es JERÁRQUICA: `parent_task_id` no-NULL = subtarea (los handoffs
-- humano↔agente suelen ocurrir a nivel de subtarea). `assignee_kind` = human|agent.
-- `requires_human_review` es POR-TAREA (decisión de José). `task_dependencies`
-- modela el orden/handoffs del pipeline (con ramas). El motor de handoffs
-- (cascada al cumplirse dependencias) llega en el slice 5.

CREATE TABLE IF NOT EXISTS tasks (
    id                    TEXT PRIMARY KEY NOT NULL,
    project_id            TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    parent_task_id        TEXT REFERENCES tasks(id) ON DELETE CASCADE,
    title                 TEXT NOT NULL,
    instructions          TEXT,
    assignee_kind         TEXT NOT NULL DEFAULT 'human',  -- 'human' | 'agent'
    assignee_id           TEXT,
    status                TEXT NOT NULL DEFAULT 'todo',    -- todo|in_progress|in_review|done|rejected|blocked
    requires_human_review INTEGER NOT NULL DEFAULT 1,
    produces_artifact     TEXT,                            -- 'bom' | 'alcances_obra' | 'doc' | ...
    order_index           INTEGER NOT NULL DEFAULT 0,
    conversation_id       TEXT,                            -- liga la ejecución de agente (slice 7)
    created_by            TEXT NOT NULL,                   -- user_id | 'hermes'
    created_at            INTEGER NOT NULL,
    updated_at            INTEGER NOT NULL,
    started_at            INTEGER,
    completed_at          INTEGER
);
CREATE INDEX IF NOT EXISTS idx_tasks_project ON tasks(project_id);
CREATE INDEX IF NOT EXISTS idx_tasks_parent  ON tasks(parent_task_id);
CREATE INDEX IF NOT EXISTS idx_tasks_status  ON tasks(project_id, status);

CREATE TABLE IF NOT EXISTS task_dependencies (
    task_id            TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    depends_on_task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    PRIMARY KEY (task_id, depends_on_task_id)
);
CREATE INDEX IF NOT EXISTS idx_task_deps_dep ON task_dependencies(depends_on_task_id);
