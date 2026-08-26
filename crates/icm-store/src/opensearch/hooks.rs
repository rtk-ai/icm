//! OpenSearch backend -- split out of the former monolithic opensearch.rs.
//!
//! Hook telemetry, the extraction queue, and code areas.

use super::*;

impl OpenSearchStore {
    pub fn increment_hook_counter(&self) -> IcmResult<usize> {
        let resp = self.post(
            &format!("{IDX_METADATA}/_update/hook_counter?_source=true"),
            json!({
                "scripted_upsert": true,
                "upsert": {},
                "script": {
                    "lang": "painless",
                    "source": "ctx._source.value = (ctx._source.value == null ? 1 : ctx._source.value + 1);"
                }
            }),
        )?;
        Ok(resp
            .get("get")
            .and_then(|g| g.get("_source"))
            .and_then(|s| s.get("value"))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0) as usize)
    }

    pub fn reset_hook_counter(&self) -> IcmResult<()> {
        self.set_metadata_int("hook_counter", 0)
    }

    pub fn enqueue_pending_extraction(
        &self,
        project: &str,
        tool_name: &str,
        raw_output: &str,
    ) -> IcmResult<String> {
        let id = ulid::Ulid::new().to_string();
        self.request(
            "PUT",
            &format!("{IDX_PENDING}/_doc/{id}?{}", self.refresh_param()),
            Some(json!({
                "project": project,
                "tool_name": tool_name,
                "raw_output": raw_output,
                "captured_at": Utc::now().to_rfc3339()
            })),
            false,
        )?;
        Ok(id)
    }

    pub fn list_pending_extractions(&self, limit: usize) -> IcmResult<Vec<PendingRow>> {
        let resp = self.post(
            &format!("{IDX_PENDING}/_search"),
            json!({"size": limit, "query": {"match_all": {}}, "sort": [{"captured_at": "asc"}]}),
        )?;
        let rows = resp
            .get("hits")
            .and_then(|h| h.get("hits"))
            .and_then(|h| h.as_array())
            .map(|hits| {
                hits.iter()
                    .filter_map(|h| {
                        let id = h.get("_id")?.as_str()?.to_string();
                        let s = h.get("_source")?;
                        Some((
                            id,
                            s.get("project")?.as_str()?.to_string(),
                            s.get("tool_name")?.as_str()?.to_string(),
                            s.get("raw_output")?.as_str()?.to_string(),
                            s.get("captured_at")?.as_str()?.to_string(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(rows)
    }

    pub fn delete_pending_extractions(&self, ids: &[String]) -> IcmResult<usize> {
        if ids.is_empty() {
            return Ok(0);
        }
        let resp = self.post(
            &format!(
                "{IDX_PENDING}/_delete_by_query?{}&conflicts=proceed",
                self.refresh_param()
            ),
            json!({"query": {"ids": {"values": ids}}}),
        )?;
        Ok(resp.get("deleted").and_then(|v| v.as_u64()).unwrap_or(0) as usize)
    }

    pub fn pending_extraction_count(&self) -> IcmResult<usize> {
        let resp = self.post(&format!("{IDX_PENDING}/_count"), json!({}))?;
        Ok(resp.get("count").and_then(|v| v.as_u64()).unwrap_or(0) as usize)
    }

    // Async consolidation queue (issue #179)

    pub fn enqueue_pending_consolidation(&self, topic: &str, project: &str) -> IcmResult<String> {
        let id = ulid::Ulid::new().to_string();
        self.request(
            "PUT",
            &format!(
                "{IDX_CONSOLIDATION_JOBS}/_doc/{id}?{}",
                self.refresh_param()
            ),
            Some(json!({
                "topic": topic,
                "project": project,
                "status": "pending",
                "created_at": Utc::now().to_rfc3339()
            })),
            false,
        )?;
        Ok(id)
    }

    fn hit_to_consolidation_job(h: &Value) -> Option<ConsolidationJob> {
        let id = h.get("_id")?.as_str()?.to_string();
        let s = h.get("_source")?;
        let created_at = s.get("created_at")?.as_str()?;
        Some(ConsolidationJob {
            id,
            topic: s.get("topic")?.as_str()?.to_string(),
            project: s
                .get("project")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            status: s.get("status")?.as_str()?.to_string(),
            error: s.get("error").and_then(|v| v.as_str()).map(String::from),
            created_at: created_at.parse().unwrap_or_else(|_| Utc::now()),
            completed_at: s
                .get("completed_at")
                .and_then(|v| v.as_str())
                .and_then(|v| v.parse().ok()),
        })
    }

    fn search_consolidation_jobs(
        &self,
        query: Value,
        limit: usize,
    ) -> IcmResult<Vec<ConsolidationJob>> {
        let resp = self.post(&format!("{IDX_CONSOLIDATION_JOBS}/_search"), query)?;
        Ok(resp
            .get("hits")
            .and_then(|h| h.get("hits"))
            .and_then(|h| h.as_array())
            .map(|hits| {
                hits.iter()
                    .filter_map(Self::hit_to_consolidation_job)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
            .into_iter()
            .take(limit)
            .collect())
    }

    pub fn list_pending_consolidation_jobs(
        &self,
        limit: usize,
    ) -> IcmResult<Vec<ConsolidationJob>> {
        self.search_consolidation_jobs(
            json!({
                "size": limit,
                "query": {"term": {"status": "pending"}},
                "sort": [{"created_at": "asc"}]
            }),
            limit,
        )
    }

    pub fn list_consolidation_jobs(
        &self,
        status: Option<&str>,
        limit: usize,
    ) -> IcmResult<Vec<ConsolidationJob>> {
        let query = match status {
            Some(s) => json!({
                "size": limit,
                "query": {"term": {"status": s}},
                "sort": [{"created_at": "desc"}]
            }),
            None => json!({
                "size": limit,
                "query": {"match_all": {}},
                "sort": [{"created_at": "desc"}]
            }),
        };
        self.search_consolidation_jobs(query, limit)
    }

    pub fn mark_consolidation_job_done(&self, id: &str) -> IcmResult<()> {
        self.request(
            "POST",
            &format!(
                "{IDX_CONSOLIDATION_JOBS}/_update/{id}?{}",
                self.refresh_param()
            ),
            Some(json!({
                "doc": {"status": "done", "completed_at": Utc::now().to_rfc3339(), "error": null}
            })),
            false,
        )?;
        Ok(())
    }

    pub fn mark_consolidation_job_failed(&self, id: &str, error: &str) -> IcmResult<()> {
        self.request(
            "POST",
            &format!(
                "{IDX_CONSOLIDATION_JOBS}/_update/{id}?{}",
                self.refresh_param()
            ),
            Some(json!({
                "doc": {"status": "failed", "completed_at": Utc::now().to_rfc3339(), "error": error}
            })),
            false,
        )?;
        Ok(())
    }

    pub fn retry_consolidation_job(&self, id: &str) -> IcmResult<bool> {
        // Scripted update (not a plain `doc` merge) so the reset is
        // conditional on the current status being `failed` — matches the
        // SQL backends' `WHERE status = 'failed'`, so retrying an
        // already-pending or already-done job is a no-op, not a silent
        // downgrade of its state.
        let resp = self.request(
            "POST",
            &format!(
                "{IDX_CONSOLIDATION_JOBS}/_update/{id}?{}",
                self.refresh_param()
            ),
            Some(json!({
                "script": {
                    "lang": "painless",
                    "source": "if (ctx._source.status == 'failed') { \
                                    ctx._source.status = 'pending'; \
                                    ctx._source.error = null; \
                                    ctx._source.completed_at = null; \
                                } else { \
                                    ctx.op = 'none'; \
                                }"
                }
            })),
            false,
        )?;
        Ok(resp
            .as_ref()
            .and_then(|v| v.get("result"))
            .and_then(|v| v.as_str())
            == Some("updated"))
    }

    pub fn pending_consolidation_count(&self) -> IcmResult<usize> {
        let resp = self.post(
            &format!("{IDX_CONSOLIDATION_JOBS}/_count"),
            json!({"query": {"term": {"status": "pending"}}}),
        )?;
        Ok(resp.get("count").and_then(|v| v.as_u64()).unwrap_or(0) as usize)
    }

    pub fn upsert_code_area(
        &self,
        project: &str,
        file_path: &str,
        description: Option<&str>,
        session_id: Option<&str>,
        tool_name: Option<&str>,
    ) -> IcmResult<()> {
        if self.readonly {
            return Err(IcmError::ReadOnly("upsert_code_area".into()));
        }
        let ts = Utc::now();
        let now = ts.to_rfc3339();
        let id = ts.timestamp_millis();
        // Deterministic id makes the same (project, file_path) a single row.
        let key = B64.encode(format!("{project}\0{file_path}"));
        self.request(
            "POST",
            &format!("{IDX_CODE_AREAS}/_update/{key}?{}", self.refresh_param()),
            Some(json!({
                "scripted_upsert": true,
                "upsert": {
                    "id": id,
                    "project": project,
                    "file_path": file_path,
                    "description": description,
                    "session_id": session_id,
                    "tool_name": tool_name,
                    "touch_count": 1,
                    "first_touched_at": now,
                    "last_touched_at": now
                },
                "script": {
                    "lang": "painless",
                    "source": "ctx._source.touch_count = (ctx._source.touch_count == null ? 1 : ctx._source.touch_count + 1); ctx._source.last_touched_at = params.now; if (params.description != null) ctx._source.description = params.description; if (params.session_id != null) ctx._source.session_id = params.session_id; if (params.tool_name != null) ctx._source.tool_name = params.tool_name;",
                    "params": {"now": now, "description": description, "session_id": session_id, "tool_name": tool_name}
                }
            })),
            false,
        )?;
        Ok(())
    }

    pub fn list_code_areas(
        &self,
        project: Option<&str>,
        in_file: Option<&str>,
        since: Option<DateTime<Utc>>,
        limit: usize,
    ) -> IcmResult<Vec<CodeArea>> {
        let mut filters: Vec<Value> = Vec::new();
        if let Some(p) = project {
            filters.push(json!({"term": {"project": p}}));
        }
        if let Some(f) = in_file {
            filters.push(json!({"wildcard": {"file_path": {"value": format!("*{f}*")}}}));
        }
        if let Some(s) = since {
            filters.push(json!({"range": {"last_touched_at": {"gte": s.to_rfc3339()}}}));
        }
        let query = if filters.is_empty() {
            json!({"match_all": {}})
        } else {
            json!({"bool": {"filter": filters}})
        };
        let resp = self.post(
            &format!("{IDX_CODE_AREAS}/_search"),
            json!({"size": limit, "query": query, "sort": [{"last_touched_at": "desc"}]}),
        )?;
        let rows = resp
            .get("hits")
            .and_then(|h| h.get("hits"))
            .and_then(|h| h.as_array())
            .map(|hits| {
                hits.iter()
                    .filter_map(|h| {
                        let s = h.get("_source")?;
                        Some(CodeArea {
                            id: s.get("id").and_then(|v| v.as_i64()).unwrap_or(0),
                            project: s.get("project")?.as_str()?.to_string(),
                            file_path: s.get("file_path")?.as_str()?.to_string(),
                            description: s
                                .get("description")
                                .and_then(|v| v.as_str())
                                .map(String::from),
                            session_id: s
                                .get("session_id")
                                .and_then(|v| v.as_str())
                                .map(String::from),
                            tool_name: s
                                .get("tool_name")
                                .and_then(|v| v.as_str())
                                .map(String::from),
                            touch_count: s.get("touch_count").and_then(|v| v.as_i64()).unwrap_or(1),
                            first_touched_at: s
                                .get("first_touched_at")
                                .and_then(|v| v.as_str())
                                .and_then(|x| DateTime::parse_from_rfc3339(x).ok())
                                .map(|d| d.with_timezone(&Utc))
                                .unwrap_or_else(Utc::now),
                            last_touched_at: s
                                .get("last_touched_at")
                                .and_then(|v| v.as_str())
                                .and_then(|x| DateTime::parse_from_rfc3339(x).ok())
                                .map(|d| d.with_timezone(&Utc))
                                .unwrap_or_else(Utc::now),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(rows)
    }

    pub fn code_area_count(&self) -> IcmResult<usize> {
        let resp = self.post(&format!("{IDX_CODE_AREAS}/_count"), json!({}))?;
        Ok(resp.get("count").and_then(|v| v.as_u64()).unwrap_or(0) as usize)
    }

    pub fn record_hook_event(&self, ev: &HookEventInsert) -> IcmResult<i64> {
        let now = Utc::now();
        let id = now.timestamp_millis();
        let doc_id = ulid::Ulid::new().to_string();
        self.request(
            "PUT",
            &format!("{IDX_HOOKS}/_doc/{doc_id}"),
            Some(json!({
                "id": id,
                "ts": now.to_rfc3339(),
                "event": ev.event,
                "project": ev.project,
                "session_id": ev.session_id,
                "tool_name": ev.tool_name,
                "duration_ms": ev.duration_ms,
                "exit_code": ev.exit_code,
                "payload_size": ev.payload_size,
                "note": ev.note
            })),
            false,
        )?;
        Ok(id)
    }

    pub fn hook_events_recent(
        &self,
        limit: usize,
        event_filter: Option<&str>,
    ) -> IcmResult<Vec<HookEvent>> {
        let query = match event_filter {
            Some(e) => json!({"term": {"event": e}}),
            None => json!({"match_all": {}}),
        };
        let resp = self.post(
            &format!("{IDX_HOOKS}/_search"),
            json!({"size": limit, "query": query, "sort": [{"ts": "desc"}]}),
        )?;
        let rows = resp
            .get("hits")
            .and_then(|h| h.get("hits"))
            .and_then(|h| h.as_array())
            .map(|hits| {
                hits.iter()
                    .filter_map(|h| {
                        let s = h.get("_source")?;
                        Some(HookEvent {
                            id: s.get("id").and_then(|v| v.as_i64()).unwrap_or(0),
                            ts: parse_dt(s.get("ts").and_then(|v| v.as_str()).unwrap_or("")),
                            event: s
                                .get("event")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            project: s.get("project").and_then(|v| v.as_str()).map(String::from),
                            session_id: s
                                .get("session_id")
                                .and_then(|v| v.as_str())
                                .map(String::from),
                            tool_name: s
                                .get("tool_name")
                                .and_then(|v| v.as_str())
                                .map(String::from),
                            duration_ms: s.get("duration_ms").and_then(|v| v.as_i64()),
                            exit_code: s.get("exit_code").and_then(|v| v.as_i64()).unwrap_or(0)
                                as i32,
                            payload_size: s.get("payload_size").and_then(|v| v.as_i64()),
                            note: s.get("note").and_then(|v| v.as_str()).map(String::from),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(rows)
    }

    pub fn hook_stats(&self, since_rfc3339: &str) -> IcmResult<Vec<HookStatsRow>> {
        let resp = self.post(
            &format!("{IDX_HOOKS}/_search"),
            json!({
                "size": 0,
                "query": {"range": {"ts": {"gte": since_rfc3339}}},
                "aggs": {"events": {
                    "terms": {"field": "event", "size": 1000},
                    "aggs": {
                        "errs": {"filter": {"bool": {"must_not": [{"term": {"exit_code": 0}}]}}},
                        "avg_dur": {"avg": {"field": "duration_ms"}},
                        "pct": {"percentiles": {"field": "duration_ms", "percents": [50, 99]}}
                    }
                }}
            }),
        )?;
        let buckets = resp
            .get("aggregations")
            .and_then(|a| a.get("events"))
            .and_then(|e| e.get("buckets"))
            .and_then(|b| b.as_array())
            .cloned()
            .unwrap_or_default();
        let mut out = Vec::new();
        for b in &buckets {
            let pct = b.get("pct").and_then(|p| p.get("values"));
            let p = |k: &str| {
                pct.and_then(|v| v.get(k))
                    .and_then(|v| v.as_f64())
                    .filter(|f| f.is_finite())
                    .unwrap_or(0.0) as i64
            };
            out.push(HookStatsRow {
                event: b
                    .get("key")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                count: b.get("doc_count").and_then(|v| v.as_i64()).unwrap_or(0),
                error_count: b
                    .get("errs")
                    .and_then(|e| e.get("doc_count"))
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0),
                avg_duration_ms: b
                    .get("avg_dur")
                    .and_then(|a| a.get("value"))
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0),
                p50_duration_ms: p("50.0"),
                p99_duration_ms: p("99.0"),
            });
        }
        out.sort_by(|a, b| a.event.cmp(&b.event));
        Ok(out)
    }

    pub fn prune_hook_events(&self, cutoff_rfc3339: &str) -> IcmResult<usize> {
        let resp = self.post(
            &format!(
                "{IDX_HOOKS}/_delete_by_query?{}&conflicts=proceed",
                self.refresh_param()
            ),
            json!({"query": {"range": {"ts": {"lt": cutoff_rfc3339}}}}),
        )?;
        Ok(resp.get("deleted").and_then(|v| v.as_u64()).unwrap_or(0) as usize)
    }

    pub fn hook_event_count(&self) -> IcmResult<usize> {
        let resp = self.post(&format!("{IDX_HOOKS}/_count"), json!({}))?;
        Ok(resp.get("count").and_then(|v| v.as_u64()).unwrap_or(0) as usize)
    }
}
