use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};

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

    /// Memoir commands — permanent knowledge layer
    Memoir {
        #[command(subcommand)]
        command: MemoirCommands,
    },

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

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing_subscriber::filter::LevelFilter::WARN.into()),
        )
        .init();

    let cli = Cli::parse();
    let store = open_store(cli.db)?;

    match cli.command {
        Commands::Store {
            topic,
            content,
            importance,
            keywords,
            raw,
        } => cmd_store(&store, topic, content, importance.into(), keywords, raw),
        Commands::Recall {
            query,
            topic,
            limit,
        } => cmd_recall(&store, &query, topic.as_deref(), limit),
        Commands::List { topic, all, sort } => cmd_list(&store, topic.as_deref(), all, sort),
        Commands::Forget { id } => cmd_forget(&store, &id),
        Commands::Topics => cmd_topics(&store),
        Commands::Stats => cmd_stats(&store),
        Commands::Decay { factor } => cmd_decay(&store, factor),
        Commands::Prune { threshold, dry_run } => cmd_prune(&store, threshold, dry_run),
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
        Commands::Serve => icm_mcp::run_server(&store),
    }
}

// ---------------------------------------------------------------------------
// Memory commands
// ---------------------------------------------------------------------------

fn cmd_store(
    store: &SqliteStore,
    topic: String,
    content: String,
    importance: Importance,
    keywords: Option<String>,
    raw: Option<String>,
) -> Result<()> {
    let mut memory = Memory::new(topic, content, importance);
    if let Some(kw) = keywords {
        memory.keywords = kw.split(',').map(|s| s.trim().to_string()).collect();
    }
    memory.raw_excerpt = raw;

    let id = store.store(memory)?;
    println!("Stored: {id}");
    Ok(())
}

fn cmd_recall(store: &SqliteStore, query: &str, topic: Option<&str>, limit: usize) -> Result<()> {
    // Try FTS first
    let mut results = store.search_fts(query, limit)?;

    // If FTS returns nothing, fall back to keyword search
    if results.is_empty() {
        let keywords: Vec<&str> = query.split_whitespace().collect();
        results = store.search_by_keywords(&keywords, limit)?;
    }

    // Filter by topic if specified
    if let Some(t) = topic {
        results.retain(|m| m.topic == t);
    }

    if results.is_empty() {
        println!("No memories found.");
        return Ok(());
    }

    for mem in &results {
        // Update access for each returned result
        let _ = store.update_access(&mem.id);
        print_memory(mem);
    }

    Ok(())
}

fn cmd_list(store: &SqliteStore, topic: Option<&str>, all: bool, sort: SortField) -> Result<()> {
    let mut memories = if let Some(t) = topic {
        store.get_by_topic(t)?
    } else if all {
        // Get all memories by listing topics and collecting
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
        // Count how many would be pruned
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

    // List concepts
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
        // Use first keyword or topic as concept name, summary as definition
        let concept_name = if !mem.keywords.is_empty() {
            mem.keywords[0].clone()
        } else {
            format!("{}-{}", from_topic, &mem.id[..8])
        };

        // Skip if concept already exists
        if store
            .get_concept_by_name(&memoir.id, &concept_name)?
            .is_some()
        {
            // Refine existing concept with new info
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
            // Convert keywords to labels
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
