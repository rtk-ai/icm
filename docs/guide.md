# ICM User Guide

## What is ICM?

ICM gives your AI coding agent a persistent memory that survives across sessions. Without ICM, every time a session ends or the context window compacts, the agent forgets everything — your architecture decisions, resolved bugs, project conventions. With ICM, it remembers.

## Quick Start

### 1. Install

```bash
# Homebrew
brew tap rtk-ai/tap && brew install icm

# Quick install (macOS / Linux) — verifies SHA256 against the release checksums
curl -fsSL https://raw.githubusercontent.com/rtk-ai/icm/main/install.sh | sh

# Quick install (Windows PowerShell)
irm https://raw.githubusercontent.com/rtk-ai/icm/main/install.ps1 | iex

# From source
cargo install --path crates/icm-cli
```

Re-running the install command upgrades an existing installation in place.

### 2. Setup

```bash
# Global setup — one database for all projects
icm init

# Per-project setup — isolated database per project
icm init --per-project
```

`--per-project` creates `.icm/config.toml` at the project's git root. All `icm` commands within that project tree automatically use the project-local database, without needing `--db` on every invocation. The database path is resolved hierarchically:

| Priority | Source | Example |
|----------|--------|---------|
| 1 | `--db` CLI flag | `icm --db /tmp/test.db store ...` |
| 2 | `ICM_DB` env var | `ICM_DB=/tmp/test.db icm recall ...` |
| 3 | Global config | `~/.config/icm/config.toml [store].path` |
| 4 | `.icm/config.toml` | `<project>/.icm/config.toml [store].path` |
| 5 | `.icm/memories.db` | Auto-detected at git root (if file exists) |
| 6 | Platform default | `~/.local/share/icm/memories.db` |

Run `icm config` to see the active resolution chain with source tracing.

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

# As many of the best matches as fit in 2000 tokens of output
icm recall "deploy pipeline" --max-tokens 2000
```

**Recall engine.** Recall ranks with the v2 engine: a full-text arm (BM25), a vector arm when embeddings are enabled, and a date arm when the query names a period ("last week", "in March 2024"), fused by rank; `--topic`, `--keyword` and `--project` are applied before results are cut. `--limit` cuts by count (default 5). `--max-tokens N` cuts by a token budget instead, estimated at four characters per token on the output format you chose, with `--limit` as a ceiling on the count.

The engine before v2 is still available, to roll back or to compare: `icm recall --engine legacy`, or `ICM_RECALL_ENGINE=legacy` in the environment (this also switches the MCP tool, the HTTP API and the prompt hook). It has no token budget: `--max-tokens` with `legacy` is an error.

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

#### Summarizer providers: bring your own LLM, or your own token

By default (`provider = "none"`) consolidation is a lexical join. A real summary needs an LLM, selected with `--summarizer-provider` or in `config.toml` (`[consolidate.summarizer]` for `consolidate` / `briefing`, `[extraction.summarizer]` for the hook extraction queue):

- `claude`, `codex`, `gemini` — shell out to the CLI you are already logged into (`claude -p`, `codex exec`, `gemini -p`). No key, your existing quota.
- `ollama` — local Ollama daemon (`OLLAMA_HOST`), `model` required.
- `anthropic`, `openai`, `google` — call the vendor API directly with a key, for servers, CI and containers where no CLI is installed. `openai` also covers any OpenAI-compatible server (OpenRouter, Mistral, Groq, vLLM, LM Studio, …) through `base_url`.
- `auto` — pick a CLI from the environment of the invoking tool. It never selects `anthropic` / `openai` / `google`: an API key in the environment is not spent unless you name the provider.
- `none` — no LLM.

```toml
[consolidate.summarizer]
provider = "anthropic"        # key from $ANTHROPIC_API_KEY
# model = "claude-haiku-4-5"  # default: the provider's low-cost model
# workspace_id = "wrkspc_…"   # only for a key not scoped to one workspace

