// Exactly one storage backend must be active.
#[cfg(all(feature = "backend-rusqlite", feature = "turso"))]
compile_error!(
    "icm-store: enable exactly one backend — default `backend-rusqlite`, \
     OR libSQL/Turso via `--no-default-features --features turso` (not both)"
);
#[cfg(not(any(feature = "backend-rusqlite", feature = "turso")))]
compile_error!(
    "icm-store: no storage backend enabled — keep the default `backend-rusqlite` \
     or build with `--features turso`"
);

#[cfg(feature = "turso")]
#[macro_use]
pub mod dbcompat;
mod schema;
mod store;

pub use store::{HookEvent, HookEventInsert, HookStatsRow, PendingRow, SqliteStore};
