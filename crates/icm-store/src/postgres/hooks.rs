//! PostgreSQL backend -- split out of the former monolithic postgres.rs.
//!
//! Hook telemetry, the extraction queue, and code areas.

use super::*;

impl PostgresStore {
    /// Atomically increment the hook call counter and return the new value.
    pub fn increment_hook_counter(&self) -> IcmResult<usize> {
        let mut c = self.conn()?;
        let row = c
            .query_one(
                "INSERT INTO icm_metadata (key, value) VALUES ('hook_counter', '1')
                 ON CONFLICT (key) DO UPDATE SET value = ((icm_metadata.value::bigint) + 1)::text
                 RETURNING value::bigint",
                &[],
            )
            .map_err(pg_err)?;
        let n: i64 = row.get(0);
        Ok(n.max(0) as usize)
    }

    /// Reset the hook call counter to 0.
    pub fn reset_hook_counter(&self) -> IcmResult<()> {
        let mut c = self.conn()?;
        c.execute(
            "INSERT INTO icm_metadata (key, value) VALUES ('hook_counter', '0')
             ON CONFLICT (key) DO UPDATE SET value = '0'",
            &[],
        )
        .map_err(pg_err)?;
        Ok(())
    }

    // Async extraction queue

    /// Enqueue raw tool output for later LLM extraction.
    pub fn enqueue_pending_extraction(
        &self,
        project: &str,
        tool_name: &str,
        raw_output: &str,
    ) -> IcmResult<String> {
        let id = ulid::Ulid::new().to_string();
        let mut c = self.conn()?;
        c.execute(
            "INSERT INTO pending_extractions (id, project, tool_name, raw_output, captured_at)
             VALUES ($1, $2, $3, $4, $5)",
            &[&id, &project, &tool_name, &raw_output, &Utc::now()],
        )
        .map_err(pg_err)?;
        Ok(id)
    }

    /// Pop up to `limit` oldest pending rows (FIFO by capture time).
    pub fn list_pending_extractions(&self, limit: usize) -> IcmResult<Vec<PendingRow>> {
        let mut c = self.conn()?;
        let rows = c
            .query(
                "SELECT id, project, tool_name, raw_output, captured_at
                 FROM pending_extractions
                 ORDER BY captured_at ASC
                 LIMIT $1",
                &[&(limit as i64)],
            )
            .map_err(pg_err)?;
        Ok(rows
            .iter()
            .map(|row| {
                let captured: DateTime<Utc> = row.get(4);
                (
                    row.get(0),
                    row.get(1),
                    row.get(2),
                    row.get(3),
                    captured.to_rfc3339(),
                )
            })
            .collect())
    }

    /// Delete pending rows by id. Used after a worker has processed them.
    pub fn delete_pending_extractions(&self, ids: &[String]) -> IcmResult<usize> {
        if ids.is_empty() {
            return Ok(0);
        }
        let ids_vec: Vec<String> = ids.to_vec();
        let mut c = self.conn()?;
        let n = c
            .execute(
                "DELETE FROM pending_extractions WHERE id = ANY($1)",
                &[&ids_vec],
            )
            .map_err(pg_err)?;
        Ok(n as usize)
    }

    /// Total rows currently waiting in the queue.
    pub fn pending_extraction_count(&self) -> IcmResult<usize> {
        let mut c = self.conn()?;
        let row = c
            .query_one("SELECT COUNT(*) FROM pending_extractions", &[])
            .map_err(pg_err)?;
        let n: i64 = row.get(0);
        Ok(n.max(0) as usize)
    }

    // Async consolidation queue (issue #179)

    pub fn enqueue_pending_consolidation(&self, topic: &str, project: &str) -> IcmResult<String> {
        let id = ulid::Ulid::new().to_string();
        let mut c = self.conn()?;
        c.execute(
            "INSERT INTO pending_consolidations (id, topic, project, status, created_at)
             VALUES ($1, $2, $3, 'pending', $4)",
            &[&id, &topic, &project, &Utc::now()],
        )
        .map_err(pg_err)?;
        Ok(id)
    }

    fn row_to_consolidation_job(row: &postgres::Row) -> ConsolidationJob {
        let created_at: DateTime<Utc> = row.get(5);
        let completed_at: Option<DateTime<Utc>> = row.get(6);
        ConsolidationJob {
            id: row.get(0),
            topic: row.get(1),
            project: row.get(2),
            status: row.get(3),
            error: row.get(4),
            created_at,
            completed_at,
        }
    }

    pub fn list_pending_consolidation_jobs(
        &self,
        limit: usize,
    ) -> IcmResult<Vec<ConsolidationJob>> {
        let mut c = self.conn()?;
        let rows = c
            .query(
                "SELECT id, topic, project, status, error, created_at, completed_at
                 FROM pending_consolidations
                 WHERE status = 'pending'
                 ORDER BY created_at ASC
                 LIMIT $1",
                &[&(limit as i64)],
            )
            .map_err(pg_err)?;
        Ok(rows.iter().map(Self::row_to_consolidation_job).collect())
    }

