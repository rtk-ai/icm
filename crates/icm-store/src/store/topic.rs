//! SQLite backend — topic merge / rename.
//!
//! Renaming a project (or reorganizing topics) otherwise leaves the old
//! topic behind and splits memory across two topics; the only workaround
//! was hand-written SQL. These operations move every row that carries a
//! `topic` column (`memories`, `feedback`, `pending_consolidations`) in one
//! transaction.

use super::rows::summary_hash;
use super::*;

impl SqliteStore {
    /// Merge every memory in `from` into `into`.
    ///
    /// `summary_hash` is recomputed for every moved row because it encodes
    /// the topic: reusing the stored value would leave the dedup index
    /// stale and silently break future `INSERT OR IGNORE` dedup for those
    /// rows. `dedupe` controls what happens when a moved row's summary
    /// already exists in the target: with `dedupe == false` the whole
    /// operation aborts before any write; with `dedupe == true` the target
    /// row is kept and the source duplicate is deleted. `dry_run` performs
    /// the work and rolls it back, so the returned stats describe what a
    /// real run would do.
    pub fn merge_topics(
        &self,
        from: &str,
        into: &str,
        dedupe: bool,
        dry_run: bool,
    ) -> IcmResult<TopicMergeStats> {
        let from = from.trim();
        let into = into.trim();
        if from.is_empty() {
            return Err(IcmError::InvalidInput(
                "source topic cannot be empty".into(),
            ));
        }
        if into.is_empty() {
            return Err(IcmError::InvalidInput(
                "target topic cannot be empty".into(),
            ));
        }
        if from == into {
            return Err(IcmError::InvalidInput(
                "source and target topics must differ".into(),
            ));
        }
        if self.readonly {
            return Err(IcmError::ReadOnly("merge_topics".into()));
        }

        self.conn
            .execute_batch("BEGIN IMMEDIATE;")
            .map_err(db_err)?;

        match self.merge_topics_inner(from, into, dedupe) {
            Ok(stats) => {
                if dry_run {
                    // Rollback is the whole point of a dry run: the stats
                    // were computed against the real data, but nothing is
                    // persisted.
                    let _ = self.conn.execute_batch("ROLLBACK;");
                    Ok(stats)
                } else {
                    self.conn.execute_batch("COMMIT;").map_err(db_err)?;
                    self.cache_clear();
                    Ok(stats)
                }
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK;");
                Err(e)
            }
        }
    }

    /// Rename `old` to `new`. Unlike [`Self::merge_topics`], the target must
    /// not already exist — use `merge_topics` to combine two live topics.
    pub fn rename_topic(&self, old: &str, new: &str, dry_run: bool) -> IcmResult<TopicMergeStats> {
        let old = old.trim();
        let new = new.trim();
        if old.is_empty() {
            return Err(IcmError::InvalidInput(
                "source topic cannot be empty".into(),
            ));
        }
        if new.is_empty() {
            return Err(IcmError::InvalidInput(
                "target topic cannot be empty".into(),
            ));
        }
        if old == new {
            return Err(IcmError::InvalidInput(
                "source and target topics must differ".into(),
            ));
        }
        if self.readonly {
            return Err(IcmError::ReadOnly("rename_topic".into()));
        }

        if self.count_topic(new)? > 0 {
            return Err(IcmError::InvalidInput(format!(
                "target topic {new:?} already exists; use `topic merge` to combine topics"
            )));
        }

        self.merge_topics(old, new, false, dry_run)
    }

    /// Transactional core of [`Self::merge_topics`]. Caller owns the
    /// transaction and the rollback on error.
    fn merge_topics_inner(
        &self,
        from: &str,
        into: &str,
        dedupe: bool,
    ) -> IcmResult<TopicMergeStats> {
        let scanned = self.count_topic(from)?;
        if scanned == 0 {
            return Err(IcmError::NotFound(format!("topic not found: {from}")));
        }
        let target_before = self.count_topic(into)?;

        let target_hashes = self.topic_hashes(into)?;
        let (planned, collisions) = self.plan_topic_move(from, into, &target_hashes)?;
        // Abort before any write so the caller's data is untouched; the
        // count tells them whether `--dedupe` would resolve it.
        if collisions > 0 && !dedupe {
            return Err(IcmError::InvalidInput(format!(
                "merge would create {collisions} duplicate(s) in topic {into:?}; \
                 rerun with --dedupe to keep the existing target rows"
            )));
        }

        let (moved, duplicates_skipped, dedupe_ids) =
            self.apply_topic_move(into, planned, target_hashes)?;
        self.delete_deduped_memories(&dedupe_ids)?;
        self.move_topic_side_tables(from, into)?;

        Ok(TopicMergeStats {
            scanned,
            moved,
            duplicates_skipped,
            target_before,
            target_after: self.count_topic(into)?,
        })
    }

