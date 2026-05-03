#!/usr/bin/env bun
// Measures ICM's qualitative correctness across four dimensions:
//
//   1. Recall    — for a labeled (query, expected target) pair, does ICM
//                  rank the target in top-K? Reports MRR, Recall@1, Recall@5.
//   2. Storage   — content survives a roundtrip with special chars / long
//                  payloads / unicode.
//   3. Consolid. — `icm consolidate` produces a summary that mentions the
//                  key concepts from each input memory.
//   4. Wake-up   — startup pack surfaces critical/high and filters low.
//
// Each dimension is scored 0-100. The overall verdict is the mean.
//
// Usage:
//   bun scripts/bench-quality.ts                  # all suites, no embeddings
//   bun scripts/bench-quality.ts --with-embeddings
//   bun scripts/bench-quality.ts --suite recall

import { spawn } from "node:child_process";
import { mkdir, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

// ─────────────────────────────────────────────────────────────────────────────
// CLI args
// ─────────────────────────────────────────────────────────────────────────────

interface Options {
  binary: string;
  outDir: string;
  withEmbeddings: boolean;
  suites: Suite[];
  verbose: boolean;
}

type Suite = "recall" | "storage" | "consolidation" | "wakeup";
const ALL_SUITES: Suite[] = ["recall", "storage", "consolidation", "wakeup"];

function parseArgs(argv: string[]): Options {
  const opts: Options = {
    binary: process.env.ICM_BIN ?? "icm",
    outDir: join(tmpdir(), `icm-quality-${dateStamp()}`),
    withEmbeddings: false,
    suites: ALL_SUITES,
    verbose: false,
  };
  for (let i = 2; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--binary") opts.binary = argv[++i] ?? opts.binary;
    else if (a === "--out") opts.outDir = argv[++i] ?? opts.outDir;
    else if (a === "--with-embeddings") opts.withEmbeddings = true;
    else if (a === "--suite") {
      const s = (argv[++i] ?? "") as Suite;
      if (!ALL_SUITES.includes(s)) {
        console.error(`[fatal] --suite must be one of ${ALL_SUITES.join(", ")}`);
        process.exit(1);
      }
      opts.suites = [s];
    }
    else if (a === "--verbose" || a === "-v") opts.verbose = true;
    else if (a === "--help" || a === "-h") {
      console.log("Usage: bun scripts/bench-quality.ts [--suite NAME] [--with-embeddings] [-v]");
      console.log("");
      console.log("Suites: recall | storage | consolidation | wakeup (default: all)");
      process.exit(0);
    }
  }
  return opts;
}

function dateStamp(): string {
  const d = new Date();
  const pad = (n: number) => n.toString().padStart(2, "0");
  return `${d.getFullYear()}${pad(d.getMonth() + 1)}${pad(d.getDate())}-${pad(d.getHours())}${pad(d.getMinutes())}${pad(d.getSeconds())}`;
}

// ─────────────────────────────────────────────────────────────────────────────
// icm subprocess helper
// ─────────────────────────────────────────────────────────────────────────────

interface RunResult {
  stdout: string;
  stderr: string;
  exitCode: number;
  durationMs: number;
}

function runIcm(binary: string, args: string[]): Promise<RunResult> {
  return new Promise((resolve) => {
    const start = performance.now();
    const child = spawn(binary, args, { stdio: ["ignore", "pipe", "pipe"] });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (d) => { stdout += d.toString(); });
    child.stderr.on("data", (d) => { stderr += d.toString(); });
    child.on("close", (code) => {
      resolve({ stdout, stderr, exitCode: code ?? -1, durationMs: performance.now() - start });
    });
    child.on("error", (err) => {
      resolve({ stdout, stderr: stderr + `\nspawn: ${err.message}`, exitCode: -1, durationMs: performance.now() - start });
    });
  });
}

interface IcmHandle {
  db: string;
  baseArgs: string[];
}

function newHandle(opts: Options, label: string): IcmHandle {
  const db = join(opts.outDir, `${label}.db`);
  const baseArgs = ["--db", db];
  if (!opts.withEmbeddings) baseArgs.push("--no-embeddings");
  return { db, baseArgs };
}

interface MemorySpec {
  topic: string;
  content: string;
  importance?: "critical" | "high" | "medium" | "low";
  keywords?: string;
}

