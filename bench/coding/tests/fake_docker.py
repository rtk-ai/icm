#!/usr/bin/env python3
"""Stand-in for the `docker` CLI, for the offline tests of the sdebench ICM arm.

It never starts a container, a model or `icm`. It records every call it receives
(one JSON line per call in $FAKE_DOCKER_STATE/calls.jsonl) and answers the calls
the harness and the ICM wrapper make with canned output, so the whole control
flow (upstream run.py + icm_sde_run.py + icm_coding.py) can be executed and
checked without Docker, network or API key.

What it does NOT prove: that `icm init` writes what setup.sh checks, that the
agent loads the hooks and the MCP server, or that `icm import` extracts anything
from the corpus. Those need the real image (`--dry-run` with Docker).

The stand-in seeding "stores" the first line of every corpus file, so the code
that looks for the task's own documents in a seed has something to find.

FAKE_DOCKER_BREAK selects a failure to simulate: `wiring` (icm init left a hook
missing), `llm-extraction` (the image runs ICM's default extraction provider,
which calls a model), `mcp` (the MCP server lists no tool), `seed` (seeding produced no
database), `grade-pass` (the grading container reports all tests green),
`nohook` (the agent ran but ICM's telemetry holds no row of it).
FAKE_DOCKER_START_LINE / FAKE_DOCKER_PROMPT_LINE add one bullet line to what the
replayed SessionStart / UserPromptSubmit hook prints.
"""

from __future__ import annotations

import json
import os
import sys
from pathlib import Path

STATE = Path(os.environ.get("FAKE_DOCKER_STATE") or "/tmp/fake-docker-state")
BREAK = os.environ.get("FAKE_DOCKER_BREAK", "")
CID = "fakecid0001"

TOOLS = ("icm_memory_store", "icm_memory_recall", "icm_memory_forget", "icm_memory_stats")


def log(argv: list[str], stdin: str | None) -> None:
    STATE.mkdir(parents=True, exist_ok=True)
    with (STATE / "calls.jsonl").open("a") as fh:
        fh.write(json.dumps({"argv": argv, "stdin_chars": len(stdin or ""),
                             "stdin_head": (stdin or "")[:120]}) + "\n")


def flag_value(argv: list[str], flag: str) -> list[str]:
    return [argv[i + 1] for i, a in enumerate(argv[:-1]) if a == flag]


def setup_output(agent: str) -> tuple[str, int]:
    seeded = (STATE / "seeded").is_file()
    wanted = next((a.split("=", 1)[1] for a in sys.argv if a.startswith("ICM_SDE_EXTRACTION=")), "none")
    actual = "auto" if BREAK == "llm-extraction" else wanted
    wiring = [f"ok extraction-{wanted}" if actual == wanted else f"MISSING extraction-{wanted} (icm config says: {actual})",
              "ok hook-prompt", "ok hook-start", "ok hook-post", "ok hook-pre", "ok hook-end",
              "ok permissions-kept", "ok instructions", "ok mcp-server", "ok mcp-allowed"]
    out = ["@@icm_version", "icm 0.0.0-fake", "@@agent_version", f"{agent} 0.0.0-fake", "@@seed",
           f"present {(STATE / 'seeded').read_text()} bytes" if seeded else "absent",
           "@@init", "[hook] Claude Code UserPromptSubmit (auto-recall): installed"]
    if BREAK == "wiring":
        wiring[1] = "MISSING hook-prompt (/root/.claude/settings.json)"
    out += ["@@extraction", actual, "@@wiring", *wiring]
    out += ["@@mcp_tools", "" if BREAK == "mcp" else " ".join(f'"name":"{t}"' for t in TOOLS)]
    out += ["@@stats", "Memories: 42", "@@topics", "decisions-boltons 12"]
    if BREAK == "wiring" or actual != wanted:
        out += ["@@error", "icm init left the wiring incomplete: "
                + ("hook-prompt" if BREAK == "wiring" else f"extraction-{wanted}")]
        return "\n".join(out) + "\n", 1
    out += ["@@ok", "1"]
    return "\n".join(out) + "\n", 0


def claude_stream(argv: list[str]) -> str:
    """A minimal `claude -p --output-format stream-json` transcript: no model behind it."""
    resumed = "--continue" in argv
    events = [
        {"type": "assistant", "message": {"content": [
            {"type": "text", "text": "fake agent: looked at the failing test"},
            {"type": "tool_use", "name": "Bash", "input": {"command": "python -m pytest -q tests/test_regression.py"}},
        ]}},
        {"type": "result", "is_error": False, "num_turns": 2 if resumed else 3,
         "total_cost_usd": 0.01, "result": "done",
         "usage": {"input_tokens": 10, "output_tokens": 20,
                   "cache_read_input_tokens": 300, "cache_creation_input_tokens": 40}},
    ]
    return "\n".join(json.dumps(e) for e in events) + "\n"


