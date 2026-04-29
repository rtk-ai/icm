# ICM Agent Effectiveness Design

## Goal

Make ICM substantially more useful for coding agents by turning it from a generic long-term note store into an operational memory layer that is aware of repository, platform, command failures, resolved errors, and current project context.

## Current State

The repository already has the right primitives, but they are still loosely connected for coding-agent workflows:

- `crates/icm-core/src/wake_up.rs` builds a compact session-start pack from stored memories.
- `crates/icm-core/src/memory.rs` stores summary, topic, keywords, source, scope, and related IDs, but it lacks first-class operational metadata such as file path, command, platform, and error fingerprint.
- `crates/icm-store/src/schema.rs` and `crates/icm-store/src/store.rs` provide SQLite + FTS storage and retrieval.
- `crates/icm-mcp/src/tools.rs` exposes store/recall/wake-up tools and already contains a first dedup threshold.
- `crates/icm-cli/src/main.rs` contains `store`, `recall`, `wake-up`, `init`, and hook entrypoints, but the hooks are still mostly generic.
- `crates/icm-mcp/src/server.rs` nudges storing after many tool calls, but it does not yet distinguish between stable operational facts and ephemeral session noise.

## Problem Statement

For coding agents, the highest-value memories are not generic summaries. They are operational facts such as:

- which command actually builds or tests the repo
- which platform-specific quirk exists on the current machine
- which failure signature was already resolved
- which file or symbol owns the behavior
- which decisions should always be injected at session start

Without first-class support for those facts, recall quality is lower than it could be, wake-up packs are less actionable, and memory tends to become noisy.

## Approaches

### Approach A: Prompt-only discipline

Keep the current schema and rely on agents to write better topics, keywords, and summaries.

Pros:

- no schema changes
- lowest implementation cost

Cons:

- quality stays agent-dependent
- weak matching for commands, errors, platform, and file ownership
- wake-up remains mostly topic-driven instead of operationally driven

### Approach B: Full graph-first operational memory

Model all operational context as concepts and links in memoirs, then build recall on top of the graph layer.

Pros:

- richest long-term representation
- strong future fit for visualization and architecture reasoning

Cons:

- too large for the next increment
- overkill for common store/recall/wake-up flows
- delays payoff on the CLI and MCP path

### Approach C: Metadata-first operational memory on top of existing memory records

Extend `Memory` and storage with a small `MemoryContext` payload for repo/platform/command/file/error metadata, then upgrade ranking, deduplication, and wake-up generation to use it.

Pros:

- immediate value for coding agents
- fits current CLI, MCP, and hook surfaces
- minimal conceptual change for users
- supports a later graph-first expansion instead of blocking it

Cons:

- requires schema migration and retrieval/ranking updates
- some matching will still be heuristic rather than fully semantic

## Recommendation

Use Approach C.

It gives the biggest practical gain for the least architectural disruption. The existing repo already centers workflows around `Memory`, CLI commands, MCP tools, and wake-up packs. Adding a structured operational metadata layer there improves the highest-frequency paths immediately and leaves memoirs free for richer semantic knowledge later.

## Proposed Design

### 1. Add operational metadata to memory records

Introduce a new optional `MemoryContext` structure on `Memory` with these fields:

- `project`: normalized project or repo name
- `repo_root`: canonical repo path when known
- `platform`: OS name such as `windows`, `linux`, `macos`
- `command`: command that reproduced, fixed, or verified something
- `file_path`: primary file related to the memory
- `symbol`: optional function/type/command name
- `error_fingerprint`: compact normalized error signature
- `kind`: enum-like string such as `decision`, `resolved_error`, `working_command`, `constraint`, `preference`, `progress`

This keeps summaries readable while making recall more precise.

### 2. Upgrade retrieval to favor operational matches

Recall should score not only by topic, summary, embeddings, and weight, but also by contextual matches:

- direct command match
- direct error fingerprint match
- project/repo match
- platform match
- file/symbol match
- stable `kind` boost for `decision`, `resolved_error`, and `working_command`

This should be additive, not a replacement for current search.

### 3. Split stable vs ephemeral memory at capture time

Not all memories should enter long-term recall equally.

- Stable memories: decisions, resolved errors, environment facts, working commands, preferences.
- Ephemeral memories: temporary status, transient progress, one-off experiments.

Stable memories should be eligible for wake-up packs and stronger ranking. Ephemeral memories should still be storable, but lower-ranked by default and excluded from wake-up unless explicitly requested later.

### 4. Make wake-up packs operational

`wake_up` should produce a short startup pack optimized for coding work:

- project facts
- environment facts
- working commands
- known traps
- recent resolved errors
- durable decisions

The pack should be intentionally short and explain why project-specific items matched.

### 5. Improve deduplication and consolidation around operational keys

Current deduplication is summary-similarity oriented. It should also merge or suppress near-duplicates that share the same:

- `kind`
- `project`
- `platform`
- `command`
- `error_fingerprint`

This reduces recall noise and improves wake-up quality.

### 6. Expose the same model across CLI, MCP, and hooks

The same memory shape and ranking rules should be available through:

- CLI: `store`, `recall`, `wake-up`, hook entrypoints
- MCP: `icm_memory_store`, `icm_memory_recall`, `icm_wake_up`
- future web/API surfaces

That avoids tool-specific behavior drift.

## File Ownership

- `crates/icm-core/src/memory.rs`
  Add `MemoryContext`, context kind, and serialization support.

- `crates/icm-core/src/wake_up.rs`
  Re-rank and re-render the startup pack using operational categories.

- `crates/icm-core/src/store.rs`
  Extend trait surfaces only where operational retrieval needs new entrypoints.

- `crates/icm-store/src/schema.rs`
  Add persistent storage for the new context payload and any supporting indexes.

- `crates/icm-store/src/store.rs`
  Read/write the new metadata and implement context-aware ranking helpers.

- `crates/icm-mcp/src/tools.rs`
  Accept context-aware arguments and return higher-signal recall results.

- `crates/icm-mcp/src/server.rs`
  Keep store nudges, but nudge toward stable operational memories specifically.

- `crates/icm-cli/src/main.rs`
  Add CLI flags and hook-driven auto-context enrichment for store/recall/wake-up.

- `crates/icm-cli/src/config.rs`
  Add config knobs for operational ranking and wake-up sections.

## Roadmap

### Phase 1: Metadata foundation

- add `MemoryContext`
- persist it in SQLite
- keep backward compatibility for old rows

### Phase 2: Context-aware recall

- parse optional context inputs in CLI and MCP
- rank by project/platform/command/error/file/symbol
- preserve current FTS and embedding behavior as fallback

### Phase 3: Operational wake-up packs

- render sections for commands, traps, errors, and decisions
- exclude ephemeral memory kinds by default
- show a compact match explanation when project filtering is active

### Phase 4: Better dedup and auto-store quality

- dedup by operational keys, not only summary similarity
- add capture helpers for resolved errors and working commands
- tighten prompts and server nudges around stable operational facts

## Non-Goals For This Increment

- replacing memories with a graph-only model
- building the 3D memoir visualization
- adding cloud-only agent memory features
- broad UI redesign of the web dashboard

## Success Criteria

The design is successful when:

- coding agents can recall the right build/test/fix command with less manual prompting
- resolved errors on a specific platform are easier to find by exact failure shape
- wake-up packs contain fewer generic summaries and more actionable project facts
- duplicate operational memories collapse instead of spamming recall results
- existing memory rows and existing tool consumers remain compatible