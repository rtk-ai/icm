//! OpenSearch backend -- split out of the former monolithic opensearch.rs.
//!
//! Neighbor expansion and pattern-mining helpers.

use super::*;

impl OpenSearchStore {
    pub fn get_many(&self, ids: &[&str]) -> IcmResult<HashMap<String, Memory>> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let resp = self.post(&format!("{IDX_MEMORIES}/_mget"), json!({"ids": ids}))?;
        let mut out = HashMap::new();
        if let Some(docs) = resp.get("docs").and_then(|d| d.as_array()) {
            for d in docs {
                if d.get("found").and_then(|f| f.as_bool()).unwrap_or(false)
                    && let (Some(id), Some(src)) =
                        (d.get("_id").and_then(|v| v.as_str()), d.get("_source"))
                {
                    out.insert(id.to_string(), Self::source_to_memory(id, src));
                }
            }
        }
        Ok(out)
    }

    pub fn get_by_topic_prefix(&self, topic: &str) -> IcmResult<Vec<Memory>> {
        let resp = self.post(
            &format!("{IDX_MEMORIES}/_search"),
            json!({
                "size": 500,
                "query": {"prefix": {"topic.keyword": topic}},
                "sort": [{"weight": "desc"}]
            }),
        )?;
        Ok(Self::hits_to_memories(&resp))
    }

    pub fn list_topics_with_prefix(&self, prefix: Option<&str>) -> IcmResult<Vec<(String, usize)>> {
        let mut topics = self.list_topics()?;
        if let Some(p) = prefix {
            topics.retain(|(t, _)| t.starts_with(p));
        }
        Ok(topics)
    }

    /// Expand a result set with graph neighbours (related ids), applying a
    /// hop discount. Pure logic over [`Self::get_many`]; identical to the
    /// other backends.
    pub fn expand_with_neighbors(
        &self,
        initial: &[(Memory, f32)],
        max_neighbors: usize,
        hop_discount: f32,
        max_total: usize,
    ) -> IcmResult<Vec<(Memory, f32)>> {
        if max_neighbors == 0 || initial.is_empty() {
            let mut out = initial.to_vec();
            out.truncate(max_total);
            return Ok(out);
        }
        let initial_ids: HashSet<String> = initial.iter().map(|(m, _)| m.id.clone()).collect();
        let mut candidates: Vec<(String, f32)> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for (m, score) in initial {
            for rid in &m.related_ids {
                if !initial_ids.contains(rid) && seen.insert(rid.clone()) {
                    candidates.push((rid.clone(), *score * hop_discount));
                    if candidates.len() >= max_neighbors {
                        break;
                    }
                }
            }
            if candidates.len() >= max_neighbors {
                break;
            }
        }

        let neighbor_ids: Vec<&str> = candidates.iter().map(|(id, _)| id.as_str()).collect();
        let fetched = self.get_many(&neighbor_ids)?;

        let mut combined: Vec<(Memory, f32)> = initial.to_vec();
        for (id, score) in candidates {
            if let Some(m) = fetched.get(&id) {
                combined.push((m.clone(), score));
            }
        }
        combined.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        combined.truncate(max_total);
        Ok(combined)
    }

    /// Pattern mining is not implemented on this backend yet.
    pub fn detect_patterns(
        &self,
        _topic: &str,
        _min_cluster_size: usize,
    ) -> IcmResult<Vec<PatternCluster>> {
        Err(IcmError::Unsupported("detect_patterns".into()))
    }

    /// See [`Self::detect_patterns`].
    pub fn extract_pattern_as_concept(
        &self,
        _cluster: &PatternCluster,
        _memoir_id: &str,
    ) -> IcmResult<String> {
        Err(IcmError::Unsupported("extract_pattern_as_concept".into()))
    }
}
