#!/usr/bin/env bun
// Spin up N parallel "agent" simulations of ICM usage.
//
// Each agent gets:
//   - A fresh temp dir + brand-new SQLite DB (--db flag)
//   - A scenario (Rust debug, infra incident, etc.) that drives a realistic
//     sequence of store / recall / wake-up / forget commands
//
// We measure per-command latency, correctness (recall hits, dedup behavior),
// and aggregate everything into a single report.
//
// Usage:
//   bun scripts/bench-agent-sim.ts                 # 10 agents, default binary
//   bun scripts/bench-agent-sim.ts --agents 20
//   bun scripts/bench-agent-sim.ts --binary ./target/release/icm
//   bun scripts/bench-agent-sim.ts --keep-dbs      # keep /tmp dirs for inspection

import { spawn } from "node:child_process";
import { mkdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

// ─────────────────────────────────────────────────────────────────────────────
// CLI args
// ─────────────────────────────────────────────────────────────────────────────

type ContentSize = "small" | "medium" | "large" | "huge";

const CONTENT_SIZE_CHARS: Record<ContentSize, number> = {
  small: 80,    // one short sentence
  medium: 300,  // one paragraph
  large: 1200,  // multi-paragraph note
  huge: 5000,   // long doc / transcript chunk
};

interface Options {
  agents: number;
  memoriesPerAgent: number;
  recallsPerAgent: number;
  contentSize: ContentSize;
  binary: string;
  keepDbs: boolean;
  outDir: string;
  verbose: boolean;
  withEmbeddings: boolean;
  sweep: boolean;
}

function parseArgs(argv: string[]): Options {
  const opts: Options = {
    agents: 10,
    memoriesPerAgent: 5,
    recallsPerAgent: 3,
    contentSize: "small",
    binary: process.env.ICM_BIN ?? "icm",
    keepDbs: false,
    outDir: join(tmpdir(), `icm-agent-sim-${dateStamp()}`),
    verbose: false,
    withEmbeddings: false,
    sweep: false,
  };
  for (let i = 2; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--agents") opts.agents = parseInt(argv[++i] ?? "10", 10);
    else if (a === "--memories-per-agent" || a === "-m") opts.memoriesPerAgent = parseInt(argv[++i] ?? "5", 10);
    else if (a === "--recalls-per-agent" || a === "-r") opts.recallsPerAgent = parseInt(argv[++i] ?? "3", 10);
    else if (a === "--content-size") opts.contentSize = (argv[++i] ?? "small") as ContentSize;
    else if (a === "--binary") opts.binary = argv[++i] ?? opts.binary;
    else if (a === "--keep-dbs") opts.keepDbs = true;
    else if (a === "--out") opts.outDir = argv[++i] ?? opts.outDir;
    else if (a === "--with-embeddings") opts.withEmbeddings = true;
    else if (a === "--sweep") opts.sweep = true;
    else if (a === "--verbose" || a === "-v") opts.verbose = true;
    else if (a === "--help" || a === "-h") {
      console.log(
        "Usage: bun scripts/bench-agent-sim.ts [options]",
      );
      console.log("");
      console.log("  --agents N                Number of parallel agents (default 10)");
      console.log("  --memories-per-agent N    Memories stored per agent (default 5)");
      console.log("  --recalls-per-agent N     Recall queries per agent (default 3)");
      console.log("  --content-size SIZE       small | medium | large | huge (default small)");
      console.log("                            ~80 / 300 / 1200 / 5000 chars per memory");
      console.log("  --sweep                   Run a matrix: agents × content-size");
      console.log("  --with-embeddings         Exercise the vector path (slow first run)");
      console.log("  --binary PATH             icm binary to test (default 'icm' on PATH)");
      console.log("  --keep-dbs                Keep /tmp DBs after the run");
      console.log("  --out DIR                 Output directory");
      console.log("  -v, --verbose             Stream per-command timings");
      process.exit(0);
    }
  }
  if (!(opts.contentSize in CONTENT_SIZE_CHARS)) {
    console.error(`[fatal] --content-size must be one of: ${Object.keys(CONTENT_SIZE_CHARS).join(", ")}`);
    process.exit(1);
  }
  return opts;
}

function dateStamp(): string {
  const d = new Date();
  const pad = (n: number) => n.toString().padStart(2, "0");
  return `${d.getFullYear()}${pad(d.getMonth() + 1)}${pad(d.getDate())}-${pad(d.getHours())}${pad(d.getMinutes())}${pad(d.getSeconds())}`;
}

// ─────────────────────────────────────────────────────────────────────────────
// Scenario bank
// ─────────────────────────────────────────────────────────────────────────────

interface Memory {
  topic: string;
  content: string;
  importance?: "critical" | "high" | "medium" | "low";
  keywords?: string;
}

interface Scenario {
  name: string;
  project: string;
  seed: Memory[];
  // Recall queries paired with one of the seed contents we expect to surface.
  // expectMatch is a substring we look for in the recall stdout.
  recalls: Array<{ query: string; expectMatch: string }>;
  // A near-duplicate of seed[0] used to test dedup at insert-time.
  duplicateVariant: Memory;
}

const SCENARIOS: Scenario[] = [
  {
    name: "rust-debug",
    project: "engine",
    seed: [
      { topic: "decisions-engine", content: "Use thiserror for typed errors and anyhow at the binary boundary", importance: "high", keywords: "rust,error,thiserror" },
      { topic: "decisions-engine", content: "Replace unwrap with ? at all I/O boundaries", importance: "high" },
      { topic: "errors-resolved", content: "Tokio task panic was caused by Send bound on Rc; switched to Arc", importance: "medium", keywords: "tokio,async,arc" },
      { topic: "preferences", content: "Always run cargo clippy with -D warnings before committing", importance: "critical" },
    ],
    recalls: [
      { query: "error handling library choice", expectMatch: "thiserror" },
      { query: "tokio panic Rc Send", expectMatch: "Arc" },
      { query: "lint enforcement", expectMatch: "clippy" },
    ],
    duplicateVariant: { topic: "Decisions-Engine", content: "  use thiserror for typed errors and anyhow at the binary boundary  ", importance: "high" },
  },
  {
    name: "ts-feature",
    project: "web",
    seed: [
      { topic: "decisions-web", content: "Use SvelteKit form actions for mutations, no client fetches", importance: "high", keywords: "sveltekit,forms" },
      { topic: "decisions-web", content: "All translatable strings live in src/lib/i18n/locales/*.json", importance: "high" },
      { topic: "preferences", content: "No French text in code identifiers, comments, or strings", importance: "critical", keywords: "i18n,language" },
      { topic: "errors-resolved", content: "Hydration mismatch from Date.now in render fixed by computing in load", importance: "medium" },
    ],
    recalls: [
      { query: "SvelteKit mutation pattern", expectMatch: "form actions" },
      { query: "hydration error fix", expectMatch: "load" },
      { query: "translation file location", expectMatch: "i18n" },
    ],
    duplicateVariant: { topic: "decisions-web", content: "USE SVELTEKIT FORM ACTIONS FOR MUTATIONS, NO CLIENT FETCHES", importance: "high" },
  },
  {
    name: "infra-incident",
    project: "ops",
    seed: [
      { topic: "incidents", content: "DB connection pool exhausted at 18:32 UTC; raised pgbouncer pool_size 20 to 50", importance: "high", keywords: "postgres,incident" },
      { topic: "decisions-ops", content: "Alert on p99 latency above 500ms for 5 minutes", importance: "high" },
      { topic: "runbooks", content: "Failover to replica via patronictl switchover before draining primary", importance: "critical" },
      { topic: "preferences", content: "Always create a postmortem within 24h of a P1 incident", importance: "high" },
    ],
    recalls: [
      { query: "pool exhausted fix", expectMatch: "pgbouncer" },
      { query: "failover procedure", expectMatch: "patronictl" },
      { query: "alerting threshold", expectMatch: "p99" },
    ],
    duplicateVariant: { topic: "incidents", content: "db connection pool exhausted at 18:32 UTC; raised pgbouncer pool_size 20 to 50", importance: "high" },
  },
  {
    name: "schema-migration",
    project: "data",
    seed: [
      { topic: "decisions-data", content: "Adding a NOT NULL column requires three-step deploy: nullable add, backfill, NOT NULL constraint", importance: "critical", keywords: "migration,postgres" },
      { topic: "errors-resolved", content: "Migration timeout fixed by setting statement_timeout to 0 for the session", importance: "medium" },
      { topic: "preferences", content: "All migrations must be idempotent and reversible", importance: "high" },
    ],
    recalls: [
      { query: "not null deploy strategy", expectMatch: "three-step" },
      { query: "long migration timeout", expectMatch: "statement_timeout" },
      { query: "migration policy", expectMatch: "idempotent" },
    ],
    duplicateVariant: { topic: "decisions-data", content: "adding a not null column requires three-step deploy: nullable add, backfill, not null constraint", importance: "critical" },
  },
  {
    name: "perf-investigation",
    project: "api",
    seed: [
      { topic: "decisions-api", content: "Switch hot path from JSON to MessagePack saves 40% bandwidth", importance: "high", keywords: "perf,serialization" },
      { topic: "errors-resolved", content: "Allocator pressure cut by replacing String with Cow<str> in hot loop", importance: "medium" },
      { topic: "preferences", content: "Always profile with perf before optimizing", importance: "high" },
    ],
    recalls: [
      { query: "serialization saves bandwidth", expectMatch: "MessagePack" },
      { query: "string allocation hot loop", expectMatch: "Cow" },
      { query: "optimization process", expectMatch: "perf" },
    ],
    duplicateVariant: { topic: "decisions-api", content: "switch hot path from json to messagepack saves 40% bandwidth", importance: "high" },
  },
  {
    name: "auth-rework",
    project: "auth",
    seed: [
      { topic: "decisions-auth", content: "Move from JWT in localStorage to httpOnly secure cookies", importance: "critical", keywords: "auth,security" },
      { topic: "decisions-auth", content: "Refresh token rotation on every use, sliding window 30 days", importance: "high" },
      { topic: "errors-resolved", content: "CSRF bypass closed by enforcing SameSite=Lax on session cookie", importance: "high" },
    ],
    recalls: [
      { query: "where to store session token", expectMatch: "httpOnly" },
      { query: "refresh token rotation", expectMatch: "sliding window" },
      { query: "csrf fix", expectMatch: "SameSite" },
    ],
    duplicateVariant: { topic: "decisions-auth", content: "MOVE FROM JWT IN LOCALSTORAGE TO HTTPONLY SECURE COOKIES", importance: "critical" },
  },
  {
    name: "ci-troubleshoot",
    project: "ci",
    seed: [
      { topic: "decisions-ci", content: "Cache cargo registry and target dirs with sccache to cut CI from 18min to 6min", importance: "high", keywords: "ci,cache,sccache" },
      { topic: "errors-resolved", content: "Flaky test fixed by removing time.Sleep in favor of polling with timeout", importance: "medium" },
      { topic: "preferences", content: "No --no-verify; always fix the hook failure", importance: "critical" },
    ],
    recalls: [
      { query: "ci speed cache", expectMatch: "sccache" },
      { query: "flaky test fix", expectMatch: "polling" },
      { query: "git hook policy", expectMatch: "no-verify" },
    ],
    duplicateVariant: { topic: "decisions-ci", content: "cache cargo registry and target dirs with sccache to cut ci from 18min to 6min", importance: "high" },
  },
  {
    name: "doc-writing",
    project: "docs",
    seed: [
      { topic: "preferences", content: "Documentation in English; UI translations via i18n locale files", importance: "critical", keywords: "docs,language" },
      { topic: "decisions-docs", content: "Use mdBook for project docs, deployed via GitHub Pages", importance: "medium" },
      { topic: "decisions-docs", content: "Every public type needs a rustdoc example", importance: "high" },
    ],
    recalls: [
      { query: "doc tooling", expectMatch: "mdBook" },
      { query: "rustdoc requirements", expectMatch: "example" },
      { query: "documentation language", expectMatch: "English" },
    ],
    duplicateVariant: { topic: "preferences", content: "documentation in english; ui translations via i18n locale files", importance: "critical" },
  },
  {
    name: "review-cycle",
    project: "review",
    seed: [
      { topic: "preferences", content: "Prefer one bundled PR over many small PRs for refactors in shared modules", importance: "high", keywords: "review,refactor" },
      { topic: "decisions-review", content: "Reviews must run /ultrareview before approving migrations", importance: "high" },
      { topic: "errors-resolved", content: "Squash-merge PR with conventional commit subject for release-please to detect", importance: "medium" },
    ],
    recalls: [
      { query: "pr granularity refactor", expectMatch: "bundled" },
      { query: "migration review", expectMatch: "ultrareview" },
      { query: "release please commit format", expectMatch: "conventional" },
    ],
    duplicateVariant: { topic: "preferences", content: "PREFER ONE BUNDLED PR OVER MANY SMALL PRS FOR REFACTORS IN SHARED MODULES", importance: "high" },
  },
  {
    name: "python-refactor",
    project: "datapipe",
    seed: [
      { topic: "decisions-datapipe", content: "Switch from pandas to polars for the ETL hot path: 6x speedup observed", importance: "high", keywords: "python,polars" },
      { topic: "errors-resolved", content: "Memory leak in long-running worker fixed by replacing global cache with functools.lru_cache(maxsize=1024)", importance: "medium" },
      { topic: "preferences", content: "Type hints required on all public functions; mypy strict mode enabled", importance: "high" },
    ],
    recalls: [
      { query: "etl performance dataframe", expectMatch: "polars" },
      { query: "worker memory leak", expectMatch: "lru_cache" },
      { query: "typing rules", expectMatch: "mypy" },
    ],
    duplicateVariant: { topic: "decisions-datapipe", content: "  switch from pandas to polars for the etl hot path: 6x speedup observed  ", importance: "high" },
  },
];

// ─────────────────────────────────────────────────────────────────────────────
// Memory generator — synthesizes additional memories beyond seed bank
// ─────────────────────────────────────────────────────────────────────────────

const FILLER_VOCAB = [
  "decision", "rationale", "constraint", "tradeoff", "rollout", "deprecation",
  "migration", "regression", "incident", "postmortem", "rollback", "canary",
  "throughput", "latency", "p95", "p99", "saturation", "headroom", "backpressure",
  "schema", "indexing", "sharding", "replication", "consistency", "isolation",
  "encoding", "compression", "checkpoint", "snapshot", "ledger", "audit",
  "compatibility", "feature-flag", "guardrail", "fallback", "degradation",
  "embedding", "tokenizer", "attention", "context-window", "retrieval", "rerank",
  "ownership", "boundary", "interface", "abstraction", "invariant", "contract",
];

const TOPICS_BANK = [
  "decisions-engine", "decisions-api", "decisions-data", "decisions-ops",
  "decisions-web", "decisions-auth", "errors-resolved", "preferences",
  "incidents", "runbooks", "performance-notes", "review-notes",
];

function makeFiller(targetChars: number, seed: number): string {
  // Deterministic filler text targeting `targetChars`. Built from FILLER_VOCAB
  // shuffled by `seed` and joined into prose-like sentences.
  let s = "";
  let i = seed;
  while (s.length < targetChars) {
    const a = FILLER_VOCAB[i % FILLER_VOCAB.length]!;
    const b = FILLER_VOCAB[(i * 7 + 3) % FILLER_VOCAB.length]!;
    const c = FILLER_VOCAB[(i * 13 + 5) % FILLER_VOCAB.length]!;
    s += `The ${a} on ${b} drives the ${c} budget. `;
    i++;
  }
  return s.slice(0, targetChars);
}

interface GeneratedMemory extends Memory {
  uniqueTag: string; // grep target for recall verification
}

function generateExtraMemories(
  scenario: Scenario,
  count: number,
  contentSize: ContentSize,
): GeneratedMemory[] {
  const target = CONTENT_SIZE_CHARS[contentSize];
  const out: GeneratedMemory[] = [];
  for (let i = 0; i < count; i++) {
    const topic = TOPICS_BANK[(i * 3 + scenario.name.length) % TOPICS_BANK.length]!;
    const uniqueTag = `MARK_${scenario.name}_${i.toString().padStart(4, "0")}`;
    // Embed the unique tag at the start so any recall on `MARK_<scenario>_NNNN`
    // can reliably hit the right memory regardless of fuzziness.
    const headline = `${uniqueTag}: synthesized memory ${i} for scenario ${scenario.name}.`;
    const filler = makeFiller(Math.max(0, target - headline.length - 1), i + 1);
    const content = `${headline} ${filler}`.slice(0, target);
    out.push({
      topic,
      content,
      importance: i % 5 === 0 ? "high" : "medium",
      keywords: `${scenario.name},gen,${uniqueTag}`,
      uniqueTag,
    });
  }
  return out;
}

// ─────────────────────────────────────────────────────────────────────────────
// Subprocess helpers
// ─────────────────────────────────────────────────────────────────────────────

interface CommandResult {
  command: string;
  args: string[];
  durationMs: number;
  exitCode: number;
  stdout: string;
  stderr: string;
}

function runIcm(binary: string, args: string[]): Promise<CommandResult> {
  return new Promise((resolve) => {
    const start = performance.now();
    const child = spawn(binary, args, { stdio: ["ignore", "pipe", "pipe"] });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (d) => { stdout += d.toString(); });
    child.stderr.on("data", (d) => { stderr += d.toString(); });
    child.on("close", (code) => {
      resolve({
        command: binary,
        args,
        durationMs: performance.now() - start,
        exitCode: code ?? -1,
        stdout,
        stderr,
      });
    });
    child.on("error", (err) => {
      resolve({
        command: binary,
        args,
        durationMs: performance.now() - start,
        exitCode: -1,
        stdout,
        stderr: stderr + `\nspawn error: ${err.message}`,
      });
    });
  });
}

