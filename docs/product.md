# ICM — Product Overview

## The Problem

AI coding agents are amnesic. Every session starts from zero. Every context window compaction erases hours of accumulated understanding.

The result:
- Agents re-read the same files every session
- Architecture decisions are re-discussed endlessly
- Resolved bugs resurface because the fix was forgotten
- Projects lose momentum as agents rebuild context from scratch

Existing solutions (Mem0, MemGPT) burn LLM tokens on every message — 2,000+ tokens per interaction just for memory management. That cost compounds fast.

## The Solution

ICM is a permanent memory system for AI agents. One binary, zero dependencies, MCP native.

**What makes ICM different:**

1. **Two memory models** — Episodic (temporal decay, topic-based) and Semantic (permanent knowledge graphs). Not everything deserves to be remembered forever.

2. **Multilingual by default** — Understands 100+ languages out of the box. Store in French, recall in English. No configuration needed.

3. **Zero LLM cost for extraction** — Rule-based pattern matching extracts architecture decisions, error resolutions, and configuration changes without calling any API.

4. **Single SQLite file** — No cloud service, no API key for storage, no network dependency. Your memory stays on your machine.

5. **14-tool setup in one command** — `icm init` configures Claude Code, Cursor, VS Code, Windsurf, Zed, and 8 more tools automatically.

## Use Cases

### Solo Developer

Store project context that persists between coding sessions. No more re-explaining your architecture every Monday morning.

```
Session 1: "I chose PostgreSQL because we need JSONB for flexible schemas"
  → Stored in ICM

Session 47: Agent recalls the decision without asking again
```

### Team Development

Shared project memoirs capture architectural knowledge as a graph. New team members' agents start with full context.

```
memoir: "backend-architecture"
├── user-service ──depends_on──► postgres
├── api-gateway ──depends_on──► user-service
└── cache-layer ──part_of──► api-gateway
```

### Multi-Model Workflows

ICM works with any MCP-compatible tool. Switch between Claude, Cursor, and Codex — the memory follows.

### Long-Running Projects

Projects that span weeks accumulate hundreds of micro-decisions. ICM's auto-decay ensures only important information persists, while consolidation keeps topics clean.

## Performance

| Metric | Value |
|--------|-------|
| Store latency | 34 µs (no embedding) / 52 µs (with embedding) |
| FTS search | 47 µs |
| Hybrid search | 951 µs |
| Memory footprint | ~15 MB for 1000 memories |
| Binary size | ~12 MB |
| First embedding load | ~2s (model cached after) |

## Recall Impact

Tested with real API calls, no mocks:

| Scenario | Without ICM | With ICM |
|----------|-------------|----------|
| Factual recall across sessions | 5% | 68% |
| Context tokens (session 3+) | 75k | 42k (-44%) |
| Agent turns needed | 5.7 | 4.0 (-29%) |
| Multilingual recall (FR+EN) | n/a | 93% |

Local models (ollama) see even larger gains: up to +93% recall improvement with qwen2.5:14b.

## Pricing

**Free** for individuals and teams up to 20 people.

Enterprise license required for organizations >20. Contact: license@rtk.ai

## Getting Started

```bash
brew tap rtk-ai/tap && brew install icm
icm init
```

Your agent now has permanent memory. No API keys for storage, no cloud setup, no configuration beyond `icm init`.

## Resources

- GitHub: [github.com/rtk-ai/icm](https://github.com/rtk-ai/icm)
- Technical docs: [docs/architecture.md](architecture.md)
- User guide: [docs/guide.md](guide.md)
