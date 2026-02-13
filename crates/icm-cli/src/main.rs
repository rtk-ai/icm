mod bench_data;
mod bench_knowledge;
mod config;
mod extract;

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use serde_json::Value;

use icm_core::{
    Concept, ConceptLink, Importance, Label, Memoir, MemoirStore, Memory, MemoryStore, Relation,
};
use icm_store::SqliteStore;

#[derive(Parser)]
#[command(
    name = "icm",
    version,
    about = "Infinite Context Memory - persistent memory for LLMs"
)]
struct Cli {
    /// Path to the SQLite database
    #[arg(long, global = true)]
    db: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Store a new memory
    Store {
        /// Topic/category
        #[arg(short, long)]
        topic: String,

        /// Content to memorize
        #[arg(short, long)]
        content: String,

        /// Importance level
        #[arg(short, long, default_value = "medium")]
        importance: CliImportance,

        /// Keywords (comma-separated)
        #[arg(short, long)]
        keywords: Option<String>,

        /// Raw excerpt (verbatim code, error message, etc.)
        #[arg(short, long)]
        raw: Option<String>,
    },

    /// Search memories
    Recall {
        /// Search query
        query: String,

        /// Filter by topic
        #[arg(short, long)]
        topic: Option<String>,

        /// Maximum results
        #[arg(short, long, default_value = "5")]
        limit: usize,

        /// Filter results by keyword
        #[arg(short = 'k', long)]
        keyword: Option<String>,
    },

    /// List memories
    List {
        /// Filter by topic
        #[arg(short, long)]
        topic: Option<String>,

        /// Show all memories
        #[arg(short, long)]
        all: bool,

        /// Sort by field
        #[arg(short, long, default_value = "weight")]
        sort: SortField,
    },

    /// Delete a memory by ID
    Forget {
        /// Memory ID
        id: String,
    },

    /// List all topics
    Topics,

    /// Show global statistics
    Stats,

    /// Apply temporal decay to memory weights
    Decay {
        /// Decay factor (default: 0.95)
        #[arg(short, long, default_value = "0.95")]
        factor: f32,
    },

    /// Prune low-weight memories
    Prune {
        /// Weight threshold (memories below this are deleted)
        #[arg(short, long, default_value = "0.1")]
        threshold: f32,

        /// Preview without deleting
        #[arg(long)]
        dry_run: bool,
    },

    /// Consolidate all memories of a topic into a single summary
    Consolidate {
        /// Topic to consolidate
        #[arg(short, long)]
        topic: String,

        /// Keep original memories after consolidation
        #[arg(long)]
        keep_originals: bool,
    },

    /// Generate embeddings for memories that don't have one yet
    Embed {
        /// Only embed memories in this topic
        #[arg(short, long)]
        topic: Option<String>,

        /// Re-embed memories that already have embeddings
        #[arg(long)]
        force: bool,

        /// Batch size for embedding
        #[arg(short, long, default_value = "32")]
        batch_size: usize,
    },

    /// Memoir commands — permanent knowledge layer
    Memoir {
        #[command(subcommand)]
        command: MemoirCommands,
    },

    /// Configure ICM integration for Claude Code / Claude Desktop
    Init {
        /// Integration mode: mcp, cli, skill, or all (default: mcp)
        #[arg(short, long, default_value = "mcp")]
        mode: InitMode,
    },

    /// Run performance benchmark on in-memory store
    Bench {
        /// Number of memories to seed
        #[arg(short, long, default_value = "1000")]
        count: usize,
    },

    /// Extract facts from text and store in ICM (rule-based, zero LLM cost)
    Extract {
        /// Project name for topic namespacing
        #[arg(short, long, default_value = "project")]
        project: String,

        /// Text to extract from (reads stdin if omitted)
        #[arg(short, long)]
        text: Option<String>,

        /// Don't store, just print extracted facts
        #[arg(long)]
        dry_run: bool,
    },

    /// Output recalled context formatted for prompt injection
    RecallContext {
        /// Search query for relevant context
        query: String,

        /// Maximum memories to include
        #[arg(short, long, default_value = "10")]
        limit: usize,
    },

    /// Benchmark memory recall accuracy with and without ICM
    BenchRecall {
        /// Model to use
        #[arg(short, long, default_value = "sonnet")]
        model: String,

        /// Number of runs to average
        #[arg(short, long, default_value = "1")]
        runs: usize,

        /// Show injected context before each question
        #[arg(short, long)]
        verbose: bool,
    },

    /// Benchmark Claude Code efficiency with and without ICM
    BenchAgent {
        /// Number of sessions per mode
        #[arg(short, long, default_value = "10")]
        sessions: usize,

        /// Model to use
        #[arg(short, long, default_value = "sonnet")]
        model: String,

        /// Number of runs to average
        #[arg(short, long, default_value = "1")]
        runs: usize,

        /// Show extracted facts and injected context
        #[arg(short, long)]
        verbose: bool,
    },

    /// Show current configuration
    Config,

    /// Launch MCP server (stdio transport for Claude Code)
    Serve,
}

#[derive(Subcommand)]
enum MemoirCommands {
    /// Create a new memoir
    Create {
        /// Unique name for the memoir
        #[arg(short, long)]
        name: String,

        /// Description of the memoir
        #[arg(short, long, default_value = "")]
        description: String,
    },

    /// List all memoirs
    List,

    /// Show memoir stats and concept count
    Show {
        /// Memoir name
        name: String,
    },

    /// Delete a memoir and all its concepts/links
    Delete {
        /// Memoir name
        name: String,
    },

    /// Add a concept to a memoir
    AddConcept {
        /// Memoir name
        #[arg(short, long)]
        memoir: String,

        /// Concept name (unique within memoir)
        #[arg(short, long)]
        name: String,

        /// Dense definition of the concept
        #[arg(short, long)]
        definition: String,

        /// Labels (comma-separated, namespace:value or plain tag)
        #[arg(short, long)]
        labels: Option<String>,
    },

    /// Refine an existing concept with a new definition
    Refine {
        /// Memoir name
        #[arg(short, long)]
        memoir: String,

        /// Concept name
        #[arg(short, long)]
        name: String,

        /// New definition
        #[arg(short, long)]
        definition: String,
    },

    /// Search concepts via full-text search
    Search {
        /// Memoir name
        #[arg(short, long)]
        memoir: String,

        /// Search query
        query: String,

        /// Filter by label (e.g. "domain:tech")
        #[arg(short = 'L', long)]
        label: Option<String>,

        /// Maximum results
        #[arg(short, long, default_value = "10")]
        limit: usize,
    },

    /// Search concepts across all memoirs
    SearchAll {
        /// Search query
        query: String,

        /// Maximum results
        #[arg(short, long, default_value = "10")]
        limit: usize,
    },

    /// Add a directed link between two concepts
    Link {
        /// Memoir name
        #[arg(short, long)]
        memoir: String,

        /// Source concept name
        #[arg(long)]
        from: String,

        /// Target concept name
        #[arg(long)]
        to: String,

        /// Relation type
        #[arg(short, long)]
        relation: CliRelation,
    },

    /// Inspect a concept and its graph neighbors
    Inspect {
        /// Memoir name
        #[arg(short, long)]
        memoir: String,

        /// Concept name
        name: String,

        /// BFS depth for neighborhood exploration
        #[arg(short = 'D', long, default_value = "1")]
        depth: usize,
    },

    /// Distill memories from a topic into concepts in a memoir
    Distill {
        /// Source memory topic
        #[arg(long)]
        from_topic: String,

        /// Target memoir name
        #[arg(long)]
        into: String,
    },
}

#[derive(Clone, ValueEnum)]
enum CliImportance {
    Critical,
    High,
    Medium,
    Low,
}

impl From<CliImportance> for Importance {
    fn from(val: CliImportance) -> Self {
        match val {
            CliImportance::Critical => Importance::Critical,
            CliImportance::High => Importance::High,
            CliImportance::Medium => Importance::Medium,
            CliImportance::Low => Importance::Low,
        }
    }
}

#[derive(Clone, ValueEnum)]
enum CliRelation {
    PartOf,
    DependsOn,
    RelatedTo,
    Contradicts,
    Refines,
    AlternativeTo,
    CausedBy,
    InstanceOf,
    SupersededBy,
}

impl From<CliRelation> for Relation {
    fn from(val: CliRelation) -> Self {
        match val {
            CliRelation::PartOf => Relation::PartOf,
            CliRelation::DependsOn => Relation::DependsOn,
            CliRelation::RelatedTo => Relation::RelatedTo,
            CliRelation::Contradicts => Relation::Contradicts,
            CliRelation::Refines => Relation::Refines,
            CliRelation::AlternativeTo => Relation::AlternativeTo,
            CliRelation::CausedBy => Relation::CausedBy,
            CliRelation::InstanceOf => Relation::InstanceOf,
            CliRelation::SupersededBy => Relation::SupersededBy,
        }
    }
}

#[derive(Clone, ValueEnum)]
enum SortField {
    Weight,
    Created,
    Accessed,
}

#[derive(Clone, ValueEnum)]
enum InitMode {
    /// MCP server plugin (Claude calls icm tools natively)
    Mcp,
    /// CLAUDE.md instructions (Claude calls icm via Bash)
    Cli,
    /// Claude Code slash commands /recall and /remember
    Skill,
    /// All integration modes
    All,
}

fn default_db_path() -> PathBuf {
    directories::ProjectDirs::from("dev", "icm", "icm")
        .map(|dirs| dirs.data_dir().join("memories.db"))
        .unwrap_or_else(|| PathBuf::from("memories.db"))
}

fn open_store(db: Option<PathBuf>) -> Result<SqliteStore> {
    let path = db.unwrap_or_else(default_db_path);
    SqliteStore::new(&path).context("failed to open database")
}

#[cfg(feature = "embeddings")]
fn init_embedder() -> Option<icm_core::FastEmbedder> {
    Some(icm_core::FastEmbedder::new())
}

