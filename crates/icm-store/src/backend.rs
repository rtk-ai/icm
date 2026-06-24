//! Runtime-dispatched storage backend (issue #301).
//!
//! [`Store`] is an enum over the compiled-in backends. The active backend
//! is chosen at runtime from `ICM_DB_BACKEND` (`sqlite` (default) /
//! `postgres` / `opensearch`), mirroring SurrealDB's `Surreal<Any>`: a
//! single binary can carry every backend and pick one without a rebuild.
//! Cargo features only decide which variants are available.

use std::collections::HashMap;
use std::path::Path;

use chrono::{DateTime, Utc};

use icm_core::{
    Concept, ConceptLink, Embedder, Fact, FactsStats, FactsStore, Feedback, FeedbackStats,
    FeedbackStore, IcmError, IcmResult, Label, Memoir, MemoirStats, MemoirStore, Memory,
    MemoryStore, Message, PatternCluster, Relation, Role, Session, StoreStats, TopicHealth,
    TranscriptHit, TranscriptStats, TranscriptStore,
};

use crate::common::{CodeArea, HookEvent, HookEventInsert, HookStatsRow, PendingRow};

#[cfg(feature = "backend-sqlite")]
use crate::store::SqliteStore;

#[cfg(feature = "postgres")]
use crate::postgres::PostgresStore;

#[cfg(feature = "opensearch")]
use crate::opensearch::OpenSearchStore;

/// Which storage backend is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    Sqlite,
    Postgres,
    OpenSearch,
}

impl BackendKind {
    /// Resolve the requested backend from `ICM_DB_BACKEND` (default
    /// `sqlite`). Unknown values are a config error.
    pub fn from_env() -> IcmResult<Self> {
        match std::env::var("ICM_DB_BACKEND")
            .ok()
            .as_deref()
            .map(str::trim)
        {
            None | Some("") | Some("sqlite") => Ok(BackendKind::Sqlite),
            Some("postgres") | Some("postgresql") | Some("pg") => Ok(BackendKind::Postgres),
            Some("opensearch") | Some("os") => Ok(BackendKind::OpenSearch),
            Some(other) => Err(IcmError::Config(format!(
                "unknown ICM_DB_BACKEND '{other}' (expected: sqlite, postgres, opensearch)"
            ))),
        }
    }
}

/// A storage backend chosen at runtime. Every method forwards to the
/// active variant.
pub enum Store {
    #[cfg(feature = "backend-sqlite")]
    Sqlite(SqliteStore),
    #[cfg(feature = "postgres")]
    Postgres(PostgresStore),
    #[cfg(feature = "opensearch")]
    OpenSearch(OpenSearchStore),
}

/// Forward a method call to the active backend variant.
macro_rules! dispatch {
    ($self:expr, $m:ident ( $($a:expr),* )) => {
        match $self {
            #[cfg(feature = "backend-sqlite")]
            Store::Sqlite(s) => s.$m($($a),*),
            #[cfg(feature = "postgres")]
            Store::Postgres(s) => s.$m($($a),*),
            #[cfg(feature = "opensearch")]
            Store::OpenSearch(s) => s.$m($($a),*),
        }
    };
}

/// Error for a backend selected at runtime but not compiled into the binary.
/// Only used in builds where some backend feature is disabled.
#[allow(dead_code)]
fn not_compiled(name: &str) -> IcmError {
    IcmError::Config(format!(
        "the '{name}' backend was requested (ICM_DB_BACKEND) but this build was \
         not compiled with its Cargo feature"
    ))
}

impl Store {
    // --- Constructors (select the backend, then build that variant) ---

    /// Open or create the active backend with the default embedding dim.
    pub fn new(path: &Path) -> IcmResult<Self> {
        Self::with_dims(path, icm_core::DEFAULT_EMBEDDING_DIMS)
    }

