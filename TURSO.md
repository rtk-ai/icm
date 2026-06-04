# ICM — libSQL / Turso backend (fork)

This fork lets ICM store its memory in a **libSQL / Turso** database instead of a
single local SQLite file, so **multiple machines / processes can read and write
the same memory concurrently** — the server serialises writes, so there are no
copies to merge and no SQLite-over-NFS corruption.

Upstream ICM is plain `rusqlite` (one local file). The change is isolated to the
`icm-store` crate: a thin synchronous facade (`src/dbcompat.rs`) that mirrors the
slice of the rusqlite API ICM uses, but drives the async `libsql` client under the
hood. ICM's 6,300 lines of SQL logic are unchanged.

## Backends (chosen by environment)

| Env | Backend | Use |
|-----|---------|-----|
| *(none)* | **Local** SQLite file (`--db` / default path) | unchanged single-machine behaviour |
| `TURSO_DATABASE_URL` (or `LIBSQL_URL`) | **Remote** libSQL/Turso server | **recommended for multi-writer** — every ICM process shares one server |
| `…URL` + `ICM_TURSO_REPLICA=1` | **Embedded replica** (local file synced to primary) | local-first reads; see limitations |

Auth token: `TURSO_AUTH_TOKEN` (or `LIBSQL_AUTH_TOKEN`); empty is fine for an
unauthenticated self-hosted `sqld`.

## Run a self-hosted primary (`sqld`) with vector search

ICM's schema uses the `vec0` virtual table (sqlite-vec), so the **server** must
load that extension (the client doesn't — remote queries run server-side):

```bash
# 1. grab the sqlite-vec loadable extension matching the crate version (0.1.6)
mkdir -p ~/.icm-ext && cd ~/.icm-ext
curl -fsSL https://github.com/asg017/sqlite-vec/releases/download/v0.1.6/sqlite-vec-0.1.6-loadable-linux-x86_64.tar.gz | tar xz
sha256sum vec0.so > trusted.lst          # sqld trusts extensions listed here

# 2. run the server (sqld is in nixpkgs)
nix run nixpkgs#sqld -- --db-path ~/.icm/primary.sqld \
  --http-listen-addr 0.0.0.0:8080 --extensions-path ~/.icm-ext
```

Then point every ICM client at it:

```bash
export TURSO_DATABASE_URL=http://<server-host>:8080
icm store --topic notes --content "shared across machines"
icm recall "shared"
```

## Verified

- Local store/recall: ✅
- Remote (sqld): store/recall ✅, sqlite-vec loaded server-side ✅
- **Concurrency: 16 independent `icm` processes writing at once → 16/16 stored,
  zero lost, no corruption** ✅

## Known limitations

- **Embedded replica + vector schema:** libSQL embedded replicas don't forward the
  `vec0` `CREATE VIRTUAL TABLE` DDL (`unsupported statement`). Use **remote** mode
  for the vector schema; embedded replica is best for keyword-only or read-heavy
  use. (Remote mode is the recommended multi-writer setup anyway.)
- **Client-side embeddings:** generating embeddings needs the fastembed model
  (downloaded on first use). For pure keyword use, pass `--no-embeddings`. Vector
  *search* still runs server-side where `vec0` is loaded.
- **Drop-on-exit warning:** a benign `libsql::hrana … no runtime was available`
  line can appear at process exit (the write already committed); it's the async
  client closing after the sync shim's runtime context ends.

## Build / run

Needs `libstdc++` and `openssl` at runtime (NixOS keeps them in the store):

```bash
cargo build --release
LD_LIBRARY_PATH="$(nix eval --raw nixpkgs#stdenv.cc.cc.lib)/lib:$(nix eval --raw nixpkgs#openssl.out)/lib" \
  ./target/release/icm ...
```

A `flake.nix` is provided for a properly-wrapped Nix build (no `LD_LIBRARY_PATH`
needed).