#[cfg(not(feature = "embeddings"))]
fn init_embedder() -> Option<()> {
    None
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing_subscriber::filter::LevelFilter::WARN.into()),
        )
        .init();

    let cli = Cli::parse();
    let store = open_store(cli.db)?;
    #[allow(unused_variables)]
    let embedder = init_embedder();

    match cli.command {
        Commands::Store {
            topic,
            content,
            importance,
            keywords,
            raw,
        } => {
            #[cfg(feature = "embeddings")]
            let emb_ref = embedder.as_ref().map(|e| e as &dyn icm_core::Embedder);
            #[cfg(not(feature = "embeddings"))]
            let emb_ref: Option<&dyn icm_core::Embedder> = None;
            cmd_store(
                &store,
                emb_ref,
                topic,
                content,
                importance.into(),
                keywords,
                raw,
            )
        }
        Commands::Recall {
            query,
            topic,
            limit,
            keyword,
        } => {
            #[cfg(feature = "embeddings")]
            let emb_ref = embedder.as_ref().map(|e| e as &dyn icm_core::Embedder);
            #[cfg(not(feature = "embeddings"))]
            let emb_ref: Option<&dyn icm_core::Embedder> = None;
            cmd_recall(
                &store,
                emb_ref,
                &query,
                topic.as_deref(),
                limit,
                keyword.as_deref(),
            )
        }
        Commands::List { topic, all, sort } => cmd_list(&store, topic.as_deref(), all, sort),
        Commands::Forget { id } => cmd_forget(&store, &id),
        Commands::Topics => cmd_topics(&store),
        Commands::Stats => cmd_stats(&store),
        Commands::Decay { factor } => cmd_decay(&store, factor),
        Commands::Prune { threshold, dry_run } => cmd_prune(&store, threshold, dry_run),
        Commands::Consolidate {
            topic,
            keep_originals,
        } => cmd_consolidate(&store, &topic, keep_originals),
        Commands::Embed {
            topic,
            force,
            batch_size,
        } => {
            #[cfg(feature = "embeddings")]
            {
                let emb = embedder.as_ref().expect("embeddings feature enabled");
                cmd_embed(&store, emb, topic.as_deref(), force, batch_size)
            }
            #[cfg(not(feature = "embeddings"))]
            {
                let _ = (topic, force, batch_size);
                bail!("embeddings feature not enabled — rebuild with `--features embeddings`")
            }
        }
        Commands::Memoir { command } => match command {
            MemoirCommands::Create { name, description } => {
                cmd_memoir_create(&store, name, description)
            }
            MemoirCommands::List => cmd_memoir_list(&store),
            MemoirCommands::Show { name } => cmd_memoir_show(&store, &name),
            MemoirCommands::Delete { name } => cmd_memoir_delete(&store, &name),
            MemoirCommands::AddConcept {
                memoir,
                name,
                definition,
                labels,
            } => cmd_memoir_add_concept(&store, &memoir, name, definition, labels),
            MemoirCommands::Refine {
                memoir,
                name,
                definition,
            } => cmd_memoir_refine(&store, &memoir, &name, &definition),
            MemoirCommands::Search {
                memoir,
                query,
                label,
                limit,
            } => cmd_memoir_search(&store, &memoir, &query, label.as_deref(), limit),
            MemoirCommands::SearchAll { query, limit } => {
                cmd_memoir_search_all(&store, &query, limit)
            }
            MemoirCommands::Link {
                memoir,
                from,
                to,
                relation,
            } => cmd_memoir_link(&store, &memoir, &from, &to, relation.into()),
            MemoirCommands::Inspect {
                memoir,
                name,
                depth,
            } => cmd_memoir_inspect(&store, &memoir, &name, depth),
            MemoirCommands::Distill { from_topic, into } => {
                cmd_memoir_distill(&store, &from_topic, &into)
            }
        },
        Commands::Init { mode } => cmd_init(mode),
        Commands::Extract {
            project,
            text,
            dry_run,
        } => cmd_extract(&store, &project, text, dry_run),
        Commands::RecallContext { query, limit } => cmd_recall_context(&store, &query, limit),
        Commands::Config => cmd_config(),
        Commands::Bench { count } => cmd_bench(count),
        Commands::BenchRecall {
            model,
            runs,
            verbose,
        } => cmd_bench_recall(&model, runs, verbose),
        Commands::BenchAgent {
            sessions,
            model,
            runs,
            verbose,
        } => cmd_bench_agent(sessions, &model, runs, verbose),
        Commands::Serve => {
            #[cfg(feature = "embeddings")]
            let emb_ref = embedder.as_ref().map(|e| e as &dyn icm_core::Embedder);
            #[cfg(not(feature = "embeddings"))]
            let emb_ref: Option<&dyn icm_core::Embedder> = None;
            icm_mcp::run_server(&store, emb_ref)
        }
    }
}

// ---------------------------------------------------------------------------
// Memory commands
// ---------------------------------------------------------------------------

fn cmd_store(
    store: &SqliteStore,
    embedder: Option<&dyn icm_core::Embedder>,
    topic: String,
    content: String,
    importance: Importance,
    keywords: Option<String>,
    raw: Option<String>,
) -> Result<()> {
    let mut memory = Memory::new(topic.clone(), content.clone(), importance);
    if let Some(kw) = keywords {
        memory.keywords = kw.split(',').map(|s| s.trim().to_string()).collect();
    }
    memory.raw_excerpt = raw;

    // Auto-embed if embedder is available
    if let Some(emb) = embedder {
        let text = format!("{topic} {content}");
        match emb.embed(&text) {
            Ok(vec) => memory.embedding = Some(vec),
            Err(e) => eprintln!("warning: embedding failed: {e}"),
        }
    }

    let id = store.store(memory)?;
    println!("Stored: {id}");
    Ok(())
}

fn cmd_recall(
    store: &SqliteStore,
    embedder: Option<&dyn icm_core::Embedder>,
    query: &str,
    topic: Option<&str>,
    limit: usize,
    keyword: Option<&str>,
) -> Result<()> {
    // Auto-decay if >24h since last decay
    let _ = store.maybe_auto_decay();

    // Try hybrid search if embedder is available
    if let Some(emb) = embedder {
        if let Ok(query_emb) = emb.embed(query) {
            if let Ok(results) = store.search_hybrid(query, &query_emb, limit) {
                let mut scored = results;
                if let Some(t) = topic {
                    scored.retain(|(m, _)| m.topic == t);
                }
                if let Some(kw) = keyword {
                    scored.retain(|(m, _)| m.keywords.iter().any(|k| k.contains(kw)));
                }

                if scored.is_empty() {
                    println!("No memories found.");
                    return Ok(());
                }

                for (mem, score) in &scored {
                    let _ = store.update_access(&mem.id);
                    print_memory_scored(mem, *score);
                }
                return Ok(());
            }
        }
    }

    // Fallback: FTS then keywords
    let mut results = store.search_fts(query, limit)?;

    if results.is_empty() {
        let keywords: Vec<&str> = query.split_whitespace().collect();
        results = store.search_by_keywords(&keywords, limit)?;
    }

    if let Some(t) = topic {
        results.retain(|m| m.topic == t);
    }
    if let Some(kw) = keyword {
        results.retain(|m| m.keywords.iter().any(|k| k.contains(kw)));
    }

    if results.is_empty() {
        println!("No memories found.");
        return Ok(());
    }

    for mem in &results {
        let _ = store.update_access(&mem.id);
        print_memory(mem);
    }

    Ok(())
}

fn cmd_list(store: &SqliteStore, topic: Option<&str>, all: bool, sort: SortField) -> Result<()> {
    let mut memories = if let Some(t) = topic {
        store.get_by_topic(t)?
    } else if all {
        let topics = store.list_topics()?;
        let mut all_mems = Vec::new();
        for (t, _) in &topics {
            all_mems.extend(store.get_by_topic(t)?);
        }
        all_mems
    } else {
        println!("Use --topic <name> or --all to list memories.");
        return Ok(());
    };

    match sort {
        SortField::Weight => memories.sort_by(|a, b| b.weight.partial_cmp(&a.weight).unwrap()),
        SortField::Created => memories.sort_by(|a, b| b.created_at.cmp(&a.created_at)),
        SortField::Accessed => memories.sort_by(|a, b| b.last_accessed.cmp(&a.last_accessed)),
    }

    if memories.is_empty() {
        println!("No memories found.");
        return Ok(());
    }

    for mem in &memories {
        print_memory(mem);
    }

    Ok(())
}

fn cmd_forget(store: &SqliteStore, id: &str) -> Result<()> {
    store.delete(id)?;
    println!("Deleted: {id}");
    Ok(())
}

fn cmd_topics(store: &SqliteStore) -> Result<()> {
    let topics = store.list_topics()?;
    if topics.is_empty() {
        println!("No topics yet.");
        return Ok(());
    }

    println!("{:<30} Count", "Topic");
    println!("{}", "-".repeat(40));
    for (topic, count) in &topics {
        println!("{topic:<30} {count}");
    }
    Ok(())
}

fn cmd_stats(store: &SqliteStore) -> Result<()> {
    let stats = store.stats()?;
    println!("Memories:  {}", stats.total_memories);
    println!("Topics:    {}", stats.total_topics);
    println!("Avg weight: {:.3}", stats.avg_weight);
    if let Some(oldest) = stats.oldest_memory {
        println!("Oldest:    {}", oldest.format("%Y-%m-%d %H:%M"));
    }
    if let Some(newest) = stats.newest_memory {
        println!("Newest:    {}", newest.format("%Y-%m-%d %H:%M"));
    }
    Ok(())
}

fn cmd_decay(store: &SqliteStore, factor: f32) -> Result<()> {
    let affected = store.apply_decay(factor)?;
    println!("Decay applied (factor={factor}) to {affected} memories.");
    Ok(())
}

