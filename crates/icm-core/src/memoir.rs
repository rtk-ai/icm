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
}

impl Memoir {
    pub fn new(name: String, description: String) -> Self {
        let now = Utc::now();
        Self {
            id: ulid::Ulid::new().to_string(),
            name,
            description,
            created_at: now,
            updated_at: now,
            consolidation_threshold: 50,
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
    pub fn new(memoir_id: String, name: String, definition: String) -> Self {
        let now = Utc::now();
        Self {
            id: ulid::Ulid::new().to_string(),
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
            id: ulid::Ulid::new().to_string(),
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

#[derive(Debug, Clone)]
pub struct MemoirStats {
    pub total_concepts: usize,
    pub total_links: usize,
    pub avg_confidence: f32,
    pub label_counts: Vec<(String, usize)>,
}