async function store(opts: Options, h: IcmHandle, m: MemorySpec): Promise<RunResult> {
  const args = [...h.baseArgs, "store", "-t", m.topic, "-c", m.content];
  if (m.importance) args.push("-i", m.importance);
  if (m.keywords) args.push("-k", m.keywords);
  return runIcm(opts.binary, args);
}

interface RecalledMemory {
  id: string;
  topic: string;
  summary: string;
  importance: string;
  weight: number;
}

async function recallJson(opts: Options, h: IcmHandle, query: string, limit = 10): Promise<RecalledMemory[]> {
  const args = [...h.baseArgs, "recall", query, "--limit", String(limit), "--format", "json"];
  const r = await runIcm(opts.binary, args);
  if (r.exitCode !== 0) return [];
  try {
    const parsed = JSON.parse(r.stdout) as RecalledMemory[];
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
}

// ─────────────────────────────────────────────────────────────────────────────
// Suite 1: Recall quality
// ─────────────────────────────────────────────────────────────────────────────

interface RecallCase {
  category: "exact" | "keyword" | "phrase" | "negative" | "disambiguation";
  query: string;
  // tag is a unique substring embedded in the content of the target memory.
  // We use it to identify the right memory in the result list.
  expectedTag: string | null; // null = should return no relevant results
}

interface RecallSeed {
  topic: string;
  content: string;
  tag: string; // unique substring inside content
  importance?: "critical" | "high" | "medium" | "low";
}

function recallGoldSet(): { seeds: RecallSeed[]; cases: RecallCase[] } {
  const seeds: RecallSeed[] = [
    { topic: "decisions-engine", content: "TGT_001 Use thiserror for typed errors and anyhow at the binary boundary", importance: "high" },
    { topic: "decisions-engine", content: "TGT_002 Switch the parser from nom to chumsky for better error messages", importance: "high" },
    { topic: "errors-resolved",  content: "TGT_003 Tokio task panic was caused by Send bound on Rc; switched to Arc", importance: "medium" },
    { topic: "errors-resolved",  content: "TGT_004 Hydration mismatch from Date.now in render fixed by computing in load", importance: "medium" },
    { topic: "preferences",      content: "TGT_005 Always run cargo clippy with -D warnings before committing", importance: "critical" },
    { topic: "preferences",      content: "TGT_006 No unwrap in production code; use ? at I/O boundaries", importance: "critical" },
    { topic: "decisions-data",   content: "TGT_007 Adding a NOT NULL column requires three-step deploy: nullable, backfill, constraint", importance: "high" },
    { topic: "decisions-data",   content: "TGT_008 Use sqlite-vec for embedding similarity, not a separate vector store", importance: "high" },
    { topic: "decisions-auth",   content: "TGT_009 Move from JWT in localStorage to httpOnly secure cookies", importance: "critical" },
    { topic: "decisions-auth",   content: "TGT_010 Refresh token rotation on every use, sliding window 30 days", importance: "high" },
    { topic: "incidents",        content: "TGT_011 DB pool exhausted at 18:32 UTC; raised pgbouncer pool_size from 20 to 50", importance: "high" },
    { topic: "decisions-perf",   content: "TGT_012 Switch hot path from JSON to MessagePack saves 40% bandwidth", importance: "high" },
    { topic: "decisions-ci",     content: "TGT_013 Cache cargo target with sccache to cut CI from 18min to 6min", importance: "high" },
    { topic: "decisions-docs",   content: "TGT_014 Documentation in English only; translations via i18n locale files", importance: "critical" },
    { topic: "decisions-review", content: "TGT_015 Run /ultrareview before approving any migration PR", importance: "high" },
  ];
  const seedsByTag = new Map(seeds.map(s => [s.content.split(" ")[0]!, s]));
  void seedsByTag;
  const cases: RecallCase[] = [
    // exact substring of the content
    { category: "exact",          query: "thiserror typed errors anyhow",                   expectedTag: "TGT_001" },
    { category: "exact",          query: "Tokio task panic Send bound Rc Arc",              expectedTag: "TGT_003" },
    { category: "exact",          query: "JWT localStorage httpOnly secure cookies",        expectedTag: "TGT_009" },
    { category: "exact",          query: "MessagePack 40% bandwidth",                       expectedTag: "TGT_012" },
    // single keyword
    { category: "keyword",        query: "clippy",                                          expectedTag: "TGT_005" },
    { category: "keyword",        query: "chumsky",                                         expectedTag: "TGT_002" },
    { category: "keyword",        query: "pgbouncer",                                       expectedTag: "TGT_011" },
    { category: "keyword",        query: "sqlite-vec",                                      expectedTag: "TGT_008" },
    { category: "keyword",        query: "ultrareview",                                     expectedTag: "TGT_015" },
    { category: "keyword",        query: "sccache",                                         expectedTag: "TGT_013" },
    // phrase / multi-keyword
    { category: "phrase",         query: "three-step deploy nullable backfill",             expectedTag: "TGT_007" },
    { category: "phrase",         query: "refresh token rotation sliding window",           expectedTag: "TGT_010" },
    { category: "phrase",         query: "i18n locale files English documentation",         expectedTag: "TGT_014" },
    // disambiguation: 2 similar-topic memories, must rank the right one first
    { category: "disambiguation", query: "hydration date render load",                      expectedTag: "TGT_004" },
    { category: "disambiguation", query: "no unwrap production I/O boundaries",             expectedTag: "TGT_006" },
    // negative: should return either nothing or no hit on any TGT_*
    { category: "negative",       query: "kubernetes helm chart deployment",                expectedTag: null },
    { category: "negative",       query: "fortran compiler optimization passes",            expectedTag: null },
  ];
  return { seeds, cases };
}

interface RecallScore {
  category: RecallCase["category"];
  query: string;
  expectedTag: string | null;
  rank: number; // 1-indexed; 0 if not found
  hitTopK: { 1: boolean; 5: boolean; 10: boolean };
  reciprocalRank: number;
}

interface RecallSummary {
  total: number;
  byCategory: Record<string, { count: number; recall1: number; recall5: number; mrr: number }>;
  overall: { recall1: number; recall5: number; recall10: number; mrr: number; negativeCorrect: number; negativeTotal: number };
  scoreOutOf100: number;
  cases: RecallScore[];
}

async function runRecallSuite(opts: Options): Promise<RecallSummary> {
  const h = newHandle(opts, "recall");
  const { seeds, cases } = recallGoldSet();
  for (const s of seeds) {
    await store(opts, h, s);
  }

  const cases2: RecallScore[] = [];
  for (const c of cases) {
    const results = await recallJson(opts, h, c.query, 10);
    let rank = 0;
    if (c.expectedTag !== null) {
      for (let i = 0; i < results.length; i++) {
        if (results[i]!.summary.includes(c.expectedTag)) {
          rank = i + 1;
          break;
        }
      }
    } else {
      // negative: rank=0 means no relevant result, which is correct
      const surfacedAnyTarget = results.some(r => /TGT_\d+/.test(r.summary));
      rank = surfacedAnyTarget ? 1 : 0; // we'll interpret rank=0 as success below
    }
    cases2.push({
      category: c.category,
      query: c.query,
      expectedTag: c.expectedTag,
      rank,
      hitTopK: { 1: rank === 1, 5: rank > 0 && rank <= 5, 10: rank > 0 && rank <= 10 },
      reciprocalRank: rank > 0 ? 1 / rank : 0,
    });
    if (opts.verbose) {
      console.error(`[recall] ${c.category.padEnd(15)} rank=${rank} q="${c.query.slice(0, 40)}"`);
    }
  }

  // Aggregate
  const positive = cases2.filter(c => c.expectedTag !== null);
  const negative = cases2.filter(c => c.expectedTag === null);

  const byCategory: RecallSummary["byCategory"] = {};
  for (const c of positive) {
    if (!byCategory[c.category]) byCategory[c.category] = { count: 0, recall1: 0, recall5: 0, mrr: 0 };
    const b = byCategory[c.category]!;
    b.count++;
    if (c.hitTopK[1]) b.recall1++;
    if (c.hitTopK[5]) b.recall5++;
    b.mrr += c.reciprocalRank;
  }
  for (const k in byCategory) {
    byCategory[k]!.recall1 = byCategory[k]!.recall1 / byCategory[k]!.count;
    byCategory[k]!.recall5 = byCategory[k]!.recall5 / byCategory[k]!.count;
    byCategory[k]!.mrr = byCategory[k]!.mrr / byCategory[k]!.count;
  }

  const recall1 = positive.filter(c => c.hitTopK[1]).length / positive.length;
  const recall5 = positive.filter(c => c.hitTopK[5]).length / positive.length;
  const recall10 = positive.filter(c => c.hitTopK[10]).length / positive.length;
  const mrr = positive.reduce((s, c) => s + c.reciprocalRank, 0) / positive.length;
  const negCorrect = negative.filter(c => c.rank === 0).length;

  // Quality score: weighted blend
  //   50% MRR + 30% recall@5 + 20% negative-correctness
  const scoreOutOf100 = Math.round(
    (mrr * 0.5 + recall5 * 0.3 + (negCorrect / Math.max(1, negative.length)) * 0.2) * 100,
  );

  return {
    total: cases2.length,
    byCategory,
    overall: { recall1, recall5, recall10, mrr, negativeCorrect: negCorrect, negativeTotal: negative.length },
    scoreOutOf100,
    cases: cases2,
  };
}

// ─────────────────────────────────────────────────────────────────────────────
// Suite 2: Storage fidelity
// ─────────────────────────────────────────────────────────────────────────────

interface StorageCase {
  name: string;
  topic: string;
  content: string;
  importance?: "critical" | "high" | "medium" | "low";
}

interface StorageScore {
  name: string;
  storedOk: boolean;
  recalledOk: boolean;
  contentMatches: boolean;
  notes: string;
}

interface StorageSummary {
  cases: StorageScore[];
  passed: number;
  scoreOutOf100: number;
}

async function runStorageSuite(opts: Options): Promise<StorageSummary> {
  const h = newHandle(opts, "storage");
  const cases: StorageCase[] = [
    { name: "ascii-short",    topic: "fidelity-ascii",   content: "FID_001 Plain ASCII content with simple words" },
    { name: "with-quotes",    topic: "fidelity-quotes",  content: `FID_002 He said "hello" and 'goodbye' in one line` },
    { name: "unicode-emoji",  topic: "fidelity-unicode", content: "FID_003 Café résumé naïve garçon — 日本語 — 🚀💾🔥" },
    { name: "code-snippet",   topic: "fidelity-code",    content: "FID_004 fn main() { let x = vec![1,2,3]; for i in &x { println!(\"{}\", i); } }" },
    { name: "newlines",       topic: "fidelity-nl",      content: "FID_005 line one\nline two\nline three" },
    { name: "long-1000",      topic: "fidelity-long",    content: "FID_006 " + "lorem ipsum dolor sit amet ".repeat(40) },
    { name: "json-blob",      topic: "fidelity-json",    content: `FID_007 {"key":"value","nested":{"a":1,"b":[1,2,3]},"flag":true}` },
    { name: "shell-special",  topic: "fidelity-shell",   content: "FID_008 echo $HOME && ls -la | grep .rs > out.txt" },
    { name: "url-with-query", topic: "fidelity-url",     content: "FID_009 https://example.com/path?a=1&b=2#frag" },
    { name: "markdown",       topic: "fidelity-md",      content: "FID_010 **bold** _italic_ `code` [link](http://x)" },
  ];

  const scores: StorageScore[] = [];
  for (const c of cases) {
    const r = await store(opts, h, c);
    const storedOk = r.exitCode === 0;
    let recalledOk = false;
    let contentMatches = false;
    let notes = "";
    if (storedOk) {
      const tag = c.content.split(" ")[0]!; // FID_NNN
      const hits = await recallJson(opts, h, tag, 5);
      const found = hits.find(h => h.summary.includes(tag));
      recalledOk = !!found;
      if (found) {
        contentMatches = found.summary === c.content;
        if (!contentMatches) {
          notes = `stored:${JSON.stringify(c.content.slice(0, 60))} recalled:${JSON.stringify(found.summary.slice(0, 60))}`;
        }
      } else {
        notes = `recall by tag '${tag}' returned ${hits.length} hits, none matched`;
      }
    } else {
      notes = `store failed: ${r.stderr.slice(0, 120)}`;
    }
    scores.push({ name: c.name, storedOk, recalledOk, contentMatches, notes });
    if (opts.verbose) {
      console.error(`[storage] ${c.name.padEnd(18)} stored=${storedOk} recalled=${recalledOk} match=${contentMatches}`);
    }
  }

  const passed = scores.filter(s => s.storedOk && s.recalledOk && s.contentMatches).length;
  const scoreOutOf100 = Math.round((passed / scores.length) * 100);
  return { cases: scores, passed, scoreOutOf100 };
}

// ─────────────────────────────────────────────────────────────────────────────
// Suite 3: Consolidation quality
// ─────────────────────────────────────────────────────────────────────────────

interface ConsolidationSummary {
  inputCount: number;
  consolidationCreated: boolean;
  newMemoryId: string | null;
  consolidatedSummary: string | null;
  conceptsCovered: number;
  conceptsTotal: number;
  compressionRatio: number;
  scoreOutOf100: number;
  notes: string;
}

async function runConsolidationSuite(opts: Options): Promise<ConsolidationSummary> {
  const h = newHandle(opts, "consolidation");
  const topic = "decisions-consolidation-test";
  // 8 distinct decisions, each with a defining keyword we grep for in the
  // consolidated output. Synthetic prefix tags would be valid scoring proxies
  // for a purely lexical consolidate, but a smart LLM may legitimately drop
  // them as noise — so we score on concept words that any honest consolidation
  // (lexical or LLM) MUST preserve.
  const inputs: MemorySpec[] = [
    { topic, content: "Use thiserror for typed library errors", importance: "high" },
    { topic, content: "Replace nom parser with chumsky for richer diagnostics", importance: "high" },
    { topic, content: "Tokio runtime configured with 4 worker threads", importance: "medium" },
    { topic, content: "Persist via SQLite with WAL mode enabled", importance: "high" },
    { topic, content: "Vector index uses sqlite-vec, not Qdrant", importance: "high" },
    { topic, content: "Deploy as a single static musl binary", importance: "medium" },
    { topic, content: "Logs go to stderr in JSON when ICM_LOG_JSON=1", importance: "medium" },
    { topic, content: "Cache layer is an LRU bounded to 256 entries", importance: "high" },
  ];
  // Defining keyword per input — must appear in consolidated output for
  // concept coverage credit.
  const conceptKeywords = ["thiserror", "chumsky", "tokio", "wal", "sqlite-vec", "musl", "icm_log_json", "lru"];
  for (const m of inputs) await store(opts, h, m);

  const r = await runIcm(opts.binary, [...h.baseArgs, "consolidate", "--topic", topic, "--keep-originals"]);
  const consolidationCreated = r.exitCode === 0;
  let newMemoryId: string | null = null;
  let consolidatedSummary: string | null = null;
  let conceptsCovered = 0;
  let notes = "";

  // Parse output: "Consolidated N memories from 'X' into <ID>"
  const m = r.stdout.match(/into\s+([A-Z0-9]{20,})/);
  if (m) newMemoryId = m[1]!;

  if (consolidationCreated && newMemoryId) {
    // The consolidation memory should exist; recall it by topic
    const hits = await recallJson(opts, h, "consolidation", 50);
    const consolidated = hits.find(h => h.id === newMemoryId);
    if (consolidated) {
      consolidatedSummary = consolidated.summary;
      conceptsCovered = conceptKeywords.filter(k => consolidated.summary.toLowerCase().includes(k)).length;
    } else {
      // Fall back to recall on the topic directly
      const topicHits = await recallJson(opts, h, topic, 50);
      const consolidated2 = topicHits.find(h => h.id === newMemoryId);
      if (consolidated2) {
        consolidatedSummary = consolidated2.summary;
        const tags = ["alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta"];
        conceptsCovered = tags.filter(t => consolidated2.summary.toLowerCase().includes(t)).length;
      } else {
        notes = "new memory id not surfaced in recall";
      }
    }
  } else if (!consolidationCreated) {
    notes = `consolidate failed: ${r.stderr.slice(0, 120)}`;
  } else {
    notes = "no new id parsed from consolidate stdout";
  }

  const conceptsTotal = 8;
  const inputChars = inputs.reduce((s, m) => s + m.content.length, 0);
  const outputChars = consolidatedSummary ? consolidatedSummary.length : 0;
  const compressionRatio = inputChars > 0 && outputChars > 0 ? outputChars / inputChars : 0;

  // Scoring: 60% concept coverage + 30% successful consolidation + 10% no-bloat
  // "No-bloat" = output length ≤ input length. For already-short inputs (8
  // bullets of ~50 chars each) a real LLM may produce bullets of similar
  // length — that's still a structural improvement over the lexical "|"-joined
  // wall, even though raw char compression is near 100%. Penalise only actual
  // bloat (> 110%).
  const compressionScore = compressionRatio > 0 && compressionRatio <= 1.10 ? 1 : 0;
  const scoreOutOf100 = Math.round(
    ((conceptsCovered / conceptsTotal) * 60 + (consolidationCreated ? 30 : 0) + compressionScore * 10),
  );

  return {
    inputCount: inputs.length,
    consolidationCreated,
    newMemoryId,
    consolidatedSummary,
    conceptsCovered,
    conceptsTotal,
    compressionRatio,
    scoreOutOf100,
    notes,
  };
}

// ─────────────────────────────────────────────────────────────────────────────
// Suite 4: Wake-up quality
// ─────────────────────────────────────────────────────────────────────────────

interface WakeupSummary {
  inserted: { critical: number; high: number; medium: number; low: number };
  surfaced: { critical: number; high: number; medium: number; low: number };
  outputBytes: number;
  scoreOutOf100: number;
  notes: string;
}

async function runWakeupSuite(opts: Options): Promise<WakeupSummary> {
  const h = newHandle(opts, "wakeup");
  // Mix of importance — tags let us scan the wake-up output
  const counts = { critical: 5, high: 8, medium: 12, low: 15 };
  const inserts: MemorySpec[] = [];
  for (let i = 0; i < counts.critical; i++) {
    inserts.push({ topic: "preferences", content: `WK_CRIT_${i.toString().padStart(2, "0")} non-negotiable rule ${i}`, importance: "critical" });
  }
  for (let i = 0; i < counts.high; i++) {
    inserts.push({ topic: "decisions-test", content: `WK_HIGH_${i.toString().padStart(2, "0")} important decision ${i}`, importance: "high" });
  }
  for (let i = 0; i < counts.medium; i++) {
    inserts.push({ topic: "notes", content: `WK_MED_${i.toString().padStart(2, "0")} useful but not critical ${i}`, importance: "medium" });
  }
  for (let i = 0; i < counts.low; i++) {
    inserts.push({ topic: "scratch", content: `WK_LOW_${i.toString().padStart(2, "0")} ephemeral scratch note ${i}`, importance: "low" });
  }
  for (const m of inserts) await store(opts, h, m);

  // High budget so all critical+high should fit; --project - to disable scoping
  const r = await runIcm(opts.binary, [...h.baseArgs, "wake-up", "--project", "-", "--max-tokens", "4000"]);
  const out = r.stdout;
  const surfaced = {
    critical: (out.match(/WK_CRIT_\d{2}/g) ?? []).length,
    high:     (out.match(/WK_HIGH_\d{2}/g) ?? []).length,
    medium:   (out.match(/WK_MED_\d{2}/g) ?? []).length,
    low:      (out.match(/WK_LOW_\d{2}/g) ?? []).length,
  };

  // Scoring:
  //   40% all critical surfaced
  //   30% all high surfaced (target — may be partial under tight budget)
  //   20% no low surfaced
  //   10% medium gets surfaced ≤50% of inserted (some leakage acceptable)
  const critScore = Math.min(1, surfaced.critical / counts.critical);
  const highScore = Math.min(1, surfaced.high / counts.high);
  const lowScore = surfaced.low === 0 ? 1 : Math.max(0, 1 - surfaced.low / counts.low);
  const medRatio = surfaced.medium / Math.max(1, counts.medium);
  const medScore = medRatio <= 0.5 ? 1 : Math.max(0, 1 - (medRatio - 0.5) * 2);

  const scoreOutOf100 = Math.round(critScore * 40 + highScore * 30 + lowScore * 20 + medScore * 10);

  let notes = "";
  if (surfaced.low > 0) notes += `WARN: ${surfaced.low} low-importance memories leaked into wake-up. `;
  if (surfaced.critical < counts.critical) notes += `WARN: ${counts.critical - surfaced.critical} critical not surfaced. `;
  if (r.exitCode !== 0) notes += `wake-up exit ${r.exitCode}. `;

  return {
    inserted: counts,
    surfaced,
    outputBytes: out.length,
    scoreOutOf100,
    notes: notes.trim(),
  };
}

// ─────────────────────────────────────────────────────────────────────────────
// Reporting
// ─────────────────────────────────────────────────────────────────────────────

interface FullReport {
  binary: string;
  version: string;
  withEmbeddings: boolean;
  recall?: RecallSummary;
  storage?: StorageSummary;
  consolidation?: ConsolidationSummary;
  wakeup?: WakeupSummary;
  overallScore: number | null;
}

function renderReport(rep: FullReport): string {
  const lines: string[] = [];
  lines.push("# ICM Quality Benchmark");
  lines.push("");
  lines.push(`Binary       : ${rep.binary} (${rep.version})`);
  lines.push(`Embeddings   : ${rep.withEmbeddings ? "ON" : "OFF (keyword search)"}`);
  if (rep.overallScore !== null) {
    lines.push(`**Overall**  : **${rep.overallScore}/100** (mean of run suites)`);
  }
  lines.push("");

  if (rep.recall) {
    const r = rep.recall;
    lines.push("## 1. Recall quality");
    lines.push("");
    lines.push(`**Score: ${r.scoreOutOf100}/100**`);
    lines.push("");
    lines.push(`- Recall@1   : ${(r.overall.recall1 * 100).toFixed(0)}%`);
    lines.push(`- Recall@5   : ${(r.overall.recall5 * 100).toFixed(0)}%`);
    lines.push(`- Recall@10  : ${(r.overall.recall10 * 100).toFixed(0)}%`);
    lines.push(`- MRR        : ${r.overall.mrr.toFixed(3)}`);
    lines.push(`- Negative correctness: ${r.overall.negativeCorrect}/${r.overall.negativeTotal}`);
    lines.push("");
    lines.push("| category | n | R@1 | R@5 | MRR |");
    lines.push("|---|---|---|---|---|");
    for (const [cat, b] of Object.entries(r.byCategory)) {
      lines.push(`| ${cat} | ${b.count} | ${(b.recall1 * 100).toFixed(0)}% | ${(b.recall5 * 100).toFixed(0)}% | ${b.mrr.toFixed(3)} |`);
    }
    lines.push("");
    const misses = r.cases.filter(c => c.expectedTag !== null && !c.hitTopK[5]);
    if (misses.length > 0) {
      lines.push("Misses (rank > 5 or not found):");
      for (const m of misses) {
        lines.push(`- [${m.category}] q="${m.query}" expected=${m.expectedTag} rank=${m.rank}`);
      }
      lines.push("");
    }
  }

  if (rep.storage) {
    const s = rep.storage;
    lines.push("## 2. Storage fidelity");
    lines.push("");
    lines.push(`**Score: ${s.scoreOutOf100}/100** (${s.passed}/${s.cases.length} pass)`);
    lines.push("");
    lines.push("| case | stored | recalled | exact match | notes |");
    lines.push("|---|---|---|---|---|");
    for (const c of s.cases) {
      lines.push(`| ${c.name} | ${c.storedOk ? "ok" : "FAIL"} | ${c.recalledOk ? "ok" : "FAIL"} | ${c.contentMatches ? "ok" : "FAIL"} | ${c.notes} |`);
    }
    lines.push("");
  }

  if (rep.consolidation) {
    const c = rep.consolidation;
    lines.push("## 3. Consolidation quality");
    lines.push("");
    lines.push(`**Score: ${c.scoreOutOf100}/100**`);
    lines.push("");
    lines.push(`- Inputs                : ${c.inputCount}`);
    lines.push(`- Consolidate succeeded : ${c.consolidationCreated}`);
    lines.push(`- New memory id         : ${c.newMemoryId ?? "(none)"}`);
    lines.push(`- Concepts covered      : ${c.conceptsCovered}/${c.conceptsTotal}`);
    lines.push(`- Compression ratio     : ${(c.compressionRatio * 100).toFixed(1)}% (output / input chars)`);
    if (c.consolidatedSummary) {
      lines.push("");
      lines.push("Output summary preview:");
      lines.push("```");
      lines.push(c.consolidatedSummary.slice(0, 600));
      lines.push("```");
    }
    if (c.notes) {
      lines.push("");
      lines.push(`Notes: ${c.notes}`);
    }
    lines.push("");
  }

  if (rep.wakeup) {
    const w = rep.wakeup;
    lines.push("## 4. Wake-up quality");
    lines.push("");
    lines.push(`**Score: ${w.scoreOutOf100}/100**`);
    lines.push("");
    lines.push("| importance | inserted | surfaced |");
    lines.push("|---|---|---|");
    lines.push(`| critical | ${w.inserted.critical} | ${w.surfaced.critical} |`);
    lines.push(`| high     | ${w.inserted.high} | ${w.surfaced.high} |`);
    lines.push(`| medium   | ${w.inserted.medium} | ${w.surfaced.medium} |`);
    lines.push(`| low      | ${w.inserted.low} | ${w.surfaced.low} |`);
    lines.push("");
    lines.push(`Output bytes: ${w.outputBytes}`);
    if (w.notes) {
      lines.push("");
      lines.push(`Notes: ${w.notes}`);
    }
    lines.push("");
  }

  return lines.join("\n");
}

// ─────────────────────────────────────────────────────────────────────────────
// Entry
// ─────────────────────────────────────────────────────────────────────────────

async function main() {
  const opts = parseArgs(process.argv);
  await mkdir(opts.outDir, { recursive: true });

  const probe = await runIcm(opts.binary, ["--version"]);
  if (probe.exitCode !== 0) {
    console.error(`[fatal] ${opts.binary} --version failed:\n${probe.stderr}`);
    process.exit(1);
  }
  const version = probe.stdout.trim();

  console.log(`Binary    : ${opts.binary} (${version})`);
  console.log(`Out dir   : ${opts.outDir}`);
  console.log(`Embeddings: ${opts.withEmbeddings ? "ON" : "OFF"}`);
  console.log(`Suites    : ${opts.suites.join(", ")}`);
  console.log("");

  const rep: FullReport = {
    binary: opts.binary,
    version,
    withEmbeddings: opts.withEmbeddings,
    overallScore: null,
  };

  if (opts.suites.includes("recall")) {
    process.stdout.write("Suite 1/4: Recall quality… ");
    rep.recall = await runRecallSuite(opts);
    console.log(`${rep.recall.scoreOutOf100}/100`);
  }
  if (opts.suites.includes("storage")) {
    process.stdout.write("Suite 2/4: Storage fidelity… ");
    rep.storage = await runStorageSuite(opts);
    console.log(`${rep.storage.scoreOutOf100}/100`);
  }
  if (opts.suites.includes("consolidation")) {
    process.stdout.write("Suite 3/4: Consolidation… ");
    rep.consolidation = await runConsolidationSuite(opts);
    console.log(`${rep.consolidation.scoreOutOf100}/100`);
  }
  if (opts.suites.includes("wakeup")) {
    process.stdout.write("Suite 4/4: Wake-up quality… ");
    rep.wakeup = await runWakeupSuite(opts);
    console.log(`${rep.wakeup.scoreOutOf100}/100`);
  }

  const scores: number[] = [];
  if (rep.recall) scores.push(rep.recall.scoreOutOf100);
  if (rep.storage) scores.push(rep.storage.scoreOutOf100);
  if (rep.consolidation) scores.push(rep.consolidation.scoreOutOf100);
  if (rep.wakeup) scores.push(rep.wakeup.scoreOutOf100);
  rep.overallScore = scores.length > 0 ? Math.round(scores.reduce((a, b) => a + b, 0) / scores.length) : null;

  const md = renderReport(rep);
  const reportPath = join(opts.outDir, "report.md");
  await writeFile(reportPath, md);
  await writeFile(join(opts.outDir, "report.json"), JSON.stringify(rep, null, 2));

  console.log("");
  console.log(md);
  console.log("");
  console.log(`Report: ${reportPath}`);
}

main().catch((err) => {
  console.error("[fatal]", err);
  process.exit(1);
});