fn cmd_prune(store: &SqliteStore, threshold: f32, dry_run: bool) -> Result<()> {
    if dry_run {
        let topics = store.list_topics()?;
        let mut count = 0;
        for (t, _) in &topics {
            for mem in store.get_by_topic(t)? {
                if mem.weight < threshold && mem.importance != Importance::Critical {
                    count += 1;
                    println!(
                        "  [dry-run] would prune: {} ({}, weight={:.3})",
                        mem.id, mem.topic, mem.weight
                    );
                }
            }
        }
        println!("Would prune {count} memories (threshold={threshold}).");
    } else {
        let pruned = store.prune(threshold)?;
        println!("Pruned {pruned} memories (threshold={threshold}).");
    }
    Ok(())
}

fn cmd_init(mode: InitMode) -> Result<()> {
    let icm_bin = std::env::current_exe().context("cannot determine icm binary path")?;
    let icm_bin_str = icm_bin.to_string_lossy().to_string();
    let home = std::env::var("HOME").context("HOME not set")?;

    let do_mcp = matches!(mode, InitMode::Mcp | InitMode::All);
    let do_cli = matches!(mode, InitMode::Cli | InitMode::All);
    let do_skill = matches!(mode, InitMode::Skill | InitMode::All);

    // --- MCP mode: configure MCP servers for all detected tools ---
    if do_mcp {
        let icm_server_entry = serde_json::json!({
            "command": icm_bin_str,
            "args": ["serve"],
            "env": {}
        });

        // Tool configs: (name, config_path)
        let tools: Vec<(&str, PathBuf)> = vec![
            ("Claude Code", PathBuf::from(&home).join(".claude.json")),
            (
                "Claude Desktop",
                PathBuf::from(&home)
                    .join("Library/Application Support/Claude/claude_desktop_config.json"),
            ),
            ("Cursor", PathBuf::from(&home).join(".cursor/mcp.json")),
            (
                "Windsurf",
                PathBuf::from(&home).join(".codeium/windsurf/mcp_config.json"),
            ),
        ];

        for (name, config_path) in &tools {
            let status = inject_mcp_server(config_path, "icm", &icm_server_entry)?;
            println!("[mcp] {name:<16} {status}");
        }
    }

    // --- CLI mode: inject CLAUDE.md instructions ---
    if do_cli {
        let cwd = std::env::current_dir().context("failed to get current directory")?;
        let claude_md_path = cwd.join("CLAUDE.md");

        let icm_block = "\
<!-- icm:start -->\n\
## Persistent memory (ICM)\n\
\n\
This project uses [ICM](https://github.com/rtk-ai/icm) for persistent memory.\n\
\n\
After completing a significant task, store a summary:\n\
```bash\n\
icm store -t \"project\" -c \"Short summary of what was done\"\n\
```\n\
\n\
Before starting work, recall relevant context:\n\
```bash\n\
icm recall \"topic keywords\"\n\
```\n\
<!-- icm:end -->";

        if claude_md_path.exists() {
            let content =
                std::fs::read_to_string(&claude_md_path).context("failed to read CLAUDE.md")?;
            if content.contains("<!-- icm:start -->") {
                println!("[cli] CLAUDE.md already configured.");
            } else {
                let new_content = format!("{}\n\n{}\n", content.trim_end(), icm_block);
                std::fs::write(&claude_md_path, new_content)
                    .context("failed to write CLAUDE.md")?;
                println!("[cli] CLAUDE.md configured with ICM instructions.");
            }
        } else {
            std::fs::write(&claude_md_path, format!("{icm_block}\n"))
                .context("failed to create CLAUDE.md")?;
            println!("[cli] CLAUDE.md created with ICM instructions.");
        }
    }

    // --- Skill mode: create /recall and /remember slash commands ---
    if do_skill {
        let skills_dir = PathBuf::from(&home).join(".claude/commands");
        std::fs::create_dir_all(&skills_dir).ok();

        let recall_path = skills_dir.join("recall.md");
        if recall_path.exists() {
            println!("[skill] /recall already configured.");
        } else {
            std::fs::write(
                &recall_path,
                "Search ICM memory for: $ARGUMENTS\n\
                 \n\
                 Use the icm_memory_recall MCP tool if available, otherwise run:\n\
                 ```bash\n\
                 icm recall \"$ARGUMENTS\"\n\
                 ```\n",
            )
            .context("cannot write recall skill")?;
            println!("[skill] /recall command created.");
        }

        let remember_path = skills_dir.join("remember.md");
        if remember_path.exists() {
            println!("[skill] /remember already configured.");
        } else {
            std::fs::write(
                &remember_path,
                "Store the following in ICM memory: $ARGUMENTS\n\
                 \n\
                 Use the icm_memory_store MCP tool if available, otherwise run:\n\
                 ```bash\n\
                 icm store -t \"note\" -c \"$ARGUMENTS\"\n\
                 ```\n",
            )
            .context("cannot write remember skill")?;
            println!("[skill] /remember command created.");
        }
    }

    println!();
    println!("  binary: {icm_bin_str}");
    println!("  db:     {}", default_db_path().display());
    println!();
    println!("Restart Claude Code / Claude Desktop to activate.");

    Ok(())
}

/// Inject ICM MCP server into a Claude config file. Returns a status string.
fn inject_mcp_server(config_path: &PathBuf, name: &str, entry: &Value) -> Result<String> {
    // Read existing config or create empty object
    let mut config: Value = if config_path.exists() {
        let content = std::fs::read_to_string(config_path)
            .with_context(|| format!("cannot read {}", config_path.display()))?;
        serde_json::from_str(&content)
            .with_context(|| format!("invalid JSON in {}", config_path.display()))?
    } else {
        // Create parent dirs if needed
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        serde_json::json!({})
    };

    let mcp_servers = config
        .as_object_mut()
        .context("config is not a JSON object")?
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}));

    // Check if already configured with same binary
    if let Some(existing) = mcp_servers.get(name) {
        if existing.get("command").and_then(|v| v.as_str())
            == entry.get("command").and_then(|v| v.as_str())
        {
            return Ok("already configured".into());
        }
    }

    mcp_servers
        .as_object_mut()
        .unwrap()
        .insert(name.to_string(), entry.clone());

    let output = serde_json::to_string_pretty(&config)?;
    std::fs::write(config_path, output)
        .with_context(|| format!("cannot write {}", config_path.display()))?;

    Ok("configured".into())
}

fn cmd_config() -> Result<()> {
    let cfg = config::load_config()?;
    println!("Config: {}", config::show_config_path());
    println!();
    println!("[store]");
    println!(
        "  path = {}",
        cfg.store
            .path
            .as_deref()
            .unwrap_or("(default platform path)")
    );
    println!();
    println!("[memory]");
    println!("  default_importance = {}", cfg.memory.default_importance);
    println!("  decay_rate = {}", cfg.memory.decay_rate);
    println!("  prune_threshold = {}", cfg.memory.prune_threshold);
    println!();
    println!("[extraction]");
    println!("  enabled = {}", cfg.extraction.enabled);
    println!("  min_score = {}", cfg.extraction.min_score);
    println!("  max_facts = {}", cfg.extraction.max_facts);
    println!();
    println!("[recall]");
    println!("  enabled = {}", cfg.recall.enabled);
    println!("  limit = {}", cfg.recall.limit);
    println!();
    println!("[mcp]");
    println!("  transport = {}", cfg.mcp.transport);
    if let Some(ref instr) = cfg.mcp.instructions {
        println!("  instructions = {instr}");
    }
    Ok(())
}

fn cmd_consolidate(store: &SqliteStore, topic: &str, keep_originals: bool) -> Result<()> {
    let memories = store.get_by_topic(topic)?;
    if memories.is_empty() {
        bail!("no memories found in topic: {topic}");
    }

    let summaries: Vec<&str> = memories.iter().map(|m| m.summary.as_str()).collect();
    let merged_summary = summaries.join(" | ");

    let mut all_keywords: Vec<String> = Vec::new();
    for mem in &memories {
        for kw in &mem.keywords {
            if !all_keywords.contains(kw) {
                all_keywords.push(kw.clone());
            }
        }
    }

    let best_importance = memories
        .iter()
        .map(|m| &m.importance)
        .min_by_key(|i| match i {
            Importance::Critical => 0,
            Importance::High => 1,
            Importance::Medium => 2,
            Importance::Low => 3,
        })
        .cloned()
        .unwrap_or(Importance::Medium);

    let mut consolidated = Memory::new(topic.to_string(), merged_summary, best_importance);
    consolidated.keywords = all_keywords;

    if keep_originals {
        let id = store.store(consolidated)?;
        println!(
            "Consolidated {} memories from '{topic}' into {id} (originals kept).",
            memories.len()
        );
    } else {
        store.consolidate_topic(topic, consolidated)?;
        println!(
            "Consolidated {} memories from '{topic}' into 1 (originals removed).",
            memories.len()
        );
    }
    Ok(())
}

fn cmd_extract(
    store: &SqliteStore,
    project: &str,
    text: Option<String>,
    dry_run: bool,
) -> Result<()> {
    let input = match text {
        Some(t) => t,
        None => {
            use std::io::Read;
            let mut buf = String::new();
            std::io::stdin()
                .read_to_string(&mut buf)
                .context("failed to read stdin")?;
            buf
        }
    };

    if dry_run {
        // Just show what would be extracted
        let facts = extract::extract_facts_public(&input, project);
        if facts.is_empty() {
            println!("No facts extracted.");
        } else {
            println!("Would extract {} facts:", facts.len());
            for (topic, content, importance) in &facts {
                println!("  [{importance}] ({topic}) {content}");
            }
        }
    } else {
        let stored = extract::extract_and_store(store, &input, project)?;
        println!("Extracted and stored {stored} facts.");
    }
    Ok(())
}

fn cmd_recall_context(store: &SqliteStore, query: &str, limit: usize) -> Result<()> {
    let ctx = extract::recall_context(store, query, limit)?;
    if ctx.is_empty() {
        eprintln!("No relevant context found.");
    } else {
        print!("{ctx}");
    }
    Ok(())
}

