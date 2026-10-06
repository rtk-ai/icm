//! Per-domain tests for the SQLite backend (`store::tests`).

use super::*;

#[test]
fn integrity_check_ok_on_healthy_db() {
    let store = test_store();
    store.store(make_memory("t", "healthy row")).unwrap();
    assert_eq!(store.integrity_check().unwrap(), vec!["ok".to_string()]);
}

#[test]
fn integrity_check_structural_works_on_a_read_only_connection() {
    // #313 follow-up: `icm doctor` / `repair --dry-run` must inspect a DB
    // without a writable open. The structural check runs `PRAGMA
    // integrity_check` only (no FTS INSERT), so it works read-only.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ro.db");
    let _ = seed_writable_db(&path);

    let ro = SqliteStore::open_readonly(&path).unwrap();
    assert!(ro.is_readonly());
    // Read-only structural check succeeds and reports healthy.
    assert_eq!(
        ro.integrity_check_structural().unwrap(),
        vec!["ok".to_string()]
    );
    // The full check issues an FTS `INSERT` a read-only connection can't
    // run, so it degrades to reporting that as a problem — which is exactly
    // why the read-only inspection paths use the structural variant.
    assert_ne!(ro.integrity_check().unwrap(), vec!["ok".to_string()]);
}

#[test]
fn rebuild_search_indexes_lists_fts_tables_and_keeps_integrity() {
    let store = test_store();
    store.store(make_memory("t", "a row to index")).unwrap();
    let rebuilt = store.rebuild_search_indexes().unwrap();
    // memories_fts always exists; the concepts/feedback/messages FTS
    // tables are created by schema init too.
    assert!(
        rebuilt.contains(&"memories_fts".to_string()),
        "got: {rebuilt:?}"
    );
    assert_eq!(store.integrity_check().unwrap(), vec!["ok".to_string()]);
}

#[test]
fn rebuild_search_indexes_regenerates_fts_from_content() {
    // The core repair mechanism (#313): rebuilding must reconstruct the
    // FTS index from the intact content table. Deterministically wipe the
    // FTS index (the "index damaged, base table intact" class) and prove
    // rebuild restores searchability.
    let store = test_store();
    store
        .store(make_memory("t", "singulartoken repairable"))
        .unwrap();

    let fts_hits = |s: &SqliteStore| -> i64 {
        s.conn
            .query_row(
                "SELECT COUNT(*) FROM memories_fts WHERE memories_fts MATCH 'singulartoken'",
                [],
                |r| r.get(0),
            )
            .unwrap()
    };
    assert_eq!(fts_hits(&store), 1, "token should be indexed after store");

    // Wipe the FTS index while leaving the content row in `memories`.
    store
        .conn
        .execute_batch("INSERT INTO memories_fts(memories_fts) VALUES('delete-all');")
        .unwrap();
    assert_eq!(fts_hits(&store), 0, "index wiped → no FTS hit");

    // integrity_check (rank=1 content check) must flag the desync so that
    // `icm repair` actually triggers on it rather than reporting healthy.
    assert_ne!(
        store.integrity_check().unwrap(),
        vec!["ok".to_string()],
        "index/content desync must be detected"
    );

    store.rebuild_search_indexes().unwrap();
    assert_eq!(fts_hits(&store), 1, "rebuild must regenerate the FTS index");
    assert_eq!(store.integrity_check().unwrap(), vec!["ok".to_string()]);
}

#[test]
fn open_maintenance_errors_on_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("absent.db");
    match SqliteStore::open_maintenance(&path) {
        Ok(_) => panic!("open_maintenance on missing file must error"),
        Err(IcmError::NotFound(_)) => {}
        Err(other) => panic!("expected NotFound, got {other:?}"),
    }
}

#[test]
fn open_maintenance_opens_existing_db_writable() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seeded.db");
    let _ = seed_writable_db(&path);
    let store = SqliteStore::open_maintenance(&path).unwrap();
    assert!(!store.is_readonly());
    assert_eq!(store.integrity_check().unwrap(), vec!["ok".to_string()]);
}

// === Read-only store (issue #263) ===

#[test]
fn open_readonly_errors_on_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("absent.db");
    match SqliteStore::open_readonly(&path) {
        Ok(_) => panic!("open_readonly on missing file must error"),
        Err(IcmError::NotFound(msg)) => {
            assert!(msg.contains("absent.db"), "msg: {msg}")
        }
        Err(other) => panic!("expected NotFound, got {other:?}"),
    }
}