// ─────────────────────────────────────────────────────────────────────────────
// Single agent simulation
// ─────────────────────────────────────────────────────────────────────────────

interface AgentResult {
  id: number;
  scenario: string;
  workspace: string;
  dbPath: string;
  totalDurationMs: number;
  commands: CommandResult[];
  failures: number;
  storedCount: number;        // unique rows after insert
  insertAttempts: number;     // total store calls
  dedupedCount: number;       // attempts - actual rows added
  recallHits: number;
  recallMisses: number;
  totalContentChars: number;  // sum of content sizes submitted
  dbSizeBytes: number;        // final on-disk DB size
}

async function runAgent(id: number, opts: Options, outRoot: string): Promise<AgentResult> {
  const scenario = SCENARIOS[id % SCENARIOS.length]!;
  const workspace = join(outRoot, `agent-${id.toString().padStart(2, "0")}-${scenario.name}`);
  await mkdir(workspace, { recursive: true });
  const dbPath = join(workspace, "icm.db");
  const dbFlag = ["--db", dbPath];
  const embedFlag = opts.withEmbeddings ? [] : ["--no-embeddings"];

  const commands: CommandResult[] = [];
  let failures = 0;
  let insertAttempts = 0;
  let recallHits = 0;
  let recallMisses = 0;
  let totalContentChars = 0;

  const exec = async (args: string[]) => {
    const r = await runIcm(opts.binary, args);
    commands.push(r);
    if (r.exitCode !== 0) failures++;
    if (opts.verbose) {
      const tag = r.exitCode === 0 ? "ok " : "ERR";
      console.error(`[agent-${id}] ${tag} ${r.durationMs.toFixed(0)}ms  ${args.slice(0, 4).join(" ")}…`);
    }
    return r;
  };

  // Decide which memories this agent will store.
  // First memoriesPerAgent slots come from the seed bank (capped at seed.length),
  // remaining slots are synthesized by the generator.
  const seedCount = Math.min(opts.memoriesPerAgent, scenario.seed.length);
  const seedSlice = scenario.seed.slice(0, seedCount);
  const generated = generateExtraMemories(
    scenario,
    Math.max(0, opts.memoriesPerAgent - seedCount),
    opts.contentSize,
  );

  // Build recall plan: first scenario.recalls (clamped), then synthetic recalls
  // hitting unique tags from the generated memories. This guarantees the recall
  // pool scales with --recalls-per-agent without depending on keyword overlap.
  const recallPlan: Array<{ query: string; expectMatch: string }> = [];
  const seedRecallCount = Math.min(opts.recallsPerAgent, scenario.recalls.length);
  recallPlan.push(...scenario.recalls.slice(0, seedRecallCount));
  let synthIdx = 0;
  while (recallPlan.length < opts.recallsPerAgent && synthIdx < generated.length) {
    const m = generated[synthIdx]!;
    recallPlan.push({ query: m.uniqueTag, expectMatch: m.uniqueTag });
    synthIdx++;
  }

  const start = performance.now();

  // 1. Cold wake-up on empty DB
  await exec([...dbFlag, ...embedFlag, "wake-up", "--project", scenario.project]);

  // 2. Seed memories (real)
  for (const m of seedSlice) {
    insertAttempts++;
    totalContentChars += m.content.length;
    const args = [...dbFlag, ...embedFlag, "store", "-t", m.topic, "-c", m.content];
    if (m.importance) args.push("-i", m.importance);
    if (m.keywords) args.push("-k", m.keywords);
    await exec(args);
  }

  // 2b. Synthesized memories (scale with --memories-per-agent + --content-size)
  for (const m of generated) {
    insertAttempts++;
    totalContentChars += m.content.length;
    const args = [...dbFlag, ...embedFlag, "store", "-t", m.topic, "-c", m.content];
    if (m.importance) args.push("-i", m.importance);
    if (m.keywords) args.push("-k", m.keywords);
    await exec(args);
  }

  // 3. Recall round
  for (const r of recallPlan) {
    const result = await exec([...dbFlag, ...embedFlag, "recall", r.query, "--limit", "5"]);
    if (result.exitCode === 0 && result.stdout.toLowerCase().includes(r.expectMatch.toLowerCase())) {
      recallHits++;
    } else {
      recallMisses++;
    }
  }

  // 4. Dedup test — attempt to store a near-duplicate of seed[0]
  if (seedSlice.length > 0) {
    insertAttempts++;
    totalContentChars += scenario.duplicateVariant.content.length;
    await exec([
      ...dbFlag, ...embedFlag, "store",
      "-t", scenario.duplicateVariant.topic,
      "-c", scenario.duplicateVariant.content,
      ...(scenario.duplicateVariant.importance ? ["-i", scenario.duplicateVariant.importance] : []),
    ]);
  }

  // 5. Stats — read final row count
  const stats = await exec([...dbFlag, ...embedFlag, "stats"]);
  const storedCount = parseStoredCount(stats.stdout);

  // 6. Warm wake-up — should hit cache, fast
  await exec([...dbFlag, ...embedFlag, "wake-up", "--project", scenario.project]);

  // 7. Topics listing
  await exec([...dbFlag, ...embedFlag, "topics"]);

  const totalDurationMs = performance.now() - start;
  const dedupedCount = insertAttempts - storedCount;

  // Measure final DB size on disk
  let dbSizeBytes = 0;
  try {
    const { statSync } = await import("node:fs");
    dbSizeBytes = statSync(dbPath).size;
  } catch {
    dbSizeBytes = 0;
  }

  // Persist per-agent transcript
  const transcript = {
    id, scenario: scenario.name, project: scenario.project,
    workspace, dbPath,
    config: {
      memoriesPerAgent: opts.memoriesPerAgent,
      recallsPerAgent: opts.recallsPerAgent,
      contentSize: opts.contentSize,
      contentSizeChars: CONTENT_SIZE_CHARS[opts.contentSize],
      withEmbeddings: opts.withEmbeddings,
    },
    summary: {
      totalDurationMs, failures, insertAttempts, storedCount, dedupedCount,
      recallHits, recallMisses, totalContentChars, dbSizeBytes,
    },
    commands: commands.map(c => ({
      args: c.args.filter((_, i, arr) => !(arr[i - 1] === "--db")),
      durationMs: Number(c.durationMs.toFixed(2)),
      exitCode: c.exitCode,
      stdoutPreview: c.stdout.slice(0, 200),
      stderrPreview: c.stderr.slice(0, 200),
    })),
  };
  await writeFile(join(workspace, "transcript.json"), JSON.stringify(transcript, null, 2));

  return {
    id, scenario: scenario.name, workspace, dbPath,
    totalDurationMs, commands, failures,
    storedCount, insertAttempts, dedupedCount,
    recallHits, recallMisses, totalContentChars, dbSizeBytes,
  };
}

