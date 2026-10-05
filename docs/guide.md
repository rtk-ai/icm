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

## Les 5 premieres minutes avec ICM

Guide eclair pour etre operationnel en 5 minutes.

### Minute 1 : Installer

```bash
brew tap rtk-ai/tap && brew install icm
```

Ou, sans Homebrew :

```bash
curl -fsSL https://raw.githubusercontent.com/rtk-ai/icm/main/install.sh | sh
```

### Minute 2 : Configurer

```bash
icm init
```

ICM detecte automatiquement vos outils IA (Claude Code, Cursor, VS Code, etc.) et configure le serveur MCP pour chacun. Verifiez la sortie — chaque outil affiche `configured` ou `already configured`.

### Minute 3 : Stocker un premier souvenir

```bash
icm store -t "test" -c "Mon premier souvenir ICM" -i high
```

Verifiez qu'il est stocke :

```bash
icm topics
icm stats
```

### Minute 4 : Rappeler un souvenir

```bash
icm recall "premier souvenir"
```

Le souvenir doit apparaitre avec son ID, topic, poids et contenu.

### Minute 5 : Tester avec votre agent

Relancez votre outil IA (Claude Code, Cursor...). Demandez-lui :

> "Rappelle le contexte ICM"

L'agent devrait utiliser automatiquement `icm_memory_recall`. S'il ne le fait pas, voir la section Troubleshooting ci-dessous.

### Et apres ?

- Stockez vos decisions d'architecture avec `-i high`
- Stockez les faits invariants (ports, URLs) avec `-i critical`
- Apres chaque bug fixe, stockez la resolution avec des mots-cles
- Pour aller plus loin : creez un **memoir** pour structurer les connaissances en graphe

---

## Troubleshooting

### 1. L'agent n'utilise pas les outils ICM

**Symptome :** L'agent ne rappelle ni ne stocke rien, meme quand on lui demande.

**Solutions :**
- Lancez `icm init` et verifiez la sortie pour chaque outil
- Verifiez que le fichier de config MCP existe (ex: `~/.claude.json` pour Claude Code)
- Testez manuellement le serveur :
  ```bash
  echo '{"jsonrpc":"2.0","id":1,"method":"initialize"}' | icm serve
  ```
  Vous devez voir une reponse JSON avec `capabilities` et `serverInfo`
- Verifiez que `icm serve` est dans votre PATH : `which icm`
- **Redemarrez votre outil IA** apres avoir lance `icm init`

### 2. `icm recall` ne retourne rien

**Symptome :** La recherche retourne "No memories found."

**Solutions :**
- `icm topics` — verifiez qu'il y a des souvenirs stockes
- `icm stats` — verifiez le total
- Essayez une requete plus large ou supprimez les filtres topic/keyword
- `icm list --all` — listez tout pour verifier le contenu
- Si les souvenirs existent mais ne matchent pas : backfill les embeddings avec `icm embed`

### 3. Les embeddings sont lents au premier lancement

**Symptome :** ICM prend 30+ secondes au premier `store` ou `recall`.

**Explication :** Le modele d'embedding (~100MB pour multilingual-e5-base) est telecharge a la premiere utilisation. Les executions suivantes chargent depuis le cache (~1-2s).

**Solutions :**
- C'est normal la premiere fois — attendez le telechargement
- Pour accelerer : utilisez un modele plus leger dans `config.toml` :
  ```toml
  [embeddings]
  model = "Xenova/bge-small-en-v1.5"  # 384d, anglais seul, le plus rapide
  ```
- Pour compiler sans embeddings : `cargo build --no-default-features`

### 4. Des souvenirs en double apparaissent

**Symptome :** Plusieurs souvenirs quasi-identiques dans le meme topic.

**Explication :** L'auto-dedup fonctionne uniquement via MCP (serveur avec embedder). Le CLI `icm store` n'a pas d'auto-dedup par defaut.

**Solutions :**
- Backfill les embeddings : `icm embed`
- Supprimez les doublons manuellement : `icm forget <id>`
- Consolidez le topic : `icm consolidate -t <topic>`

