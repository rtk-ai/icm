use std::path::{Path, PathBuf};

use crate::error::IcmResult;
use crate::memoir::{Concept, ConceptLink, Label, Memoir, Relation};
use crate::memoir_store::MemoirStore;

/// Result of learning a project.
#[derive(Debug, Clone)]
pub struct LearnResult {
    pub memoir_id: String,
    pub project_name: String,
    pub total_concepts: usize,
    pub link_count: usize,
}

impl std::fmt::Display for LearnResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Learned {}: {} concepts, {} links (memoir: {})",
            self.project_name, self.total_concepts, self.link_count, self.memoir_id
        )
    }
}

/// Learn a project by scanning its directory and creating a Memoir knowledge graph.
pub fn learn_project(
    store: &dyn MemoirStore,
    dir: &Path,
    name: Option<&str>,
) -> IcmResult<LearnResult> {
    let project_name = name
        .or_else(|| dir.file_name().and_then(|f| f.to_str()))
        .unwrap_or("project");

    // Check if memoir already exists — delete and recreate
    if let Ok(Some(existing)) = store.get_memoir_by_name(project_name) {
        store.delete_memoir(&existing.id)?;
    }

    let memoir = Memoir::new(
        project_name.to_string(),
        format!("Knowledge graph for {project_name}"),
    );
    let memoir_id = store.create_memoir(memoir)?;

    // Scan and create concepts
    let project_concept_id = scan_project_identity(store, &memoir_id, dir, project_name)?;
    let dep_ids = scan_dependencies(store, &memoir_id, dir)?;
    let module_ids = scan_modules(store, &memoir_id, dir)?;
    let entry_ids = scan_entrypoints(store, &memoir_id, dir, &module_ids)?;
    let config_ids = scan_configs(store, &memoir_id, dir)?;
    let script_ids = scan_scripts(store, &memoir_id, dir)?;

    // Create links
    let mut link_count = 0usize;

    // Modules → Project: PartOf
    for (mod_id, _) in &module_ids {
        store.add_link(ConceptLink::new(
            mod_id.clone(),
            project_concept_id.clone(),
            Relation::PartOf,
        ))?;
        link_count += 1;
    }

    // Project → Dependency: DependsOn
    for dep_id in &dep_ids {
        store.add_link(ConceptLink::new(
            project_concept_id.clone(),
            dep_id.clone(),
            Relation::DependsOn,
        ))?;
        link_count += 1;
    }

    // Entrypoint → Module: PartOf (or → Project if no matching module)
    for (entry_id, parent_module) in &entry_ids {
        let target = parent_module
            .as_ref()
            .and_then(|mod_name| {
                module_ids
                    .iter()
                    .find(|(_, n)| n == mod_name)
                    .map(|(id, _)| id.clone())
            })
            .unwrap_or_else(|| project_concept_id.clone());
        store.add_link(ConceptLink::new(entry_id.clone(), target, Relation::PartOf))?;
        link_count += 1;
    }

    // Config → Project: RelatedTo
    for cfg_id in &config_ids {
        store.add_link(ConceptLink::new(
            cfg_id.clone(),
            project_concept_id.clone(),
            Relation::RelatedTo,
        ))?;
        link_count += 1;
    }

    // Script → Project: RelatedTo
    for script_id in &script_ids {
        store.add_link(ConceptLink::new(
            script_id.clone(),
            project_concept_id.clone(),
            Relation::RelatedTo,
        ))?;
        link_count += 1;
    }

    let total_concepts = 1
        + dep_ids.len()
        + module_ids.len()
        + entry_ids.len()
        + config_ids.len()
        + script_ids.len();

    Ok(LearnResult {
        memoir_id,
        project_name: project_name.to_string(),
        total_concepts,
        link_count,
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn add_concept(
    store: &dyn MemoirStore,
    memoir_id: &str,
    name: &str,
    definition: &str,
    kind: &str,
) -> IcmResult<String> {
    let mut concept = Concept::new(
        memoir_id.to_string(),
        name.to_string(),
        definition.to_string(),
    );
    concept.labels = vec![Label::new("kind", kind)];
    concept.confidence = 1.0;
    let id = store.add_concept(concept)?;
    Ok(id)
}

// ---------------------------------------------------------------------------
// Scanners
// ---------------------------------------------------------------------------

/// Read project identity from Cargo.toml / package.json / pyproject.toml / go.mod.
fn scan_project_identity(
    store: &dyn MemoirStore,
    memoir_id: &str,
    dir: &Path,
    project_name: &str,
) -> IcmResult<String> {
    let mut description = format!("Project: {project_name}");

    if let Some(info) = read_cargo_toml(dir) {
        description = info;
    } else if let Some(info) = read_package_json(dir) {
        description = info;
    } else if let Some(info) = read_pyproject_toml(dir) {
        description = info;
    } else if let Some(info) = read_go_mod(dir) {
        description = info;
    }

    add_concept(store, memoir_id, project_name, &description, "project")
}

/// Extract top dependencies from config files.
fn scan_dependencies(
    store: &dyn MemoirStore,
    memoir_id: &str,
    dir: &Path,
) -> IcmResult<Vec<String>> {
    let mut dep_ids = Vec::new();
    let deps = collect_dependencies(dir);

    for (name, version) in deps.into_iter().take(15) {
        let def = if version.is_empty() {
            format!("Dependency: {name}")
        } else {
            format!("Dependency: {name} ({version})")
        };
        let id = add_concept(store, memoir_id, &name, &def, "dependency")?;
        dep_ids.push(id);
    }

    Ok(dep_ids)
}

/// Scan workspace members / top-level modules.
/// Returns (concept_id, module_name) pairs.
fn scan_modules(
    store: &dyn MemoirStore,
    memoir_id: &str,
    dir: &Path,
) -> IcmResult<Vec<(String, String)>> {
    let mut module_ids = Vec::new();
    let modules = collect_modules(dir);

    for (name, def) in &modules {
        let id = add_concept(store, memoir_id, name, def, "module")?;
        module_ids.push((id, name.clone()));
    }

    Ok(module_ids)
}

/// Find entry point files (main.rs, lib.rs, index.ts, etc.).
/// Returns (concept_id, Option<parent_module_name>).
fn scan_entrypoints(
    store: &dyn MemoirStore,
    memoir_id: &str,
    dir: &Path,
    modules: &[(String, String)],
) -> IcmResult<Vec<(String, Option<String>)>> {
    let mut result = Vec::new();
    let entrypoints = collect_entrypoints(dir);

    for (rel_path, parent) in &entrypoints {
        let path_str = rel_path.display().to_string();
        let def = format!("Entry point: {path_str}");

        // Resolve parent module
        let parent_module = parent
            .as_ref()
            .and_then(|p| modules.iter().find(|(_, n)| n == p).map(|(_, n)| n.clone()));

        let concept_name = path_str;
        let id = add_concept(store, memoir_id, &concept_name, &def, "entrypoint")?;
        result.push((id, parent_module));
    }

    Ok(result)
}

/// Find config/CI files.
fn scan_configs(store: &dyn MemoirStore, memoir_id: &str, dir: &Path) -> IcmResult<Vec<String>> {
    let mut ids = Vec::new();
    let configs = collect_configs(dir);

    for (rel_path, def) in &configs {
        let path_str = rel_path.display().to_string();
        let id = add_concept(store, memoir_id, &path_str, def, "config")?;
        ids.push(id);
    }

    Ok(ids)
}

/// Find scripts (Makefile, justfile, etc.).
fn scan_scripts(store: &dyn MemoirStore, memoir_id: &str, dir: &Path) -> IcmResult<Vec<String>> {
    let mut ids = Vec::new();
    let scripts = collect_scripts(dir);

    for (rel_path, def) in &scripts {
        let path_str = rel_path.display().to_string();
        let id = add_concept(store, memoir_id, &path_str, def, "script")?;
        ids.push(id);
    }

    Ok(ids)
}

// ---------------------------------------------------------------------------
// File readers
// ---------------------------------------------------------------------------

fn read_cargo_toml(dir: &Path) -> Option<String> {
    let path = dir.join("Cargo.toml");
    let content = std::fs::read_to_string(&path).ok()?;
    let parsed: toml::Value = content.parse().ok()?;

    let pkg = parsed.get("package");
    let name = pkg
        .and_then(|p| p.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let version = pkg
        .and_then(|p| p.get("version"))
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    let desc = pkg
        .and_then(|p| p.get("description"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let is_workspace = parsed.get("workspace").is_some();
    let lang = if is_workspace {
        "Rust workspace"
    } else {
        "Rust"
    };

    let mut info = format!("{lang} project: {name} v{version}");
    if !desc.is_empty() {
        info.push_str(&format!(" — {desc}"));
    }
    Some(info)
}

fn read_package_json(dir: &Path) -> Option<String> {
    let path = dir.join("package.json");
    let content = std::fs::read_to_string(&path).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&content).ok()?;

    let name = parsed
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let version = parsed
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    let desc = parsed
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let mut info = format!("Node.js project: {name} v{version}");
    if !desc.is_empty() {
        info.push_str(&format!(" — {desc}"));
    }
    Some(info)
}

fn read_pyproject_toml(dir: &Path) -> Option<String> {
    let path = dir.join("pyproject.toml");
    let content = std::fs::read_to_string(&path).ok()?;
    let parsed: toml::Value = content.parse().ok()?;

    let project = parsed
        .get("project")
        .or_else(|| parsed.get("tool").and_then(|t| t.get("poetry")))?;
    let name = project
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let version = project
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    let desc = project
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let mut info = format!("Python project: {name} v{version}");
    if !desc.is_empty() {
        info.push_str(&format!(" — {desc}"));
    }
    Some(info)
}

fn read_go_mod(dir: &Path) -> Option<String> {
    let path = dir.join("go.mod");
    let content = std::fs::read_to_string(&path).ok()?;

    let module = content
        .lines()
        .find(|l| l.starts_with("module "))
        .map(|l| l.trim_start_matches("module ").trim())?;

    Some(format!("Go project: {module}"))
}

// ---------------------------------------------------------------------------
// Dependency collection
// ---------------------------------------------------------------------------

fn collect_dependencies(dir: &Path) -> Vec<(String, String)> {
    if let Some(deps) = collect_cargo_deps(dir) {
        return deps;
    }
    if let Some(deps) = collect_npm_deps(dir) {
        return deps;
    }
    if let Some(deps) = collect_python_deps(dir) {
        return deps;
    }
    if let Some(deps) = collect_go_deps(dir) {
        return deps;
    }
    Vec::new()
}

fn collect_cargo_deps(dir: &Path) -> Option<Vec<(String, String)>> {
    let path = dir.join("Cargo.toml");
    let content = std::fs::read_to_string(&path).ok()?;
    let parsed: toml::Value = content.parse().ok()?;

    let mut deps = Vec::new();

    // Check workspace dependencies first, then package dependencies
    for section in &["workspace.dependencies", "dependencies"] {
        let table = section
            .split('.')
            .fold(Some(&parsed), |acc, key| acc.and_then(|v| v.get(key)));
        if let Some(toml::Value::Table(table)) = table {
            for (name, val) in table {
                let version = match val {
                    toml::Value::String(s) => s.clone(),
                    toml::Value::Table(t) => t
                        .get("version")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    _ => String::new(),
                };
                deps.push((name.clone(), version));
            }
        }
    }

    if deps.is_empty() {
        return None;
    }
    Some(deps)
}

fn collect_npm_deps(dir: &Path) -> Option<Vec<(String, String)>> {
    let path = dir.join("package.json");
    let content = std::fs::read_to_string(&path).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&content).ok()?;

    let mut deps = Vec::new();
    for section in &["dependencies", "devDependencies"] {
        if let Some(serde_json::Value::Object(map)) = parsed.get(*section) {
            for (name, val) in map {
                let version = val.as_str().unwrap_or("").to_string();
                deps.push((name.clone(), version));
            }
        }
    }

    if deps.is_empty() {
        return None;
    }
    Some(deps)
}

fn collect_python_deps(dir: &Path) -> Option<Vec<(String, String)>> {
    let path = dir.join("pyproject.toml");
    let content = std::fs::read_to_string(&path).ok()?;
    let parsed: toml::Value = content.parse().ok()?;

    let mut deps = Vec::new();

    // PEP 621: project.dependencies
    if let Some(dep_arr) = parsed
        .get("project")
        .and_then(|p| p.get("dependencies"))
        .and_then(|d| d.as_array())
    {
        for d in dep_arr {
            if let Some(s) = d.as_str() {
                let name = s
                    .split(|c: char| !c.is_alphanumeric() && c != '-' && c != '_')
                    .next()
                    .unwrap_or(s);
                deps.push((name.to_string(), String::new()));
            }
        }
    }

    // Poetry: tool.poetry.dependencies
    if let Some(toml::Value::Table(table)) = parsed
        .get("tool")
        .and_then(|t| t.get("poetry"))
        .and_then(|p| p.get("dependencies"))
    {
        for (name, val) in table {
            if name == "python" {
                continue;
            }
            let version = match val {
                toml::Value::String(s) => s.clone(),
                _ => String::new(),
            };
            deps.push((name.clone(), version));
        }
    }

    if deps.is_empty() {
        return None;
    }
    Some(deps)
}

fn collect_go_deps(dir: &Path) -> Option<Vec<(String, String)>> {
    let path = dir.join("go.mod");
    let content = std::fs::read_to_string(&path).ok()?;

    let mut deps = Vec::new();
    let mut in_require = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("require (") || trimmed == "require (" {
            in_require = true;
            continue;
        }
        if in_require && trimmed == ")" {
            in_require = false;
            continue;
        }
        if in_require {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 2 {
                let name = parts[0].rsplit('/').next().unwrap_or(parts[0]);
                deps.push((name.to_string(), parts[1].to_string()));
            }
        }
    }

    if deps.is_empty() {
        return None;
    }
    Some(deps)
}

// ---------------------------------------------------------------------------
// Module collection
// ---------------------------------------------------------------------------

fn collect_modules(dir: &Path) -> Vec<(String, String)> {
    // Rust workspace: read workspace members
    if let Some(modules) = collect_rust_workspace_members(dir) {
        return modules;
    }

    // Node.js workspaces
    if let Some(modules) = collect_npm_workspaces(dir) {
        return modules;
    }

    // Fallback: scan top-level dirs in src/
    collect_src_dirs(dir)
}

fn collect_rust_workspace_members(dir: &Path) -> Option<Vec<(String, String)>> {
    let path = dir.join("Cargo.toml");
    let content = std::fs::read_to_string(&path).ok()?;
    let parsed: toml::Value = content.parse().ok()?;

    let members = parsed
        .get("workspace")
        .and_then(|w| w.get("members"))
        .and_then(|m| m.as_array())?;

    let mut result = Vec::new();
    for member in members {
        if let Some(pattern) = member.as_str() {
            // Expand simple globs like "crates/*"
            let expanded = expand_workspace_glob(dir, pattern);
            for member_path in expanded {
                let name = member_path
                    .file_name()
                    .and_then(|f| f.to_str())
                    .unwrap_or("unknown")
                    .to_string();

                // Try to read the member's Cargo.toml for description
                let member_cargo = member_path.join("Cargo.toml");
                let desc = if let Ok(mc) = std::fs::read_to_string(&member_cargo) {
                    if let Ok(mp) = mc.parse::<toml::Value>() {
                        mp.get("package")
                            .and_then(|p| p.get("description"))
                            .and_then(|v| v.as_str())
                            .map(|d| format!("Rust crate: {name} — {d}"))
                            .unwrap_or_else(|| format!("Rust crate: {name}"))
                    } else {
                        format!("Rust crate: {name}")
                    }
                } else {
                    format!("Rust crate: {name}")
                };

                result.push((name, desc));
            }
        }
    }

    if result.is_empty() {
        return None;
    }
    Some(result)
}

fn expand_workspace_glob(dir: &Path, pattern: &str) -> Vec<PathBuf> {
    if pattern.contains('*') {
        // Simple glob: "crates/*" → list directories in crates/
        let prefix = pattern.trim_end_matches("/*").trim_end_matches("/*");
        let parent = dir.join(prefix);
        if let Ok(entries) = std::fs::read_dir(&parent) {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().map(|ft| ft.is_dir()).unwrap_or(false))
                .map(|e| e.path())
                .collect()
        } else {
            Vec::new()
        }
    } else {
        let full = dir.join(pattern);
        if full.is_dir() {
            vec![full]
        } else {
            Vec::new()
        }
    }
}

fn collect_npm_workspaces(dir: &Path) -> Option<Vec<(String, String)>> {
    let path = dir.join("package.json");
    let content = std::fs::read_to_string(&path).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&content).ok()?;

    let workspaces = parsed.get("workspaces").and_then(|w| w.as_array())?;

    let mut result = Vec::new();
    for ws in workspaces {
        if let Some(pattern) = ws.as_str() {
            let expanded = expand_workspace_glob(dir, pattern);
            for ws_path in expanded {
                let name = ws_path
                    .file_name()
                    .and_then(|f| f.to_str())
                    .unwrap_or("unknown")
                    .to_string();
                result.push((name.clone(), format!("Package: {name}")));
            }
        }
    }

    if result.is_empty() {
        return None;
    }
    Some(result)
}

fn collect_src_dirs(dir: &Path) -> Vec<(String, String)> {
    let src = dir.join("src");
    if !src.is_dir() {
        return Vec::new();
    }

    let mut result = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&src) {
        for entry in entries.filter_map(|e| e.ok()) {
            if entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
                let name = entry.file_name().to_str().unwrap_or("unknown").to_string();
                result.push((name.clone(), format!("Module: {name}")));
            }
        }
    }

    result
}

