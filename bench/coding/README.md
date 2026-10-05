# ICM on a coding-agent benchmark (sdebench)

Measures ICM where it is meant to be used: installed in a coding agent by
`icm init`, with its hooks and its MCP server, on bug-fix tasks whose correct fix
depends on a decision made in an earlier developer conversation.

The benchmark is sdebench, the coding-agent part of the Agent Memory Benchmark
(harness: https://github.com/vectorize-io/agent-memory-benchmark, dataset:
https://github.com/vectorize-io/sde-bench). Both belong to Vectorize, the
publisher of Hindsight. The harness declares no license, so nothing of it is
copied here: the scripts load it from a checkout and add one arm. The dataset is
MIT.

Status. The adapter's control flow is tested offline against the real upstream
harness with a stand-in `docker` (see Tests). The four container scripts have
also been run outside Docker with a real `icm` 0.10.65 binary on the real task
data: `icm init --mode all` writes what `setup.sh` checks, the MCP server lists
its tools, both hooks are replayed, the telemetry has the format `collect.sh`
counts. What has never run: the image itself, and a real agent. The first thing
to do with Docker available is the dry run and its gate, below.

## Files

- `icm_sde_run.py` - one task: wraps upstream `sdebench/harness/run.py`, adds
  `--history icm` and `--dry-run`
- `icm_corpus.py` - a task's corpus laid out for `icm import` (the import
  order), and the test "does this injected line come from the task's own
  documents?"
- `icm_coding.py` - `icm-coding` memory provider (one seeded database per task)
  and the coding mode that dispatches to it
- `run_sdebench.py` - launcher: upstream CLI with the two above registered
- `container/setup.sh` - in the agent container: `icm init`, then checks what it wrote
- `container/probe.sh` - replays one of ICM's two injecting hooks (SessionStart,
  UserPromptSubmit) on a copy of the database
- `container/collect.sh` - reads ICM's hook telemetry after the last turn
- `container/seed.sh` - builds a task's database with `icm import`
- `Dockerfile.agent-icm` - agent images (claude, codex, opencode) with the `icm`
  binary built from this checkout
- `sdebench_stats.py` - reads result files: first-try rate, corrections, by
  source, the ICM arm's own measures, paired tests between two runs, the gate
- `tests/` - offline tests and the stand-in `docker`

## What the ICM arm is

Same exposure as upstream's reference memory arm (`hindsight-coding`), with ICM
in place of the Hindsight plugin:

- full repository with its git history, as in every arm;
- the past developer conversations are NOT placed on disk (upstream does that
  for the no-memory arm only);
- one memory store per task, filled before the agent starts;
- the agent runs in a fresh container where `icm init --mode all` has run:
  SessionStart, UserPromptSubmit, PreToolUse, PostToolUse, PreCompact and
  SessionEnd hooks, the MCP server, the instruction block in `CLAUDE.md`, the
  slash commands. Nothing is wired by hand; `setup.sh` fails the task if a piece
  is missing;
- the seeded database is copied into the container, never mounted: what the
  agent stores during a task is discarded with the container, and the same seed
  serves every repetition.

Four differences from the reference arm, to state with any published number:

1. Seeding. The reference arm fills its bank with the plugin's `deepen` engine:
   LLM extraction over the conversations and the git history, then LLM-written
   knowledge pages. ICM has no git-history ingestion at all. The seed here is
   `icm import` (no LLM, no key) over the dataset's own corpus for the task: the
   task's conversation(s), the 140 decoy conversations, and the commit messages
   (decision commit of a history task, 100 commits of the host repository).
   `ICM_SDE_GIT_INGEST=none` leaves the commit messages out. `icm import` keeps
   sentences, verbatim: it selects them with the local embedding model by
   default (`ICM_SDE_IMPORT=default`, about 8 minutes per task measured outside
   Docker on a 6-core laptop) or with keyword rules (`ICM_SDE_IMPORT=rules`,
   about a second per task, nothing embedded, so the agent's own recalls are
   keyword-only too). One of the 61 tasks, `boltons-omdset-001`, has no document
   of its own in the dataset's corpus: memory cannot help there.
2. Delivery. The reference hook asks the Hindsight server for a `reflect`: a
   server-side LLM writes an answer to the bug report, and that answer is
   injected. ICM injects stored notes, twice, with no model involved:
   - at SessionStart, a pack of about 2 000 characters: the latest notes of the
     project, then its earliest high-importance notes. It is selected by
     importance and insertion order, not by search. See "Import order";
   - at every UserPromptSubmit, up to 5 notes of 400 characters each, found by a
     lexical search on the first 200 bytes of the prompt. On this benchmark 88
     to 103 of those bytes are the harness' own header, which leaves room for a
     quarter to a half of the bug report.

   The agent can also call the MCP tools or the `icm` CLI on its own.
