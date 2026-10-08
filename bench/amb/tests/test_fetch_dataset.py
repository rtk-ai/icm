#!/usr/bin/env python3
"""fetch_dataset.py against a local server that fails the way a real download does:
refused requests, a connection cut half-way, a body that is not the file. And
entrypoint.sh, which must use the file already there and fetch it otherwise. Offline.

    python tests/test_fetch_dataset.py
"""
import json
import subprocess
import sys
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from _common import BENCH, PY, path_with_python, workdir

sys.path.insert(0, str(BENCH))
import fetch_dataset  # noqa: E402

PAYLOAD = json.dumps([{"question_id": f"q{i}", "text": "x" * 500} for i in range(6000)]).encode()  # about 3 MB


class Flaky:
    """Serves PAYLOAD. `script` says what each successive request gets: 'ok', 'refuse' (HTTP 503),
    'cut' (announces the whole length, sends the first third, closes), 'html' (200 with another body)."""

    def __init__(self, script: list[str], ranges: bool = True):
        self.script, self.ranges, self.requests = list(script), ranges, []
        outer = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_GET(self):
                step = outer.script.pop(0) if outer.script else "ok"
                start = 0
                header = self.headers.get("Range")
                outer.requests.append((step, header))
                if step == "refuse":
                    self.send_error(503)
                    return
                if step == "html":
                    body = b"<html>rate limited</html>"
                    self.send_response(200)
                    self.send_header("Content-Length", str(len(body)))
                    self.end_headers()
                    self.wfile.write(body)
                    return
                if header and outer.ranges:
                    start = int(header.split("=")[1].rstrip("-"))
                    self.send_response(206)
                else:
                    self.send_response(200)
                body = PAYLOAD[start:]
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body[: len(body) // 3] if step == "cut" else body)
                if step == "cut":
                    self.close_connection = True

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.url = f"http://127.0.0.1:{self.server.server_address[1]}/longmemeval_s_cleaned.json"
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def close(self):
        self.server.shutdown()
        self.server.server_close()


class Fetch(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.root = workdir("fetch-dataset")

    def serve(self, script, ranges=True) -> Flaky:
        server = Flaky(script, ranges)
        self.addCleanup(server.close)
        return server

    def test_a_refused_request_and_a_cut_connection_end_with_the_whole_file(self):
        server = self.serve(["refuse", "cut", "cut", "ok"])
        dest = self.root / "a" / "lme.json"
        self.assertTrue(fetch_dataset.fetch(server.url, dest, attempts=6, backoff_s=0))
        self.assertEqual(dest.read_bytes(), PAYLOAD)
        self.assertFalse(dest.with_name("lme.json.part").exists())
        steps = [s for s, _ in server.requests]
        self.assertEqual(steps, ["refuse", "cut", "cut", "ok"])
        # each try after a cut resumes where the last one stopped instead of starting over
        third = len(PAYLOAD) // 3
        self.assertEqual([r for _, r in server.requests],
                         [None, None, f"bytes={third}-", f"bytes={third + (len(PAYLOAD) - third) // 3}-"])

    def test_a_server_without_ranges_is_fetched_again_from_the_start(self):
        server = self.serve(["cut", "ok"], ranges=False)
        dest = self.root / "b" / "lme.json"
        self.assertTrue(fetch_dataset.fetch(server.url, dest, attempts=3, backoff_s=0))
        self.assertEqual(dest.read_bytes(), PAYLOAD)

    def test_nothing_is_left_in_place_when_every_try_fails(self):
        server = self.serve(["refuse"] * 3 + ["ok"])
        dest = self.root / "c" / "lme.json"
        self.assertFalse(fetch_dataset.fetch(server.url, dest, attempts=3, backoff_s=0))
        self.assertFalse(dest.exists(), "a file that exists is a whole file")
        self.assertEqual(len(server.requests), 3)
        # the command line says so with its exit code
        down = self.serve(["refuse"] * 2)
        proc = subprocess.run([PY, str(BENCH / "fetch_dataset.py"), "longmemeval", str(dest), "--url", down.url,
                               "--attempts", "2", "--backoff", "0"], capture_output=True, text=True)
        self.assertEqual(proc.returncode, 1, proc.stdout + proc.stderr)
        self.assertFalse(dest.exists())

    def test_a_body_that_is_not_the_file_is_not_kept(self):
        server = self.serve(["html", "ok"])
        dest = self.root / "d" / "lme.json"
        self.assertTrue(fetch_dataset.fetch(server.url, dest, attempts=2, backoff_s=0))
        self.assertEqual(dest.read_bytes(), PAYLOAD)
        only_html = self.serve(["html", "html"])
        dest = self.root / "e" / "lme.json"
        self.assertFalse(fetch_dataset.fetch(only_html.url, dest, attempts=2, backoff_s=0))
        self.assertFalse(dest.exists())

    def test_a_whole_file_already_there_is_kept_and_a_truncated_one_is_not(self):
        server = self.serve([])
        dest = self.root / "f" / "lme.json"
        dest.parent.mkdir(parents=True)
        dest.write_bytes(PAYLOAD)
        self.assertTrue(fetch_dataset.fetch(server.url, dest, attempts=1, backoff_s=0))
        self.assertEqual(server.requests, [])
        dest.write_bytes(PAYLOAD[: len(PAYLOAD) // 2])  # what a cut urlretrieve leaves behind
        self.assertTrue(fetch_dataset.fetch(server.url, dest, attempts=1, backoff_s=0))
        self.assertEqual(dest.read_bytes(), PAYLOAD)
        self.assertEqual(len(server.requests), 1)

    def test_the_default_source_is_the_one_the_harness_uses(self):
        try:
            from _common import amb_home
            home = amb_home()
        except unittest.SkipTest:
            self.skipTest("no harness checkout: set AMB_HOME")
        source = (home / "src" / "memory_bench" / "dataset" / "longmemeval.py").read_text()
        head, tail = fetch_dataset.URLS["longmemeval"].split("/resolve/")
        self.assertIn(head, source)
        self.assertIn("/resolve/" + tail, source)


class Entrypoint(unittest.TestCase):
    """DATASET=longmemeval: the pod uses the file of the image, and fetches it with retries when it has none."""

    @classmethod
    def setUpClass(cls):
        cls.root = workdir("fetch-entrypoint")
        cls.tools = cls.root / "tools"
        cls.tools.mkdir()
        stub = ("#!/usr/bin/env python3\nimport json, os, sys\nfrom pathlib import Path\na = sys.argv[1:]\n"
                "get = lambda f: a[a.index(f) + 1]\n"
                "out = Path(get('--output-dir')) / get('--dataset') / get('--name') / get('--mode') / (get('--split') + '.json')\n"
                "out.parent.mkdir(parents=True, exist_ok=True)\n"
                "p = os.environ.get('LONGMEMEVAL_DATA_PATH')\n"
                "out.write_text(json.dumps({'data_path': p, 'size': Path(p).stat().st_size if p and Path(p).exists() else None,\n"
                "                           'results': [{'query_id': 'q1'}]}))\n")
        (cls.tools / "recall_only.py").write_text(stub)
        for name in ("gcs_sync.py", "fetch_dataset.py"):
            (cls.tools / name).write_bytes((BENCH / name).read_bytes())
        (cls.root / "src" / "memory_bench").mkdir(parents=True)

    def pod(self, run_id: str, **extra) -> tuple[subprocess.CompletedProcess, Path]:
        import os
        keep = {k: v for k, v in os.environ.items() if not k.startswith(("ICM_AMB_", "LONGMEMEVAL_", "RUN_", "MODE", "MEMORY"))}
        env = dict(keep, DATASET="longmemeval", SPLIT="s", MODE="recall", MEMORY="bm25", RUN_NAME="bm25", RUN_ID=run_id,
                   TOOLS=str(self.tools), WORK_DIR=str(self.root / f"work-{run_id}"), RESULTS_REMOTE=str(self.root / "remote"),
                   AMB_HOME=str(self.root), SYNC_INTERVAL="1000", ICM_AMB_FETCH_BACKOFF_S="0",
                   PATH=path_with_python(self.root), **extra)
        proc = subprocess.run(["sh", str(BENCH / "entrypoint.sh")], env=env, capture_output=True, text=True)
        return proc, self.root / "remote" / run_id / "all" / "longmemeval" / "bm25" / "recall" / "s.json"

    def test_no_file_in_the_image_fetched_with_retries_before_the_run(self):
        server = Flaky(["refuse", "cut", "ok"])
        self.addCleanup(server.close)
        proc, result = self.pod("fetched", ICM_AMB_DATASET_URL=server.url)
        self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
        got = json.loads(result.read_text())
        self.assertEqual(got["data_path"], str(self.root / "work-fetched" / "datasets" / "longmemeval_s_cleaned.json"))
        self.assertEqual(got["size"], len(PAYLOAD))
        self.assertEqual([s for s, _ in server.requests], ["refuse", "cut", "ok"])

    def test_the_file_of_the_image_is_used_as_it_is(self):
        server = Flaky([])
        self.addCleanup(server.close)
        baked = self.root / "baked.json"
        baked.write_bytes(PAYLOAD)
        proc, result = self.pod("baked", ICM_AMB_DATASET_URL=server.url, LONGMEMEVAL_DATA_PATH=str(baked))
        self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
        self.assertEqual(json.loads(result.read_text())["data_path"], str(baked))
        self.assertEqual(server.requests, [])
        # the variable set by the image but the file left out of it (LONGMEMEVAL=0): fetched
        proc, result = self.pod("left-out", ICM_AMB_DATASET_URL=server.url, LONGMEMEVAL_DATA_PATH=str(self.root / "absent.json"))
        self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
        self.assertEqual(json.loads(result.read_text())["size"], len(PAYLOAD))
        self.assertEqual(len(server.requests), 1)

    def test_a_download_that_never_succeeds_stops_the_pod_before_the_run(self):
        server = Flaky(["refuse"] * 20)
        self.addCleanup(server.close)
        proc, result = self.pod("never", ICM_AMB_DATASET_URL=server.url)
        self.assertEqual(proc.returncode, 1)
        self.assertIn("cannot fetch the LongMemEval-S file", proc.stdout)
        self.assertFalse(result.exists())
        self.assertEqual(len(server.requests), 6)


if __name__ == "__main__":
    unittest.main(verbosity=2)
