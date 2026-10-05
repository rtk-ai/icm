## locomo/locomo10: 1540 queries (recall only, no LLM)

| Metric | linux-legacy | linux-v2 | Delta |
|---|---|---|---|
| ICM version | 0.10.65 | 0.10.65 | |
| Settings | engine legacy, no date sent, k=50, chunks 512 | engine v2, document date sent (created_at), question date sent (now), k=50, chunks 512 | |
| Sent to ICM (HTTP trace) | /store with created_at 0/876; /recall with now 0/1540; engine field: legacy x1540 | /store with created_at 876/876; /recall with now 1540/1540; engine field: v2 x1540 | |
| Queries | 1540 | 1540 | |
| Queries with gold ids | 1531 | 1531 | |
| Gold sessions any@5 | 76.5% | 86.7% | +10.1 pts |
| Gold sessions all@5 | 64.3% | 73.8% | +9.5 pts |
| Gold sessions frac@5 | 69.9% | 79.9% | +10.0 pts |
| Gold sessions any@10 | 83.0% | 93.3% | +10.4 pts |
| Gold sessions all@10 | 71.7% | 82.8% | +11.1 pts |
| Gold sessions frac@10 | 77.3% | 88.3% | +11.1 pts |
| Gold sessions any@20 | 87.7% | 97.9% | +10.2 pts |
| Gold sessions all@20 | 78.2% | 91.0% | +12.8 pts |
| Gold sessions frac@20 | 83.4% | 95.1% | +11.7 pts |
| Gold sessions any@50 | 99.2% | 99.6% | +0.5 pts |
| Gold sessions all@50 | 95.2% | 98.2% | +3.0 pts |
| Gold sessions frac@50 | 97.7% | 99.1% | +1.4 pts |
| Memories returned (avg) | 34.2 | 50.0 | +15.8 |
| Context tokens (avg, tiktoken) | 16078.7 | 24064.3 | +7985.6 |
| Ingestion (s) | 1271.9 | 1250.8 | -21.1 |
| Memories stored | 876 | 876 | +0.0 |
| Ingestion (s per memory) | 1.45 | 1.43 | -0.0 |
| Recall latency avg (ms) | 134.8 | 143.4 | +8.6 |
| Recall latency median (ms) | 131.1 | 136.2 | +5.1 |
