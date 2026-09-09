//! SQLite backend — split out of the former monolithic `store.rs`.
//!
//! `SqliteStore` and the shared row/parse helpers live in `super`
//! (`store/mod.rs`); each submodule here holds one trait impl (or a
//! coherent group of inherent methods) on that type.

use super::*;
use rusqlite::OptionalExtension;

impl SqliteStore {
    /// Read a raw string value from `icm_metadata` by key.
    /// Returns `Ok(None)` when the key is absent.
    pub fn get_metadata_str(&self, key: &str) -> IcmResult<Option<String>> {
        self.conn
            .query_row(
                "SELECT value FROM icm_metadata WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_err)
    }

    /// Upsert a raw string value into `icm_metadata`.
    pub fn set_metadata_str(&self, key: &str, value: &str) -> IcmResult<()> {
        self.conn
            .execute(
                "INSERT INTO icm_metadata (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
            .map_err(db_err)?;
        Ok(())
    }

    /// Create a consistent, point-in-time backup of the database using the
    /// SQLite Online Backup API (`sqlite3_backup_init/step/finish`).
    ///
    /// Unlike `std::fs::copy`, this is safe while other processes are writing:
    /// the backup API locks pages one at a time, retries any page touched by a
    /// concurrent writer, and produces a fully checkpointed destination that
    /// includes every committed WAL frame. No need to copy `-wal`/`-shm`
    /// sidecars separately.
    ///
    /// The destination file is created (or overwritten) by the API itself.
    /// Returns an error if the source cannot be opened for backup or the
    /// destination cannot be written.
    pub fn backup_to(&self, dst: &Path) -> IcmResult<()> {
        // Ensure the destination directory exists (parallel to how with_dims
        // creates the parent dir for the main DB).
        if let Some(parent) = dst.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .map_err(|e| IcmError::Database(format!("creating backup dir: {e}")))?;
        }
        // `Connection::backup` is gated behind the `backup` rusqlite feature.
        // It calls sqlite3_backup_init/step/finish and handles the "source
        // page was dirtied during backup" retry loop internally, so a
        // concurrent writer does not corrupt the destination.
        self.conn
            .backup(rusqlite::DatabaseName::Main, dst, None)
            .map_err(|e| IcmError::Database(format!("sqlite online backup failed: {e}")))?;

        // POW-1: Restrict permissions on the backup file. A backup contains
        // the full memory database (potentially including API keys, client
        // data) and must not be world-readable. Mirrors the 0o600 mode used
        // for credentials in cloud.rs. Non-fatal — warn rather than aborting
        // a successful backup over a permission failure (e.g. FAT32 / NFS).
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Err(e) = std::fs::set_permissions(dst, std::fs::Permissions::from_mode(0o600)) {
                tracing::warn!(
                    path = %dst.display(),
                    "backup_to: could not restrict permissions to 0o600: {e}"
                );
            }
        }

        Ok(())
    }

    /// Atomically claim the backup slot.
    ///
    /// Returns `true` if **this** process should perform the backup (i.e. no
    /// other process has claimed the slot within the configured interval).
    /// Modelled on `maybe_auto_decay` — a single SQL statement wins the race
    /// so that in a thundering-herd scenario (many agents starting at once)
    /// exactly one performs the backup and the rest skip it (KRIT-5).
    pub fn claim_backup_slot(&self, interval_days: u32) -> IcmResult<bool> {
        let now = chrono::Utc::now().to_rfc3339();
        let changed = self
            .conn
            .execute(
                "INSERT INTO icm_metadata (key, value) VALUES ('last_backup_at', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = ?1
                 WHERE value IS NULL
                    OR julianday(?1) - julianday(value) >= ?2",
                rusqlite::params![now, f64::from(interval_days)],
            )
            .map_err(db_err)?;
        Ok(changed > 0)
    }

