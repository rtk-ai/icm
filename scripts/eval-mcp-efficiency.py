#!/usr/bin/env python3
"""Compare MCP latency and wire-token estimates across ICM binaries.

The eval keeps every scenario on a warm, long-lived stdio server and reports
median/p95 request latency plus compact JSON request/response sizes. Token
counts are deliberately labelled estimates: they use the same documented ICM
heuristic as wake-up/context snapshots (roughly four UTF-8 bytes per token).

Example:
  python3 scripts/eval-mcp-efficiency.py \
    --binary develop=/tmp/icm-develop \
    --binary original-prs=/tmp/icm-original \
    --binary reviewed=/tmp/icm-reviewed \
    --iterations 100 \
    --output docs/evals/mcp-stack.md
"""

from __future__ import annotations

import argparse
import json
import math
import os
import re
import statistics
import subprocess
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable

MODERN_VERSION = "2026-07-28"
LEGACY_VERSION = "2024-11-05"


def compact_json(value: Any) -> str:
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"))


def estimated_tokens(byte_count: int) -> int:
    return math.ceil(byte_count / 4)


def percentile(values: list[float], fraction: float) -> float:
    ordered = sorted(values)
    index = math.ceil(fraction * len(ordered)) - 1
    return ordered[max(0, index)]


def modern_meta() -> dict[str, Any]:
    return {
        "_meta": {
            "io.modelcontextprotocol/protocolVersion": MODERN_VERSION,
            "io.modelcontextprotocol/clientCapabilities": {},
        }
    }


def rpc(request_id: int, method: str, params: dict[str, Any]) -> dict[str, Any]:
    return {
        "jsonrpc": "2.0",
        "id": request_id,
        "method": method,
        "params": params,
    }


@dataclass(frozen=True)
class Binary:
    label: str
    path: Path


@dataclass(frozen=True)
class Scenario:
    name: str
    era: str
    request: Callable[[int], dict[str, Any]]
    supported: Callable[[dict[str, Any]], bool]
    seed_recall: bool = False
    seed_feedback: bool = False


@dataclass
class Measurement:
    binary: str
    scenario: str
    supported: bool
    iterations: int = 0
    median_ms: float = 0
    p95_ms: float = 0
    request_bytes: int = 0
    response_bytes: int = 0
    estimated_wire_tokens: int = 0
    error: str | None = None


@dataclass
class QualityMeasurement:
    binary: str
    era: str
    supported: bool
    queries: int = 0
    hit_at_3: float = 0
    recall_at_3: float = 0
    ndcg_at_3: float = 0
    structured_parity: float | None = None
    field_coverage: float | None = None
    error: str | None = None


QUALITY_MEMORIES = [
    (
        "QUALITY_AUTH_PRIMARY",
        "OAuth token refresh fails after key rotation. Refresh the signing-key cache before retrying.",
    ),
    (
        "QUALITY_AUTH_SECONDARY",
        "OAuth token refresh must retry once after a signing key rotation.",
    ),
    (
        "QUALITY_BILLING_PRIMARY",
        "Billing webhook idempotency uses the provider event identifier as the unique key.",
    ),
    (
        "QUALITY_BILLING_SECONDARY",
        "Billing webhook idempotency prevents duplicate invoice processing.",
    ),
    (
        "QUALITY_DISTRACTOR_UI",
        "The dashboard navigation uses a compact sidebar on small screens.",
    ),
    (
        "QUALITY_DISTRACTOR_CACHE",
        "Static image cache entries expire after twenty four hours.",
    ),
]

QUALITY_QUERIES = [
    ("oauth token refresh", {"QUALITY_AUTH_PRIMARY", "QUALITY_AUTH_SECONDARY"}),
    (
        "billing webhook idempotency",
        {"QUALITY_BILLING_PRIMARY", "QUALITY_BILLING_SECONDARY"},
    ),
]