3. MCP. The reference arm uses no MCP server for Claude Code. `--mode all` adds
   ICM's. Set `ICM_SDE_INIT_MODE=standard` for ICM's own default (hooks,
   instructions and slash commands, no MCP).
4. Writing during the task. Upstream turns the plugin's write-back off for its
   reference arm (no session is written into the bank between rounds). ICM's
   write hooks stay on, as `icm init` installs them: PostToolUse extracts notes
   from tool output, PreCompact and SessionEnd from the transcript. What the
   first turn stores can be recalled in a correction round of the same task.
   Nothing crosses tasks. Each row records `stored_during_task`.

One ICM setting differs from its default, in the image. With
`extraction.summarizer.provider = auto` (the default), the SessionEnd hook
starts a worker that runs the agent's own CLI (`claude -p --model
claude-haiku-4-5`) to extract notes: model calls made with the benchmark's key
that the harness does not see and the cost column does not contain. The image
sets the provider to `none` (ICM's local extractor), and `setup.sh` fails the
task when `icm config` says otherwise. To measure ICM's default instead, build
with `--build-arg ICM_EXTRACTION_PROVIDER=auto`, run with
`ICM_SDE_EXTRACTION=auto`, and take the cost from the provider's console.

## Import order

`icm import` reads a directory in file-name order, and the SessionStart pack
selects by insertion order: with every note imported at the same moment and
half of them rated high, "the earliest high-importance notes" are the first
files imported and "the latest notes" the last ones. `icm wake-up` and the
`icm_wake_up` MCP tool return the same pack. The dataset has no dates, so the
import order is the adapter's choice, and it must not decide what the agent
reads. Imported in the dataset's own order (the task's conversation first), the
pack carries lines of the conversation that holds the decision on 54 of the 61
tasks, whatever the bug report says (measured, rule-based import).

So the adapter (`icm_corpus.py`):

- writes conversations and commit messages in one directory, imported in one
  pass (two passes would make the decision commit one of "the latest");
- orders the shared documents (decoys, host commits) by `sha256(seed, document
  id)`, a keyed shuffle that is the same on any machine. The seed is
  `ICM_SDE_ORDER_SEED` (default `icm-sde-1`) and is recorded in each task's
  `seed.json` and result;
- places the task's own documents, by the same hash, in the middle half of that
  order, never in the first or last quarter. A plain shuffle still puts one of
  them at an end now and then: the pack carried their lines for 5 of 183 task
  and seed pairs tried. They keep the dataset's order among themselves, so an
  amending conversation stays after the one it amends;
- names files and session ids by import rank only: nothing says which
  conversation is the task's and which is a decoy;
- checks the outcome instead of trusting the construction: SessionStart is
  replayed on every task before the agent starts, and the injected lines that
  are passages of the task's own documents are counted (`start_task_lines`). A
  dry run reports the count. A paid run refuses the task when it is not zero.

This is the conservative choice: the pack is kept as `icm init` installs it,
and it never carries the decision. In real use a recent decision would often be
among the latest notes; this benchmark cannot measure that. With the default
seed the pack carries no line of the task's documents on 61 of 61 tasks. On the
one task tried, `icm wake-up` had to be asked for more than 8 000 tokens (the
command's default is 200) before one appeared. What the search hook returns is identical
under both import orders on 61 of 61 tasks: it does not depend on position.

## Prerequisites

On one machine with a local Docker daemon (the upstream harness drives `docker`
directly; it does not run in a Kubernetes Job):

```bash
# 1. Harness at the pinned commit, with the dataset submodule
git clone https://github.com/vectorize-io/agent-memory-benchmark.git /path/to/amb
git -C /path/to/amb checkout f618ed7b1f0eb9cad7b42e876f91a42f0eadb150
git -C /path/to/amb submodule update --init sdebench/datasets
export AMB_HOME=/path/to/amb

# 2. Host repository of the tasks (every task's build.py copies it)
git clone https://github.com/vectorize-io/boltons /path/to/boltons
export SDEBENCH_BOLTONS_HOST=/path/to/boltons

# 3. Python: `python` must be on PATH (upstream calls `python build.py`)
uv venv .venv --python 3.12 && . .venv/bin/activate
uv pip install -r bench/amb/requirements.txt