### 5. Erreur "embeddings feature not enabled"

**Symptome :** `icm embed` echoue avec un message sur le feature.

**Solution :** Recompilez avec le feature embeddings :
```bash
cargo build --release  # Le feature "embeddings" est actif par defaut
```

Si vous utilisez le binaire pre-compile depuis les releases GitHub, les embeddings sont toujours inclus.

### 6. Corruption de la base de donnees

**Symptome :** `icm stats` ou `icm recall` echoue avec une erreur SQLite.

**Solutions :**
- Localisez la base :
  - macOS : `~/Library/Application Support/dev.icm.icm/memories.db`
  - Linux : `~/.local/share/dev.icm.icm/memories.db`
- Sauvegardez le fichier `.db` et ses fichiers WAL (`.db-wal`, `.db-shm`)
- Supprimez et reconstruisez si necessaire — la migration est automatique
- Pour tester avec une base propre : `icm --db /tmp/test.db stats`

### 7. `icm init` ne detecte pas mon outil

**Symptome :** L'outil n'apparait pas dans la sortie de `icm init`.

**Solutions :**
- Verifiez que l'outil est installe et que son fichier de config existe
- Pour Claude Code : `~/.claude.json` doit exister (cree au premier lancement)
- Configuration manuelle : `claude mcp add icm -- icm serve`
- Pour les outils non supportes, ajoutez manuellement dans leur config MCP :
  ```json
  { "command": "/chemin/vers/icm", "args": ["serve"] }
  ```

### 8. Le decay est trop agressif / pas assez

**Symptome :** Les souvenirs disparaissent trop vite, ou s'accumulent sans etre nettoyes.

**Solutions :**
- Ajustez dans `~/.config/icm/config.toml` :
  ```toml
  [memory]
  decay_rate = 0.98      # Plus lent (defaut: 0.95)
  prune_threshold = 0.05 # Seuil plus bas (defaut: 0.1)
  ```
- Utilisez `icm prune --dry-run --threshold 0.2` pour previsualiser
- Marquez les souvenirs importants en `high` ou `critical` pour les proteger

### 9. Le mode compact ne s'active pas

**Symptome :** Les reponses MCP restent longues malgre la configuration.

**Solutions :**
- Verifiez `config.toml` :
  ```toml
  [mcp]
  compact = true
  ```
- Ou forcez via le flag : changez `icm serve` en `icm serve --compact` dans la config MCP
- Redemarrez votre outil IA apres la modification

### 10. Erreur "memoir not found" malgre sa creation

**Symptome :** `icm memoir show <nom>` echoue juste apres `icm memoir create`.

**Solutions :**
- Verifiez le nom exact : `icm memoir list`
- Les noms sont sensibles a la casse : `Archi` != `archi`
- Verifiez que vous n'utilisez pas `--db` avec un chemin different

### 11. Performance degradee avec beaucoup de souvenirs

**Symptome :** `recall` devient lent avec >1000 souvenirs.

**Solutions :**
- La recherche hybride prend ~1ms par requete pour 1000 souvenirs — c'est normal
- Consolidez les topics volumineux : `icm consolidate -t <topic>`
- Prunez les souvenirs perimees : `icm prune`
- Reduisez le `limit` dans les recherches

### 12. L'extraction ne detecte rien

**Symptome :** `icm extract` retourne "No facts extracted."

