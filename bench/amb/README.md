# ICM on the Agent Memory Benchmark (AMB)

Runs ICM through the same harness as the public Hindsight leaderboard
(https://benchmarks.hindsight.vectorize.io, harness:
https://github.com/vectorize-io/agent-memory-benchmark), so ICM numbers can sit in
the same table.

The harness is not vendored here (upstream declares no license). `run_amb.py`
imports it from a checkout, registers the `icm` provider, and delegates to the
upstream CLI. Every upstream command and flag works unchanged.

## Files

- `icm_provider.py` - the `MemoryProvider` implementation
- `full_context_provider.py` - `--memory full-context`: no retrieval, the whole unit to the answer model
- `run_amb.py` - launcher (registers `icm` and `full-context`, optional trace / sharding / Vertex auth)
- `recall_at_k.py` - gold-document recall from a retrieve trace (no LLM)
- `recall_only.py`, `merge_recall.py` - recall-only run (no model at all) and its merge + scoring
- `dry_run_tokens.py` - what a run would send to the answer model, measured without calling it
- `rejudge_strict.py` - second, strict judge on a stratified sample of an answer run
- `estimate_cost.py` - tokens and dollars of an answer run, from its result and usage files
- `compare_builds.py` - replay one slice for two ICM builds, before/after table
- `merge_shards.py` - merge the result files of a sharded run, after checking they are complete
- `fetch_dataset.py` - LongMemEval-S download with retries and resume (image build, pod start)
- `tests/` - offline tests of the scripts, of the Job template and of the commands of this
  page; `fake_icm.py` stands in for the binary, no model and no cluster are called
- `requirements.txt` - light Python dependencies (enough for `icm` and `bm25`);
  `tests/requirements.txt` - what the tests add
- `Dockerfile`, `Dockerfile.tooling`, `entrypoint.sh`, `gcs_sync.py`, `cloudbuild*.yaml`, `k8s/` - GKE run
  (`k8s/render.sh` renders the Job template)

## Local setup

```bash
# 1. Harness checkout at the pinned commit. Sparse and without blobs: the repository is
#    ~740 MB with its data and outputs, this fetches the 3 MB of `src` at that commit.
git clone --filter=blob:none --no-checkout --sparse \
  https://github.com/vectorize-io/agent-memory-benchmark.git /path/to/amb
git -C /path/to/amb sparse-checkout set src
git -C /path/to/amb checkout f618ed7b1f0eb9cad7b42e876f91a42f0eadb150   # the commit of the baseline

# 2. Python environment (the second file is only needed to run the tests)
uv venv .venv --python 3.12
VIRTUAL_ENV=.venv uv pip install -r bench/amb/requirements.txt -r bench/amb/tests/requirements.txt

# 3. The ICM binary to measure
cargo build --release -p icm-cli
```

Tests. They need the checkout above (`AMB_HOME`), the two requirements files and
`envsubst` (gettext) on `PATH`; a missing module or a missing `envsubst` is a
failure, not a skip. Two parts skip on purpose when their data is absent: the
LoCoMo part of `test_full_context.py` reads `AMB_HOME/.datasets/locomo/locomo10.json`
(the harness downloads it on the first LoCoMo run; the test does not), and one check
of `test_rejudge_strict.py` wants the full merged result file in `MERGED_LOCOMO_RESULT`.

```bash
AMB_HOME=/path/to/amb .venv/bin/python -m unittest discover -s bench/amb/tests -v
```

## Running

The published leaderboard runs all use `gemini-3.1-pro-preview` for answers and
`gemini-2.5-flash-lite` as judge, in `rag` mode. The commands below use the same
models and the same mode. Two things differ from the published LoCoMo and
PersonaMem runs, and belong in any publication of an ICM figure next to them:

- Temperature. The harness sets `temperature=0.0` for answers and judging since
  its commit aa9273ab (2026-06-23), which the pinned commit f618ed7b includes.
  The published LoCoMo and PersonaMem results date from 2026-03-23, when the
  harness set no temperature: they ran at the API default.
- Endpoint. The GKE Job calls Gemini through Vertex AI (location `global`), not
  through the API-key endpoint the upstream CLI asks for. Which endpoint the
  published runs used is not recorded in their result files.

```bash
export AMB_HOME=/path/to/amb
export ICM_AMB_BIN=$PWD/target/release/icm      # binary under test
export OMB_ANSWER_LLM=gemini OMB_ANSWER_MODEL=gemini-3.1-pro-preview
export OMB_JUDGE_LLM=gemini  OMB_JUDGE_MODEL=gemini-2.5-flash-lite

# Gemini access, one of:
export GEMINI_API_KEY=...                                    # a) API key
export GOOGLE_GENAI_USE_VERTEXAI=true GOOGLE_CLOUD_PROJECT=rtk-ai-labs-01 \
       GOOGLE_CLOUD_LOCATION=global                          # b) Vertex AI + ADC
export ICM_AMB_GCLOUD_AUTH=1 GOOGLE_CLOUD_PROJECT=rtk-ai-labs-01 \
       GOOGLE_CLOUD_LOCATION=global                          # c) Vertex AI + active gcloud account

# Small slice, ICM then the harness' own bm25 baseline on the same queries.
# ICM_AMB_ENGINE is required with --memory icm: see "Recall engine: always named".
ICM_AMB_ENGINE=v2 ICM_AMB_TRACE=out/trace-locomo-icm.jsonl .venv/bin/python bench/amb/run_amb.py run \
  --dataset locomo --split locomo10 --memory icm --query-limit 20 --output-dir out
ICM_AMB_TRACE=out/trace-locomo-bm25.jsonl .venv/bin/python bench/amb/run_amb.py run \
  --dataset locomo --split locomo10 --memory bm25 --query-limit 20 --output-dir out

# Retrieval-only metric from the traces (no LLM)
.venv/bin/python bench/amb/recall_at_k.py --dataset locomo --split locomo10 \
  --trace out/trace-locomo-icm.jsonl
```

Results land in `out/<dataset>/<run name>/rag/<split>.json` (accuracy, ingestion
time, average retrieve time, average context tokens, and every answer). ICM's
databases and `ingest-stats.json` are under
`out/<dataset>/<run name>/_store/<split>/all/icm/`.

Comparing two ICM builds: change `ICM_AMB_BIN`, name the engine of each run and
give each run its own `--name` (for example `--name icm-0.10.65` and `--name icm-next`);
`compare_builds.py` does it for you.

Full splits used by the leaderboard: `longmemeval/s`, `locomo/locomo10`,
`personamem/32k`, `beam/100k|500k|1m|10m`, `lifebench/en`.

## Provider design

- Driving ICM: one long-lived `icm serve --http 127.0.0.1:<port>` per isolation
  unit. A cold `icm recall` reloads the embedding model in every process; the
  warm server answers a recall in tens of milliseconds.
- Isolation: one SQLite database per harness `user_id`, inside the storage
  directory the harness provides. Not a shared database scoped by topic: `/recall`
  applies the topic filter after the candidate pool is cut, so other units'
  memories would displace the unit's own. `--db` is always passed, `ICM_CONFIG`
  points at an empty file (ICM defaults), and `ICM_DB` is removed from the server
  environment, so the user's real database and config are never read.
- Timestamps: the document timestamp and the harness' provenance line are written
  as a one-line header of each memory, as a user storing a dated note would. With
  `ICM_AMB_ENGINE=v2` the document date is also sent as `created_at` on `/store` and
  the question date as `now` on `/recall` (each can be withheld); the other engine
  values send no date.
- Chunking: the harness' `chunk_text`, 512 cl100k tokens without overlap, the
  same unit as the upstream `bm25` and `hybrid-search` baselines.
- Recall: `POST /recall` with `limit=k` (default 50, the budget of the upstream
  `hybrid-search` baseline), hybrid FTS5 + vector with ICM's default embedding
  model, on the engine the run names. No LLM at ingest, no reranker, no
  dataset-specific code path.
- The provider returns `raw_response=None`: LongMemEval and LoCoMo otherwise send
  `json.dumps(raw_response)` to the answer model instead of the rendered memories.

Knobs are environment variables, documented at the top of `icm_provider.py`.

## Memory or reader model? Full context, BM25, and ICM at 5, 10, 20

Five runs of the same split with the same answer model, the same judge and the same
prompt separate what the memory brings from what the answer model does alone:

- `--memory full-context`: every document of the question's unit (LoCoMo: all the
  sessions of the conversation), whole, oldest first, with the date line ICM writes
  in its memories. It is handed the same `Document` list as every other provider
  and never sees a question's gold answer or gold ids (`tests/test_full_context.py`
  rebuilds each context from the raw dataset file and compares).