#[cfg(feature = "embeddings")]
fn cmd_embed(
    store: &SqliteStore,
    embedder: &dyn icm_core::Embedder,
    topic: Option<&str>,
    force: bool,
    batch_size: usize,
) -> Result<()> {
    let memories = if let Some(t) = topic {
        store.get_by_topic(t)?
    } else {
        let topics = store.list_topics()?;
        let mut all = Vec::new();
        for (t, _) in &topics {
            all.extend(store.get_by_topic(t)?);
        }
        all
    };

    let to_embed: Vec<&Memory> = if force {
        memories.iter().collect()
    } else {
        memories.iter().filter(|m| m.embedding.is_none()).collect()
    };

    if to_embed.is_empty() {
        println!("All memories already have embeddings.");
        return Ok(());
    }

    let total = to_embed.len();
    println!("Embedding {total} memories (batch_size={batch_size})...");

    let mut embedded = 0;
    let mut errors = 0;

    for chunk in to_embed.chunks(batch_size) {
        let texts: Vec<String> = chunk
            .iter()
            .map(|m| format!("{} {}", m.topic, m.summary))
            .collect();
        let text_refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();

        match embedder.embed_batch(&text_refs) {
            Ok(embeddings) => {
                for (mem, emb) in chunk.iter().zip(embeddings) {
                    let mut updated = (*mem).clone();
                    updated.embedding = Some(emb);
                    if store.update(&updated).is_ok() {
                        embedded += 1;
                    } else {
                        errors += 1;
                    }
                }
            }
            Err(e) => {
                eprintln!("batch embedding error: {e}");
                errors += chunk.len();
            }
        }

        if embedded % 100 == 0 && embedded > 0 {
            println!("  {embedded}/{total} done...");
        }
    }

    println!("Embedded {embedded}/{total} memories ({errors} errors).");
    Ok(())
}

fn print_memory(mem: &Memory) {
    println!("--- {} ---", mem.id);
    println!("  topic:      {}", mem.topic);
    println!("  importance: {}", mem.importance);
    println!("  weight:     {:.3}", mem.weight);
    println!("  created:    {}", mem.created_at.format("%Y-%m-%d %H:%M"));
    println!(
        "  accessed:   {} (x{})",
        mem.last_accessed.format("%Y-%m-%d %H:%M"),
        mem.access_count
    );
    println!("  summary:    {}", mem.summary);
    if !mem.keywords.is_empty() {
        println!("  keywords:   {}", mem.keywords.join(", "));
    }
    if let Some(ref raw) = mem.raw_excerpt {
        println!("  raw:        {raw}");
    }
    if mem.embedding.is_some() {
        println!("  embedding:  yes");
    }
    println!();
}

fn print_memory_scored(mem: &Memory, score: f32) {
    println!("--- {} [score: {:.3}] ---", mem.id, score);
    println!("  topic:      {}", mem.topic);
    println!("  importance: {}", mem.importance);
    println!("  weight:     {:.3}", mem.weight);
    println!("  created:    {}", mem.created_at.format("%Y-%m-%d %H:%M"));
    println!(
        "  accessed:   {} (x{})",
        mem.last_accessed.format("%Y-%m-%d %H:%M"),
        mem.access_count
    );
    println!("  summary:    {}", mem.summary);
    if !mem.keywords.is_empty() {
        println!("  keywords:   {}", mem.keywords.join(", "));
    }
    if let Some(ref raw) = mem.raw_excerpt {
        println!("  raw:        {raw}");
    }
    println!();
}

// ---------------------------------------------------------------------------
// Benchmark
// ---------------------------------------------------------------------------

fn cmd_bench(count: usize) -> Result<()> {
    const DIMS: usize = 384;
    const SEARCH_ITERS: usize = 100;

    let topics = [
        "architecture",
        "preferences",
        "errors-resolved",
        "context-project",
        "decisions",
    ];
    let queries = [
        "database architecture",
        "authentication flow",
        "error handling",
        "user preferences",
        "deployment config",
    ];

    // --- Seed without embeddings ---
    let store_plain = SqliteStore::in_memory()?;
    let t0 = Instant::now();
    for i in 0..count {
        let topic = topics[i % topics.len()].to_string();
        let content = format!(
            "Benchmark memory number {i} about {topic} with some extra words for FTS matching"
        );
        let importance = match i % 4 {
            0 => Importance::Critical,
            1 => Importance::High,
            2 => Importance::Medium,
            _ => Importance::Low,
        };
        let mut mem = Memory::new(topic, content, importance);
        mem.keywords = vec![format!("kw{}", i % 50), format!("bench{}", i % 20)];
        store_plain.store(mem)?;
    }
    let store_plain_ms = t0.elapsed().as_secs_f64() * 1000.0;

    // --- Seed with embeddings ---
    let store_vec = SqliteStore::in_memory()?;
    let t0 = Instant::now();
    for i in 0..count {
        let topic = topics[i % topics.len()].to_string();
        let content = format!(
            "Benchmark memory number {i} about {topic} with some extra words for FTS matching"
        );
        let importance = match i % 4 {
            0 => Importance::Critical,
            1 => Importance::High,
            2 => Importance::Medium,
            _ => Importance::Low,
        };
        let mut mem = Memory::new(topic, content, importance);
        mem.keywords = vec![format!("kw{}", i % 50), format!("bench{}", i % 20)];
        // Vary embedding so vectors aren't identical
        let mut emb = vec![0.1_f32; DIMS];
        emb[i % DIMS] += (i as f32) * 0.001;
        mem.embedding = Some(emb);
        store_vec.store(mem)?;
    }
    let store_vec_ms = t0.elapsed().as_secs_f64() * 1000.0;

    // --- FTS search ---
    let t0 = Instant::now();
    for i in 0..SEARCH_ITERS {
        let q = queries[i % queries.len()];
        let _ = store_vec.search_fts(q, 10)?;
    }
    let fts_ms = t0.elapsed().as_secs_f64() * 1000.0;

    // --- Vector search ---
    let query_emb = vec![0.1_f32; DIMS];
    let t0 = Instant::now();
    for _ in 0..SEARCH_ITERS {
        let _ = store_vec.search_by_embedding(&query_emb, 10)?;
    }
    let vec_ms = t0.elapsed().as_secs_f64() * 1000.0;

    // --- Hybrid search ---
    let t0 = Instant::now();
    for i in 0..SEARCH_ITERS {
        let q = queries[i % queries.len()];
        let _ = store_vec.search_hybrid(q, &query_emb, 10)?;
    }
    let hybrid_ms = t0.elapsed().as_secs_f64() * 1000.0;

    // --- Decay ---
    let t0 = Instant::now();
    let _ = store_vec.apply_decay(0.95)?;
    let decay_ms = t0.elapsed().as_secs_f64() * 1000.0;

    // --- Output ---
    println!("ICM Benchmark ({count} memories, {DIMS}d embeddings)");
    println!("{}", "─".repeat(58));
    print_bench_row("Store (no embeddings)", count, store_plain_ms);
    print_bench_row("Store (with embeddings)", count, store_vec_ms);
    print_bench_row("FTS5 search", SEARCH_ITERS, fts_ms);
    print_bench_row("Vector search (KNN)", SEARCH_ITERS, vec_ms);
    print_bench_row("Hybrid search", SEARCH_ITERS, hybrid_ms);
    print_bench_row("Decay (batch)", 1, decay_ms);
    println!("{}", "─".repeat(58));
    println!("DB size: in-memory (N/A)");
    println!(
        "Platform: {}-{}",
        std::env::consts::ARCH,
        std::env::consts::OS
    );

    Ok(())
}

fn print_bench_row(label: &str, ops: usize, total_ms: f64) {
    let per_op = total_ms / ops as f64;
    let (total_str, per_str) = (format_duration(total_ms), format_duration(per_op));
    println!(
        "{:<24} {:>6} ops {:>12} {:>12}/op",
        label, ops, total_str, per_str
    );
}

fn format_duration(ms: f64) -> String {
    if ms < 0.001 {
        format!("{:.1} ns", ms * 1_000_000.0)
    } else if ms < 1.0 {
        format!("{:.1} µs", ms * 1000.0)
    } else if ms < 1000.0 {
        format!("{:.1} ms", ms)
    } else {
        format!("{:.2} s", ms / 1000.0)
    }
}

// ---------------------------------------------------------------------------
// Agent Benchmark
// ---------------------------------------------------------------------------

struct SessionResult {
    num_turns: u64,
    input_tokens: u64,
    output_tokens: u64,
    cost_usd: f64,
    duration_ms: u64,
    response: String,
}

struct CleanupDir(PathBuf);

impl Drop for CleanupDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn cmd_bench_recall(model: &str, runs: usize, verbose: bool) -> Result<()> {
    // Check claude is in PATH
    let check = std::process::Command::new("claude")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    match check {
        Ok(s) if s.success() => {}
        _ => bail!("'claude' not found in PATH. Install Claude Code CLI first."),
    }

    let questions = bench_knowledge::QUESTIONS;
    let total_questions = questions.len();

    // Accumulate scores across runs
    // Per-question: Vec of (matches_wo, matches_wi) across runs
    let mut all_scores_wo: Vec<Vec<(usize, usize, f64)>> = Vec::new();
    let mut all_scores_wi: Vec<Vec<(usize, usize, f64)>> = Vec::new();
    let mut last_responses_wo: Vec<String> = Vec::new();
    let mut last_responses_wi: Vec<String> = Vec::new();

