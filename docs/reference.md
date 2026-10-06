# ICM reference

Everything the [README](../README.md) leaves out: install options, per-tool setup, CLI and MCP details, the HTTP API, the dashboard, internals and auto-extraction.

## Install

```bash
# Homebrew (macOS / Linux)
brew tap rtk-ai/tap && brew install icm

# Quick install (macOS / Linux) — verifies SHA256 against the release checksums
curl -fsSL https://raw.githubusercontent.com/rtk-ai/icm/main/install.sh | sh

# Quick install (Windows PowerShell)
irm https://raw.githubusercontent.com/rtk-ai/icm/main/install.ps1 | iex

# From source
cargo install --path crates/icm-cli

# Nix / NixOS (flake)
nix run github:rtk-ai/icm -- --version
nix profile install github:rtk-ai/icm
```

Re-run the install command to upgrade to the latest release. To pin a version, pass `--version icm-vX.Y.Z` (sh: `sh -s -- --version …`).

### Semantic search runtime

Keyword search works out of the box everywhere. **Semantic (vector) search** needs the ONNX Runtime, and whether it is already there depends on the build you installed (releases after 0.10.65; `icm embeddings status` tells you which case you are in):

| Build | Semantic search |
|---|---|
| macOS Apple Silicon and Windows x86_64 archives, `.rpm` | Built in — nothing to do |
| Linux x86_64 / aarch64 archives and `.deb` (glibc ≥ 2.35: Debian 12+, Ubuntu 22.04+) | After `icm embeddings download`, once per user (~11 MB) |
| macOS Intel archive | Bring your own ONNX Runtime ≥ 1.24 via `ORT_DYLIB_PATH` — none is published for this platform |
| Linux x86_64 static musl archive (older glibc, Alpine) | Not available — keyword search only |

`install.sh`, `icm upgrade` and the Homebrew tap install these same archives; `install.sh` picks the static musl one by itself on older systems.

```bash
# Enable semantic search on a build that loads the runtime on demand
icm embeddings download

# Check the runtime state
icm embeddings status
```

Builds that load the runtime on demand offer the download once, on first use in a terminal. If declined (or in a non-interactive context — MCP server, hooks, CI), ICM stays keyword-only until you run `icm embeddings download`, and says nothing about it: an agent that only talks to `icm serve` or to the hooks never sees a prompt. To use your own runtime, set `ORT_DYLIB_PATH` to an ONNX Runtime **1.24 or newer** library (the download fetches 1.29.0).

**After an update from 0.10.63 or older on Linux (archive, `.deb`, `icm upgrade`, Homebrew) or on an Intel Mac:** those releases bundled the runtime, the current ones do not, so the update turns semantic search off until you run `icm embeddings download` once (on an Intel Mac: until you set `ORT_DYLIB_PATH`). `install.sh`, the `.deb` and Homebrew print a reminder when they install such a build; `icm upgrade` does not, so run `icm embeddings status` after it.

Why the Linux archives no longer bundle the runtime: the prebuilt ONNX Runtime that the embedding stack links statically now needs glibc 2.38, which would leave Debian 12 and Ubuntu 22.04 without a working binary. The on-demand runtime only needs glibc 2.28.

## Setup

```bash
# Auto-detect and configure all supported tools (global database)
icm init

# Per-project database (stores memories in .icm/memories.db)
icm init --per-project
```

`--per-project` creates a project-local `.icm/config.toml` at the git
root, so all `icm` commands run from within the project automatically
use an isolated database. Combine with global `icm init` — global
settings (tools, hooks) are unaffected; only the database is scoped.

The default (`standard`) sets up instructions, skills and hooks, without an MCP server. `icm init --mode all` also registers the MCP server, which is how the 18 tools below are covered ([full integration guide](integrations.md)):

