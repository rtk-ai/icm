# ICM User Guide

## What is ICM?

ICM gives your AI coding agent a persistent memory that survives across sessions. Without ICM, every time a session ends or the context window compacts, the agent forgets everything — your architecture decisions, resolved bugs, project conventions. With ICM, it remembers.

## Quick Start

### 1. Install

```bash
# Homebrew
brew tap rtk-ai/tap && brew install icm

# Quick install
curl -fsSL https://raw.githubusercontent.com/rtk-ai/icm/main/install.sh | sh

# From source
cargo install --path crates/icm-cli
```

### 2. Setup

```bash
icm init
```

This auto-detects your AI tools and configures the MCP server. Supports 14 tools: Claude Code, Claude Desktop, Cursor, Windsurf, VS Code, Gemini, Zed, Amp, Amazon Q, Cline, Roo Code, Kilo Code, Codex CLI, OpenCode.

### 3. Use

That's it. Your agent now has access to 18 MCP tools. It uses them automatically based on the server instructions.

## Two Memory Models

ICM has two complementary memory systems — use both.

### Memories (Episodic)

For things that happen: decisions, errors, configurations, preferences. Organized by **topic**. Memories decay over time unless accessed or marked important.

```bash
# Store a decision
icm store -t "project-api" -c "Chose REST over GraphQL for v1 simplicity" -i high

# Store an error resolution
icm store -t "errors-resolved" -c "CORS issue fixed by adding origin header in nginx" -i medium -k "cors,nginx"

# Store a critical fact (never forgotten)
icm store -t "credentials" -c "Production DB is on port 5433, not 5432" -i critical

# Recall relevant context
icm recall "API design choices"
icm recall "nginx" --topic "errors-resolved"
icm recall "database" --keyword "postgres"
```

**Importance levels:**

| Level | Decay | Auto-prune | When to use |
|-------|-------|------------|-------------|
| `critical` | Never | Never | Core architecture, credentials, must-know facts |
| `high` | Slow (0.5x) | Never | Important decisions, recurring patterns |
| `medium` | Normal (1.0x) | Yes | Context, configurations, one-time fixes |
| `low` | Fast (2.0x) | Yes | Temporary notes, exploration results |

Decay is access-aware: memories recalled often decay slower. Formula: `decay / (1 + access_count × 0.1)`.

### Memoirs (Semantic)

For structured knowledge that should be permanent: architecture as a graph, concept relationships, domain models. Concepts are never decayed — they get refined.

```bash
# Create a knowledge container
icm memoir create -n "backend-arch" -d "Backend architecture decisions"

# Add concepts with labels
icm memoir add-concept -m "backend-arch" -n "user-service" \
  -d "Handles user registration, authentication, and profile management" \
  -l "domain:auth,type:microservice"

icm memoir add-concept -m "backend-arch" -n "postgres" \
  -d "Primary datastore for user and transaction data" \
  -l "type:database"

icm memoir add-concept -m "backend-arch" -n "redis" \
  -d "Session cache and rate limiting" \
  -l "type:database,domain:infra"

# Link concepts
icm memoir link -m "backend-arch" --from "user-service" --to "postgres" -r depends-on
icm memoir link -m "backend-arch" --from "user-service" --to "redis" -r depends-on

# Refine a concept (increments revision, increases confidence)
icm memoir refine -m "backend-arch" -n "user-service" \
  -d "Handles registration, auth (JWT + OAuth2), profile, and 2FA"

# Search within a memoir
icm memoir search -m "backend-arch" "authentication"
icm memoir search -m "backend-arch" "service" --label "domain:auth"

# Search across ALL memoirs
icm memoir search-all "database"

# Explore concept neighborhood (BFS traversal)
icm memoir inspect -m "backend-arch" "user-service" -D 2
```

**9 relation types:** `part_of`, `depends_on`, `related_to`, `contradicts`, `refines`, `alternative_to`, `caused_by`, `instance_of`, `superseded_by`.

