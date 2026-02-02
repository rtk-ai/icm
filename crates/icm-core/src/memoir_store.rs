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

    // --- Stats ---
    fn memoir_stats(&self, memoir_id: &str) -> IcmResult<MemoirStats>;
}