- `--memory bm25`: the harness' own baseline, unchanged: 10 chunks of 512 tokens,
  and no date in what the answer model reads. That last point is a known handicap
  on time questions, not a retrieval difference.
- `--memory icm` with `ICM_AMB_K=5`, `10`, `20` (and the engine of the published run).

Sizes on `locomo/locomo10` (1,540 questions), measured with `dry_run_tokens.py`
(real harness, counting stand-ins instead of the models, tiktoken cl100k):

| run | context tokens per question | answer-model input, whole run |
|---|---|---|
| full-context | 39,971 (21,700 to 52,290) | 61.9 M |
| bm25 (k=10) | 4,953 | 7.9 M |
| icm k=5 | about 2,500 | about 4.1 M |
| icm k=10 | about 5,000 | about 8.0 M |
| icm k=20 | about 9,900 | about 15.5 M |
| icm k=50 (published run) | 24,064 | 37.4 M |

The three ICM lines below k=50 are read from the k=50 run (first 5, 10, 20 memories
of each recorded context), so they assume the ranking does not change with the cut.
The judge reads about 450 tokens per question (0.7 M per run). cl100k is the
harness' unit: the provider bills its own token count. Measure a provider before
paying for it:

```bash
.venv/bin/python bench/amb/dry_run_tokens.py --dataset locomo --split locomo10 --memory full-context
```