**Solutions :**
- Le texte doit contenir des signaux reconnus (mots-cles d'architecture, erreurs, decisions)
- Testez avec un texte explicite :
  ```bash
  echo "We decided to use PostgreSQL instead of MySQL" | icm extract --dry-run
  ```
- Ajustez le seuil dans `config.toml` :
  ```toml
  [extraction]
  min_score = 2.0  # Plus bas = plus de faits extraits (defaut: 3.0)
  ```

---

## Guides d'integration par outil

### Claude Code

**Setup :**
```bash
icm init  # Configure automatiquement ~/.claude.json
```

**Configuration manuelle :**
```bash
claude mcp add icm -- icm serve
```

**Fichier de config :** `~/.claude.json`
```json
{
  "mcpServers": {
    "icm": {
      "command": "/chemin/vers/icm",
      "args": ["serve"]
    }
  }
}
```

**Slash commands (optionnel) :**
```bash
icm init --mode skill
```
Installe `/recall` et `/remember` dans `~/.claude/commands/`.

**Hook PostToolUse (optionnel) :**
```bash
icm init --mode hook
```
Installe un hook qui extrait automatiquement le contexte apres chaque appel d'outil (git commit, edit, etc.).

**Mode compact recommande :** Claude Code beneficie du mode compact pour economiser des tokens. Activez dans `~/.config/icm/config.toml` :
```toml
[mcp]
compact = true
```

**Instructions CLAUDE.md (optionnel) :**
```bash
icm init --mode cli
```
Ajoute les instructions ICM au `CLAUDE.md` du projet courant.

---

### Cursor

**Setup :**
```bash
icm init  # Configure automatiquement ~/.cursor/mcp.json
```

**Fichier de config :** `~/.cursor/mcp.json`
```json
{
  "mcpServers": {
    "icm": {
      "command": "/chemin/vers/icm",
      "args": ["serve"]
    }
  }
}
```

**Regle Cursor (optionnel) :**
```bash
icm init --mode skill
```
Cree `~/.cursor/rules/icm.mdc` avec une regle `alwaysApply: true` qui rappelle a l'agent d'utiliser ICM.

**Apres configuration :** Redemarrez Cursor. Les outils ICM apparaissent dans la palette MCP.

---

### VS Code / GitHub Copilot

**Setup :**
```bash
icm init  # Configure automatiquement ~/Library/.../Code/User/mcp.json
```

**Fichier de config :**
- macOS : `~/Library/Application Support/Code/User/mcp.json`
- Linux : `~/.config/Code/User/mcp.json`

```json
{
  "servers": {
    "icm": {
      "command": "/chemin/vers/icm",
      "args": ["serve"]
    }
  }
}
```

**Note :** VS Code utilise `"servers"` au lieu de `"mcpServers"`. `icm init` gere cette difference automatiquement.

---

### Windsurf

**Setup :**
```bash
icm init  # Configure automatiquement ~/.codeium/windsurf/mcp_config.json
```

**Fichier de config :** `~/.codeium/windsurf/mcp_config.json`
```json
{
  "mcpServers": {
    "icm": {
      "command": "/chemin/vers/icm",
      "args": ["serve"]
    }
  }
}
```

---

### Zed

**Setup :**
```bash
icm init  # Configure automatiquement ~/.zed/settings.json
```

Zed utilise un format different avec `context_servers` :
```json
{
  "context_servers": {
    "icm": {
      "command": {
        "path": "/chemin/vers/icm",
        "args": ["serve"]
      },
      "settings": {}
    }
  }
}
```

---

### Amp

**Setup :**
```bash
icm init  # Configure automatiquement ~/.config/amp/settings.json
```

**Slash commands (optionnel) :**
```bash
icm init --mode skill
```
Installe `/icm-recall` et `/icm-remember` dans `~/.config/amp/skills/`.

---

### OpenAI Codex CLI

**Setup :**
```bash
icm init  # Configure automatiquement ~/.codex/config.toml
```

**Fichier de config (TOML) :** `~/.codex/config.toml`
```toml
[mcp_servers.icm]
command = "/chemin/vers/icm"
args = ["serve"]
```

---

### Claude Desktop

**Setup :**
```bash
icm init  # Configure automatiquement
```

**Fichier de config :** `~/Library/Application Support/Claude/claude_desktop_config.json`
```json
{
  "mcpServers": {
    "icm": {
      "command": "/chemin/vers/icm",
      "args": ["serve"]
    }
  }
}
```

---

### Autres outils (Gemini, Amazon Q, Cline, Roo Code, Kilo Code, OpenCode)

Tous sont configures automatiquement par `icm init`. Le format est toujours le meme :

```json
{
  "command": "/chemin/vers/icm",
  "args": ["serve"]
}
```

La seule difference est le fichier de config et la cle JSON. `icm init` gere toutes ces variations.

---

## FAQ

### Q1 : ICM envoie-t-il des donnees sur internet ?

**Non.** ICM stocke tout localement dans un fichier SQLite. Le modele d'embedding tourne localement (via fastembed/ONNX Runtime). Aucune donnee ne quitte votre machine. Le seul acces reseau est le telechargement initial du modele d'embedding (~100MB, une seule fois).

### Q2 : Puis-je utiliser ICM avec plusieurs projets ?

**Oui.** Tous les projets partagent la meme base SQLite. Utilisez des topics prefixes par projet (ex: `decisions-api`, `decisions-frontend`) pour les separer. Vous pouvez aussi utiliser `--db <chemin>` pour isoler completement les bases.

### Q3 : Comment sauvegarder/restaurer ma memoire ?

Sauvegardez le fichier SQLite :
```bash
# macOS
cp ~/Library/Application\ Support/dev.icm.icm/memories.db ~/backup-icm.db

# Restaurer
cp ~/backup-icm.db ~/Library/Application\ Support/dev.icm.icm/memories.db
```

### Q4 : ICM fonctionne-t-il avec des modeles locaux (ollama) ?

**Oui.** ICM est un serveur MCP standard. Il fonctionne avec tout client MCP, y compris ceux utilisant des modeles locaux. Les benchmarks montrent jusqu'a +93% de rappel avec qwen2.5:14b via ollama.

### Q5 : Quelle est la difference entre `icm consolidate` (CLI) et `icm_memory_consolidate` (MCP) ?

Le CLI fusionne automatiquement les summaries (concatenation avec ` | `). Le MCP demande a l'agent de fournir le resume, ce qui produit un resultat plus intelligent car l'agent comprend le contenu et peut synthetiser.

### Q6 : Puis-je changer de modele d'embedding sans perdre mes donnees ?

**Oui.** Les souvenirs (texte) sont toujours conserves. Seuls les vecteurs sont effaces et recreees. Apres avoir change le modele dans `config.toml`, lancez `icm embed --force` pour regenerer tous les vecteurs.

### Q7 : Combien de souvenirs ICM peut-il gerer ?

La base SQLite gere des millions de lignes sans probleme. Les benchmarks montrent ~34us par store et ~951us par recherche hybride pour 1000 souvenirs. La performance se degrade lineairement, pas exponentiellement.

### Q8 : Comment supprimer toute ma memoire ?

```bash
# macOS
rm ~/Library/Application\ Support/dev.icm.icm/memories.db*

# Linux
rm ~/.local/share/dev.icm.icm/memories.db*
```

La base est recreee automatiquement au prochain lancement.

### Q9 : ICM consomme-t-il des tokens LLM ?

**Non pour le stockage et le rappel.** ICM n'appelle aucune API LLM. Les seuls tokens consommes sont ceux de l'agent qui appelle les outils MCP -- exactement comme tout autre outil MCP. Le mode compact (`--compact`) reduit ces tokens de ~40%.

L'extraction (Layer 0) est purement regles -- zero cout LLM. Le Layer 1 (PreCompact, planifie) utilisera ~500 tokens par session.

### Q10 : Puis-je partager ma memoire avec mon equipe ?

Pas directement (c'est un fichier SQLite local). Cependant, les **memoirs** sont conçus pour capturer des connaissances structurees qui peuvent etre exportees et partagees. Une fonctionnalite d'import/export est prevue.

### Q11 : L'auto-dedup est-il fiable ?

L'auto-dedup utilise la similarite hybride (BM25 + cosine) avec un seuil de 85%. Il fonctionne bien pour les doublons proches mais laisse passer les reformulations tres differentes du meme fait. C'est volontaire : mieux vaut un doublon qu'une perte de donnees.

### Q12 : Comment fonctionne le "store nudge" du serveur MCP ?

Le serveur compte les appels d'outils consecutifs sans `icm_memory_store`. Apres 10 appels, il ajoute un hint a la reponse :
```
[ICM: 12 tool calls since last store. Consider saving important context.]
```
Le compteur se reinitialise a chaque `icm_memory_store`. C'est un rappel discret pour que l'agent n'oublie pas de stocker.

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
