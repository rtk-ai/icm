use serde_json::{json, Value};

use icm_core::{
    build_context_snapshot_from_memories, project::project_from_path, ContextSnapshotOptions,
    MemoryStore, SnapshotFormat,
};
use icm_store::Store;

const CURRENT_CONTEXT_URI: &str = "icm://context/current";
const MAX_CONTEXT_TOKENS: usize = 1_200;

#[derive(Debug)]
pub(crate) enum ResourceError {
    NotFound(String),
    Internal(String),
}

impl ResourceError {
    pub(crate) fn message(&self) -> &str {
        match self {
            Self::NotFound(message) | Self::Internal(message) => message,
        }
    }
}

pub(crate) fn list() -> Value {
    json!({
        "resources": [{
            "uri": CURRENT_CONTEXT_URI,
            "name": "current-project-context",
            "title": "Current project context",
            "description": "Bounded ICM context for the project in the server's working directory",
            "mimeType": "text/markdown",
            "annotations": {
                "audience": ["assistant"]
            }
        }]
    })
}

pub(crate) fn templates() -> Value {
    json!({"resourceTemplates": []})
}

pub(crate) fn read(store: &Store, uri: &str) -> Result<Value, ResourceError> {
    if uri != CURRENT_CONTEXT_URI {
        return Err(ResourceError::NotFound(format!(
            "resource not found: {uri}"
        )));
    }

    let project = std::env::current_dir()
        .ok()
        .and_then(|path| project_from_path(&path.to_string_lossy()))
        .ok_or_else(|| ResourceError::Internal("could not determine the current project".into()))?;

    read_for_project(store, uri, &project)
}

fn read_for_project(store: &Store, uri: &str, project: &str) -> Result<Value, ResourceError> {
    let topics = [
        project.to_owned(),
        format!("context-{project}"),
        format!("contexte-{project}"),
    ];
    let mut memories = Vec::new();
    for topic in topics {
        memories.extend(
            store
                .get_by_topic(&topic)
                .map_err(|error| ResourceError::Internal(error.to_string()))?,
        );
    }

    let options = ContextSnapshotOptions {
        project: Some(project),
        max_tokens: MAX_CONTEXT_TOKENS,
        format: SnapshotFormat::Markdown,
    };
    let snapshot = build_context_snapshot_from_memories(memories, &options);
    let text = snapshot.render(options.format);

    Ok(json!({
        "contents": [{
            "uri": uri,
            "mimeType": "text/markdown",
            "text": text
        }]
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use icm_core::{Importance, Memory};

    fn test_store() -> Store {
        Store::in_memory().unwrap()
    }

    #[test]
    fn lists_only_the_current_project_resource() {
        let resources = list();
        assert_eq!(resources["resources"].as_array().unwrap().len(), 1);
        assert_eq!(resources["resources"][0]["uri"], CURRENT_CONTEXT_URI);
        assert!(templates()["resourceTemplates"]
            .as_array()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn reads_bounded_context_without_other_projects() {
        let store = test_store();
        store
            .store(Memory::new(
                "context-icm".into(),
                "ICM-only context".into(),
                Importance::High,
            ))
            .unwrap();
        store
            .store(Memory::new(
                "context-other".into(),
                "other-project secret".into(),
                Importance::Critical,
            ))
            .unwrap();
        store
            .store(Memory::new(
                "preferences".into(),
                "global preference".into(),
                Importance::Critical,
            ))
            .unwrap();
        store
            .store(Memory::new(
                "icm".into(),
                "bare project context".into(),
                Importance::Medium,
            ))
            .unwrap();
        store
            .store(Memory::new(
                "contexte-icm".into(),
                "localized project context".into(),
                Importance::Medium,
            ))
            .unwrap();

        let result = read_for_project(&store, CURRENT_CONTEXT_URI, "icm").unwrap();
        let text = result["contents"][0]["text"].as_str().unwrap();
        assert!(text.contains("ICM-only context"));
        assert!(text.contains("bare project context"));
        assert!(text.contains("localized project context"));
        assert!(!text.contains("other-project secret"));
        assert!(!text.contains("global preference"));
        assert!(text.len() <= MAX_CONTEXT_TOKENS * 4);
    }

    #[test]
    fn truncates_large_project_context_to_the_resource_budget() {
        let store = test_store();
        for index in 0..4 {
            store
                .store(Memory::new(
                    "context-icm".into(),
                    format!("{index}: {}", "x".repeat(MAX_CONTEXT_TOKENS * 2 - 200)),
                    Importance::High,
                ))
                .unwrap();
        }

        let result = read_for_project(&store, CURRENT_CONTEXT_URI, "icm").unwrap();
        let text = result["contents"][0]["text"].as_str().unwrap();
        assert!(text.len() <= MAX_CONTEXT_TOKENS * 4);
        assert!(text.contains("entries dropped"));
    }

    #[test]
    fn empty_project_context_is_an_empty_resource() {
        let result = read_for_project(&test_store(), CURRENT_CONTEXT_URI, "icm").unwrap();
        assert_eq!(result["contents"][0]["text"], "");
    }

    #[test]
    fn rejects_every_other_uri() {
        assert!(matches!(
            read(&test_store(), "icm://context/other"),
            Err(ResourceError::NotFound(_))
        ));
    }
}
