# ICM Agent Effectiveness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add operational memory metadata and context-aware retrieval so ICM becomes more effective for coding-agent workflows on real repositories and real developer machines.

**Architecture:** Extend the existing `Memory` record with a compact optional context payload, persist it in SQLite, and reuse that shape across CLI, MCP, and wake-up flows. Keep the current FTS/embedding pipeline, but add contextual ranking and wake-up rendering optimized for project facts, commands, platform quirks, and resolved errors.

**Tech Stack:** Rust, Clap, Serde, rusqlite/SQLite, FTS5, existing ICM CLI/MCP/store crates

---

## File Map

- Modify: `crates/icm-core/src/memory.rs`
  Define `MemoryContext` and related enums/serialization.

- Modify: `crates/icm-core/src/wake_up.rs`
  Add operational categories and filtering/ranking.

- Modify: `crates/icm-core/src/store.rs`
  Add any minimal trait helpers required for context-aware retrieval.

- Modify: `crates/icm-store/src/schema.rs`
  Persist the new context payload and add indexes if needed.

- Modify: `crates/icm-store/src/store.rs`
  Serialize/deserialize context and add ranking helpers/tests.

- Modify: `crates/icm-mcp/src/tools.rs`
  Accept context-aware store/recall arguments and update tests.

- Modify: `crates/icm-mcp/src/server.rs`
  Improve store nudges to target stable operational facts.

- Modify: `crates/icm-cli/src/config.rs`
  Add config for recall boosts and wake-up sections.

- Modify: `crates/icm-cli/src/main.rs`
  Add CLI flags and hook-driven context enrichment.

- Test: `crates/icm-mcp/src/tools.rs`
  Extend current MCP tests for wake-up and recall.

- Test: `crates/icm-store/src/store.rs`
  Add store-level tests for persistence and ranking helpers.

### Task 1: Add operational metadata to `Memory`

**Files:**
- Modify: `crates/icm-core/src/memory.rs`
- Modify: `crates/icm-store/src/schema.rs`
- Modify: `crates/icm-store/src/store.rs`

- [ ] **Step 1: Write the failing persistence test**

Add a store test that proves context survives round-trip storage.

```rust
#[test]
fn test_memory_context_round_trip() {
    let store = test_store();
    let mut memory = make_memory("context-icm", "Windows build uses cargo from user profile");
    memory.context = Some(MemoryContext {
        project: Some("icm".into()),
        repo_root: Some("D:/Projects/WORK/other/icm".into()),
        platform: Some("windows".into()),
        command: Some("cargo build --release -p icm-cli --features web".into()),
        file_path: Some("crates/icm-cli/src/main.rs".into()),
        symbol: Some("Commands::Init".into()),
        error_fingerprint: None,
        kind: MemoryKind::WorkingCommand,
    });

    let id = store.store(memory.clone()).unwrap();
    let fetched = store.get(&id).unwrap().unwrap();
    assert_eq!(fetched.context.unwrap().platform.as_deref(), Some("windows"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icm-store test_memory_context_round_trip -- --exact`

Expected: FAIL because `Memory` has no `context` field yet or store does not persist it.

- [ ] **Step 3: Add the new core types**

Extend `Memory` with an optional `context` field and supporting types.

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MemoryContext {
    pub project: Option<String>,
    pub repo_root: Option<String>,
    pub platform: Option<String>,
    pub command: Option<String>,
    pub file_path: Option<String>,
    pub symbol: Option<String>,
    pub error_fingerprint: Option<String>,
    pub kind: MemoryKind,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    Decision,
    ResolvedError,
    WorkingCommand,
    Constraint,
    Preference,
    Progress,
    #[default]
    Note,
}
```

- [ ] **Step 4: Persist the context payload in SQLite**

Add a nullable `context_data` JSON column and update read/write paths.

```sql
ALTER TABLE memories ADD COLUMN context_data TEXT;
```

```rust
let context_json = serde_json::to_string(&memory.context).map_err(IcmError::Serialization)?;
```

```rust
let context: Option<MemoryContext> = row
    .get::<_, Option<String>>(context_idx)?
    .and_then(|s| serde_json::from_str(&s).ok());
