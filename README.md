[English](README.md) | [Français](README_fr.md) | [Español](README_es.md) | [Deutsch](README_de.md) | [Italiano](README_it.md) | [Português](README_pt.md) | [Nederlands](README_nl.md) | [Polski](README_pl.md) | [Русский](README_ru.md) | [日本語](README_ja.md) | [中文](README_zh.md) | [العربية](README_ar.md) | [한국어](README_ko.md)

<h1 align="center">ICM</h1>

<p align="center">
  <b>Long-term memory for AI coding agents, shared across your tools.</b><br>
  One binary, one SQLite file. No LLM call to store or recall a memory.
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/icm/actions/workflows/ci.yml"><img src="https://github.com/rtk-ai/icm/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/icm/releases/latest"><img src="https://img.shields.io/github/v/release/rtk-ai/icm?color=purple" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-Apache--2.0-blue.svg" alt="Apache-2.0"></a>
</p>

Tell Claude Code how your project handles auth on Monday, and Tuesday's Gemini CLI session already knows it. ICM keeps what your coding agents learn (decisions, fixes, conventions, preferences) in one SQLite file on your machine, and gives the relevant part back at the start of each session and with each prompt you send. Up to 18 agents and editors share that memory, so you stop re-explaining your project every time you open a session or switch tools.