QUALITY_TAG = re.compile(r"QUALITY_[A-Z_]+")
REQUIRED_MEMORY_FIELDS = {
    "id",
    "topic",
    "summary",
    "importance",
    "weight",
    "score",
}


class McpServer:
    def __init__(
        self,
        binary: Binary,
        cwd: Path,
        seed_recall: bool = False,
        quality_corpus: bool = False,
    ) -> None:
        self._tempdir = tempfile.TemporaryDirectory(prefix="icm-mcp-eval-")
        db = Path(self._tempdir.name) / "memories.db"
        seeds: list[tuple[str, str]] = []
        if seed_recall:
            content = "EVAL_MARKER " + ("structured memory payload " * 60)
            seeds.append(("context-icm", content))
        if quality_corpus:
            seeds.extend(
                ("eval-quality", f"{tag} {content}")
                for tag, content in QUALITY_MEMORIES
            )

        for topic, content in seeds:
            seed = subprocess.run(
                [
                    str(binary.path),
                    "--db",
                    str(db),
                    "--no-embeddings",
                    "store",
                    "--topic",
                    topic,
                    "--content",
                    content,
                    "--importance",
                    "high",
                ],
                cwd=cwd,
                text=True,
                capture_output=True,
                timeout=30,
                check=False,
            )
            if seed.returncode != 0:
                raise RuntimeError(f"seed failed: {seed.stderr.strip()}")

        self._process = subprocess.Popen(
            [
                str(binary.path),
                "--db",
                str(db),
                "--no-embeddings",
                "serve",
            ],
            cwd=cwd,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            encoding="utf-8",
            bufsize=1,
        )

    def request(self, value: dict[str, Any]) -> tuple[dict[str, Any], float, int, int]:
        assert self._process.stdin is not None
        assert self._process.stdout is not None
        encoded = compact_json(value)
        started = time.perf_counter_ns()
        self._process.stdin.write(encoded + "\n")
        self._process.stdin.flush()
        response_line = self._process.stdout.readline()
        elapsed_ms = (time.perf_counter_ns() - started) / 1_000_000
        if not response_line:
            code = self._process.poll()
            raise RuntimeError(f"server closed stdout (exit={code})")
        response = json.loads(response_line)
        return response, elapsed_ms, len(encoded.encode()), len(response_line.rstrip().encode())

    def close(self) -> None:
        if self._process.stdin is not None:
            self._process.stdin.close()
        try:
            self._process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self._process.terminate()
            self._process.wait(timeout=5)
        self._tempdir.cleanup()


