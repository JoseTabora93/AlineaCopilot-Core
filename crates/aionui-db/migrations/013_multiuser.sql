-- Migration 013: multi-user roles, activation flag, display name, registration policy
--
-- All ALTER TABLE statements are additive (SQLite only supports ADD COLUMN).
-- Existing rows receive DEFAULT values automatically.

-- users: add role, activation flag, and optional display name
ALTER TABLE users ADD COLUMN role         TEXT    NOT NULL DEFAULT 'member';
ALTER TABLE users ADD COLUMN is_active    INTEGER NOT NULL DEFAULT 1;
ALTER TABLE users ADD COLUMN display_name TEXT;

-- Promote the seeded system_default_user to admin so the first WebUI login
-- already has full management access.
-- On fresh databases this UPDATE touches 0 rows (user is inserted by
-- ensure_system_user AFTER migrations run, with role='admin' explicitly).
UPDATE users SET role = 'admin' WHERE id = 'system_default_user';

CREATE INDEX IF NOT EXISTS idx_users_role ON users(role);

-- system_settings: configurable registration policy
-- Modes: 'invite_only' (default, admin-creates only) | 'domain_allowlist'
-- (email domain must match registration_domain) | 'open' (anyone can register)
ALTER TABLE system_settings ADD COLUMN registration_mode   TEXT DEFAULT 'invite_only';
ALTER TABLE system_settings ADD COLUMN registration_domain TEXT;
