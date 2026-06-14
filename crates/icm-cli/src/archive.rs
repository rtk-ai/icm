//! Hook auto-archive (issue #272).
//!
//! Tees user prompts and tool outputs into the verbatim
//! `sessions`/`messages` tables so an FTS5-backed `icm sessions search`
//! works at zero recall cost. Opt-in via `[archive].enabled = true`.
//!
//! Distinct from extraction (which is curated/lossy): archive keeps
//! every event under a session keyed by the host agent's session id
//! so re-fires within the same Claude Code / Codex / Gemini turn land
//! under the same row.

use icm_core::transcript::Role;
use icm_core::TranscriptStore;
use icm_store::SqliteStore;
use serde_json::Value;

use crate::config::ArchiveConfig;

/// Truncate `s` at a UTF-8 char boundary so the tail of the cut never
/// lands inside a multibyte sequence (issue #110 keeps biting us).
/// `max_bytes == 0` disables truncation.
fn cap_bytes(s: &str, max_bytes: usize) -> &str {
    if max_bytes == 0 || s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Try to pull a session id from the hook stdin JSON. Falls back to
/// the basename of `transcript_path` (which Claude Code includes) when
/// `session_id` is absent. Returns `None` only when neither is present
/// — caller skips archiving rather than create a junk-keyed row.
pub fn session_id_from_stdin(json: &Value) -> Option<String> {
    if let Some(s) = json.get("session_id").and_then(|v| v.as_str()) {
        if !s.is_empty() {
            return Some(s.to_string());
        }
    }
    json.get("transcript_path")
        .and_then(|v| v.as_str())
        .and_then(|p| {
            std::path::Path::new(p)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
        })
        .filter(|s| !s.is_empty())
}

/// Heuristic agent label from the hook env / payload. Best-effort —
/// `unknown` is fine; we just want the column populated for the stats
/// breakdown.
pub fn agent_label_from_env() -> String {
    if std::env::var("CURSOR_VERSION").is_ok() || std::env::var("CURSOR_PROJECT_DIR").is_ok() {
        return "cursor".into();
    }
    if std::env::var("CLAUDE_CONFIG_DIR").is_ok() || std::env::var("CLAUDECODE").is_ok() {
        return "claude-code".into();
    }
    if std::env::var("CODEX_HOME").is_ok() {
        return "codex".into();
    }
    if std::env::var("GEMINI_CONFIG_DIR").is_ok() {
        return "gemini".into();
    }
    "unknown".into()
}

/// Archive a single event. No-op when `[archive].enabled = false`.
///
/// Errors are swallowed and logged on stderr so a failure to archive
/// never breaks the hook chain — the curated `Memory` path is the
/// agent's source of truth; the archive is fallback fidelity.
pub fn record_event(
    store: &SqliteStore,
    cfg: &ArchiveConfig,
    json: &Value,
    role: Role,
    content: &str,
    tool_name: Option<&str>,
) {
    if !cfg.enabled || content.is_empty() {
        return;
    }
    let Some(session_id) = session_id_from_stdin(json) else {
        return;
    };

    let project = json
        .get("cwd")
        .and_then(|v| v.as_str())
        .map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| std::path::PathBuf::from(p)))
        .as_ref()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_string());

    let agent = agent_label_from_env();
    if let Err(e) = store.ensure_session(&session_id, &agent, project.as_deref(), None) {
        eprintln!("[icm archive] ensure_session failed: {e}");
        return;
    }

    let capped = cap_bytes(content, cfg.effective_max_bytes());
    if let Err(e) = store.record_message(&session_id, role, capped, tool_name, None, None) {
        eprintln!("[icm archive] record_message failed: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cap_bytes_disabled_when_zero() {
        assert_eq!(cap_bytes("hello", 0), "hello");
    }

    #[test]
    fn cap_bytes_passthrough_when_under_cap() {
        assert_eq!(cap_bytes("hello", 100), "hello");
    }

    #[test]
    fn cap_bytes_lands_on_char_boundary_for_multibyte() {
        // 3 bytes per arrow, cap at 7 → must back off to 6.
        let s = "\u{2192}\u{2192}\u{2192}";
        let out = cap_bytes(s, 7);
        assert!(out.len() <= 7);
        assert!(s.is_char_boundary(out.len()));
        assert!(out.chars().all(|c| c == '\u{2192}'));
    }

    #[test]
    fn session_id_pulled_from_stdin() {
        let v: Value = serde_json::from_str(r#"{"session_id":"abc-123","cwd":"/tmp"}"#).unwrap();
        assert_eq!(session_id_from_stdin(&v).as_deref(), Some("abc-123"));
    }

    #[test]
    fn session_id_falls_back_to_transcript_stem() {
        let v: Value =
            serde_json::from_str(r#"{"transcript_path":"/var/log/agent/sess-xyz.jsonl"}"#).unwrap();
        assert_eq!(session_id_from_stdin(&v).as_deref(), Some("sess-xyz"));
    }

    #[test]
    fn session_id_none_when_absent() {
        let v: Value = serde_json::from_str("{}").unwrap();
        assert_eq!(session_id_from_stdin(&v), None);
    }

    #[test]
    fn record_event_is_noop_when_disabled() {
        let store = SqliteStore::in_memory().unwrap();
        let cfg = ArchiveConfig::default(); // enabled = false
        let v: Value = serde_json::from_str(r#"{"session_id":"s1","cwd":"/tmp"}"#).unwrap();
        record_event(&store, &cfg, &v, Role::User, "hello", None);
        // No sessions table row, no messages.
        let sessions = store.list_sessions(None, 10).unwrap();
        assert!(sessions.is_empty(), "archive should be no-op when disabled");
    }

    #[test]
    fn record_event_skips_when_no_session_id() {
        let store = SqliteStore::in_memory().unwrap();
        let cfg = ArchiveConfig {
            enabled: true,
            max_bytes_per_event: 0,
        };
        let v: Value = serde_json::from_str(r#"{"cwd":"/tmp"}"#).unwrap();
        record_event(&store, &cfg, &v, Role::User, "hello", None);
        let sessions = store.list_sessions(None, 10).unwrap();
        assert!(
            sessions.is_empty(),
            "no session_id and no transcript_path → skip rather than create junk row",
        );
    }

    #[test]
    fn record_event_persists_under_external_id() {
        let store = SqliteStore::in_memory().unwrap();
        let cfg = ArchiveConfig {
            enabled: true,
            max_bytes_per_event: 0,
        };
        let v: Value = serde_json::from_str(r#"{"session_id":"s1","cwd":"/tmp"}"#).unwrap();
        record_event(&store, &cfg, &v, Role::User, "first turn", None);
        record_event(&store, &cfg, &v, Role::Tool, "tool output", Some("bash"));
        let sessions = store.list_sessions(None, 10).unwrap();
        assert_eq!(
            sessions.len(),
            1,
            "should reuse the single session: {sessions:?}"
        );
        assert_eq!(sessions[0].id, "s1");
        let msgs = store.list_session_messages("s1", 10, 0).unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].content, "first turn");
        assert_eq!(msgs[1].content, "tool output");
        assert_eq!(msgs[1].tool_name.as_deref(), Some("bash"));
    }

    #[test]
    fn record_event_truncates_at_byte_cap() {
        let store = SqliteStore::in_memory().unwrap();
        let cfg = ArchiveConfig {
            enabled: true,
            max_bytes_per_event: 16,
        };
        let v: Value = serde_json::from_str(r#"{"session_id":"s1","cwd":"/tmp"}"#).unwrap();
        record_event(
            &store,
            &cfg,
            &v,
            Role::Tool,
            &"abcdefghij".repeat(100),
            Some("noisy"),
        );
        let msgs = store.list_session_messages("s1", 10, 0).unwrap();
        assert_eq!(msgs.len(), 1);
        assert!(
            msgs[0].content.len() <= 16,
            "expected truncation under 16 bytes, got {}",
            msgs[0].content.len()
        );
    }
}
