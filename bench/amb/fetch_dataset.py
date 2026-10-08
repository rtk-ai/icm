#!/usr/bin/env python3
"""Fetch a dataset file once, with retries and resume, where the harness tries once.

    python fetch_dataset.py longmemeval /work/datasets/longmemeval_s_cleaned.json
    export LONGMEMEVAL_DATA_PATH=/work/datasets/longmemeval_s_cleaned.json

The harness downloads LongMemEval-S (277 MB, Hugging Face) with a single
`urllib.request.urlretrieve`: no retry, no resume, and a connection cut half-way
leaves a truncated file that the next start reads as the dataset. In a sharded run
every pod does it. This script is what the images run at build time and what
entrypoint.sh falls back to when the file is not in the image:

* up to --attempts tries (default 6), waiting 5, 10, 20... seconds between them;
* a try that fails half-way is resumed with an HTTP Range request when the server
  allows it, restarted otherwise;
* the bytes go to `<dest>.part`; `<dest>` appears only once the size matches the
  Content-Length the server announced and the content starts and ends as a JSON
  array, so a file that exists is a whole file;
* an existing, whole `<dest>` is kept: nothing is fetched.

Exit 0 when `<dest>` is in place, 1 otherwise. Only the standard library.

Environment: ICM_AMB_DATASET_URL replaces the source (a mirror, a bucket's public
URL), as `--url` does; ICM_AMB_FETCH_BACKOFF_S is the wait before the second try.
"""

from __future__ import annotations

import argparse
import os
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

URLS = {
    # The URL the harness itself uses (memory_bench/dataset/longmemeval.py, _DATA_URL).
    "longmemeval": "https://huggingface.co/datasets/xiaowu0162/longmemeval-cleaned/resolve/main/longmemeval_s_cleaned.json",
}
_CHUNK = 1 << 20


def looks_whole(path: Path) -> bool:
    """A JSON array from its first byte to its last (cheap: the 277 MB are not parsed)."""
    size = path.stat().st_size
    if size < 2:
        return False
    with open(path, "rb") as fh:
        head = fh.read(64).lstrip()
        fh.seek(max(0, size - 64))
        tail = fh.read().rstrip()
    return head.startswith(b"[") and tail.endswith(b"]")


def _attempt(url: str, part: Path, timeout_s: float) -> None:
    """One try. Appends to `part` when the server honours the Range request. Raises on any failure."""
    have = part.stat().st_size if part.exists() else 0
    request = urllib.request.Request(url, headers={"Range": f"bytes={have}-"} if have else {})
    try:
        response = urllib.request.urlopen(request, timeout=timeout_s)
    except urllib.error.HTTPError as e:
        if e.code == 416 and have:  # nothing left after `have`, or a stale part: start over
            part.unlink()
            raise RuntimeError("HTTP 416 on resume: partial file dropped") from e
        raise
    with response:
        resumed = have > 0 and response.status == 206
        if have and not resumed:
            have = 0  # the server sent the whole file again
        length = response.headers.get("Content-Length")
        expected = have + int(length) if length is not None else None
        with open(part, "ab" if resumed else "wb") as fh:
            while True:
                block = response.read(_CHUNK)
                if not block:
                    break
                fh.write(block)
    size = part.stat().st_size
    if expected is not None and size != expected:
        raise RuntimeError(f"connection closed at {size} of {expected} bytes")


def fetch(url: str, dest: Path, attempts: int = 6, backoff_s: float = 5.0, timeout_s: float = 120.0) -> bool:
    if dest.exists() and looks_whole(dest):
        print(f"[fetch_dataset] {dest} is already there ({dest.stat().st_size} bytes)", flush=True)
        return True
    dest.parent.mkdir(parents=True, exist_ok=True)
    part = dest.with_name(dest.name + ".part")
    delay = backoff_s
    for attempt in range(1, attempts + 1):
        try:
            _attempt(url, part, timeout_s)
            if not looks_whole(part):
                part.unlink()
                raise RuntimeError("the file received is not a JSON array from end to end: dropped")
            os.replace(part, dest)
            print(f"[fetch_dataset] {dest}: {dest.stat().st_size} bytes after {attempt} attempt(s)", flush=True)
            return True
        except (OSError, RuntimeError) as e:  # URLError and timeouts are OSError
            kept = part.stat().st_size if part.exists() else 0
            print(f"[fetch_dataset] attempt {attempt}/{attempts} failed: {type(e).__name__}: {e} "
                  f"({kept} bytes kept)", file=sys.stderr, flush=True)
            if attempt < attempts:
                time.sleep(delay)
                delay = min(delay * 2, 120.0)
    return False


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("dataset", choices=sorted(URLS))
    ap.add_argument("dest", type=Path)
    ap.add_argument("--url", help="another source for the same file (a mirror, a bucket's public URL)")
    ap.add_argument("--attempts", type=int, default=6)
    ap.add_argument("--backoff", type=float, default=float(os.environ.get("ICM_AMB_FETCH_BACKOFF_S") or 5.0),
                    help="seconds before the second try; doubles each time")
    args = ap.parse_args(argv)
    url = args.url or os.environ.get("ICM_AMB_DATASET_URL") or URLS[args.dataset]
    return 0 if fetch(url, args.dest, max(1, args.attempts), args.backoff) else 1


if __name__ == "__main__":
    sys.exit(main())