    /// Open or create the active backend with a specific embedding dim.
    pub fn with_dims(path: &Path, embedding_dims: usize) -> IcmResult<Self> {
        match BackendKind::from_env()? {
            BackendKind::Sqlite => {
                #[cfg(feature = "backend-sqlite")]
                {
                    Ok(Store::Sqlite(SqliteStore::with_dims(path, embedding_dims)?))
                }
                #[cfg(not(feature = "backend-sqlite"))]
                {
                    let _ = (path, embedding_dims);
                    Err(not_compiled("sqlite"))
                }
            }
            BackendKind::Postgres => {
                #[cfg(feature = "postgres")]
                {
                    Ok(Store::Postgres(PostgresStore::with_dims(
                        path,
                        embedding_dims,
                    )?))
                }
                #[cfg(not(feature = "postgres"))]
                {
                    let _ = (path, embedding_dims);
                    Err(not_compiled("postgres"))
                }
            }
            BackendKind::OpenSearch => {
                #[cfg(feature = "opensearch")]
                {
                    Ok(Store::OpenSearch(OpenSearchStore::with_dims(
                        path,
                        embedding_dims,
                    )?))
                }
                #[cfg(not(feature = "opensearch"))]
                {
                    let _ = (path, embedding_dims);
                    Err(not_compiled("opensearch"))
                }
            }
        }
    }

    /// Open the active backend read-only (issue #263).
    pub fn open_readonly(path: &Path) -> IcmResult<Self> {
        match BackendKind::from_env()? {
            BackendKind::Sqlite => {
                #[cfg(feature = "backend-sqlite")]
                {
                    Ok(Store::Sqlite(SqliteStore::open_readonly(path)?))
                }
                #[cfg(not(feature = "backend-sqlite"))]
                {
                    let _ = path;
                    Err(not_compiled("sqlite"))
                }
            }
            BackendKind::Postgres => {
                #[cfg(feature = "postgres")]
                {
                    Ok(Store::Postgres(PostgresStore::open_readonly(path)?))
                }
                #[cfg(not(feature = "postgres"))]
                {
                    let _ = path;
                    Err(not_compiled("postgres"))
                }
            }
            BackendKind::OpenSearch => {
                #[cfg(feature = "opensearch")]
                {
                    Ok(Store::OpenSearch(OpenSearchStore::open_readonly(path)?))
                }
                #[cfg(not(feature = "opensearch"))]
                {
                    let _ = path;
                    Err(not_compiled("opensearch"))
                }
            }
        }
    }

    /// In-memory store for the active backend (remote backends connect to
    /// their configured endpoint).
    pub fn in_memory() -> IcmResult<Self> {
        Self::in_memory_with_dims(icm_core::DEFAULT_EMBEDDING_DIMS)
    }

    /// See [`Self::in_memory`].
    pub fn in_memory_with_dims(embedding_dims: usize) -> IcmResult<Self> {
        match BackendKind::from_env()? {
            BackendKind::Sqlite => {
                #[cfg(feature = "backend-sqlite")]
                {
                    Ok(Store::Sqlite(SqliteStore::in_memory_with_dims(
                        embedding_dims,
                    )?))
                }
                #[cfg(not(feature = "backend-sqlite"))]
                {
                    let _ = embedding_dims;
                    Err(not_compiled("sqlite"))
                }
            }
            BackendKind::Postgres => {
                #[cfg(feature = "postgres")]
                {
                    Ok(Store::Postgres(PostgresStore::in_memory_with_dims(
                        embedding_dims,
                    )?))
                }
                #[cfg(not(feature = "postgres"))]
                {
                    let _ = embedding_dims;
                    Err(not_compiled("postgres"))
                }
            }
            BackendKind::OpenSearch => {
                #[cfg(feature = "opensearch")]
                {
                    Ok(Store::OpenSearch(OpenSearchStore::in_memory_with_dims(
                        embedding_dims,
                    )?))
                }
                #[cfg(not(feature = "opensearch"))]
                {
                    let _ = embedding_dims;
                    Err(not_compiled("opensearch"))
                }
            }
        }
    }