// ---------------------------------------------------------------------------
// Entrypoints
// ---------------------------------------------------------------------------

/// Returns (relative_path, Option<parent_module_name>).
fn collect_entrypoints(dir: &Path) -> Vec<(PathBuf, Option<String>)> {
    let entry_names = [
        "main.rs",
        "lib.rs",
        "index.ts",
        "index.js",
        "main.py",
        "__main__.py",
        "main.go",
        "mod.rs",
    ];

    let mut result = Vec::new();

    // Check root src/
    let src = dir.join("src");
    if src.is_dir() {
        for name in &entry_names {
            let p = src.join(name);
            if p.exists() {
                let rel = PathBuf::from("src").join(name);
                result.push((rel, None));
            }
        }
    }

    // Check workspace member entry points
    if let Some(modules) = collect_rust_workspace_members(dir) {
        for (mod_name, _) in &modules {
            // Try common locations for workspace members
            for prefix in &["crates", "packages", "libs", "apps", ""] {
                let base = if prefix.is_empty() {
                    dir.join(mod_name)
                } else {
                    dir.join(prefix).join(mod_name)
                };
                let src_dir = base.join("src");
                if src_dir.is_dir() {
                    for name in &entry_names {
                        if name == &"mod.rs" {
                            continue; // skip mod.rs in workspace members
                        }
                        let p = src_dir.join(name);
                        if p.exists() {
                            let rel = if prefix.is_empty() {
                                PathBuf::from(mod_name).join("src").join(name)
                            } else {
                                PathBuf::from(prefix).join(mod_name).join("src").join(name)
                            };
                            result.push((rel, Some(mod_name.clone())));
                        }
                    }
                }
            }
        }
    }

    // Check root for non-Rust entry points
    for name in &["index.ts", "index.js", "main.py", "main.go"] {
        let p = dir.join(name);
        if p.exists() {
            result.push((PathBuf::from(name), None));
        }
    }

    result
}

