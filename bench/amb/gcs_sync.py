#!/usr/bin/env python3
"""Copy benchmark result files between a local directory and a remote prefix.

    gcs_sync.py down  <remote> <local_dir>                 restore a previous attempt (pod start)
    gcs_sync.py up    <local_dir> <remote>                 one pass (pod end)
    gcs_sync.py watch <local_dir> <remote> [--interval S]  one pass each time a .json file changes
    gcs_sync.py check <result.json>                        exit 0 when the file is a readable result

<remote> is `gs://bucket/prefix` (Application Default Credentials, i.e. Workload
Identity on GKE) or a plain directory (tests and dry runs, no network).

Only result files travel (*.json, *.jsonl, *.log); the per-unit SQLite databases
under `_store/` stay on the pod, except the provider's `ingest-stats.json`.

What `up` and `watch` guarantee, because every answer in these files was paid for:

* Only whole files. A `.json` is parsed from the very bytes that are sent, so a
  file caught while the harness rewrites it is skipped and picked up by the next
  pass; a `.jsonl` is cut at its last complete line.
* Never a less complete file over a more complete one. A harness result file
  (JSON with a `results` list keyed by `query_id`) replaces the remote object only
  when it holds every remote `query_id`; an append-only file (`.jsonl`, `.log`)
  only when it is not shorter. The write is conditional on the remote generation
  that was compared, so two pods cannot race past the check.

Exit status of `up`: 0 everything is on the remote; 2 a file was refused (the
remote copy is more complete and was left untouched; the local bytes are stored
next to it as `<name>.rejected-<host>-<time>`); 3 a `.json` was not valid JSON.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import socket
import sys
import time
from pathlib import Path

_SUFFIXES = (".json", ".jsonl", ".log")
_KEPT_IN_STORE = ("ingest-stats.json",)
_REJECTED = ".rejected-"
_TIMEOUT_S = 300


class RemoteChanged(Exception):
    """The remote object is not the generation the decision was based on."""


class _DirRemote:
    """A directory standing in for a bucket prefix. Generation = content digest."""

    def __init__(self, root: str):
        self._root = Path(root)

    def _generation(self, path: Path) -> str | None:
        return hashlib.sha256(path.read_bytes()).hexdigest() if path.is_file() else None

    def list(self) -> list[str]:
        if not self._root.is_dir():
            return []
        return sorted(p.relative_to(self._root).as_posix() for p in self._root.rglob("*") if p.is_file())

    def stat(self, name: str) -> tuple[int, str] | None:
        path = self._root / name
        return (path.stat().st_size, self._generation(path)) if path.is_file() else None

    def read(self, name: str) -> tuple[bytes, str] | None:
        path = self._root / name
        if not path.is_file():
            return None
        data = path.read_bytes()
        return data, hashlib.sha256(data).hexdigest()

    def write(self, name: str, data: bytes, expect: str | None) -> None:
        path = self._root / name
        if self._generation(path) != expect:
            raise RemoteChanged(name)
        path.parent.mkdir(parents=True, exist_ok=True)
        tmp = path.with_name(path.name + ".part")
        tmp.write_bytes(data)
        os.replace(tmp, path)


class _GcsRemote:
    """A `gs://bucket/prefix`. Objects appear whole or not at all."""

    def __init__(self, url: str):
        from google.cloud import storage

        bucket, _, prefix = url[5:].partition("/")
        if not bucket:
            sys.exit(f"not a gs://bucket/prefix URL: {url}")
        self._client = storage.Client()
        self._bucket = self._client.bucket(bucket)
        self._prefix = prefix.strip("/")

    def _key(self, name: str) -> str:
        return f"{self._prefix}/{name}" if self._prefix else name

    def list(self) -> list[str]:
        start = f"{self._prefix}/" if self._prefix else ""
        return [blob.name[len(start):] for blob in self._client.list_blobs(self._bucket, prefix=start)
                if not blob.name.endswith("/")]

    def stat(self, name: str) -> tuple[int, int] | None:
        blob = self._bucket.get_blob(self._key(name), timeout=_TIMEOUT_S)
        return None if blob is None else (blob.size, blob.generation)

    def read(self, name: str) -> tuple[bytes, int] | None:
        from google.api_core.exceptions import NotFound

        blob = self._bucket.get_blob(self._key(name), timeout=_TIMEOUT_S)
        if blob is None:
            return None
        try:
            # get_blob() pins the generation: these are the bytes of that generation.
            return blob.download_as_bytes(timeout=_TIMEOUT_S), blob.generation
        except NotFound as e:  # replaced between the two calls
            raise RemoteChanged(name) from e

    def write(self, name: str, data: bytes, expect: int | None) -> None:
        from google.api_core.exceptions import PreconditionFailed

        content_type = "application/json" if name.endswith(".json") else "text/plain"
        try:
            # if_generation_match=0 means "only when the object does not exist yet".
            self._bucket.blob(self._key(name)).upload_from_string(
                data, content_type=content_type, if_generation_match=expect or 0, timeout=_TIMEOUT_S)
        except PreconditionFailed as e:
            raise RemoteChanged(name) from e


def _remote(url: str):
    return _GcsRemote(url) if url.startswith("gs://") else _DirRemote(url)


def result_ids(data: bytes) -> set[str] | None:
    """Query ids of a harness result file; None for any other JSON. ValueError when not JSON."""
    doc = json.loads(data)
    if isinstance(doc, dict) and isinstance(doc.get("results"), list):
        return {str(row.get("query_id")) for row in doc["results"] if isinstance(row, dict)}
    return None


def _travels(path: Path, local: Path) -> bool:
    if not path.is_file() or path.suffix not in _SUFFIXES:
        return False
    return "_store" not in path.relative_to(local).parts or path.name in _KEPT_IN_STORE


def _signature(path: Path) -> tuple[int, int]:
    st = path.stat()
    return st.st_mtime_ns, st.st_size


def _push_one(name: str, data: bytes, remote) -> tuple[str, str]:
    """Send one file under the rules of the module docstring. Returns (outcome, detail)."""
    if name.endswith(".json"):
        try:
            ids = result_ids(data)
        except ValueError:
            return "invalid", "not valid JSON (being written?)"
        if ids is None:
            current = remote.stat(name)
            remote.write(name, data, current[1] if current else None)
            return "sent", "json"
        current = remote.read(name)
        if current is None:
            remote.write(name, data, None)
            return "sent", f"{len(ids)} rows (new)"
        remote_data, generation = current
        if remote_data == data:
            return "same", f"{len(ids)} rows"
        try:
            remote_ids = result_ids(remote_data) or set()
        except ValueError:
            remote_ids = set()  # an unreadable remote copy holds nothing worth keeping
        missing = remote_ids - ids
        if missing:
            return "refused", (f"remote has {len(remote_ids)} rows, local {len(ids)}: {len(missing)} remote "
                               f"rows are not in the local file (e.g. {sorted(missing)[:3]})")
        remote.write(name, data, generation)
        return "sent", f"{len(ids)} rows (remote had {len(remote_ids)})"

    data = data[:data.rfind(b"\n") + 1]  # append-only: whole lines only
    current = remote.stat(name)
    if current is not None and current[0] > len(data):
        return "refused", f"remote is longer ({current[0]} bytes) than local ({len(data)} bytes)"
    if current is not None and current[0] == len(data):
        return "same", f"{len(data)} bytes"
    remote.write(name, data, current[1] if current else None)
    return "sent", f"{len(data)} bytes"


def push(local: Path, remote, seen: dict[str, tuple[int, int]] | None = None, keep_rejected: bool = False) -> dict:
    """One upload pass. `seen` (watch mode) skips the files already dealt with at this size and date."""
    counts = {"sent": 0, "same": 0, "refused": 0, "invalid": 0}
    for path in sorted(local.rglob("*")):
        if not _travels(path, local):
            continue
        name = path.relative_to(local).as_posix()
        signature = _signature(path)
        if seen is not None and seen.get(name) == signature:
            continue
        data = path.read_bytes()
        outcome, detail = "", ""
        for _ in range(3):  # the remote moved under us: decide again on the new remote
            try:
                outcome, detail = _push_one(name, data, remote)
                break
            except RemoteChanged:
                outcome, detail = "refused", "the remote object kept changing during the upload"
        counts[outcome] += 1
        if outcome in ("refused", "invalid"):
            print(f"[gcs_sync] {outcome.upper()} {name}: {detail}", flush=True)
        if outcome == "refused" and keep_rejected:
            side = f"{name}{_REJECTED}{socket.gethostname()}-{int(time.time())}"
            remote.write(side, data, None)
            print(f"[gcs_sync] local copy kept on the remote as {side}", flush=True)
        if seen is not None and outcome != "invalid":
            seen[name] = signature
    return counts


def up(local: Path, url: str) -> int:
    counts = push(local, _remote(url), keep_rejected=True)
    print(f"[gcs_sync] up {url}: {counts['sent']} sent, {counts['same']} unchanged, "
          f"{counts['refused']} refused, {counts['invalid']} invalid", flush=True)
    return 2 if counts["refused"] else 3 if counts["invalid"] else 0


def watch(local: Path, url: str, interval: float) -> None:
    """Upload each time a .json file (a result save) changes, until killed.

    Errors never end the loop: the entrypoint's final `up` is the safety net."""
    remote = None
    seen: dict[str, tuple[int, int]] = {}
    while True:
        time.sleep(interval)
        try:
            changed = [p for p in local.rglob("*.json")
                       if _travels(p, local) and seen.get(p.relative_to(local).as_posix()) != _signature(p)]
            if not changed:
                continue
            remote = remote or _remote(url)
            counts = push(local, remote, seen)
            print(f"[gcs_sync] watch {url}: {counts['sent']} sent, {counts['refused']} refused, "
                  f"{counts['invalid']} invalid", flush=True)
        except Exception as e:  # noqa: BLE001 - keep watching whatever the storage error
            print(f"[gcs_sync] watch pass failed, will retry: {e!r}", flush=True)
            remote = None