    pub fn list_consolidation_jobs(
        &self,
        status: Option<&str>,
        limit: usize,
    ) -> IcmResult<Vec<ConsolidationJob>> {
        let mut c = self.conn()?;
        let rows = match status {
            Some(s) => c
                .query(
                    "SELECT id, topic, project, status, error, created_at, completed_at
                     FROM pending_consolidations
                     WHERE status = $1
                     ORDER BY created_at DESC
                     LIMIT $2",
                    &[&s, &(limit as i64)],
                )
                .map_err(pg_err)?,
            None => c
                .query(
                    "SELECT id, topic, project, status, error, created_at, completed_at
                     FROM pending_consolidations
                     ORDER BY created_at DESC
                     LIMIT $1",
                    &[&(limit as i64)],
                )
                .map_err(pg_err)?,
        };
        Ok(rows.iter().map(Self::row_to_consolidation_job).collect())
    }

    pub fn mark_consolidation_job_done(&self, id: &str) -> IcmResult<()> {
        let mut c = self.conn()?;
        c.execute(
            "UPDATE pending_consolidations SET status = 'done', completed_at = $2, error = NULL
             WHERE id = $1",
            &[&id, &Utc::now()],
        )
        .map_err(pg_err)?;
        Ok(())
    }

    pub fn mark_consolidation_job_failed(&self, id: &str, error: &str) -> IcmResult<()> {
        let mut c = self.conn()?;
        c.execute(
            "UPDATE pending_consolidations SET status = 'failed', completed_at = $2, error = $3
             WHERE id = $1",
            &[&id, &Utc::now(), &error],
        )
        .map_err(pg_err)?;
        Ok(())
    }

    pub fn retry_consolidation_job(&self, id: &str) -> IcmResult<bool> {
        let mut c = self.conn()?;
        let n = c
            .execute(
                "UPDATE pending_consolidations
                 SET status = 'pending', error = NULL, completed_at = NULL
                 WHERE id = $1 AND status = 'failed'",
                &[&id],
            )
            .map_err(pg_err)?;
        Ok(n > 0)
    }

    pub fn pending_consolidation_count(&self) -> IcmResult<usize> {
        let mut c = self.conn()?;
        let row = c
            .query_one(
                "SELECT COUNT(*) FROM pending_consolidations WHERE status = 'pending'",
                &[],
            )
            .map_err(pg_err)?;
        let n: i64 = row.get(0);
        Ok(n.max(0) as usize)
    }

    // Code areas (issue #196)

    /// Insert or refresh a row for `(project, file_path)`.
    pub fn upsert_code_area(
        &self,
        project: &str,
        file_path: &str,
        description: Option<&str>,
        session_id: Option<&str>,
        tool_name: Option<&str>,
    ) -> IcmResult<()> {
        let now = Utc::now();
        let mut c = self.conn()?;
        c.execute(
            "INSERT INTO code_areas
                (project, file_path, description, session_id, tool_name,
                 touch_count, first_touched_at, last_touched_at)
             VALUES ($1, $2, $3, $4, $5, 1, $6, $6)
             ON CONFLICT (project, file_path) DO UPDATE SET
                touch_count = code_areas.touch_count + 1,
                last_touched_at = EXCLUDED.last_touched_at,
                session_id = COALESCE(EXCLUDED.session_id, code_areas.session_id),
                tool_name = COALESCE(EXCLUDED.tool_name, code_areas.tool_name),
                description = COALESCE(EXCLUDED.description, code_areas.description)",
            &[
                &project,
                &file_path,
                &description,
                &session_id,
                &tool_name,
                &now,
            ],
        )
        .map_err(pg_err)?;
        Ok(())
    }

