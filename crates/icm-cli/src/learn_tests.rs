#[cfg(test)]
mod tests {
    use icm_core::learn::learn_project;
    use icm_core::MemoirStore;
    use icm_store::Store;
    use std::fs;
    use tempfile::TempDir;

    fn test_store() -> (TempDir, Store) {
        let tmp = TempDir::new().expect("failed to create temp dir");
        let db_path = tmp.path().join("test.db");
        let store = Store::with_dims(&db_path, 384).expect("failed to create store");
        (tmp, store)
    }

    #[test]
    fn test_learn_rust_project() {
        let (tmp, store) = test_store();

        // Create a fake Rust workspace project
        let project_dir = tmp.path().join("my-project");
        fs::create_dir_all(project_dir.join("crates/core/src")).unwrap();
        fs::create_dir_all(project_dir.join("crates/cli/src")).unwrap();
        fs::create_dir_all(project_dir.join(".github/workflows")).unwrap();
        fs::create_dir_all(project_dir.join("scripts")).unwrap();

        // Root Cargo.toml (workspace)
        fs::write(
            project_dir.join("Cargo.toml"),
            r#"
[workspace]
resolver = "2"
members = ["crates/*"]

[workspace.dependencies]
serde = "1.0"
tokio = { version = "1", features = ["full"] }
anyhow = "1"
"#,
        )
        .unwrap();

        // Core crate
        fs::write(
            project_dir.join("crates/core/Cargo.toml"),
            r#"
[package]
name = "my-core"
version = "0.1.0"
edition = "2021"
description = "Core library"
"#,
        )
        .unwrap();
        fs::write(
            project_dir.join("crates/core/src/lib.rs"),
            "pub fn hello() {}",
        )
        .unwrap();

        // CLI crate
        fs::write(
            project_dir.join("crates/cli/Cargo.toml"),
            r#"
[package]
name = "my-cli"
version = "0.1.0"
edition = "2021"
description = "CLI tool"
"#,
        )
        .unwrap();
        fs::write(project_dir.join("crates/cli/src/main.rs"), "fn main() {}").unwrap();

        // GitHub workflow
        fs::write(
            project_dir.join(".github/workflows/ci.yml"),
            "name: CI\non: push\n",
        )
        .unwrap();

        // Script
        fs::write(
            project_dir.join("scripts/build.sh"),
            "#!/bin/bash\ncargo build",
        )
        .unwrap();

        // Makefile
        fs::write(project_dir.join("Makefile"), "build:\n\tcargo build").unwrap();

        // Run learn
        let result = learn_project(&store, &project_dir, None).expect("learn_project failed");

        // Verify memoir was created
        let memoir = store.get_memoir(&result.memoir_id).unwrap().unwrap();
        assert_eq!(memoir.name, "my-project");

        // Verify concepts
        let concepts = store.list_concepts(&result.memoir_id).unwrap();
        let kinds: Vec<String> = concepts
            .iter()
            .flat_map(|c| {
                c.labels
                    .iter()
                    .map(|l| format!("{}:{}", l.namespace, l.value))
            })
            .collect();

        assert!(kinds.contains(&"kind:project".to_string()));
        assert!(kinds.contains(&"kind:dependency".to_string()));
        assert!(kinds.contains(&"kind:module".to_string()));
        assert!(kinds.contains(&"kind:entrypoint".to_string()));
        assert!(kinds.contains(&"kind:config".to_string()));
        assert!(kinds.contains(&"kind:script".to_string()));

        // Verify links exist
        let links = store.get_links_for_memoir(&result.memoir_id).unwrap();
        assert!(!links.is_empty(), "should have created links");

        // Verify we have at least: 1 project + 3 deps + 2 modules + entrypoints + configs + scripts
        assert!(
            concepts.len() >= 7,
            "expected at least 7 concepts, got {}",
            concepts.len()
        );
    }

    #[test]
    fn test_learn_node_project() {
        let (tmp, store) = test_store();

        let project_dir = tmp.path().join("my-node-app");
        fs::create_dir_all(project_dir.join("src")).unwrap();

        fs::write(
            project_dir.join("package.json"),
            r#"{
  "name": "my-node-app",
  "version": "1.0.0",
  "description": "A Node.js app",
  "dependencies": {
    "express": "^4.18.0",
    "lodash": "^4.17.0"
  }
}"#,
        )
        .unwrap();

        fs::write(project_dir.join("index.ts"), "console.log('hello')").unwrap();
        fs::write(project_dir.join("tsconfig.json"), "{}").unwrap();