```

- [ ] **Step 5: Run focused tests**

Run: `cargo test -p icm-store test_memory_context_round_trip -- --exact`

Expected: PASS.

- [ ] **Step 6: Run a narrow compile check**

Run: `cargo test -p icm-core memory -- --nocapture`

Expected: PASS or no new type errors from the `Memory` changes.

### Task 2: Add context-aware recall ranking

**Files:**
- Modify: `crates/icm-store/src/store.rs`
- Modify: `crates/icm-mcp/src/tools.rs`
- Modify: `crates/icm-cli/src/main.rs`

- [ ] **Step 1: Write the failing recall ranking test**

Add an MCP test where two memories match by text, but only one matches by operational context.

```rust
#[test]
fn test_recall_prefers_matching_platform_and_command() {
    let store = test_store();
    call_tool(&store, None, "icm_memory_store", &json!({
        "topic": "context-icm",
        "content": "Build succeeds with cargo build --release",
        "importance": "high",
        "context": {
            "project": "icm",
            "platform": "windows",
            "command": "cargo build --release -p icm-cli --features web",
            "kind": "working_command"
        }
    }), false);
    call_tool(&store, None, "icm_memory_store", &json!({
        "topic": "context-icm",
        "content": "Build succeeds with cargo build --release",
        "importance": "high",
        "context": {
            "project": "icm",
            "platform": "linux",
            "command": "cargo build --release",
            "kind": "working_command"
        }
    }), false);

    let result = call_tool(&store, None, "icm_memory_recall", &json!({
        "query": "build release",
        "project": "icm",
        "platform": "windows",
        "command": "cargo build --release -p icm-cli --features web"
    }), false);

    assert!(!result.is_error);
    assert!(result.content[0].text.contains("features web"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icm-mcp test_recall_prefers_matching_platform_and_command -- --exact`

Expected: FAIL because recall does not accept or use these arguments yet.

- [ ] **Step 3: Extend MCP and CLI argument parsing**

Accept optional context filters on recall.

```rust
let project = args.get("project").and_then(|v| v.as_str());
let platform = args.get("platform").and_then(|v| v.as_str());
let command = args.get("command").and_then(|v| v.as_str());
let file_path = args.get("file_path").and_then(|v| v.as_str());
let symbol = args.get("symbol").and_then(|v| v.as_str());
let error_fingerprint = args.get("error_fingerprint").and_then(|v| v.as_str());
```

- [ ] **Step 4: Add a contextual rank boost helper in the store layer**

Keep existing search results, but re-rank them with context boosts.

```rust
fn contextual_boost(memory: &Memory, query: &RecallContext) -> f32 {
    let Some(ctx) = &memory.context else { return 0.0; };
    let mut boost = 0.0;
    if query.project == ctx.project.as_deref() { boost += 2.0; }
    if query.platform == ctx.platform.as_deref() { boost += 1.5; }
    if query.command == ctx.command.as_deref() { boost += 3.0; }
    if query.file_path == ctx.file_path.as_deref() { boost += 1.0; }
    if query.symbol == ctx.symbol.as_deref() { boost += 1.0; }
    if query.error_fingerprint == ctx.error_fingerprint.as_deref() { boost += 3.0; }
    boost
}
```

- [ ] **Step 5: Re-run the focused MCP test**

Run: `cargo test -p icm-mcp test_recall_prefers_matching_platform_and_command -- --exact`

Expected: PASS.

- [ ] **Step 6: Run a narrow end-to-end CLI check**

Run: `cargo test -p icm-cli recall -- --nocapture`

Expected: PASS or no new CLI parsing failures.

### Task 3: Make wake-up packs operational and shorter

**Files:**
- Modify: `crates/icm-core/src/wake_up.rs`
- Modify: `crates/icm-mcp/src/tools.rs`
- Modify: `crates/icm-cli/src/config.rs`

- [ ] **Step 1: Write the failing wake-up rendering test**

Add a test that expects operational sections instead of only generic categories.

```rust
#[test]
fn test_wake_up_includes_working_commands_and_known_traps() {
    let store = test_store();
    call_tool(&store, None, "icm_memory_store", &json!({
        "topic": "context-icm",
        "content": "Use cargo from C:/Users/times/.cargo/bin on Windows",
        "importance": "high",
        "context": {"project": "icm", "platform": "windows", "kind": "working_command", "command": "C:/Users/times/.cargo/bin/cargo.exe build --release -p icm-cli --features web"}
    }), false);
    call_tool(&store, None, "icm_memory_store", &json!({
        "topic": "errors-resolved",
        "content": "PowerShell alias icm shadows the binary",
        "importance": "high",
        "context": {"project": "icm", "platform": "windows", "kind": "resolved_error", "error_fingerprint": "powershell-alias-icm"}
    }), false);

    let result = call_tool(&store, None, "icm_wake_up", &json!({"project": "icm"}), false);
    let text = &result.content[0].text;
    assert!(text.contains("Working commands"));
    assert!(text.contains("Known traps"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icm-mcp test_wake_up_includes_working_commands_and_known_traps -- --exact`

Expected: FAIL because wake-up does not yet render those sections.

- [ ] **Step 3: Rework category mapping in `wake_up.rs`**

Add operational categories driven by `MemoryKind`.

```rust
enum Category {
    ProjectFacts,
    WorkingCommands,
    KnownTraps,
    ResolvedErrors,
    DurableDecisions,
    Preferences,
}
```

- [ ] **Step 4: Add config knobs for operational sections**

Add toggles for short packs and section limits.

```rust
pub struct WakeUpConfig {
    pub max_tokens: usize,
    pub include_preferences: bool,
    pub include_working_commands: bool,
    pub include_known_traps: bool,
    pub section_limit: usize,
}
```

- [ ] **Step 5: Re-run the focused test**

Run: `cargo test -p icm-mcp test_wake_up_includes_working_commands_and_known_traps -- --exact`

Expected: PASS.

- [ ] **Step 6: Run a second regression test for empty packs**

Run: `cargo test -p icm-mcp test_mcp_wake_up_empty_store -- --exact`

Expected: PASS.

### Task 4: Reduce noise with operational deduplication

**Files:**
- Modify: `crates/icm-mcp/src/tools.rs`
- Modify: `crates/icm-store/src/store.rs`
- Modify: `crates/icm-mcp/src/server.rs`

- [ ] **Step 1: Write the failing dedup test**

Add a test that stores the same resolved error twice with different wording and expects one durable memory.

```rust
#[test]
fn test_store_dedups_same_error_fingerprint() {
    let store = test_store();
    let first = call_tool(&store, None, "icm_memory_store", &json!({
        "topic": "errors-resolved",
        "content": "PowerShell alias icm points to Invoke-Command",
        "importance": "high",
        "context": {"project": "icm", "platform": "windows", "kind": "resolved_error", "error_fingerprint": "powershell-alias-icm"}
    }), false);
    let second = call_tool(&store, None, "icm_memory_store", &json!({
        "topic": "errors-resolved",
        "content": "Fixed icm alias conflict in PowerShell profile",
        "importance": "high",
        "context": {"project": "icm", "platform": "windows", "kind": "resolved_error", "error_fingerprint": "powershell-alias-icm"}
    }), false);

    assert!(!first.is_error && !second.is_error);
    let items = store.get_by_topic("errors-resolved").unwrap();
    assert_eq!(items.len(), 1);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icm-mcp test_store_dedups_same_error_fingerprint -- --exact`

Expected: FAIL because dedup only uses summary similarity.

- [ ] **Step 3: Add operational-key dedup before summary-similarity fallback**

Use context fields when available.

```rust
fn same_operational_fact(existing: &Memory, incoming: &Memory) -> bool {
    match (&existing.context, &incoming.context) {
        (Some(a), Some(b)) => {
            a.kind == b.kind
                && a.project == b.project
                && a.platform == b.platform
                && a.error_fingerprint.is_some()
                && a.error_fingerprint == b.error_fingerprint
        }
        _ => false,
    }
}
```

- [ ] **Step 4: Tighten the store reminder in the MCP server**

Bias the reminder toward durable operational facts instead of generic summaries.

```rust
result.append_hint(
    "\n[ICM: store durable facts such as resolved errors, working commands, decisions, or environment constraints before they are lost.]",
);
```

- [ ] **Step 5: Re-run the focused dedup test**

Run: `cargo test -p icm-mcp test_store_dedups_same_error_fingerprint -- --exact`

Expected: PASS.

- [ ] **Step 6: Run narrow regression coverage**

Run: `cargo test -p icm-mcp wake_up -- --nocapture`

Expected: PASS with no regression in current wake-up tool behavior.

### Task 5: Hook-driven auto-context enrichment for CLI store flows

**Files:**
- Modify: `crates/icm-cli/src/main.rs`
- Modify: `crates/icm-cli/src/config.rs`

- [ ] **Step 1: Write the failing helper test**

Add a test for a helper that enriches a memory with inferred project/platform context.

```rust
#[test]
fn test_build_auto_context_from_current_project() {
    let ctx = build_auto_context(Some("cargo test -p icm-cli"), None);
    assert_eq!(ctx.project.as_deref(), Some("icm"));
    assert!(ctx.platform.is_some());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icm-cli test_build_auto_context_from_current_project -- --exact`

Expected: FAIL because helper does not exist yet.

- [ ] **Step 3: Add the helper and CLI flags**

Allow explicit override, but default to automatic project and platform detection.

```rust
fn build_auto_context(command: Option<&str>, file_path: Option<&str>) -> MemoryContext {
    MemoryContext {
        project: Some(detect_project()),
        repo_root: std::env::current_dir().ok().map(|p| p.display().to_string()),
        platform: Some(std::env::consts::OS.to_string()),
        command: command.map(str::to_string),
        file_path: file_path.map(str::to_string),
        symbol: None,
        error_fingerprint: None,
        kind: MemoryKind::Note,
    }
}
```

- [ ] **Step 4: Use the helper in stable save flows**

Wire it into flows such as `cmd_save_project`, `hook start`, and future targeted store commands.

```rust
memory.context = Some(build_auto_context(command.as_deref(), file_path.as_deref()));
```

- [ ] **Step 5: Re-run the focused CLI helper test**

Run: `cargo test -p icm-cli test_build_auto_context_from_current_project -- --exact`

Expected: PASS.

- [ ] **Step 6: Run a compile-level validation for the touched crate**

Run: `cargo test -p icm-cli --no-run`

Expected: PASS.

## Final Verification

- [ ] **Step 1: Run targeted crate tests**

Run: `cargo test -p icm-store`

Expected: PASS.

- [ ] **Step 2: Run MCP tests**

Run: `cargo test -p icm-mcp`

Expected: PASS.

- [ ] **Step 3: Run CLI tests / compile check**

Run: `cargo test -p icm-cli --no-run`

Expected: PASS.

- [ ] **Step 4: Run workspace formatting and linting if already standard in the repo**

Run: `cargo fmt --check`

Expected: PASS.

Run: `cargo clippy --all-targets --all-features -- -D warnings`

Expected: PASS or pre-existing warnings only.

## Notes

- Keep the new context payload optional so existing databases remain readable.
- Do not block plain-text usage: topic/summary/keywords must still work when no context is supplied.
- Keep wake-up compact; operational value matters more than completeness.
- Do not move this work into memoirs yet. Make the common CLI/MCP memory path excellent first.