## What a run costs, before launching it

Three quantities make the bill, and a result file holds only the first:

1. the input of the answer model: the context of each question plus about 200 tokens
   of instructions. `dry_run_tokens.py` measures it for a run that has not happened
   (`answer model input`), `context_tokens` holds it for a finished one. Both are
   tiktoken cl100k counts, not billed tokens;
2. what the answer model writes. The harness asks for a JSON object with `reasoning`
   and `answer`, and the model also spends thinking tokens, billed at the output
   price and returned nowhere in the answer. The harness records neither count;
3. the judge: one short prompt per question (about 450 tokens on LoCoMo), a one-line
   reason back.

So measure before estimating. Every answer run started by `entrypoint.sh`, or with
`ICM_AMB_USAGE=<file>` set, writes one line per model response with the token counts
the API itself reports (`usage_metadata`: prompt, output, thinking, cached). A slice
of twenty questions is enough to know the two ratios an estimate needs: billed input
tokens per cl100k token, and thinking tokens per answer.

```bash
# 1. a paid slice of 20 questions, with the API's counts kept
ICM_AMB_ENGINE=v2 ICM_AMB_USAGE=out/usage-slice.jsonl .venv/bin/python bench/amb/run_amb.py run \
  --dataset locomo --split locomo10 --memory icm --name slice --query-limit 20 --output-dir out
# 2. its measured cost, and the two ratios, at the prices of the day
.venv/bin/python bench/amb/estimate_cost.py --results out/locomo/slice/rag/locomo10.json \
  --usage out/usage-slice.jsonl --answer-price <in> <out> --judge-price <in> <out>
# 3. the run you plan: its input from dry_run_tokens.py, the ratios from step 2
.venv/bin/python bench/amb/estimate_cost.py --results out/locomo/slice/rag/locomo10.json \
  --answer-input-tokens <answer model input of the dry run> --questions 1540 \
  --max-call-input <largest prompt of the dry run> \
  --answer-price <in> <out> --judge-price <in> <out> --long-context <threshold> <in> <out> \
  --token-ratio <low> <high> --thinking-per-answer <low> <high>
```

Prices are arguments, in US dollars per million tokens: the script holds none. Read
them the day you launch on the provider's list (Vertex AI:
https://cloud.google.com/vertex-ai/generative-ai/pricing) and check three things
there: that the output price covers reasoning (thinking) tokens, where the
long-context tier starts (a call over the threshold is charged at the higher price
for its input and its output; `estimate_cost.py --long-context` applies it), and
whether the endpoint you call (`global` here) has its own price.

Without a usage file `estimate_cost.py` still prints the floor (the tokens that are
in the result file, at one billed token per cl100k token, visible output only) and
gives a range only when both assumptions are stated on the command line. It never
mixes the two: each line says `measured` or `assumed`. Not in any estimate: the
calls the harness repeats when it cannot parse an answer, and a pod restart, which
pays again for the answers of the unit in progress (see "GKE", restarts).

## Recall only (no model): LongMemEval-S, session recall

`recall_only.py` ingests, queries and writes down the ranked session ids; nothing in
it can call a model (the harness' LLM entry points are replaced by functions that
raise). `merge_recall.py` checks the run is complete and scores it.

