-- Migration 015: Ledger de consumos $ (Alinea Fase 2 #3 — blueprint v2 §12).
--
-- Mide TODA llamada LLM por usuario (los 3 motores: Copilot/OpenClaw/Hermes)
-- y soporta límites de gasto por usuario (soft avisa / hard bloquea).
-- created_at en TimestampMs (ms epoch), igual que el resto del esquema.

------------------------------------------------------------------------
-- usage_events: un evento por llamada LLM (o bucket de sistema)
------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS usage_events (
    id              TEXT PRIMARY KEY NOT NULL,
    -- Dueño del costo. Para buckets de fondo se atribuye al owner real
    -- (cron→owner, mail-triage→user) o a un pseudo-usuario 'system'.
    user_id         TEXT NOT NULL,
    conversation_id TEXT,
    project_id      TEXT,
    -- Motor que originó el costo: 'copilot' | 'openclaw' | 'hermes'
    -- | 'system:kb-index' | 'system:cron' | 'system:mail-triage' | ...
    engine          TEXT NOT NULL,
    -- Proveedor del modelo: 'anthropic' | 'zai' | 'minimax' | 'openrouter' | ...
    provider        TEXT,
    model           TEXT,
    tokens_in       INTEGER NOT NULL DEFAULT 0,
    tokens_out      INTEGER NOT NULL DEFAULT 0,
    cache_read      INTEGER NOT NULL DEFAULT 0,
    cache_write     INTEGER NOT NULL DEFAULT 0,
    -- Costo estimado en USD (precio del modelo aplicado en el Core).
    cost_usd        REAL NOT NULL DEFAULT 0,
    created_at      INTEGER NOT NULL,
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);
-- Agregación "mi consumo" / panel admin: por usuario + ventana temporal.
CREATE INDEX IF NOT EXISTS idx_usage_events_user_time ON usage_events(user_id, created_at);

------------------------------------------------------------------------
-- user_usage_limit: límite de gasto por usuario (pre-flight)
------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS user_usage_limit (
    user_id    TEXT PRIMARY KEY NOT NULL,
    -- Umbral en USD del período. NULL = sin límite de ese tipo.
    soft_usd   REAL,   -- avisa (no bloquea)
    hard_usd   REAL,   -- bloquea nuevas llamadas
    -- Ventana de acumulación. Por ahora solo 'monthly' (reset por mes natural).
    period     TEXT NOT NULL DEFAULT 'monthly',
    updated_at INTEGER NOT NULL,
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);