[extraction.summarizer]
provider = "openai"
base_url = "https://openrouter.ai/api/v1"   # any OpenAI-compatible endpoint
api_key_env = "OPENROUTER_API_KEY"          # the variable's NAME, not the key
model = "mistralai/mistral-small"           # required with a third-party base_url
```

**The key.** It is read from an environment variable, never from the config file, and sent in a header to the host in `base_url` only (redirects are not followed). On the vendor's own host the variable defaults to `ANTHROPIC_API_KEY`, `OPENAI_API_KEY` or `GEMINI_API_KEY`. On any other `base_url` nothing is sent unless `api_key_env` names a variable: a local server (LM Studio, vLLM) needs no key and no placeholder, and your real OpenAI key is never shipped to it by default. A gateway in front of a vendor needs `api_key_env` spelled out. `base_url` is `scheme://host[:port]/path` and nothing else: a query string, a fragment, credentials, or a host that is not a plain name or IP address is refused, so that the host ICM checks is the host the request goes to. `http://` is refused for the vendors' hosts and flagged for any other non-loopback host (the key and the text travel unencrypted). `icm config` shows the variable, whether it is set, and that warning — never the key. `RUST_LOG` cannot make the HTTP client print request headers.

**A provider flag and the config.** `api_key_env`, `base_url`, `workspace_id` and `model` apply to the provider the config names. `--summarizer-provider` / `--provider` with a different provider ignores them (and says so): that provider runs on its own defaults, with `--summarizer-model` / `--model` if given. One exception: when the config names no provider (`none` or `auto`), its `model` is used for whichever provider the flag picks — `model = "qwen3:8b"` plus `--summarizer-provider ollama` keeps working.

**Models.** Defaults when `model` is empty: `claude-haiku-4-5` (anthropic), `gpt-6-luna` (openai, sent with `reasoning_effort: none`), `gemini-3.5-flash-lite` (google) — each vendor's current low-cost model, not necessarily the cheapest ID on its price list: older or deprecated models may cost less, set `model` to use one. OpenAI's regional endpoints (`https://eu.api.openai.com/v1`, …) count as OpenAI. `max_tokens` is the approximate summary budget; the hard cap sent to the API is 4x that, with a minimum of 8192 on a vendor's own API (at most 64000 for anthropic) so reasoning models have room to answer, and of 2048 on any other `base_url`. A compatible server with a small context window (vLLM or TGI serving an 8K model) that rejects even that gets one retry at exactly 4x `max_tokens`: lower `max_tokens` to fit it. An Anthropic key that spans several workspaces needs `workspace_id`.

**When the provider fails.** Rate limits and outages (429, 5xx, dropped connections) are retried twice within `timeout_secs`, honoring `retry-after`. After that — or at once for a missing key, 401/403, a spent quota, a redirect, a timeout, an empty, truncated or unfinished answer — the error says what to fix, and no memory is removed that was not summarized:

