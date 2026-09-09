//! SQLite backend — split out of the former monolithic `store.rs`.
//!
//! `SqliteStore` and the shared row/parse helpers live in `super`
//! (`store/mod.rs`); each submodule here holds one trait impl (or a
//! coherent group of inherent methods) on that type.

use super::*;
// Memory helpers

pub(crate) fn source_type(source: &MemorySource) -> &'static str {
    match source {
        MemorySource::ClaudeCode { .. } => "claude_code",
        MemorySource::Conversation { .. } => "conversation",
        MemorySource::Manual => "manual",
    }
}

pub(crate) fn source_data(source: &MemorySource) -> Option<String> {
    match source {
        MemorySource::Manual => None,
        other => serde_json::to_string(other).ok(),
    }
}

pub(crate) fn parse_source(source_type_str: &str, source_data_str: Option<String>) -> MemorySource {
    match source_type_str {
        "manual" => MemorySource::Manual,
        _ => source_data_str
            .and_then(|d| serde_json::from_str(&d).ok())
            .unwrap_or(MemorySource::Manual),
    }
}

pub(crate) fn embedding_to_blob(embedding: &[f32]) -> Vec<u8> {
    embedding.as_bytes().to_vec()
}

pub(crate) fn blob_to_embedding(blob: &[u8]) -> Vec<f32> {
    if !blob.len().is_multiple_of(4) {
        tracing::warn!(
            blob_size = blob.len(),
            "embedding blob size not divisible by 4, truncating"
        );
    }
    blob.as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect()
}

pub(crate) fn row_to_memory(row: &rusqlite::Row) -> rusqlite::Result<Memory> {
    // Column order: id(0), created_at(1), updated_at(2), last_accessed(3),
    //   access_count(4), weight(5), topic(6), summary(7), raw_excerpt(8),
    //   keywords(9), importance(10), source_type(11), source_data(12),
    //   related_ids(13), embedding(14)
    let keywords_json: String = row.get::<_, Option<String>>(9)?.unwrap_or_default();
    let keywords: Vec<String> = serde_json::from_str(&keywords_json).unwrap_or_default();

    let importance_str: String = row.get(10)?;
    let importance = importance_str.parse().unwrap_or(Importance::Medium);

    let source_type_str: String = row.get(11)?;
    let source_data_str: Option<String> = row.get(12)?;
    let source = parse_source(&source_type_str, source_data_str);

    let related_json: String = row.get::<_, Option<String>>(13)?.unwrap_or_default();
    let related_ids: Vec<String> = serde_json::from_str(&related_json).unwrap_or_default();

    let embedding: Option<Vec<f32>> = row
        .get::<_, Option<Vec<u8>>>(14)?
        .map(|b| blob_to_embedding(&b));

    let created_at_str: String = row.get(1)?;
    let updated_at_str: String = row.get::<_, Option<String>>(2)?.unwrap_or_default();
    let last_accessed_str: String = row.get(3)?;

    let created_at = parse_dt(&created_at_str);

    Ok(Memory {
        id: row.get(0)?,
        created_at,
        updated_at: if updated_at_str.is_empty() {
            created_at
        } else {
            parse_dt(&updated_at_str)
        },
        last_accessed: parse_dt(&last_accessed_str),
        access_count: row.get::<_, u32>(4)?,
        weight: row.get(5)?,
        topic: row.get(6)?,
        summary: row.get(7)?,
        raw_excerpt: row.get(8)?,
        keywords,
        importance,
        source,
        related_ids,
        embedding,
        scope: icm_core::Scope::User, // default for existing local memories
    })
}

pub(crate) const SELECT_COLS: &str = "id, created_at, updated_at, last_accessed, access_count, weight, \
                           topic, summary, raw_excerpt, keywords, \
                           importance, source_type, source_data, related_ids, embedding";