function parseStoredCount(statsOutput: string): number {
  // Stats prints something like "Memories: 4" or "memories=4" — try a few patterns.
  const patterns = [
    /memor(?:y|ies)[:\s=]+(\d+)/i,
    /total[:\s=]+(\d+)/i,
    /count[:\s=]+(\d+)/i,
  ];
  for (const re of patterns) {
    const m = statsOutput.match(re);
    if (m && m[1]) return parseInt(m[1], 10);
  }
  return -1;
}

// ─────────────────────────────────────────────────────────────────────────────
// Aggregation + reporting
// ─────────────────────────────────────────────────────────────────────────────

function pct(n: number, d: number): string {
  if (d === 0) return "n/a";
  return ((n / d) * 100).toFixed(1) + "%";
}

function ms(n: number): string {
  return n < 1000 ? `${n.toFixed(0)}ms` : `${(n / 1000).toFixed(2)}s`;
}

function p(arr: number[], q: number): number {
  if (arr.length === 0) return 0;
  const sorted = [...arr].sort((a, b) => a - b);
  const idx = Math.min(sorted.length - 1, Math.max(0, Math.floor(q * sorted.length)));
  return sorted[idx]!;
}

function bytes(n: number): string {
  if (n < 1024) return `${n}B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)}KB`;
  return `${(n / 1024 / 1024).toFixed(2)}MB`;
}

