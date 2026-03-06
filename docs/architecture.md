# ICM Technical Architecture

## Overview

ICM is a Rust workspace of 4 crates that compile into a single binary. No runtime dependencies, no external services.

```
icm (binary)
├── icm-core      Types, traits, embedder
├── icm-store     SQLite + FTS5 + sqlite-vec
├── icm-mcp       MCP server (JSON-RPC 2.0 over stdio)
└── icm-cli       CLI, config, extraction, benchmarks
```

## Crate Dependency Graph

```
icm-cli ──────► icm-core
   │               ▲
   ├──► icm-store ─┘
   │       ▲
   └──► icm-mcp ──► icm-store
            │
            └──► icm-core
```

## icm-core

Foundation crate. No I/O, no database — only types and traits.

### Data Types

#### Memory

```rust
pub struct Memory {
    pub id: String,                     // ULID
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_accessed: DateTime<Utc>,
    pub access_count: u32,
    pub weight: f32,                    // 1.0 at creation, decays over time
    pub topic: String,
    pub summary: String,
    pub raw_excerpt: Option<String>,
    pub keywords: Vec<String>,
    pub importance: Importance,         // Critical | High | Medium | Low
    pub source: MemorySource,           // ClaudeCode | Conversation | Manual
    pub related_ids: Vec<String>,
    pub embedding: Option<Vec<f32>>,    // 384/768/1024d depending on model
}
```

#### Importance

```rust
pub enum Importance {
    Critical,   // decay: 0.0 (never), prune: never
    High,       // decay: 0.5x rate, prune: never
    Medium,     // decay: 1.0x rate, prune: when weight < threshold
    Low,        // decay: 2.0x rate, prune: when weight < threshold
}
```

#### Memoir (Knowledge Graph)

```rust
pub struct Memoir {
    pub id: String,
    pub name: String,                   // unique
    pub description: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub consolidation_threshold: u32,   // default: 50
}

pub struct Concept {
    pub id: String,
    pub memoir_id: String,
    pub name: String,                   // unique within memoir
    pub definition: String,
    pub labels: Vec<Label>,             // namespace:value pairs
    pub confidence: f32,                // 0.0-1.0, grows with refinement
    pub revision: u32,                  // incremented on refine
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub source_memory_ids: Vec<String>,
}

pub struct ConceptLink {
    pub id: String,
    pub source_id: String,
    pub target_id: String,
    pub relation: Relation,             // 9 types (see below)
    pub weight: f32,
    pub created_at: DateTime<Utc>,
}
```

#### Relations

```rust
pub enum Relation {
    PartOf,          // A is part of B
    DependsOn,       // A requires B
    RelatedTo,       // A is associated with B
    Contradicts,     // A conflicts with B
    Refines,         // A is a more precise version of B
    AlternativeTo,   // A can replace B
    CausedBy,        // A is caused by B
    InstanceOf,      // A is an instance of B
    SupersededBy,    // A is replaced by B (marks obsolescence)
}
```

Parsing accepts both `snake_case` and `camelCase` (`depends_on` = `dependson`). Self-links (source == target) are rejected at the database level via CHECK constraint.

### Traits

#### MemoryStore

```rust
pub trait MemoryStore {
    fn store(&self, memory: Memory) -> IcmResult<String>;
    fn get(&self, id: &str) -> IcmResult<Option<Memory>>;
    fn update(&self, memory: &Memory) -> IcmResult<()>;
    fn delete(&self, id: &str) -> IcmResult<()>;

    fn search_by_keywords(&self, keywords: &[&str], limit: usize) -> IcmResult<Vec<Memory>>;
    fn search_fts(&self, query: &str, limit: usize) -> IcmResult<Vec<Memory>>;
    fn search_by_embedding(&self, embedding: &[f32], limit: usize) -> IcmResult<Vec<(Memory, f32)>>;
    fn search_hybrid(&self, query: &str, embedding: &[f32], limit: usize) -> IcmResult<Vec<(Memory, f32)>>;

    fn update_access(&self, id: &str) -> IcmResult<()>;
    fn apply_decay(&self, decay_factor: f32) -> IcmResult<usize>;
    fn prune(&self, weight_threshold: f32) -> IcmResult<usize>;

    fn get_by_topic(&self, topic: &str) -> IcmResult<Vec<Memory>>;
    fn list_topics(&self) -> IcmResult<Vec<(String, usize)>>;
    fn consolidate_topic(&self, topic: &str, consolidated: Memory) -> IcmResult<()>;

    fn count(&self) -> IcmResult<usize>;
    fn count_by_topic(&self, topic: &str) -> IcmResult<usize>;
    fn stats(&self) -> IcmResult<StoreStats>;
    fn topic_health(&self, topic: &str) -> IcmResult<TopicHealth>;
}
```

