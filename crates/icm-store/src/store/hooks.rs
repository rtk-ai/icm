//! SQLite backend — split out of the former monolithic `store.rs`.
//!
//! `SqliteStore` and the shared row/parse helpers live in `super`
//! (`store/mod.rs`); each submodule here holds one trait impl (or a
//! coherent group of inherent methods) on that type.

use super::*;

fn row_to_consolidation_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<ConsolidationJob> {
    let created_at: String = row.get(5)?;
    let completed_at: Option<String> = row.get(6)?;
    Ok(ConsolidationJob {
        id: row.get(0)?,
        topic: row.get(1)?,
        project: row.get(2)?,
        status: row.get(3)?,
        error: row.get(4)?,
        created_at: parse_dt(&created_at),
        completed_at: completed_at.map(|s| parse_dt(&s)),
    })
}

impl SqliteStore {
    /// Atomically increment the hook call counter and return the new value.
    pub fn increment_hook_counter(&self) -> IcmResult<usize> {
        let count: usize = self
            .conn
            .query_row(
                "INSERT INTO icm_metadata (key, value) VALUES ('hook_counter', '1')
                 ON CONFLICT(key) DO UPDATE SET value = CAST(CAST(value AS INTEGER) + 1 AS TEXT)
                 RETURNING CAST(value AS INTEGER)",
                [],
                |row| row.get(0),
            )
            .map_err(db_err)?;
        Ok(count)
    }

    /// Reset the hook call counter to 0.
    pub fn reset_hook_counter(&self) -> IcmResult<()> {
        self.conn
            .execute(
                "INSERT INTO icm_metadata (key, value) VALUES ('hook_counter', '0')
                 ON CONFLICT(key) DO UPDATE SET value = '0'",
                [],
            )
            .map_err(db_err)?;
        Ok(())
    }

    // Async extraction queue
    //
    // Row tuple shape: `(id, project, tool_name, raw_output, captured_at)`
    //
    // When `[extraction.summarizer].provider` is set to something other
    // than `"none"`, PostToolUse hooks INSERT raw tool output here in
    // ~50ms (no embedder load) and a worker (`icm extract-pending` or
    // the SessionEnd async fork) dequeues batches and runs the LLM CLI.

    /// Enqueue raw tool output for later LLM extraction. Returns the
    /// generated row id so the caller can correlate logs.
    pub fn enqueue_pending_extraction(
        &self,
        project: &str,
        tool_name: &str,
        raw_output: &str,
    ) -> IcmResult<String> {
        let id = ulid::Ulid::new().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        self.conn
            .execute(
                "INSERT INTO pending_extractions (id, project, tool_name, raw_output, captured_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![id, project, tool_name, raw_output, now],
            )
            .map_err(db_err)?;
        Ok(id)
    }

