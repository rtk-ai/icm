mod schema;
mod store;

pub use store::SqliteStore;

/// Returned by store-open functions to report whether a dimension-change
/// migration was executed.  `dim_changed = false` means the vector table was
/// untouched; `true` means the old table was dropped, all `memories.embedding`
/// rows were set to NULL, and the table was recreated with `new_dim` columns.
#[derive(Debug, Clone, Default)]
pub struct MigrationStatus {
    pub dim_changed: bool,
    pub old_dim: usize,
    pub new_dim: usize,
    pub affected_rows: usize,
}
