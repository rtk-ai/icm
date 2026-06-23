# PostgreSQL backend (opt-in)

By default ICM stores memory in a node-local SQLite file. That is perfect
for a single machine, but a local file cannot be shared between several
ICM processes or between Kubernetes replicas, so memory can't be shared
across instances.

The **PostgreSQL backend** (issue #301) runs the same memory model over a
network-accessible PostgreSQL database. Every instance reads and writes
one shared store, and PostgreSQL serialises concurrent writers, so N
replicas can `icm store` into the same memory with no lost rows.

It is **opt-in** and selected at build time via a Cargo feature. The
default build is unchanged: SQLite only, single binary, zero external
services.

## What it covers

This first cut implements the **core memory surface** — exactly the part
that benefits from being shared:

- `store` (with dedup + metadata merge), `get`, `update`, `forget`
- keyword search, full-text search (PostgreSQL `tsvector` + GIN),
  vector KNN (`pgvector`, cosine), and the hybrid blend (30% FTS / 70%
  vector)
- `list`, `topics`, `stats`, `health`
- temporal `decay` (access-aware) and `prune`
- the ancillary tables used by the normal store/recall/hook path:
  hook telemetry, the async extraction queue, code areas, and the
  key/value metadata.

The heavier subsystems — memoir graph, verbatim transcripts, structured
facts, feedback, and pattern mining — return a clear
`operation not supported on this storage backend` error on PostgreSQL for
now. They remain fully available on the default SQLite backend.

## Requirements

PostgreSQL with the [`pgvector`](https://github.com/pgvector/pgvector)
extension available (the backend runs `CREATE EXTENSION IF NOT EXISTS
vector`). The `pgvector/pgvector` images and Azure Database for
PostgreSQL Flexible Server (with `vector` allow-listed) both work.

## Build

```sh
cargo build -p icm-cli --release \
    --no-default-features \
    --features "postgres,embeddings,tui,http-api"
```

`postgres` and `backend-sqlite` are mutually exclusive; `--no-default-features`
drops the default SQLite backend so only PostgreSQL is compiled in.

## Configure

The connection string comes from the environment:

```sh
export ICM_POSTGRES_URL="postgres://user:pass@host:5432/icm"
# DATABASE_URL is accepted as a fallback.
```

The `--db` flag (a SQLite file path) is ignored by this backend.

The schema — including the `vector(N)` embedding column whose dimension
`N` matches your embedder — is created on first connect. The stored
dimension is authoritative afterwards, so always initialise a database
with the embedder you intend to use (or `--no-embeddings` for
keyword/FTS-only).

## Verify

```sh
docker run -d --name icm-pg \
    -e POSTGRES_PASSWORD=icm -e POSTGRES_USER=icm -e POSTGRES_DB=icm \
    -p 55432:5432 pgvector/pgvector:pg16

export ICM_POSTGRES_URL="postgres://icm:icm@127.0.0.1:55432/icm"

icm store -t demo -c "PostgreSQL backend shares memory across replicas" -i high
icm recall "shared memory"
icm stats

# Integration tests (skipped automatically when the env var is unset):
cargo test -p icm-store --no-default-features --features postgres -- --test-threads=1
```

## Kubernetes

A network backend is what makes a horizontally-scaled deployment share
memory. Point every replica's `ICM_POSTGRES_URL` at the same PostgreSQL
service and they read/write one store. See
[`deploy/k8s`](../deploy/k8s) for a minimal manifest set used to validate
this on a real cluster.