// ---------------------------------------------------------------------------
// Configs
// ---------------------------------------------------------------------------

fn collect_configs(dir: &Path) -> Vec<(PathBuf, String)> {
    let mut result = Vec::new();

    // GitHub workflows
    let workflows = dir.join(".github").join("workflows");
    if workflows.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&workflows) {
            for entry in entries.filter_map(|e| e.ok()) {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if name_str.ends_with(".yml") || name_str.ends_with(".yaml") {
                    let rel = PathBuf::from(".github/workflows").join(&name);
                    result.push((rel, format!("GitHub Actions workflow: {name_str}")));
                }
            }
        }
    }

    // Docker
    let docker_files = [
        ("Dockerfile", "Docker build file"),
        ("docker-compose.yml", "Docker Compose configuration"),
        ("docker-compose.yaml", "Docker Compose configuration"),
    ];
    for (file, desc) in &docker_files {
        if dir.join(file).exists() {
            result.push((PathBuf::from(file), desc.to_string()));
        }
    }

    // CI configs
    let ci_files = [
        (".gitlab-ci.yml", "GitLab CI configuration"),
        (".travis.yml", "Travis CI configuration"),
        ("Jenkinsfile", "Jenkins pipeline"),
        (".circleci/config.yml", "CircleCI configuration"),
    ];
    for (file, desc) in &ci_files {
        if dir.join(file).exists() {
            result.push((PathBuf::from(file), desc.to_string()));
        }
    }

    // Misc config files
    let config_files = [
        (".env.example", "Environment variables template"),
        ("rust-toolchain.toml", "Rust toolchain configuration"),
        ("rust-toolchain", "Rust toolchain configuration"),
        (".rustfmt.toml", "Rust formatting configuration"),
        ("clippy.toml", "Clippy lint configuration"),
        ("tsconfig.json", "TypeScript configuration"),
        (".eslintrc.json", "ESLint configuration"),
        (".prettierrc", "Prettier configuration"),
    ];
    for (file, desc) in &config_files {
        if dir.join(file).exists() {
            result.push((PathBuf::from(file), desc.to_string()));
        }
    }

    result
}

// ---------------------------------------------------------------------------
// Scripts
// ---------------------------------------------------------------------------

fn collect_scripts(dir: &Path) -> Vec<(PathBuf, String)> {
    let mut result = Vec::new();

    let script_files = [
        ("Makefile", "Makefile build system"),
        ("justfile", "Just command runner"),
        ("Justfile", "Just command runner"),
        ("Taskfile.yml", "Task runner configuration"),
        ("Rakefile", "Ruby Rake build file"),
    ];
    for (file, desc) in &script_files {
        if dir.join(file).exists() {
            result.push((PathBuf::from(file), desc.to_string()));
        }
    }

    // scripts/ directory
    let scripts_dir = dir.join("scripts");
    if scripts_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&scripts_dir) {
            for entry in entries.filter_map(|e| e.ok()) {
                let name = entry.file_name();
                let name_str = name.to_string_lossy().to_string();
                if name_str.ends_with(".sh")
                    || name_str.ends_with(".py")
                    || name_str.ends_with(".ts")
                    || name_str.ends_with(".js")
                {
                    let rel = PathBuf::from("scripts").join(&name);
                    result.push((rel, format!("Script: {name_str}")));
                }
            }
        }
    }

    result
}
