## locomo/locomo10: 1540 queries (recall only, no LLM)

| Metric | linux-legacy-noemb | linux-v2-noemb | Delta |
|---|---|---|---|
| ICM version | 0.10.65 | 0.10.65 | |
| Settings | engine legacy, no date sent, k=50, chunks 512 | engine v2, document date sent (created_at), question date sent (now), k=50, chunks 512 | |
| Sent to ICM (HTTP trace) | /store with created_at 0/876; /recall with now 0/1540; engine field: legacy x1540 | /store with created_at 876/876; /recall with now 1540/1540; engine field: v2 x1540 | |
| Queries | 1540 | 1540 | |
| Queries with gold ids | 1531 | 1531 | |
| Gold sessions any@5 | 12.0% | 88.6% | +76.6 pts |
| Gold sessions all@5 | 7.8% | 77.1% | +69.3 pts |
| Gold sessions frac@5 | 9.4% | 82.3% | +72.8 pts |
| Gold sessions any@10 | 17.4% | 94.2% | +76.8 pts |
| Gold sessions all@10 | 11.4% | 83.2% | +71.8 pts |
| Gold sessions frac@10 | 13.8% | 88.7% | +75.0 pts |
| Gold sessions any@20 | 29.8% | 97.5% | +67.7 pts |
| Gold sessions all@20 | 20.4% | 90.1% | +69.7 pts |
| Gold sessions frac@20 | 24.5% | 94.4% | +69.8 pts |
| Gold sessions any@50 | 63.8% | 99.9% | +36.1 pts |
| Gold sessions all@50 | 52.7% | 98.7% | +46.0 pts |
| Gold sessions frac@50 | 58.3% | 99.5% | +41.2 pts |
| Memories returned (avg) | 47.8 | 49.9 | +2.1 |
| Context tokens (avg, tiktoken) | 22540.7 | 25431.6 | +2890.9 |
| Ingestion (s) | 13.2 | 9.1 | -4.1 |
| Memories stored | 876 | 876 | +0.0 |
| Ingestion (s per memory) | 0.02 | 0.01 | -0.0 |
| Recall latency avg (ms) | 6.8 | 9.4 | +2.6 |
| Recall latency median (ms) | 6.2 | 6.8 | +0.6 |
