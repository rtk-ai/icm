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

### Types

| Type | Purpose |
|------|---------|
| `Memory` | Episodic memory with temporal decay (id, topic, summary, weight, importance, embedding) |
| `Importance` | Enum: Critical, High, Medium, Low — controls decay rate and prune eligibility |
| `MemorySource` | Enum: ClaudeCode, Conversation, Manual — tracks origin |
| `StoreStats` | Aggregate statistics (counts, averages, date range) |
| `TopicHealth` | Per-topic hygiene metrics (staleness, consolidation need) |
| `Memoir` | Named knowledge container with consolidation threshold |
| `Concept` | Node in a knowledge graph (name, definition, labels, confidence, revision) |
| `ConceptLink` | Typed edge between concepts (9 relation types) |
| `Label` | Namespace:value pair for concept classification |

### Traits

```rust
trait MemoryStore    // CRUD + search + decay + consolidation for memories
trait MemoirStore    // CRUD + search + graph traversal for memoirs/concepts
trait Embedder       // embed(&str) -> Vec<f32>, dimensions(), model_name()
```

### Embedder

Feature-gated (`embeddings`). Uses fastembed v4 with lazy model initialization.

Default model: `intfloat/multilingual-e5-base` (768 dimensions, 100+ languages).

The model is loaded on first call via `OnceLock` + `Mutex` double-check pattern (because `OnceLock::get_or_try_init` is unstable).

## icm-store

SQLite implementation of `MemoryStore` + `MemoirStore`.

### Schema

```
memories          Main table (id, topic, summary, weight, importance, embedding, ...)
memories_fts      FTS5 virtual table (synchronized via triggers)
vec_memories      sqlite-vec virtual table for cosine similarity search
memoirs           Knowledge containers
concepts          Graph nodes (with FTS via concepts_fts)
concept_links     Graph edges with CHECK(source != target)
icm_metadata      Key-value store for internal state (embedding_dims, last_decay_at)
```

### Migrations

Schema is auto-migrating:
- Missing columns (`updated_at`, `embedding`) are added on startup
- Dimension changes (model switch) drop `vec_memories` and clear embeddings
- FTS tables are created only if missing (idempotent)

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
                          └─ No results? ──► Keyword LIKE fallback
```

### Decay Model

Decay is applied on recall if >24h since last run. Formula per memory:

```
rate = base_decay_rate × importance_multiplier / (1 + access_count × 0.1)

importance_multiplier:
  Critical = 0.0 (no decay)
  High     = 0.5
  Medium   = 1.0
  Low      = 2.0
```

Prune eligibility: only Medium and Low importance memories can be auto-pruned.

### sqlite-vec Integration

sqlite-vec is loaded via `sqlite3_auto_extension` with `transmute` (required by the C extension API). This runs once per process via `std::sync::Once`.

Vector table uses `distance_metric=cosine` (important: default is L2, which gives negative similarities).

## icm-mcp

MCP server implementing JSON-RPC 2.0 over stdio.

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

### Store Nudge

The server tracks consecutive non-store tool calls. After 10 calls without an `icm_memory_store`, it appends a hint to the response:

```
[ICM: 12 tool calls since last store. Consider saving important context.]
```

Counter resets on every `icm_memory_store` call.

### Compact Mode

`icm serve --compact` produces shorter responses:
- Store: `ok:<id>` instead of `Stored memory: <id>`
- Recall: `[topic] summary` per line instead of multi-line verbose format

Saves ~40% tokens on recall output.

## icm-cli

The binary entrypoint. Handles:

- **Config**: TOML config at `~/.config/icm/config.toml` with `[embeddings]` section
- **Init**: Auto-detect and configure 14 AI tools via JSON/TOML config injection
- **Extract**: Rule-based fact extraction from text (zero LLM cost)
- **Benchmarks**: `bench`, `bench-agent`, `bench-recall` subcommands

### Extraction (Layer 0)

Pattern-based scoring. Each sentence is scored by keyword matches:

| Signal | Keywords | Score boost |
|--------|----------|-------------|
| Architecture | `uses`, `architecture`, `pattern`, `algorithm` | +3 |
| Error/Fix | `error`, `fixed`, `bug`, `workaround` | +3 |
| Decision | `decided`, `chose`, `prefer`, `switched to` | +4 |
| Config | `configured`, `setup`, `installed`, `enabled` | +2 |

Sentences scoring above threshold are stored with auto-dedup via Jaccard similarity (>0.6 = skip).

## Build

```bash
cargo build --release                           # Full build with embeddings
cargo build --release --no-default-features     # Without embeddings (fast, small)
```

The `embeddings` feature adds fastembed + ort (~2GB debug build). Use `--no-default-features` for fast iteration.

## Testing

```bash
cargo test                     # 103 tests across all crates
cargo clippy -- -D warnings    # Lint
cargo fmt --check              # Format check
```

Test categories:
- **Unit**: Core types, store CRUD, FTS, vector search, schema migrations
- **Security**: SQL injection, FTS injection, null bytes, unicode, large inputs
- **Performance**: Bulk insert/search, decay/prune at scale, topic management
- **UX**: Error messages, compact output, empty state handling, edge cases
- **Integration**: MCP tool dispatch, roundtrip store+recall, protocol serialization

## Configuration Reference

```toml
# ~/.config/icm/config.toml

[embeddings]
model = "intfloat/multilingual-e5-base"    # Any fastembed model code
```

Environment variables:
- `ICM_CONFIG` — override config file path
- `ICM_DB` — override database file path
- `ICM_LOG` — set log level (debug, info, warn, error)