#### MemoirStore

```rust
pub trait MemoirStore {
    fn create_memoir(&self, memoir: Memoir) -> IcmResult<String>;
    fn get_memoir(&self, id: &str) -> IcmResult<Option<Memoir>>;
    fn get_memoir_by_name(&self, name: &str) -> IcmResult<Option<Memoir>>;
    fn update_memoir(&self, memoir: &Memoir) -> IcmResult<()>;
    fn delete_memoir(&self, id: &str) -> IcmResult<()>;  // CASCADE: deletes concepts + links
    fn list_memoirs(&self) -> IcmResult<Vec<Memoir>>;

    fn add_concept(&self, concept: Concept) -> IcmResult<String>;
    fn get_concept(&self, id: &str) -> IcmResult<Option<Concept>>;
    fn get_concept_by_name(&self, memoir_id: &str, name: &str) -> IcmResult<Option<Concept>>;
    fn update_concept(&self, concept: &Concept) -> IcmResult<()>;
    fn delete_concept(&self, id: &str) -> IcmResult<()>;

    fn list_concepts(&self, memoir_id: &str) -> IcmResult<Vec<Concept>>;
    fn search_concepts_fts(&self, memoir_id: &str, query: &str, limit: usize) -> IcmResult<Vec<Concept>>;
    fn search_concepts_by_label(&self, memoir_id: &str, label: &Label, limit: usize) -> IcmResult<Vec<Concept>>;
    fn search_all_concepts_fts(&self, query: &str, limit: usize) -> IcmResult<Vec<Concept>>;

    fn refine_concept(&self, id: &str, new_definition: &str, new_source_ids: &[String]) -> IcmResult<()>;

    fn add_link(&self, link: ConceptLink) -> IcmResult<String>;
    fn get_links_from(&self, concept_id: &str) -> IcmResult<Vec<ConceptLink>>;
    fn get_links_to(&self, concept_id: &str) -> IcmResult<Vec<ConceptLink>>;
    fn delete_link(&self, id: &str) -> IcmResult<()>;
    fn get_neighbors(&self, concept_id: &str, relation: Option<Relation>) -> IcmResult<Vec<Concept>>;
    fn get_neighborhood(&self, concept_id: &str, depth: usize) -> IcmResult<(Vec<Concept>, Vec<ConceptLink>)>;

    fn memoir_stats(&self, memoir_id: &str) -> IcmResult<MemoirStats>;
}
```

#### Embedder

```rust
pub trait Embedder: Send + Sync {
    fn embed(&self, text: &str) -> IcmResult<Vec<f32>>;
    fn embed_batch(&self, texts: &[&str]) -> IcmResult<Vec<Vec<f32>>>;
    fn dimensions(&self) -> usize;
}
```

Feature-gated (`embeddings`). Uses fastembed v4 with lazy initialization via `OnceLock` + `Mutex` double-check pattern (because `OnceLock::get_or_try_init` is unstable).

Default model: `intfloat/multilingual-e5-base` (768d, 100+ languages). Configurable via `config.toml`.

### Error Types

```rust
pub enum IcmError {
    NotFound(String),                   // get/delete with unknown ID
    Database(String),                   // SQLite errors
    Serialization(serde_json::Error),   // JSON serialization
    Config(String),                     // Configuration errors
    Embedding(String),                  // Embedding model errors
}

pub type IcmResult<T> = Result<T, IcmError>;
```

## icm-store

SQLite implementation of `MemoryStore` + `MemoirStore` via rusqlite 0.34 (synchronous, not async).

### Constructors

```rust
SqliteStore::new(path: &Path) -> IcmResult<Self>                // default 384d
SqliteStore::with_dims(path: &Path, dims: usize) -> IcmResult<Self>  // custom dimensions
SqliteStore::in_memory() -> IcmResult<Self>                     // for tests
```

### Schema (8 tables)

