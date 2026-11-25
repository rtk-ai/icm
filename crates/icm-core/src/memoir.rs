use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

// ---------------------------------------------------------------------------
// Memoir
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memoir {
    pub id: String,
    pub name: String,
    pub description: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub consolidation_threshold: u32,
    /// Optional schema the memoir's concepts and links must follow. `None`
    /// keeps the historical free-form behaviour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ontology: Option<Ontology>,
}

impl Memoir {
    pub fn new(name: String, description: String) -> Self {
        let now = Utc::now();
        Self {
            id: ulid::Ulid::generate().to_string(),
            name,
            description,
            created_at: now,
            updated_at: now,
            consolidation_threshold: 50,
            ontology: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Ontology
// ---------------------------------------------------------------------------

/// Label namespace that carries a concept's type (`type:service`).
pub const TYPE_NAMESPACE: &str = "type";

/// A lightweight, per-memoir schema: which concept types exist and which
/// relations may connect them.
///
/// The point is a shared vocabulary. Several agents writing into one memoir
/// otherwise each invent their own types (`svc`, `service`, `microservice`)
/// and their own reading of a relation, and the graph stops answering
/// questions reliably. This is the practical subset of an OWL/SHACL stack:
/// a closed type set and a domain/range per relation, checked on write.
/// There is no inference engine.
///
/// A concept's type is its `type:<name>` label.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Ontology {
    /// Allowed concept types. Empty means any type is accepted.
    #[serde(default)]
    pub types: Vec<String>,
    /// Every concept must carry a `type:` label.
    #[serde(default)]
    pub require_type: bool,
    /// Allowed relations with their domain/range. Empty means any relation
    /// between any types.
    #[serde(default)]
    pub relations: Vec<RelationRule>,
}

/// One allowed edge shape: `from -relation-> to`. `None` on either side
/// accepts any type, untyped concepts included.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelationRule {
    pub relation: Relation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
}

impl std::str::FromStr for RelationRule {
    type Err = String;

    /// Parses `relation` or `from:relation:to`, with `*` for "any type":
    /// `incident:caused_by:deploy`, `*:depends_on:service`, `refines`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let side = |t: &str| {
            let t = t.trim();
            (t != "*" && !t.is_empty()).then(|| t.to_string())
        };
        match s.trim().split(':').collect::<Vec<_>>().as_slice() {
            [rel] => Ok(Self {
                relation: rel.trim().parse()?,
                from: None,
                to: None,
            }),
            [from, rel, to] => Ok(Self {
                relation: rel.trim().parse()?,
                from: side(from),
                to: side(to),
            }),
            _ => Err(format!(
                "invalid relation rule '{s}': expected 'relation' or 'from:relation:to'"
            )),
        }
    }
}

impl fmt::Display for RelationRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let side = |t: &Option<String>| t.as_deref().unwrap_or("*").to_string();
        write!(
            f,
            "{}:{}:{}",
            side(&self.from),
            self.relation,
            side(&self.to)
        )
    }
}

/// The type of a concept: the value of its first `type:` label, if any.
pub fn concept_type(labels: &[Label]) -> Option<&str> {
    labels
        .iter()
        .find(|l| l.namespace == TYPE_NAMESPACE)
        .map(|l| l.value.as_str())
}

impl Ontology {
    /// True when the ontology constrains nothing.
    pub fn is_empty(&self) -> bool {
        self.types.is_empty() && !self.require_type && self.relations.is_empty()
    }

    /// Checks a concept's `type:` labels against the type rules.
    pub fn validate_concept(&self, concept: &Concept) -> Result<(), String> {
        let types: Vec<&str> = concept
            .labels
            .iter()
            .filter(|l| l.namespace == TYPE_NAMESPACE)
            .map(|l| l.value.as_str())
            .collect();
        if types.len() > 1 {
            return Err(format!(
                "concept '{}' has {} type labels ({}); the ontology allows one",
                concept.name,
                types.len(),
                types.join(", ")
            ));
        }
        match types.first() {
            None if self.require_type => Err(format!(
                "concept '{}' has no type label; this memoir requires one of: {}",
                concept.name,
                self.type_list()
            )),
            Some(t) if !self.types.is_empty() && !self.types.iter().any(|a| a == t) => {
                Err(format!(
                    "concept '{}' has unknown type '{t}'; allowed types: {}",
                    concept.name,
                    self.type_list()
                ))
            }
            _ => Ok(()),
        }
    }

