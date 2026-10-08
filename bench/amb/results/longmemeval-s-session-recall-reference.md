# LongMemEval-S, session recall without any model: reference points (2026-10-05)

This page holds the floor (BM25), the two published figures re-scored with our
scorer, and ICM's run on the same unit and with the same scorer.

ICM, measured on 2026-10-06 at commit b49db9b (recall code identical to 0.11.0),
engine v2 with its default embedding model, unit session-user, no date sent
(`ICM_AMB_STORE_DATE=0`, `ICM_AMB_QUERY_NOW=0`), per-question results in
`longmemeval-s-icm-v2-session-user-nodate-20261006.json`:

| unit | any@1 | any@3 | any@5 | any@10 | all@5 | all@10 |
|---|---|---|---|---|---|---|
| session-user | 89.4 | 96.2 | 97.4 (487/500) | 98.6 | 88.6 (443/500) | 94.8 |

All rows: 500 questions of `longmemeval_s_cleaned.json` (abstention questions
included), one index per question, expected sessions = `answer_session_ids`,
top k = first k results. `any` = at least one expected session in the top k,
`all` = every expected session in the top k.

BM25 over the same units, measured by us with `recall_only.py --memory bm25`
(rank_bm25 Okapi; `words` = lower-cased runs of letters and digits; `harness` =
the upstream bm25 provider's `lower().split()`):

| unit | tokenizer | any@1 | any@3 | any@5 | any@10 | all@5 | all@10 |
|---|---|---|---|---|---|---|---|
| session-user | words | 84.0 | 93.6 | 94.6 | 96.4 | 81.2 | 88.2 |
| session-user | harness | 73.4 | 86.2 | 90.2 | 93.0 | 75.6 | 81.6 |
| session-all | words | 86.6 | 94.4 | 96.2 | 98.0 | 83.0 | 89.6 |
| session-all | harness | 77.4 | 88.4 | 91.4 | 95.4 | 75.4 | 83.2 |
| chunk (512 tokens, top k chunks) | words | 83.4 | 91.6 | 95.4 | 97.4 | 75.4 | 82.4 |
| chunk (512 tokens, top k chunks) | harness | 74.6 | 86.4 | 89.8 | 93.0 | 65.0 | 76.2 |

Without the 30 abstention questions (470), session-user / words: any@5 94.7, all@5 81.9.

Published figures, recomputed by us from the vendors' committed result files with
`merge_recall.score` (their rankings, not a re-run):

| system | file | any@5 | any@10 | all@5 | all@10 |
|---|---|---|---|---|---|
| MemPalace raw (session-user unit) | `benchmarks/results_mempal_raw_session_20260414_1629.jsonl` | 96.6 (483/500) | 98.2 | 85.0 | 93.2 |
| agentmemory BM25+vector (session-all unit) | `benchmark/data/longmemeval_results_hybrid.json` | 95.2 (476/500) | 98.6 | 81.8 | 93.4 |
| agentmemory BM25 only | `benchmark/data/longmemeval_results_bm25.json` | 86.2 (431/500) | 94.6 | 66.4 | 80.4 |

`any@5` and `any@10` equal what each vendor publishes; `all@k` is not published by
either. Plain BM25 on the same unit is within 2.0 points of MemPalace's any@5
(94.6 against 96.6) and above agentmemory's on its unit (96.2 against 95.2): a
recall_any@5 in the mid-nineties on this split is what keyword search gives.

Dataset facts measured on the file: 23,867 sessions (38 to 62 per question, 47.7
on average); 71 sessions have no user turn (none is expected); 13 questions list a
session id twice in their haystack; the harness' gold (`has_answer` turns) differs
from `answer_session_ids` on 62 questions and is empty for 21; one session is over
64 KiB.
