# icm

Permanent memory for AI agents. Single binary, zero dependencies, MCP native.

## What is ICM?

ICM gives your AI agent a real memory — not a note-taking tool, not a context manager, a **memory**.

Two memory models:

- **Topics** — store/recall with temporal decay by importance. Critical memories never fade, low-importance ones decay naturally. Consolidate when a topic grows too large.
- **Memoirs** — permanent knowledge graphs. Concepts linked by typed relations (`depends_on`, `contradicts`, `refines`, `part_of`, ...). The agent builds structured knowledge it can traverse and reason over.

## Install

```bash
# Homebrew (macOS / Linux)
brew tap rtk-ai/tap && brew install icm

# Quick install
curl -fsSL https://raw.githubusercontent.com/rtk-ai/icm/main/install.sh | sh

# From source
cargo install --path crates/icm-cli
```

## Usage with Claude Code

### 1. Add ICM as an MCP server

```bash
claude mcp add icm -- icm serve
```

### 2. Add the prompt to your project

Add this to your project's `CLAUDE.md`:

```markdown
## ICM — Persistent memory

When the `icm` MCP server is available, use long-term memory contextually:

### Recall
- Do an `icm_recall` when the current task might have past context (decisions, resolved errors, preferences)
- Do NOT dump everything at startup — only search for what's relevant to the current task

### Proactive store
Automatically store:
- Important architecture decisions
- Resolved errors and their solutions
- User preferences discovered during the session
- Project context when finishing significant work

Do NOT store:
- Trivial or temporary details
- What's already in the project's CLAUDE.md
- Ephemeral info (build state, etc.)
```

### 3. That's it

Claude Code now has persistent memory across sessions. It remembers your decisions, your preferences, your resolved errors — and forgets what doesn't matter.

## CLI

```bash
# Store a memory
icm store -t "my-project" -c "Use PostgreSQL for the main DB" -i high

# Recall memories
icm recall "database choice"
icm recall "auth setup" --topic "my-project" --limit 10

# Forget a memory
icm forget <memory-id>

# Consolidate a topic into a single summary
icm consolidate --topic "my-project"

# List topics
icm topics

# Stats
icm stats
```

### Memoirs (knowledge graphs)

```bash
# Create a memoir
icm memoir create "system-architecture"

# Add concepts
icm memoir add-concept "system-architecture" "auth-service" "Handles JWT tokens and OAuth2 flows"
icm memoir add-concept "system-architecture" "api-gateway" "Routes requests, rate limiting"

# Link concepts
icm memoir link "system-architecture" "api-gateway" "depends_on" "auth-service"

# Inspect a concept and its neighborhood
icm memoir inspect "system-architecture" "auth-service" --depth 2

# Search
icm memoir search "system-architecture" "authentication"
```

## MCP Tools (14)

| Tool | Description |
|------|-------------|
| `icm_store` | Store a memory with topic, importance, keywords |
| `icm_recall` | Search memories by query, optionally filtered by topic |
| `icm_forget` | Delete a memory by ID |
| `icm_consolidate` | Merge all memories of a topic into one summary |
| `icm_list_topics` | List all topics with counts |
| `icm_stats` | Global memory statistics |
| `icm_memoir_create` | Create a new memoir (knowledge container) |
| `icm_memoir_list` | List all memoirs |
| `icm_memoir_show` | Show memoir details and all concepts |
| `icm_memoir_add_concept` | Add a concept to a memoir |
| `icm_memoir_refine` | Update a concept's definition |
| `icm_memoir_search` | Full-text search within a memoir |
| `icm_memoir_link` | Create typed relation between concepts |
| `icm_memoir_inspect` | Inspect a concept and its graph neighborhood (BFS) |

## Relation types

`part_of` · `depends_on` · `related_to` · `contradicts` · `refines` · `alternative_to` · `caused_by` · `instance_of`

## How it works

- **Storage**: SQLite with FTS5 full-text search
- **Decay**: Configurable per importance level — `critical` never decays, `low` decays fast
- **Single binary**: ~4 MB, no runtime dependencies
- **MCP transport**: stdio (native Claude Code integration)

## License

[MIT](LICENSE)
