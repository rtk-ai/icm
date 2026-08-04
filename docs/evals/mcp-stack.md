# MCP efficiency evaluation

- Warm-up requests per scenario: 20
- Measured requests per scenario: 300
- Latency: client-observed stdio request/response time on a warm server
- Token estimate: compact JSON request + response bytes divided by four
- Token estimates are comparative wire-size proxies, not provider billing data

| Binary | Scenario | Median ms | p95 ms | Request bytes | Response bytes | Est. wire tokens |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| develop | legacy tools/list | 0.351 | 0.474 | 61 | 13924 | 3497 |
| develop | legacy stats | 0.055 | 0.154 | 101 | 119 | 55 |
| develop | modern tools/list | unsupported | — | — | — | — |
| develop | modern stats | unsupported | — | — | — | — |
| develop | modern recall | unsupported | — | — | — | — |
| develop | modern feedback search | unsupported | — | — | — | — |
| develop | modern transcript stats | unsupported | — | — | — | — |
| develop | resources/list | unsupported | — | — | — | — |
| develop | resources/read current | unsupported | — | — | — | — |
| original-prs | legacy tools/list | 0.390 | 0.513 | 61 | 13924 | 3497 |
| original-prs | legacy stats | 0.063 | 0.173 | 101 | 119 | 55 |
| original-prs | modern tools/list | 0.650 | 0.896 | 173 | 27231 | 6851 |
| original-prs | modern stats | 0.066 | 0.160 | 214 | 414 | 157 |
| original-prs | modern recall | 4.916 | 5.711 | 259 | 5382 | 1411 |
| original-prs | modern feedback search | 0.167 | 0.320 | 257 | 2496 | 689 |
| original-prs | modern transcript stats | 0.138 | 0.268 | 218 | 648 | 217 |
| original-prs | resources/list | 0.067 | 0.161 | 177 | 819 | 249 |
| original-prs | resources/read current | unsupported | — | — | — | — |
| reviewed | legacy tools/list | 0.386 | 0.505 | 61 | 13924 | 3497 |
| reviewed | legacy stats | 0.056 | 0.216 | 101 | 119 | 55 |
| reviewed | modern tools/list | 0.695 | 0.851 | 173 | 22866 | 5760 |
| reviewed | modern stats | 0.069 | 0.176 | 214 | 346 | 140 |
| reviewed | modern recall | 3.435 | 5.363 | 259 | 2216 | 619 |
| reviewed | modern feedback search | 0.094 | 0.164 | 257 | 1486 | 436 |
| reviewed | modern transcript stats | 0.064 | 0.078 | 218 | 377 | 149 |
| reviewed | resources/list | 0.021 | 0.025 | 177 | 450 | 157 |
| reviewed | resources/read current | 2.491 | 3.194 | 207 | 259 | 117 |

## Pairwise changes

### develop → original-prs

| Scenario | Median latency | Response bytes | Est. wire tokens |
| --- | ---: | ---: | ---: |
| legacy tools/list | +11.3% | +0.0% | +0.0% |
| legacy stats | +14.4% | +0.0% | +0.0% |

### original-prs → reviewed

| Scenario | Median latency | Response bytes | Est. wire tokens |
| --- | ---: | ---: | ---: |
| legacy tools/list | -1.1% | +0.0% | +0.0% |
| legacy stats | -11.2% | +0.0% | +0.0% |
| modern tools/list | +6.9% | -16.0% | -15.9% |
| modern stats | +4.1% | -16.4% | -10.8% |
| modern recall | -30.1% | -58.8% | -56.1% |
| modern feedback search | -43.5% | -40.5% | -36.7% |
| modern transcript stats | -53.6% | -41.8% | -31.3% |
| resources/list | -69.3% | -45.1% | -36.9% |

## Recall relevance and output quality

The deterministic corpus contains two relevant memories for each of two
queries plus unrelated distractors. Modern output is compared with a
legacy recall from the same binary; this tests transport fidelity, not
subjective answer style.

| Binary | Era | Hit@3 | Recall@3 | nDCG@3 | Structured/legacy order parity | Required field coverage |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| develop | legacy | 100.0% | 100.0% | 1.000 | — | — |
| develop | modern | unsupported | — | — | — | — |
| original-prs | legacy | 100.0% | 100.0% | 1.000 | — | — |
| original-prs | modern | 100.0% | 100.0% | 1.000 | 100.0% | 100.0% |
| reviewed | legacy | 100.0% | 100.0% | 1.000 | — | — |
| reviewed | modern | 100.0% | 100.0% | 1.000 | 100.0% | 100.0% |

A 100% structured/legacy parity score means the same memory markers
appear in the same order. Required-field coverage checks `id`, `topic`,
`summary`, `importance`, `weight`, and `score` on every structured hit.

## Interpretation

- Modern recall wire-token estimate changed -56.1% from `original-prs` to `reviewed`.
- Retrieval relevance and structured-output fidelity were preserved.
- This protocol work does not change ICM's ranking algorithm, so it should not be described as a retrieval-quality improvement.
- Sub-millisecond scenarios are sensitive to host scheduling; latency deltas are directional observations, not a performance guarantee.
