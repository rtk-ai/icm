use crate::error::IcmResult;
use crate::memoir::{Concept, ConceptLink, Label, Memoir, MemoirStats, Relation};

pub trait MemoirStore {
    // --- Memoir CRUD ---
    fn create_memoir(&self, memoir: Memoir) -> IcmResult<String>;
    fn get_memoir(&self, id: &str) -> IcmResult<Option<Memoir>>;
    fn get_memoir_by_name(&self, name: &str) -> IcmResult<Option<Memoir>>;
    fn update_memoir(&self, memoir: &Memoir) -> IcmResult<()>;
    fn delete_memoir(&self, id: &str) -> IcmResult<()>;
    fn list_memoirs(&self) -> IcmResult<Vec<Memoir>>;

    // --- Concept CRUD ---
    fn add_concept(&self, concept: Concept) -> IcmResult<String>;
    fn get_concept(&self, id: &str) -> IcmResult<Option<Concept>>;
    fn get_concept_by_name(&self, memoir_id: &str, name: &str) -> IcmResult<Option<Concept>>;
    fn update_concept(&self, concept: &Concept) -> IcmResult<()>;
    fn delete_concept(&self, id: &str) -> IcmResult<()>;

    // --- Concept Search ---
    fn list_concepts(&self, memoir_id: &str) -> IcmResult<Vec<Concept>>;
    fn search_concepts_fts(
        &self,
        memoir_id: &str,
        query: &str,
        limit: usize,
    ) -> IcmResult<Vec<Concept>>;
    fn search_concepts_by_label(
        &self,
        memoir_id: &str,
        label: &Label,
        limit: usize,
    ) -> IcmResult<Vec<Concept>>;

    /// Search concepts across all memoirs via FTS.
    fn search_all_concepts_fts(&self, query: &str, limit: usize) -> IcmResult<Vec<Concept>>;

    // --- Refinement ---
    fn refine_concept(
        &self,
        id: &str,
        new_definition: &str,
        new_source_ids: &[String],
    ) -> IcmResult<()>;

    // --- Graph ---
    fn add_link(&self, link: ConceptLink) -> IcmResult<String>;
    fn get_links_from(&self, concept_id: &str) -> IcmResult<Vec<ConceptLink>>;
    fn get_links_to(&self, concept_id: &str) -> IcmResult<Vec<ConceptLink>>;
    fn delete_link(&self, id: &str) -> IcmResult<()>;

    /// Marks a link as no longer true without deleting it, so the graph
    /// keeps the history ("A depended on B until <date>"). Re-adding the
    /// same `(source, target, relation)` later revives it.
    fn invalidate_link(&self, id: &str) -> IcmResult<()> {
        let _ = id;
        Err(crate::IcmError::Unsupported(
            "memoir.invalidate_link".to_string(),
        ))
    }

    /// Every link touching a concept, invalidated ones included, oldest first.
    fn get_link_history(&self, concept_id: &str) -> IcmResult<Vec<ConceptLink>> {
        let _ = concept_id;
        Err(crate::IcmError::Unsupported(
            "memoir.get_link_history".to_string(),
        ))
    }
    fn get_neighbors(
        &self,
        concept_id: &str,
        relation: Option<Relation>,
    ) -> IcmResult<Vec<Concept>>;
    fn get_neighborhood(
        &self,
        concept_id: &str,
        depth: usize,
    ) -> IcmResult<(Vec<Concept>, Vec<ConceptLink>)>;

    /// Get all links for concepts belonging to a memoir (batch, avoids N+1).
    fn get_links_for_memoir(&self, memoir_id: &str) -> IcmResult<Vec<ConceptLink>>;

    // --- Stats ---
    fn memoir_stats(&self, memoir_id: &str) -> IcmResult<MemoirStats>;

    /// Get concept counts for all memoirs in a single query.
    fn batch_memoir_concept_counts(&self) -> IcmResult<std::collections::HashMap<String, usize>>;
}