Use `superseded_by` to mark obsolete facts instead of deleting them — the history is valuable.

## Topic Organization

Good topic naming helps recall. Suggested patterns:

| Pattern | Example | Use for |
|---------|---------|---------|
| `decisions-{project}` | `decisions-api` | Architecture and design choices |
| `errors-resolved` | `errors-resolved` | Bug fixes with their solutions |
| `preferences` | `preferences` | User coding style, tool preferences |
| `context-{project}` | `context-frontend` | Project-specific knowledge |
| `conventions-{project}` | `conventions-api` | Code style, naming, file structure |
| `credentials` | `credentials` | Ports, URLs, service names (use `critical`) |

## Memory Lifecycle

### Consolidation

When a topic accumulates many entries, consolidate them into a dense summary:

```bash
# See which topics need consolidation
icm health

# Consolidate (replaces all entries with one summary)
icm consolidate --topic "errors-resolved"

# Keep originals alongside the consolidated summary
icm consolidate --topic "errors-resolved" --keep-originals
```

ICM warns when a topic has >7 entries via the MCP `icm_memory_store` response.

### Decay and Pruning

```bash
# Manually apply decay (normally runs automatically on recall, every 24h)
icm decay
icm decay --factor 0.9    # Custom decay factor

# Preview what would be pruned
icm prune --threshold 0.2 --dry-run

# Actually prune
icm prune --threshold 0.1
```

### Health Check

```bash
icm stats                          # Global overview (counts, avg weight, date range)
icm topics                         # List all topics with entry counts
icm health                         # Per-topic hygiene report
icm health --topic "decisions-api" # Single topic
```

The health report flags:
- Topics needing consolidation (>7 entries)
- Stale entries (low weight, many accesses but not reinforced)
- Topics with no recent activity

## Auto-Extraction

ICM extracts facts from text without any LLM cost:

```bash
# Pipe any text
echo "Fixed the CORS bug by adding Access-Control-Allow-Origin to nginx.conf" | icm extract -p my-project

# Extract from a file
cat session-log.txt | icm extract -p my-project

# Preview without storing
echo "Switched from MySQL to PostgreSQL for JSONB support" | icm extract -p api --dry-run
```

Detected signals: architecture patterns, error resolutions, decisions, configurations, refactors, deployments.

## Context Injection

Inject relevant memories at session start:

```bash
icm recall-context "my-project backend API"
icm recall-context "authentication" --limit 20
```

Returns a formatted block ready for prompt prepending. Used by the SessionStart hook for automatic context loading.

## Embedding Configuration

Default: multilingual embeddings for semantic search across 100+ languages.

```bash
icm config    # Show current settings
```

Edit `~/.config/icm/config.toml`:

```toml
[embeddings]
# Multilingual (recommended)
model = "intfloat/multilingual-e5-base"       # 768d, 100+ languages

# Lighter alternative
# model = "intfloat/multilingual-e5-small"    # 384d, faster, multilingual

# Best accuracy
# model = "intfloat/multilingual-e5-large"    # 1024d, multilingual

# English-only (fastest)
# model = "Xenova/bge-small-en-v1.5"          # 384d

# Code-optimized
# model = "jinaai/jina-embeddings-v2-base-code"  # 768d
```

Changing the model automatically migrates the vector index on next startup (existing embeddings are cleared). Regenerate with:

```bash
icm embed                     # Embed all memories without embeddings
icm embed --force             # Re-embed everything
icm embed --topic "decisions" # Only one topic
```

## MCP Tools Reference

### Memory tools (9)