# 4. Images. Grading image from upstream, agent image from this checkout.
docker build -t sdebench-base -f $AMB_HOME/sdebench/Dockerfile $AMB_HOME/sdebench
docker build -f bench/coding/Dockerfile.agent-icm --target claude \
  --build-arg CLAUDE_CODE_VERSION=<pin a version> -t icm-sde-agent-claude .
```

The harness repository is large (published result files). `git fetch --depth 1
--filter=blob:none origin <commit>` with a sparse checkout of `src/`,
`sdebench/` and `pyproject.toml` is enough for everything here.

The agent image compiles `icm` (several minutes) and downloads the embedding
model (about 2 GB). Pin the agent version and write it in the run description:
the upstream images install whatever is current on the day of the build, and the
published result files do not record it.

The task's project name must be the one the seed is stored under: ICM derives
it in the container from the `origin` remote of `/work`, which is the remote of
the `boltons` clone above (`ICM_SDE_PROJECT`, default `boltons`). A SessionStart
replay that injects nothing from a seeded database means the two differ.

## 1. Dry run (no model, no key, no cost)

```bash
export SDE_AGENT=claude-code
ICM_SDE_DRY_RUN=1 python bench/coding/run_sdebench.py run \
  --dataset sdebench --split boltons --mode coding --memory icm-coding \
  --name icm-dry --output-dir out

python bench/coding/sdebench_stats.py --gate out/sdebench/icm-dry/coding/boltons.json
```

`SDE_TASK_FILTER=boltons-dedupe` restricts it to three tasks, one per source.
For each task the dry run seeds the database with the real `icm import`, starts
the real container, runs the real `icm init`, checks the hooks, the MCP entry,
the permission list and the extraction provider, asks the MCP server for its
tools, replays the SessionStart hook and then the UserPromptSubmit hook on the
real first prompt, and grades the untouched repository. No agent is started.
Without a Claude credential in the environment it passes a placeholder key, so
that upstream does not mount `~/.sdebench/claude_creds.json` (Docker would
create a directory at that path when the file is missing). Expected: one row per
task, all unsolved, `interventions` 0, cost 0.

The stats line of a dry run reads, per source:

- `in seed n/N`: tasks whose seeded database holds at least one note taken from
  the task's own documents (the conversation or commit that carries the
  decision). Elsewhere memory cannot help;
- `by position n/N`: tasks where the replayed SessionStart pack carries such a
  note. Must be 0;
- `by search n/N`: tasks where the replayed UserPromptSubmit hook carries such a
  note for the first prompt. This is what a paid run can measure;
- `hooks 0/N`: a dry run starts no agent.

`--gate` exits 1 unless a paid run can measure retrieval: SessionStart replayed
on every task and no task delivered by position, the decision in the seed for
at least `--min-seed` of the conversation tasks (default 0.9), found by search
for at least `--min-search` of them (default 0.5; conversation and
conversation-amended together). The second threshold is a choice, not a
derivation: set it before reading the numbers.

Then read, per task:

- `out/sdebench/icm-dry/_store/boltons/all/icm-coding/<task>/seed.json` and
  `seed-report.txt`: the import mode and order, the rank of the task's
  documents, how many notes `icm import` kept and how many come from the task's
  own documents;
- the `context` of each row in `out/sdebench/icm-dry/coding/boltons.json`: the
  two hooks' outputs, apart, SessionStart first. Compare the second with the
  task's `policy` field in its `task.json`: a note from the right conversation
  is not always the sentence that states the rule;
- `/tmp/sdebench/run/<task>_icm_<id>/result.json`: the `icm` block and the
  `memory_diag` events (wiring lines, MCP tools, both replays with their
  `task_lines`).

One task without the AMB runner (it needs a database already seeded, in
`ICM_SDE_DB`; without it the agent starts with an empty memory):

```bash
python bench/coding/icm_sde_run.py --dry-run --agent claude-code \
  --task $AMB_HOME/sdebench/datasets/boltons-dedupe/tasks/main/task.json
```

Measured so far, outside Docker, with `icm` 0.10.65, `ICM_SDE_IMPORT=rules` and
the default order seed, on the 61 tasks:

| source | n | in seed | by position | by search |
| --- | --- | --- | --- | --- |
| conversation | 27 | 26 | 0 | 18 |
| conversation-amended | 6 | 6 | 0 | 4 |
| history | 28 | 24 | 0 | 17 |

On the 33 conversation tasks the search hook finds a note of the right
conversation for 22 with the real prompt, 28 when the 200 bytes start at the bug
report instead of the harness header, 24 with the whole prompt and 29 with the
whole bug report. With the default import (embedding model) four conversation
tasks were tried, and the search hook found a note of the right conversation
for three: redo the dry run in the image before paying, the gate is there for
that.

The dry run does not prove that Claude Code loads the hooks and the MCP server
from the files `icm init` wrote. That is read on the first paid task.

## 2. Paid runs

```bash
export SDE_AGENT=claude-code              # model: claude-sonnet-5 (upstream default)
export ANTHROPIC_API_KEY=...              # or CLAUDE_CODE_OAUTH_TOKEN=...

