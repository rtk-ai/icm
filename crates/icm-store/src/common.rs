//! Shared public types used by every storage backend.
//!
//! These live in one always-compiled module so that the backends
//! (`store`/SQLite, `postgres`, `opensearch`) can be compiled together in
//! a single binary without colliding type definitions. The runtime
//! [`crate::Store`] enum dispatches across whichever backends are enabled.

use chrono::{DateTime, Utc};

/// One row of the `hook_events` telemetry table.
#[derive(Debug, Clone)]
pub struct HookEvent {
    pub id: i64,
    pub ts: DateTime<Utc>,
    pub event: String,
    pub project: Option<String>,
    pub session_id: Option<String>,
    pub tool_name: Option<String>,
    pub duration_ms: Option<i64>,
    pub exit_code: i32,
    pub payload_size: Option<i64>,
    pub note: Option<String>,
}

/// Insert payload for a single `hook_events` row. `id` and `ts` are filled
/// in by the store.
#[derive(Debug, Clone, Default)]
pub struct HookEventInsert {
    pub event: String,
    pub project: Option<String>,
    pub session_id: Option<String>,
    pub tool_name: Option<String>,
    pub duration_ms: Option<i64>,
    pub exit_code: i32,
    pub payload_size: Option<i64>,
    pub note: Option<String>,
}

/// One row of the `code_areas` table. A code area is a file the agent
/// touched during a session; the same `(project, file_path)` increments
/// `touch_count` on each re-touch rather than producing a duplicate row.
#[derive(Debug, Clone)]
pub struct CodeArea {
    pub id: i64,
    pub project: String,
    pub file_path: String,
    pub description: Option<String>,
    pub session_id: Option<String>,
    pub tool_name: Option<String>,
    pub touch_count: i64,
    pub first_touched_at: DateTime<Utc>,
    pub last_touched_at: DateTime<Utc>,
}

/// Aggregate counts/percentiles for a slice of hook history. Returned by
/// the `hook_stats` call.
#[derive(Debug, Clone, Default)]
pub struct HookStatsRow {
    pub event: String,
    pub count: i64,
    pub error_count: i64,
    pub avg_duration_ms: f64,
    pub p50_duration_ms: i64,
    pub p99_duration_ms: i64,
}

/// One row from the async extraction queue:
/// `(id, project, tool_name, raw_output, captured_at)` where
/// `captured_at` is RFC3339.
pub type PendingRow = (String, String, String, String, String);