    /// Peek the stored embedding dim for the active backend (no full open).
    pub fn read_stored_embedding_dims(path: &Path) -> IcmResult<Option<usize>> {
        match BackendKind::from_env()? {
            BackendKind::Sqlite => {
                #[cfg(feature = "backend-sqlite")]
                {
                    SqliteStore::read_stored_embedding_dims(path)
                }
                #[cfg(not(feature = "backend-sqlite"))]
                {
                    let _ = path;
                    Ok(None)
                }
            }
            BackendKind::Postgres => {
                #[cfg(feature = "postgres")]
                {
                    PostgresStore::read_stored_embedding_dims(path)
                }
                #[cfg(not(feature = "postgres"))]
                {
                    let _ = path;
                    Ok(None)
                }
            }
            BackendKind::OpenSearch => {
                #[cfg(feature = "opensearch")]
                {
                    OpenSearchStore::read_stored_embedding_dims(path)
                }
                #[cfg(not(feature = "opensearch"))]
                {
                    let _ = path;
                    Ok(None)
                }
            }
        }
    }

    /// Whether the active store was opened read-only.
    pub fn is_readonly(&self) -> bool {
        dispatch!(self, is_readonly())
    }

    // --- Inherent store/recall/hook surface (forwarded) ---

    pub fn maybe_auto_decay(&self) -> IcmResult<()> {
        dispatch!(self, maybe_auto_decay())
    }
    pub fn increment_hook_counter(&self) -> IcmResult<usize> {
        dispatch!(self, increment_hook_counter())
    }
    pub fn reset_hook_counter(&self) -> IcmResult<()> {
        dispatch!(self, reset_hook_counter())
    }
    pub fn enqueue_pending_extraction(
        &self,
        project: &str,
        tool_name: &str,
        raw_output: &str,
    ) -> IcmResult<String> {
        dispatch!(
            self,
            enqueue_pending_extraction(project, tool_name, raw_output)
        )
    }
    pub fn list_pending_extractions(&self, limit: usize) -> IcmResult<Vec<PendingRow>> {
        dispatch!(self, list_pending_extractions(limit))
    }
    pub fn delete_pending_extractions(&self, ids: &[String]) -> IcmResult<usize> {
        dispatch!(self, delete_pending_extractions(ids))
    }
    pub fn pending_extraction_count(&self) -> IcmResult<usize> {
        dispatch!(self, pending_extraction_count())
    }
    pub fn upsert_code_area(
        &self,
        project: &str,
        file_path: &str,
        description: Option<&str>,
        session_id: Option<&str>,
        tool_name: Option<&str>,
    ) -> IcmResult<()> {
        dispatch!(
            self,
            upsert_code_area(project, file_path, description, session_id, tool_name)
        )
    }
    pub fn list_code_areas(
        &self,
        project: Option<&str>,
        in_file: Option<&str>,
        since: Option<DateTime<Utc>>,
        limit: usize,
    ) -> IcmResult<Vec<CodeArea>> {
        dispatch!(self, list_code_areas(project, in_file, since, limit))
    }
    pub fn code_area_count(&self) -> IcmResult<usize> {
        dispatch!(self, code_area_count())
    }
    pub fn record_hook_event(&self, ev: &HookEventInsert) -> IcmResult<i64> {
        dispatch!(self, record_hook_event(ev))
    }
    pub fn hook_events_recent(
        &self,
        limit: usize,
        event_filter: Option<&str>,
    ) -> IcmResult<Vec<HookEvent>> {
        dispatch!(self, hook_events_recent(limit, event_filter))
    }
    pub fn hook_stats(&self, since_rfc3339: &str) -> IcmResult<Vec<HookStatsRow>> {
        dispatch!(self, hook_stats(since_rfc3339))
    }
    pub fn prune_hook_events(&self, cutoff_rfc3339: &str) -> IcmResult<usize> {
        dispatch!(self, prune_hook_events(cutoff_rfc3339))
    }
    pub fn hook_event_count(&self) -> IcmResult<usize> {
        dispatch!(self, hook_event_count())
    }
    pub fn auto_consolidate(&self, topic: &str, threshold: usize) -> IcmResult<bool> {
        dispatch!(self, auto_consolidate(topic, threshold))
    }
    pub fn auto_consolidate_with_embedder(
        &self,
        topic: &str,
        threshold: usize,
        embedder: Option<&dyn Embedder>,
    ) -> IcmResult<bool> {
        dispatch!(
            self,
            auto_consolidate_with_embedder(topic, threshold, embedder)
        )
    }
    pub fn expand_with_neighbors(
        &self,
        initial: &[(Memory, f32)],
        max_neighbors: usize,
        hop_discount: f32,
        max_total: usize,
    ) -> IcmResult<Vec<(Memory, f32)>> {
        dispatch!(
            self,
            expand_with_neighbors(initial, max_neighbors, hop_discount, max_total)
        )
    }
    pub fn get_many(&self, ids: &[&str]) -> IcmResult<HashMap<String, Memory>> {
        dispatch!(self, get_many(ids))
    }
    pub fn get_by_topic_prefix(&self, topic: &str) -> IcmResult<Vec<Memory>> {
        dispatch!(self, get_by_topic_prefix(topic))
    }
    pub fn list_topics_with_prefix(&self, prefix: Option<&str>) -> IcmResult<Vec<(String, usize)>> {
        dispatch!(self, list_topics_with_prefix(prefix))
    }
    pub fn detect_patterns(
        &self,
        topic: &str,
        min_cluster_size: usize,
    ) -> IcmResult<Vec<PatternCluster>> {
        dispatch!(self, detect_patterns(topic, min_cluster_size))
    }
    pub fn extract_pattern_as_concept(
        &self,
        cluster: &PatternCluster,
        memoir_id: &str,
    ) -> IcmResult<String> {
        dispatch!(self, extract_pattern_as_concept(cluster, memoir_id))
    }
}