def main() -> int:
    argv = sys.argv[1:]
    stdin = None if sys.stdin.isatty() else sys.stdin.read()
    log(argv, stdin)
    if not argv:
        return 0
    verb = argv[0]

    if verb == "image":            # docker image inspect <image>
        return 0

    if verb == "cp":               # docker cp <db> <cid>:/root/icm/memories.db
        source = Path(argv[1])
        (STATE / "seeded").write_text(str(source.stat().st_size))
        return 0

    if verb == "rm":
        return 0

    if verb == "run":
        if "-d" in argv:           # the long-lived agent container
            sys.stdout.write(CID + "\n")
            return 0
        if "pytest" in argv:       # the grading container
            if BREAK == "grade-pass":
                sys.stdout.write("....\n4 passed in 0.01s\n")
                return 0
            sys.stdout.write("F...\nFAILED tests/test_hidden.py::test_policy - AssertionError\n"
                             "1 failed, 3 passed in 0.01s\n")
            return 1
        mounts = flag_value(argv, "-v")
        out_dirs = [m.split(":")[0] for m in mounts if m.split(":")[1:2] == ["/out"]]
        corpus = [m.split(":")[0] for m in mounts if m.split(":")[1:2] == ["/corpus"]]
        if out_dirs and "import /corpus" in (stdin or ""):     # the seeding container
            out = Path(out_dirs[0])
            files = sorted(p for p in Path(corpus[0]).iterdir() if p.is_file()) if corpus else []
            notes = []
            for path in files:                                 # in the order `icm import` reads
                first = path.read_text().splitlines()[0]
                text = json.loads(first)["message"]["content"] if path.suffix == ".jsonl" else first
                notes.append({"type": "memory", "summary": text, "importance": "high",
                              "source": {"thread_id": path.stem}})
            mode = next((a.split("=", 1)[1] for a in argv if a.startswith("ICM_SDE_IMPORT=")), "default")
            report = ["@@icm_version", "icm 0.0.0-fake", "@@import_mode", mode, "@@import",
                      f"Imported {len(notes)} facts from {len(files)} files.",
                      "@@stats", f"Memories: {len(notes)}"]
            if BREAK == "seed":
                (out / "seed-report.txt").write_text("\n".join(report + ["@@error", "no database produced"]) + "\n")
                return 3
            (out / "memories.db").write_bytes(b"SQLite format 3\x00fake")
            (out / "seed-export.jsonl").write_text("".join(json.dumps(n) + "\n" for n in notes))
            (out / "seed-report.txt").write_text("\n".join(report + ["@@ok", "1"]) + "\n")
            return 0
        return 0

    if verb == "exec":
        text = stdin or ""
        if "claude" in argv and "-p" in argv:
            sys.stdout.write(claude_stream(argv))
            return 0
        if "sh" in argv and "-s" in argv:
            if "icm init --mode" in text:
                agent = argv[-1] if argv[-1] in ("claude-code", "codex", "opencode") else "claude-code"
                out, code = setup_output(agent)
                sys.stdout.write(out)
                return code
            if 'icm hook "$EVENT"' in text:                    # probe.sh <start|prompt>
                event = argv[-1]
                payload = json.loads((STATE / f"probe-{event}.json").read_text())
                if event == "start":
                    out = ("# ICM Wake-up (project: boltons)\n\n## Project context\n"
                           "- (fake) a note from some other conversation of the corpus\n")
                    extra = os.environ.get("FAKE_DOCKER_START_LINE")
                else:
                    out = ("Here is context recalled from ICM's memory store (fake):\n\n"
                           f"- (fake) {payload['prompt'][:60]}\n")
                    extra = os.environ.get("FAKE_DOCKER_PROMPT_LINE")
                sys.stdout.write(out + (f"- {extra}\n" if extra else ""))
                return 0
            if "hook-log" in text:
                turns = sum(1 for line in (STATE / "calls.jsonl").read_text().splitlines()
                            if '"claude", "-p"' in line)
                rows = 0 if BREAK == "nohook" else turns
                sys.stdout.write(f"@@hook_prompt_rows\n{rows}\n@@hook_start_rows\n{min(rows, 1)}\n"
                                 f"@@hook_post_rows\n{rows}\n@@hook_end_rows\n{rows}\n"
                                 f"@@hook_stats\nprompt {rows}\n"
                                 f"@@stats\nMemories: {42 + rows}\n@@topics\ndecisions-boltons 12\n")
                return 0
            return 0
        if "sh" in argv and "-c" in argv and "icm-sde-probe-" in argv[-1]:
            event = argv[-1].rsplit("icm-sde-probe-", 1)[1].split(".")[0]
            (STATE / f"probe-{event}.json").write_text(text)
            return 0
        return 0

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