def scenarios() -> list[Scenario]:
    def result(response: dict[str, Any]) -> dict[str, Any]:
        value = response.get("result")
        return value if isinstance(value, dict) else {}

    def structured(response: dict[str, Any]) -> dict[str, Any]:
        value = result(response).get("structuredContent")
        if not isinstance(value, dict):
            return {}
        data = value.get("data")
        return data if isinstance(data, dict) else value

    return [
        Scenario(
            "legacy tools/list",
            "legacy",
            lambda i: rpc(i, "tools/list", {}),
            lambda response: isinstance(result(response).get("tools"), list),
        ),
        Scenario(
            "legacy stats",
            "legacy",
            lambda i: rpc(
                i,
                "tools/call",
                {"name": "icm_memory_stats", "arguments": {}},
            ),
            lambda response: isinstance(result(response).get("content"), list),
        ),
        Scenario(
            "modern tools/list",
            "modern",
            lambda i: rpc(i, "tools/list", modern_meta()),
            lambda response: result(response).get("resultType") == "complete"
            and any("annotations" in tool for tool in result(response).get("tools", [])),
        ),
        Scenario(
            "modern stats",
            "modern",
            lambda i: rpc(
                i,
                "tools/call",
                {
                    "name": "icm_memory_stats",
                    "arguments": {},
                    **modern_meta(),
                },
            ),
            lambda response: "structuredContent" in result(response),
        ),
        Scenario(
            "modern recall",
            "modern",
            lambda i: rpc(
                i,
                "tools/call",
                {
                    "name": "icm_memory_recall",
                    "arguments": {
                        "query": "EVAL_MARKER",
                        "project": "",
                        "limit": 1,
                    },
                    **modern_meta(),
                },
            ),
            lambda response: "structuredContent" in result(response)
            and structured(response).get("count") == 1,
            seed_recall=True,
        ),
        Scenario(
            "modern feedback search",
            "modern",
            lambda i: rpc(
                i,
                "tools/call",
                {
                    "name": "icm_feedback_search",
                    "arguments": {"query": "FEEDBACK_EVAL_MARKER", "limit": 1},
                    **modern_meta(),
                },
            ),
            lambda response: "structuredContent" in result(response),
            seed_feedback=True,
        ),
        Scenario(
            "modern transcript stats",
            "modern",
            lambda i: rpc(
                i,
                "tools/call",
                {
                    "name": "icm_transcript_stats",
                    "arguments": {},
                    **modern_meta(),
                },
            ),
            lambda response: "structuredContent" in result(response),
        ),
        Scenario(
            "resources/list",
            "modern",
            lambda i: rpc(i, "resources/list", modern_meta()),
            lambda response: isinstance(result(response).get("resources"), list),
        ),
        Scenario(
            "resources/read current",
            "modern",
            lambda i: rpc(
                i,
                "resources/read",
                {"uri": "icm://context/current", **modern_meta()},
            ),
            lambda response: isinstance(result(response).get("contents"), list),
        ),
    ]


def initialize_legacy(server: McpServer) -> None:
    response, _, _, _ = server.request(
        rpc(
            0,
            "initialize",
            {
                "protocolVersion": LEGACY_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "icm-efficiency-eval", "version": "1"},
            },
        )
    )
    if "error" in response:
        raise RuntimeError(f"legacy initialize failed: {compact_json(response)}")


def measure(
    binary: Binary,
    scenario: Scenario,
    cwd: Path,
    warmup: int,
    iterations: int,
) -> Measurement:
    server = McpServer(binary, cwd, scenario.seed_recall)
    try:
        if scenario.era == "legacy":
            initialize_legacy(server)
        if scenario.seed_feedback:
            seeded, _, _, _ = server.request(
                rpc(
                    -1,
                    "tools/call",
                    {
                        "name": "icm_feedback_record",
                        "arguments": {
                            "topic": "eval-feedback",
                            "context": "FEEDBACK_EVAL_MARKER " + "context " * 30,
                            "predicted": "old prediction " * 20,
                            "corrected": "corrected prediction " * 20,
                            "reason": "structured output evaluation",
                            "source": "mcp-eval",
                        },
                        **modern_meta(),
                    },
                )
            )
            if "error" in seeded:
                raise RuntimeError(
                    f"feedback seed failed: {compact_json(seeded)}"
                )

        probe, _, _, _ = server.request(scenario.request(1))
        if not scenario.supported(probe):
            return Measurement(binary.label, scenario.name, False)

        for i in range(warmup):
            server.request(scenario.request(10 + i))

        latencies: list[float] = []
        request_sizes: list[int] = []
        response_sizes: list[int] = []
        for i in range(iterations):
            _, elapsed, request_bytes, response_bytes = server.request(
                scenario.request(1_000 + i)
            )
            latencies.append(elapsed)
            request_sizes.append(request_bytes)
            response_sizes.append(response_bytes)

        request_bytes = round(statistics.median(request_sizes))
        response_bytes = round(statistics.median(response_sizes))
        return Measurement(
            binary=binary.label,
            scenario=scenario.name,
            supported=True,
            iterations=iterations,
            median_ms=statistics.median(latencies),
            p95_ms=percentile(latencies, 0.95),
            request_bytes=request_bytes,
            response_bytes=response_bytes,
            estimated_wire_tokens=estimated_tokens(request_bytes + response_bytes),
        )
    except Exception as error:  # keep the remaining matrix useful
        return Measurement(binary.label, scenario.name, False, error=str(error))
    finally:
        server.close()


