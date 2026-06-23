//! Storage backends for ICM.
//!
//! The store is pluggable (issue #301). Exactly one backend is selected
//! at build time via a Cargo feature, and both expose the same public
//! surface through the [`Store`] type alias so `icm-cli` / `icm-mcp` are
//! backend-agnostic:
//!
//! - **`backend-sqlite`** (default) — in-process SQLite via `rusqlite` +
//!   `sqlite-vec`. Single binary, zero external services. This is the
//!   only built-in backend and is byte-for-byte unchanged from before.
//! - **`postgres`** (opt-in) — a network-accessible PostgreSQL backend so
//!   multiple ICM processes / Kubernetes replicas can share one memory
//!   store, which a node-local SQLite file cannot do. Build with
//!   `--no-default-features --features postgres`.

// Exactly one backend must be active.
#[cfg(all(feature = "backend-sqlite", feature = "postgres"))]
compile_error!(
    "features `backend-sqlite` and `postgres` are mutually exclusive; \
     build the postgres backend with `--no-default-features --features postgres`"
);
#[cfg(not(any(feature = "backend-sqlite", feature = "postgres")))]
compile_error!(
    "a storage backend must be selected: enable `backend-sqlite` (default) or `postgres`"
);

#[cfg(feature = "backend-sqlite")]
mod schema;
#[cfg(feature = "backend-sqlite")]
mod store;

#[cfg(feature = "postgres")]
mod postgres;

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