impl MemoryStore for Store {
    fn store(&self, memory: Memory) -> IcmResult<String> {
        dispatch!(self, store(memory))
    }
    fn get(&self, id: &str) -> IcmResult<Option<Memory>> {
        dispatch!(self, get(id))
    }
    fn update(&self, memory: &Memory) -> IcmResult<()> {
        dispatch!(self, update(memory))
    }
    fn delete(&self, id: &str) -> IcmResult<()> {
        dispatch!(self, delete(id))
    }
    fn search_by_keywords(&self, keywords: &[&str], limit: usize) -> IcmResult<Vec<Memory>> {
        dispatch!(self, search_by_keywords(keywords, limit))
    }
    fn search_fts(&self, query: &str, limit: usize) -> IcmResult<Vec<Memory>> {
        dispatch!(self, search_fts(query, limit))
    }
    fn search_by_embedding(
        &self,
        embedding: &[f32],
        limit: usize,
    ) -> IcmResult<Vec<(Memory, f32)>> {
        dispatch!(self, search_by_embedding(embedding, limit))
    }
    fn search_hybrid(
        &self,
        query: &str,
        embedding: &[f32],
        limit: usize,
    ) -> IcmResult<Vec<(Memory, f32)>> {
        dispatch!(self, search_hybrid(query, embedding, limit))
    }
    fn update_access(&self, id: &str) -> IcmResult<()> {
        dispatch!(self, update_access(id))
    }
    fn batch_update_access(&self, ids: &[&str]) -> IcmResult<usize> {
        dispatch!(self, batch_update_access(ids))
    }
    fn apply_decay(&self, decay_factor: f32) -> IcmResult<usize> {
        dispatch!(self, apply_decay(decay_factor))
    }
    fn prune(&self, weight_threshold: f32) -> IcmResult<usize> {
        dispatch!(self, prune(weight_threshold))
    }
    fn list_all(&self) -> IcmResult<Vec<Memory>> {
        dispatch!(self, list_all())
    }
    fn get_by_topic(&self, topic: &str) -> IcmResult<Vec<Memory>> {
        dispatch!(self, get_by_topic(topic))
    }
    fn list_topics(&self) -> IcmResult<Vec<(String, usize)>> {
        dispatch!(self, list_topics())
    }
    fn consolidate_topic(&self, topic: &str, consolidated: Memory) -> IcmResult<()> {
        dispatch!(self, consolidate_topic(topic, consolidated))
    }
    fn count(&self) -> IcmResult<usize> {
        dispatch!(self, count())
    }
    fn count_by_topic(&self, topic: &str) -> IcmResult<usize> {
        dispatch!(self, count_by_topic(topic))
    }
    fn stats(&self) -> IcmResult<StoreStats> {
        dispatch!(self, stats())
    }
    fn topic_health(&self, topic: &str) -> IcmResult<TopicHealth> {
        dispatch!(self, topic_health(topic))
    }
}