| Table | Type | Purpose |
|-------|------|---------|
| `memories` | regular | Main memory storage (id, topic, summary, weight, embedding, ...) |
| `memories_fts` | FTS5 virtual | Full-text search on id, topic, summary, keywords |
| `vec_memories` | vec0 virtual | Cosine similarity search via sqlite-vec |
| `memoirs` | regular | Named knowledge containers |
| `concepts` | regular | Knowledge graph nodes with UNIQUE(memoir_id, name) |
| `concepts_fts` | FTS5 virtual | Full-text search on concept id, name, definition, labels |
| `concept_links` | regular | Typed edges with CHECK(source_id != target_id) |
| `icm_metadata` | regular | Key-value store (embedding_dims, last_decay_at) |

FTS tables are synchronized via AFTER INSERT/UPDATE/DELETE triggers.

### Auto-Migrations

On startup, the schema is checked and migrated if needed:

1. Missing columns (`updated_at`, `embedding`) → `ALTER TABLE ADD COLUMN`
2. Missing FTS/vec tables → created if absent
3. Dimension change (model switch) → drops `vec_memories`, clears all embeddings, recreates with new dimensions, stores new dim in `icm_metadata`

### Search Pipeline

```
Query arrives
    │
    ├─ Has embedder? ──► Hybrid search
    │                      ├─ FTS5 BM25 (30% weight)
    │                      ├─ Cosine similarity via sqlite-vec (70% weight)
    │                      └─ Merge + deduplicate by memory ID
    │
    └─ No embedder ──► FTS5 search
                          │
                          └─ No FTS results? ──► Keyword LIKE fallback
```

FTS queries are sanitized: special characters (`-`, `*`, `:`, etc.) are stripped and each token is quoted to prevent FTS5 syntax injection.

### Decay Model

Decay runs automatically on recall if >24h since last run. Stored in `icm_metadata.last_decay_at`.

```
effective_rate = base_decay × importance_multiplier / (1 + access_count × 0.1)

importance_multiplier:
  Critical = 0.0 (no decay ever)
  High     = 0.5 (half speed)
  Medium   = 1.0 (normal)
  Low      = 2.0 (double speed)

new_weight = weight × (1 - effective_rate)
```

Prune: only Medium and Low importance memories with `weight < threshold` are deleted.

### sqlite-vec

Loaded via `sqlite3_auto_extension` with `transmute` (required by C extension API). Initialization runs once per process via `std::sync::Once`.

Vector table: `distance_metric=cosine` (L2 is the default but gives negative similarities for normalized vectors).

### Dedup

On store via MCP, if an existing memory in the same topic has >85% hybrid search similarity, the existing memory is updated instead of creating a duplicate.

### Cascade Delete

`DELETE memoir` → cascades to all concepts → cascades to all links (via `ON DELETE CASCADE`).

## icm-mcp

MCP server implementing JSON-RPC 2.0 over stdio. 18 tools.

### Protocol Flow

```
Client                              ICM Server
  │                                      │
  ├── initialize ───────────────────────►│
  │◄── capabilities + instructions ──────┤
  │                                      │
  ├── tools/list ───────────────────────►│
  │◄── 18 tool definitions ─────────────┤
  │                                      │
  ├── tools/call {name, arguments} ─────►│
  │◄── ToolResult {content, isError} ───┤
  │                                      │
  └── (stdin closes) ──────────────────►│ exit
```

### Tool Dispatch

| Tool | Required args | Optional args |
|------|--------------|---------------|
| `icm_memory_store` | `topic`, `content` | `importance`, `keywords[]`, `raw_excerpt` |
| `icm_memory_recall` | `query` | `topic`, `keyword`, `limit` |
| `icm_memory_update` | `id`, `content` | `importance`, `keywords[]` |
| `icm_memory_forget` | `id` | — |
| `icm_memory_consolidate` | `topic`, `summary` | — |
| `icm_memory_list_topics` | — | — |
| `icm_memory_stats` | — | — |
| `icm_memory_health` | — | `topic` |
| `icm_memory_embed_all` | — | `topic` |
| `icm_memoir_create` | `name` | `description` |
| `icm_memoir_list` | — | — |
| `icm_memoir_show` | `name` | — |
| `icm_memoir_add_concept` | `memoir`, `name`, `definition` | `labels` |
| `icm_memoir_refine` | `memoir`, `name`, `definition` | — |
| `icm_memoir_search` | `memoir`, `query` | `label`, `limit` |
| `icm_memoir_search_all` | `query` | `limit` |
| `icm_memoir_link` | `memoir`, `from`, `to`, `relation` | — |
| `icm_memoir_inspect` | `memoir`, `name` | `depth` |