/// Sanitize a query string for FTS5 MATCH.
///
/// FTS5 treats characters like `-`, `*`, `"`, `:`, `^`, `+`, `~` as operators.
/// A query like `"sqlite-vec"` makes FTS5 interpret `-` as NOT and `vec` as a
/// column name, causing "no such column: vec".
///
/// Escape `%`, `_`, and the escape character itself so a keyword can be
/// safely wrapped in a `%...%` LIKE pattern. Pair with `ESCAPE '\'` in the
/// SQL — without it, a keyword containing `%` matches every row and `_`
/// matches any single character (audit finding).
pub(crate) fn escape_like_wildcards(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// Cap on auxiliary metadata (transcript sessions/messages) - best-effort
/// truncation, not rejection, matching MAX_MESSAGE_BYTES's rationale.
pub(crate) const MAX_METADATA_BYTES: usize = 8 * 1024;

/// Truncate `s` to at most `max` bytes without splitting a UTF-8 char.
pub(crate) fn truncate_at_char_boundary(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut cut = max;
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    &s[..cut]
}

/// Strip FTS5 operator chars and return each surviving token wrapped in
/// double quotes (so token content is never parsed as FTS5 syntax).
/// Shared by [`sanitize_fts_query`] (AND join) and
/// [`sanitize_fts_query_any`] (OR join).
fn quoted_fts_tokens(query: &str) -> Vec<String> {
    // Limit input length to prevent abuse (UTF-8 safe truncation)
    let query = if query.len() > 10_000 {
        let mut end = 10_000;
        while end > 0 && !query.is_char_boundary(end) {
            end -= 1;
        }
        &query[..end]
    } else {
        query
    };

    // Replace FTS5 operator chars with spaces, then quote each resulting token.
    // FTS5 tokenizer (unicode61) splits on `-` too, so we must keep tokens separate.
    let cleaned: String = query
        .chars()
        .map(|c| {
            if matches!(
                c,
                '-' | '*' | '"' | '(' | ')' | '{' | '}' | ':' | '^' | '+' | '~' | '\\'
            ) {
                ' '
            } else {
                c
            }
        })
        .collect();

    cleaned
        .split_whitespace()
        .filter(|w| !w.is_empty())
        .take(100) // Limit token count to prevent excessive query complexity
        .map(|w| {
            // Strip any remaining quotes from tokens before wrapping in quotes
            let stripped = w.replace('"', "");
            format!("\"{stripped}\"")
        })
        .collect()
}

/// This function strips special chars and wraps each token in double quotes,
/// AND-joined (FTS5's default when tokens are merely adjacent): every token
/// must match. Intended for literal/precise lookups (plain FTS fallback,
/// feedback and memoir search).
pub(crate) fn sanitize_fts_query(query: &str) -> String {
    quoted_fts_tokens(query).join(" ")
}

/// Same tokenization as [`sanitize_fts_query`], OR-joined: any token
/// matching is enough.
///
/// For a natural-language question ("What does Melanie do with her family
/// on hikes?" -> 8+ tokens), AND-joining effectively kills FTS5 recall —
/// no single short dialogue turn contains every query word, so the FTS
/// side of a hybrid vector+FTS blend contributes ~0 regardless of its
/// weight, silently reducing "hybrid search" to vector-only. OR-joining
/// lets a turn that shares just the distinctive words ("hikes", "family")
/// score via BM25 too, which is the actual point of blending FTS with
/// vector similarity. Used by `search_hybrid`'s FTS component only — the
/// precise-lookup call sites keep AND semantics.
pub(crate) fn sanitize_fts_query_any(query: &str) -> String {
    quoted_fts_tokens(query).join(" OR ")
}

/// Whether `e` is FTS5 rejecting a malformed MATCH query (e.g. "hello AND",
/// unbalanced parens) rather than a genuine database error. Used by
/// `search_transcripts` to degrade to "no results" instead of surfacing a
/// raw sqlite error, without pre-sanitizing the query text away from valid
/// FTS5 syntax (which callers rely on — see
/// `test_transcript_search_fts5_boolean_and_phrase`).
pub(crate) fn is_fts5_syntax_error(e: &rusqlite::Error) -> bool {
    matches!(
        e,
        rusqlite::Error::SqliteFailure(_, Some(msg)) if msg.contains("fts5: syntax error")
    )
}

// MemoryStore impl

/// Maximum byte length of a stored summary. Audit finding: a transcript
/// containing a 1 MB unbroken text block landed as a single memory whose
/// summary was the full 1 MB blob. Caps the cost of a single bad write
/// (memory bloat, embedding compute, FTS5 index growth) to a generous
/// but bounded 64 KB.
pub(crate) const MAX_SUMMARY_BYTES: usize = 64 * 1024;

/// Maximum byte length of a stored topic. Topics surface in `icm
/// topics` listings and as the routing key for project filters; a
/// thousand-byte topic is always a bug, never legitimate user input.
pub(crate) const MAX_TOPIC_BYTES: usize = 256;

/// Validate and normalize a `Memory` before insertion. Trims topic
/// whitespace and rejects inputs that we know corrupt or break the
/// store:
///
/// - Empty or whitespace-only `topic` / `summary` — these would surface
///   as blank rows in `icm topics` / `icm list` and pollute the FTS5
///   index without conveying information.
/// - NUL byte (`\0`) in `topic` or `summary` — libsql binds text via a
///   NUL-terminated C string, so anything past the first `\0` is
///   silently dropped. Rather than silently truncate, refuse the
///   write so the caller knows.
/// - Newline / CR / tab in `topic` — these break the `icm topics`
///   tabular layout and could enable display-spoofing of topic names
///   (e.g. a topic that visually overlaps another in TUI/log output).
///   Allowed in `summary` since it's free-form prose.
/// - `topic` longer than `MAX_TOPIC_BYTES` or `summary` longer than
///   `MAX_SUMMARY_BYTES` — see the constant docs for rationale.
pub(crate) fn validate_and_normalize(mut memory: Memory) -> IcmResult<Memory> {
    memory.topic = memory.topic.trim().to_string();
    validate_fields(&memory.topic, &memory.summary)?;
    Ok(memory)
}

/// The borrowed core of [`validate_and_normalize`], shared with `update()`
/// (audit finding: the update path previously bypassed every size/content
/// check, so oversized or NUL-carrying payloads could enter the store by
/// storing small then updating big).
pub(crate) fn validate_fields(topic: &str, summary: &str) -> IcmResult<()> {
    if topic.is_empty() {
        return Err(IcmError::InvalidInput("topic cannot be empty".into()));
    }
    if summary.trim().is_empty() {
        return Err(IcmError::InvalidInput("summary cannot be empty".into()));
    }
    if topic.contains('\0') {
        return Err(IcmError::InvalidInput(
            "topic must not contain NUL bytes".into(),
        ));
    }
    if summary.contains('\0') {
        return Err(IcmError::InvalidInput(
            "summary must not contain NUL bytes".into(),
        ));
    }
    if topic.contains(['\n', '\r', '\t']) {
        return Err(IcmError::InvalidInput(
            "topic must not contain newline / CR / tab characters".into(),
        ));
    }
    if topic.len() > MAX_TOPIC_BYTES {
        return Err(IcmError::InvalidInput(format!(
            "topic exceeds {} bytes",
            MAX_TOPIC_BYTES
        )));
    }
    if summary.len() > MAX_SUMMARY_BYTES {
        return Err(IcmError::InvalidInput(format!(
            "summary exceeds {} bytes",
            MAX_SUMMARY_BYTES
        )));
    }
    Ok(())
}

/// Local total order on `Importance` (Critical > High > Medium > Low).
/// `Importance` does not implement `Ord` because the project did not
/// want to imply a globally meaningful ordering across all uses
/// (e.g. presentation, filtering). For the dedup-merge path we *do*
/// want to take the maximum so re-storing with a higher priority
/// upgrades the existing row.
pub(crate) fn importance_rank(i: Importance) -> u8 {
    match i {
        Importance::Critical => 4,
        Importance::High => 3,
        Importance::Medium => 2,
        Importance::Low => 1,
    }
}

/// Return the higher-priority importance. Used by the dedup path so
/// `store(...)` semantics are "re-store with critical upgrades, never
/// downgrades".
pub(crate) fn max_importance(a: Importance, b: Importance) -> Importance {
    if importance_rank(a) >= importance_rank(b) {
        a
    } else {
        b
    }
}

/// SHA-256 over the normalized `(topic, summary)` pair, hex-encoded.
/// Normalization: trim + lowercase + collapse whitespace runs to single
/// spaces. Topic and summary are joined by `\0` to prevent boundary
/// ambiguity (e.g. `"a"|"bc"` vs `"ab"|"c"` would otherwise hash the
/// same). Used by the dedup `INSERT OR IGNORE` path.
pub(crate) fn summary_hash(topic: &str, summary: &str) -> String {
    let topic_n = topic.trim().to_lowercase();
    let summary_n: String = summary
        .split_whitespace()
        .collect::<Vec<&str>>()
        .join(" ")
        .to_lowercase();
    let mut h = Sha256::new();
    h.update(topic_n.as_bytes());
    h.update(b"\0");
    h.update(summary_n.as_bytes());
    format!("{:x}", h.finalize())
}

pub(crate) fn parse_dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}