function bucketSubcommand(args: string[]): string {
  for (let i = 0; i < args.length; i++) {
    const a = args[i]!;
    if (a === "--db") { i++; continue; }
    if (a.startsWith("-")) continue;
    return a;
  }
  return args[0] ?? "?";
}

function aggregate(results: AgentResult[], opts: Options, wallMs: number): string {
  const all = results.flatMap(r => r.commands);
  const byCmd = new Map<string, number[]>();
  for (const c of all) {
    const sub = bucketSubcommand(c.args);
    if (!byCmd.has(sub)) byCmd.set(sub, []);
    byCmd.get(sub)!.push(c.durationMs);
  }

  const totalCommands = all.length;
  const totalFailures = results.reduce((s, r) => s + r.failures, 0);
  const totalRecallHits = results.reduce((s, r) => s + r.recallHits, 0);
  const totalRecallTotal = results.reduce((s, r) => s + r.recallHits + r.recallMisses, 0);
  const totalDedup = results.reduce((s, r) => s + r.dedupedCount, 0);
  const totalInserts = results.reduce((s, r) => s + r.insertAttempts, 0);
  const totalStored = results.reduce((s, r) => s + r.storedCount, 0);
  const totalContentChars = results.reduce((s, r) => s + r.totalContentChars, 0);
  const totalDbBytes = results.reduce((s, r) => s + r.dbSizeBytes, 0);

  // Throughput is over the wall time of the parallel batch (longest agent run
  // since they run concurrently).
  const wallSec = wallMs / 1000;
  const opsPerSec = wallSec > 0 ? totalCommands / wallSec : 0;
  const insertsPerSec = wallSec > 0 ? totalStored / wallSec : 0;
  const charsPerSec = wallSec > 0 ? totalContentChars / wallSec : 0;

  const lines: string[] = [];
  lines.push("# ICM Agent-Simulation Benchmark");
  lines.push("");
  lines.push(`Binary           : ${opts.binary}`);
  lines.push(`Agents           : ${results.length}`);
  lines.push(`Memories/agent   : ${opts.memoriesPerAgent} (seed + synthetic)`);
  lines.push(`Recalls/agent    : ${opts.recallsPerAgent}`);
  lines.push(`Content size     : ${opts.contentSize} (~${CONTENT_SIZE_CHARS[opts.contentSize]} chars/memory)`);
  lines.push(`Embeddings       : ${opts.withEmbeddings ? "ON (vector path)" : "OFF (keyword only)"}`);
  lines.push(`Total commands   : ${totalCommands}`);
  lines.push(`Wall time        : ${ms(wallMs)}`);
  lines.push("");
  lines.push("## Throughput");
  lines.push("");
  lines.push(`- **${opsPerSec.toFixed(0)} ops/sec** total (commands across all agents)`);
  lines.push(`- **${insertsPerSec.toFixed(0)} inserts/sec** (stored rows / wall)`);
  lines.push(`- **${(charsPerSec / 1024).toFixed(1)} KB/sec** content ingested`);
  lines.push(`- Total content ingested: ${bytes(totalContentChars)}; total DB on-disk: ${bytes(totalDbBytes)}`);
  lines.push("");
  lines.push("## Per-agent summary");
  lines.push("");
  lines.push("| # | scenario | wall | cmds | fail | inserts | stored | dedup | recall hit | DB size |");
  lines.push("|---|---|---|---|---|---|---|---|---|---|");
  for (const r of results) {
    lines.push(
      `| ${r.id} | ${r.scenario} | ${ms(r.totalDurationMs)} | ${r.commands.length} | ${r.failures} | ${r.insertAttempts} | ${r.storedCount} | ${r.dedupedCount} | ${r.recallHits}/${r.recallHits + r.recallMisses} | ${bytes(r.dbSizeBytes)} |`,
    );
  }
  lines.push("");

  lines.push("## Aggregate");
  lines.push("");
  lines.push(`- Command success rate: **${pct(totalCommands - totalFailures, totalCommands)}** (${totalCommands - totalFailures}/${totalCommands})`);
  lines.push(`- Recall hit rate: **${pct(totalRecallHits, totalRecallTotal)}** (${totalRecallHits}/${totalRecallTotal})`);
  lines.push(`- Dedup rate on insert: **${pct(totalDedup, totalInserts)}** (${totalDedup}/${totalInserts}; ≥1 expected per agent from the duplicate variant)`);
  lines.push("");
  lines.push("## Per-subcommand latency");
  lines.push("");
  lines.push("| subcommand | n | p50 | p95 | p99 | max |");
  lines.push("|---|---|---|---|---|---|");
  for (const [sub, durs] of [...byCmd.entries()].sort()) {
    lines.push(`| ${sub} | ${durs.length} | ${ms(p(durs, 0.5))} | ${ms(p(durs, 0.95))} | ${ms(p(durs, 0.99))} | ${ms(Math.max(...durs))} |`);
  }
  lines.push("");

  return lines.join("\n");
}