#[test]
fn open_readonly_can_read_existing_db() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seeded.db");
    let seeded = seed_writable_db(&path);

    let ro = SqliteStore::open_readonly(&path).unwrap();
    assert!(ro.is_readonly());

    // Read by id: must return the seeded memory verbatim.
    let got = ro.get(&seeded.id).unwrap().expect("memory must be present");
    assert_eq!(got.topic, "project:icm");
    assert_eq!(got.summary, "read-only fixture summary");
}

#[test]
fn read_only_connection_sees_writes_committed_after_open() {
    // #319: a long-lived `--read-only serve` connection must observe
    // writes that hooks/CLI commit to the same DB *after* the server
    // opened. The old `immutable=1` open served a permanently stale
    // snapshot (and eventually spurious "database disk image is malformed"
    // on a healthy DB).
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("live.db");
    let _ = seed_writable_db(&path); // 1 memory, WAL mode

    let ro = SqliteStore::open_readonly(&path).unwrap();
    assert_eq!(ro.count().unwrap(), 1);

    // A separate writer commits a new memory to the same file.
    {
        let rw = SqliteStore::new(&path).unwrap();
        let mut m = make_memory("project:icm", "written after the reader opened");
        m.embedding = Some(vec![0.2_f32; icm_core::DEFAULT_EMBEDDING_DIMS]);
        rw.store(m).unwrap();
    }

    // The already-open read-only connection must now see it.
    assert_eq!(
        ro.count().unwrap(),
        2,
        "read-only connection must see writes committed after it opened"
    );
}

#[cfg(unix)]
#[test]
fn open_readonly_falls_back_to_immutable_on_unwritable_dir() {
    // #263 must keep working after #319: on a `chmod -w` parent directory
    // the live `mode=ro` open can't create the `-shm` sidecar for a WAL
    // DB, so open_readonly must fall back to `immutable=1` and still read.
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sandboxed.db");
    let seeded = seed_writable_db(&path);
    // Keep the DB in WAL mode (that's what forces the -shm requirement)
    // but fold all rows into the main file and drop the sidecars, so a
    // fresh read-only open must recreate -shm — which fails in a
    // read-only dir. (A DELETE-mode DB would open fine via plain mode=ro
    // and never exercise the fallback.)
    {
        let rw = SqliteStore::new(&path).unwrap();
        rw.conn
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .unwrap();
    }
    for ext in ["-wal", "-shm"] {
        let mut side = path.as_os_str().to_os_string();
        side.push(ext);
        let _ = std::fs::remove_file(std::path::PathBuf::from(side));
    }

    let original = std::fs::metadata(dir.path()).unwrap().permissions();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o555)).unwrap();

    // Guard: confirm the live `mode=ro` path is genuinely unusable here,
    // otherwise this test would pass without ever exercising the fallback.
    let live_reads = match open_readonly_uri(&path, false) {
        Ok(conn) => conn
            .query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(()))
            .is_ok(),
        Err(_) => false,
    };
    // The public open_readonly must still succeed via the immutable fallback.
    let opened = SqliteStore::open_readonly(&path);

    // Restore permissions before asserting so tempdir cleanup succeeds.
    std::fs::set_permissions(dir.path(), original).unwrap();

    assert!(
        !live_reads,
        "sandbox setup must make the live mode=ro path unusable, else the fallback isn't tested"
    );
    let ro = opened.expect("read-only open must fall back to immutable on a chmod -w dir");
    let got = ro
        .get(&seeded.id)
        .unwrap()
        .expect("memory must be readable");
    assert_eq!(got.topic, "project:icm");
}

#[test]
fn read_only_recall_path_skips_access_bookkeeping() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seeded.db");
    let seeded = seed_writable_db(&path);

    let ro = SqliteStore::open_readonly(&path).unwrap();
    // The exact methods recall depends on:
    ro.maybe_auto_decay().unwrap();
    ro.update_access(&seeded.id).unwrap();
    ro.batch_update_access(&[&seeded.id]).unwrap();

    // Re-open writable to confirm nothing actually changed.
    let rw = SqliteStore::new(&path).unwrap();
    let after = rw.get(&seeded.id).unwrap().unwrap();
    assert_eq!(
        after.access_count, seeded.access_count,
        "access_count must NOT have been bumped by a read-only call",
    );
    assert_eq!(
        after.last_accessed, seeded.last_accessed,
        "last_accessed must NOT have been touched by a read-only call",
    );
}