    fn count_topic(&self, topic: &str) -> IcmResult<usize> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM memories WHERE topic = ?1",
                params![topic],
                |row| row.get(0),
            )
            .map_err(db_err)
    }

    /// Hashes already present in `topic`. Legacy rows predate
    /// `summary_hash` (NULL), so theirs is computed from the summary —
    /// otherwise a duplicate could slip past the partial unique index.
    fn topic_hashes(&self, topic: &str) -> IcmResult<HashSet<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT summary_hash, summary FROM memories WHERE topic = ?1")
            .map_err(db_err)?;
        let rows = stmt
            .query_map(params![topic], |row| {
                Ok((row.get::<_, Option<String>>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(db_err)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(db_err)?;

        Ok(rows
            .into_iter()
            .map(|(stored, summary)| stored.unwrap_or_else(|| summary_hash(topic, &summary)))
            .collect())
    }

    /// Read the source rows and compute the hash each will carry in `into`.
    /// Also counts rows that would collide with the target or with each
    /// other, so the caller can abort before writing.
    fn plan_topic_move(
        &self,
        from: &str,
        into: &str,
        target_hashes: &HashSet<String>,
    ) -> IcmResult<(Vec<(String, String)>, usize)> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, summary FROM memories WHERE topic = ?1")
            .map_err(db_err)?;
        let rows = stmt
            .query_map(params![from], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(db_err)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(db_err)?;

        let mut planned = Vec::with_capacity(rows.len());
        let mut seen = HashSet::new();
        let mut collisions = 0usize;
        for (id, summary) in rows {
            let new_hash = summary_hash(into, &summary);
            if target_hashes.contains(&new_hash) || !seen.insert(new_hash.clone()) {
                collisions += 1;
            }
            planned.push((id, new_hash));
        }
        Ok((planned, collisions))
    }

    /// Write the planned moves. `claimed` starts as the target's hashes and
    /// grows with each move, so a duplicate is detected even when the
    /// source itself contains two rows with the same summary.
    fn apply_topic_move(
        &self,
        into: &str,
        planned: Vec<(String, String)>,
        mut claimed: HashSet<String>,
    ) -> IcmResult<(usize, usize, Vec<String>)> {
        let mut moved = 0usize;
        let mut duplicates_skipped = 0usize;
        let mut dedupe_ids = Vec::new();
        for (id, new_hash) in planned {
            if claimed.contains(&new_hash) {
                duplicates_skipped += 1;
                dedupe_ids.push(id);
                continue;
            }
            claimed.insert(new_hash.clone());
            self.conn
                .execute(
                    "UPDATE memories SET topic = ?1, summary_hash = ?2 WHERE id = ?3",
                    params![into, new_hash, id],
                )
                .map_err(db_err)?;
            moved += 1;
        }
        Ok((moved, duplicates_skipped, dedupe_ids))
    }

    /// Delete dropped duplicates and anything that references them, so no
    /// orphaned `vec_memories` rows or dangling `related_ids` survive —
    /// the same contract as `delete`.
    fn delete_deduped_memories(&self, ids: &[String]) -> IcmResult<()> {
        for id in ids {
            self.conn
                .execute("DELETE FROM vec_memories WHERE memory_id = ?1", params![id])
                .map_err(db_err)?;
            self.conn
                .execute("DELETE FROM memories WHERE id = ?1", params![id])
                .map_err(db_err)?;
            self.conn
                .execute(
                    "UPDATE memories
                        SET related_ids = (
                            SELECT COALESCE(json_group_array(value), '[]')
                            FROM json_each(memories.related_ids)
                            WHERE value != ?1
                        )
                        WHERE related_ids LIKE '%\"' || ?1 || '\"%'",
                    params![id],
                )
                .map_err(db_err)?;
        }
        Ok(())
    }

    /// `topic` is also a column on the feedback and consolidation-queue
    /// tables; move those so the topics stay consistent.
    fn move_topic_side_tables(&self, from: &str, into: &str) -> IcmResult<()> {
        self.conn
            .execute(
                "UPDATE feedback SET topic = ?1 WHERE topic = ?2",
                params![into, from],
            )
            .map_err(db_err)?;
        self.conn
            .execute(
                "UPDATE pending_consolidations SET topic = ?1 WHERE topic = ?2",
                params![into, from],
            )
            .map_err(db_err)?;
        Ok(())
    }
}