    /// Check database integrity (issue #313) and return a list of problems.
    /// A healthy database yields exactly `["ok"]`; a damaged one yields one
    /// entry per problem.
    ///
    /// Two complementary checks run:
    /// 1. `PRAGMA integrity_check` — structural b-tree / page validation.
    ///    This is what caught the shadow-table and index damage reported in
    ///    the incident (`btreeInitPage`, `wrong # of entries in index …`).
    /// 2. FTS5 `'integrity-check'` per shadow table — validates each FTS
    ///    index's internal structure, complementing the structural pass for
    ///    damage confined to the FTS shadow tables.
    ///
    /// This never returns `Err`: even a failure to *run* a check (e.g. an FTS
    /// vtable too damaged to instantiate) is recorded as a problem, so the
    /// caller — `icm doctor` / `icm repair` — always gets a usable verdict on
    /// a badly corrupt database instead of a propagated error.
    pub fn integrity_check(&self) -> IcmResult<Vec<String>> {
        let mut problems = Vec::new();

        // 1. Structural check. Record a run failure as a problem instead of
        //    aborting the whole verdict.
        match self.run_integrity_pragma() {
            Ok(lines) => problems.extend(lines.into_iter().filter(|l| l != "ok")),
            Err(e) => problems.push(format!("integrity_check pragma failed: {e}")),
        }

        // 2. Per-FTS-table consistency.
        for table in FTS_TABLES {
            // `rank = 1` makes FTS5 verify the index against the *content*
            // table, not just its own internal structure. Without it, an
            // index that is stale or out of step with the base table (e.g.
            // an interrupted write) is reported as healthy. Requires
            // SQLite ≥ 3.37 (bundled rusqlite is well past that).
            let sql = format!("INSERT INTO {table}({table}, rank) VALUES('integrity-check', 1);");
            match self.conn.execute_batch(&sql) {
                Ok(()) => {}
                Err(e) if is_missing_table(&e) => {} // legacy DB without this table
                Err(e) => problems.push(format!("fts5 {table}: {e}")),
            }
        }

        if problems.is_empty() {
            Ok(vec!["ok".to_string()])
        } else {
            Ok(problems)
        }
    }

    /// Structural-only integrity check (`PRAGMA integrity_check`), safe on a
    /// read-only connection (issue #313 follow-up). Unlike
    /// [`Self::integrity_check`] it does NOT run the FTS5 `'integrity-check'`
    /// (which is an `INSERT` and needs a writable connection), so it never
    /// mutates the DB or triggers a WAL checkpoint. Used by the read-only
    /// inspection paths (`icm doctor`, `icm repair --dry-run`). A healthy DB
    /// yields `["ok"]`. Never returns `Err`.
    pub fn integrity_check_structural(&self) -> IcmResult<Vec<String>> {
        let problems: Vec<String> = match self.run_integrity_pragma() {
            Ok(lines) => lines.into_iter().filter(|l| l != "ok").collect(),
            Err(e) => vec![format!("integrity_check pragma failed: {e}")],
        };
        if problems.is_empty() {
            Ok(vec!["ok".to_string()])
        } else {
            Ok(problems)
        }
    }