def down(url: str, local: Path) -> None:
    remote = _remote(url)
    count = 0
    for name in remote.list():
        if _REJECTED in name:
            continue
        found = remote.read(name)
        if found is None:
            continue
        target = local / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(found[0])
        count += 1
    print(f"[gcs_sync] downloaded {count} files from {url}", flush=True)


def check(path: Path) -> int:
    try:
        ids = result_ids(path.read_bytes())
    except (OSError, ValueError) as e:
        print(f"[gcs_sync] {path} is not a readable result file: {e}", flush=True)
        return 1
    if ids is None:
        print(f"[gcs_sync] {path} is JSON but not a harness result file (no `results` list)", flush=True)
        return 1
    print(f"[gcs_sync] {path}: {len(ids)} result rows", flush=True)
    return 0


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="command", required=True)
    for name in ("up", "watch"):
        p = sub.add_parser(name)
        p.add_argument("local", type=Path)
        p.add_argument("remote")
        if name == "watch":
            p.add_argument("--interval", type=float, default=15.0)
    p = sub.add_parser("down")
    p.add_argument("remote")
    p.add_argument("local", type=Path)
    p = sub.add_parser("check")
    p.add_argument("path", type=Path)
    args = ap.parse_args()

    if args.command == "up":
        sys.exit(up(args.local, args.remote))
    if args.command == "watch":
        watch(args.local, args.remote, args.interval)
    elif args.command == "down":
        down(args.remote, args.local)
    else:
        sys.exit(check(args.path))


if __name__ == "__main__":
    main()
