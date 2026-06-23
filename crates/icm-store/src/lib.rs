//! Storage backends for ICM.
//!
//! The store is pluggable (issue #301). Exactly one backend is selected
//! at build time via a Cargo feature, and all expose the same public
//! surface through the [`Store`] type alias so `icm-cli` / `icm-mcp` are
//! backend-agnostic:
//!
//! - **`backend-sqlite`** (default) — in-process SQLite via `rusqlite` +
//!   `sqlite-vec`. Single binary, zero external services. This is the
//!   only built-in backend and is byte-for-byte unchanged from before.
//! - **`postgres`** (opt-in) — a network-accessible PostgreSQL backend so
//!   multiple ICM processes / Kubernetes replicas can share one memory
//!   store. Build with `--no-default-features --features postgres`.
//! - **`opensearch`** (opt-in) — a search-native OpenSearch backend
//!   (BM25 + `knn_vector` HNSW) sharing memory across replicas. Build
//!   with `--no-default-features --features opensearch`.

// Exactly one backend must be active.
#[cfg(any(
    all(feature = "backend-sqlite", feature = "postgres"),
    all(feature = "backend-sqlite", feature = "opensearch"),
    all(feature = "postgres", feature = "opensearch"),
))]
compile_error!(
    "the storage backends `backend-sqlite`, `postgres` and `opensearch` are \
     mutually exclusive; build a remote backend with `--no-default-features \
     --features <postgres|opensearch>`"
);
#[cfg(not(any(
    feature = "backend-sqlite",
    feature = "postgres",
    feature = "opensearch"
)))]
compile_error!(
    "a storage backend must be selected: enable `backend-sqlite` (default), \
     `postgres`, or `opensearch`"
);

#[cfg(feature = "backend-sqlite")]
mod schema;
#[cfg(feature = "backend-sqlite")]
mod store;

#[cfg(feature = "postgres")]
mod postgres;

#[cfg(feature = "opensearch")]
mod opensearch;

#[cfg(feature = "backend-sqlite")]
pub use store::{HookEvent, HookEventInsert, HookStatsRow, PendingRow, SqliteStore};

/// The active storage backend. `icm-cli` and `icm-mcp` use this alias
/// everywhere instead of a concrete type, so swapping backends is a
/// build-feature change with no call-site churn.
#[cfg(feature = "backend-sqlite")]
pub type Store = SqliteStore;

#[cfg(feature = "postgres")]
pub use postgres::{HookEvent, HookEventInsert, HookStatsRow, PendingRow, PostgresStore};

/// The active storage backend (PostgreSQL build). See the `backend-sqlite`
/// variant above.
#[cfg(feature = "postgres")]
pub type Store = PostgresStore;

#[cfg(feature = "opensearch")]
pub use opensearch::{HookEvent, HookEventInsert, HookStatsRow, OpenSearchStore, PendingRow};

/// The active storage backend (OpenSearch build). See the `backend-sqlite`
/// variant above.
#[cfg(feature = "opensearch")]
pub type Store = OpenSearchStore;