```bash
# BM25 over the same units: seconds, no binary needed
.venv/bin/python bench/amb/recall_only.py run --dataset longmemeval --split s --memory bm25 \
  --name bm25-user --unit session-user --output-dir out
# ICM, the figure to put next to the published ones: engine v2, no date handed to it
# (about 23,800 memories to embed: hours, see "GKE")
ICM_AMB_ENGINE=v2 ICM_AMB_STORE_DATE=0 ICM_AMB_QUERY_NOW=0 .venv/bin/python bench/amb/recall_only.py run \
  --dataset longmemeval --split s --memory icm --name icm-v2-nodate-user --unit session-user --output-dir out
.venv/bin/python bench/amb/merge_recall.py out/longmemeval/icm-v2-nodate-user/recall/s.json \
  --expect-queries 500 --expect-docs 23867 -o out/merged/longmemeval/icm-v2-nodate-user/recall/s.json
# Second run, to publish under its own label: v2 with the session dates and the question date
ICM_AMB_ENGINE=v2 .venv/bin/python bench/amb/recall_only.py run --dataset longmemeval --split s \
  --memory icm --name icm-v2-dated-user --unit session-user --output-dir out
```

Which of the two is comparable. MemPalace's script and agentmemory's send the text
of the question and nothing else: no session date at indexing, no question date at
query time. ICM's v2 engine can use both, and the provider sends them by default.
That is a different protocol: a figure obtained with the dates does not sit in the
same column as 96.6 % and 95.2 %. The run without dates
(`ICM_AMB_STORE_DATE=0 ICM_AMB_QUERY_NOW=0`) is the comparable one; the dated run is
ICM using what the dataset offers, to publish next to it and name as such. Each
result file says which it is (`provider.store_date`, `provider.query_now`, and the
description ends with `engine v2, no date sent` or `engine v2, document date sent
(created_at), question date sent (now)`).

The protocol is the one behind the two published figures, read in their code and
checked against their committed result files (our scorer on their rankings gives
their numbers: MemPalace raw 483/500 = 96.6 % at 5, agentmemory 95.2 % hybrid and
86.2 % BM25):

- question set: the 500 questions of `longmemeval_s_cleaned.json`, abstention
  questions included (both do; agentmemory's filter on `question_type` removes none
  in this file). `merge_recall.py` also gives the 470 others on their own.
- one fresh index per question, holding only that question's haystack.
- expected sessions: the dataset's `answer_session_ids`. The harness' own gold
  (sessions with a `has_answer` turn) differs on 62 questions and is empty for 21.
- `recall_any@k`: at least one expected session among the first k results;
  `recall_all@k`: all of them (the strict one; neither vendor headlines it).
- indexed unit, `--unit`: `session-user` is MemPalace's (the user turns of a
  session joined by newlines; the 71 sessions without a user turn, none of them
  expected, are not indexed), `session-all` is agentmemory's (`role: content` lines).
  `chunk` is the unit of the answer runs, for reference.

What still differs, to state next to an ICM figure: ICM is the product driven
through `icm serve` (MemPalace's script calls ChromaDB directly, none of its own
code); its embedding model reads the first 512 tokens of a unit and FTS the whole
of it (MemPalace's default embedder reads 256 word pieces, agentmemory embeds the
first 512 characters); in the dated run only, ICM is also given each session's
date and the question's date (day precision: the harness' LongMemEval loader drops
the time of day); one session of 23,867 is over ICM's 64 KiB limit per memory and
is stored as two.

Two guards of the result file. `llm_calls` counts the attempts to reach a model
during the run (each one raises; `merge_recall.py` refuses a file where the count is
not 0). A returned memory whose id is not a document of the question's unit stops
the run instead of being scored as a miss.

BM25 on the same units is the floor to quote with any of these figures. Measured
here with `--memory bm25` (rank_bm25, tokenizer `words`), 500 questions:

| unit | recall_any@5 | recall_all@5 | recall_any@10 | recall_all@10 |
|---|---|---|---|---|
| session-user | 94.6 % | 81.2 % | 96.4 % | 88.2 % |
| session-all | 96.2 % | 83.0 % | 98.0 % | 89.6 % |
| chunk (first 5 or 10 chunks) | 95.4 % | 75.4 % | 97.4 % | 82.4 % |

With the harness' own tokenizer (`--bm25-tokenizer harness`, punctuation left on
the words) the first line is 90.2 % / 75.6 %.

## Second, strict judge