def result_object(response: dict[str, Any]) -> dict[str, Any]:
    result = response.get("result")
    return result if isinstance(result, dict) else {}


def result_text(response: dict[str, Any]) -> str:
    content = result_object(response).get("content")
    if not isinstance(content, list):
        return ""
    return "\n".join(
        item.get("text", "")
        for item in content
        if isinstance(item, dict) and isinstance(item.get("text"), str)
    )


def ordered_tags(text: str) -> list[str]:
    return list(dict.fromkeys(QUALITY_TAG.findall(text)))


def quality_request(request_id: int, query: str, modern: bool) -> dict[str, Any]:
    params: dict[str, Any] = {
        "name": "icm_memory_recall",
        "arguments": {"query": query, "project": "", "limit": 3},
    }
    if modern:
        params.update(modern_meta())
    return rpc(request_id, "tools/call", params)


def ndcg_at_3(tags: list[str], relevant: set[str]) -> float:
    gains = [1.0 if tag in relevant else 0.0 for tag in tags[:3]]
    dcg = sum(gain / math.log2(rank + 2) for rank, gain in enumerate(gains))
    ideal_count = min(3, len(relevant))
    ideal = sum(1.0 / math.log2(rank + 2) for rank in range(ideal_count))
    return dcg / ideal if ideal else 1.0


def measure_quality(binary: Binary, cwd: Path, modern: bool) -> QualityMeasurement:
    era = "modern" if modern else "legacy"
    server = McpServer(binary, cwd, quality_corpus=True)
    legacy_server = McpServer(binary, cwd, quality_corpus=True) if modern else server
    try:
        initialize_legacy(legacy_server)

        hits: list[float] = []
        recalls: list[float] = []
        ndcgs: list[float] = []
        parities: list[float] = []
        field_coverages: list[float] = []

        for index, (query, relevant) in enumerate(QUALITY_QUERIES):
            legacy, _, _, _ = legacy_server.request(
                quality_request(10 + index * 2, query, False)
            )
            legacy_tags = ordered_tags(result_text(legacy))
            if "error" in legacy or not legacy_tags:
                raise RuntimeError(
                    f"legacy recall did not return tagged results for {query!r}"
                )

            response = legacy
            tags = legacy_tags
            if modern:
                response, _, _, _ = server.request(
                    quality_request(11 + index * 2, query, True)
                )
                structured = result_object(response).get("structuredContent")
                if not isinstance(structured, dict):
                    return QualityMeasurement(binary.label, era, False)
                payload = structured.get("data", structured)
                memories = payload.get("memories") if isinstance(payload, dict) else None
                if not isinstance(memories, list):
                    return QualityMeasurement(binary.label, era, False)
                tags = ordered_tags(
                    "\n".join(
                        compact_json(memory)
                        for memory in memories
                        if isinstance(memory, dict)
                    )
                )
                parities.append(1.0 if tags == legacy_tags else 0.0)
                for memory in memories:
                    if isinstance(memory, dict):
                        present = sum(
                            1 for field in REQUIRED_MEMORY_FIELDS if field in memory
                        )
                        field_coverages.append(present / len(REQUIRED_MEMORY_FIELDS))

            relevant_returned = len(set(tags[:3]) & relevant)
            hits.append(1.0 if relevant_returned else 0.0)
            recalls.append(relevant_returned / len(relevant))
            ndcgs.append(ndcg_at_3(tags, relevant))

        return QualityMeasurement(
            binary=binary.label,
            era=era,
            supported=True,
            queries=len(QUALITY_QUERIES),
            hit_at_3=statistics.mean(hits),
            recall_at_3=statistics.mean(recalls),
            ndcg_at_3=statistics.mean(ndcgs),
            structured_parity=statistics.mean(parities) if parities else None,
            field_coverage=(
                statistics.mean(field_coverages) if field_coverages else None
            ),
        )
    except Exception as error:
        return QualityMeasurement(binary.label, era, False, error=str(error))
    finally:
        server.close()
        if legacy_server is not server:
            legacy_server.close()