    /// List code areas, optionally filtered, newest-touch first.
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
             FROM code_areas WHERE TRUE",
        );
        let mut owned: Vec<Box<dyn ToSql + Sync>> = Vec::new();
        if let Some(p) = project {
            owned.push(Box::new(p.to_string()));
            sql.push_str(&format!(" AND project = ${}", owned.len()));
        }
        if let Some(f) = in_file {
            owned.push(Box::new(f.to_string()));
            let exact = owned.len();
            owned.push(Box::new(format!("%/{f}")));
            let suffix = owned.len();
            sql.push_str(&format!(
                " AND (file_path = ${exact} OR file_path LIKE ${suffix})"
            ));
        }
        if let Some(t) = since {
            owned.push(Box::new(t));
            sql.push_str(&format!(" AND last_touched_at >= ${}", owned.len()));
        }
        owned.push(Box::new(limit as i64));
        sql.push_str(&format!(
            " ORDER BY last_touched_at DESC LIMIT ${}",
            owned.len()
        ));

        let params: Vec<&(dyn ToSql + Sync)> = owned.iter().map(|b| b.as_ref()).collect();
        let mut c = self.conn()?;
        let rows = c.query(&sql, &params).map_err(pg_err)?;
        Ok(rows
            .iter()
            .map(|row| CodeArea {
                id: row.get(0),
                project: row.get(1),
                file_path: row.get(2),
                description: row.get(3),
                session_id: row.get(4),
                tool_name: row.get(5),
                touch_count: row.get(6),
                first_touched_at: row.get(7),
                last_touched_at: row.get(8),
            })
            .collect())
    }

    /// Total rows in `code_areas`.
    pub fn code_area_count(&self) -> IcmResult<usize> {
        let mut c = self.conn()?;
        let row = c
            .query_one("SELECT COUNT(*) FROM code_areas", &[])
            .map_err(pg_err)?;
        let n: i64 = row.get(0);
        Ok(n.max(0) as usize)
    }

    // Hook telemetry

    /// Append one hook telemetry row, returning its id.
    pub fn record_hook_event(&self, ev: &HookEventInsert) -> IcmResult<i64> {
        let mut c = self.conn()?;
        let row = c
            .query_one(
                "INSERT INTO hook_events
                 (ts, event, project, session_id, tool_name,
                  duration_ms, exit_code, payload_size, note)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                 RETURNING id",
                &[
                    &Utc::now(),
                    &ev.event,
                    &ev.project,
                    &ev.session_id,
                    &ev.tool_name,
                    &ev.duration_ms,
                    &ev.exit_code,
                    &ev.payload_size,
                    &ev.note,
                ],
            )
            .map_err(pg_err)?;
        Ok(row.get(0))
    }

    /// Most recent `limit` hook events, newest first; optional event filter.
    pub fn hook_events_recent(
        &self,
        limit: usize,
        event_filter: Option<&str>,
    ) -> IcmResult<Vec<HookEvent>> {
        let mut c = self.conn()?;
        let rows = match event_filter {
            Some(ev) => c.query(
                "SELECT id, ts, event, project, session_id, tool_name,
                        duration_ms, exit_code, payload_size, note
                 FROM hook_events WHERE event = $1 ORDER BY id DESC LIMIT $2",
                &[&ev, &(limit as i64)],
            ),
            None => c.query(
                "SELECT id, ts, event, project, session_id, tool_name,
                        duration_ms, exit_code, payload_size, note
                 FROM hook_events ORDER BY id DESC LIMIT $1",
                &[&(limit as i64)],
            ),
        }
        .map_err(pg_err)?;
        Ok(rows
            .iter()
            .map(|row| HookEvent {
                id: row.get(0),
                ts: row.get(1),
                event: row.get(2),
                project: row.get(3),
                session_id: row.get(4),
                tool_name: row.get(5),
                duration_ms: row.get(6),
                exit_code: row.get(7),
                payload_size: row.get(8),
                note: row.get(9),
            })
            .collect())
    }

    /// Per-event aggregate stats since `since_rfc3339`.
    pub fn hook_stats(&self, since_rfc3339: &str) -> IcmResult<Vec<HookStatsRow>> {
        let since = DateTime::parse_from_rfc3339(since_rfc3339)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now() - chrono::Duration::days(7));
        let mut c = self.conn()?;
        let rows = c
            .query(
                "SELECT event,
                        COUNT(*)::bigint,
                        COUNT(*) FILTER (WHERE exit_code <> 0)::bigint,
                        COALESCE(AVG(duration_ms), 0)::float8,
                        COALESCE(percentile_cont(0.5) WITHIN GROUP (ORDER BY duration_ms), 0)::float8,
                        COALESCE(percentile_cont(0.99) WITHIN GROUP (ORDER BY duration_ms), 0)::float8
                 FROM hook_events WHERE ts >= $1
                 GROUP BY event ORDER BY event",
                &[&since],
            )
            .map_err(pg_err)?;
        Ok(rows
            .iter()
            .map(|row| {
                let p50: f64 = row.get(4);
                let p99: f64 = row.get(5);
                HookStatsRow {
                    event: row.get(0),
                    count: row.get(1),
                    error_count: row.get(2),
                    avg_duration_ms: row.get(3),
                    p50_duration_ms: p50 as i64,
                    p99_duration_ms: p99 as i64,
                }
            })
            .collect())
    }

    /// Delete hook events older than `cutoff_rfc3339`.
    pub fn prune_hook_events(&self, cutoff_rfc3339: &str) -> IcmResult<usize> {
        let cutoff = DateTime::parse_from_rfc3339(cutoff_rfc3339)
            .map(|d| d.with_timezone(&Utc))
            .map_err(|e| IcmError::InvalidInput(format!("invalid cutoff timestamp: {e}")))?;
        let mut c = self.conn()?;
        let n = c
            .execute("DELETE FROM hook_events WHERE ts < $1", &[&cutoff])
            .map_err(pg_err)?;
        Ok(n as usize)
    }

    /// Total rows in `hook_events`.
    pub fn hook_event_count(&self) -> IcmResult<usize> {
        let mut c = self.conn()?;
        let row = c
            .query_one("SELECT COUNT(*) FROM hook_events", &[])
            .map_err(pg_err)?;
        let n: i64 = row.get(0);
        Ok(n.max(0) as usize)
    }
}