- `consolidate`, `consolidate-all`, `consolidate-pending` and the dashboard report the failure and leave in place everything the provider did not summarize. A topic that takes several passes keeps the passes that succeeded: the error says how many memories were already folded into a partial summary. `icm consolidate -t <topic> --keep-originals` adds a lexical join next to the originals instead of waiting for the provider. This holds for the CLI providers too.
- `consolidate-all` goes on to the next topic after one failure (it may be that topic's own: an answer cut short, a content filter) and stops after two in a row. It exits non-zero when a topic failed, and zero otherwise — including when a topic could only be folded in part.
- `consolidate-pending` marks the job `failed` and leaves the rest of the queue pending. A topic is queued at most once.
- `extract-pending` hands the affected rows to the local extractor.
- `briefing` fails.

An answer only counts when the provider says it finished normally: `end_turn` / `stop_sequence` (anthropic), `STOP` (google), `stop` and its equivalents (openai format), `done: true` with `done_reason: "stop"` (Ollama; older servers that send no `done_reason` are accepted unless the answer used the whole output cap). One cut by the output cap or the context window, stopped for any other reason, or with no finish reason at all, is discarded. On an OpenAI-compatible server a leading `<think>…</think>` scratchpad is removed from the answer, unless the memories being summarized mention such tags or a tag is left after it; an answer with a closing `</think>` and no opening tag is always refused: reasoning cannot be told from a quotation of the tag, and cutting there would store half a summary. Have the server separate the reasoning (vLLM `--reasoning-parser`, Groq `reasoning_format`), or use a model that does not emit it.

Ollama is asked for a context window sized on the prompt (`num_ctx`) and an output cap of 4x `max_tokens`, minimum 2048 (`num_predict`): its default window is smaller than a full consolidation prompt and it would otherwise drop the start of it without saying so. If the window still fills up, the answer is discarded.

**What a consolidation removes.** Only the memories whose text went into the summary, in the same transaction that writes it, and only if they still say what was read:

- A memory stored while the provider was answering is kept. One edited or forgotten meanwhile makes the store refuse the summary; the pass is redone from a fresh read, so a correction is never deleted unsummarized and a forgotten memory never comes back through a summary. Two runs over the same topic do not leave two summaries.
- `critical` memories take no part: they are never removed, never sent to be summarized, and the summary takes the highest importance of what it replaces (never `critical`).
- A pass needs at least two memories; a topic with fewer is left alone, without a provider call.
- A topic too large for one pass (more than 500 memories, more text than the provider prompt takes — about 20 000 characters — or, for the lexical join, than one memory can hold — 64 KB) is folded in several passes, each summary being an entry of the next where it fits. A memory too large to share a pass is set aside and reported, and the rest of the topic is consolidated without it.

`POST /consolidate`, the web dashboard and the TUI go through the same code (the TUI does one pass per keypress and says how much is left). The MCP tool `icm_memory_consolidate` works in two steps, because the summary is the agent's: called with only `topic` it replaces nothing and lists the topic's memories with their ids; called with `summary` and the `ids` it covers, it replaces exactly those. Without `ids` it never replaces anything.

**Auto-consolidation loses content.** `auto_consolidate_enabled` (off by default) is the one exception, by design: when no LLM summarizer is configured, its rollup keeps only the summaries of the 3 heaviest memories of the topic and deletes the other non-critical memories it read. Leave it off unless that is what you want, or configure `[consolidate.summarizer]`: the topic is then queued for a real summary (`icm consolidate-pending`) instead, on the CLI, hook and MCP store paths alike. It never removes memories it did not read.

**Where your text goes.** With an API-key provider, the text being summarized is sent to that provider under its data terms: memory summaries for `consolidate` / `briefing`, and for `[extraction.summarizer]` the queued tool output as captured (up to 8 KB per entry, not redacted). For `google` in particular: with a key from a project without billing (free tier), Google's Gemini API terms say prompts and responses are used to improve its products and may be read by human reviewers, and that sensitive or confidential data must not be submitted; this does not apply in the EEA, Switzerland and the UK, nor to a key from a billing-enabled project. Use a billed project before pointing `[extraction.summarizer]` at `google` — see https://ai.google.dev/gemini-api/terms. The same goes for the `gemini` CLI signed into a free account.

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

The prompt hook runs the same search on every message, on the default recall engine with keywords only: it never loads the embedding model and never writes to the database. `ICM_RECALL_ENGINE=legacy` switches it back to the previous search.

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

The model that produced the stored vectors is recorded in the database and wins over the config file: editing `model` clears nothing. To change model, run `icm embed --migrate` (backup, new index, every memory re-embedded). Other uses of `icm embed`:

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
| `icm_memory_recall` | Search by query. Filters: `topic`, `keyword`, `project`, `limit` (1 to 20). `max_tokens` returns as many of the best matches as fit in a token budget instead. Auto-decay if >24h. |
| `icm_memory_update` | Edit content, importance, or keywords of an existing memory by ID. |
| `icm_memory_forget` | Delete a memory by ID. |
| `icm_memory_consolidate` | Replace the memories you list (`ids`) with a summary you wrote from them. Called with only `topic`, it replaces nothing and lists the topic's memories with their ids. |
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
Linux:   ~/.local/share/icm/memories.db
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

## The First 5 Minutes with ICM

A quick guide to get you up and running in 5 minutes.

### Minute 1: Install

```bash
brew tap rtk-ai/tap && brew install icm
```

Or, without Homebrew:

```bash
curl -fsSL https://raw.githubusercontent.com/rtk-ai/icm/main/install.sh | sh
```

### Minute 2: Configure

```bash
icm init
```

ICM automatically detects your AI tools (Claude Code, Cursor, VS Code, etc.) and configures the MCP server for each one. Check the output: each tool shows `configured` or `already configured`.

### Minute 3: Store a First Memory

```bash
icm store -t "test" -c "My first ICM memory" -i high
```

Verify that it was stored:

```bash
icm topics
icm stats
```

### Minute 4: Recall a Memory

```bash
icm recall "first memory"
```

The memory should appear with its ID, topic, weight, and content.

### Minute 5: Test with Your Agent

Restart your AI tool (Claude Code, Cursor...) and ask it:

> "Recall the ICM context"

The agent should automatically use `icm_memory_recall`. If it doesn't, see Detailed Troubleshooting below.

### What's Next?

- Store architecture decisions with `-i high`
- Store invariant facts (ports, URLs) with `-i critical`
- After each bug fix, store the resolution with keywords
- To go further: create a **memoir** to structure knowledge as a graph

---

## Detailed Troubleshooting

### 1. The agent doesn't use the ICM tools

**Symptom:** The agent neither recalls nor stores anything, even when asked.

**Solutions:**
- Run `icm init` and check the output for each tool
- Check that the MCP config file exists (e.g. `~/.claude.json` for Claude Code)
- Test the server manually:
  ```bash
  echo '{"jsonrpc":"2.0","id":1,"method":"initialize"}' | icm serve
  ```
  You should see a JSON response with `capabilities` and `serverInfo`
- Check that `icm serve` is in your PATH: `which icm`
- **Restart your AI tool** after running `icm init`

### 2. `icm recall` returns nothing

**Symptom:** The search returns "No memories found."

**Solutions:**
- `icm topics` — check that there are stored memories
- `icm stats` — check the total
- Try a broader query or remove the topic/keyword filters
- `icm list --all` — list everything to check the content
- If memories exist but don't match: backfill the embeddings with `icm embed`

### 3. Embeddings are slow on first launch

**Symptom:** ICM takes 30+ seconds on the first `store` or `recall`.

**Explanation:** The embedding model (about 2 GB for Qdrant/multilingual-e5-large-onnx, the default) is downloaded on first use. Subsequent runs load from the cache (~1-2s).

**Solutions:**
- This is normal the first time — wait for the download
- To speed things up, use a lighter model in `config.toml`:
  ```toml
  [embeddings]
  model = "Xenova/bge-small-en-v1.5"  # 384d, English only, the fastest
  ```
- To build without embeddings: `cargo build --no-default-features`

### 4. Duplicate memories appear

**Symptom:** Several nearly identical memories in the same topic.

**Explanation:** Auto-dedup only works through MCP (the server with an embedder). The CLI `icm store` has no auto-dedup by default.

**Solutions:**
- Backfill the embeddings: `icm embed`
- Delete duplicates manually: `icm forget <id>`
- Consolidate the topic: `icm consolidate -t <topic>`

### 5. "embeddings feature not enabled" error

**Symptom:** `icm embed` fails with a message about the feature.

**Solution:** Rebuild with the embeddings feature:
```bash
cargo build --release  # The "embeddings" feature is on by default
```

If you use the precompiled binary from the GitHub releases, embeddings are always included.

### 6. Database corruption

**Symptom:** `icm stats` or `icm recall` fails with a SQLite error.

**Solutions:**
- Locate the database:
  - macOS: `~/Library/Application Support/dev.icm.icm/memories.db`
  - Linux: `~/.local/share/icm/memories.db`
- Back up the `.db` file and its WAL files (`.db-wal`, `.db-shm`)
- Delete and rebuild if needed — the migration is automatic
- To test with a clean database: `icm --db /tmp/test.db stats`

### 7. `icm init` doesn't detect my tool

**Symptom:** The tool doesn't appear in the `icm init` output.

**Solutions:**
- Check that the tool is installed and that its config file exists
- For Claude Code: `~/.claude.json` must exist (it is created on first launch)
- Manual configuration: `claude mcp add icm -- icm serve`
- For unsupported tools, add it manually to their MCP config:
  ```json
  { "command": "/path/to/icm", "args": ["serve"] }
  ```

### 8. Decay is too aggressive or not aggressive enough

**Symptom:** Memories disappear too quickly, or pile up without being cleaned up.

**Solutions:**
- Adjust in `~/.config/icm/config.toml`:
  ```toml
  [memory]
  decay_rate = 0.98      # Slower (default: 0.95)
  prune_threshold = 0.05 # Lower threshold (default: 0.1)
  ```
- Use `icm prune --dry-run --threshold 0.2` to preview
- Mark important memories as `high` or `critical` to protect them

### 9. Compact mode doesn't turn on

**Symptom:** MCP responses stay long despite the configuration.

**Solutions:**
- Check `config.toml`:
  ```toml
  [mcp]
  compact = true
  ```
- Or force it with the flag: change `icm serve` to `icm serve --compact` in the MCP config
- Restart your AI tool after the change

### 10. "memoir not found" error even after creating it

**Symptom:** `icm memoir show <name>` fails right after `icm memoir create`.

**Solutions:**
- Check the exact name: `icm memoir list`
- Names are case-sensitive: `Archi` != `archi`
- Check that you're not using `--db` with a different path

### 11. Performance drops with many memories

**Symptom:** `recall` gets slow with more than 1000 memories.

**Solutions:**
- Hybrid search takes ~1ms per query for 1000 memories — this is normal
- Consolidate large topics: `icm consolidate -t <topic>`
- Prune outdated memories: `icm prune`
- Reduce the `limit` in your searches

### 12. Extraction finds nothing

**Symptom:** `icm extract` returns "No facts extracted."

**Solutions:**
- The text must contain recognized signals (architecture, error, and decision keywords)
- Test with an explicit text:
  ```bash
  echo "We decided to use PostgreSQL instead of MySQL" | icm extract --dry-run
  ```
- Adjust the threshold in `config.toml`:
  ```toml
  [extraction]
  min_score = 2.0  # Lower = more facts extracted (default: 3.0)
  ```

---

## Integration Guides by Tool

### Claude Code

**Setup:**
```bash
icm init  # Configures ~/.claude.json automatically
```

**Manual configuration:**
```bash
claude mcp add icm -- icm serve
```

**Config file:** `~/.claude.json`
```json
{
  "mcpServers": {
    "icm": {
      "command": "/path/to/icm",
      "args": ["serve"]
    }
  }
}
```

**Slash commands (optional):**
```bash
icm init --mode skill
```
Installs `/recall` and `/remember` into `~/.claude/commands/`.

**PostToolUse hook (optional):**
```bash
icm init --mode hook
```
Installs a hook that automatically extracts context after each tool call (git commit, edit, etc.).

**Compact mode recommended:** Claude Code benefits from compact mode to save tokens. Enable it in `~/.config/icm/config.toml`:
```toml
[mcp]
compact = true
```

**CLAUDE.md instructions (optional):**
```bash
icm init --mode cli
```
Adds the ICM instructions to the `CLAUDE.md` of the current project.

---

### Cursor

**Setup:**
```bash
icm init  # Configures ~/.cursor/mcp.json automatically
```

**Config file:** `~/.cursor/mcp.json`
```json
{
  "mcpServers": {
    "icm": {
      "command": "/path/to/icm",
      "args": ["serve"]
    }
  }
}
```

**Cursor rule (optional):**
```bash
icm init --mode skill
```
Creates `~/.cursor/rules/icm.mdc` with an `alwaysApply: true` rule that reminds the agent to use ICM.

**After setup:** Restart Cursor. The ICM tools appear in the MCP palette.

---

### VS Code / GitHub Copilot

**Setup:**
```bash
icm init  # Configures ~/Library/.../Code/User/mcp.json automatically
```

**Config file:**
- macOS: `~/Library/Application Support/Code/User/mcp.json`
- Linux: `~/.config/Code/User/mcp.json`

```json
{
  "servers": {
    "icm": {
      "command": "/path/to/icm",
      "args": ["serve"]
    }
  }
}
```

**Note:** VS Code uses `"servers"` instead of `"mcpServers"`. `icm init` handles this difference automatically.

---

### Windsurf

**Setup:**
```bash
icm init  # Configures ~/.codeium/windsurf/mcp_config.json automatically
```

**Config file:** `~/.codeium/windsurf/mcp_config.json`
```json
{
  "mcpServers": {
    "icm": {
      "command": "/path/to/icm",
      "args": ["serve"]
    }
  }
}
```

---

### Zed

**Setup:**
```bash
icm init  # Configures ~/.zed/settings.json automatically
```

Zed uses a different format with `context_servers`:
```json
{
  "context_servers": {
    "icm": {
      "command": {
        "path": "/path/to/icm",
        "args": ["serve"]
      },
      "settings": {}
    }
  }
}
```

---

### Amp

**Setup:**
```bash
icm init  # Configures ~/.config/amp/settings.json automatically
```

**Slash commands (optional):**
```bash
icm init --mode skill
```
Installs `/icm-recall` and `/icm-remember` into `~/.config/amp/skills/`.

---

### OpenAI Codex CLI

**Setup:**
```bash
icm init  # Configures ~/.codex/config.toml automatically
```

**Config file (TOML):** `~/.codex/config.toml`
```toml
[mcp_servers.icm]
command = "/path/to/icm"
args = ["serve"]
```

---

### Claude Desktop

**Setup:**
```bash
icm init  # Configures automatically
```

**Config file:** `~/Library/Application Support/Claude/claude_desktop_config.json`
```json
{
  "mcpServers": {
    "icm": {
      "command": "/path/to/icm",
      "args": ["serve"]
    }
  }
}
```

---

### Other tools (Gemini, Amazon Q, Cline, Roo Code, Kilo Code, OpenCode)

All of these are configured automatically by `icm init`. The format is always the same:

```json
{
  "command": "/path/to/icm",
  "args": ["serve"]
}
```

The only difference is the config file and the JSON key. `icm init` handles all of these variations.

---

## FAQ

### Q1: Does ICM send data over the internet?

**Not for storing or recalling.** ICM stores everything locally in a SQLite file, and the embedding model runs locally (via fastembed/ONNX Runtime). There are two exceptions: the initial download of the embedding model (about 2 GB for the default model, one time only), and automatic extraction, which hands the captured text to the command-line LLM tool you already have installed (Claude Code, Codex, or Gemini CLI) as long as `[extraction.summarizer] provider` is `"auto"` (the default). With `provider = "none"`, nothing leaves the machine after the model download. A provider that uses an API key (`anthropic`, `openai`, `google`) sends the summarized text to that provider, under its terms.

### Q2: Can I use ICM with several projects?

**Yes.** All projects share the same SQLite database. Use project-prefixed topics (e.g. `decisions-api`, `decisions-frontend`) to separate them. You can also use `--db <path>` to fully isolate the databases.

### Q3: How do I back up and restore my memory?

Back up the SQLite file:
```bash
# macOS
cp ~/Library/Application\ Support/dev.icm.icm/memories.db ~/backup-icm.db

# Restore
cp ~/backup-icm.db ~/Library/Application\ Support/dev.icm.icm/memories.db
```

### Q4: Does ICM work with local models (ollama)?

**Yes.** ICM is a standard MCP server. It works with any MCP client, including those that use local models. Benchmarks show up to +93% recall with qwen2.5:14b via ollama.

### Q5: What's the difference between `icm consolidate` (CLI) and `icm_memory_consolidate` (MCP)?

The CLI automatically merges the summaries (concatenated with ` | `). The MCP asks the agent to provide the summary, which gives a smarter result because the agent understands the content and can synthesize it.

### Q6: Can I change the embedding model without losing my data?

**Yes.** The memories (text) are always kept, and so are the vectors as long as you don't migrate: the model that produced the vectors is recorded in the database and takes precedence over `config.toml`. To change the model, run `icm embed --migrate`: it writes a backup, recreates the index at the new dimension, and recomputes all the vectors.

### Q7: How many memories can ICM handle?

The SQLite database handles millions of rows without trouble. Benchmarks show ~34us per store and ~951us per hybrid search for 1000 memories. Performance degrades linearly, not exponentially.

### Q8: How do I delete all my memory?

```bash
# macOS
rm ~/Library/Application\ Support/dev.icm.icm/memories.db*

# Linux
rm ~/.local/share/icm/memories.db*
```

The database is recreated automatically on the next launch.

### Q9: Does ICM consume LLM tokens?

**No, for storing and recalling.** ICM doesn't call any LLM API. The only tokens consumed are those of the agent calling the MCP tools, just like any other MCP tool. Compact mode (`--compact`) reduces these tokens by ~40%.

Extraction (Layer 0) is purely rule-based, with zero LLM cost. Layer 1 (PreCompact, planned) will use ~500 tokens per session.

### Q10: Can I share my memory with my team?

Not directly, since it's a local SQLite file. However, **memoirs** are designed to capture structured knowledge that can be exported and shared. An import/export feature is planned.

### Q11: Is auto-dedup reliable?

Auto-dedup uses hybrid similarity (BM25 + cosine) with an 85% threshold. It works well for near-duplicates, but it lets through very different rewordings of the same fact. This is deliberate: a duplicate is better than data loss.

### Q12: How does the MCP server's "store nudge" work?

The server counts consecutive tool calls without `icm_memory_store`. After 10 calls, it adds a hint to the response:
```
[ICM: 12 tool calls since last store. Consider saving important context.]
```
The counter resets on every `icm_memory_store`. It's a discreet reminder so the agent doesn't forget to store.

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
- Normal: model downloads on first use (about 2 GB for the default Qdrant/multilingual-e5-large-onnx)
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
