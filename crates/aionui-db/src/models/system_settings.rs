use aionui_common::TimestampMs;
use serde::{Deserialize, Serialize};

/// Row mapping for the `system_settings` table.
///
/// Single-row table (id is always 1). Boolean fields are stored as INTEGER
/// in SQLite (0/1) and mapped to `bool` via sqlx.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct SystemSettings {
    pub id: i64,
    pub language: String,
    pub notification_enabled: bool,
    pub cron_notification_enabled: bool,
    pub command_queue_enabled: bool,
    pub save_upload_to_workspace: bool,
    pub updated_at: TimestampMs,
    /// User registration policy.
    ///
    /// - `"invite_only"` (default) — admin creates all accounts; public register is closed.
    /// - `"domain_allowlist"` — self-registration allowed when email ends with `registration_domain`.
    /// - `"open"` — anyone may register without restrictions.
    pub registration_mode: Option<String>,
    /// Email domain required for `domain_allowlist` mode (e.g. `"ingelmec.ai"`).
    pub registration_domain: Option<String>,
}

impl SystemSettings {
    /// Returns the effective registration mode, defaulting to `"invite_only"` when unset.
    pub fn effective_registration_mode(&self) -> &str {
        self.registration_mode.as_deref().unwrap_or("invite_only")
    }
}