// Compact aggregate suitable for one row in a sweep table
interface SweepRow {
  agents: number;
  contentSize: ContentSize;
  contentChars: number;
  wallMs: number;
  totalCommands: number;
  failures: number;
  storedRows: number;
  recallHitPct: number;
  storeP50: number;
  storeP95: number;
  recallP50: number;
  recallP95: number;
  wakeupP50: number;
  wakeupP95: number;
  opsPerSec: number;
  insertsPerSec: number;
  totalDbBytes: number;
}

function summarizeForSweep(results: AgentResult[], opts: Options, wallMs: number): SweepRow {
  const all = results.flatMap(r => r.commands);
  const byCmd = new Map<string, number[]>();
  for (const c of all) {
    const sub = bucketSubcommand(c.args);
    if (!byCmd.has(sub)) byCmd.set(sub, []);
    byCmd.get(sub)!.push(c.durationMs);
  }
  const totalCommands = all.length;
  const totalFailures = results.reduce((s, r) => s + r.failures, 0);
  const totalRecallHits = results.reduce((s, r) => s + r.recallHits, 0);
  const totalRecallTotal = results.reduce((s, r) => s + r.recallHits + r.recallMisses, 0);
  const totalStored = results.reduce((s, r) => s + r.storedCount, 0);
  const totalDbBytes = results.reduce((s, r) => s + r.dbSizeBytes, 0);
  const wallSec = wallMs / 1000;
  return {
    agents: opts.agents,
    contentSize: opts.contentSize,
    contentChars: CONTENT_SIZE_CHARS[opts.contentSize],
    wallMs,
    totalCommands,
    failures: totalFailures,
    storedRows: totalStored,
    recallHitPct: totalRecallTotal > 0 ? (totalRecallHits / totalRecallTotal) * 100 : 0,
    storeP50: p(byCmd.get("store") ?? [], 0.5),
    storeP95: p(byCmd.get("store") ?? [], 0.95),
    recallP50: p(byCmd.get("recall") ?? [], 0.5),
    recallP95: p(byCmd.get("recall") ?? [], 0.95),
    wakeupP50: p(byCmd.get("wake-up") ?? [], 0.5),
    wakeupP95: p(byCmd.get("wake-up") ?? [], 0.95),
    opsPerSec: wallSec > 0 ? totalCommands / wallSec : 0,
    insertsPerSec: wallSec > 0 ? totalStored / wallSec : 0,
    totalDbBytes,
  };
}