    for run in 0..runs {
        if runs > 1 {
            eprintln!("\n{}", "=".repeat(60));
            eprintln!("=== RUN {}/{runs} ===", run + 1);
        }

        let pid = std::process::id();
        let bench_dir = std::env::temp_dir().join(format!("icm-bench-recall-{pid}-{run}"));
        let _cleanup = CleanupDir(bench_dir.clone());
        std::fs::create_dir_all(&bench_dir)?;

        std::fs::write(bench_dir.join("CLAUDE.md"), "Answer questions concisely.")?;

        let no_mcp_path = bench_dir.join("no-mcp.json");
        std::fs::write(&no_mcp_path, r#"{"mcpServers":{}}"#)?;

        let icm_bin = std::env::current_exe().context("cannot determine icm binary path")?;
        let icm_db = bench_dir.join("icm-bench.db");
        let mcp_config_path = bench_dir.join("icm-mcp.json");
        let mcp_config = serde_json::json!({
            "mcpServers": {
                "icm": {
                    "command": icm_bin.to_string_lossy(),
                    "args": ["--db", icm_db.to_string_lossy(), "serve"]
                }
            }
        });
        std::fs::write(&mcp_config_path, serde_json::to_string_pretty(&mcp_config)?)?;
        {
            let _ = SqliteStore::new(&icm_db)?;
        }

        // === WITHOUT ICM ===
        eprintln!("=== WITHOUT ICM ===");
        let s1_prompt = format!(
            "{}{}",
            bench_knowledge::SESSION1_PROMPT,
            bench_knowledge::SOURCE_DOCUMENT
        );
        eprint!("  Session 1 (read document)...");
        let s1_result = run_claude_session(&s1_prompt, model, &bench_dir, &no_mcp_path)?;
        eprintln!(" done ({:.1}s)", s1_result.duration_ms as f64 / 1000.0);

        let mut scores_without: Vec<(usize, usize, f64)> = Vec::new();
        let mut responses_without: Vec<String> = Vec::new();
        for (i, q) in questions.iter().enumerate() {
            let prompt = format!("{}Question: {}", bench_knowledge::RECALL_PREFIX, q.prompt);
            eprint!("  Q{}/{}...", i + 1, total_questions);
            match run_claude_session(&prompt, model, &bench_dir, &no_mcp_path) {
                Ok(result) => {
                    let score = bench_knowledge::score_answer(&result.response, q);
                    eprintln!(" {}/{} keywords ({:.0}%)", score.0, score.1, score.2);
                    if verbose {
                        eprintln!("    Response: {}", truncate_words(&result.response, 200));
                    }
                    scores_without.push(score);
                    responses_without.push(result.response);
                }
                Err(e) => {
                    eprintln!(" FAILED: {e}");
                    scores_without.push((0, q.expected.len(), 0.0));
                    responses_without.push(String::new());
                }
            }
        }

        // === WITH ICM ===
        eprintln!("\n=== WITH ICM (MCP + auto-extraction) ===");
        eprint!("  Session 1 (read + memorize)...");
        let s1_icm = run_claude_session(&s1_prompt, model, &bench_dir, &mcp_config_path)?;
        eprintln!(" done ({:.1}s)", s1_icm.duration_ms as f64 / 1000.0);

        {
            let store = SqliteStore::new(&icm_db)?;
            let ext1 =
                extract::extract_and_store(&store, bench_knowledge::SOURCE_DOCUMENT, "meridian")?;
            let ext2 = extract::extract_and_store(&store, &s1_icm.response, "meridian")?;
            eprintln!("    Extracted {} + {} facts", ext1, ext2);

            if verbose {
                let all_mems = store.get_by_topic("context-meridian")?;
                eprintln!("    Stored facts:");
                for m in &all_mems {
                    eprintln!("      - {}", truncate_words(&m.summary, 120));
                }
            }
        }

        let mut scores_with: Vec<(usize, usize, f64)> = Vec::new();
        let mut responses_with: Vec<String> = Vec::new();
        for (i, q) in questions.iter().enumerate() {
            let store = SqliteStore::new(&icm_db)?;
            let ctx = extract::recall_context(&store, q.prompt, 15)?;
            if verbose && !ctx.is_empty() {
                eprintln!("  [verbose] Context injected for Q{}:", i + 1);
                for line in ctx.lines().take(10) {
                    eprintln!("    {line}");
                }
            }
            let prompt = format!(
                "{}{}\nQuestion: {}",
                ctx,
                bench_knowledge::RECALL_PREFIX,
                q.prompt
            );
            eprint!("  Q{}/{}...", i + 1, total_questions);
            match run_claude_session(&prompt, model, &bench_dir, &mcp_config_path) {
                Ok(result) => {
                    let score = bench_knowledge::score_answer(&result.response, q);
                    eprintln!(" {}/{} keywords ({:.0}%)", score.0, score.1, score.2);
                    if verbose {
                        eprintln!("    Response: {}", truncate_words(&result.response, 200));
                    }
                    {
                        let store = SqliteStore::new(&icm_db)?;
                        let _ = extract::extract_and_store(&store, &result.response, "meridian");
                    }
                    scores_with.push(score);
                    responses_with.push(result.response);
                }
                Err(e) => {
                    eprintln!(" FAILED: {e}");
                    scores_with.push((0, q.expected.len(), 0.0));
                    responses_with.push(String::new());
                }
            }
        }

        all_scores_wo.push(scores_without);
        all_scores_wi.push(scores_with);
        last_responses_wo = responses_without;
        last_responses_wi = responses_with;
    }

    // === Display Results (averaged across runs) ===
    println!();
    let w = 70;
    if runs > 1 {
        println!(
            "ICM Recall Benchmark ({total_questions} questions, model: {model}, {runs} runs averaged)"
        );
    } else {
        println!("ICM Recall Benchmark ({total_questions} questions, model: {model})");
    }
    println!("{}", "\u{2550}".repeat(w));
    println!("{:<40} {:>12} {:>12}", "Question", "No ICM", "With ICM");
    println!("{}", "\u{2500}".repeat(w));

    let mut total_wo = 0.0;
    let mut total_wi = 0.0;
    let mut pass_wo = 0;
    let mut pass_wi = 0;

    for (i, q) in questions.iter().enumerate() {
        // Average across runs
        let avg_matches_wo: f64 =
            all_scores_wo.iter().map(|r| r[i].0 as f64).sum::<f64>() / runs as f64;
        let avg_matches_wi: f64 =
            all_scores_wi.iter().map(|r| r[i].0 as f64).sum::<f64>() / runs as f64;
        let avg_score_wo: f64 = all_scores_wo.iter().map(|r| r[i].2).sum::<f64>() / runs as f64;
        let avg_score_wi: f64 = all_scores_wi.iter().map(|r| r[i].2).sum::<f64>() / runs as f64;
        let total_expected = all_scores_wo[0][i].1;

        let q_short = if q.prompt.len() > 38 {
            format!("{}...", &q.prompt[..35])
        } else {
            q.prompt.to_string()
        };

        let wo_str = format!(
            "{:.1}/{} ({:.0}%)",
            avg_matches_wo, total_expected, avg_score_wo
        );
        let wi_str = format!(
            "{:.1}/{} ({:.0}%)",
            avg_matches_wi, total_expected, avg_score_wi
        );

        println!("{:<40} {:>12} {:>12}", q_short, wo_str, wi_str);

        total_wo += avg_score_wo;
        total_wi += avg_score_wi;
        if avg_score_wo >= 100.0 {
            pass_wo += 1;
        }
        if avg_score_wi >= 100.0 {
            pass_wi += 1;
        }
    }

    let avg_wo = total_wo / total_questions as f64;
    let avg_wi = total_wi / total_questions as f64;

    println!("{}", "\u{2500}".repeat(w));
    println!(
        "{:<40} {:>12} {:>12}",
        "Average score",
        format!("{avg_wo:.0}%"),
        format!("{avg_wi:.0}%"),
    );
    println!(
        "{:<40} {:>12} {:>12}",
        "Questions passed",
        format!("{pass_wo}/{total_questions}"),
        format!("{pass_wi}/{total_questions}"),
    );
    println!("{}", "\u{2550}".repeat(w));

    // Show a sample answer comparison (from last run)
    if !last_responses_wo.is_empty() {
        println!();
        println!("Sample: \"{}\"", questions[0].prompt);
        println!("{}", "\u{2500}".repeat(w));
        println!("WITHOUT ICM:");
        println!("  {}", truncate_words(&last_responses_wo[0], 300));
        println!();
        println!("WITH ICM:");
        println!("  {}", truncate_words(&last_responses_wi[0], 300));
    }

    Ok(())
}

fn cmd_bench_agent(sessions: usize, model: &str, runs: usize, verbose: bool) -> Result<()> {
    // Check claude is in PATH
    let check = std::process::Command::new("claude")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    match check {
        Ok(s) if s.success() => {}
        _ => bail!("'claude' not found in PATH. Install Claude Code CLI first."),
    }

    // Accumulate results across runs
    let mut all_results_wo: Vec<Vec<SessionResult>> = Vec::new();
    let mut all_results_wi: Vec<Vec<SessionResult>> = Vec::new();

    for run in 0..runs {
        if runs > 1 {
            eprintln!("\n{}", "=".repeat(60));
            eprintln!("=== RUN {}/{runs} ===", run + 1);
        }

        let pid = std::process::id();
        let bench_dir = std::env::temp_dir().join(format!("icm-bench-agent-{pid}-{run}"));
        let _cleanup = CleanupDir(bench_dir.clone());

        std::fs::create_dir_all(bench_dir.join("src"))?;
        for (path, content) in bench_data::PROJECT_FILES {
            let full = bench_dir.join(path);
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(full, content)?;
        }
        eprintln!(
            "Test project: {} files in {}",
            bench_data::PROJECT_FILES.len(),
            bench_dir.display()
        );

        let prompts: Vec<&str> = (0..sessions)
            .map(|i| bench_data::SESSION_PROMPTS[i % bench_data::SESSION_PROMPTS.len()])
            .collect();

        // --- Without ICM ---
        eprintln!("Running {sessions} sessions WITHOUT ICM...");
        let no_mcp_path = bench_dir.join("no-mcp.json");
        std::fs::write(&no_mcp_path, r#"{"mcpServers":{}}"#)?;

        let mut results_without: Vec<SessionResult> = Vec::new();
        for (i, prompt) in prompts.iter().enumerate() {
            eprint!("  Session {}/{}...", i + 1, sessions);
            match run_claude_session(prompt, model, &bench_dir, &no_mcp_path) {
                Ok(result) => {
                    eprintln!(" done ({:.1}s)", result.duration_ms as f64 / 1000.0);
                    results_without.push(result);
                }
                Err(e) => {
                    eprintln!(" FAILED: {e}");
                    results_without.push(SessionResult {
                        num_turns: 0,
                        input_tokens: 0,
                        output_tokens: 0,
                        cost_usd: 0.0,
                        duration_ms: 0,
                        response: String::new(),
                    });
                }
            }
        }

        // --- With ICM (MCP + auto-extraction) ---
        eprintln!("Running {sessions} sessions WITH ICM (MCP + auto-extraction)...");
        let icm_bin = std::env::current_exe().context("cannot determine icm binary path")?;
        let icm_db = bench_dir.join("icm-bench.db");
        let mcp_config_path = bench_dir.join("icm-mcp.json");
        let mcp_config = serde_json::json!({
            "mcpServers": {
                "icm": {
                    "command": icm_bin.to_string_lossy(),
                    "args": ["--db", icm_db.to_string_lossy(), "serve"]
                }
            }
        });
        std::fs::write(&mcp_config_path, serde_json::to_string_pretty(&mcp_config)?)?;
        {
            let _ = SqliteStore::new(&icm_db)?;
        }

        let mut results_with: Vec<SessionResult> = Vec::new();
        for (i, prompt) in prompts.iter().enumerate() {
            let effective_prompt = if i > 0 {
                let store = SqliteStore::new(&icm_db)?;
                let ctx = extract::recall_context(&store, prompt, 15)?;
                if verbose && !ctx.is_empty() {
                    eprintln!("  [verbose] Context injected for session {}:", i + 1);
                    for line in ctx.lines().take(8) {
                        eprintln!("    {line}");
                    }
                }
                if ctx.is_empty() {
                    prompt.to_string()
                } else {
                    format!("{ctx}{prompt}")
                }
            } else {
                prompt.to_string()
            };

            eprint!("  Session {}/{}...", i + 1, sessions);
            match run_claude_session(&effective_prompt, model, &bench_dir, &mcp_config_path) {
                Ok(result) => {
                    eprintln!(" done ({:.1}s)", result.duration_ms as f64 / 1000.0);
                    {
                        let store = SqliteStore::new(&icm_db)?;
                        let extracted =
                            extract::extract_and_store(&store, &result.response, "mathlib")?;
                        if extracted > 0 {
                            eprintln!("    Extracted {extracted} facts");
                        }
                        if verbose {
                            let all_mems = store.get_by_topic("context-mathlib")?;
                            eprintln!("    Total facts in DB: {}", all_mems.len());
                        }
                    }
                    results_with.push(result);
                }
                Err(e) => {
                    eprintln!(" FAILED: {e}");
                    results_with.push(SessionResult {
                        num_turns: 0,
                        input_tokens: 0,
                        output_tokens: 0,
                        cost_usd: 0.0,
                        duration_ms: 0,
                        response: String::new(),
                    });
                }
            }
        }

        // Display per-run results
        if runs > 1 {
            eprintln!("  Run {} totals:", run + 1);
            let wo_turns: u64 = results_without.iter().map(|r| r.num_turns).sum();
            let wi_turns: u64 = results_with.iter().map(|r| r.num_turns).sum();
            let wo_ctx: u64 = results_without.iter().map(|r| r.input_tokens).sum();
            let wi_ctx: u64 = results_with.iter().map(|r| r.input_tokens).sum();
            let wo_cost: f64 = results_without.iter().map(|r| r.cost_usd).sum();
            let wi_cost: f64 = results_with.iter().map(|r| r.cost_usd).sum();
            eprintln!(
                "    Turns: {} vs {} ({:+.0}%)",
                wo_turns,
                wi_turns,
                pct_delta(wo_turns as f64, wi_turns as f64)
            );
            eprintln!(
                "    Context: {:.1}k vs {:.1}k ({:+.0}%)",
                wo_ctx as f64 / 1000.0,
                wi_ctx as f64 / 1000.0,
                pct_delta(wo_ctx as f64, wi_ctx as f64)
            );
            eprintln!(
                "    Cost: ${:.4} vs ${:.4} ({:+.0}%)",
                wo_cost,
                wi_cost,
                pct_delta(wo_cost, wi_cost)
            );
        }

        all_results_wo.push(results_without);
        all_results_wi.push(results_with);
    }

    // --- Display averaged results ---
    if runs > 1 {
        display_bench_results_averaged(&all_results_wo, &all_results_wi, sessions, model, runs);
    } else {
        display_bench_results(&all_results_wo[0], &all_results_wi[0], sessions, model);
    }

    Ok(())
}

fn pct_delta(a: f64, b: f64) -> f64 {
    if a == 0.0 {
        0.0
    } else {
        ((b - a) / a) * 100.0
    }
}

fn run_claude_session(
    prompt: &str,
    model: &str,
    cwd: &std::path::Path,
    mcp_config: &std::path::Path,
) -> Result<SessionResult> {
    let mut cmd = std::process::Command::new("claude");
    cmd.arg("-p")
        .arg(prompt)
        .arg("--output-format")
        .arg("json")
        .arg("--model")
        .arg(model)
        .arg("--max-turns")
        .arg("10")
        .arg("--mcp-config")
        .arg(mcp_config)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .current_dir(cwd);

    let start = Instant::now();
    let mut child = cmd.spawn().context("failed to spawn 'claude'")?;

    // Timeout: 180 seconds per session
    let timeout = std::time::Duration::from_secs(180);
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => break,
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    bail!("claude timed out after {}s", timeout.as_secs());
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            Err(e) => bail!("error waiting for claude: {e}"),
        }
    }