#[test]
fn read_only_apply_decay_returns_readonly_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seeded.db");
    seed_writable_db(&path);
    let ro = SqliteStore::open_readonly(&path).unwrap();
    let err = ro.apply_decay(0.9).unwrap_err();
    match err {
        IcmError::ReadOnly(op) => assert_eq!(op, "apply_decay"),
        other => panic!("expected ReadOnly, got {other:?}"),
    }
}

#[test]
fn read_only_mutation_attempts_are_rejected_by_sqlite() {
    // Defense-in-depth: even mutation methods that aren't explicitly
    // gated must fail because the SQLite connection itself is
    // opened RO. If this test ever passes, SQLite's RO flag has
    // been bypassed somewhere.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seeded.db");
    seed_writable_db(&path);
    let ro = SqliteStore::open_readonly(&path).unwrap();
    let m = make_memory("project:icm", "should not land");
    let err = ro.store(m).unwrap_err();
    // SQLite's own "attempt to write a readonly database" wraps into
    // IcmError::Database — the actual variant doesn't matter, only
    // that the write was blocked.
    match err {
        IcmError::Database(_) | IcmError::ReadOnly(_) => {}
        other => panic!("expected Database or ReadOnly, got {other:?}"),
    }
}

// === Embedding dim peek (issue #267) ===

/// `read_stored_embedding_dims` must NOT trigger schema init: it is
/// called from the CLI *before* `with_dims` precisely to avoid the
/// destructive recreate path when running in `--no-embeddings`
/// against a DB that was populated with a non-default dim.
#[test]
fn read_stored_dims_returns_none_for_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("does-not-exist.db");
    assert_eq!(
        SqliteStore::read_stored_embedding_dims(&path).unwrap(),
        None
    );
}

#[test]
fn read_stored_dims_returns_none_for_legacy_db_without_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.db");
    // Plain SQLite file with no icm_metadata table — pretends to be a
    // pre-metadata legacy DB.
    let conn = Connection::open(&path).unwrap();
    conn.execute("CREATE TABLE foo (id TEXT)", []).unwrap();
    drop(conn);
    assert_eq!(
        SqliteStore::read_stored_embedding_dims(&path).unwrap(),
        None
    );
}

#[test]
fn read_stored_dims_returns_stored_value_for_populated_db() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("populated.db");
    // Build a DB at 1024 dims (representative of multilingual-e5-large).
    let store = SqliteStore::with_dims(&path, 1024).unwrap();
    drop(store);

    assert_eq!(
        SqliteStore::read_stored_embedding_dims(&path).unwrap(),
        Some(1024),
        "should return the stored dim, not the default 384",
    );
}

/// Issue #267 regression: opening the store at the *stored* dim
/// (the path the CLI now takes when no embedder is loaded) must
/// leave `vec_memories` and the `embedding` blobs intact. Before
/// the fix the CLI passed `DEFAULT_EMBEDDING_DIMS` instead and the
/// `stored != requested` branch of `init_db_with_dims` would
/// silently DROP `vec_memories` + NULL out every embedding.
#[test]
fn opening_at_stored_dims_preserves_vectors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("preserve.db");

    // First open: build the DB at 1024 dims and seed a memory with a
    // matching embedding.
    {
        let store = SqliteStore::with_dims(&path, 1024).unwrap();
        let mut mem = make_memory("topic-1024", "user prefers e5-large");
        mem.embedding = Some(vec![0.5_f32; 1024]);
        store.store(mem.clone()).unwrap();
    }

    // CLI-side resolution: peek the stored dims, then reopen at
    // exactly that value (the fix path).
    let stored = SqliteStore::read_stored_embedding_dims(&path)
        .unwrap()
        .expect("stored dim must be readable on populated DB");
    assert_eq!(stored, 1024);

    let store = SqliteStore::with_dims(&path, stored).unwrap();
    // Vec table still exists with the original dim.
    let dim_str: String = store
        .conn
        .query_row(
            "SELECT value FROM icm_metadata WHERE key = 'embedding_dims'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(dim_str, "1024");

    // The seeded memory's embedding still has the original 1024
    // floats — the destructive migration did NOT run.
    let kept_bytes: Option<i64> = store
        .conn
        .query_row(
            "SELECT length(embedding) FROM memories WHERE topic = 'topic-1024'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        kept_bytes,
        Some(1024 * 4),
        "embedding blob must NOT have been NULLed by the open path",
    );
}

// ──────────────────────────────────────────────────────────────
// claim_backup_slot tests
// ──────────────────────────────────────────────────────────────

/// With no existing row in icm_metadata, the very first call must claim
/// the slot and return `true`.
#[test]
fn claim_backup_slot_first_call_wins() {
    let store = test_store();
    let claimed = store.claim_backup_slot(7).unwrap();
    assert!(claimed, "first call should claim the backup slot");
}