| Tool | What it does |
|------|-------------|
| `icm_memory_store` | Store a memory. Auto-dedup: >85% similar in same topic → update. Warns at >7 entries. |
| `icm_memory_recall` | Search by query. Filters: `topic`, `keyword`, `limit`. Auto-decay if >24h. |
| `icm_memory_update` | Edit content, importance, or keywords of an existing memory by ID. |
| `icm_memory_forget` | Delete a memory by ID. |
| `icm_memory_consolidate` | Replace all memories of a topic with a single summary. |
| `icm_memory_list_topics` | List all topics with entry counts. |
| `icm_memory_stats` | Total memories, topics, average weight, date range. |
| `icm_memory_health` | Per-topic audit: staleness, consolidation needs, access patterns. |
| `icm_memory_embed_all` | Backfill embeddings for memories that don't have one. |

### Memoir tools (9)

| Tool | What it does |
|------|-------------|
| `icm_memoir_create` | Create a named knowledge container. |
| `icm_memoir_list` | List all memoirs with concept counts. |
| `icm_memoir_show` | Show memoir details, stats, and all concepts. |
| `icm_memoir_add_concept` | Add a concept with definition and labels. |
| `icm_memoir_refine` | Update a concept's definition (increments revision, boosts confidence). |
| `icm_memoir_search` | Full-text search within a memoir, optionally filtered by label. |
| `icm_memoir_search_all` | Search across all memoirs at once. |
| `icm_memoir_link` | Create a typed relation between two concepts. |
| `icm_memoir_inspect` | Inspect a concept and its graph neighborhood (BFS to depth N). |

## Init Modes

```bash
icm init                  # Auto-detect and configure MCP for all found tools
icm init --mode skill     # Install slash commands and rules
icm init --mode hook      # Install Claude Code PostToolUse hook for auto-extraction
icm init --mode cli       # Show manual CLI setup instructions
```

### Skills

`icm init --mode skill` installs:
- **Claude Code**: `/recall` and `/remember` slash commands
- **Cursor**: `.cursor/rules/icm.mdc` rule file
- **Roo Code**: `.roo/rules/icm.md` rule file
- **Amp**: `/icm-recall` and `/icm-remember` commands

## Compact Mode

For token-constrained environments:

```bash
icm serve --compact
```

Produces shorter MCP responses (~40% fewer tokens):
- Store: `ok:<id>` instead of `Stored memory: <id> [+ consolidation hint]`
- Recall: `[topic] summary` per line instead of multi-line verbose format

## Database

Single SQLite file with WAL mode. No external services.

```
macOS:   ~/Library/Application Support/dev.icm.icm/memories.db
Linux:   ~/.local/share/dev.icm.icm/memories.db
```

Override: `--db <path>` flag or `ICM_DB` environment variable.

## Benchmarking

```bash
# Storage performance (in-memory, single-threaded)
icm bench --count 1000

# Knowledge retention: can the agent recall facts across sessions?
icm bench-recall --model haiku --runs 5

# Agent efficiency: turns, tokens, cost with/without ICM
icm bench-agent --sessions 10 --model haiku --runs 3
```

All benchmarks use real API calls, no mocks. Each run uses its own tempdir and fresh DB.

## Troubleshooting

**Agent doesn't use ICM tools**
- Run `icm init` and check the output for errors
- Verify the MCP config file exists for your tool
- Test manually: `echo '{"jsonrpc":"2.0","id":1,"method":"initialize"}' | icm serve`
- Check `icm serve` starts without errors

**Recall returns nothing**
- `icm topics` — are there stored memories?
- `icm stats` — check total count
- Try broader queries or remove topic/keyword filters
- `icm health` — check if memories decayed too much

**Embeddings slow on first run**
- Normal: model downloads on first use (~100MB for multilingual-e5-base)
- Subsequent runs load from cache (~1-2s)
- Build without embeddings: `cargo build --no-default-features`

**Duplicate memories**
- Auto-dedup works via MCP (>85% similarity in same topic → update)
- CLI `icm store` does not auto-dedup (no embedder in basic mode)
- Backfill embeddings: `icm embed`, then dedup happens on next store

**Database issues**
- WAL mode: safe for concurrent reads, single writer
- Corruption: `icm stats` will fail → delete DB file and re-populate
- Migration: automatic on startup, no manual steps needed
