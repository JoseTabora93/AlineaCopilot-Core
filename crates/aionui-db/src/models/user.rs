use aionui_common::TimestampMs;
use serde::{Deserialize, Serialize};

/// Row mapping for the `users` table.
///
/// All fields match the SQLite column names and types exactly.
/// Optional fields correspond to nullable columns.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct User {
    pub id: String,
    pub username: String,
    pub email: Option<String>,
    pub password_hash: String,
    pub avatar_path: Option<String>,
    pub jwt_secret: Option<String>,
    pub created_at: TimestampMs,
    pub updated_at: TimestampMs,
    pub last_login: Option<TimestampMs>,
    /// RBAC role. Known values: `"admin"`, `"member"`.
    pub role: String,
    /// Whether the account is active. Inactive accounts are rejected at login.
    /// Stored as SQLite INTEGER (0/1); sqlx maps this to bool automatically.
    pub is_active: bool,
    /// Optional human-readable name shown in the UI (separate from login username).
    pub display_name: Option<String>,
}