def markdown(
    binaries: list[Binary],
    measurements: list[Measurement],
    quality: list[QualityMeasurement],
    warmup: int,
    iterations: int,
) -> str:
    lines = [
        "# MCP efficiency evaluation",
        "",
        f"- Warm-up requests per scenario: {warmup}",
        f"- Measured requests per scenario: {iterations}",
        "- Latency: client-observed stdio request/response time on a warm server",
        "- Token estimate: compact JSON request + response bytes divided by four",
        "- Token estimates are comparative wire-size proxies, not provider billing data",
        "",
        "| Binary | Scenario | Median ms | p95 ms | Request bytes | Response bytes | Est. wire tokens |",
        "| --- | --- | ---: | ---: | ---: | ---: | ---: |",
    ]
    for measurement in measurements:
        if not measurement.supported:
            note = f"unsupported ({measurement.error})" if measurement.error else "unsupported"
            lines.append(
                f"| {measurement.binary} | {measurement.scenario} | {note} | — | — | — | — |"
            )
            continue
        lines.append(
            "| "
            + " | ".join(
                [
                    measurement.binary,
                    measurement.scenario,
                    f"{measurement.median_ms:.3f}",
                    f"{measurement.p95_ms:.3f}",
                    str(measurement.request_bytes),
                    str(measurement.response_bytes),
                    str(measurement.estimated_wire_tokens),
                ]
            )
            + " |"
        )

    lines.extend(["", "## Pairwise changes", ""])
    by_key = {(m.binary, m.scenario): m for m in measurements}
    for earlier, later in zip(binaries, binaries[1:]):
        lines.append(f"### {earlier.label} → {later.label}")
        lines.append("")
        lines.append("| Scenario | Median latency | Response bytes | Est. wire tokens |")
        lines.append("| --- | ---: | ---: | ---: |")
        comparable = False
        for scenario in scenarios():
            before = by_key[(earlier.label, scenario.name)]
            after = by_key[(later.label, scenario.name)]
            if not before.supported or not after.supported:
                continue
            comparable = True

            def delta(new: float, old: float, unit: str = "") -> str:
                if old == 0:
                    return "—"
                change = (new - old) / old * 100
                return f"{change:+.1f}%{unit}"

            lines.append(
                f"| {scenario.name} | "
                f"{delta(after.median_ms, before.median_ms)} | "
                f"{delta(after.response_bytes, before.response_bytes)} | "
                f"{delta(after.estimated_wire_tokens, before.estimated_wire_tokens)} |"
            )
        if not comparable:
            lines.append("| — | No common supported scenarios | — | — |")
        lines.append("")

    lines.extend(
        [
            "## Recall relevance and output quality",
            "",
            "The deterministic corpus contains two relevant memories for each of two",
            "queries plus unrelated distractors. Modern output is compared with a",
            "legacy recall from the same binary; this tests transport fidelity, not",
            "subjective answer style.",
            "",
            "| Binary | Era | Hit@3 | Recall@3 | nDCG@3 | Structured/legacy order parity | Required field coverage |",
            "| --- | --- | ---: | ---: | ---: | ---: | ---: |",
        ]
    )
    for measurement in quality:
        if not measurement.supported:
            note = f"unsupported ({measurement.error})" if measurement.error else "unsupported"
            lines.append(
                f"| {measurement.binary} | {measurement.era} | {note} | — | — | — | — |"
            )
            continue

        def percent(value: float | None) -> str:
            return "—" if value is None else f"{value * 100:.1f}%"

        lines.append(
            f"| {measurement.binary} | {measurement.era} | "
            f"{percent(measurement.hit_at_3)} | "
            f"{percent(measurement.recall_at_3)} | "
            f"{measurement.ndcg_at_3:.3f} | "
            f"{percent(measurement.structured_parity)} | "
            f"{percent(measurement.field_coverage)} |"
        )
    lines.extend(
        [
            "",
            "A 100% structured/legacy parity score means the same memory markers",
            "appear in the same order. Required-field coverage checks `id`, `topic`,",
            "`summary`, `importance`, `weight`, and `score` on every structured hit.",
            "",
        ]
    )
    if len(binaries) >= 2:
        before = by_key.get((binaries[-2].label, "modern recall"))
        after = by_key.get((binaries[-1].label, "modern recall"))
        before_quality = next(
            (
                item
                for item in quality
                if item.binary == binaries[-2].label
                and item.era == "modern"
                and item.supported
            ),
            None,
        )
        after_quality = next(
            (
                item
                for item in quality
                if item.binary == binaries[-1].label
                and item.era == "modern"
                and item.supported
            ),
            None,
        )
        if (
            before
            and after
            and before.supported
            and after.supported
            and before_quality
            and after_quality
        ):
            token_change = (
                (after.estimated_wire_tokens - before.estimated_wire_tokens)
                / before.estimated_wire_tokens
                * 100
            )
            quality_preserved = (
                before_quality.recall_at_3 == after_quality.recall_at_3
                and before_quality.ndcg_at_3 == after_quality.ndcg_at_3
                and after_quality.structured_parity == 1.0
                and after_quality.field_coverage == 1.0
            )
            lines.extend(
                [
                    "## Interpretation",
                    "",
                    f"- Modern recall wire-token estimate changed {token_change:+.1f}% "
                    f"from `{binaries[-2].label}` to `{binaries[-1].label}`.",
                    "- Retrieval relevance and structured-output fidelity were "
                    + ("preserved." if quality_preserved else "not fully preserved."),
                    "- This protocol work does not change ICM's ranking algorithm, so "
                    "it should not be described as a retrieval-quality improvement.",
                    "- Sub-millisecond scenarios are sensitive to host scheduling; "
                    "latency deltas are directional observations, not a performance "
                    "guarantee.",
                    "",
                ]
            )
    return "\n".join(lines).rstrip() + "\n"