# ICM arm, three repetitions; the seed is built once and reused
python bench/coding/run_sdebench.py run --dataset sdebench --split boltons --mode coding \
  --memory icm-coding --name icm-claude-1 --output-dir out \
  --description "ICM <git sha>, icm init --mode all, seed icm import <mode>, order seed icm-sde-1, claude-code <version>"
for i in 2 3; do
  mkdir -p out/sdebench/icm-claude-$i && cp -R out/sdebench/icm-claude-1/_store out/sdebench/icm-claude-$i/
  python bench/coding/run_sdebench.py run --dataset sdebench --split boltons --mode coding \
    --memory icm-coding --name icm-claude-$i --output-dir out --skip-ingestion
done

# No-memory control, SAME image (same agent version), three repetitions
for i in 1 2 3; do
  SDE_AGENT_IMAGE_CLAUDE=icm-sde-agent-claude python bench/coding/run_sdebench.py run \
    --dataset sdebench --split boltons --mode coding --memory vanilla \
    --name vanilla-claude-$i --output-dir out
done

python bench/coding/sdebench_stats.py out/sdebench/*/coding/boltons.json
python bench/coding/sdebench_stats.py --pair out/sdebench/vanilla-claude-1/coding/boltons.json \
                                             out/sdebench/icm-claude-1/coding/boltons.json
```

`--category conversation` restricts a run to the 27 tasks whose decision lives
only in a past conversation, `--category conversation-amended` to the 6 where a
later conversation amends an earlier one. Those 33 tasks are where the published
arms differ; the 28 `history` tasks do not separate them with Claude Code.

A paid task of the ICM arm stops the run, and writes no row, when:

- `icm init` left a piece of the wiring missing, the MCP server lists no tool,
  or the extraction provider is not the one asked for (before the agent starts);
- the SessionStart pack carries a line of the task's own documents (before the
  agent starts);
- no Claude credential is set and the credentials file upstream would mount
  does not exist (before the container starts);
- ICM's telemetry holds no UserPromptSubmit row once the agent has finished:
  the agent ran without ICM's hooks. The task was paid for; its `result.json`
  is kept with `memory_run: false`, and the run stops there rather than
  scoring it as a memory run.

The control arm is upstream's own `vanilla` arm: upstream starts each of its
tasks with `uv run python run.py` inside `AMB_HOME`, so `uv` must be on PATH
(the launcher checks it), and the first such run creates upstream's environment
in the checkout (network, a few minutes).

Keys. Claude Code: `ANTHROPIC_API_KEY` or `CLAUDE_CODE_OAUTH_TOKEN` (or the
credentials file upstream mounts from `~/.sdebench/claude_creds.json`). Codex:
`OPENAI_API_KEY`. opencode: `GEMINI_API_KEY`. The upstream CLI refuses to start
without `GEMINI_API_KEY` and builds a Groq client by default, even in coding
mode where neither is used; the launcher fills a placeholder and selects the
Gemini class for agents other than opencode. ICM itself needs no key. Upstream
passes the keys to `docker run` as `-e NAME=value`, visible in the process list
of the machine: use a dedicated machine or a short-lived key.

Cost and time, from the published result files (Claude Code, `claude-sonnet-5`,
61 tasks, three runs each; agent cost as reported by Claude Code, agent time
summed over the tasks):

| arm | cost per run (USD) | agent time per run |
| --- | --- | --- |
| no memory | 26.33 to 27.75 | 4 905 to 5 367 s |
| Hindsight plugin | 19.53 to 21.50 | 4 368 to 4 893 s |
| same, 33 conversation tasks only, no memory | 18.84 to 19.58 | 3 780 to 3 936 s |
| same, 33 conversation tasks only, Hindsight | 12.53 to 13.75 | 2 847 to 3 053 s |

An ICM run should land between the two arms if ICM helps, and at or above the
no-memory arm if it does not (the hooks add context to every prompt). Budget for
the six runs above: 140 to 170 USD. The 33-task variant: 95 to 120 USD. A first
signal costs one ICM run on the 33 tasks, 13 to 20 USD. The Hindsight figures
exclude what its server's own LLM costs (extraction at ingestion, one `reflect`
per task): the result files record neither.

Wall time. The runner handles one sdebench task at a time (one query per
isolation unit), whatever `SDE_CONCURRENCY` says. The six runs therefore take
their 8 to 9 hours of agent time in wall time too, plus the repository builds,
the grading containers, ICM's own hooks, and the seeding of the 61 databases
(once; about 8 hours with `ICM_SDE_IMPORT=default` at the rate measured above,
about a minute with `rules`).

To go faster, split one run into processes over disjoint tasks and read the
parts as one run:

```bash
SDE_TASK_FILTER=boltons-a,boltons-b,boltons-c python bench/coding/run_sdebench.py run ... --name icm-1-part1 &
SDE_TASK_FILTER=boltons-d,boltons-e,boltons-f python bench/coding/run_sdebench.py run ... --name icm-1-part2 &
wait
python bench/coding/sdebench_stats.py \
  out/sdebench/icm-1-part1/coding/boltons.json,out/sdebench/icm-1-part2/coding/boltons.json
```

`SDE_TASK_FILTER` is a comma-separated list of substrings of the task directory
names. The parts must not share a task: upstream rebuilds
`/tmp/sdebench/hist/<task directory>` for every task it runs, so two processes
on the same task at the same time (two parts that overlap, two repetitions, the
ICM arm and its control) break each other. The stats script refuses a task
present in two parts.

## Reading a result

The solve rate does not separate the arms: with five corrections allowed, every
published run solves 60 or 61 of 61 tasks. Read `first_try` and `corrections`,
by source (`sdebench_stats.py`). Published values, Claude Code, 27 conversation
tasks: first try 0 to 1 without memory, 11 to 16 with Hindsight; corrections 37
to 41 without, 12 to 18 with. One system varies by that much from run to run, so
a difference of a few tasks between two memory systems needs three runs each.

For a run of the ICM arm the stats line replaces `injected` by the adapter's own
measures: `hooks n/N` (tasks where the agent's own UserPromptSubmit reached ICM,
from ICM's telemetry; anything below N/N is flagged, those rows are not memory
runs), then `in seed`, `by position` and `by search` as in the dry run. The
`context` of a row holds the two replays made before the agent started, not a
record of what the agent received: it says what ICM would inject, `hooks` says
whether the agent's session asked for it. `session_start_fired` in the row's
`reasoning` says whether Claude Code in `-p` mode triggered SessionStart at
all; look at it, and at `mcp__icm__*` calls in the trajectory, on the first paid
task.

## Tests

```bash
python -m unittest discover -s bench/coding/tests -v      # unit tests only

ICM_SDE_TEST_AMB_HOME=/path/to/amb ICM_SDE_TEST_BOLTONS=/path/to/boltons \
  python -m unittest discover -s bench/coding/tests -v    # + upstream harness, stand-in docker
```

The unit tests cover the import order, the recognition of the task's own lines
in a hook's output, the stats and the gate, the launcher's preflight, and
`setup.sh` and `seed.sh` run against a stand-in `icm`. The integration tests
execute upstream `run.py` and the AMB runner for real (repository build, prompt,
correction loop, result files) with `tests/fake_docker.py` answering the
`docker` calls. They cover: dry run with both replays, a task delivered by
position (reported in a dry run, refused in a paid one), scripted agent with a
correction round, solved task, a paid task without hooks, missing credentials,
broken wiring (the task stops, nothing is scored), missing or failed seed,
unseeded run, other arms passing through untouched, the AMB runner end to end
with the ICM arm and with upstream's control arm. They run one at a time: like
upstream, they share `/tmp/sdebench`.

## DolphinBench (Mem0)

No adapter here. DolphinBench is not a coding benchmark: three simulated people,
about 500 000 tokens of dated personal and work messages each, and 600 tasks
performed through simulated apps (mail, calendar, chat, documents). Claude Code
is one of its two agent harnesses, used as a personal assistant. Its public
interface (five methods in a Python class, `docs/DRIVER_CONTRACT.md` of
https://github.com/mem0ai/dolphinbench) would take ICM, and
`harness/claude_driver.py` already runs Claude Code with injected hooks and an
MCP configuration. As published by Mem0, a run costs 1 133 to 1 831 USD per
memory system with Claude Code, of which more than 90 % is the ingestion of the
history through the agent. sdebench comes first: it is a coding benchmark, and a
first signal costs a hundredth of that.