        let result = learn_project(&store, &project_dir, Some("node-test")).expect("learn failed");

        let memoir = store.get_memoir(&result.memoir_id).unwrap().unwrap();
        assert_eq!(memoir.name, "node-test");

        let concepts = store.list_concepts(&result.memoir_id).unwrap();
        assert!(
            concepts.len() >= 4,
            "expected at least 4 concepts (project + 2 deps + entrypoint), got {}",
            concepts.len()
        );
    }

    #[test]
    fn test_learn_replaces_existing_memoir() {
        let (tmp, store) = test_store();

        let project_dir = tmp.path().join("replace-test");
        fs::create_dir_all(&project_dir).unwrap();
        fs::write(
            project_dir.join("Cargo.toml"),
            r#"
[package]
name = "replace-test"
version = "0.1.0"
edition = "2021"
"#,
        )
        .unwrap();

        // First learn
        let r1 = learn_project(&store, &project_dir, Some("replace-test")).unwrap();
        // Second learn should replace
        let r2 = learn_project(&store, &project_dir, Some("replace-test")).unwrap();

        assert_ne!(r1.memoir_id, r2.memoir_id, "should create a new memoir");
        // Old memoir should be gone
        assert!(store.get_memoir(&r1.memoir_id).unwrap().is_none());
    }

    /// Audit regression: a package listed in both `workspace.dependencies`
    /// and `dependencies` (e.g. via `{ workspace = true }`, common in real
    /// Cargo workspaces) used to be collected twice with the same name.
    /// `add_concept`'s underlying `UNIQUE(memoir_id, name)` constraint then
    /// turned the second insert into a hard error, aborting the whole learn
    /// run and losing the memoir that a prior successful learn had created.
    #[test]
    fn test_learn_project_dedupes_duplicate_cargo_dependency_names() {
        let (tmp, store) = test_store();

        let project_dir = tmp.path().join("dup-deps-project");
        fs::create_dir_all(project_dir.join("src")).unwrap();

        fs::write(
            project_dir.join("Cargo.toml"),
            r#"
[package]
name = "dup-deps-project"
version = "0.1.0"
edition = "2021"

[workspace.dependencies]
serde = "1.0"

[dependencies]
serde = { workspace = true }
anyhow = "1"
"#,
        )
        .unwrap();
        fs::write(project_dir.join("src/lib.rs"), "pub fn hello() {}").unwrap();

        let result =
            learn_project(&store, &project_dir, None).expect("duplicate dep name must not abort");

        let concepts = store.list_concepts(&result.memoir_id).unwrap();
        let serde_count = concepts.iter().filter(|c| c.name == "serde").count();
        assert_eq!(serde_count, 1, "serde must be deduped to a single concept");
    }

    /// Audit regression: a `workspace.members` glob pattern resolving
    /// outside the project directory (via `../..` or a symlink) used to be
    /// followed as-is, letting a malicious repo's manifest pull in and
    /// expose files/directories the user never asked to learn.
    #[test]
    fn test_learn_project_rejects_workspace_member_path_traversal() {
        let (tmp, store) = test_store();

        // A sibling directory *outside* the project root, containing a
        // secret the traversal must not be able to reach.
        fs::create_dir_all(tmp.path().join("secret-outside")).unwrap();
        fs::write(
            tmp.path().join("secret-outside/Cargo.toml"),
            r#"
[package]
name = "leaked-secret"
version = "0.1.0"
edition = "2021"
description = "should never be learned"
"#,
        )
        .unwrap();

        let project_dir = tmp.path().join("traversal-project");
        fs::create_dir_all(project_dir.join("src")).unwrap();
        fs::write(
            project_dir.join("Cargo.toml"),
            r#"
[package]
name = "traversal-project"
version = "0.1.0"
edition = "2021"

[workspace]
members = ["../secret-outside"]
"#,
        )
        .unwrap();
        fs::write(project_dir.join("src/lib.rs"), "pub fn hello() {}").unwrap();

        let result = learn_project(&store, &project_dir, None).unwrap();

        // The module concept name is the *directory* name ("secret-outside"),
        // and its description is only populated by reading the escaped
        // Cargo.toml — either is proof the traversal was followed.
        let concepts = store.list_concepts(&result.memoir_id).unwrap();
        assert!(
            !concepts.iter().any(|c| c.name == "secret-outside"),
            "workspace member outside the project root must not be learned as a module"
        );
        assert!(
            !concepts
                .iter()
                .any(|c| c.definition.contains("should never be learned")),
            "the escaped Cargo.toml's description must never have been read"
        );
    }
}
