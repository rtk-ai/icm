#!/usr/bin/env python3
"""Launcher: the upstream Agent Memory Benchmark CLI with the ICM coding arm registered.

    python run_sdebench.py run --dataset sdebench --split boltons --mode coding \
        --memory icm-coding --name icm-claude-1 --output-dir out

Everything after the script name is upstream's own CLI. This launcher only adds
`icm-coding` to the provider registry and replaces the `coding` mode by a subclass
that knows that provider (icm_coding.py). `--memory vanilla` and every other
provider behave exactly as upstream.

The harness is imported from AMB_HOME, never copied (upstream declares no
license). bench/amb/run_amb.py does the import work (optional-dependency
placeholders, sys.path); it is reused as is when present.

    AMB_HOME                 agent-memory-benchmark checkout at the pinned commit,
                             with the sde-bench submodule (sdebench/datasets)
    SDEBENCH_BOLTONS_HOST    clone of https://github.com/vectorize-io/boltons
    SDE_AGENT                claude-code | codex | opencode (upstream default: opencode)

`python` must resolve on PATH: upstream runs each task's build script with
`python build.py`. Activate the virtualenv instead of calling its interpreter
by path.

Every arm other than `icm-coding` is run by upstream's own coding mode, which
starts each task with `uv run python run.py` inside AMB_HOME: `uv` must be on
PATH, and the first such run creates upstream's environment there (network,
a few minutes, a `.venv` in the checkout).
"""

from __future__ import annotations

import os
import shutil
import sys
from pathlib import Path

_HERE = Path(__file__).resolve().parent
_AMB_TOOLS = _HERE.parent / "amb"


def _option(argv: list[str], name: str) -> str | None:
    for i, a in enumerate(argv):
        if a == name and i + 1 < len(argv):
            return argv[i + 1]
        if a.startswith(name + "="):
            return a.split("=", 1)[1]
    return None


def preflight(argv: list[str] | None = None) -> None:
    """Refuse the mistakes that otherwise surface as silently wrong numbers."""
    argv = sys.argv[2:] if argv is None else argv
    problems = []
    home = os.environ.get("AMB_HOME")
    if not home:
        problems.append("AMB_HOME is not set")
    else:
        datasets = Path(home).expanduser() / "sdebench" / "datasets"
        if not any(datasets.glob("boltons-*/tasks/main/task.json")):
            problems.append(f"{datasets} holds no task: run `git submodule update --init` in AMB_HOME")
    host = os.environ.get("SDEBENCH_BOLTONS_HOST")
    if not host:
        # Without it every build.py exits, and the dataset loader silently drops the
        # host-history noise from the corpus (it looks for ~/dev/_sdebench_hosts/boltons).
        problems.append("SDEBENCH_BOLTONS_HOST is not set (clone https://github.com/vectorize-io/boltons)")
    elif not (Path(host).expanduser() / ".git").exists():
        problems.append(f"SDEBENCH_BOLTONS_HOST={host} is not a git clone")
    if shutil.which("python") is None:
        problems.append("`python` is not on PATH (upstream calls `python build.py`): activate the virtualenv")
    for tool in ("git", "docker"):
        if shutil.which(tool) is None:
            problems.append(f"`{tool}` is not on PATH")
    memory = _option(argv, "--memory")
    if _is_coding_run(argv) and memory != "icm-coding" and shutil.which("uv") is None:
        # Found here rather than at the first task, after the store was prepared.
        problems.append(f"`uv` is not on PATH: upstream runs the `{memory or 'default'}` arm "
                        f"with `uv run python run.py` in AMB_HOME")
    if problems:
        raise SystemExit("sdebench preflight failed:\n  - " + "\n  - ".join(problems))


def coding_mode_env() -> None:
    """Satisfy two upstream start-up checks that the coding mode never uses.

    `amb run` exits without GEMINI_API_KEY and builds the answer LLM (default: a Groq
    client) before it looks at --mode, although the coding mode calls neither: the
    agent's own CLI talks to its model. Only opencode, which runs on Gemini, needs a
    real Gemini key; for it nothing is filled in.
    """
    os.environ.setdefault("OMB_ANSWER_LLM", "gemini")
    agent = os.environ.get("SDE_AGENT", "opencode")
    if agent != "opencode" and not (os.environ.get("GEMINI_API_KEY") or os.environ.get("GOOGLE_API_KEY")):
        os.environ["GEMINI_API_KEY"] = "unused-in-coding-mode"


def _is_coding_run(argv: list[str]) -> bool:
    for i, a in enumerate(argv):
        if a == "--mode" and argv[i + 1:i + 2] == ["coding"]:
            return True
        if a == "--mode=coding":
            return True
    return False


def bootstrap():
    """Import the upstream CLI with the ICM coding arm registered. Returns the cli module."""
    sys.path.insert(0, str(_HERE))
    cli = None
    if (_AMB_TOOLS / "run_amb.py").is_file():
        sys.path.insert(0, str(_AMB_TOOLS))
        try:
            import run_amb  # bench/amb launcher: placeholders + registry
            cli = run_amb.bootstrap()
        except Exception as exc:  # bench/amb is optional here; fall back to a plain import
            print(f"[run_sdebench] bench/amb bootstrap not used ({type(exc).__name__}: {exc})",
                  file=sys.stderr)
    if cli is None:
        home = Path(os.environ["AMB_HOME"]).expanduser().resolve()
        sys.path.insert(0, str(home / "src"))
        from memory_bench import cli

    import memory_bench.memory as memory_pkg
    import memory_bench.modes as modes_pkg
    from icm_coding import PROVIDER_NAME, IcmCodingMode, IcmCodingProvider

    memory_pkg.REGISTRY[PROVIDER_NAME] = IcmCodingProvider
    modes_pkg.REGISTRY["coding"] = IcmCodingMode
    return cli


def main() -> None:
    wants_help = any(a in ("-h", "--help") for a in sys.argv[1:]) or len(sys.argv) == 1
    if not wants_help and sys.argv[1:2] == ["run"]:
        preflight()
        if _is_coding_run(sys.argv[2:]):
            coding_mode_env()
    if not os.environ.get("AMB_HOME"):
        raise SystemExit("AMB_HOME is not set")
    cli = bootstrap()
    cli.app()


if __name__ == "__main__":
    main()