    /// Checks that `source -relation-> target` matches a declared rule.
    pub fn validate_link(
        &self,
        relation: Relation,
        source: &Concept,
        target: &Concept,
    ) -> Result<(), String> {
        if self.relations.is_empty() {
            return Ok(());
        }
        let (st, tt) = (concept_type(&source.labels), concept_type(&target.labels));
        let side_ok = |rule: &Option<String>, actual: Option<&str>| match rule {
            None => true,
            Some(r) => actual == Some(r.as_str()),
        };
        if self
            .relations
            .iter()
            .any(|r| r.relation == relation && side_ok(&r.from, st) && side_ok(&r.to, tt))
        {
            return Ok(());
        }
        let allowed: Vec<String> = self
            .relations
            .iter()
            .filter(|r| r.relation == relation)
            .map(|r| r.to_string())
            .collect();
        let shape = format!(
            "{} -{relation}-> {}",
            st.unwrap_or("(untyped)"),
            tt.unwrap_or("(untyped)")
        );
        Err(if allowed.is_empty() {
            format!("relation '{relation}' is not declared in this memoir's ontology ({shape})")
        } else {
            format!(
                "link {shape} ('{}' -> '{}') does not match the ontology; allowed for '{relation}': {}",
                source.name,
                target.name,
                allowed.join(", ")
            )
        })
    }

    fn type_list(&self) -> String {
        if self.types.is_empty() {
            "(any)".to_string()
        } else {
            self.types.join(", ")
        }
    }
}

// ---------------------------------------------------------------------------
// Label
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Label {
    pub namespace: String,
    pub value: String,
}

impl Label {
    pub fn new(namespace: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            value: value.into(),
        }
    }
}

impl fmt::Display for Label {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.namespace, self.value)
    }
}

