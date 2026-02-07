use std::path::PathBuf;

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

    /// Configure ICM as MCP server for Claude Code
    Init,

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
        }
    }
}

#[derive(Clone, ValueEnum)]
enum SortField {
    Weight,
    Created,
    Accessed,
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
            cmd_store(&store, emb_ref, topic, content, importance.into(), keywords, raw)
        }
        Commands::Recall {
            query,
            topic,
            limit,
        } => {
            #[cfg(feature = "embeddings")]
            let emb_ref = embedder.as_ref().map(|e| e as &dyn icm_core::Embedder);
            #[cfg(not(feature = "embeddings"))]
            let emb_ref: Option<&dyn icm_core::Embedder> = None;
            cmd_recall(&store, emb_ref, &query, topic.as_deref(), limit)
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
                limit,
            } => cmd_memoir_search(&store, &memoir, &query, limit),
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
        Commands::Init => cmd_init(),
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
) -> Result<()> {
    // Try hybrid search if embedder is available
    if let Some(emb) = embedder {
        if let Ok(query_emb) = emb.embed(query) {
            if let Ok(results) = store.search_hybrid(query, &query_emb, limit) {
                let mut scored = results;
                if let Some(t) = topic {
                    scored.retain(|(m, _)| m.topic == t);
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

fn cmd_init() -> Result<()> {
    // Find the icm binary path
    let icm_bin = std::env::current_exe().context("cannot determine icm binary path")?;
    let icm_bin_str = icm_bin.to_string_lossy().to_string();

    // Locate ~/.claude.json
    let home = std::env::var("HOME").context("HOME not set")?;
    let claude_config_path = PathBuf::from(&home).join(".claude.json");

    // Read existing config or create empty object
    let mut config: Value = if claude_config_path.exists() {
        let content =
            std::fs::read_to_string(&claude_config_path).context("cannot read ~/.claude.json")?;
        serde_json::from_str(&content).context("invalid JSON in ~/.claude.json")?
    } else {
        serde_json::json!({})
    };

    // Ensure mcpServers object exists
    let mcp_servers = config
        .as_object_mut()
        .context("~/.claude.json is not a JSON object")?
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}));

    // Check if already configured
    if mcp_servers.get("icm").is_some() {
        let existing_cmd = mcp_servers["icm"]["command"].as_str().unwrap_or("");
        if existing_cmd == icm_bin_str {
            println!("ICM is already configured in Claude Code.");
            println!("  binary: {icm_bin_str}");
            println!("\nRestart Claude Code to pick up any changes.");
            return Ok(());
        }
        println!("Updating ICM configuration (was: {existing_cmd})");
    }

    // Set icm MCP server config
    mcp_servers.as_object_mut().unwrap().insert(
        "icm".to_string(),
        serde_json::json!({
            "command": icm_bin_str,
            "args": ["serve"],
            "env": {}
        }),
    );

    // Write back
    let output = serde_json::to_string_pretty(&config)?;
    std::fs::write(&claude_config_path, output).context("cannot write ~/.claude.json")?;

    println!("ICM configured for Claude Code!");
    println!();
    println!("  binary:  {icm_bin_str}");
    println!("  config:  {}", claude_config_path.display());
    println!("  db:      {}", default_db_path().display());
    println!();
    println!("Restart Claude Code to activate ICM memory.");
    println!("All sessions will share the same memory database.");
    println!("Instructions are built into the MCP server — no CLAUDE.md changes needed.");

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
    limit: usize,
) -> Result<()> {
    let memoir = resolve_memoir(store, memoir_name)?;
    let results = store.search_concepts_fts(&memoir.id, query, limit)?;

    if results.is_empty() {
        println!("No concepts found.");
        return Ok(());
    }

    for c in &results {
        print_concept(c);
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