    /// Run `PRAGMA integrity_check` and collect its result rows.
    fn run_integrity_pragma(&self) -> IcmResult<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("PRAGMA integrity_check")
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(db_err)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(db_err)?);
        }
        Ok(out)
    }

    /// Rebuild the FTS5 shadow tables from their content tables and `REINDEX`
    /// every b-tree index (issue #313). This repairs the most common
    /// corruption class — damaged indexes / FTS shadow tables with intact
    /// base tables — without touching row data.
    ///
    /// Best-effort by design: a shadow table too damaged for `'rebuild'` to
    /// even instantiate is skipped rather than aborting the whole repair, and
    /// `REINDEX` failure is tolerated too. The caller re-runs
    /// [`Self::integrity_check`] afterwards and reports any damage that
    /// survived, so nothing is silently claimed fixed. Returns the FTS tables
    /// that were successfully rebuilt.
    pub fn rebuild_search_indexes(&self) -> IcmResult<Vec<String>> {
        let mut rebuilt = Vec::new();
        for table in FTS_TABLES {
            let sql = format!("INSERT INTO {table}({table}) VALUES('rebuild');");
            // A missing (legacy DB) or too-corrupt-to-instantiate shadow table
            // is skipped rather than aborting; the post-repair integrity check
            // surfaces any table that could not be restored.
            if self.conn.execute_batch(&sql).is_ok() {
                rebuilt.push(table.to_string());
            }
        }
        // May fail on a badly damaged b-tree; the post-repair integrity check
        // reports whatever REINDEX could not fix.
        let _ = self.conn.execute_batch("REINDEX;");
        Ok(rebuilt)
    }

    /// Apply decay if more than 24 hours since last decay.
    /// Called automatically on recall to avoid manual `icm decay` cron.
    ///
    /// No-op when the store is read-only (issue #263): recall must work
    /// against a DB the process cannot write to, and the bookkeeping
    /// writes here would otherwise abort the whole read with
    /// "attempt to write a readonly database".
    pub fn maybe_auto_decay(&self) -> IcmResult<()> {
        if self.readonly {
            return Ok(());
        }
        let now = Utc::now();
        let now_str = now.to_rfc3339();

        // Audit finding: this used to apply a flat 0.95 step whenever >= 1
        // day had passed, regardless of HOW MANY days had actually passed —
        // a machine touched once a week decayed at 0.95/week (≈0.993/day)
        // instead of the documented 0.95/day. Read the previous timestamp
        // first so the step can be `0.95 ^ elapsed_days` (compounded),
        // matching the documented per-day rate regardless of gaps between
        // calls. (The 0.95 base itself stays hardcoded here — wiring the
        // CLI's configurable `decay_rate` through to this crate is a
        // separate, out-of-scope change.)
        let last_decay_at: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM icm_metadata WHERE key = 'last_decay_at'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_err)?;
        let elapsed_days = last_decay_at
            .as_deref()
            .and_then(|prev| prev.parse::<DateTime<Utc>>().ok())
            .map(|prev| (now - prev).num_seconds() as f64 / 86_400.0)
            .filter(|d| d.is_finite() && *d > 0.0)
            .unwrap_or(1.0); // first run ever: preserve the historical single-step behavior

        // Atomic check-and-update: only one caller wins the race. (A narrow
        // window between the read above and this claim could let a losing
        // racer's `elapsed_days` be computed from a slightly stale
        // timestamp, but only one claim ever succeeds — a day or two of
        // imprecision in a decay RATE is harmless, not worth a stricter
        // compare-and-swap loop.)
        let changed = self
            .conn
            .execute(
                "INSERT INTO icm_metadata (key, value) VALUES ('last_decay_at', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = ?1
                 WHERE value IS NULL OR julianday(?1) - julianday(value) >= 1.0",
                params![now_str],
            )
            .map_err(db_err)?;

        if changed > 0 {
            let factor = 0.95_f64.powf(elapsed_days) as f32;
            self.apply_decay(factor)?;
        }

        Ok(())
    }

    /// Delete hook telemetry rows older than `cutoff_rfc3339`. Used by an
    /// optional retention pass (`icm hook-log --prune-older-than ...`).
    pub fn prune_hook_events(&self, cutoff_rfc3339: &str) -> IcmResult<usize> {
        let n = self
            .conn
            .execute(
                "DELETE FROM hook_events WHERE ts < ?1",
                rusqlite::params![cutoff_rfc3339],
            )
            .map_err(db_err)?;
        Ok(n)
    }

    /// Automatically consolidate a topic if it exceeds the threshold.
    ///
    /// Keeps the top 3 summaries (by weight), merges all unique keywords,
    /// and replaces all memories with a single consolidated memory.
    /// Returns `true` if consolidation was performed.
    ///
    /// Backwards-compatible no-embedder variant. Prefer
    /// [`auto_consolidate_with_embedder`] for new code so the
    /// consolidated memory keeps a fresh embedding instead of being
    /// silently un-recallable via vector search.
    pub fn auto_consolidate(&self, topic: &str, threshold: usize) -> IcmResult<bool> {
        self.auto_consolidate_with_embedder(topic, threshold, None)
    }

    /// Same as [`auto_consolidate`] but also embeds the consolidated
    /// memory when an embedder is available.
    ///
    /// Audit finding M2/AC2: the no-embedder variant produced a
    /// consolidated memory with `embedding = None`, leaving it
    /// invisible to hybrid / vector search until a manual `icm embed`
    /// rebuilt it. With this variant the embedder is invoked inline so
    /// the consolidated memory is recall-ready as soon as the topic is
    /// rolled up.
    pub fn auto_consolidate_with_embedder(
        &self,
        topic: &str,
        threshold: usize,
        embedder: Option<&dyn Embedder>,
    ) -> IcmResult<bool> {
        let count = self.count_by_topic(topic)?;
        if count < threshold {
            return Ok(false);
        }

        let mut memories = self.get_by_topic(topic)?;
        // `critical` memories are exempt from consolidation (consolidate_topic
        // won't delete them), so they neither count toward the threshold nor
        // feed the rollup summary — otherwise a topic holding >= threshold
        // criticals would re-consolidate on every store, churning forever.
        memories.retain(|m| !matches!(m.importance, Importance::Critical));
        if memories.is_empty() || memories.len() < threshold {
            return Ok(false);
        }

        // Sort by weight DESC (get_by_topic already does this, but be explicit)
        memories.sort_by(|a, b| {
            b.weight
                .partial_cmp(&a.weight)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Take the top 3 summaries for the consolidated summary
        let top_summaries: Vec<&str> = memories
            .iter()
            .take(3)
            .map(|m| m.summary.as_str())
            .collect();
        let mut consolidated_summary = top_summaries.join(" | ");
        // Three max-size summaries joined can exceed MAX_SUMMARY_BYTES, and
        // consolidate_topic now validates its input — truncate on a char
        // boundary so the rollup can't fail its own size check.
        if consolidated_summary.len() > MAX_SUMMARY_BYTES {
            let mut cut = MAX_SUMMARY_BYTES;
            while !consolidated_summary.is_char_boundary(cut) {
                cut -= 1;
            }
            consolidated_summary.truncate(cut);
        }

        // Merge all unique keywords
        let mut all_keywords: Vec<String> = Vec::new();
        let mut seen_keywords: HashSet<String> = HashSet::new();
        for mem in &memories {
            for kw in &mem.keywords {
                let lower = kw.to_lowercase();
                if seen_keywords.insert(lower) {
                    all_keywords.push(kw.clone());
                }
            }
        }

        let original_count = memories.len();

        // Build the consolidated memory
        let mut consolidated = Memory::new(topic.into(), consolidated_summary, Importance::High);
        consolidated.keywords = all_keywords;
        consolidated.raw_excerpt =
            Some(format!("auto-consolidated from {original_count} memories"));
        consolidated.weight = 1.0;

        // Embed the consolidated content if an embedder is available so
        // hybrid recall picks it up immediately. Errors are logged and
        // swallowed — a partial consolidation (no embedding) is still
        // better than blocking the whole rollup on an embedder hiccup.
        if let Some(emb) = embedder {
            match emb.embed(&consolidated.embed_text()) {
                Ok(vec) => consolidated.embedding = Some(vec),
                Err(e) => {
                    tracing::warn!(
                        "auto-consolidate: embedding failed for topic '{topic}': {e}; \
                         consolidated memory will lack vector representation"
                    );
                }
            }
        }

        // Replace all memories in the topic with the consolidated one
        self.consolidate_topic(topic, consolidated)?;

        Ok(true)
    }
}
