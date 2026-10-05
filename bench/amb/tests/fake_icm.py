#!/usr/bin/env python3
"""Stand-in for the `icm` binary in the offline tests: no model, no embedding, no real store.

Speaks the part of the CLI and of `icm serve --http` that icm_provider.py uses:

    fake_icm.py --db X --version
    fake_icm.py --db X serve --http 127.0.0.1:PORT [--no-embeddings]
        GET  /health
        POST /store?format=json   {topic, content, created_at?}  -> [{"id", "summary", ...}]
        POST /recall?format=json  {query, limit, engine?, now?}  -> [{"id", "summary"}, ...]

Like the real server it refuses a memory over 64 KiB and an unknown `engine`
(HTTP 400, which is how the provider recognises a build with the v2 fields).
Two switches, read from the environment, make it another server:

    FAKE_ICM_NO_ENGINE_FIELD=1  a build older than v2: unknown JSON fields are dropped,
                                so `engine`, `now` and `created_at` are accepted and ignored
    FAKE_ICM_FOREIGN_ID=1       /recall also returns a memory nobody stored here
Ranking: number of distinct query words present in the memory, ties in storage
order; a memory sharing no word is not returned. Every request body is appended to
`X.store.jsonl` / `X.recall.jsonl` so a test can read what was sent.
"""
import json
import os
import re
import signal
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

MAX_CONTENT = 64 * 1024
WORD = re.compile(r"[a-z0-9]+")


def main() -> None:
    argv = sys.argv[1:]
    db = Path(argv[argv.index("--db") + 1])
    if "--version" in argv:
        print("icm 0.0.0-fake")
        return
    host, _, port = argv[argv.index("--http") + 1].partition(":")
    memories: list[dict] = []

    def log(kind: str, body: dict) -> None:
        with open(db.with_suffix(f".{kind}.jsonl"), "a") as fh:
            fh.write(json.dumps(body) + "\n")

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def reply(self, status: int, payload) -> None:
            data = (payload if isinstance(payload, str) else json.dumps(payload)).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def do_GET(self):
            self.reply(200 if self.path.startswith("/health") else 404, {"ok": True})

        def do_POST(self):
            body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            if self.path.startswith("/store"):
                if len(body["content"].encode()) > MAX_CONTENT:
                    return self.reply(400, "content exceeds maximum length")
                log("store", body)
                memory = {"id": f"mem-{len(memories)}", "summary": body["content"], "topic": body["topic"]}
                memories.append(memory)
                return self.reply(200, [memory])
            if self.path.startswith("/recall"):
                old_build = os.environ.get("FAKE_ICM_NO_ENGINE_FIELD") == "1"
                if not old_build and body.get("engine") not in (None, "legacy", "v2"):
                    return self.reply(400, f"unknown engine {body['engine']!r}")
                log("recall", body)
                words = set(WORD.findall(body["query"].lower()))
                scored = [(len(words & set(WORD.findall(m["summary"].lower()))), -i, m) for i, m in enumerate(memories)]
                ranked = [m for score, _, m in sorted(scored, key=lambda s: (s[0], s[1]), reverse=True) if score > 0]
                if not ranked:
                    return self.reply(200, "No memories found.")
                ranked = ranked[: int(body.get("limit", 10))]
                if os.environ.get("FAKE_ICM_FOREIGN_ID") == "1":
                    ranked = [{"id": "mem-from-elsewhere", "summary": "not stored by this run", "topic": "x"}] + ranked
                return self.reply(200, ranked)
            self.reply(404, "not found")

    server = ThreadingHTTPServer((host, int(port)), Handler)
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))
    server.serve_forever()


if __name__ == "__main__":
    main()
