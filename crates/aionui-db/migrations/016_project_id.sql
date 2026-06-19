-- Migration 016: las conversaciones pertenecen a un PROYECTO (Alinea Fase 2 #2).
--
-- `project_id` referencia el proyecto en el motor de PM (Paca) / la entidad de
-- proyecto del workspace. Es la pieza que permite chats y RAG scopeados por
-- proyecto. Nullable: una conversación puede no pertenecer a ningún proyecto.
-- La visibilidad por usuario/rol se hace cumplir en el enforcement del Core
-- (identidad firmada + resource_acl con resource_type='project', migración 014).
ALTER TABLE conversations ADD COLUMN project_id TEXT;
CREATE INDEX IF NOT EXISTS idx_conversations_project ON conversations(project_id);
