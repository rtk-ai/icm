# OpenSearch backend (opt-in)

By default ICM stores memory in a node-local SQLite file, which cannot be
shared between several ICM processes or Kubernetes replicas.

The **OpenSearch backend** (issue #301) is a *search-native* shared store:
BM25 full-text and `knn_vector` HNSW vector search live in one engine, so
horizontally-scaled replicas share one memory store. It is **opt-in** and
selected at build time via a Cargo feature; the default build is unchanged
(SQLite only, single binary).

OpenSearch is Apache-2.0 licensed, which matches ICM's own license.

## What it covers

The **core memory surface** — exactly the part that benefits from being
shared:

- `store` (with dedup + metadata merge), `get`, `update`, `forget`
- keyword search, full-text search (BM25), vector KNN (`knn_vector`,
  cosine), and the hybrid blend (30% BM25 / 70% vector)
- `list`, `topics`, `stats`, `health`
- temporal `decay` (access-aware) and `prune`
- the ancillary store/recall/hook collections: hook telemetry, the async
  extraction queue, code areas, and the key/value metadata.

The heavier subsystems (memoir graph, transcripts, structured facts,
feedback, pattern mining) return a clear "not supported on this backend"
error for now and stay fully available on SQLite.

## Build

```sh
cargo build -p icm-cli --release --no-default-features \
    --features "opensearch,embeddings,tui,http-api"
```

Drop `embeddings` for a lean container image that uses keyword + BM25
search only.

## Configure

The backend talks to the OpenSearch REST API over the blocking `ureq`
client (no async runtime). Point it at your cluster:

```sh
export ICM_OPENSEARCH_URL=http://localhost:9201
# optional basic auth (when the security plugin is enabled):
export ICM_OPENSEARCH_USER=admin
export ICM_OPENSEARCH_PASSWORD=...
```

The `--db` flag is ignored by this backend.

### Local OpenSearch

```sh
docker run -d --name icm-os -p 9201:9200 \
    -e discovery.type=single-node \
    -e DISABLE_SECURITY_PLUGIN=true \
    -e DISABLE_INSTALL_DEMO_CONFIG=true \
    -e "OPENSEARCH_JAVA_OPTS=-Xms512m -Xmx512m" \
    opensearchproject/opensearch:2
```

## Kubernetes

Point every replica's `ICM_OPENSEARCH_URL` at the same OpenSearch service
and they read/write one store. See [`deploy/k8s`](../deploy/k8s) for a
minimal manifest set (`opensearch.yaml` + a concurrent-writer Job) used to
validate this on a real cluster.
