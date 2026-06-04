# libSQL / Turso storage backend (opt-in)

ICM stores memory in a single local SQLite file, so it can't be shared or written
concurrently from more than one machine/process. This **opt-in** backend runs the
existing `icm-store` SQL over the async **`libsql`** client instead, so the store
can live in a libSQL/Turso database — a local file, a remote `sqld`/Turso server,
or an embedded replica. A remote server is the multi-writer path: every ICM
process talks to the one server, which serialises writes.

**The default build is unchanged** — it uses `rusqlite` exactly as before. The
two backends are mutually exclusive Cargo features.

## Building

```bash
# default — rusqlite (no change)
cargo build

# libSQL/Turso backend
cargo build -p icm-cli --no-default-features --features turso,embeddings,tui
# or just the store crate:
cargo test -p icm-store --no-default-features --features turso
```

(A `flake.nix` builds the turso binary via Nix with the C deps wired up.)

## Choosing the backend at runtime (turso build)

| Env | Backend |
|-----|---------|
| *(none)* | local SQLite file (`--db` / default path) |
| `TURSO_DATABASE_URL` (or `LIBSQL_URL`) [+ `TURSO_AUTH_TOKEN`] | remote libSQL/Turso server — recommended for multi-writer |
| `…URL` + `ICM_TURSO_REPLICA=1` | local embedded replica syncing to the primary |

## Self-hosted server with vector search

ICM's schema uses the `vec0` virtual table (sqlite-vec), so the **server** must
load that extension (the client doesn't — remote queries run server-side):

```bash
mkdir -p ~/.icm-ext && cd ~/.icm-ext
curl -fsSL https://github.com/asg017/sqlite-vec/releases/download/v0.1.6/sqlite-vec-0.1.6-loadable-linux-x86_64.tar.gz | tar xz
sha256sum vec0.so > trusted.lst
nix run nixpkgs#sqld -- --db-path ~/.icm/primary.sqld \
  --http-listen-addr 0.0.0.0:8080 --extensions-path ~/.icm-ext

export TURSO_DATABASE_URL=http://<host>:8080
icm store --topic notes --content "shared across machines"
```

## Implementation

`icm-store` gains a sync-over-async facade (`src/dbcompat.rs`) that mirrors the
slice of the rusqlite API the store uses. Under `--features turso` it's aliased
as `rusqlite`, so `store.rs`/`schema.rs` are byte-identical to the rusqlite build
apart from the alias and the connection-open path.

## Verified

- Default backend: **162/162** `icm-store` tests pass (unchanged).
- Turso backend: **161/162** (only `perf_fts_search_100` regresses — see below).
- Against a self-hosted `sqld`: store/recall, sqlite-vec server-side, and **16
  concurrent `icm` processes wrote with zero lost rows**.

## Known limitations (turso backend only)

- `perf_fts_search_100` regresses: the block-on-per-call bridge adds overhead
  (worse over the network). Needs connection reuse or an async store path.
- Embedded replicas can't forward the `vec0` `CREATE VIRTUAL TABLE` DDL
  (`unsupported statement`), so remote mode is the vector path.
- A benign `libsql::hrana … no runtime was available` line can appear at process
  exit (the write already committed).