impl MemoirStore for Store {
    fn create_memoir(&self, memoir: Memoir) -> IcmResult<String> {
        dispatch!(self, create_memoir(memoir))
    }
    fn get_memoir(&self, id: &str) -> IcmResult<Option<Memoir>> {
        dispatch!(self, get_memoir(id))
    }
    fn get_memoir_by_name(&self, name: &str) -> IcmResult<Option<Memoir>> {
        dispatch!(self, get_memoir_by_name(name))
    }
    fn update_memoir(&self, memoir: &Memoir) -> IcmResult<()> {
        dispatch!(self, update_memoir(memoir))
    }
    fn delete_memoir(&self, id: &str) -> IcmResult<()> {
        dispatch!(self, delete_memoir(id))
    }
    fn list_memoirs(&self) -> IcmResult<Vec<Memoir>> {
        dispatch!(self, list_memoirs())
    }
    fn add_concept(&self, concept: Concept) -> IcmResult<String> {
        dispatch!(self, add_concept(concept))
    }
    fn get_concept(&self, id: &str) -> IcmResult<Option<Concept>> {
        dispatch!(self, get_concept(id))
    }
    fn get_concept_by_name(&self, memoir_id: &str, name: &str) -> IcmResult<Option<Concept>> {
        dispatch!(self, get_concept_by_name(memoir_id, name))
    }
    fn update_concept(&self, concept: &Concept) -> IcmResult<()> {
        dispatch!(self, update_concept(concept))
    }
    fn delete_concept(&self, id: &str) -> IcmResult<()> {
        dispatch!(self, delete_concept(id))
    }
    fn list_concepts(&self, memoir_id: &str) -> IcmResult<Vec<Concept>> {
        dispatch!(self, list_concepts(memoir_id))
    }
    fn search_concepts_fts(
        &self,
        memoir_id: &str,
        query: &str,
        limit: usize,
    ) -> IcmResult<Vec<Concept>> {
        dispatch!(self, search_concepts_fts(memoir_id, query, limit))
    }
    fn search_concepts_by_label(
        &self,
        memoir_id: &str,
        label: &Label,
        limit: usize,
    ) -> IcmResult<Vec<Concept>> {
        dispatch!(self, search_concepts_by_label(memoir_id, label, limit))
    }
    fn search_all_concepts_fts(&self, query: &str, limit: usize) -> IcmResult<Vec<Concept>> {
        dispatch!(self, search_all_concepts_fts(query, limit))
    }
    fn refine_concept(
        &self,
        id: &str,
        new_definition: &str,
        new_source_ids: &[String],
    ) -> IcmResult<()> {
        dispatch!(self, refine_concept(id, new_definition, new_source_ids))
    }
    fn add_link(&self, link: ConceptLink) -> IcmResult<String> {
        dispatch!(self, add_link(link))
    }
    fn get_links_from(&self, concept_id: &str) -> IcmResult<Vec<ConceptLink>> {
        dispatch!(self, get_links_from(concept_id))
    }
    fn get_links_to(&self, concept_id: &str) -> IcmResult<Vec<ConceptLink>> {
        dispatch!(self, get_links_to(concept_id))
    }
    fn delete_link(&self, id: &str) -> IcmResult<()> {
        dispatch!(self, delete_link(id))
    }
    fn get_neighbors(
        &self,
        concept_id: &str,
        relation: Option<Relation>,
    ) -> IcmResult<Vec<Concept>> {
        dispatch!(self, get_neighbors(concept_id, relation))
    }
    fn get_neighborhood(
        &self,
        concept_id: &str,
        depth: usize,
    ) -> IcmResult<(Vec<Concept>, Vec<ConceptLink>)> {
        dispatch!(self, get_neighborhood(concept_id, depth))
    }
    fn get_links_for_memoir(&self, memoir_id: &str) -> IcmResult<Vec<ConceptLink>> {
        dispatch!(self, get_links_for_memoir(memoir_id))
    }
    fn memoir_stats(&self, memoir_id: &str) -> IcmResult<MemoirStats> {
        dispatch!(self, memoir_stats(memoir_id))
    }
    fn batch_memoir_concept_counts(&self) -> IcmResult<HashMap<String, usize>> {
        dispatch!(self, batch_memoir_concept_counts())
    }
}

