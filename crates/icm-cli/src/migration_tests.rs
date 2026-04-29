#[cfg(test)]
mod tests {
    use icm_core::{Importance, Memory, MemoryStore};
    use icm_store::SqliteStore;
    use tempfile::TempDir;

    fn make_memory(idx: u32, dims: usize) -> Memory {
        let mut m = Memory::new(
            "test-topic".into(),
            format!("test memory {idx}"),
            Importance::Medium,
        );
        m.id = format!("mem-{idx:04}");
        m.embedding = Some(vec![0.0f32; dims]);
        m
    }

    #[test]
    fn test_dim_change_detection_and_nulling() {
        let tmp = TempDir::new().expect("tempdir");
        let db_path = tmp.path().join("migration_test.db");

        // Open with dim 384 and store 5 memories with embeddings.
        {
            let (store, status) =
                SqliteStore::with_dims(&db_path, 384).expect("open store @384");
            assert!(!status.dim_changed, "fresh store should not report dim change");

            for i in 0..5u32 {
                store.store(make_memory(i, 384)).expect("store memory");
            }

            // Verify all 5 have embeddings.
            let all = store.list_all().expect("list_all");
            assert_eq!(all.len(), 5);
            assert!(
                all.iter().all(|m| m.embedding.is_some()),
                "all memories should have embeddings after initial store"
            );
        }

        // Re-open with a different dim (768) — triggers the migration.
        let (store2, status) =
            SqliteStore::with_dims(&db_path, 768).expect("open store @768");

        assert!(status.dim_changed, "dim_changed should be true");
        assert_eq!(status.old_dim, 384, "old_dim should be 384");
        assert_eq!(status.new_dim, 768, "new_dim should be 768");
        assert_eq!(
            status.affected_rows, 5,
            "affected_rows should equal the 5 stored memories"
        );

        // Verify all rows now have embedding == None.
        let all = store2.list_all().expect("list_all after migration");
        assert_eq!(all.len(), 5, "all 5 memories should still exist");
        assert!(
            all.iter().all(|m| m.embedding.is_none()),
            "all embeddings should be NULL after dim-change migration"
        );
    }
}