### Store Nudge

The server tracks consecutive non-store tool calls. After 10 calls without `icm_memory_store`, it appends a hint to the response:

```
[ICM: 12 tool calls since last store. Consider saving important context.]
```

Counter resets on every `icm_memory_store`.

### Compact Mode

`icm serve --compact` produces shorter responses:
- Store: `ok:<id>` instead of `Stored memory: <id>`
- Recall: `[topic] summary\n` per line instead of multi-line verbose format

Saves ~40% tokens on recall output.

### Auto-behaviors

- **Auto-dedup**: `icm_memory_store` checks hybrid similarity >85% in same topic → updates existing instead of duplicating
- **Auto-decay**: `icm_memory_recall` runs decay if >24h since last run
- **Consolidation hint**: `icm_memory_store` warns when topic has >7 entries
- **Auto-embed**: if embedder is available, memories are embedded on store/update

## icm-cli

Binary entrypoint. All commands:

```
icm store         Store a memory
icm recall        Search memories
icm forget        Delete a memory by ID
icm topics        List all topics
icm stats         Global statistics
icm health        Per-topic hygiene report
icm decay         Apply temporal decay
icm prune         Delete low-weight memories
icm consolidate   Merge topic into single summary
icm embed         Backfill embeddings
icm extract       Rule-based fact extraction from stdin/text
icm recall-context  Format recalled memories for prompt injection
icm memoir        Subcommands: create, show, add-concept, refine, search, search-all, link, inspect, list
icm init          Auto-configure 14 AI tools (mcp, cli, skill, hook modes)
icm serve         Start MCP server (--compact for shorter output)
icm config        Show active configuration
icm bench         Storage performance benchmark
icm bench-recall  Knowledge retention benchmark
icm bench-agent   Multi-session agent efficiency benchmark
```

### Extraction (Layer 0)

Pattern-based scoring. Each sentence gets a score from keyword matches:

| Signal | Example keywords | Score boost |
|--------|-----------------|-------------|
| Architecture | `uses`, `architecture`, `pattern`, `algorithm` | +3 |
| Error/Fix | `error`, `fixed`, `bug`, `workaround` | +3 |
| Decision | `decided`, `chose`, `prefer`, `switched to` | +4 |
| Config | `configured`, `setup`, `installed`, `enabled` | +2 |
| Dev signals | `commit`, `deploy`, `migrate`, `refactor` | +2 |

Sentences below threshold are dropped. Dedup via Jaccard similarity (>0.6 = skip).

## Build

```bash
cargo build --release                           # Full build with embeddings
cargo build --release --no-default-features     # Without embeddings (fast, small)
```

The `embeddings` feature adds fastembed + ort (~2GB debug build). Use `--no-default-features` for fast iteration on non-embedding code.

## Testing

```bash
cargo test          # 110 tests across all crates
cargo clippy        # Lint (CI uses -D warnings)
cargo fmt --check   # Format check
```

Test categories:

| Category | Count | What's tested |
|----------|-------|---------------|
| Unit | ~50 | Core CRUD, FTS, vector search, schema migrations, memoirs, concepts, links, graph traversal |
| Security | ~10 | SQL injection (topic, summary, keywords, FTS), null bytes, unicode, XSS via MCP, large inputs |
| Performance | 7 | 1000 stores, 100 FTS/vector/hybrid searches, decay on 1000 memories, 1000 gets (all with time assertions) |
| UX | ~15 | Missing params, unknown tools, empty states, compact output, protocol serialization |
| Integration | ~28 | MCP tool dispatch, store+recall roundtrip, consolidation, topic filtering, dedup |

## Configuration

```toml
# ~/.config/icm/config.toml

[embeddings]
model = "intfloat/multilingual-e5-base"    # Any fastembed model code
```

Environment variables:
- `ICM_CONFIG` — override config file path
- `ICM_DB` — override database file path
- `ICM_LOG` — set log level (debug, info, warn, error)

## Security

- All SQL queries use parameterized statements (no string interpolation)
- FTS5 queries are sanitized (special chars stripped, tokens quoted)
- No network access for storage (local SQLite only)
- Embedding model runs locally (no API calls unless explicitly configured)
- Self-links rejected via SQL CHECK constraint
- Tested against: SQL injection, FTS injection, null bytes, unicode boundaries, 500KB payloads
