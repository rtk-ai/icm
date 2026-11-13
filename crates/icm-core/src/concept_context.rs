//! Graph context for a recall: the memoir concepts a query is about, with
//! their direct relations.
//!
//! A recall returns memories, which are flat text. When the same knowledge
//! also lives in a memoir, the concept carries what the memories do not:
//! typed relations to other concepts (`api --depends_on--> redis`,
//! `outage --caused_by--> deploy-v42`). Appending them lets the agent follow
//! the structure instead of reconstructing it from prose.

use std::collections::HashMap;

use crate::error::IcmResult;
use crate::memoir::{Concept, ConceptLink};
use crate::memoir_store::MemoirStore;

/// How many relations are shown per concept, outgoing and incoming together.
pub const MAX_LINKS_PER_CONCEPT: usize = 8;

/// One concept with its direct relations, ready to render.
#[derive(Debug, Clone)]
pub struct ConceptContext {
    pub memoir: String,
    pub concept: Concept,
    /// `(source name, relation, target name)` of active links, outgoing first.
    pub links: Vec<(String, String, String)>,
}

/// Up to `limit` concepts relevant to `query`, each with up to
/// [`MAX_LINKS_PER_CONCEPT`] active relations.
pub fn concept_context<S: MemoirStore + ?Sized>(
    store: &S,
    query: &str,
    limit: usize,
) -> IcmResult<Vec<ConceptContext>> {
    let concepts = store.rank_concepts(query, limit)?;
    let mut memoir_names: HashMap<String, String> = HashMap::new();
    let mut concept_names: HashMap<String, String> = HashMap::new();
    let mut out = Vec::with_capacity(concepts.len());

    for concept in concepts {
        let memoir = match memoir_names.get(&concept.memoir_id) {
            Some(n) => n.clone(),
            None => {
                let n = store
                    .get_memoir(&concept.memoir_id)?
                    .map(|m| m.name)
                    .unwrap_or_else(|| "?".to_string());
                memoir_names.insert(concept.memoir_id.clone(), n.clone());
                n
            }
        };
        concept_names.insert(concept.id.clone(), concept.name.clone());

        let mut raw: Vec<ConceptLink> = store.get_links_from(&concept.id)?;
        raw.extend(store.get_links_to(&concept.id)?);
        raw.truncate(MAX_LINKS_PER_CONCEPT);

        let mut links = Vec::with_capacity(raw.len());
        for l in raw {
            let src = name_of(store, &mut concept_names, &l.source_id)?;
            let tgt = name_of(store, &mut concept_names, &l.target_id)?;
            links.push((src, l.relation.to_string(), tgt));
        }
        out.push(ConceptContext {
            memoir,
            concept,
            links,
        });
    }
    Ok(out)
}

fn name_of<S: MemoirStore + ?Sized>(
    store: &S,
    cache: &mut HashMap<String, String>,
    id: &str,
) -> IcmResult<String> {
    if let Some(n) = cache.get(id) {
        return Ok(n.clone());
    }
    let n = store
        .get_concept(id)?
        .map(|c| c.name)
        .unwrap_or_else(|| "?".to_string());
    cache.insert(id.to_string(), n.clone());
    Ok(n)
}

/// Plain-text rendering, appended after the memories of a recall. Empty
/// string when there is nothing to show.
pub fn format_concept_context(items: &[ConceptContext]) -> String {
    if items.is_empty() {
        return String::new();
    }
    let mut out = String::from("\nRelated concepts (memoirs):\n");
    for item in items {
        let labels = item.concept.format_labels();
        let labels = if labels.is_empty() {
            String::new()
        } else {
            format!(" ({labels})")
        };
        out.push_str(&format!(
            "- [{}] {}{}: {}\n",
            item.memoir,
            item.concept.name,
            labels,
            one_line(&item.concept.definition)
        ));
        for (src, rel, tgt) in &item.links {
            out.push_str(&format!("    {src} --{rel}--> {tgt}\n"));
        }
    }
    out
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memoir::Label;

    #[test]
    fn formatting_lists_concepts_and_links() {
        let mut c = Concept::new("m".into(), "api".into(), "Public\nHTTP API".into());
        c.labels.push(Label::new("type", "service"));
        let items = vec![ConceptContext {
            memoir: "ops".into(),
            concept: c,
            links: vec![("api".into(), "depends_on".into(), "redis".into())],
        }];
        let text = format_concept_context(&items);
        assert!(
            text.contains("- [ops] api (type:service): Public HTTP API"),
            "{text}"
        );
        assert!(text.contains("    api --depends_on--> redis"), "{text}");
        assert_eq!(format_concept_context(&[]), "");
    }
}