function renderSweepTable(rows: SweepRow[]): string {
  const lines: string[] = [];
  lines.push("# ICM Sweep Benchmark");
  lines.push("");
  lines.push("Matrix of (agents × content-size). Each row is one batch; agents run in parallel.");
  lines.push("");
  lines.push("| agents | size | chars | wall | ops/s | ins/s | store p50 | store p95 | recall p50 | recall p95 | wake p50 | wake p95 | recall hit | DB total | fail |");
  lines.push("|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|");
  for (const r of rows) {
    lines.push(
      `| ${r.agents} | ${r.contentSize} | ${r.contentChars} | ${ms(r.wallMs)} ` +
      `| ${r.opsPerSec.toFixed(0)} | ${r.insertsPerSec.toFixed(0)} ` +
      `| ${ms(r.storeP50)} | ${ms(r.storeP95)} ` +
      `| ${ms(r.recallP50)} | ${ms(r.recallP95)} ` +
      `| ${ms(r.wakeupP50)} | ${ms(r.wakeupP95)} ` +
      `| ${r.recallHitPct.toFixed(0)}% | ${bytes(r.totalDbBytes)} | ${r.failures} |`,
    );
  }
  lines.push("");
  return lines.join("\n");
}

// ─────────────────────────────────────────────────────────────────────────────
// Entry point
// ─────────────────────────────────────────────────────────────────────────────