- **92.9% on LoCoMo (1,540 questions, mean of three runs), level with Hindsight (92.0%)**, with a third less context per question. [Details and caveats below](#benchmark-comparison).
- **No LLM call to store or recall.** Hindsight, Mem0, Graphiti (Zep) and claude-mem call an LLM for every memory they store by default. ICM does not. Only its automatic extraction of facts from tool output goes through the LLM command-line tool you already use, when one is installed; `provider = "none"` keeps that local too (see [Quickstart](#quickstart)).
- **Useful without an embedding model.** Keyword recall alone puts at least one of the right sessions in the top 5 for 88.6% of LoCoMo questions, with a median of 6.8 ms per recall on Linux x86-64.
- **97.4% on LongMemEval-S, retrieval with no LLM**: one of the right sessions in the top 5 for 487 of 500 questions, against 96.6% for MemPalace and 95.2% for agentmemory on the same measure.
- **Not ahead everywhere.** On PersonaMem (a user's evolving preferences), Hindsight leads: 86.6% against ICM's 81.7%.

<p align="center">
  <img src="assets/demo.svg" alt="Terminal: three memories stored with icm store, then two questions answered by icm recall, each returning the right memory">
</p>

## Quickstart

```bash
brew tap rtk-ai/tap && brew install icm    # or: curl -fsSL https://raw.githubusercontent.com/rtk-ai/icm/main/install.sh | sh
icm init                                   # instructions, skills and hooks for every agent it detects
```

That is the whole setup. Open a new session in Claude Code, Codex, Gemini CLI or Copilot CLI: your agent now starts with a short pack of the most important memories of the project it is in (those whose topic carries the repository's name, such as `decisions-myapp` in a repository called `myapp`, plus your preferences) and receives the memories relevant to each prompt you send. What it learns from its tool output is queued, and the queue is turned into memories at the end of each Claude Code session; with the other tools, run `icm extract-pending` (from a cron job, for instance).

Storing and recalling stay on your machine. The automatic extraction hands text to the LLM command-line tool you already use (Claude Code, Codex or Gemini CLI) when one is installed; set `provider = "none"` under `[extraction.summarizer]` to keep it entirely local.

To see it work right away, store and recall by hand:

```console
$ icm store -t decisions-myapp -c "Auth uses short-lived JWTs, refreshed through /auth/refresh" -i high
Stored: 01M47YY6CHVZ48BYKQRCRNZTCF

$ icm recall "how does auth work"
memories[1]{id,topic,importance,weight,summary}:
  01M47YY6CHVZ48BYKQRCRNZTCF,decisions-myapp,high,0.975,"Auth uses short-lived JWTs, refreshed through /auth/refresh"
```

The first store or recall with semantic search downloads the multilingual embedding model once (`Qdrant/multilingual-e5-large-onnx`, about 2 GB). To try ICM without it, add `--no-embeddings` (keyword recall, as in the output above) or pick a lighter model in the config. `icm init` writes hooks and instructions into each detected agent's configuration; `icm uninstall --dry-run` shows how to remove them. Windows, Linux, Nix and building from source: [Install](#install).

## Benchmark comparison

Answer accuracy on [LoCoMo](https://github.com/snap-research/locomo) (10 long conversations, 1,540 questions), measured with the public [Agent Memory Benchmark](https://github.com/vectorize-io/agent-memory-benchmark) harness: the memory system retrieves context, `gemini-3.1-pro-preview` answers from it, `gemini-2.5-flash-lite` judges the answer.

| System | LoCoMo accuracy | Context per question | LLM calls to store a memory | Runs as | Result |
|--------|:---------------:|:--------------------:|:---------------------------:|---------|--------|
| **ICM** 0.11.0 (recall engine v2) | **92.9%** (1,430, 1,433 and 1,430 / 1,540 in three runs) | 24.1k tokens | none | one Rust binary, SQLite file | our runs, 2026-10-06 |
| Hindsight | 92.0% (1,417 / 1,540) | 36.2k tokens | LLM fact extraction | Python service, PostgreSQL + pgvector | published by the harness |
| Hybrid search baseline (dense + sparse, RRF) | 79.1% (1,218 / 1,540) | 22.2k tokens | none | Qdrant | published by the harness |

On [PersonaMem](https://arxiv.org/abs/2504.14225) 32k (589 multiple-choice questions about a user's evolving preferences, same harness and answering model, scored by letter match):

| System | PersonaMem accuracy | Context per question | Result |
|--------|:-------------------:|:--------------------:|--------|
| **ICM** 0.11.0 (recall engine v2) | **81.7%** (486, 486 and 472 / 589 in three runs) | 16.2k tokens | our runs, 2026-10-06 |
| Hindsight | 86.6% (510 / 589) | 15.8k tokens | published by the harness |
| Hybrid search baseline | 84.4% (497 / 589) | 24.2k tokens | published by the harness |

Retrieval alone, on LoCoMo, with no answering model: the share of questions for which at least one of the right sessions is in the top results. This is the evidence for the recall engine itself.

| Recall engine | Search | Top 5 | Top 10 | Top 20 |
|---|---|:---:|:---:|:---:|
| **0.11** (default) | keywords + embedding model | **86.7%** | **93.3%** | **97.9%** |
| **0.11** (default) | keywords only | **88.6%** | **94.2%** | **97.5%** |
| 0.10 (`--engine legacy`) | keywords + embedding model | 76.5% | 83.0% | 87.7% |
| 0.10 (`--engine legacy`) | keywords only | 12.0% | 17.4% | 29.8% |

Both engines were measured with the same 0.11 build. The 0.11 runs received each session's date and the question's date; the 0.10 engine has no date input, so its runs received none.

LongMemEval-S, retrieval only, with no LLM (ICM with its default embedding model; 500 questions; each question comes with about 48 past sessions to search; one memory per session, user turns only, as MemPalace indexes them; no date given to ICM):

| Right sessions in the top 5 | **ICM** 0.11.0 | MemPalace | agentmemory | BM25 alone |
|---|:---:|:---:|:---:|:---:|
| At least one (the published measure) | **97.4%** (487 / 500) | 96.6% (483 / 500) | 95.2% (476 / 500) | 94.6% |
| All of them | **88.6%** (443 / 500) | 85.0% | 81.8% | 81.2% |

MemPalace and agentmemory figures are recomputed with our scorer from the result files each project publishes; they match their published numbers. agentmemory indexes all turns of a session; on that unit BM25 alone reaches 96.2% and 83.0%. A plain BM25 already scores in the mid-nineties on the first measure, which is why the second one, all the right sessions, separates the systems better.

What these numbers do and do not show:

- **ICM and Hindsight are tied on LoCoMo.** ICM's three runs (92.9%, 93.1%, 92.9%) are each about 1 point above Hindsight's published 92.0%, a gap of about 14 questions, inside the sampling error (95% interval for ICM: 91.6 to 94.2). ICM gets there with a third less context and without calling an LLM when a memory is stored.
- **On PersonaMem, Hindsight is ahead** by 4.9 points, outside the sampling error (95% interval for ICM: 78.6 to 84.8). ICM is also 2.7 points below the hybrid search baseline, inside that interval, while reading a third less context than it. The three runs spread from 80.1% to 82.5%.
- **Not identical conditions.** The harness is maintained by Vectorize, the vendor of Hindsight. The published LoCoMo results predate a change that set the answer and judge temperature to 0; our run uses the current harness (commit `f618ed7`) and Vertex AI.
- **At 50 chunks, much of each conversation is returned,** so this accuracy also measures the answering model. The retrieval table is the evidence for the recall engine itself.
- **Three runs per dataset.** The answering model varies from run to run: 0.2 points on LoCoMo, 2.4 points on PersonaMem. The intervals above cover the sampling of questions.

<details>
<summary>Per-category results, latency and further caveats</summary>

- **By question type** (LoCoMo, harness labels, three runs): open-domain 96.7% (841 questions), temporal 90.9% (321), single-hop 88.8% (282), multi-hop 78.8% (96).
- **Latency.** Median recall latency 137 to 149 ms on the cluster's 4-vCPU nodes during the LoCoMo runs; 272 sessions ingested with no LLM call. In the retrieval-only runs on Linux x86-64: median 136 ms with embeddings, 6.8 ms keyword-only.
- **Session dates.** The benchmark adapter writes each session's date into ICM's memory text; Hindsight receives the same dates as metadata, and every system gets the date of the question.
- **The weakest category is the 96 questions the harness labels multi-hop** (78.8%). Category names do not line up across benchmarks: other LoCoMo evaluations call this category open-domain, and call multi-hop the 282 questions the harness labels single-hop (88.8% here). Compare by question count, not by label.
- **Recall engine v2 is the default** for `icm recall`, the MCP `icm_memory_recall` tool, HTTP `/recall` and the prompt hook. The previous engine stays available to roll back or to compare: `icm recall --engine legacy`, `"engine": "legacy"` on HTTP `/recall`, or `ICM_RECALL_ENGINE=legacy`.

</details>

Per-question results for every run (three per dataset, plus the LongMemEval-S recall run) are in [`bench/amb/results/`](bench/amb/results/); the adapter, the exact settings and the commands to reproduce are in [`bench/amb/README.md`](bench/amb/README.md).

## One memory for every tool

Every tool configured by `icm init` reads and writes the same SQLite database, and topics (`decisions-myapp`, `preferences`, `errors-resolved`, ...) are not partitioned by tool. A memory stored from Claude Code is immediately visible to Codex, Gemini, Cursor, Roo, Amp, Aider, ...

Want isolation instead? `icm init --per-project` creates a project-local database under `.icm/` (and writes the agents' instruction files, such as `CLAUDE.md` and `AGENTS.md`, in the current directory); `--db <path>` or `ICM_DB` point to any other file. Each path is an independent corpus.

> **Project status: beta.** ICM is pre-1.0: breaking changes can land in any minor release, and hook and MCP configuration formats may shift. Beta refers to API stability, not to day-to-day usefulness: I (the maintainer) use ICM every day as my primary AI coding memory. My main focus is [rtk](https://github.com/rtk-ai/rtk), so issues and pull requests are reviewed on a best-effort cadence.
>
> Apache-2.0, shipped **as-is, without warranty of any kind** (see [LICENSE](LICENSE)). Before any destructive operation, run the read-only equivalent first (`icm uninstall --dry-run`, `icm uninstall --check`).

## Install

```bash
# macOS / Linux, Homebrew
brew tap rtk-ai/tap && brew install icm

# macOS / Linux, script (verifies SHA256 against the release checksums)
curl -fsSL https://raw.githubusercontent.com/rtk-ai/icm/main/install.sh | sh

# Windows, PowerShell
irm https://raw.githubusercontent.com/rtk-ai/icm/main/install.ps1 | iex
```

On Windows, type `icm.exe` in PowerShell: there `icm` alone is a built-in alias of `Invoke-Command`. cmd and Git Bash accept both, and `icm init` writes `icm.exe` in the instructions it gives your agents.

Keyword search works everywhere. Run `icm embeddings status` to see whether semantic search is on: it is built into the macOS Apple Silicon, Windows and `.rpm` builds; the Linux glibc archives and the `.deb` need one `icm embeddings download`; the Intel Mac build needs your own ONNX Runtime (`ORT_DYLIB_PATH`); the static Linux musl build is keyword-only. Nix, building from source, version pinning and the details: [reference](docs/reference.md#install).

## Setup

```bash
icm init                  # global database, every detected agent
icm init --per-project    # database under .icm/ at the git root
icm init --mode all       # also register the MCP server in every tool that supports it
```

The default mode (`standard`) writes instructions, skills and hooks, without an MCP server. `--mode all` adds the MCP server; with it (plus `--per-project` for Aider, whose conventions file is per project), that covers the 19 tools below ([integration guide](docs/integrations.md)):

| Tool | MCP server | Hooks |
|------|:---:|:-----:|
| Claude Code | yes | yes |
| Claude Desktop | yes | — |
| Gemini CLI | yes | yes |
| Codex CLI | yes | yes |
| Copilot CLI | yes | yes |
| Cursor | yes | — |
| Windsurf | yes | — |
| VS Code | yes | — |
| Amp | yes | — |
| Amazon Q | yes | — |
| Cline | yes | — |
| Roo Code | yes | — |
| Kilo Code | yes | — |
| Zed | yes | — |
| OpenCode | yes | yes |
| Continue.dev | yes | — |
| Aider | — | — |
| Pi | — | — |
| Mistral Vibe | yes | yes (pre/post tool) |

Or register the MCP server by hand: `claude mcp add icm -- icm serve` (any MCP client: command `icm`, args `["serve"]`).

What the hooks do:

| Hook | What it does |
|------|-------------|
| `icm hook start` | Inject a wake-up pack of critical/high memories at session start (~500 tokens) |
| `icm hook pre` | Auto-allow `icm` CLI commands (no permission prompt) |
| `icm hook post` | Extract facts from tool output every N calls (auto-extraction) |
| `icm hook compact` | Extract memories from transcript before context compression |
| `icm hook prompt` | Inject recalled context at the start of each user prompt |

Per-tool hook tables, skills, instruction files and the Codex note: [reference](docs/reference.md#setup).

## Use

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

# Extract facts from text (rule-based, no LLM call)
echo "The parser uses Pratt algorithm" | icm extract -p my-project
```

ICM also keeps **memoirs** (permanent knowledge graphs of concepts and typed relations), **feedback** (corrections to learn from) and **verbatim transcripts**, exposes **32 MCP tools** (31 without an embedding model), an **HTTP API** that keeps the embedding model warm and a **terminal dashboard** (`icm dashboard`). All of it is in the [reference](docs/reference.md).

## How it works

Recall fuses up to three ranked lists by reciprocal rank (RRF): **FTS5 BM25** keyword matching, always on; **semantic vector search** via sqlite-vec when an embedding model is loaded (default `Qdrant/multilingual-e5-large-onnx`, 1024 dimensions, 100+ languages); and a **date window** when the query names a period ("last week", "in March 2024"). Project, topic and keyword filters apply before the cut. Memories decay over time according to their importance (`critical` never fades); with an embedding model loaded, a new memory almost identical to one in the same topic (cosine similarity above 0.95) is merged into it; and the model that produced the stored vectors is recorded in the database, so changing `model` in the config never clears them (`icm embed --migrate` is the explicit way to switch).

Everything lives in one SQLite file, with no external service:

```
~/Library/Application Support/dev.icm.icm/memories.db     # macOS (dev.icm.icm is the app identifier, not a dev build)
~/.local/share/icm/memories.db                            # Linux
%APPDATA%\icm\icm\data\memories.db                        # Windows
<project-root>/.icm/memories.db                           # icm init --per-project
```

`icm config` shows the active configuration; [config/default.toml](config/default.toml) lists every option. Details: [reference](docs/reference.md#how-it-works), [architecture diagrams](docs/architecture.md#architecture-at-a-glance).

## Documentation

| Document | Description |
|----------|-------------|
| [Integration Guide](docs/integrations.md) | Per-tool MCP setup: Claude Code, Cursor, Windsurf, Zed, Amp, Codex, Cline, Roo Code, etc. |
| [Technical Architecture](docs/architecture.md) | Architecture diagram, function flows, crate structure, search pipeline, decay model, sqlite-vec integration, testing |
| [User Guide](docs/guide.md) | Installation, topic organization, consolidation, extraction, troubleshooting |
| [Product Overview](docs/product.md) | Use cases, benchmarks, comparison with alternatives |
| [Reference](docs/reference.md) | Install options, per-tool setup, CLI, 32 MCP tools, HTTP API, dashboard, internals |
| [Benchmark adapter](bench/amb/README.md) | How the comparison above was run, and how to reproduce it |
| [Demonstrations](docs/demonstrations.md) | Storage micro-benchmarks and small demos |

## License

[Apache-2.0](LICENSE)