The harness' LoCoMo judge prompt asks the judge to "be generous" and to accept an
answer that "touches on the same topic". `rejudge_strict.py` draws 200 answers of a
merged run (fixed seed, stratified by question type), has them judged again with
the same prompt minus those instructions (`--show-diff` prints exactly what
changes) by a model of your choice on Vertex AI, and reports the agreement, the
confusion matrix, the accuracy under the strict judge with its interval, and each
disagreement for a human to read. An output that is not a boolean verdict is
never counted (the harness counts it correct): the row is reported unjudged.

```bash
export AMB_HOME=/path/to/amb GOOGLE_CLOUD_PROJECT=rtk-ai-labs-01 GOOGLE_CLOUD_LOCATION=global
.venv/bin/python bench/amb/rejudge_strict.py --dry-run --control --out out/rejudge-k50 \
  --results bench/amb/results/locomo10-icm-v2-k50-gke-20261005.json     # sample + sizes, no call
.venv/bin/python bench/amb/rejudge_strict.py --model <judge model> --control --out out/rejudge-k50 \
  --results bench/amb/results/locomo10-icm-v2-k50-gke-20261005.json     # 400 calls, about 200 k input tokens
```

- `--control` also has the same model judge the sample with the harness' unchanged
  prompt. Unless the strict judge is the run's own judge model, the gap between the
  two judges mixes the model and the prompt; the control arm splits it in a model
  gap and a prompt gap. Without it (200 calls) the report says which of the two
  cases it is.
- One `--out` per result file. Verdicts are written as they come and a second run
  pays only for the missing ones, but a verdict is reused only for the exact prompt
  it answered (the rendered prompt is hashed in each line), and a directory that
  holds the sample of another result file is refused: the five LoCoMo runs share
  their question ids, their answers differ.
- A request the API refuses for good (unknown model, no access: HTTP 400, 401, 403,
  404) stops the run at the first call, exit 2. So do eight rows in a row without a
  verdict. Exit 3: finished, some rows unjudged and left out of the figures.

## Comparing two builds

`compare_builds.py` replays the same questions, with the same answer model and
judge, for two ICM binaries (or two settings of one binary) and prints a table:
accuracy, gold-session recall at k, context tokens as the harness counts them
(tiktoken), memories returned, ingestion time, recall latency. Each side names its
engine (`--before-env ICM_AMB_ENGINE=...`, `--after-env ICM_AMB_ENGINE=...`): a side
without one is refused before anything runs. The table has a "Sent to ICM" line
counted in each side's HTTP trace (requests that carried `created_at`, `now`, and
each `engine` value), so the comparison shows what was on the wire.

```bash
# Gold recall only: no answer model, no judge, no model cost
.venv/bin/python bench/amb/compare_builds.py --no-llm \
  --before /path/to/icm-baseline --before-env ICM_AMB_ENGINE=legacy \
  --after target/release/icm --after-env ICM_AMB_ENGINE=v2 \
  --dataset locomo --split locomo10 --out out

# With answers and judging on a slice (leaderboard models unless OMB_* say otherwise)
.venv/bin/python bench/amb/compare_builds.py \
  --before /path/to/icm-baseline --before-env ICM_AMB_ENGINE=legacy \
  --after target/release/icm --after-env ICM_AMB_ENGINE=v2 --after-env ICM_AMB_MAX_TOKENS=23000 \
  --dataset locomo --split locomo10 --query-limit 20 --out out
```

`ICM_AMB_MAX_TOKENS=23000` is a budget in ICM's unit, not 23,000 context tokens:
read the context size in the `avg_context_tokens` column (see "Token budget").

`--reuse` keeps a side whose result already exists in `--out`, so the baseline is
ingested once. The table is also written to `out/compare-*.md` and `.json`.

## Recall engine: always named

`ICM_AMB_ENGINE` is required. The provider refuses to start without it, and so do
`run_amb.py --memory icm`, `recall_only.py --memory icm`, each side of
`compare_builds.py`, `entrypoint.sh` and `k8s/render.sh` with `MEMORY=icm`. The
reason: the engine a binary runs when a request names none changed (v2 on current
builds, the previous engine before), so an unnamed run measured one thing on an
old build and another on a new one, under the same label.

| `ICM_AMB_ENGINE` | `/recall` carries | dates sent | what it measures |
|---|---|---|---|
| `v2` | `"engine": "v2"` | document date as `created_at` on `/store`, question date as `now` on `/recall` | v2 with everything the dataset gives it: the setting of the published LoCoMo and PersonaMem runs |
| `v2` + `ICM_AMB_STORE_DATE=0 ICM_AMB_QUERY_NOW=0` | `"engine": "v2"` | none | v2 as a system that is handed no date |
| `legacy` | `"engine": "legacy"` | none | the previous engine: the baseline. Also the value for a binary older than v2, which ignores the field and runs its only engine |
| `binary-default-no-dates` | no `engine` field | none | whatever the binary picks by itself, as a bare HTTP client gets it. Not the baseline, not the v2 setting; two builds measured this way may not run the same engine |