async function runOneBatch(opts: Options): Promise<{ results: AgentResult[]; wallMs: number }> {
  const t0 = performance.now();
  const results = await Promise.all(
    Array.from({ length: opts.agents }, (_, i) => runAgent(i, opts, opts.outDir)),
  );
  const wallMs = performance.now() - t0;
  return { results, wallMs };
}

async function main() {
  const opts = parseArgs(process.argv);
  await mkdir(opts.outDir, { recursive: true });

  // Sanity: binary present and works
  const probe = await runIcm(opts.binary, ["--version"]);
  if (probe.exitCode !== 0) {
    console.error(`[fatal] ${opts.binary} --version failed (${probe.exitCode}):\n${probe.stderr}`);
    process.exit(1);
  }

  console.log(`Binary  : ${opts.binary} (${probe.stdout.trim()})`);
  console.log(`Output  : ${opts.outDir}`);
  console.log(`Scenarios available: ${SCENARIOS.length}`);
  console.log("");

  if (opts.sweep) {
    // Matrix sweep: cartesian product of agent counts × content sizes.
    // Each batch goes into its own subdir so DBs and transcripts don't collide.
    const agentCounts = [10, 25, 50, 100];
    const sizes: ContentSize[] = ["small", "medium", "large", "huge"];
    const sweepRows: SweepRow[] = [];

    for (const agents of agentCounts) {
      for (const size of sizes) {
        const subdir = join(opts.outDir, `sweep-${agents}a-${size}`);
        await mkdir(subdir, { recursive: true });
        const subOpts: Options = {
          ...opts,
          agents,
          contentSize: size,
          outDir: subdir,
          // For huge content we keep memoriesPerAgent moderate to avoid blowing
          // up wall time on the larger batches; user can override explicitly.
          memoriesPerAgent: opts.memoriesPerAgent,
          recallsPerAgent: opts.recallsPerAgent,
        };
        process.stdout.write(`[sweep] ${agents} agents × ${size} (${CONTENT_SIZE_CHARS[size]}ch)… `);
        const { results, wallMs } = await runOneBatch(subOpts);
        const row = summarizeForSweep(results, subOpts, wallMs);
        sweepRows.push(row);
        const detail = aggregate(results, subOpts, wallMs);
        await writeFile(join(subdir, "report.md"), detail);
        console.log(`${ms(wallMs)} (${row.opsPerSec.toFixed(0)} ops/s)`);
      }
    }

    const sweepReport = renderSweepTable(sweepRows);
    const sweepPath = join(opts.outDir, "sweep.md");
    await writeFile(sweepPath, sweepReport);
    console.log("");
    console.log(sweepReport);
    console.log(`Sweep report: ${sweepPath}`);
    return;
  }

  console.log(`Agents  : ${opts.agents}`);
  console.log(`Memories: ${opts.memoriesPerAgent}/agent, ${opts.recallsPerAgent} recalls/agent`);
  console.log(`Content : ${opts.contentSize} (~${CONTENT_SIZE_CHARS[opts.contentSize]} chars)`);
  console.log("");
  console.log(`Spawning ${opts.agents} agents in parallel…`);

  const { results, wallMs } = await runOneBatch(opts);
  const report = aggregate(results, opts, wallMs);
  const reportPath = join(opts.outDir, "report.md");
  await writeFile(reportPath, report);

  console.log("");
  console.log(report);
  console.log("");
  console.log(`Report    : ${reportPath}`);
}

main().catch((err) => {
  console.error("[fatal]", err);
  process.exit(1);
});
