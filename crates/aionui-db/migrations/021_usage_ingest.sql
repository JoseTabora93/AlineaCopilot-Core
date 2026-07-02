-- Migration 021: Ingest externo de usage + profile_id (Alinea Fase ledger — tarea C1).
--
-- Cierra el punto ciego de emisores externos (pipeline de preventa, Hermes):
-- hoy `usage_events` solo se escribe desde el hot-path del Core mismo
-- (`stream_relay.rs`) autenticado por sesión de usuario. `service_tokens`
-- permite que procesos externos (fuera de una sesión de usuario) posteen
-- eventos a `POST /api/usage/ingest` con un token de servicio propio,
-- fail-closed: sin token válido, no se graba nada.
--
-- Nota de numeración: 020 está reservada para `feat/agent-profiles` (tarea
-- A1, en paralelo); se numera 021 a propósito para no chocar. Posible
-- conflicto trivial de orden al mergear ambas ramas — ver nota en el PR.

------------------------------------------------------------------------
-- usage_events.profile_id: atribución opcional a un perfil de agente
-- (registro de perfiles aún no existe en esta rama; es un TEXT suelto,
-- sin FK, para no acoplar esta migración a `agent_profiles`).
------------------------------------------------------------------------
ALTER TABLE usage_events ADD COLUMN profile_id TEXT;

CREATE INDEX IF NOT EXISTS idx_usage_events_profile_time ON usage_events(profile_id, created_at);
CREATE INDEX IF NOT EXISTS idx_usage_events_engine_time ON usage_events(engine, created_at);

------------------------------------------------------------------------
-- service_tokens: credenciales de emisores externos (pipeline preventa,
-- Hermes, etc). El token en claro se muestra UNA sola vez al crearlo; solo
-- se persiste su hash (SHA-256, hex) — igual que un secreto de API.
------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS service_tokens (
    id          TEXT PRIMARY KEY NOT NULL,
    name        TEXT NOT NULL UNIQUE,
    token_hash  TEXT NOT NULL,
    is_active   INTEGER NOT NULL DEFAULT 1,
    created_at  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_service_tokens_hash ON service_tokens(token_hash);

------------------------------------------------------------------------
-- usage_ingest_idempotency: dedupe de `POST /api/usage/ingest` por
-- `idempotency_key` opcional. Reintentos del emisor (fallback JSONL local
-- que reintenta tras una caída del Core) no duplican el evento.
------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS usage_ingest_idempotency (
    idempotency_key TEXT PRIMARY KEY NOT NULL,
    usage_event_id  TEXT NOT NULL,
    created_at      INTEGER NOT NULL
);