The choice is written where a reader of the result will find it: at the end of the
run description in the result file (`... | engine v2, document date sent
(created_at), question date sent (now)`), in the provider description, in
`ingest-stats.json` (`engine`, `store_date`, `query_now`, and
`engine_field_in_binary`: whether the binary knew the `engine` field), and, when
`ICM_AMB_HTTP_TRACE=<file>` is set, request by request as it was sent
(`entrypoint.sh` and `compare_builds.py` set it). Under `v2` a binary without the
v2 fields stops the run (it accepts an unknown engine name) rather than being
measured under a v2 label.

## Token budget

`ICM_AMB_MAX_TOKENS` (v2 only) replaces the cut at k by a token budget.
The budget is in ICM's unit, which is not the harness' unit. On
`/recall?format=json`, the format this provider uses, a memory costs
ceil(characters of its whole indented JSON row / 4): the summary and every other
field of the memory, roughly 110 to 140 tokens of envelope per 512-token chunk.
The harness counts the summaries with tiktoken, so the context it reports is
smaller than the budget. Simulated ratio ICM cost / tiktoken (a Python replica of
the formula on the real chunks, not a measurement on the binary): 1.13 on LoCoMo,
1.60 on PersonaMem; `max_tokens=32768` gives about 28,900 context tokens on
LoCoMo and 20,500 on PersonaMem. An earlier version of this page gave the unit as
characters / 4 plus 8 per memory with ratios 0.89 and 1.10: that was the unit
before the JSON row was charged. Do not derive the parameter from a target:
calibrate it with `compare_builds.py --no-llm`, and report the
`avg_context_tokens` of the result file, not the parameter.

## Recall latency

`avg_retrieve_time_ms` is the harness' mean over the questions. On a dataset the
harness runs unit by unit (LoCoMo) the unit's server is warm from its ingestion
and the mean is ICM's recall latency. On a dataset it runs in one batch
(PersonaMem: everything is ingested, then every question is asked) each of the 37
contexts starts its `icm serve` and loads the embedding model inside the timed
retrieve, and the three other concurrent questions wait for it. Measured locally
on one PersonaMem shard with the real binary (21 questions, 2 contexts): mean
1,579 ms in batch, 310 ms with `ICM_AMB_UNIT_CHECKPOINT=personamem`, which keeps
the server warm as on LoCoMo. For a batch run, quote the median
(`median_retrieve_time_ms`, written by `merge_shards.py`) and say that the mean
includes one server start per context.

## GKE

Target: cluster `rtk-bench` (zone `europe-west9-a`, project `rtk-ai-labs-01`).
The local kubeconfig also holds a production cluster, so every command names the
context explicitly:

```bash
CTX=gke_rtk-ai-labs-01_europe-west9-a_rtk-bench

# Image (native amd64 build on Cloud Build, pushed to the rtk-bench registry)
gcloud builds submit --project rtk-ai-labs-01 --config bench/amb/cloudbuild.yaml \
  --substitutions _TAG=baseline-0.10.65 .

# Namespace + service account (see the prerequisites listed in k8s/base.yaml)
kubectl --context $CTX apply -f bench/amb/k8s/base.yaml

# One dataset = one Indexed Job. render.sh fills the defaults (one replacement
# pod per shard, provider defaults) and refuses a missing or malformed variable.
# What every Job of this page shares is exported once; what belongs to one run is
# set inside the parentheses of that run, so nothing of a run reaches the next one.
export IMAGE=europe-west9-docker.pkg.dev/rtk-ai-labs-01/rtk-bench/icm-amb@sha256:<digest>
export RESULTS_BUCKET=<bucket> PARALLELISM=3

# LoCoMo: 10 conversations, 3 shards (4, 3 and 3 conversations: 663, 389, 488 questions).
# RUN_ID: no dots, new for each configuration. ICM_AMB_ENGINE: required with MEMORY=icm.
( export RUN_ID=v2-20261004 RUN_NAME=icm-v2 MEMORY=icm ICM_AMB_ENGINE=v2 \
    DATASET=locomo SPLIT=locomo10 SHARDS=3 \
    RUN_DESCRIPTION="ICM <git sha>, LoCoMo answers, k=50, chunks 512, date header"
  bench/amb/k8s/render.sh | kubectl --context $CTX apply -f - )

# PersonaMem: one checkpoint per context instead of one save at the end of the shard
( export RUN_ID=v2-20261004 RUN_NAME=icm-v2 MEMORY=icm ICM_AMB_ENGINE=v2 \
    DATASET=personamem SPLIT=32k SHARDS=3 ICM_AMB_UNIT_CHECKPOINT=personamem \
    RUN_DESCRIPTION="ICM <git sha>, PersonaMem answers, k=50, chunks 512, date header"
  bench/amb/k8s/render.sh | kubectl --context $CTX apply -f - )

# Results: wait for every index of the Job to succeed, then check and merge
gcloud storage cp -r gs://$RESULTS_BUCKET/v2-20261004 ./out-gke
python bench/amb/merge_shards.py out-gke/v2-20261004/shard-*/locomo/icm-v2/rag/locomo10.json \
  --expect-queries 1540 --expect-docs 272 -o out-gke/merged/locomo/icm-v2/rag/locomo10.json
python bench/amb/merge_shards.py out-gke/v2-20261004/shard-*/personamem/icm-v2/rag/32k.json \
  --expect-queries 589 --expect-docs 195 -o out-gke/merged/personamem/icm-v2/rag/32k.json
# What the run cost, from the API's own counts (the pods keep them next to the results)
python bench/amb/estimate_cost.py --results out-gke/merged/locomo/icm-v2/rag/locomo10.json \
  --usage out-gke/v2-20261004/shard-*/usage-locomo-locomo10-icm-v2.jsonl \
  --answer-price <in> <out> --judge-price <in> <out>
```

The description stored in the result file is `RUN_DESCRIPTION`, then the engine and
the dates sent (added by the benchmark process, from `ICM_AMB_ENGINE`), then the
image reference: the file says what produced it without the launch command.

Baselines and recall-only runs use the same template. `MEMORY=full-context` or
`MEMORY=bm25` (with their own `RUN_ID`, `RUN_NAME` and description) run the two LoCoMo
baselines; `ICM_AMB_K=5` sets ICM's cut. `MODE=recall` swaps the benchmark process
for `recall_only.py` (no model call; `MEMORY` icm or bm25, `ICM_AMB_RECALL_UNIT` for
the indexed unit) and keeps the sharding, the per-unit save, the resume and the sync:

```bash
# The five LoCoMo runs of "Memory or reader model?": one Job at a time (shared quota)
( export RUN_ID=s4-full RUN_NAME=full-context MEMORY=full-context \
    DATASET=locomo SPLIT=locomo10 SHARDS=10 ICM_AMB_SHARD_BY=rank \
    RUN_DESCRIPTION="LoCoMo full context: every session, oldest first, date header, no retrieval"
  bench/amb/k8s/render.sh | kubectl --context $CTX apply -f - )
( export RUN_ID=s4-bm25 RUN_NAME=bm25 MEMORY=bm25 \
    DATASET=locomo SPLIT=locomo10 SHARDS=10 ICM_AMB_SHARD_BY=rank \
    RUN_DESCRIPTION="LoCoMo harness bm25, k=10, chunks 512, no date"
  bench/amb/k8s/render.sh | kubectl --context $CTX apply -f - )
for K in 5 10 20; do
  ( export RUN_ID=s4-icm-k$K RUN_NAME=icm-v2-k$K MEMORY=icm ICM_AMB_ENGINE=v2 ICM_AMB_K=$K \
      DATASET=locomo SPLIT=locomo10 SHARDS=10 ICM_AMB_SHARD_BY=rank \
      RUN_DESCRIPTION="ICM <git sha>, LoCoMo answers, k=$K, chunks 512, date header"
    bench/amb/k8s/render.sh | kubectl --context $CTX apply -f - )
done

# LongMemEval-S, session recall: 500 questions, 23,867 sessions to embed, no model.
# First the run that compares with the published figures: v2, no date handed to ICM.
( export RUN_ID=lme-icm-nodate RUN_NAME=icm-v2-nodate-user MODE=recall MEMORY=icm \
    ICM_AMB_ENGINE=v2 ICM_AMB_STORE_DATE=0 ICM_AMB_QUERY_NOW=0 ICM_AMB_RECALL_UNIT=session-user \
    DATASET=longmemeval SPLIT=s SHARDS=12 ICM_AMB_SHARD_BY=rank BACKOFF_LIMIT_PER_INDEX=3 \
    RUN_DESCRIPTION="ICM <git sha>, LongMemEval-S recall only, unit session-user, protocol of the published figures"
  bench/amb/k8s/render.sh | kubectl --context $CTX apply -f - )
# Then, under its own name, v2 with the session dates and the question date.
( export RUN_ID=lme-icm-dated RUN_NAME=icm-v2-dated-user MODE=recall MEMORY=icm \
    ICM_AMB_ENGINE=v2 ICM_AMB_RECALL_UNIT=session-user \
    DATASET=longmemeval SPLIT=s SHARDS=12 ICM_AMB_SHARD_BY=rank BACKOFF_LIMIT_PER_INDEX=3 \
    RUN_DESCRIPTION="ICM <git sha>, LongMemEval-S recall only, unit session-user, dated variant"
  bench/amb/k8s/render.sh | kubectl --context $CTX apply -f - )
gcloud storage cp -r gs://$RESULTS_BUCKET/lme-icm-nodate ./out-gke
python bench/amb/merge_recall.py out-gke/lme-icm-nodate/shard-*/longmemeval/icm-v2-nodate-user/recall/s.json \
  --expect-queries 500 --expect-docs 23867 -o out-gke/merged/longmemeval/icm-v2-nodate-user/recall/s.json
```

