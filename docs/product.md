# ICM — Product Overview

## The Problem

AI coding agents are amnesic. Every session starts from zero. Every context window compaction erases hours of accumulated understanding.

**What this costs you:**
- Agents re-read the same files every session → wasted tokens and time
- Architecture decisions are re-discussed endlessly → friction on every task
- Resolved bugs resurface because the fix was forgotten → regression in efficiency
- New team members' agents have zero project context → slow ramp-up
- Switching between tools (Claude, Cursor, Codex) loses all accumulated knowledge

**Existing solutions fall short:**

| Solution | Problem |
|----------|---------|
| Mem0 | 2 LLM calls per message (~2k tokens/interaction). Cost compounds fast. |
| MemGPT/Letta | Agent manages its own memory. No cross-tool portability. |
| CLAUDE.md files | Static, manual, no search, no decay, no structure. |
| Context window stuffing | Doesn't scale. 200k tokens isn't infinite. |

## The Solution

ICM is a permanent memory system for AI agents. One binary, zero dependencies, MCP native.

### 5 Differentiators

**1. Two memory models**

Not everything deserves the same treatment. Episodic memories (decisions, errors) decay naturally. Semantic knowledge (architecture graphs) persists forever. ICM models both.

**2. Multilingual by default**

Store in French, recall in English. 100+ languages supported out of the box. No configuration needed. Benchmarked at 93% multilingual recall accuracy.

**3. Zero LLM cost for extraction**

Rule-based pattern matching detects architecture decisions, error resolutions, and configuration changes. No API calls, no token cost, no latency.

**4. Local-first, single file**

Everything in one SQLite file. No cloud service, no API key for storage, no network dependency. Your memory stays on your machine. Works offline.

**5. Universal tool support**

`icm init` configures 14 tools in one command. Works with Claude Code, Cursor, VS Code, Windsurf, Zed, Amp, Amazon Q, Cline, Roo Code, Kilo Code, Codex CLI, OpenCode, Claude Desktop, Gemini. Switch tools without losing memory.

## Use Cases

### Solo Developer — Session Continuity

Problem: every Monday morning, your agent has forgotten everything from Friday.

```
Session 1 (Friday):
  "I chose PostgreSQL because we need JSONB for flexible schemas"
  "The auth service uses JWT with 24h expiry, refresh tokens in Redis"
  "Fixed: connection pool exhaustion under load — switched to PgBouncer"
  → All stored in ICM automatically

Session 47 (3 weeks later):
  Agent: "Based on your previous decision to use PostgreSQL with JSONB..."
  → Recalls without re-reading any file
```

Result: -44% context tokens by session 3. -29% agent turns needed.

### Team Development — Shared Knowledge

Problem: new team members' agents start from zero context every time.

```
Memoir: "backend-architecture"
├── user-service ──depends_on──► postgres
│                ──depends_on──► redis
├── api-gateway  ──depends_on──► user-service
│                ──depends_on──► auth-middleware
├── auth-middleware ──part_of──► api-gateway
└── postgres     ──superseded_by──► cockroachdb (planned Q3)
```

New developer's agent gets full architectural context from day one via `icm recall-context`.

### Multi-Tool Workflow — Portable Memory

Problem: you use Claude Code for backend, Cursor for frontend, Codex for scripts. Each tool is amnesic about the others.

ICM is MCP-native. All tools connect to the same memory. Backend decisions made in Claude Code are recalled in Cursor. Error fixes from Codex sessions inform future Claude Code work.

### Long-Running Projects — Managed Knowledge

Problem: after 6 months, you have 500+ micro-decisions scattered across conversations.

ICM's auto-decay ensures only important information persists:
- Critical decisions stay forever
- One-time fixes fade naturally
- Consolidation merges N entries into one dense summary
- Health reports flag stale topics and suggest cleanup

### Local LLM Users — Context Injection

Problem: local models (ollama) can't use MCP tools. No tool use = no memory.

ICM's context injection works without tool use:

```bash
# Inject recalled memories into the prompt
context=$(icm recall-context "my-project")
ollama run qwen2.5:14b "$context\n\nQuestion: How does auth work?"
```

Benchmarked: +93% recall improvement with qwen2.5:14b, +89% with mistral:7b.

## Performance

### Latency

| Operation | Latency |
|-----------|---------|
| Store (no embedding) | 34 µs |
| Store (with embedding) | 52 µs |
| FTS search | 47 µs |
| Vector search (KNN) | 590 µs |
| Hybrid search | 951 µs |
| Batch decay (1000 memories) | 5.8 ms |

Apple M1 Pro, in-memory SQLite, single-threaded.

### Impact on Agent Efficiency

Tested with real API calls on a real Rust project (12 files, ~550 lines):

| Metric | Without ICM | With ICM | Delta |
|--------|-------------|----------|-------|
| Factual recall (session 2+) | 5% | 68% | +63% |
| Context tokens (session 3) | 75k | 42k | -44% |
| Agent turns (session 2) | 5.7 | 4.0 | -29% |
| Cost per session | $0.030 | $0.025 | -17% |
| Multilingual recall (FR+EN) | — | 93% | — |

### Local Models (ollama)

Pure context injection, no tool use needed:

| Model | Params | Without ICM | With ICM | Delta |
|-------|--------|-------------|----------|-------|
| qwen2.5:14b | 14B | 4% | 97% | +93% |
| mistral:7b | 7B | 4% | 93% | +89% |
| llama3.1:8b | 8B | 4% | 93% | +89% |
| qwen2.5:7b | 7B | 4% | 90% | +86% |
| phi4:14b | 14B | 6% | 79% | +73% |
| llama3.2:3b | 3B | 0% | 76% | +76% |

### Test Protocol

All benchmarks use real API calls — no mocks, no simulated responses.

- **Agent benchmark**: real Rust project in tempdir, N sessions via `claude -p`, measures turns/tokens/cost
- **Knowledge retention**: fictional technical document, 10 factual questions, keyword-matched scoring
- **Isolation**: each run uses fresh tempdir + fresh DB, no session persistence

## Architecture Summary

```
Single binary (icm)
├── Storage: SQLite + FTS5 + sqlite-vec (cosine)
├── Search: 30% BM25 + 70% cosine similarity
├── Embeddings: fastembed, multilingual-e5-base (768d, 100+ langs)
├── Protocol: MCP JSON-RPC 2.0 over stdio
├── Extraction: rule-based pattern matching (zero LLM cost)
└── Tests: 110 tests (unit, security, performance, UX, integration)
```

No cloud. No API key for storage. No Docker. No configuration beyond `icm init`.

## Security

- All SQL queries are parameterized (no string interpolation)
- FTS queries are sanitized against injection
- No network access for storage (local SQLite only)
- Embedding model runs locally
- Tested against: SQL injection, FTS injection, XSS, null bytes, 500KB payloads

## Pricing

**Free** for individuals and teams up to 20 people.

Enterprise license required for organizations >20. Contact: contact@rtk-ai.app

## Getting Started

```bash
brew tap rtk-ai/tap && brew install icm
icm init
```

Two commands. Your agent now has permanent memory.

## Resources

- GitHub: [github.com/rtk-ai/icm](https://github.com/rtk-ai/icm)
- [Technical Architecture](architecture.md)
- [User Guide](guide.md)