/// A second call within the interval should see that the row was recently
/// written and return `false` (another process already owns this window).
#[test]
fn claim_backup_slot_second_call_within_interval_skips() {
    let store = test_store();
    // First call sets last_backup_at to now.
    let first = store.claim_backup_slot(7).unwrap();
    assert!(first, "first call must win");
    // Second call with the same interval — elapsed days ≈ 0, threshold 7.
    let second = store.claim_backup_slot(7).unwrap();
    assert!(!second, "second call within interval should be skipped");
}

/// With interval_days = 0 the condition `elapsed >= 0` is always true, so
/// even a call immediately after a previous one should win again.
#[test]
fn claim_backup_slot_after_interval_wins_again() {
    let store = test_store();
    let first = store.claim_backup_slot(0).unwrap();
    assert!(first, "first call must win");
    // With interval_days = 0 any positive elapsed time satisfies >= 0,
    // and julianday(now) - julianday(now) == 0, which still satisfies >= 0.
    let second = store.claim_backup_slot(0).unwrap();
    assert!(second, "with interval_days=0 every call should win");
}

/// Error-recovery path: after a failed backup the code resets
/// `last_backup_at` to the Unix epoch ("1970-01-01T00:00:00+00:00") so
/// the next startup will retry immediately. Verify that a subsequent call
/// returns `true` regardless of the configured interval.
#[test]
fn claim_backup_slot_epoch_reset_allows_immediate_retry() {
    let store = test_store();
    // Simulate the failure-reset: write the epoch timestamp directly.
    store
        .set_metadata_str("last_backup_at", "1970-01-01T00:00:00+00:00")
        .unwrap();
    // Any realistic interval — say 7 days. Elapsed since epoch is huge.
    let claimed = store.claim_backup_slot(7).unwrap();
    assert!(
        claimed,
        "epoch-reset should allow an immediate retry regardless of interval"
    );
}

// === MemoryStore tests ===

/// Reported failure: with several agent sessions on one database, reads
/// hung or failed behind another process's write. The side-writes of the
/// read path (backup slot, daily decay, access counters) must not wait for
/// the write lock when there is nothing to do, and must give up quickly
/// when there is.
#[test]
fn read_path_bookkeeping_does_not_wait_for_a_writer() {
    use rusqlite::Connection;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("busy.db");
    let store = SqliteStore::with_dims(&path, 64).unwrap();
    let id = store
        .store(make_memory("t", "bookkeeping under contention"))
        .unwrap();
    // Both slots are claimed once, so neither is due any more.
    assert!(store.claim_backup_slot(7).unwrap());
    store.maybe_auto_decay().unwrap();

    let writer = Connection::open(&path).unwrap();
    writer
        .execute_batch(
            "BEGIN IMMEDIATE;
             INSERT INTO icm_metadata (key, value) VALUES ('held', 'by-writer');",
        )
        .unwrap();

    let started = std::time::Instant::now();
    // Not due: answered from a read, no write lock needed.
    assert!(!store.claim_backup_slot(7).unwrap());
    store.maybe_auto_decay().unwrap();
    assert!(
        started.elapsed() < std::time::Duration::from_millis(500),
        "not-due slot checks waited {:?} behind a writer",
        started.elapsed()
    );

    // A real write on the read path gives up after the short timeout
    // instead of the 30s one; callers treat it as best-effort.
    let started = std::time::Instant::now();
    let bumped = store.batch_update_access(&[id.as_str()]);
    assert!(bumped.is_err(), "the writer still holds the lock");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "access bookkeeping waited {:?} behind a writer",
        started.elapsed()
    );

    // Hook telemetry and the hook counter run in every hook: same rule.
    let started = std::time::Instant::now();
    let event = HookEventInsert {
        event: "prompt".to_string(),
        project: None,
        session_id: None,
        tool_name: None,
        duration_ms: Some(1),
        exit_code: 0,
        payload_size: None,
        note: None,
    };
    assert!(store.record_hook_event(&event).is_err());
    assert!(store.increment_hook_counter().is_err());
    assert!(
        started.elapsed() < std::time::Duration::from_secs(8),
        "hook bookkeeping waited {:?} behind a writer",
        started.elapsed()
    );

    writer.execute_batch("ROLLBACK;").unwrap();
    // Lock released: bookkeeping works again, at the normal timeout.
    assert_eq!(store.batch_update_access(&[id.as_str()]).unwrap(), 1);
}
