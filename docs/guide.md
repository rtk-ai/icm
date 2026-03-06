# ICM User Guide

## What is ICM?

ICM gives your AI coding agent a persistent memory that survives across sessions. Without ICM, every time a session ends or the context window compacts, the agent forgets everything — your architecture decisions, resolved bugs, project conventions. With ICM, it remembers.

## Quick Start

### 1. Install

```bash
brew tap rtk-ai/tap && brew install icm
```

### 2. Setup

```bash
icm init
```

This auto-detects your AI tools (Claude Code, Cursor, VS Code, etc.) and configures the MCP server for each one.

### 3. Use

That's it. Your agent now has access to `icm_memory_store` and `icm_memory_recall`. It will use them automatically based on the MCP instructions.

## Two Memory Models

### Memories (Episodic)

For things that happen: decisions, errors, configurations, preferences. Organized by **topic**.

```bash
# Store a decision
icm store -t "project-api" -c "Chose REST over GraphQL for v1 simplicity" -i high

# Store an error resolution
icm store -t "errors-resolved" -c "CORS issue fixed by adding origin header in nginx" -i medium -k "cors,nginx"

# Recall relevant context
icm recall "API design choices"
icm recall "nginx" --topic "errors-resolved"
```

Memories have **temporal decay** — low-importance memories fade over time, critical ones persist forever. This keeps the memory space clean without manual intervention.

**Importance levels:**

| Level | Decay | Auto-prune | When to use |
|-------|-------|------------|-------------|
| `critical` | Never | Never | Core architecture, must-know facts |
| `high` | Slow | Never | Important decisions, recurring patterns |
| `medium` | Normal | Yes | Context, configurations, one-time fixes |
| `low` | Fast | Yes | Temporary notes, exploration results |

### Memoirs (Semantic)

For structured knowledge that should be permanent: architecture diagrams as graphs, concept relationships, domain models.

```bash
# Create a knowledge container
icm memoir create -n "backend-arch" -d "Backend architecture decisions"

# Add concepts
icm memoir add-concept -m "backend-arch" -n "user-service" \
  -d "Handles user registration, authentication, and profile management" \
  -l "domain:auth,type:microservice"

icm memoir add-concept -m "backend-arch" -n "postgres" \
  -d "Primary datastore for user and transaction data" \
  -l "type:database"

# Link them
icm memoir link -m "backend-arch" --from "user-service" --to "postgres" -r depends-on

# Search
icm memoir search -m "backend-arch" "authentication"

# Explore neighborhood
icm memoir inspect -m "backend-arch" "user-service" -D 2
```

Concepts are never decayed — they get **refined** (new definition, incremented revision, increased confidence) or marked as **superseded**.

## Topic Organization

Good topic naming helps recall. Suggested patterns:

| Pattern | Example | Use for |
|---------|---------|---------|
| `decisions-{project}` | `decisions-api` | Architecture and design choices |
| `errors-resolved` | `errors-resolved` | Bug fixes with their solutions |
| `preferences` | `preferences` | User coding style, tool preferences |
| `context-{project}` | `context-frontend` | Project-specific knowledge |
| `conventions-{project}` | `conventions-api` | Code style, naming, file structure |

## Consolidation

When a topic accumulates many entries, consolidate them:

```bash
# Via CLI
icm consolidate --topic "errors-resolved"

# Via MCP
# The agent calls icm_memory_consolidate with a summary
```

Consolidation replaces N individual memories with one dense summary. ICM warns when a topic has >7 entries.

## Embedding Configuration

By default, ICM uses multilingual embeddings for semantic search. You can change the model:

```bash
icm config    # Show current settings
```

Edit `~/.config/icm/config.toml`:

```toml
[embeddings]
# Multilingual (recommended)
model = "intfloat/multilingual-e5-base"       # 768d, 100+ languages

# Lighter alternative
# model = "intfloat/multilingual-e5-small"    # 384d, faster

# English-only (fastest)
# model = "Xenova/bge-small-en-v1.5"          # 384d

# Code-optimized
# model = "jinaai/jina-embeddings-v2-base-code"  # 768d
```

Changing the model automatically migrates the vector index on next startup.

## Auto-Extraction

ICM can extract facts from agent output without any LLM cost:

```bash
# Pipe any text
echo "Fixed the CORS bug by adding Access-Control-Allow-Origin to nginx.conf" | icm extract -p my-project

# Extract from a file
cat session-log.txt | icm extract -p my-project
```

The extraction is rule-based: it scores sentences by keyword signals (architecture terms, error patterns, decision language) and stores high-scoring ones.

## Context Injection

At session start, inject relevant memories into the agent's context:

```bash
icm recall-context "my-project backend API"
```

This returns a formatted block of relevant memories that can be prepended to the agent's prompt.

## Health Check

Monitor your memory space:

```bash
icm stats            # Global overview
icm topics           # List all topics with entry counts
icm health           # Per-topic hygiene report
icm health --topic "decisions-api"   # Single topic
```

The health report flags:
- Topics needing consolidation (>7 entries)
- Stale entries (low weight, many accesses but not reinforced)
- Topics with no recent activity

## Skills and Rules

For deeper integration with specific tools:

```bash
icm init --mode skill
```

This installs:
- **Claude Code**: `/recall` and `/remember` slash commands
- **Cursor**: `.mdc` rule file for automatic memory usage
- **Roo Code**: `.md` rule file
- **Amp**: `/icm-recall` and `/icm-remember` commands

## Database Location

```
macOS:   ~/Library/Application Support/dev.icm.icm/memories.db
Linux:   ~/.local/share/dev.icm.icm/memories.db
```

Override with `--db <path>` or `ICM_DB` environment variable.

## Compact Mode

For token-constrained environments:

```bash
icm serve --compact
```

Produces shorter MCP responses (~40% fewer tokens):
- Store: `ok:<id>`
- Recall: `[topic] summary` per line

## Troubleshooting

**Agent doesn't use ICM tools**
- Verify: `icm init` ran successfully
- Check MCP config: the tool should show in the agent's tool list
- Try `icm serve` manually to confirm it starts without errors

**Recall returns nothing**
- Check: `icm topics` — are there any stored memories?
- Try broader queries or remove topic filters
- Run `icm health` to check for weight decay issues

**Embeddings slow on first run**
- Normal: the model downloads on first use (~100MB)
- Subsequent runs load from cache (~1-2s)
- Use `--no-default-features` build for no embeddings

**Database issues**
- ICM uses WAL mode — safe for concurrent reads
- If corrupted: `icm stats` will fail — delete the DB file and re-populate