def parse_binary(raw: str) -> Binary:
    label, separator, path = raw.partition("=")
    if not separator or not label or not path:
        raise argparse.ArgumentTypeError("--binary must use LABEL=PATH")
    resolved = Path(path).expanduser().resolve()
    if not resolved.is_file() or not os.access(resolved, os.X_OK):
        raise argparse.ArgumentTypeError(f"binary is not executable: {resolved}")
    return Binary(label, resolved)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--binary",
        action="append",
        type=parse_binary,
        required=True,
        help="label and executable path as LABEL=PATH; repeat to compare builds",
    )
    parser.add_argument("--warmup", type=int, default=10)
    parser.add_argument("--iterations", type=int, default=100)
    parser.add_argument("--output", type=Path)
    parser.add_argument(
        "--cwd",
        type=Path,
        default=Path.cwd(),
        help="working directory used by each MCP server (default: current directory)",
    )
    args = parser.parse_args()
    if args.warmup < 0 or args.iterations < 1:
        parser.error("--warmup must be >= 0 and --iterations must be >= 1")
    labels = [binary.label for binary in args.binary]
    if len(labels) != len(set(labels)):
        parser.error("--binary labels must be unique")

    measurements = [
        measure(binary, scenario, args.cwd.resolve(), args.warmup, args.iterations)
        for binary in args.binary
        for scenario in scenarios()
    ]
    quality = [
        measure_quality(binary, args.cwd.resolve(), modern)
        for binary in args.binary
        for modern in (False, True)
    ]
    report = markdown(
        args.binary, measurements, quality, args.warmup, args.iterations
    )
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(report, encoding="utf-8")
    print(report, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