A recall-only pod saves after every question, so a restart loses one question's
ingestion (about 48 memories). Nothing is paid per attempt, which is why the
example allows three replacements per shard.

The LongMemEval-S file (277 MB on Hugging Face) is in the image (`fetch_dataset.py`
runs at build time, with retries and resume; `--build-arg LONGMEMEVAL=0` leaves it
out). A pod whose image does not have it fetches it the same way before the run
starts, instead of leaving it to the harness' single unretried download.

To keep the binary of an earlier run and only change the tooling, build
`Dockerfile.tooling` on top of that run's image (`cloudbuild.tooling.yaml`).

Shards. `ICM_AMB_SHARD_BY` unset deals the isolation units by sha1 modulo
`SHARDS`, which is uneven: LoCoMo with `SHARDS=10` gives 3 empty shards and 3
shards of two conversations (one conversation per shard needs `SHARDS=26`, 16 of
them empty). An empty shard is harmless but not free: its pod starts, clones the
harness, writes a result file with 0 questions and succeeds, and `merge_shards.py`
needs that file like any other. `ICM_AMB_SHARD_BY=rank` deals the units
round-robin in sorted order: `SHARDS=10` is then exactly one LoCoMo conversation
per pod. More shards do not reduce what a restart costs on LoCoMo (the unit in
progress, in both cases); they only isolate a failing unit.

Restarts. The harness saves the result file after each finished isolation unit;
`entrypoint.sh` uploads it within `SYNC_INTERVAL` (15 s) of each save and once
more when the pod stops, SIGTERM included. A replacement pod downloads it and
resumes at the first unfinished unit; if the download fails it retries, then the
pod fails before any model call instead of starting over. An upload never
replaces a result that holds questions the local file lacks. What a restart costs:
the answers of the unit in progress (LoCoMo: up to 199 questions; PersonaMem with
`ICM_AMB_UNIT_CHECKPOINT`: up to 28). Without `ICM_AMB_UNIT_CHECKPOINT` PersonaMem
has no unit: nothing is saved before the end and a restart replays the whole
shard; use `SHARDS=12` then (shards of 21 to 128 questions).

`ICM_AMB_UNIT_CHECKPOINT=personamem` changes the order of the work, not the
benchmark: same 589 questions, same documents reachable by each question, same
prompts (checked row by row against the batch run). It is still a departure from
the upstream schedule, to state when publishing, and it changes the retrieve
latency (see "Recall latency").

Rate limits. Vertex AI answers HTTP 429 often on these models (about 500 per pod
and per hour were seen with 3 pods); the harness retries them (6 tries per call,
then 4 per question). The quota is shared by every pod of the project: keep
`PARALLELISM` at 3 or below and do not run two datasets at once.

After the merge. `merge_shards.py` refuses a missing shard, a shard given twice
and a total that differs from `--expect-queries`, and exits 3 when rows went
through the harness' silent fallback (a judge output it could not parse is
counted correct): the expected count is 0.

One-time prerequisites, not created by the manifests: a results bucket; a Google
service account `icm-amb` with `roles/aiplatform.user` on the project and object
admin on the bucket; its Workload Identity binding to `icm-bench/icm-amb`. The
cluster already has Workload Identity enabled. `kubectl` needs
`gke-gcloud-auth-plugin` to authenticate to GKE.