| Tool | MCP | Hooks | CLI | Skills |
|------|:---:|:-----:|:---:|:------:|
| Claude Code | `~/.claude.json` | 6 hooks | `CLAUDE.md` | `/recall` `/remember` |
| Claude Desktop | JSON | — | — | — |
| Gemini CLI | `~/.gemini/settings.json` | 5 hooks | `GEMINI.md` | — |
| Codex CLI | `~/.codex/config.toml` | 3 hooks (PostToolUse opt-in, see #288) | `AGENTS.md` | — |
| Copilot CLI | `~/.copilot/mcp-config.json` | 4 hooks | `.github/copilot-instructions.md` | — |
| Cursor | `~/.cursor/mcp.json` | — | — | `.mdc` rule |
| Windsurf | JSON | — | `.windsurfrules` | — |
| VS Code | `~/Library/.../Code/User/mcp.json` | — | — | — |
| Amp | JSON | — | — | `/icm-recall` `/icm-remember` |
| Amazon Q | JSON | — | — | — |
| Cline | VS Code globalStorage | — | — | — |
| Roo Code | VS Code globalStorage | — | — | `.md` rule |
| Kilo Code | VS Code globalStorage | — | — | — |
| Zed | `~/.zed/settings.json` | — | — | — |
| OpenCode | JSON | TS plugin | — | `icm-recall` `icm-remember` `icm-remember-session` |
| Continue.dev | `~/.continue/config.yaml` | — | — | — |
| Aider | — | — | `.aider.conventions.md` | — |
| Pi | — | TS ext (TBD) | `~/.pi/agent/AGENTS.md` | `/icm-recall` `/icm-remember` |

Or manually:

```bash
# Claude Code
claude mcp add icm -- icm serve

# Compact mode (shorter responses, saves tokens)
claude mcp add icm -- icm serve --compact

# Any MCP client: command = "icm", args = ["serve"]
```

### Skills / rules

```bash
icm init --mode skill
```

Installs slash commands and rules for Claude Code (`/recall`, `/remember`), Cursor (`.mdc` rule), Roo Code (`.md` rule), and Amp (`/icm-recall`, `/icm-remember`).

### CLI instructions

```bash
icm init --mode cli
```

Injects ICM instructions into each tool's instruction file:

| Tool | File |
|------|------|
| Claude Code | `CLAUDE.md` |
| GitHub Copilot | `.github/copilot-instructions.md` |
| Windsurf | `.windsurfrules` |
| OpenAI Codex | `AGENTS.md` |
| Gemini | `~/.gemini/GEMINI.md` |

### Hooks (5 tools)

```bash
icm init --mode hook
```

Installs auto-extraction and auto-recall hooks for all supported tools:

| Tool | SessionStart | PreTool | PostTool | Compact | PromptRecall | Config |
|------|:-----------:|:-------:|:--------:|:-------:|:------------:|--------|
| Claude Code | `icm hook start` | `icm hook pre` | `icm hook post` | `icm hook compact` | `icm hook prompt` | `~/.claude/settings.json` |
| Gemini CLI | `icm hook start` | `icm hook pre` | `icm hook post` | `icm hook compact` | `icm hook prompt` | `~/.gemini/settings.json` |
| Codex CLI | `icm hook start` | `icm hook pre` | `icm hook post`¹ | — | `icm hook prompt` | `~/.codex/hooks.json` |
| Copilot CLI | `icm hook start` | `icm hook pre` | `icm hook post` | — | `icm hook prompt` | `~/.copilot/settings.json` |
| OpenCode | session start | — | tool extract | compaction | — | `~/.config/opencode/plugins/icm.ts` |

**What each hook does:**

| Hook | What it does |
|------|-------------|
| `icm hook start` | Inject a wake-up pack of critical/high memories at session start (~500 tokens) |
| `icm hook pre` | Auto-allow `icm` CLI commands (no permission prompt) |
| `icm hook post` | Extract facts from tool output every N calls (auto-extraction) |
| `icm hook compact` | Extract memories from transcript before context compression |
| `icm hook prompt` | Inject recalled context at the start of each user prompt |

¹ **Codex CLI PostToolUse is off by default.** Codex fires PostToolUse on every shell command — a session generates ~14k events / 24h, which floods the store with tool-output bloat (issue #288). Opt in with `icm init --with-codex-post-hook` if you want it; tune `[extraction]` first (`extract_every`, `min_score`, `store_raw = false`). MCP + `AGENTS.md` alone still let Codex save via the `icm_memory_store` tool.

## CLI vs MCP

ICM can be used via CLI (`icm` commands) or MCP server (`icm serve`). Both access the same database.

| | CLI | MCP |
|---|-----|-----|
| **Token cost** | the context the hooks inject | the tool schemas, plus what the agent asks for |
| **Setup** | `icm init --mode hook` | `icm init --mode mcp` |
| **Works with** | Claude Code, Gemini, Codex, Copilot, OpenCode (via hooks) | The 16 tools with an MCP column in the setup table |
| **Auto-extraction** | Yes (hooks trigger `icm extract`) | Yes (MCP tools call store) |
| **Best for** | Power users, token savings | Universal compatibility |

## HTTP API (warm model)

```bash
# Persistent local server — embedding model loads once, stays warm.
icm serve --http 127.0.0.1:11435 --db ~/.local/share/icm/memories.db &

curl -s -X POST 127.0.0.1:11435/store \
  -H 'content-type: application/json' \
  -d '{"topic":"t","content":"hello world","keywords":"x"}'

# TOON by default (lowest token cost on LLM-side reads).
curl -s -X POST 127.0.0.1:11435/recall \
  -H 'content-type: application/json' \
  -d '{"query":"hello","topic":"t","limit":5}'

# JSON variant: ?format=json or Accept: application/json
curl -s -X POST '127.0.0.1:11435/recall?format=json' \
  -H 'content-type: application/json' \
  -d '{"query":"hello","topic":"t"}'
```

Endpoints: `POST /store`, `POST /recall`, `POST /consolidate`, `GET /stats`, `GET /topics`, `GET /health`. Optional `--token <T>` enables `Authorization: Bearer <T>` on every request (health stays open as a liveness probe). Bound to whatever address you pass; `127.0.0.1:<port>` keeps the server localhost-only.

The embedding model is loaded once instead of on every CLI call, so any scripting language can hit semantic recall with plain `curl`. Requires the `http-api` feature (enabled by default). Issue [#290](https://github.com/rtk-ai/icm/issues/290).

## Dashboard

```bash
icm dashboard    # or: icm tui
```

Interactive TUI with 6 tabs: Overview, Topics, Memories, Health, Memoirs, Graph. Keyboard navigation (vim-style: j/k, g/G, Tab, 1-6), live search (/), auto-refresh.

Requires the `tui` feature (enabled by default). Build without: `cargo install --path crates/icm-cli --no-default-features --features embeddings-static,backend-sqlite,http-api`.

## CLI

### Memories (episodic, with decay)

```bash
# Store
icm store -t "my-project" -c "Use PostgreSQL for the main DB" -i high -k "db,postgres"

# Recall
icm recall "database choice"
icm recall "auth setup" --topic "my-project" --limit 10
icm recall "architecture" --keyword "postgres"

# Manage
icm forget <memory-id>
icm consolidate --topic "my-project"
icm topics
icm stats

# Extract facts from text (rule-based, zero LLM cost)
echo "The parser uses Pratt algorithm" | icm extract -p my-project
```

### Memoirs (permanent knowledge graphs)

```bash
# Create a memoir
icm memoir create -n "system-architecture" -d "System design decisions"

# Add concepts with labels
icm memoir add-concept -m "system-architecture" -n "auth-service" \
  -d "Handles JWT tokens and OAuth2 flows" -l "domain:auth,type:service"

# Link concepts
icm memoir link -m "system-architecture" --from "api-gateway" --to "auth-service" -r depends-on

# Search with label filter
icm memoir search -m "system-architecture" "authentication"
icm memoir search -m "system-architecture" "service" --label "domain:auth"

# Inspect neighborhood
icm memoir inspect -m "system-architecture" "auth-service" -D 2

# Export graph (formats: json, dot, ascii, ai)
icm memoir export -m "system-architecture" -f ascii   # Box-drawing with confidence bars
icm memoir export -m "system-architecture" -f dot      # Graphviz DOT (color = confidence level)
icm memoir export -m "system-architecture" -f ai       # Markdown optimized for LLM context
icm memoir export -m "system-architecture" -f json     # Structured JSON with all metadata

# Generate SVG visualization
icm memoir export -m "system-architecture" -f dot | dot -Tsvg > graph.svg
```

### Transcripts (verbatim session replay)

Store every message exchanged with an agent as-is — no summarization, no extraction.
Search later with FTS5 (BM25 + boolean + phrase + prefix). Useful for session replay,
post-mortem review, compliance audit, training data. Complementary to curated memories.

```bash
# 1. Start a session
SID=$(icm transcript start-session --agent claude-code --project myapp)

# 2. Record every turn verbatim
icm transcript record -s "$SID" -r user      -c "Pourquoi on avait choisi Postgres ?"
icm transcript record -s "$SID" -r assistant -c "JSONB natif, BRIN pour les logs, auto-vacuum tuné."
icm transcript record -s "$SID" -r tool      -c '{"cmd":"psql -c ..."}' -t Bash --tokens 42

# 3. Replay, search, inspect
icm transcript list-sessions --project myapp
icm transcript show "$SID" --limit 200
icm transcript search "postgres JSONB"                    # BM25 ranked
icm transcript search '"auto-vacuum"'                     # phrase match
icm transcript search "postgres OR mysql" --session "$SID" # boolean, scoped
icm transcript stats

# 4. Delete a session (cascade deletes its messages)
icm transcript forget "$SID"
```

Rust + SQLite + FTS5, no external service: the whole transcript lives in the same SQLite file as your
memories and memoirs.

## MCP Tools (31)

### Memory tools

| Tool | Description |
|------|-------------|
| `icm_memory_store` | Store, merging into a near-identical memory of the same topic (cosine above 0.95, with embeddings) |
| `icm_memory_recall` | Search by query, filter by topic / keyword / project |
| `icm_memory_update` | Edit a memory in-place (content, importance, keywords) |
| `icm_memory_forget` | Delete a memory by ID |
| `icm_memory_forget_topic` | Delete all memories in a given topic |
| `icm_memory_consolidate` | Replace the memories you list (`ids`) with your summary; without `ids`, lists the topic |
| `icm_memory_extract_patterns` | Detect recurring patterns within a topic and surface them as concepts |
| `icm_memory_list_topics` | List all topics with counts |
| `icm_memory_stats` | Global memory statistics |
| `icm_memory_health` | Per-topic hygiene audit (staleness, consolidation needs) |
| `icm_memory_embed_all` | Backfill embeddings for vector search |

### Session tools

| Tool | Description |
|------|-------------|
| `icm_wake_up` | Build a project-scoped wake-up pack (critical/high memories + preferences) for SessionStart-style context injection |
| `icm_learn` | Scan a project directory and seed a Memoir knowledge graph from its code/docs |

### Memoir tools (knowledge graphs)

| Tool | Description |
|------|-------------|
| `icm_memoir_create` | Create a new memoir (knowledge container) |
| `icm_memoir_list` | List all memoirs |
| `icm_memoir_show` | Show memoir details and all concepts |
| `icm_memoir_add_concept` | Add a concept with labels |
| `icm_memoir_refine` | Update a concept's definition |
| `icm_memoir_search` | Full-text search, optionally filtered by label |
| `icm_memoir_search_all` | Search across all memoirs |
| `icm_memoir_link` | Create typed relation between concepts |
| `icm_memoir_inspect` | Inspect concept and graph neighborhood (BFS) |
| `icm_memoir_export` | Export graph (json, dot, ascii, ai) with confidence levels |

### Feedback tools (learning from mistakes)

| Tool | Description |
|------|-------------|
| `icm_feedback_record` | Record a correction when an AI prediction was wrong |
| `icm_feedback_search` | Search past corrections to inform future predictions |
| `icm_feedback_stats` | Feedback statistics: total count, breakdown by topic, most applied |

### Transcript tools (verbatim session replay)

| Tool | Description |
|------|-------------|
| `icm_transcript_start_session` | Create a session for verbatim message capture; returns `session_id` |
| `icm_transcript_record` | Append a raw message (role, content, optional tool + tokens + metadata) |
| `icm_transcript_search` | FTS5 search across messages (BM25, boolean, phrase, prefix) |
| `icm_transcript_show` | Replay full message thread of a session, chronologically |
| `icm_transcript_stats` | Sessions, messages, bytes, breakdown by role/agent/top-sessions |

### Relation types

`part_of` · `depends_on` · `related_to` · `contradicts` · `refines` · `alternative_to` · `caused_by` · `instance_of` · `superseded_by`

## How it works

ICM gives your AI agent a real memory — not a note-taking tool, not a context manager, a **memory**.

```
                       ICM (Infinite Context Memory)
            ┌──────────────────────┬──────────────────────────┐
            │   MEMORIES (Topics)  │   MEMOIRS (Knowledge)    │
            │                      │                          │
            │  Episodic, temporal  │  Permanent, structured   │
            │                      │                          │
            │  ┌───┐ ┌───┐ ┌───┐   │    ┌───┐                 │
            │  │ m │ │ m │ │ m │   │    │ C │──depends_on──┐  │
            │  └─┬─┘ └─┬─┘ └─┬─┘   │    └───┘              │  │
            │    │decay│     │     │      │ refines        │  │
            │    ▼     ▼     ▼     │    ┌─▼─┐            ┌─▼─┐│
            │  weight decreases    │    │ C │──part_of──>│ C ││
            │  over time unless    │    └───┘            └───┘│
            │  accessed/critical   │  Concepts + Relations    │
            ├──────────────────────┴──────────────────────────┤
            │          SQLite + FTS5 + sqlite-vec             │
            │  Hybrid recall: BM25 + vectors + dates (RRF)    │
            └─────────────────────────────────────────────────┘
```

**Three kinds of memory:**

- **Memories** — store/recall with temporal decay by importance. Critical memories never fade, low-importance ones decay naturally. Filter by topic or keyword.
- **Memoirs** — permanent knowledge graphs. Concepts linked by typed relations (`depends_on`, `contradicts`, `superseded_by`, ...). Filter by label.
- **Feedback** — record corrections when AI predictions are wrong. Search past mistakes before making new predictions. Closed-loop learning.

### Dual memory model

**Episodic memory (Topics)** captures decisions, errors, preferences. Each memory has a weight that decays over time based on importance:

| Importance | Decay | Prune | Behavior |
|-----------|-------|-------|----------|
| `critical` | none | never | Never forgotten, never pruned |
| `high` | slow (0.5x rate) | never | Fades slowly, never auto-deleted |
| `medium` | normal | yes | Standard decay, pruned when weight < threshold |
| `low` | fast (2x rate) | yes | Quickly forgotten |

Decay is **access-aware**: frequently recalled memories decay slower (`decay / (1 + min(access_count, 5) × 0.1)`). Applied automatically on recall (if >24h since last decay).

**Memory hygiene** is built-in:
- **Auto-dedup**: with an embedding model loaded, storing content whose cosine similarity to a memory in the same topic exceeds 0.95 merges it into that memory instead of creating a duplicate
- **Consolidation hints**: when a topic exceeds 7 entries, `icm_memory_store` warns the caller to consolidate
- **Health audit**: `icm_memory_health` reports per-topic entry count, average weight, stale entries, and consolidation needs
- **No silent data loss**: critical and high-importance memories are never auto-pruned

**Semantic memory (Memoirs)** captures structured knowledge as a graph. Concepts are permanent — they get refined, never decayed. Use `superseded_by` to mark obsolete facts instead of deleting them.

### Hybrid search

Recall fuses up to three ranked lists by reciprocal rank (RRF):
- **FTS5 BM25** — full-text keyword matching, always on
- **Cosine similarity** — semantic vector search via sqlite-vec, when an embedding model is loaded
- **Date window** — when the query names a period ("last week", "in March 2024")

Project, topic and keyword filters apply before the cut. Without an embedding model recall is keyword-only; on LoCoMo that is as good as the hybrid (see [Benchmark comparison](../README.md#benchmark-comparison)).

Default model: `Qdrant/multilingual-e5-large-onnx` (1024d, 100+ languages). Configurable in your [config file](#configuration):

```toml
[embeddings]
# enabled = false                          # Disable entirely (no model download)
# model = "intfloat/multilingual-e5-base"  # 768d, multilingual (lighter)
# model = "intfloat/multilingual-e5-small" # 384d, multilingual (lightest)
# model = "Xenova/bge-small-en-v1.5"       # 384d, English-only (fastest)
# model = "jinaai/jina-embeddings-v2-base-code"  # 768d, code-optimized
```

To skip the embedding model download entirely, use any of these:
```bash
icm --no-embeddings serve          # CLI flag
ICM_NO_EMBEDDINGS=1 icm serve     # Environment variable
```
Or set `enabled = false` in your config file.

The model that produced the stored vectors is recorded in the database and wins over the config file, so editing `model` never clears anything. To change model, run `icm embed --migrate`: it writes a backup, resets the vector index at the new dimension and re-embeds every memory.

### Storage

Single SQLite file. No external services, no network dependency.

Default (global) database location:

```
~/Library/Application Support/dev.icm.icm/memories.db                    # macOS
~/.local/share/icm/memories.db                                           # Linux
C:\Users\<user>\AppData\Roaming\icm\icm\data\memories.db                 # Windows
```

Per-project database (created by `icm init --per-project`):

```
<project-root>/.icm/memories.db
```

ICM auto-detects a project-local `.icm/config.toml` from the current
working directory. A relative `[store].path` is resolved against the
git root, so all `icm` commands within the project tree use the
scoped database without needing `--db` on every invocation.

### Configuration

```bash
icm config                    # Show active config
```

Config file location (platform-specific, or `$ICM_CONFIG`):

```
~/Library/Application Support/dev.icm.icm/config.toml                    # macOS
~/.config/icm/config.toml                                                # Linux
C:\Users\<user>\AppData\Roaming\icm\icm\config\config.toml              # Windows
```

See [config/default.toml](../config/default.toml) for all options.

## Multi-project & multi-agent

ICM is built for the case where one user collaborates with many agents across many projects. Memories must stay relevant: a decision from project A should never leak into project B, and a `dev` agent should not be hydrated with what a `mentor` agent stored.

### Project isolation

ICM scopes memories by **topic naming convention**, not by a separate column. The convention:

```
{kind}-{project}              # e.g. decisions-icm, errors-resolved-icm, contexte-rtk-cloud
preferences                   # global, always included
identity                      # global, always included
```

`icm_wake_up { project: "icm" }` does **segment-aware** matching: `"icm"` matches `decisions-icm`, `errors-icm-core`, `contexte-icm` — but never `icmp-notes` (no false positives). Topics are split on `-`, `.`, `_`, `/`, `:`. Preference and identity topics are cross-project by design — user-level guidance is never stripped.

The `UserPromptSubmit` hook (`icm hook prompt`) and the `SessionStart` hook (`icm hook start`) both derive the project from the `cwd` field in the hook JSON (`basename` of the working directory). Run each project from its own directory and isolation is automatic.

### Writing good memories

`icm_memory_store` requires the agent to choose `topic` and `content` — there is no auto-classifier. Best practice:

| Field | Guidance |
|------|----------|
| `topic` | `{kind}-{project}`. Kinds: `decisions`, `errors-resolved`, `contexte`, `preferences`. |
| `content` | One fact per store. Dense English summary — `topic + content` is the embedding text. |
| `raw_excerpt` | Verbatim only (code, exact error message, command output). |
| `keywords` | 3–5 terms to boost BM25 retrieval. |
| `importance` | `critical` for never-forget, `high` for project decisions, `medium` default, `low` for ephemeral. |

ICM handles the rest: **dedup above 0.95 cosine similarity (with embeddings)**, **auto-link** between semantically close memories, **auto-consolidation** above 10 entries per topic (off by default), and **decay** weighted by access count. One fact per call beats batched dumps — the retriever ranks individually-stored facts higher.

### Multi-agent roles

ICM does not yet have a first-class `role` column. Today, roles are emulated by topic suffixes plus per-agent working directories:

```
decisions-icm-dev             # dev agent: code patterns, library choices, refactors
decisions-icm-architect       # architect: design, workflows, subtask decomposition
decisions-icm-mentor          # mentor / BA: business goals, non-technical context
```

Each agent runs in its own working directory (`~/projects/icm-dev/`, `~/projects/icm-architect/`, ...) so that `icm hook prompt` and `icm hook start` derive a different project segment from `cwd` and only recall the matching memories. Preferences remain global — user identity carries across all roles.

Within a single agent, you can also narrow recall manually:

```jsonc
// icm_memory_recall
{ "query": "auth flow", "topic": "decisions-icm-architect", "limit": 5 }
```

A first-class `role` field (with native filtering in wake-up and recall) is on the roadmap. Until then, the topic-suffix convention is the supported pattern.

## Auto-extraction

ICM captures and reinjects memories through three hooks. Capture is rule-based; when an LLM command-line tool is installed (Claude Code, Codex or Gemini CLI), the default `[extraction.summarizer] provider = "auto"` also hands the captured text to it to write the facts (`provider = "none"` keeps everything local):

```
  Layer 0: Pattern hooks              Layer 1: PreCompact           Layer 2: UserPromptSubmit
  (rules; your LLM CLI if auto)       (rules, no LLM call)          (no LLM call)  
  ┌──────────────────┐                ┌──────────────────┐          ┌──────────────────┐
  │ PostToolUse hook  │                │ PreCompact hook   │          │ UserPromptSubmit  │
  │                   │                │                   │          │                   │
  │ • Bash errors     │                │ Context about to  │          │ User sends prompt │
  │ • git commits     │                │ be compressed →   │          │ → icm recall      │
  │ • config changes  │                │ extract memories  │          │ → inject context  │
  │ • decisions       │                │ from transcript   │          │                   │
  │ • preferences     │                │ before they're    │          │ Agent starts with  │
  │ • learnings       │                │ lost forever      │          │ relevant memories  │
  │ • constraints     │                │                   │          │ already loaded     │
  │                   │                │ Same patterns +   │          │                   │
  │ Rule-based, no LLM│                │ --store-raw fallbk│          │                   │
  └──────────────────┘                └──────────────────┘          └──────────────────┘
```

| Layer | Status | LLM call | Hook command | Description |
|-------|--------|----------|-------------|-------------|
| Layer 0 | Implemented | only with `provider = "auto"` and an LLM CLI installed | `icm hook post` | Rule-based extraction from tool output |
| Layer 1 | Implemented | none | `icm hook compact` | Extract from transcript before context compression |
| Layer 2 | Implemented | none | `icm hook prompt` | Inject recalled memories on each user prompt (recall, not extraction) |

All 3 layers are installed automatically by `icm init --mode hook`.