    let output = child
        .wait_with_output()
        .context("failed to get claude output")?;
    let wall_ms = start.elapsed().as_millis() as u64;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "claude exited with {}: {}",
            output.status,
            stderr.chars().take(500).collect::<String>()
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: Value = serde_json::from_str(stdout.trim()).with_context(|| {
        format!(
            "failed to parse claude JSON: {}",
            &stdout[..stdout.len().min(200)]
        )
    })?;

    Ok(parse_session_result(&json, wall_ms))
}

fn parse_session_result(json: &Value, wall_ms: u64) -> SessionResult {
    let num_turns = json.get("num_turns").and_then(|v| v.as_u64()).unwrap_or(1);

    let usage = json.get("usage");

    // Total input = input_tokens + cache_creation_input_tokens + cache_read_input_tokens
    let input_direct = usage
        .and_then(|u| u.get("input_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let cache_creation = usage
        .and_then(|u| u.get("cache_creation_input_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let cache_read = usage
        .and_then(|u| u.get("cache_read_input_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let input_tokens = input_direct + cache_creation + cache_read;

    let output_tokens = usage
        .and_then(|u| u.get("output_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    let cost_usd = json
        .get("total_cost_usd")
        .and_then(|v| v.as_f64())
        .or_else(|| json.get("cost_usd").and_then(|v| v.as_f64()))
        .unwrap_or(0.0);

    let duration_ms = json
        .get("duration_ms")
        .and_then(|v| v.as_u64())
        .or_else(|| json.get("duration_api_ms").and_then(|v| v.as_u64()))
        .unwrap_or(wall_ms);

    let response = json
        .get("result")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    SessionResult {
        num_turns,
        input_tokens,
        output_tokens,
        cost_usd,
        duration_ms,
        response,
    }
}

fn display_bench_results(
    without: &[SessionResult],
    with_icm: &[SessionResult],
    sessions: usize,
    model: &str,
) {
    let w = 66;
    println!();
    println!("ICM Agent Benchmark ({sessions} sessions, model: {model})");
    println!("{}", "\u{2550}".repeat(w));
    println!(
        "{:<22} {:>16} {:>16} {:>10}",
        "", "Without ICM", "With ICM", "Delta"
    );

    for i in 0..sessions {
        let wo = &without[i];
        let wi = &with_icm[i];

        println!("Session {}", i + 1);
        println!(
            "  {:<20} {:>16} {:>16} {:>10}",
            "Turns",
            wo.num_turns,
            wi.num_turns,
            fmt_delta(wo.num_turns as f64, wi.num_turns as f64)
        );
        println!(
            "  {:<20} {:>16} {:>16} {:>10}",
            "Tokens (in/out)",
            format!(
                "{}/{}",
                fmt_tokens(wo.input_tokens),
                fmt_tokens(wo.output_tokens)
            ),
            format!(
                "{}/{}",
                fmt_tokens(wi.input_tokens),
                fmt_tokens(wi.output_tokens)
            ),
            fmt_delta(
                (wo.input_tokens + wo.output_tokens) as f64,
                (wi.input_tokens + wi.output_tokens) as f64,
            )
        );
        println!(
            "  {:<20} {:>16} {:>16} {:>10}",
            "Context (input)",
            fmt_tokens(wo.input_tokens),
            fmt_tokens(wi.input_tokens),
            fmt_delta(wo.input_tokens as f64, wi.input_tokens as f64)
        );
        println!(
            "  {:<20} {:>16} {:>16} {:>10}",
            "Cost",
            fmt_cost(wo.cost_usd),
            fmt_cost(wi.cost_usd),
            fmt_delta(wo.cost_usd, wi.cost_usd)
        );
        println!();
    }

    // Totals
    let total_wo = aggregate_results(without);
    let total_wi = aggregate_results(with_icm);

    println!("{}", "\u{2500}".repeat(w));
    println!("Total");
    println!(
        "  {:<20} {:>16} {:>16} {:>10}",
        "Turns",
        total_wo.num_turns,
        total_wi.num_turns,
        fmt_delta(total_wo.num_turns as f64, total_wi.num_turns as f64)
    );
    println!(
        "  {:<20} {:>16} {:>16} {:>10}",
        "Context (input)",
        fmt_tokens(total_wo.input_tokens),
        fmt_tokens(total_wi.input_tokens),
        fmt_delta(total_wo.input_tokens as f64, total_wi.input_tokens as f64)
    );
    println!(
        "  {:<20} {:>16} {:>16} {:>10}",
        "Tokens (total)",
        fmt_tokens(total_wo.input_tokens + total_wo.output_tokens),
        fmt_tokens(total_wi.input_tokens + total_wi.output_tokens),
        fmt_delta(
            (total_wo.input_tokens + total_wo.output_tokens) as f64,
            (total_wi.input_tokens + total_wi.output_tokens) as f64,
        )
    );
    println!(
        "  {:<20} {:>16} {:>16} {:>10}",
        "Cost",
        fmt_cost(total_wo.cost_usd),
        fmt_cost(total_wi.cost_usd),
        fmt_delta(total_wo.cost_usd, total_wi.cost_usd)
    );
    println!(
        "  {:<20} {:>16} {:>16} {:>10}",
        "Duration",
        fmt_duration_s(total_wo.duration_ms),
        fmt_duration_s(total_wi.duration_ms),
        fmt_delta(total_wo.duration_ms as f64, total_wi.duration_ms as f64)
    );
    println!("{}", "\u{2550}".repeat(w));

    // --- Response comparison ---
    println!();
    println!("Response samples (session 2: recall test)");
    println!("{}", "\u{2500}".repeat(w));
    if without.len() >= 2 {
        println!("WITHOUT ICM:");
        println!("  {}", truncate_words(&without[1].response, 200));
        println!();
        println!("WITH ICM:");
        println!("  {}", truncate_words(&with_icm[1].response, 200));
    }

    // Response length comparison
    println!();
    println!("Response lengths (chars):");
    let wo_avg = without.iter().map(|s| s.response.len()).sum::<usize>() / sessions.max(1);
    let wi_avg = with_icm.iter().map(|s| s.response.len()).sum::<usize>() / sessions.max(1);
    println!(
        "  avg without ICM: {} chars | avg with ICM: {} chars",
        wo_avg, wi_avg
    );
}

fn display_bench_results_averaged(
    all_wo: &[Vec<SessionResult>],
    all_wi: &[Vec<SessionResult>],
    sessions: usize,
    model: &str,
    runs: usize,
) {
    let w = 66;
    println!();
    println!("ICM Agent Benchmark ({sessions} sessions, model: {model}, {runs} runs averaged)");
    println!("{}", "\u{2550}".repeat(w));
    println!(
        "{:<22} {:>16} {:>16} {:>10}",
        "", "Without ICM", "With ICM", "Delta"
    );

    // Average totals across runs
    let mut avg_turns_wo = 0.0f64;
    let mut avg_turns_wi = 0.0f64;
    let mut avg_ctx_wo = 0.0f64;
    let mut avg_ctx_wi = 0.0f64;
    let mut avg_cost_wo = 0.0f64;
    let mut avg_cost_wi = 0.0f64;
    let mut avg_dur_wo = 0.0f64;
    let mut avg_dur_wi = 0.0f64;

    // Per-run totals for min/max
    let mut run_delta_turns: Vec<f64> = Vec::new();
    let mut run_delta_ctx: Vec<f64> = Vec::new();
    let mut run_delta_cost: Vec<f64> = Vec::new();

    for run in 0..runs {
        let wo = aggregate_results(&all_wo[run]);
        let wi = aggregate_results(&all_wi[run]);
        avg_turns_wo += wo.num_turns as f64;
        avg_turns_wi += wi.num_turns as f64;
        avg_ctx_wo += wo.input_tokens as f64;
        avg_ctx_wi += wi.input_tokens as f64;
        avg_cost_wo += wo.cost_usd;
        avg_cost_wi += wi.cost_usd;
        avg_dur_wo += wo.duration_ms as f64;
        avg_dur_wi += wi.duration_ms as f64;

        run_delta_turns.push(pct_delta(wo.num_turns as f64, wi.num_turns as f64));
        run_delta_ctx.push(pct_delta(wo.input_tokens as f64, wi.input_tokens as f64));
        run_delta_cost.push(pct_delta(wo.cost_usd, wi.cost_usd));
    }

    let r = runs as f64;
    avg_turns_wo /= r;
    avg_turns_wi /= r;
    avg_ctx_wo /= r;
    avg_ctx_wi /= r;
    avg_cost_wo /= r;
    avg_cost_wi /= r;
    avg_dur_wo /= r;
    avg_dur_wi /= r;

    // Per-session averages
    for s in 0..sessions {
        let s_turns_wo: f64 = all_wo.iter().map(|r| r[s].num_turns as f64).sum::<f64>() / r;
        let s_turns_wi: f64 = all_wi.iter().map(|r| r[s].num_turns as f64).sum::<f64>() / r;
        let s_ctx_wo: f64 = all_wo.iter().map(|r| r[s].input_tokens as f64).sum::<f64>() / r;
        let s_ctx_wi: f64 = all_wi.iter().map(|r| r[s].input_tokens as f64).sum::<f64>() / r;
        let s_cost_wo: f64 = all_wo.iter().map(|r| r[s].cost_usd).sum::<f64>() / r;
        let s_cost_wi: f64 = all_wi.iter().map(|r| r[s].cost_usd).sum::<f64>() / r;

        println!("Session {} (avg)", s + 1);
        println!(
            "  {:<20} {:>16} {:>16} {:>10}",
            "Turns",
            format!("{:.1}", s_turns_wo),
            format!("{:.1}", s_turns_wi),
            fmt_delta(s_turns_wo, s_turns_wi)
        );
        println!(
            "  {:<20} {:>16} {:>16} {:>10}",
            "Context (input)",
            fmt_tokens(s_ctx_wo as u64),
            fmt_tokens(s_ctx_wi as u64),
            fmt_delta(s_ctx_wo, s_ctx_wi)
        );
        println!(
            "  {:<20} {:>16} {:>16} {:>10}",
            "Cost",
            fmt_cost(s_cost_wo),
            fmt_cost(s_cost_wi),
            fmt_delta(s_cost_wo, s_cost_wi)
        );
        println!();
    }

    println!("{}", "\u{2500}".repeat(w));
    println!("Total (averaged over {runs} runs)");
    println!(
        "  {:<20} {:>16} {:>16} {:>10}",
        "Turns",
        format!("{:.0}", avg_turns_wo),
        format!("{:.0}", avg_turns_wi),
        fmt_delta(avg_turns_wo, avg_turns_wi)
    );
    println!(
        "  {:<20} {:>16} {:>16} {:>10}",
        "Context (input)",
        fmt_tokens(avg_ctx_wo as u64),
        fmt_tokens(avg_ctx_wi as u64),
        fmt_delta(avg_ctx_wo, avg_ctx_wi)
    );
    println!(
        "  {:<20} {:>16} {:>16} {:>10}",
        "Cost",
        fmt_cost(avg_cost_wo),
        fmt_cost(avg_cost_wi),
        fmt_delta(avg_cost_wo, avg_cost_wi)
    );
    println!(
        "  {:<20} {:>16} {:>16} {:>10}",
        "Duration",
        fmt_duration_s(avg_dur_wo as u64),
        fmt_duration_s(avg_dur_wi as u64),
        fmt_delta(avg_dur_wo, avg_dur_wi)
    );
    println!("{}", "\u{2550}".repeat(w));

    // Variance summary
    if runs > 1 {
        let min_t = run_delta_turns
            .iter()
            .cloned()
            .fold(f64::INFINITY, f64::min);
        let max_t = run_delta_turns
            .iter()
            .cloned()
            .fold(f64::NEG_INFINITY, f64::max);
        let min_c = run_delta_cost.iter().cloned().fold(f64::INFINITY, f64::min);
        let max_c = run_delta_cost
            .iter()
            .cloned()
            .fold(f64::NEG_INFINITY, f64::max);
        let min_x = run_delta_ctx.iter().cloned().fold(f64::INFINITY, f64::min);
        let max_x = run_delta_ctx
            .iter()
            .cloned()
            .fold(f64::NEG_INFINITY, f64::max);
        println!();
        println!("Variance across {runs} runs:");
        println!("  Turns delta:   {:.0}% to {:.0}%", min_t, max_t);
        println!("  Context delta: {:.0}% to {:.0}%", min_x, max_x);
        println!("  Cost delta:    {:.0}% to {:.0}%", min_c, max_c);
    }
}

fn aggregate_results(results: &[SessionResult]) -> SessionResult {
    SessionResult {
        num_turns: results.iter().map(|s| s.num_turns).sum(),
        input_tokens: results.iter().map(|s| s.input_tokens).sum(),
        output_tokens: results.iter().map(|s| s.output_tokens).sum(),
        cost_usd: results.iter().map(|s| s.cost_usd).sum(),
        duration_ms: results.iter().map(|s| s.duration_ms).sum(),
        response: String::new(),
    }
}

fn fmt_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        format!("{n}")
    }
}

fn fmt_delta(without: f64, with_icm: f64) -> String {
    if without == 0.0 {
        return "N/A".into();
    }
    let pct = ((with_icm - without) / without) * 100.0;
    if pct >= 0.0 {
        format!("+{pct:.0}%")
    } else {
        format!("{pct:.0}%")
    }
}

fn fmt_cost(c: f64) -> String {
    format!("${c:.4}")
}

fn fmt_duration_s(ms: u64) -> String {
    format!("{:.1}s", ms as f64 / 1000.0)
}

fn truncate_words(s: &str, max_chars: usize) -> String {
    let s = s.replace('\n', " ");
    if s.len() <= max_chars {
        s
    } else {
        let truncated: String = s.chars().take(max_chars).collect();
        format!("{truncated}...")
    }
}

// ---------------------------------------------------------------------------
// Memoir commands
// ---------------------------------------------------------------------------

fn resolve_memoir(store: &SqliteStore, name: &str) -> Result<Memoir> {
    store
        .get_memoir_by_name(name)?
        .ok_or_else(|| anyhow::anyhow!("memoir not found: {name}"))
}

fn cmd_memoir_create(store: &SqliteStore, name: String, description: String) -> Result<()> {
    let memoir = Memoir::new(name, description);
    let id = store.create_memoir(memoir)?;
    println!("Created memoir: {id}");
    Ok(())
}

fn cmd_memoir_list(store: &SqliteStore) -> Result<()> {
    let memoirs = store.list_memoirs()?;
    if memoirs.is_empty() {
        println!("No memoirs yet.");
        return Ok(());
    }

    println!("{:<25} {:<8} Description", "Name", "Concepts");
    println!("{}", "-".repeat(60));
    for m in &memoirs {
        let stats = store.memoir_stats(&m.id)?;
        println!(
            "{:<25} {:<8} {}",
            m.name,
            stats.total_concepts,
            truncate(&m.description, 40)
        );
    }
    Ok(())
}

fn cmd_memoir_show(store: &SqliteStore, name: &str) -> Result<()> {
    let memoir = resolve_memoir(store, name)?;
    let stats = store.memoir_stats(&memoir.id)?;

    println!("Memoir: {}", memoir.name);
    if !memoir.description.is_empty() {
        println!("  description: {}", memoir.description);
    }
    println!(
        "  created:     {}",
        memoir.created_at.format("%Y-%m-%d %H:%M")
    );
    println!(
        "  updated:     {}",
        memoir.updated_at.format("%Y-%m-%d %H:%M")
    );
    println!("  concepts:    {}", stats.total_concepts);
    println!("  links:       {}", stats.total_links);
    println!("  avg conf:    {:.2}", stats.avg_confidence);

    if !stats.label_counts.is_empty() {
        println!("  labels:");
        for (label, count) in &stats.label_counts {
            println!("    {label} ({count})");
        }
    }

    let concepts = store.list_concepts(&memoir.id)?;
    if !concepts.is_empty() {
        println!("\n  Concepts:");
        for c in &concepts {
            let labels_str = c
                .labels
                .iter()
                .map(|l| l.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            println!(
                "    {} [r{} c{:.2}] {}",
                c.name,
                c.revision,
                c.confidence,
                if labels_str.is_empty() {
                    String::new()
                } else {
                    format!("({labels_str})")
                }
            );
        }
    }

    Ok(())
}

fn cmd_memoir_delete(store: &SqliteStore, name: &str) -> Result<()> {
    let memoir = resolve_memoir(store, name)?;
    store.delete_memoir(&memoir.id)?;
    println!("Deleted memoir: {name}");
    Ok(())
}

fn cmd_memoir_add_concept(
    store: &SqliteStore,
    memoir_name: &str,
    name: String,
    definition: String,
    labels_str: Option<String>,
) -> Result<()> {
    let memoir = resolve_memoir(store, memoir_name)?;
    let mut concept = Concept::new(memoir.id, name, definition);

    if let Some(ls) = labels_str {
        concept.labels = ls
            .split(',')
            .map(|s| s.trim().parse::<Label>())
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| anyhow::anyhow!(e))?;
    }

    let id = store.add_concept(concept)?;
    println!("Added concept: {id}");
    Ok(())
}

fn cmd_memoir_refine(
    store: &SqliteStore,
    memoir_name: &str,
    concept_name: &str,
    new_definition: &str,
) -> Result<()> {
    let memoir = resolve_memoir(store, memoir_name)?;
    let concept = store
        .get_concept_by_name(&memoir.id, concept_name)?
        .ok_or_else(|| anyhow::anyhow!("concept not found: {concept_name}"))?;

    store.refine_concept(&concept.id, new_definition, &[])?;

    let updated = store.get_concept(&concept.id)?.expect("just refined");
    println!(
        "Refined: {} (r{}, confidence={:.2})",
        concept_name, updated.revision, updated.confidence
    );
    Ok(())
}

fn cmd_memoir_search(
    store: &SqliteStore,
    memoir_name: &str,
    query: &str,
    label: Option<&str>,
    limit: usize,
) -> Result<()> {
    let memoir = resolve_memoir(store, memoir_name)?;

    let results = if let Some(label_str) = label {
        let parsed: Label = label_str.parse().map_err(|e: String| anyhow::anyhow!(e))?;
        let mut by_label = store.search_concepts_by_label(&memoir.id, &parsed, limit)?;
        if !query.is_empty() {
            let q = query.to_lowercase();
            by_label.retain(|c| {
                c.name.to_lowercase().contains(&q) || c.definition.to_lowercase().contains(&q)
            });
        }
        by_label
    } else {
        store.search_concepts_fts(&memoir.id, query, limit)?
    };

    if results.is_empty() {
        println!("No concepts found.");
        return Ok(());
    }

    for c in &results {
        print_concept(c);
    }
    Ok(())
}

fn cmd_memoir_search_all(store: &SqliteStore, query: &str, limit: usize) -> Result<()> {
    let results = store.search_all_concepts_fts(query, limit)?;

    if results.is_empty() {
        println!("No concepts found.");
        return Ok(());
    }

    // Build memoir_id -> name map
    let memoirs: std::collections::HashMap<String, String> = store
        .list_memoirs()?
        .into_iter()
        .map(|m| (m.id.clone(), m.name))
        .collect();

    for c in &results {
        let memoir_name = memoirs.get(&c.memoir_id).map(|s| s.as_str()).unwrap_or("?");
        println!("--- {} ({}) ---", c.name, memoir_name);
        println!("  definition: {}", c.definition);
        println!("  confidence: {:.2}", c.confidence);
        println!("  revision:   {}", c.revision);
        if !c.labels.is_empty() {
            let labels_str = c
                .labels
                .iter()
                .map(|l| l.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            println!("  labels:     {labels_str}");
        }
        println!();
    }
    Ok(())
}

fn cmd_memoir_link(
    store: &SqliteStore,
    memoir_name: &str,
    from_name: &str,
    to_name: &str,
    relation: Relation,
) -> Result<()> {
    let memoir = resolve_memoir(store, memoir_name)?;

    let from = store
        .get_concept_by_name(&memoir.id, from_name)?
        .ok_or_else(|| anyhow::anyhow!("concept not found: {from_name}"))?;
    let to = store
        .get_concept_by_name(&memoir.id, to_name)?
        .ok_or_else(|| anyhow::anyhow!("concept not found: {to_name}"))?;

    let link = ConceptLink::new(from.id, to.id, relation);
    let id = store.add_link(link)?;
    println!("Linked: {from_name} --{relation}--> {to_name} ({id})");
    Ok(())
}

fn cmd_memoir_inspect(
    store: &SqliteStore,
    memoir_name: &str,
    concept_name: &str,
    depth: usize,
) -> Result<()> {
    let memoir = resolve_memoir(store, memoir_name)?;
    let concept = store
        .get_concept_by_name(&memoir.id, concept_name)?
        .ok_or_else(|| anyhow::anyhow!("concept not found: {concept_name}"))?;

    print_concept(&concept);

    let (neighbors, links) = store.get_neighborhood(&concept.id, depth)?;

    if links.is_empty() {
        println!("  (no links)");
        return Ok(());
    }

    println!("  Graph (depth={depth}):");
    for link in &links {
        let src_name = neighbors
            .iter()
            .find(|c| c.id == link.source_id)
            .map(|c| c.name.as_str())
            .unwrap_or("?");
        let tgt_name = neighbors
            .iter()
            .find(|c| c.id == link.target_id)
            .map(|c| c.name.as_str())
            .unwrap_or("?");
        println!("    {src_name} --{}--> {tgt_name}", link.relation);
    }

    Ok(())
}

fn cmd_memoir_distill(store: &SqliteStore, from_topic: &str, into_name: &str) -> Result<()> {
    let memoir = resolve_memoir(store, into_name)?;
    let memories = store.get_by_topic(from_topic)?;

    if memories.is_empty() {
        bail!("no memories found in topic: {from_topic}");
    }

    let mut created = 0;
    for mem in &memories {
        let concept_name = if !mem.keywords.is_empty() {
            mem.keywords[0].clone()
        } else {
            format!("{}-{}", from_topic, &mem.id[..8])
        };

        if store
            .get_concept_by_name(&memoir.id, &concept_name)?
            .is_some()
        {
            let existing = store
                .get_concept_by_name(&memoir.id, &concept_name)?
                .expect("just checked");
            let merged_def = format!("{}\n---\n{}", existing.definition, mem.summary);
            store.refine_concept(&existing.id, &merged_def, std::slice::from_ref(&mem.id))?;
            println!("  Refined: {concept_name}");
        } else {
            let mut concept =
                Concept::new(memoir.id.clone(), concept_name.clone(), mem.summary.clone());
            concept.source_memory_ids = vec![mem.id.clone()];
            for kw in &mem.keywords {
                concept.labels.push(Label::new("tag", kw));
            }
            store.add_concept(concept)?;
            created += 1;
            println!("  Created: {concept_name}");
        }
    }

    println!(
        "Distilled {} memories from '{from_topic}' into memoir '{into_name}' ({created} new concepts).",
        memories.len()
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Display helpers
// ---------------------------------------------------------------------------

fn print_concept(c: &Concept) {
    println!("--- {} ---", c.name);
    println!("  id:         {}", c.id);
    println!("  definition: {}", c.definition);
    println!("  confidence: {:.2}", c.confidence);
    println!("  revision:   {}", c.revision);
    if !c.labels.is_empty() {
        let labels_str = c
            .labels
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        println!("  labels:     {labels_str}");
    }
    println!("  created:    {}", c.created_at.format("%Y-%m-%d %H:%M"));
    println!("  updated:    {}", c.updated_at.format("%Y-%m-%d %H:%M"));
    if !c.source_memory_ids.is_empty() {
        println!("  sources:    {}", c.source_memory_ids.join(", "));
    }
    println!();
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max.saturating_sub(3)])
    }
}
