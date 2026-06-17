-- Migration 013: RBAC de 3 ejes (Alinea Fase 2 #5 — Identidad + Segregación).
--
-- Eje 1 (rol/etiqueta): roles + user_roles + acl_policy(etiqueta -> roles).
-- Eje 2 (ownership/membership): resource_acl(recurso, principal, perm).
-- Eje 3 (project-scope): se acota por el project_id del token firmado (sin tabla;
--   el filtro vive en el enforcement + en resource_acl con resource_type='project').
-- El acceso a un recurso requiere las TRES condiciones (AND).
-- created_at=0 en los seeds = filas de sistema (la app usa TimestampMs reales).

------------------------------------------------------------------------
-- Eje 1: roles + asignación a usuarios
------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS roles (
    id         TEXT PRIMARY KEY NOT NULL,
    name       TEXT NOT NULL UNIQUE,
    label      TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

-- Los 6 roles de Alinea (alineados con las categorías de OpenClaw).
INSERT OR IGNORE INTO roles (id, name, label, created_at) VALUES
  ('admin',      'admin',      'Administrador', 0),
  ('gerencia',   'gerencia',   'Gerencia',      0),
  ('tecnica',    'tecnica',    'Técnica',       0),
  ('comercial',  'comercial',  'Comercial',     0),
  ('financiera', 'financiera', 'Financiera',    0),
  ('ingenieria', 'ingenieria', 'Ingeniería',    0);

CREATE TABLE IF NOT EXISTS user_roles (
    user_id    TEXT NOT NULL,
    role_id    TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (user_id, role_id),
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,
    FOREIGN KEY (role_id) REFERENCES roles(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_user_roles_user ON user_roles(user_id);

------------------------------------------------------------------------
-- Eje 1 (clasificación): etiqueta de doc/skill -> roles que pueden acceder
------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS acl_policy (
    id         TEXT PRIMARY KEY NOT NULL,
    etiqueta   TEXT NOT NULL,   -- 'publico' | 'interno' | 'confidencial-<area>'
    role_id    TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    FOREIGN KEY (role_id) REFERENCES roles(id) ON DELETE CASCADE,
    UNIQUE (etiqueta, role_id)
);
CREATE INDEX IF NOT EXISTS idx_acl_policy_etiqueta ON acl_policy(etiqueta);

-- Política base: publico/interno -> todos; confidencial-<area> -> admin + el área dueña.
INSERT OR IGNORE INTO acl_policy (id, etiqueta, role_id, created_at) VALUES
  ('pub-admin','publico','admin',0),('pub-ger','publico','gerencia',0),('pub-tec','publico','tecnica',0),
  ('pub-com','publico','comercial',0),('pub-fin','publico','financiera',0),('pub-ing','publico','ingenieria',0),
  ('int-admin','interno','admin',0),('int-ger','interno','gerencia',0),('int-tec','interno','tecnica',0),
  ('int-com','interno','comercial',0),('int-fin','interno','financiera',0),('int-ing','interno','ingenieria',0),
  ('cger-admin','confidencial-gerencia','admin',0),('cger-ger','confidencial-gerencia','gerencia',0),
  ('cfin-admin','confidencial-financiera','admin',0),('cfin-fin','confidencial-financiera','financiera',0),
  ('ctec-admin','confidencial-tecnica','admin',0),('ctec-tec','confidencial-tecnica','tecnica',0),
  ('ccom-admin','confidencial-comercial','admin',0),('ccom-com','confidencial-comercial','comercial',0),
  ('cing-admin','confidencial-ingenieria','admin',0),('cing-ing','confidencial-ingenieria','ingenieria',0);

------------------------------------------------------------------------
-- Eje 2: ownership/membership por recurso (proyecto/conversación/archivo/buzón/doc)
------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS resource_acl (
    id             TEXT PRIMARY KEY NOT NULL,
    resource_type  TEXT NOT NULL,   -- 'project' | 'conversation' | 'file' | 'mailbox' | 'doc'
    resource_id    TEXT NOT NULL,
    principal_type TEXT NOT NULL,   -- 'user' | 'role' | 'group'
    principal_id   TEXT NOT NULL,
    perm           TEXT NOT NULL DEFAULT 'read',  -- 'read' | 'write' | 'owner'
    created_at     INTEGER NOT NULL,
    UNIQUE (resource_type, resource_id, principal_type, principal_id)
);
CREATE INDEX IF NOT EXISTS idx_resource_acl_res  ON resource_acl(resource_type, resource_id);
CREATE INDEX IF NOT EXISTS idx_resource_acl_prin ON resource_acl(principal_type, principal_id);