impl std::str::FromStr for Label {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some((ns, val)) = s.split_once(':') {
            Ok(Self {
                namespace: ns.to_string(),
                value: val.to_string(),
            })
        } else {
            Ok(Self {
                namespace: "tag".to_string(),
                value: s.to_string(),
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Concept
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Concept {
    pub id: String,
    pub memoir_id: String,
    pub name: String,
    pub definition: String,
    pub labels: Vec<Label>,
    pub confidence: f32,
    pub revision: u32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub source_memory_ids: Vec<String>,
}

impl Concept {
    pub fn format_labels(&self) -> String {
        self.labels
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub fn confidence_color(&self) -> &'static str {
        if self.confidence >= 0.8 {
            "#4CAF50"
        } else if self.confidence >= 0.5 {
            "#FFC107"
        } else if self.confidence >= 0.3 {
            "#FF9800"
        } else {
            "#F44336"
        }
    }

    pub fn confidence_bar(&self) -> String {
        let clamped = self.confidence.clamp(0.0, 1.0);
        let filled = (clamped * 5.0).round() as usize;
        let empty = 5 - filled;
        format!("{}{}", "#".repeat(filled), ".".repeat(empty))
    }

    pub fn new(memoir_id: String, name: String, definition: String) -> Self {
        let now = Utc::now();
        Self {
            id: ulid::Ulid::generate().to_string(),
            memoir_id,
            name,
            definition,
            labels: Vec::new(),
            confidence: 0.5,
            revision: 1,
            created_at: now,
            updated_at: now,
            source_memory_ids: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Relation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Relation {
    PartOf,
    DependsOn,
    RelatedTo,
    Contradicts,
    Refines,
    AlternativeTo,
    CausedBy,
    InstanceOf,
    SupersededBy,
}

impl fmt::Display for Relation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PartOf => write!(f, "part_of"),
            Self::DependsOn => write!(f, "depends_on"),
            Self::RelatedTo => write!(f, "related_to"),
            Self::Contradicts => write!(f, "contradicts"),
            Self::Refines => write!(f, "refines"),
            Self::AlternativeTo => write!(f, "alternative_to"),
            Self::CausedBy => write!(f, "caused_by"),
            Self::InstanceOf => write!(f, "instance_of"),
            Self::SupersededBy => write!(f, "superseded_by"),
        }
    }
}

impl std::str::FromStr for Relation {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "part_of" | "partof" => Ok(Self::PartOf),
            "depends_on" | "dependson" => Ok(Self::DependsOn),
            "related_to" | "relatedto" => Ok(Self::RelatedTo),
            "contradicts" => Ok(Self::Contradicts),
            "refines" => Ok(Self::Refines),
            "alternative_to" | "alternativeto" => Ok(Self::AlternativeTo),
            "caused_by" | "causedby" => Ok(Self::CausedBy),
            "instance_of" | "instanceof" => Ok(Self::InstanceOf),
            "superseded_by" | "supersededby" => Ok(Self::SupersededBy),
            _ => Err(format!("invalid relation: {s}")),
        }
    }
}

// ---------------------------------------------------------------------------
// ConceptLink
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConceptLink {
    pub id: String,
    pub source_id: String,
    pub target_id: String,
    pub relation: Relation,
    pub weight: f32,
    pub created_at: DateTime<Utc>,
}

impl ConceptLink {
    pub fn new(source_id: String, target_id: String, relation: Relation) -> Self {
        Self {
            id: ulid::Ulid::generate().to_string(),
            source_id,
            target_id,
            relation,
            weight: 1.0,
            created_at: Utc::now(),
        }
    }
}

// ---------------------------------------------------------------------------
// MemoirStats
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct MemoirStats {
    pub total_concepts: usize,
    pub total_links: usize,
    pub avg_confidence: f32,
    pub label_counts: Vec<(String, usize)>,
}

/// Lists every concept and link of a memoir that breaks `ontology`. Used when
/// an ontology is attached to a memoir that already has content: existing
/// rows are reported, not rejected.
pub fn ontology_violations(
    ontology: &Ontology,
    concepts: &[Concept],
    links: &[ConceptLink],
) -> Vec<String> {
    let mut out: Vec<String> = concepts
        .iter()
        .filter_map(|c| ontology.validate_concept(c).err())
        .collect();
    let by_id: std::collections::HashMap<&str, &Concept> =
        concepts.iter().map(|c| (c.id.as_str(), c)).collect();
    for link in links {
        if let (Some(s), Some(t)) = (
            by_id.get(link.source_id.as_str()),
            by_id.get(link.target_id.as_str()),
        ) && let Err(e) = ontology.validate_link(link.relation, s, t)
        {
            out.push(e);
        }
    }
    out
}

#[cfg(test)]
mod ontology_tests {
    use super::*;

    fn concept(name: &str, ty: Option<&str>) -> Concept {
        let mut c = Concept::new("m".into(), name.into(), "def".into());
        if let Some(t) = ty {
            c.labels.push(Label::new(TYPE_NAMESPACE, t));
        }
        c
    }

    fn ops() -> Ontology {
        Ontology {
            types: vec!["service".into(), "incident".into(), "deploy".into()],
            require_type: true,
            relations: vec![
                "incident:caused_by:deploy".parse().unwrap(),
                "service:depends_on:service".parse().unwrap(),
                "related_to".parse().unwrap(),
            ],
        }
    }

    #[test]
    fn rule_parsing_round_trips() {
        let r: RelationRule = "*:depends_on:service".parse().unwrap();
        assert_eq!(r.from, None);
        assert_eq!(r.to.as_deref(), Some("service"));
        assert_eq!(r.to_string(), "*:depends_on:service");
        assert!("a:b".parse::<RelationRule>().is_err());
        assert!("x:nope:y".parse::<RelationRule>().is_err());
    }

    #[test]
    fn empty_ontology_accepts_everything() {
        let o = Ontology::default();
        assert!(o.is_empty());
        assert!(o.validate_concept(&concept("a", None)).is_ok());
        assert!(
            o.validate_link(
                Relation::Refines,
                &concept("a", None),
                &concept("b", Some("x"))
            )
            .is_ok()
        );
    }

    #[test]
    fn concept_types_are_checked() {
        let o = ops();
        assert!(o.validate_concept(&concept("api", Some("service"))).is_ok());
        let missing = o.validate_concept(&concept("api", None)).unwrap_err();
        assert!(missing.contains("requires one of"), "{missing}");
        let unknown = o
            .validate_concept(&concept("api", Some("svc")))
            .unwrap_err();
        assert!(unknown.contains("unknown type 'svc'"), "{unknown}");
        let mut two = concept("api", Some("service"));
        two.labels.push(Label::new(TYPE_NAMESPACE, "deploy"));
        assert!(o.validate_concept(&two).is_err());
    }

    #[test]
    fn links_need_a_matching_rule() {
        let o = ops();
        let (inc, dep, svc) = (
            concept("outage", Some("incident")),
            concept("v42", Some("deploy")),
            concept("api", Some("service")),
        );
        assert!(o.validate_link(Relation::CausedBy, &inc, &dep).is_ok());
        // Wrong direction.
        let err = o.validate_link(Relation::CausedBy, &dep, &inc).unwrap_err();
        assert!(err.contains("incident:caused_by:deploy"), "{err}");
        // Undeclared relation.
        let err = o.validate_link(Relation::PartOf, &svc, &svc).unwrap_err();
        assert!(err.contains("not declared"), "{err}");
        // Wildcard rule.
        assert!(o.validate_link(Relation::RelatedTo, &inc, &svc).is_ok());
    }

    #[test]
    fn memoir_without_ontology_deserializes() {
        let json = r#"{"id":"1","name":"n","description":"","created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z","consolidation_threshold":50}"#;
        let m: Memoir = serde_json::from_str(json).unwrap();
        assert!(m.ontology.is_none());
    }
}