impl FeedbackStore for Store {
    fn store_feedback(&self, feedback: Feedback) -> IcmResult<String> {
        dispatch!(self, store_feedback(feedback))
    }
    fn search_feedback(
        &self,
        query: &str,
        topic: Option<&str>,
        limit: usize,
    ) -> IcmResult<Vec<Feedback>> {
        dispatch!(self, search_feedback(query, topic, limit))
    }
    fn list_feedback(&self, topic: Option<&str>, limit: usize) -> IcmResult<Vec<Feedback>> {
        dispatch!(self, list_feedback(topic, limit))
    }
    fn increment_applied(&self, id: &str) -> IcmResult<()> {
        dispatch!(self, increment_applied(id))
    }
    fn delete_feedback(&self, id: &str) -> IcmResult<()> {
        dispatch!(self, delete_feedback(id))
    }
    fn feedback_stats(&self) -> IcmResult<FeedbackStats> {
        dispatch!(self, feedback_stats())
    }
}

impl FactsStore for Store {
    fn set_fact(&self, entity: &str, key: &str, value: &str, source: &str) -> IcmResult<String> {
        dispatch!(self, set_fact(entity, key, value, source))
    }
    fn get_fact(&self, entity: &str, key: &str) -> IcmResult<Option<Fact>> {
        dispatch!(self, get_fact(entity, key))
    }
    fn list_facts(&self, entity: &str, key_prefix: Option<&str>) -> IcmResult<Vec<Fact>> {
        dispatch!(self, list_facts(entity, key_prefix))
    }
    fn history(&self, entity: &str, key: &str) -> IcmResult<Vec<Fact>> {
        dispatch!(self, history(entity, key))
    }
    fn forget_fact(&self, entity: &str, key: &str) -> IcmResult<usize> {
        dispatch!(self, forget_fact(entity, key))
    }
    fn facts_stats(&self) -> IcmResult<FactsStats> {
        dispatch!(self, facts_stats())
    }
}

impl TranscriptStore for Store {
    fn create_session(
        &self,
        agent: &str,
        project: Option<&str>,
        metadata: Option<&str>,
    ) -> IcmResult<String> {
        dispatch!(self, create_session(agent, project, metadata))
    }
    fn ensure_session(
        &self,
        id: &str,
        agent: &str,
        project: Option<&str>,
        metadata: Option<&str>,
    ) -> IcmResult<String> {
        dispatch!(self, ensure_session(id, agent, project, metadata))
    }
    fn get_session(&self, id: &str) -> IcmResult<Option<Session>> {
        dispatch!(self, get_session(id))
    }
    fn list_sessions(&self, project: Option<&str>, limit: usize) -> IcmResult<Vec<Session>> {
        dispatch!(self, list_sessions(project, limit))
    }
    fn record_message(
        &self,
        session_id: &str,
        role: Role,
        content: &str,
        tool_name: Option<&str>,
        tokens: Option<i64>,
        metadata: Option<&str>,
    ) -> IcmResult<String> {
        dispatch!(
            self,
            record_message(session_id, role, content, tool_name, tokens, metadata)
        )
    }
    fn list_session_messages(
        &self,
        session_id: &str,
        limit: usize,
        offset: usize,
    ) -> IcmResult<Vec<Message>> {
        dispatch!(self, list_session_messages(session_id, limit, offset))
    }
    fn search_transcripts(
        &self,
        query: &str,
        session_id: Option<&str>,
        project: Option<&str>,
        limit: usize,
    ) -> IcmResult<Vec<TranscriptHit>> {
        dispatch!(self, search_transcripts(query, session_id, project, limit))
    }
    fn forget_session(&self, id: &str) -> IcmResult<()> {
        dispatch!(self, forget_session(id))
    }
    fn transcript_stats(&self) -> IcmResult<TranscriptStats> {
        dispatch!(self, transcript_stats())
    }
}