    /// Pop up to `limit` oldest pending rows. Caller is expected to call
    /// `delete_pending_extractions` after successful processing.
    pub fn list_pending_extractions(&self, limit: usize) -> IcmResult<Vec<PendingRow>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, project, tool_name, raw_output, captured_at
                 FROM pending_extractions
                 ORDER BY captured_at ASC
                 LIMIT ?1",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map([limit as i64], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        Ok(rows)
    }

    /// Delete pending rows by id. Used after a worker has processed them.
    pub fn delete_pending_extractions(&self, ids: &[String]) -> IcmResult<usize> {
        if ids.is_empty() {
            return Ok(0);
        }
        let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!("DELETE FROM pending_extractions WHERE id IN ({placeholders})");
        let params: Vec<&dyn rusqlite::ToSql> =
            ids.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        let n = self.conn.execute(&sql, params.as_slice()).map_err(db_err)?;
        Ok(n)
    }

    /// Total rows currently waiting in the queue. Used by `icm doctor`.
    pub fn pending_extraction_count(&self) -> IcmResult<usize> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM pending_extractions", [], |r| r.get(0))
            .map_err(db_err)?;
        Ok(n as usize)
    }

    // Async consolidation queue (issue #179)
    //
    // Mirrors the extraction queue above, but rows keep their terminal
    // status instead of being deleted so `icm consolidate-jobs` can show
    // history and a failed job can be retried.

    /// Enqueue a topic for later LLM consolidation. Returns the generated
    /// row id.
    pub fn enqueue_pending_consolidation(&self, topic: &str, project: &str) -> IcmResult<String> {
        let id = ulid::Ulid::new().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        self.conn
            .execute(
                "INSERT INTO pending_consolidations (id, topic, project, status, created_at)
                 VALUES (?1, ?2, ?3, 'pending', ?4)",
                rusqlite::params![id, topic, project, now],
            )
            .map_err(db_err)?;
        Ok(id)
    }

    /// Pop up to `limit` oldest `pending` jobs (FIFO by enqueue time).
    pub fn list_pending_consolidation_jobs(
        &self,
        limit: usize,
    ) -> IcmResult<Vec<ConsolidationJob>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, topic, project, status, error, created_at, completed_at
                 FROM pending_consolidations
                 WHERE status = 'pending'
                 ORDER BY created_at ASC
                 LIMIT ?1",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map([limit as i64], row_to_consolidation_job)
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        Ok(rows)
    }

    /// List jobs, optionally filtered by status (`pending` | `done` |
    /// `failed`); all statuses when `None`. Used by `icm consolidate-jobs`.
    pub fn list_consolidation_jobs(
        &self,
        status: Option<&str>,
        limit: usize,
    ) -> IcmResult<Vec<ConsolidationJob>> {
        let rows: Vec<ConsolidationJob> = match status {
            Some(s) => {
                let mut stmt = self
                    .conn
                    .prepare(
                        "SELECT id, topic, project, status, error, created_at, completed_at
                         FROM pending_consolidations
                         WHERE status = ?1
                         ORDER BY created_at DESC
                         LIMIT ?2",
                    )
                    .map_err(db_err)?;
                let result = stmt
                    .query_map(rusqlite::params![s, limit as i64], row_to_consolidation_job)
                    .map_err(db_err)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(db_err)?;
                result
            }
            None => {
                let mut stmt = self
                    .conn
                    .prepare(
                        "SELECT id, topic, project, status, error, created_at, completed_at
                         FROM pending_consolidations
                         ORDER BY created_at DESC
                         LIMIT ?1",
                    )
                    .map_err(db_err)?;
                let result = stmt
                    .query_map([limit as i64], row_to_consolidation_job)
                    .map_err(db_err)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(db_err)?;
                result
            }
        };
        Ok(rows)
    }

    /// Mark a job `done`. Non-fatal if the id no longer exists.
    pub fn mark_consolidation_job_done(&self, id: &str) -> IcmResult<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn
            .execute(
                "UPDATE pending_consolidations SET status = 'done', completed_at = ?2, error = NULL
                 WHERE id = ?1",
                rusqlite::params![id, now],
            )
            .map_err(db_err)?;
        Ok(())
    }

    /// Mark a job `failed`, capturing the error so `icm consolidate-jobs`
    /// can surface it and the user can retry.
    pub fn mark_consolidation_job_failed(&self, id: &str, error: &str) -> IcmResult<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn
            .execute(
                "UPDATE pending_consolidations SET status = 'failed', completed_at = ?2, error = ?3
                 WHERE id = ?1",
                rusqlite::params![id, now, error],
            )
            .map_err(db_err)?;
        Ok(())
    }

    /// Reset a `failed` job back to `pending` so the next drain retries it.
    /// Returns `false` if the id doesn't exist or isn't `failed`.
    pub fn retry_consolidation_job(&self, id: &str) -> IcmResult<bool> {
        let n = self
            .conn
            .execute(
                "UPDATE pending_consolidations
                 SET status = 'pending', error = NULL, completed_at = NULL
                 WHERE id = ?1 AND status = 'failed'",
                rusqlite::params![id],
            )
            .map_err(db_err)?;
        Ok(n > 0)
    }

    /// Total jobs currently `pending`. Used by `icm doctor`.
    pub fn pending_consolidation_count(&self) -> IcmResult<usize> {
        let n: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pending_consolidations WHERE status = 'pending'",
                [],
                |r| r.get(0),
            )
            .map_err(db_err)?;
        Ok(n as usize)
    }

    // Code areas (auto-captured file edits — issue #196)
    //
    // `cmd_hook_post` calls `upsert_code_area` whenever the upstream
    // tool was Edit / Write / MultiEdit / NotebookEdit. Same project +
    // file_path => touch_count++ via ON CONFLICT.

    /// Insert or refresh a row for `(project, file_path)`. On conflict
    /// bumps `touch_count`, updates `last_touched_at`, refreshes
    /// `session_id` / `tool_name`, and only overwrites `description` if
    /// the caller passes `Some` (so the most recent meaningful hint
    /// wins without clobbering an existing one with `None`).
    pub fn upsert_code_area(
        &self,
        project: &str,
        file_path: &str,
        description: Option<&str>,
        session_id: Option<&str>,
        tool_name: Option<&str>,
    ) -> IcmResult<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn
            .execute(
                "INSERT INTO code_areas (project, file_path, description,
                    session_id, tool_name, touch_count,
                    first_touched_at, last_touched_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?6)
                 ON CONFLICT(project, file_path) DO UPDATE SET
                    touch_count = touch_count + 1,
                    last_touched_at = excluded.last_touched_at,
                    session_id = COALESCE(excluded.session_id, session_id),
                    tool_name = COALESCE(excluded.tool_name, tool_name),
                    description = COALESCE(excluded.description, description)",
                rusqlite::params![project, file_path, description, session_id, tool_name, now],
            )
            .map_err(db_err)?;
        Ok(())
    }

    /// List code areas, optionally filtered by project / file_path /
    /// since timestamp. `limit` caps the result count (use `usize::MAX`
    /// to disable). Ordered by `last_touched_at DESC` so the freshest
    /// edits come first.
    pub fn list_code_areas(
        &self,
        project: Option<&str>,
        in_file: Option<&str>,
        since: Option<DateTime<Utc>>,
        limit: usize,
    ) -> IcmResult<Vec<CodeArea>> {
        let mut sql = String::from(
            "SELECT id, project, file_path, description, session_id, tool_name,
                    touch_count, first_touched_at, last_touched_at
             FROM code_areas
             WHERE 1=1",
        );
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(p) = project {
            sql.push_str(" AND project = ?");
            params.push(Box::new(p.to_string()));
        }
        if let Some(f) = in_file {
            // Match either an exact file_path or a path that ends with
            // the provided fragment so users can pass a short suffix.
            sql.push_str(" AND (file_path = ? OR file_path LIKE ?)");
            params.push(Box::new(f.to_string()));
            params.push(Box::new(format!("%/{f}")));
        }
        if let Some(t) = since {
            sql.push_str(" AND last_touched_at >= ?");
            params.push(Box::new(t.to_rfc3339()));
        }
        sql.push_str(" ORDER BY last_touched_at DESC LIMIT ?");
        params.push(Box::new(limit as i64));

        let mut stmt = self.conn.prepare(&sql).map_err(db_err)?;
        let param_refs: Vec<&dyn rusqlite::ToSql> = params
            .iter()
            .map(|p| p.as_ref() as &dyn rusqlite::ToSql)
            .collect();
        let rows = stmt
            .query_map(param_refs.as_slice(), |row| {
                let first: String = row.get(7)?;
                let last: String = row.get(8)?;
                Ok(CodeArea {
                    id: row.get(0)?,
                    project: row.get(1)?,
                    file_path: row.get(2)?,
                    description: row.get(3)?,
                    session_id: row.get(4)?,
                    tool_name: row.get(5)?,
                    touch_count: row.get(6)?,
                    first_touched_at: DateTime::parse_from_rfc3339(&first)
                        .map(|d| d.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now()),
                    last_touched_at: DateTime::parse_from_rfc3339(&last)
                        .map(|d| d.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now()),
                })
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        Ok(rows)
    }

    /// Total rows in `code_areas`. Cheap; used by stats / doctor.
    pub fn code_area_count(&self) -> IcmResult<usize> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM code_areas", [], |r| r.get(0))
            .map_err(db_err)?;
        Ok(n as usize)
    }

    // Hook telemetry
    //
    // Every `icm hook <event>` fire writes one row to `hook_events`. Read
    // back via `hook_events_recent` / `hook_stats`. Inserts are designed
    // to be cheap (single statement, no FTS) so they stay well under the
    // <50ms async-path budget.

    /// Append one hook telemetry row. Errors are swallowed by callers in
    /// hook paths (logging must never block the user), but tests can
    /// inspect the `Result`.
    pub fn record_hook_event(&self, ev: &HookEventInsert) -> IcmResult<i64> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn
            .execute(
                "INSERT INTO hook_events
                 (ts, event, project, session_id, tool_name,
                  duration_ms, exit_code, payload_size, note)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                rusqlite::params![
                    now,
                    ev.event,
                    ev.project,
                    ev.session_id,
                    ev.tool_name,
                    ev.duration_ms,
                    ev.exit_code,
                    ev.payload_size,
                    ev.note,
                ],
            )
            .map_err(db_err)?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Most recent `limit` hook events, newest first. Optional `event`
    /// filter (e.g. `Some("end")` to see only SessionEnd hooks).
    pub fn hook_events_recent(
        &self,
        limit: usize,
        event_filter: Option<&str>,
    ) -> IcmResult<Vec<HookEvent>> {
        let limit_i64 = limit as i64;
        let row_to_event = |row: &rusqlite::Row<'_>| -> rusqlite::Result<HookEvent> {
            let ts_str: String = row.get(1)?;
            let ts = chrono::DateTime::parse_from_rfc3339(&ts_str)
                .map(|t| t.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());
            Ok(HookEvent {
                id: row.get(0)?,
                ts,
                event: row.get(2)?,
                project: row.get(3)?,
                session_id: row.get(4)?,
                tool_name: row.get(5)?,
                duration_ms: row.get(6)?,
                exit_code: row.get(7)?,
                payload_size: row.get(8)?,
                note: row.get(9)?,
            })
        };
        match event_filter {
            Some(e) => {
                let mut stmt = self
                    .conn
                    .prepare(
                        "SELECT id, ts, event, project, session_id, tool_name,
                                duration_ms, exit_code, payload_size, note
                         FROM hook_events
                         WHERE event = ?1
                         ORDER BY id DESC
                         LIMIT ?2",
                    )
                    .map_err(db_err)?;
                let rows = stmt
                    .query_map(rusqlite::params![e, limit_i64], row_to_event)
                    .map_err(db_err)?;
                collect_rows(rows)
            }
            None => {
                let mut stmt = self
                    .conn
                    .prepare(
                        "SELECT id, ts, event, project, session_id, tool_name,
                                duration_ms, exit_code, payload_size, note
                         FROM hook_events
                         ORDER BY id DESC
                         LIMIT ?1",
                    )
                    .map_err(db_err)?;
                let rows = stmt
                    .query_map(rusqlite::params![limit_i64], row_to_event)
                    .map_err(db_err)?;
                collect_rows(rows)
            }
        }
    }

    /// Aggregate counts and latency percentiles per event type, over a
    /// time window starting `since` (RFC3339). Used by `icm hook-stats`.
    pub fn hook_stats(&self, since_rfc3339: &str) -> IcmResult<Vec<HookStatsRow>> {
        // Pull each event type and compute percentiles in Rust — SQLite
        // has no native percentile function and the row count is small
        // enough (~1k/day worst case) that an in-process sort is fine.
        let mut stmt = self
            .conn
            .prepare(
                "SELECT event, duration_ms, exit_code
                 FROM hook_events
                 WHERE ts >= ?1
                 ORDER BY event",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map([since_rfc3339], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<i64>>(1)?,
                    r.get::<_, i32>(2)?,
                ))
            })
            .map_err(db_err)?;
        let mut by_event: std::collections::BTreeMap<String, Vec<(Option<i64>, i32)>> =
            std::collections::BTreeMap::new();
        for r in rows {
            let (ev, dur, exit) = r.map_err(db_err)?;
            by_event.entry(ev).or_default().push((dur, exit));
        }
        let mut out = Vec::with_capacity(by_event.len());
        for (event, mut items) in by_event {
            let count = items.len() as i64;
            let error_count = items.iter().filter(|(_, e)| *e != 0).count() as i64;
            let mut durations: Vec<i64> = items.iter().filter_map(|(d, _)| *d).collect();
            durations.sort_unstable();
            let avg = if durations.is_empty() {
                0.0
            } else {
                durations.iter().sum::<i64>() as f64 / durations.len() as f64
            };
            let p = |q: f64| -> i64 {
                if durations.is_empty() {
                    0
                } else {
                    let idx = ((durations.len() as f64 - 1.0) * q).round() as usize;
                    durations[idx.min(durations.len() - 1)]
                }
            };
            out.push(HookStatsRow {
                event,
                count,
                error_count,
                avg_duration_ms: avg,
                p50_duration_ms: p(0.50),
                p99_duration_ms: p(0.99),
            });
            // Avoid clippy 'unused variable' on items after move
            let _ = &mut items;
        }
        Ok(out)
    }

    /// Total rows currently in `hook_events`. Used by tests and `icm doctor`.
    pub fn hook_event_count(&self) -> IcmResult<usize> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM hook_events", [], |r| r.get(0))
            .map_err(db_err)?;
        Ok(n as usize)
    }
}
