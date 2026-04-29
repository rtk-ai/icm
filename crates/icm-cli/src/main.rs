mod bench_data;
mod bench_knowledge;
pub mod cloud;
mod config;
mod extract;
mod import;
#[cfg(test)]
mod learn_tests;
#[cfg(feature = "tui")]
mod tui;
mod upgrade;
#[cfg(feature = "web")]
mod web;

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use serde_json::Value;

use icm_core::{
    build_wake_up, keyword_matches, topic_matches, Concept, ConceptLink, Feedback, FeedbackStore,
    Importance, Label, Memoir, MemoirStore, Memory, MemoryStore, Relation, WakeUpFormat,
    WakeUpOptions, MSG_NO_MEMORIES,
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

    /// Disable embeddings (skip model download, use keyword search only)
    #[arg(long, global = true)]
    no_embeddings: bool,

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

    /// Forget (delete) a memory by ID, or all memories in a topic
    Forget {
        /// Memory ID to forget
        id: Option<String>,

        /// Delete all memories in this topic
        #[arg(short, long)]
        topic: Option<String>,
    },

    /// Update an existing memory in-place
    Update {
        /// Memory ID to update
        id: String,

        /// New content (replaces existing summary)
        #[arg(short, long)]
        content: String,

        /// New importance level (optional, keeps existing if not set)
        #[arg(short, long)]
        importance: Option<CliImportance>,

        /// New keywords (comma-separated, optional)
        #[arg(short, long)]
        keywords: Option<String>,
    },

    /// Show memory health report (staleness, consolidation needs)
    Health {
        /// Check a specific topic (checks all if omitted)
        #[arg(short, long)]
        topic: Option<String>,
    },

    /// Feedback subcommands — record and search prediction corrections
    Feedback {
        #[command(subcommand)]
        command: FeedbackCommands,
    },

    /// Transcript subcommands — verbatim sessions + messages (session replay)
    Transcript {
        #[command(subcommand)]
        command: TranscriptCommands,
    },

    /// Detect recurring patterns in a topic and optionally create memoir concepts
    ExtractPatterns {
        /// Topic to analyze
        #[arg(short, long)]
        topic: String,

        /// Memoir name — if provided, creates concepts from detected patterns
        #[arg(short, long)]
        memoir: Option<String>,

        /// Minimum cluster size to form a pattern (default: 3)
        #[arg(long, default_value = "3")]
        min_cluster_size: usize,
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
        /// Integration mode: mcp, cli, skill, hook, or all (default: mcp)
        #[arg(short, long, default_value = "mcp")]
        mode: InitMode,

        /// Overwrite existing hook entries that point at a stale icm binary path
        /// (e.g. a deleted target/release/icm). Without --force, existing entries
        /// are left untouched, even if their binary path no longer exists.
        #[arg(short, long)]
        force: bool,
    },

    /// Diagnose ICM integration: check hook binary paths in Claude Code settings
    Doctor,

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

        /// Store raw text as low-importance memory when no facts are extracted
        #[arg(long)]
        store_raw: bool,
    },

    /// Import conversations from external sources (Claude.ai, ChatGPT, Claude Code, Slack, text)
    Import {
        /// Path to file or directory to import
        path: PathBuf,

        /// Format (auto-detected if omitted)
        #[arg(short, long, default_value = "auto")]
        format: CliImportFormat,

        /// Project name for topic namespacing
        #[arg(short, long, default_value = "project")]
        project: String,

        /// Preview without storing
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

    /// Auto-recall context for the current project (detects from PWD / git remote)
    RecallProject {
        /// Maximum memories to include
        #[arg(short, long, default_value = "10")]
        limit: usize,
    },

    /// Print a compact critical-facts pack for LLM system-prompt injection
    ///
    /// Selects critical/high memories (and preferences) optionally scoped by
    /// project, ranks them by importance × recency × weight, then truncates
    /// to fit the token budget. Inspired by MemPalace's `wake-up` command.
    WakeUp {
        /// Project filter (default: auto-detect from PWD/git remote; use "-" to disable)
        #[arg(short, long)]
        project: Option<String>,

        /// Approximate token budget (1 token ≈ 4 characters)
        #[arg(short = 't', long, default_value = "200")]
        max_tokens: usize,

        /// Output format
        #[arg(short, long, default_value = "markdown")]
        format: CliWakeUpFormat,

        /// Exclude global preferences/identity memories
        #[arg(long)]
        no_preferences: bool,
    },

    /// Auto-save context for the current project (detects from PWD / git remote)
    SaveProject {
        /// Summary of what was done in this session
        content: String,

        /// Importance level
        #[arg(short, long, default_value = "medium")]
        importance: CliImportance,

        /// Additional keywords (comma-separated)
        #[arg(short, long)]
        keywords: Option<String>,
    },

    /// Scan a project and save its structure as a Memoir knowledge graph
    Learn {
        /// Directory to scan (default: current directory)
        #[arg(short, long)]
        dir: Option<String>,

        /// Memoir name (default: directory name)
        #[arg(short, long)]
        name: Option<String>,
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

    /// Upgrade icm to the latest release (with SHA256 verification)
    Upgrade {
        /// Download and install the new binary (required for actual upgrade)
        #[arg(long)]
        apply: bool,

        /// Only check if an update is available (don't prompt to apply)
        #[arg(long)]
        check: bool,
    },

    /// RTK Cloud commands (login, sync, status)
    Cloud {
        #[command(subcommand)]
        command: CloudCommands,
    },

    /// Launch MCP server (stdio transport for Claude Code)
    Serve {
        /// Compact output mode (shorter responses to save tokens)
        #[arg(long)]
        compact: bool,

        /// Launch web dashboard instead of MCP stdio server
        #[cfg(feature = "web")]
        #[arg(long)]
        expose: bool,
    },

    /// Claude Code hook handlers (read JSON from stdin, output hook response)
    Hook {
        #[command(subcommand)]
        command: HookCommands,
    },

    /// Launch interactive TUI dashboard
    #[cfg(feature = "tui")]
    Dashboard,

    /// Launch interactive TUI dashboard (alias for dashboard)
    #[cfg(feature = "tui")]
    #[command(hide = true)]
    Tui,
}

#[derive(Subcommand)]
enum HookCommands {
    /// PreToolUse hook: auto-allow `icm` CLI commands (no permission prompt)
    Pre,
    /// PostToolUse hook: auto-extract context every N tool calls
    Post {
        /// Extract every N tool calls (default from config, fallback: 10)
        #[arg(long, default_value = "15")]
        every: usize,
    },
    /// PreCompact hook: extract memories from transcript before context compression
    Compact,
    /// UserPromptSubmit hook: inject recalled context at the start of each prompt
    Prompt,
    /// SessionStart hook: inject a wake-up pack of critical facts into the session
    Start {
        /// Approximate token budget for the wake-up pack (0 = use config value)
        #[arg(long, default_value = "0")]
        max_tokens: usize,
    },
    /// SessionEnd hook: extract memories from transcript before the session closes
    End,
}

#[derive(Subcommand)]
enum CloudCommands {
    /// Login to RTK Cloud (OAuth browser or email/password)
    Login {
        /// RTK Cloud endpoint
        #[arg(short, long, default_value = "https://cloud.rtk-ai.app")]
        endpoint: String,
        /// Use email/password instead of browser OAuth
        #[arg(long)]
        password: bool,
    },
    /// Logout from RTK Cloud
    Logout,
    /// Show cloud connection status
    Status,
    /// Push local memories to cloud (project/org scope)
    Push {
        /// Scope to push (project or org)
        #[arg(short, long, default_value = "project")]
        scope: String,
        /// Only push memories from this topic
        #[arg(short, long)]
        topic: Option<String>,
    },
    /// Pull shared memories from cloud
    Pull {
        /// Scope to pull (project or org)
        #[arg(short, long, default_value = "project")]
        scope: String,
        /// Only pull memories updated since this ISO timestamp
        #[arg(long)]
        since: Option<String>,
    },
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

    /// Export memoir graph as JSON or DOT (Graphviz)
    Export {
        /// Memoir name
        #[arg(short, long)]
        memoir: String,

        /// Output format: json or dot
        #[arg(short, long, default_value = "json")]
        format: String,
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

#[derive(Subcommand)]
enum FeedbackCommands {
    /// Record a prediction correction (what AI predicted vs what was correct)
    Record {
        /// Topic/category for the feedback
        #[arg(short, long)]
        topic: String,

        /// Context in which the prediction was made
        #[arg(short, long)]
        context: String,

        /// What the AI predicted
        #[arg(short, long)]
        predicted: String,

        /// What the correct answer was
        #[arg(long)]
        corrected: String,

        /// Why the prediction was wrong (optional)
        #[arg(short, long)]
        reason: Option<String>,

        /// Source of the feedback (e.g. "user", "ci", "review")
        #[arg(short, long, default_value = "cli")]
        source: String,
    },

    /// Search feedback entries
    Search {
        /// Search query
        query: String,

        /// Filter by topic
        #[arg(short, long)]
        topic: Option<String>,

        /// Maximum results
        #[arg(short, long, default_value = "5")]
        limit: usize,
    },

    /// Show feedback statistics
    Stats,
}

#[derive(Subcommand)]
enum TranscriptCommands {
    /// Create a new session and print its id
    StartSession {
        /// Agent identifier (e.g. "claude-code", "cursor")
        #[arg(short, long, default_value = "cli")]
        agent: String,

        /// Project name (optional, usually cwd basename)
        #[arg(short, long)]
        project: Option<String>,

        /// Arbitrary metadata as JSON
        #[arg(short, long)]
        metadata: Option<String>,
    },

    /// Record a single message into a session
    Record {
        /// Session id (from `icm transcript start-session`)
        #[arg(short, long)]
        session: String,

        /// Role: user, assistant, system, or tool
        #[arg(short, long)]
        role: String,

        /// Raw message content
        #[arg(short, long)]
        content: String,

        /// Tool name if role=tool (optional)
        #[arg(short, long)]
        tool: Option<String>,

        /// Token count (optional)
        #[arg(long)]
        tokens: Option<i64>,

        /// Arbitrary metadata as JSON
        #[arg(short, long)]
        metadata: Option<String>,
    },

    /// Full-text search across transcript messages (BM25)
    Search {
        /// Query (FTS5 syntax supported: "postgres OR mysql", "auth*", "\"exact phrase\"")
        query: String,

        /// Only within this session
        #[arg(short, long)]
        session: Option<String>,

        /// Only within this project
        #[arg(short, long)]
        project: Option<String>,

        /// Max results
        #[arg(short, long, default_value = "10")]
        limit: usize,
    },

    /// List all sessions, newest first
    ListSessions {
        /// Filter by project
        #[arg(short, long)]
        project: Option<String>,

        /// Max results
        #[arg(short, long, default_value = "20")]
        limit: usize,
    },

    /// Replay the full message thread of a session, chronologically
    Show {
        /// Session id
        session: String,

        /// Max messages to show
        #[arg(short, long, default_value = "200")]
        limit: usize,
    },

    /// Show global transcript statistics (sessions, messages, bytes, top sessions)
    Stats,

    /// Delete a session and all its messages
    Forget {
        /// Session id
        session: String,
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

#[derive(Clone, Copy, ValueEnum)]
enum CliWakeUpFormat {
    Markdown,
    Plain,
}

impl From<CliWakeUpFormat> for WakeUpFormat {
    fn from(val: CliWakeUpFormat) -> Self {
        match val {
            CliWakeUpFormat::Markdown => WakeUpFormat::Markdown,
            CliWakeUpFormat::Plain => WakeUpFormat::Plain,
        }
    }
}

#[derive(Clone, ValueEnum)]
enum CliImportFormat {
    Auto,
    ClaudeAi,
    Chatgpt,
    ClaudeCode,
    Slack,
    Text,
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
    /// Claude Code PostToolUse hook (auto-extract context)
    Hook,
    /// All integration modes
    All,
}

fn default_db_path() -> PathBuf {
    directories::ProjectDirs::from("dev", "icm", "icm")
        .map(|dirs| dirs.data_dir().join("memories.db"))
        .unwrap_or_else(|| PathBuf::from("memories.db"))
}

fn open_store(
    db: Option<PathBuf>,
    embedding_dims: usize,
) -> Result<(SqliteStore, icm_store::MigrationStatus)> {
    let path = db.unwrap_or_else(default_db_path);
    SqliteStore::with_dims(&path, embedding_dims).context("failed to open database")
}

#[cfg(any(feature = "embeddings", feature = "jina-v5"))]
fn init_embedder(
    cfg: &config::EmbeddingsConfig,
) -> Result<Option<Box<dyn icm_core::Embedder>>> {
    use config::EmbedderBackend;
    match cfg.backend {
        #[cfg(feature = "embeddings")]
        EmbedderBackend::Fastembed => Ok(Some(Box::new(icm_core::FastEmbedder::with_model(
            &cfg.model,
        )))),
        #[cfg(not(feature = "embeddings"))]
        EmbedderBackend::Fastembed => Err(anyhow::anyhow!(
            "config requests backend `fastembed` but this binary was built \
             without the `embeddings` feature. Rebuild with \
             `--features embeddings` or set `embeddings.backend = \"jina-v5-nano\"`."
        )),
        #[cfg(feature = "jina-v5")]
        EmbedderBackend::JinaV5Nano => {
            let emb = icm_core::JinaV5NanoEmbedder::new(cfg.truncate_dim)
                .map_err(|e| anyhow::anyhow!("jina-v5-nano init: {e}"))?;
            Ok(Some(Box::new(emb)))
        }
        #[cfg(not(feature = "jina-v5"))]
        EmbedderBackend::JinaV5Nano => Err(anyhow::anyhow!(
            "config requests backend `jina-v5-nano` but this binary was built \
             without the `jina-v5` feature. Rebuild with `--features jina-v5` \
             or set `embeddings.backend = \"fastembed\"`."
        )),
    }
}

#[cfg(not(any(feature = "embeddings", feature = "jina-v5")))]
fn init_embedder(
    cfg: &config::EmbeddingsConfig,
) -> Result<Option<Box<dyn icm_core::Embedder>>> {
    use config::EmbedderBackend;
    match cfg.backend {
        EmbedderBackend::Fastembed => Err(anyhow::anyhow!(
            "config requests backend `fastembed` but this binary was built \
             without the `embeddings` feature."
        )),
        EmbedderBackend::JinaV5Nano => Err(anyhow::anyhow!(
            "config requests backend `jina-v5-nano` but this binary was built \
             without the `jina-v5` feature."
        )),
    }
}

fn main() -> Result<()> {
    // Reset SIGPIPE to default so piped commands (e.g. `icm export | head`)
    // don't panic on broken pipe.
    #[cfg(unix)]
    {
        unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing_subscriber::filter::LevelFilter::WARN.into()),
        )
        .init();

    let cli = Cli::parse();
    let cfg = config::load_config()?;
    let embeddings_enabled =
        cfg.embeddings.enabled && !cli.no_embeddings && std::env::var("ICM_NO_EMBEDDINGS").is_err();
    let embedder: Option<Box<dyn icm_core::Embedder>> = if embeddings_enabled {
        init_embedder(&cfg.embeddings)?
    } else {
        None
    };
    let embedding_dims = embedder
        .as_ref()
        .map(|e| e.dimensions())
        .unwrap_or(icm_core::DEFAULT_EMBEDDING_DIMS);
    let db_path = cli.db.clone().unwrap_or_else(default_db_path);
    let (store, migration_status) = open_store(cli.db, embedding_dims)?;
    if migration_status.dim_changed {
        eprintln!(
            "Embedding dim changed ({} -> {}): {} memories need re-embedding. \
             Run `icm embed --all` to repopulate.",
            migration_status.old_dim,
            migration_status.new_dim,
            migration_status.affected_rows
        );
    }

    match cli.command {
        Commands::Store {
            topic,
            content,
            importance,
            keywords,
            raw,
        } => {
            let emb_ref = embedder.as_deref();
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
            let emb_ref = embedder.as_deref();
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
        Commands::Forget { id, topic } => cmd_forget(&store, id.as_deref(), topic.as_deref()),
        Commands::Update {
            id,
            content,
            importance,
            keywords,
        } => {
            let emb_ref = embedder.as_deref();
            cmd_update(&store, emb_ref, &id, content, importance, keywords)
        }
        Commands::Health { topic } => cmd_health(&store, topic.as_deref()),
        Commands::Feedback { command } => match command {
            FeedbackCommands::Record {
                topic,
                context,
                predicted,
                corrected,
                reason,
                source,
            } => cmd_feedback_record(&store, topic, context, predicted, corrected, reason, source),
            FeedbackCommands::Search {
                query,
                topic,
                limit,
            } => cmd_feedback_search(&store, &query, topic.as_deref(), limit),
            FeedbackCommands::Stats => cmd_feedback_stats(&store),
        },
        Commands::Transcript { command } => match command {
            TranscriptCommands::StartSession {
                agent,
                project,
                metadata,
            } => cmd_transcript_start_session(
                &store,
                &agent,
                project.as_deref(),
                metadata.as_deref(),
            ),
            TranscriptCommands::Record {
                session,
                role,
                content,
                tool,
                tokens,
                metadata,
            } => cmd_transcript_record(
                &store,
                &session,
                &role,
                &content,
                tool.as_deref(),
                tokens,
                metadata.as_deref(),
            ),
            TranscriptCommands::Search {
                query,
                session,
                project,
                limit,
            } => cmd_transcript_search(
                &store,
                &query,
                session.as_deref(),
                project.as_deref(),
                limit,
            ),
            TranscriptCommands::ListSessions { project, limit } => {
                cmd_transcript_list_sessions(&store, project.as_deref(), limit)
            }
            TranscriptCommands::Show { session, limit } => {
                cmd_transcript_show(&store, &session, limit)
            }
            TranscriptCommands::Stats => cmd_transcript_stats(&store),
            TranscriptCommands::Forget { session } => cmd_transcript_forget(&store, &session),
        },
        Commands::ExtractPatterns {
            topic,
            memoir,
            min_cluster_size,
        } => cmd_extract_patterns(&store, &topic, memoir.as_deref(), min_cluster_size),
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
                let emb = match embedder.as_deref() {
                    Some(e) => e,
                    None => bail!("embeddings not available — check your configuration"),
                };
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
            MemoirCommands::Export { memoir, format } => {
                cmd_memoir_export(&store, &memoir, &format)
            }
            MemoirCommands::Distill { from_topic, into } => {
                cmd_memoir_distill(&store, &from_topic, &into)
            }
        },
        Commands::Init { mode, force } => cmd_init(mode, force),
        Commands::Doctor => cmd_doctor(),
        Commands::Extract {
            project,
            text,
            dry_run,
            store_raw,
        } => cmd_extract(&store, &project, text, dry_run, store_raw),
        Commands::Import {
            path,
            format,
            project,
            dry_run,
        } => {
            let fmt = match format {
                CliImportFormat::Auto => None,
                CliImportFormat::ClaudeAi => Some(import::ImportFormat::ClaudeAi),
                CliImportFormat::Chatgpt => Some(import::ImportFormat::ChatGpt),
                CliImportFormat::ClaudeCode => Some(import::ImportFormat::ClaudeCode),
                CliImportFormat::Slack => Some(import::ImportFormat::Slack),
                CliImportFormat::Text => Some(import::ImportFormat::Text),
            };
            import::cmd_import(&store, path, fmt, project, dry_run)
        }
        Commands::RecallContext { query, limit } => cmd_recall_context(&store, &query, limit),
        Commands::RecallProject { limit } => cmd_recall_project(&store, limit),
        Commands::WakeUp {
            project,
            max_tokens,
            format,
            no_preferences,
        } => cmd_wake_up(&store, project, max_tokens, format, no_preferences),
        Commands::SaveProject {
            content,
            importance,
            keywords,
        } => {
            let emb_ref = embedder.as_deref();
            cmd_save_project(&store, emb_ref, &content, importance.into(), keywords)
        }
        Commands::Learn { dir, name } => {
            let dir = dir
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
            let result = icm_core::learn_project(&store, &dir, name.as_deref())?;
            println!("{result}");
            Ok(())
        }
        Commands::Config => cmd_config(),
        Commands::Upgrade { apply, check } => upgrade::cmd_upgrade(apply, check),
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
        Commands::Cloud { command } => cmd_cloud(command, &store),
        Commands::Serve {
            compact,
            #[cfg(feature = "web")]
            expose,
        } => {
            #[cfg(feature = "web")]
            if expose {
                let password = web::resolve_password(&cfg.web)?;
                return web::run_web_server(
                    store,
                    &cfg.web.host,
                    cfg.web.port,
                    cfg.web.username.clone(),
                    password,
                );
            }
            let emb_ref = embedder.as_deref();
            // --compact flag overrides, otherwise use config (default: true)
            let use_compact = compact || cfg.mcp.compact;
            icm_mcp::run_server(&store, emb_ref, use_compact)
        }
        Commands::Hook { command } => match command {
            HookCommands::Pre => cmd_hook_pre(),
            HookCommands::Post { every } => {
                let extract_every = if every != 15 {
                    every // CLI flag overrides config
                } else {
                    cfg.extraction.extract_every
                };
                cmd_hook_post(&store, extract_every, cfg.extraction.store_raw)
            }
            HookCommands::Compact => cmd_hook_compact(&store),
            HookCommands::Prompt => cmd_hook_prompt(&store),
            HookCommands::Start { max_tokens } => {
                let tokens = if max_tokens > 0 {
                    max_tokens
                } else {
                    cfg.wakeup.max_tokens
                };
                cmd_hook_start(&store, tokens)
            }
            HookCommands::End => cmd_hook_end(&store),
        },
        #[cfg(feature = "tui")]
        Commands::Dashboard => {
            let db_path_str = db_path.to_string_lossy().to_string();
            tui::run_dashboard(&store, Some(&db_path_str))
        }
        #[cfg(feature = "tui")]
        Commands::Tui => {
            let db_path_str = db_path.to_string_lossy().to_string();
            tui::run_dashboard(&store, Some(&db_path_str))
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
        match emb.embed(&memory.embed_text()) {
            Ok(vec) => memory.embedding = Some(vec),
            Err(e) => eprintln!("warning: embedding failed: {e}"),
        }
    }

    // Auto-link: wire the new memory into the existing graph before
    // persisting. No-op when embedding is unavailable.
    let auto_link_opts = icm_core::AutoLinkOptions::default();
    let linked_ids = if memory.embedding.is_some() {
        icm_core::auto_link_memory(store, &mut memory, &auto_link_opts).unwrap_or_else(|e| {
            eprintln!("warning: auto-link failed: {e}");
            Vec::new()
        })
    } else {
        Vec::new()
    };

    let id = store.store(memory)?;

    // Back-refs: update each linked memory so the edges are bidirectional.
    if !linked_ids.is_empty() {
        if let Err(e) = icm_core::add_backrefs(store, &id, &linked_ids) {
            eprintln!("warning: auto-link back-refs failed: {e}");
        }
    }

    if linked_ids.is_empty() {
        println!("Stored: {id}");
    } else {
        println!(
            "Stored: {id} (+{} link{})",
            linked_ids.len(),
            if linked_ids.len() == 1 { "" } else { "s" }
        );
    }
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
    if let Err(e) = store.maybe_auto_decay() {
        tracing::warn!(error = %e, "auto-decay failed during recall");
    }

    // Try hybrid search if embedder is available
    if let Some(emb) = embedder {
        if let Ok(query_emb) = emb.embed_query(query) {
            if let Ok(results) = store.search_hybrid(query, &query_emb, limit) {
                let mut scored = results;
                if let Some(t) = topic {
                    scored.retain(|(m, _)| topic_matches(&m.topic, t));
                }
                if let Some(kw) = keyword {
                    scored.retain(|(m, _)| keyword_matches(&m.keywords, kw));
                }

                // Graph-aware expansion: follow related_ids one hop and
                // fold neighbors into results (discounted 0.5× score).
                let max_neighbors = (limit / 3).max(1);
                let expanded = store
                    .expand_with_neighbors(&scored, max_neighbors, 0.5, limit)
                    .unwrap_or(scored);

                if expanded.is_empty() {
                    println!("{MSG_NO_MEMORIES}");
                    return Ok(());
                }

                let ids: Vec<&str> = expanded.iter().map(|(m, _)| m.id.as_str()).collect();
                let _ = store.batch_update_access(&ids);
                for (mem, score) in &expanded {
                    print_memory_detail(mem, Some(*score));
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
        results.retain(|m| topic_matches(&m.topic, t));
    }
    if let Some(kw) = keyword {
        results.retain(|m| keyword_matches(&m.keywords, kw));
    }

    // Graph-aware expansion also runs in the keyword-only fallback path.
    let scored: Vec<(Memory, f32)> = results.into_iter().map(|m| (m, 1.0)).collect();
    let max_neighbors = (limit / 3).max(1);
    let expanded = store
        .expand_with_neighbors(&scored, max_neighbors, 0.5, limit)
        .unwrap_or(scored);

    if expanded.is_empty() {
        println!("{MSG_NO_MEMORIES}");
        return Ok(());
    }

    let ids: Vec<&str> = expanded.iter().map(|(m, _)| m.id.as_str()).collect();
    let _ = store.batch_update_access(&ids);
    for (mem, _) in &expanded {
        print_memory_detail(mem, None);
    }

    Ok(())
}

fn cmd_list(store: &SqliteStore, topic: Option<&str>, all: bool, sort: SortField) -> Result<()> {
    let mut memories = if let Some(t) = topic {
        store.get_by_topic(t)?
    } else if all {
        store.list_all()?
    } else {
        println!("Use --topic <name> or --all to list memories.");
        return Ok(());
    };

    match sort {
        SortField::Weight => memories.sort_by(|a, b| b.weight.partial_cmp(&a.weight).unwrap()),
        SortField::Created => memories.sort_by_key(|b| std::cmp::Reverse(b.created_at)),
        SortField::Accessed => memories.sort_by_key(|b| std::cmp::Reverse(b.last_accessed)),
    }

    if memories.is_empty() {
        println!("{MSG_NO_MEMORIES}");
        return Ok(());
    }

    for mem in &memories {
        print_memory_detail(mem, None);
    }

    Ok(())
}

fn cmd_forget(store: &SqliteStore, id: Option<&str>, topic: Option<&str>) -> Result<()> {
    match (id, topic) {
        (_, Some(topic)) => {
            let memories = store.get_by_topic(topic)?;
            let count = memories.len();
            for m in &memories {
                store.delete(&m.id)?;
            }
            println!("Deleted {count} memories from topic: {topic}");
        }
        (Some(id), None) => {
            store.delete(id)?;
            println!("Deleted: {id}");
        }
        (None, None) => {
            anyhow::bail!("either --topic or a memory ID is required");
        }
    }
    Ok(())
}

fn cmd_update(
    store: &SqliteStore,
    embedder: Option<&dyn icm_core::Embedder>,
    id: &str,
    content: String,
    importance: Option<CliImportance>,
    keywords: Option<String>,
) -> Result<()> {
    let mut memory = store
        .get(id)?
        .with_context(|| format!("memory not found: {id}"))?;

    memory.summary = content.clone();
    memory.updated_at = chrono::Utc::now();
    memory.weight = 1.0; // Reset weight on update (refreshed content)

    if let Some(imp) = importance {
        memory.importance = imp.into();
    }

    if let Some(kw) = keywords {
        memory.keywords = kw.split(',').map(|s| s.trim().to_string()).collect();
    }

    // Re-embed if embedder available
    if let Some(emb) = embedder {
        match emb.embed(&memory.embed_text()) {
            Ok(vec) => memory.embedding = Some(vec),
            Err(e) => eprintln!("warning: re-embedding failed: {e}"),
        }
    }

    store.update(&memory)?;
    println!("Updated: {id}");
    Ok(())
}

fn cmd_health(store: &SqliteStore, topic_filter: Option<&str>) -> Result<()> {
    let topics = if let Some(t) = topic_filter {
        vec![(t.to_string(), 0usize)]
    } else {
        store.list_topics()?
    };

    if topics.is_empty() {
        println!("No topics yet.");
        return Ok(());
    }

    println!(
        "{:<30} {:<20} {:>7} {:>8} {:>6}",
        "Topic", "Status", "Entries", "AvgWgt", "Stale"
    );
    println!("{}", "-".repeat(75));

    let mut total_stale = 0usize;
    let mut needs_consolidation = 0usize;

    for (topic, _) in &topics {
        match store.topic_health(topic) {
            Ok(health) => {
                let status = health.status();

                println!(
                    "{:<30} {:<20} {:>7} {:>8.2} {:>6}",
                    topic, status, health.entry_count, health.avg_weight, health.stale_count
                );

                if health.needs_consolidation {
                    needs_consolidation += 1;
                }
                total_stale += health.stale_count;
            }
            Err(_) => {
                println!("{:<30} (error reading)", topic);
            }
        }
    }

    println!("{}", "-".repeat(75));
    println!(
        "{} topics, {} need consolidation, {} stale entries",
        topics.len(),
        needs_consolidation,
        total_stale
    );
    Ok(())
}

fn cmd_feedback_record(
    store: &SqliteStore,
    topic: String,
    context: String,
    predicted: String,
    corrected: String,
    reason: Option<String>,
    source: String,
) -> Result<()> {
    let feedback = Feedback::new(
        topic.clone(),
        context,
        predicted.clone(),
        corrected.clone(),
        reason,
        source,
    );
    let id = store.store_feedback(feedback)?;
    println!("Feedback recorded: {id}");
    println!("  topic: {topic}");
    println!("  predicted: {predicted}");
    println!("  corrected: {corrected}");
    Ok(())
}

fn cmd_feedback_search(
    store: &SqliteStore,
    query: &str,
    topic: Option<&str>,
    limit: usize,
) -> Result<()> {
    let results = store.search_feedback(query, topic, limit)?;
    if results.is_empty() {
        println!("No feedback found.");
        return Ok(());
    }

    for fb in &results {
        println!("--- {} [{}] ---", fb.id, fb.topic);
        println!("  context:   {}", fb.context);
        println!("  predicted: {}", fb.predicted);
        println!("  corrected: {}", fb.corrected);
        if let Some(ref reason) = fb.reason {
            println!("  reason:    {reason}");
        }
        if !fb.source.is_empty() {
            println!("  source:    {}", fb.source);
        }
        if fb.applied_count > 0 {
            println!("  applied:   {} times", fb.applied_count);
        }
    }
    Ok(())
}

fn cmd_feedback_stats(store: &SqliteStore) -> Result<()> {
    let stats = store.feedback_stats()?;
    println!("Feedback total: {}", stats.total);

    if !stats.by_topic.is_empty() {
        println!("\nBy topic:");
        for (topic, count) in &stats.by_topic {
            println!("  {topic}: {count}");
        }
    }

    if !stats.most_applied.is_empty() {
        println!("\nMost applied:");
        for (id, count) in &stats.most_applied {
            println!("  {id}: {count} times");
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Transcript commands — verbatim sessions + messages
// ---------------------------------------------------------------------------

fn cmd_transcript_start_session(
    store: &SqliteStore,
    agent: &str,
    project: Option<&str>,
    metadata: Option<&str>,
) -> Result<()> {
    use icm_core::TranscriptStore;
    let id = store.create_session(agent, project, metadata)?;
    println!("{id}");
    Ok(())
}

fn cmd_transcript_record(
    store: &SqliteStore,
    session: &str,
    role: &str,
    content: &str,
    tool: Option<&str>,
    tokens: Option<i64>,
    metadata: Option<&str>,
) -> Result<()> {
    use icm_core::{Role, TranscriptStore};
    let parsed_role = Role::parse(role)
        .ok_or_else(|| anyhow::anyhow!("role must be user|assistant|system|tool, got '{role}'"))?;
    let id = store.record_message(session, parsed_role, content, tool, tokens, metadata)?;
    println!("{id}");
    Ok(())
}

fn cmd_transcript_search(
    store: &SqliteStore,
    query: &str,
    session: Option<&str>,
    project: Option<&str>,
    limit: usize,
) -> Result<()> {
    use icm_core::TranscriptStore;
    let hits = store.search_transcripts(query, session, project, limit)?;
    if hits.is_empty() {
        println!("No matches.");
        return Ok(());
    }
    for hit in hits {
        let preview: String = hit.message.content.chars().take(280).collect();
        let suffix = if hit.message.content.chars().count() > 280 {
            "…"
        } else {
            ""
        };
        let proj = hit.session.project.as_deref().unwrap_or("-");
        println!("--- {} ---", hit.message.id);
        println!(
            "  session:  {} ({}, project={}, agent={})",
            hit.session.id, hit.message.role, proj, hit.session.agent
        );
        println!("  ts:       {}", hit.message.ts.format("%Y-%m-%d %H:%M:%S"));
        println!("  score:    {:.3}", hit.score);
        if let Some(t) = &hit.message.tool_name {
            println!("  tool:     {t}");
        }
        println!("  content:  {preview}{suffix}");
        println!();
    }
    Ok(())
}

fn cmd_transcript_list_sessions(
    store: &SqliteStore,
    project: Option<&str>,
    limit: usize,
) -> Result<()> {
    use icm_core::TranscriptStore;
    let sessions = store.list_sessions(project, limit)?;
    if sessions.is_empty() {
        println!("No sessions.");
        return Ok(());
    }
    println!(
        "{:<28} {:<14} {:<18} {:<20} {:<20}",
        "ID", "AGENT", "PROJECT", "STARTED", "UPDATED"
    );
    println!("{}", "-".repeat(102));
    for s in sessions {
        let proj = s.project.as_deref().unwrap_or("-");
        let short_id = if s.id.len() > 26 { &s.id[..26] } else { &s.id };
        println!(
            "{:<28} {:<14} {:<18} {:<20} {:<20}",
            short_id,
            truncate(&s.agent, 14),
            truncate(proj, 18),
            s.started_at.format("%Y-%m-%d %H:%M:%S"),
            s.updated_at.format("%Y-%m-%d %H:%M:%S"),
        );
    }
    Ok(())
}

fn cmd_transcript_show(store: &SqliteStore, session: &str, limit: usize) -> Result<()> {
    use icm_core::TranscriptStore;
    let meta = store.get_session(session)?;
    let meta = match meta {
        Some(s) => s,
        None => {
            println!("Session not found: {session}");
            return Ok(());
        }
    };
    println!("=== Session {} ===", meta.id);
    println!(
        "agent={} project={} started={} updated={}",
        meta.agent,
        meta.project.as_deref().unwrap_or("-"),
        meta.started_at.format("%Y-%m-%d %H:%M:%S"),
        meta.updated_at.format("%Y-%m-%d %H:%M:%S"),
    );
    println!();

    let messages = store.list_session_messages(session, limit, 0)?;
    for m in messages {
        let ts = m.ts.format("%H:%M:%S");
        let tool = m
            .tool_name
            .as_ref()
            .map(|t| format!(" [{t}]"))
            .unwrap_or_default();
        let tokens = m.tokens.map(|t| format!(" ({t}t)")).unwrap_or_default();
        println!("[{ts}] {}{tool}{tokens}", m.role);
        for line in m.content.lines() {
            println!("    {line}");
        }
        println!();
    }
    Ok(())
}

fn cmd_transcript_stats(store: &SqliteStore) -> Result<()> {
    use icm_core::TranscriptStore;
    let s = store.transcript_stats()?;
    println!("Sessions:      {}", s.total_sessions);
    println!("Messages:      {}", s.total_messages);
    println!(
        "Bytes:         {} ({:.1} KB)",
        s.total_bytes,
        s.total_bytes as f64 / 1024.0
    );
    if let (Some(o), Some(n)) = (&s.oldest, &s.newest) {
        println!(
            "Range:         {} -> {}",
            o.format("%Y-%m-%d %H:%M"),
            n.format("%Y-%m-%d %H:%M")
        );
    }
    if !s.by_role.is_empty() {
        println!("\nBy role:");
        for (role, count) in &s.by_role {
            println!("  {role}: {count}");
        }
    }
    if !s.by_agent.is_empty() {
        println!("\nBy agent:");
        for (agent, count) in &s.by_agent {
            let label = if agent.is_empty() {
                "(unset)"
            } else {
                agent.as_str()
            };
            println!("  {label}: {count}");
        }
    }
    if !s.top_sessions.is_empty() {
        println!("\nTop sessions:");
        for (sid, count) in &s.top_sessions {
            let short = if sid.len() > 26 { &sid[..26] } else { sid };
            println!("  {short}  {count} msg");
        }
    }
    Ok(())
}

fn cmd_transcript_forget(store: &SqliteStore, session: &str) -> Result<()> {
    use icm_core::TranscriptStore;
    store.forget_session(session)?;
    println!("Deleted session {session}");
    Ok(())
}

// ---------------------------------------------------------------------------
// Hook commands (full Rust, no shell scripts)
// ---------------------------------------------------------------------------

/// PreToolUse hook: auto-allow `icm` CLI commands.
/// Reads JSON from stdin, outputs hook response JSON to stdout.
fn cmd_hook_pre() -> Result<()> {
    use std::io::Read;
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;

    let json: Value = match serde_json::from_str(&input) {
        Ok(v) => v,
        Err(_) => return Ok(()), // Malformed input — pass through silently
    };

    // Only handle Bash/shell tool calls (name varies by tool:
    //   Claude Code/Codex: "Bash", Gemini CLI: "run_shell_command")
    let tool_name = json.get("tool_name").and_then(|v| v.as_str()).unwrap_or("");
    if !matches!(tool_name, "Bash" | "run_shell_command") {
        return Ok(());
    }

    // Command path varies: Claude/Codex use tool_input.command,
    // Gemini uses tool_input.command or input.command
    let cmd = json
        .pointer("/tool_input/command")
        .or_else(|| json.pointer("/input/command"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if cmd.is_empty() {
        return Ok(());
    }

    // Check if command involves `icm`
    if !is_icm_command(cmd) {
        return Ok(());
    }

    // Auto-allow: output hook response JSON
    let tool_input = json
        .get("tool_input")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));

    let response = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "allow",
            "permissionDecisionReason": "ICM auto-allow",
            "updatedInput": tool_input
        }
    });

    println!("{}", serde_json::to_string(&response)?);
    Ok(())
}

/// Check if a bash command involves `icm`.
fn is_icm_command(cmd: &str) -> bool {
    // Split on shell operators and check each segment
    for segment in cmd.split(['&', '|', ';']) {
        let trimmed = segment.trim().trim_start_matches('(');
        if trimmed == "icm" || trimmed.starts_with("icm ") {
            return true;
        }
    }
    false
}

/// PostToolUse hook: auto-extract context every N tool calls.
/// Reads JSON from stdin. Runs extraction asynchronously.
fn cmd_hook_post(store: &SqliteStore, extract_every: usize, store_raw: bool) -> Result<()> {
    use std::io::Read;
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;

    let json: Value = match serde_json::from_str(&input) {
        Ok(v) => v,
        Err(_) => return Ok(()),
    };

    let tool_name = json.get("tool_name").and_then(|v| v.as_str()).unwrap_or("");

    // Skip ICM's own tools (avoid infinite loop)
    if tool_name.starts_with("icm_") || tool_name.starts_with("mcp__icm__") {
        return Ok(());
    }

    // Track tool calls in SQLite (atomic, persists across reboots)
    let count = store.increment_hook_counter().unwrap_or(1);

    // Not time to extract yet
    if count < extract_every {
        return Ok(());
    }

    // Reset counter after triggering extraction
    let _ = store.reset_hook_counter();

    // Extract from tool output
    let tool_output = json
        .get("tool_output")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if tool_output.is_empty() {
        return Ok(());
    }

    // Get project name from cwd
    let project = std::env::current_dir()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
        .unwrap_or_else(|| "project".to_string());

    // Extract facts and store (with raw text fallback if enabled)
    match extract::extract_and_store_with_opts(store, tool_output, &project, store_raw) {
        Ok(n) if n > 0 => eprintln!("[icm] auto-extracted {n} facts from tool output"),
        _ => {}
    }

    Ok(())
}

/// PreCompact hook (Layer 1): extract memories from transcript before context compression.
fn cmd_hook_compact(store: &SqliteStore) -> Result<()> {
    extract_from_hook_transcript(store, "pre-compact")
}

/// SessionEnd hook (Layer 1b): extract memories from transcript before the
/// session terminates. Catches the `/exit`, `/clear`, and tool-quit paths
/// that PreCompact misses (compaction does not fire on `/clear`).
///
/// Same transcript-parsing logic as PreCompact — the only difference is the
/// log prefix. SqliteStore handles its own dedup so a session that triggers
/// both PreCompact and SessionEnd back-to-back will not double-store facts.
fn cmd_hook_end(store: &SqliteStore) -> Result<()> {
    extract_from_hook_transcript(store, "session-end")
}

/// Read JSON from stdin, locate the transcript file, parse the last 100
/// assistant messages, and extract facts. Used by both PreCompact and
/// SessionEnd hooks. `source` is purely a log-prefix tag.
///
/// Reads JSON from stdin with `transcript_path`, reads the JSONL transcript,
/// and extracts facts from assistant messages.
fn extract_from_hook_transcript(store: &SqliteStore, source: &str) -> Result<()> {
    use std::io::Read;
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;

    let json: Value = match serde_json::from_str(&input) {
        Ok(v) => v,
        Err(_) => return Ok(()),
    };

    let transcript_path = match json.get("transcript_path").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => return Ok(()), // No transcript path — nothing to do
    };

    let transcript = match std::fs::read_to_string(transcript_path) {
        Ok(t) => t,
        Err(_) => return Ok(()), // Can't read transcript — fail silently
    };

    // Extract assistant text from the last 100 JSONL lines.
    // Supported formats:
    //   Claude Code: {"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"..."}]}}
    //   Codex:       {"type":"response_item","payload":{"role":"developer","content":[{"type":"text","text":"..."}]}}
    //   Simple:      {"role":"assistant","content":"..."}
    let mut assistant_text = String::new();
    for line in transcript.lines().rev().take(100) {
        if let Ok(entry) = serde_json::from_str::<Value>(line) {
            // Find the message object (varies by format)
            let msg = if entry.get("type").and_then(|t| t.as_str()) == Some("assistant") {
                // Claude Code: type=assistant, content in message.*
                entry.get("message")
            } else if entry.get("type").and_then(|t| t.as_str()) == Some("response_item") {
                // Codex: type=response_item, content in payload.*
                entry.get("payload")
            } else if entry.get("role").is_some() {
                // Simple format: role+content at top level
                Some(&entry)
            } else {
                None
            };

            let msg = match msg {
                Some(m) => m,
                None => continue,
            };

            // Check role (assistant, developer, model — varies by tool)
            let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("");
            if !matches!(role, "assistant" | "developer" | "model") {
                continue;
            }

            // Content as array of {type: "text", text: "..."}
            if let Some(arr) = msg.get("content").and_then(|c| c.as_array()) {
                for block in arr {
                    if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                        if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                            assistant_text.push_str(text);
                            assistant_text.push('\n');
                        }
                    }
                }
            }
            // Content as plain string
            else if let Some(content) = msg.get("content").and_then(|c| c.as_str()) {
                assistant_text.push_str(content);
                assistant_text.push('\n');
            }
        }
    }

    if assistant_text.is_empty() {
        return Ok(());
    }

    // Truncate to last 4000 chars to keep extraction reasonable
    let text = if assistant_text.len() > 4000 {
        &assistant_text[assistant_text.len() - 4000..]
    } else {
        &assistant_text
    };

    let project = json
        .get("cwd")
        .and_then(|v| v.as_str())
        .and_then(|p| std::path::Path::new(p).file_name())
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "project".to_string());

    match extract::extract_and_store_with_opts(store, text, &project, true) {
        Ok(n) if n > 0 => eprintln!("[icm] {source}: extracted {n} facts from transcript"),
        _ => {}
    }

    Ok(())
}

/// Truncate `s` to at most `max_bytes` bytes, cutting at the nearest preceding
/// UTF-8 char boundary. Result length is always `<= max_bytes`. Never panics —
/// bare `&s[..max_bytes]` does when the offset lands inside a multi-byte char
/// (Cyrillic=2B, CJK=3B, emoji=4B). See issue #110.
pub(crate) fn truncate_at_char_boundary(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    // Walk backwards from max_bytes until we land on a char boundary.
    // `is_char_boundary(0)` is always true, so this terminates.
    let mut end = max_bytes;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// UserPromptSubmit hook (Layer 2): inject recalled context at the start of each prompt.
/// Reads JSON from stdin with `user_message`, recalls relevant memories,
/// and prints context to stdout (Claude Code appends it as system-reminder).
fn cmd_hook_prompt(store: &SqliteStore) -> Result<()> {
    use std::io::Read;
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;

    let json: Value = match serde_json::from_str(&input) {
        Ok(v) => v,
        Err(_) => return Ok(()),
    };

    // Extract query from user message (field name varies by tool:
    //   Claude Code: "user_message", Codex: "user_message",
    //   Gemini BeforeAgent: "prompt" or "input", fallback: "message")
    let message = json
        .get("user_message")
        .or_else(|| json.get("prompt"))
        .or_else(|| json.get("input"))
        .or_else(|| json.get("message"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if message.is_empty() {
        return Ok(());
    }

    // Project name (from hook cwd) is used as a hard filter on recalled
    // memories — not as a soft hint embedded in the FTS query, which used
    // to let high-FTS-score memories from other projects bleed in.
    let project = json
        .get("cwd")
        .and_then(|v| v.as_str())
        .and_then(|p| std::path::Path::new(p).file_name())
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    // Truncate query to at most 200 bytes at a safe UTF-8 char boundary.
    // See issue #110 — bare `&query[..200]` panics when the cut lands inside
    // a multi-byte UTF-8 char (Cyrillic=2B, CJK=3B, emoji=4B).
    let query = truncate_at_char_boundary(message, 200);

    let project_filter = if project.is_empty() {
        None
    } else {
        Some(project.as_str())
    };
    let ctx = extract::recall_context(store, query, project_filter, 5)?;
    if !ctx.is_empty() {
        print!("{ctx}");
    }

    Ok(())
}

/// SessionStart hook (Layer 0): inject a wake-up pack of critical memories at
/// session start. Reads `cwd` from the Claude Code hook JSON to auto-detect
/// the project, builds the pack via `build_wake_up`, and writes it to stdout.
///
/// Claude Code injects stdout from SessionStart hooks as additional system
/// context for the session. If the pack is empty (no critical memories), we
/// write nothing so the session starts unchanged.
///
/// **Trust boundary**: the pack content is drawn from the user's own ICM
/// store and auto-injected into the session without user confirmation.
/// Summaries are sanitized (newlines flattened in `wake_up::sanitize_summary`)
/// but backticks / code fences / prompt-injection markers are NOT escaped.
/// This is acceptable because ICM memories are user-authored — the user is
/// the only party who can influence the injected content.
///
/// Set `ICM_HOOK_DEBUG=1` in the environment to get stderr diagnostics when
/// the hook decides to suppress output (empty store, no matching memories).
fn cmd_hook_start(store: &SqliteStore, max_tokens: usize) -> Result<()> {
    use std::io::Read;
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;

    let pack = build_hook_start_pack(store, &input, max_tokens)?;
    if pack.is_empty() {
        if std::env::var("ICM_HOOK_DEBUG").is_ok() {
            eprintln!("[icm hook start] suppressed (empty store or no matching memories)");
        }
        return Ok(());
    }
    print!("{pack}");
    Ok(())
}

/// Build the SessionStart wake-up pack from hook stdin + store. Pure helper
/// for unit testing: no I/O beyond the store query.
///
/// Returns the pack as a String, or an empty string if there is nothing
/// meaningful to inject (empty store, or placeholder output).
fn build_hook_start_pack(
    store: &SqliteStore,
    stdin_json: &str,
    max_tokens: usize,
) -> Result<String> {
    // Tolerate missing/malformed stdin — fall back to PWD-based detection.
    let cwd: Option<String> = serde_json::from_str::<Value>(stdin_json)
        .ok()
        .and_then(|v| v.get("cwd").and_then(|c| c.as_str()).map(String::from));

    let project_name = match cwd.as_deref() {
        Some(path) if !path.is_empty() => project_from_path(path),
        _ => {
            let detected = detect_project();
            if detected.is_empty() || detected == "unknown" {
                None
            } else {
                Some(detected)
            }
        }
    };

    let opts = icm_core::WakeUpOptions {
        project: project_name.as_deref(),
        max_tokens,
        format: icm_core::WakeUpFormat::Markdown,
        include_preferences: true,
    };

    let pack = icm_core::build_wake_up(store, &opts)?;

    // If the store is empty, skip injecting the placeholder output into the
    // session — let the user start clean. We detect the empty case via the
    // exported header constant, not substring matching the body, to stay
    // decoupled from the exact wording in `icm_core::wake_up::render()`.
    if pack.trim().is_empty() || pack.starts_with(icm_core::EMPTY_PACK_HEADER) {
        return Ok(String::new());
    }

    Ok(pack)
}

/// Extract a project name from a filesystem path (basename), treating empty
/// or root paths as "no project".
fn project_from_path(path: &str) -> Option<String> {
    let p = std::path::Path::new(path);
    p.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .filter(|s| !s.is_empty() && s != "/")
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

fn cmd_extract_patterns(
    store: &SqliteStore,
    topic: &str,
    memoir: Option<&str>,
    min_cluster_size: usize,
) -> Result<()> {
    let patterns = store.detect_patterns(topic, min_cluster_size)?;

    if patterns.is_empty() {
        println!("No patterns detected in topic '{topic}' (min cluster size: {min_cluster_size}).");
        return Ok(());
    }

    println!(
        "Detected {} pattern(s) in topic '{topic}':\n",
        patterns.len()
    );

    for (i, cluster) in patterns.iter().enumerate() {
        println!(
            "  Pattern #{}: {} memories, keywords: [{}]",
            i + 1,
            cluster.count,
            cluster.keywords.join(", ")
        );
        println!(
            "    Summary: {}",
            &cluster.representative_summary[..cluster.representative_summary.len().min(120)]
        );
    }

    if let Some(memoir_name) = memoir {
        // Resolve memoir
        let memoirs = store.list_memoirs()?;
        let memoir_obj = memoirs
            .iter()
            .find(|m| m.name == memoir_name)
            .ok_or_else(|| anyhow::anyhow!("Memoir '{memoir_name}' not found. Create it first with `icm memoir create -n {memoir_name}`"))?;

        println!("\nCreating concepts in memoir '{memoir_name}'...");
        for cluster in &patterns {
            let concept_id = store.extract_pattern_as_concept(cluster, &memoir_obj.id)?;
            println!("  Created concept: {concept_id}");
        }
        println!("Done. {} concept(s) created.", patterns.len());
    } else {
        println!("\nTo create concepts from these patterns, add --memoir <name>.");
    }

    Ok(())
}

fn cmd_init(mode: InitMode, force: bool) -> Result<()> {
    let icm_bin = std::env::current_exe().context("cannot determine icm binary path")?;
    let icm_bin_str = icm_bin.to_string_lossy().to_string();
    let home = std::env::var("HOME").context("HOME not set")?;

    let do_mcp = matches!(mode, InitMode::Mcp | InitMode::All);
    let do_cli = matches!(mode, InitMode::Cli | InitMode::All);
    let do_skill = matches!(mode, InitMode::Skill | InitMode::All);
    let do_hook = matches!(mode, InitMode::Hook | InitMode::All);

    // --- MCP mode: configure MCP servers for all detected tools ---
    if do_mcp {
        let icm_server_entry = serde_json::json!({
            "command": icm_bin_str,
            "args": ["serve"],
            "env": {}
        });

        let vscode_data = if cfg!(target_os = "macos") {
            PathBuf::from(&home).join("Library/Application Support/Code/User")
        } else {
            PathBuf::from(&home).join(".config/Code/User")
        };

        // Standard JSON tools: (name, path, json_key)
        let tools: Vec<(&str, PathBuf, &str)> = vec![
            // --- Editors & IDEs ---
            (
                "Claude Code",
                PathBuf::from(&home).join(".claude.json"),
                "mcpServers",
            ),
            (
                "Claude Desktop",
                PathBuf::from(&home)
                    .join("Library/Application Support/Claude/claude_desktop_config.json"),
                "mcpServers",
            ),
            (
                "Cursor",
                PathBuf::from(&home).join(".cursor/mcp.json"),
                "mcpServers",
            ),
            (
                "Windsurf",
                PathBuf::from(&home).join(".codeium/windsurf/mcp_config.json"),
                "mcpServers",
            ),
            ("VS Code", vscode_data.join("mcp.json"), "servers"),
            (
                "Gemini",
                PathBuf::from(&home).join(".gemini/settings.json"),
                "mcpServers",
            ),
            // --- Terminal tools ---
            (
                "Amp",
                PathBuf::from(&home).join(".config/amp/settings.json"),
                "amp.mcpServers",
            ),
            (
                "Amazon Q",
                PathBuf::from(&home).join(".aws/amazonq/mcp.json"),
                "mcpServers",
            ),
            // --- VS Code extensions ---
            (
                "Cline",
                vscode_data
                    .join("globalStorage/saoudrizwan.claude-dev/settings/cline_mcp_settings.json"),
                "mcpServers",
            ),
            (
                "Roo Code",
                vscode_data
                    .join("globalStorage/rooveterinaryinc.roo-cline/settings/mcp_settings.json"),
                "mcpServers",
            ),
            (
                "Kilo Code",
                vscode_data.join("globalStorage/kilocode.kilo-code/settings/mcp_settings.json"),
                "mcpServers",
            ),
        ];

        for (name, config_path, key) in &tools {
            if !force && !detect_tool(name, &home, &vscode_data) {
                println!("[mcp] {name:<16} skipped (not detected)");
                continue;
            }
            let status = inject_mcp_server(config_path, "icm", &icm_server_entry, key)?;
            println!("[mcp] {name:<16} {status}");
        }

        // Zed uses nested command.path format
        let zed_path = if cfg!(target_os = "macos") {
            PathBuf::from(&home).join(".zed/settings.json")
        } else {
            PathBuf::from(&home).join(".config/zed/settings.json")
        };
        if !force && !detect_tool("Zed", &home, &vscode_data) {
            println!("[mcp] {:<16} skipped (not detected)", "Zed");
        } else {
            let zed_status = inject_zed_mcp_server(&zed_path, "icm", &icm_bin_str)?;
            println!("[mcp] {:<16} {zed_status}", "Zed");
        }

        // Codex CLI uses TOML format
        let codex_path = PathBuf::from(&home).join(".codex/config.toml");
        if !force && !detect_tool("Codex CLI", &home, &vscode_data) {
            println!("[mcp] {:<16} skipped (not detected)", "Codex CLI");
        } else {
            let codex_status = inject_codex_mcp_server(&codex_path, "icm", &icm_bin_str)?;
            println!("[mcp] {:<16} {codex_status}", "Codex CLI");
        }

        // OpenCode uses different JSON structure (command is array, key is "mcp")
        let opencode_path = PathBuf::from(&home).join(".config/opencode/opencode.json");
        if !force && !detect_tool("OpenCode", &home, &vscode_data) {
            println!("[mcp] {:<16} skipped (not detected)", "OpenCode");
        } else {
            let opencode_status = inject_opencode_mcp_server(&opencode_path, "icm", &icm_bin_str)?;
            println!("[mcp] {:<16} {opencode_status}", "OpenCode");
        }

        // Copilot CLI uses mcpServers key with explicit "type": "local"
        let copilot_path = PathBuf::from(&home).join(".copilot/mcp-config.json");
        if !force && !detect_tool("Copilot CLI", &home, &vscode_data) {
            println!("[mcp] {:<16} skipped (not detected)", "Copilot CLI");
        } else {
            let copilot_status = inject_copilot_cli_mcp_server(&copilot_path, "icm", &icm_bin_str)?;
            println!("[mcp] {:<16} {copilot_status}", "Copilot CLI");
        }

        // Continue.dev uses YAML config with mcpServers key
        let continue_path = PathBuf::from(&home).join(".continue/config.yaml");
        if !force && !detect_tool("Continue.dev", &home, &vscode_data) {
            println!("[mcp] {:<16} skipped (not detected)", "Continue.dev");
        } else {
            let continue_status = inject_continue_mcp_server(&continue_path, "icm", &icm_bin_str)?;
            println!("[mcp] {:<16} {continue_status}", "Continue.dev");
        }
    }

    // --- CLI mode: inject instructions into each tool's file ---
    if do_cli {
        let cwd = std::env::current_dir().context("failed to get current directory")?;

        let icm_block = "\
<!-- icm:start -->\n\
## Persistent memory (ICM) — MANDATORY\n\
\n\
This project uses [ICM](https://github.com/rtk-ai/icm) for persistent memory across sessions.\n\
You MUST use it actively. Not optional.\n\
\n\
### Recall (before starting work)\n\
```bash\n\
icm recall \"query\"                        # search memories\n\
icm recall \"query\" -t \"topic-name\"        # filter by topic\n\
icm recall-context \"query\" --limit 5      # formatted for prompt injection\n\
```\n\
\n\
### Store — MANDATORY triggers\n\
You MUST call `icm store` when ANY of the following happens:\n\
1. **Error resolved** → `icm store -t errors-resolved -c \"description\" -i high -k \"keyword1,keyword2\"`\n\
2. **Architecture/design decision** → `icm store -t decisions-{project} -c \"description\" -i high`\n\
3. **User preference discovered** → `icm store -t preferences -c \"description\" -i critical`\n\
4. **Significant task completed** → `icm store -t context-{project} -c \"summary of work done\" -i high`\n\
5. **Conversation exceeds ~20 tool calls without a store** → store a progress summary\n\
\n\
Do this BEFORE responding to the user. Not after. Not later. Immediately.\n\
\n\
Do NOT store: trivial details, info already in CLAUDE.md, ephemeral state (build logs, git status).\n\
\n\
### Other commands\n\
```bash\n\
icm update <id> -c \"updated content\"     # edit memory in-place\n\
icm health                                # topic hygiene audit\n\
icm topics                                # list all topics\n\
```\n\
<!-- icm:end -->";

        // Each AI tool uses its own instruction file
        let instruction_files: Vec<(&str, PathBuf)> = vec![
            ("Claude Code", cwd.join("CLAUDE.md")),
            ("Codex", cwd.join("AGENTS.md")),
            ("Gemini", PathBuf::from(&home).join(".gemini/GEMINI.md")),
            ("Copilot", cwd.join(".github/copilot-instructions.md")),
            ("Windsurf", cwd.join(".windsurfrules")),
            ("Aider", cwd.join(".aider.conventions.md")),
        ];

        for (tool_name, path) in &instruction_files {
            let status = inject_icm_block(path, icm_block)?;
            println!("[cli] {tool_name:<16} {status}");
        }
    }

    // --- Skill mode: create slash commands / rules for all tools ---
    if do_skill {
        let icm_recall_prompt = "\
Search ICM memory for: $ARGUMENTS

Run:
```bash
icm recall \"$ARGUMENTS\"
```
";
        let icm_remember_prompt = "\
Store the following in ICM memory: $ARGUMENTS

Run:
```bash
icm store -t \"note\" -c \"$ARGUMENTS\"
```
";

        // Claude Code: ~/.claude/commands/
        let claude_skills_dir = PathBuf::from(&home).join(".claude/commands");
        install_skill(
            &claude_skills_dir,
            "recall.md",
            icm_recall_prompt,
            "Claude Code /recall",
        )?;
        install_skill(
            &claude_skills_dir,
            "remember.md",
            icm_remember_prompt,
            "Claude Code /remember",
        )?;

        // Cursor: ~/.cursor/rules/ (project or global)
        let cursor_rules_dir = PathBuf::from(&home).join(".cursor/rules");
        let cursor_icm_rule = "\
---
description: ICM persistent memory for AI agents
globs:
alwaysApply: true
---

This project uses ICM (Infinite Context Memory) for persistent memory. Usage is MANDATORY.

**Recall** — at the start of each task, search for relevant past context:
```bash
icm recall \"query\"
```

**Store** — you MUST store when any of these happens:
1. Error resolved → `icm store -t errors-resolved -c \"description\" -i high`
2. Architecture decision → `icm store -t decisions-{project} -c \"description\" -i high`
3. User preference discovered → `icm store -t preferences -c \"description\" -i critical`
4. Significant task completed → `icm store -t context-{project} -c \"summary\" -i high`
5. Conversation exceeds ~20 tool calls without a store → store progress summary

Do this BEFORE responding to the user. Not optional.
";
        install_skill(&cursor_rules_dir, "icm.mdc", cursor_icm_rule, "Cursor rule")?;

        // Roo Code: ~/.roo/rules/ (global)
        let roo_rules_dir = PathBuf::from(&home).join(".roo/rules");
        install_skill(&roo_rules_dir, "icm.md", cursor_icm_rule, "Roo Code rule")?;

        // Amp: ~/.config/amp/skills/
        let amp_skills_dir = PathBuf::from(&home).join(".config/amp/skills");
        install_skill(
            &amp_skills_dir,
            "icm-recall.md",
            icm_recall_prompt,
            "Amp /icm-recall",
        )?;
        install_skill(
            &amp_skills_dir,
            "icm-remember.md",
            icm_remember_prompt,
            "Amp /icm-remember",
        )?;
    }

    // --- Hook mode: install Claude Code hooks (full Rust, no shell scripts) ---
    if do_hook {
        let claude_settings_path = PathBuf::from(&home).join(".claude/settings.json");

        // PreToolUse hook: `icm hook pre` (auto-allow icm commands)
        let pre_cmd = format!("{} hook pre", icm_bin_str);
        let pre_status = inject_settings_hook(
            &claude_settings_path,
            "PreToolUse",
            &pre_cmd,
            Some("Bash"),
            &["icm-pretool", "icm hook pre"],
            force,
        )?;
        println!("[hook] Claude Code PreToolUse (auto-allow): {pre_status}");

        // PostToolUse hook: `icm hook post` (auto-extract context)
        let post_cmd = format!("{} hook post", icm_bin_str);
        let post_status = inject_settings_hook(
            &claude_settings_path,
            "PostToolUse",
            &post_cmd,
            None,
            &["icm hook", "icm-post-tool"],
            force,
        )?;
        println!("[hook] Claude Code PostToolUse (auto-extract): {post_status}");

        // PreCompact hook: `icm hook compact` (extract from transcript before compression)
        let compact_cmd = format!("{} hook compact", icm_bin_str);
        let compact_status = inject_settings_hook(
            &claude_settings_path,
            "PreCompact",
            &compact_cmd,
            None,
            &["icm hook", "icm-post-tool"],
            force,
        )?;
        println!("[hook] Claude Code PreCompact (transcript extract): {compact_status}");

        // UserPromptSubmit hook: `icm hook prompt` (recall context on each prompt)
        let prompt_cmd = format!("{} hook prompt", icm_bin_str);
        let prompt_status = inject_settings_hook(
            &claude_settings_path,
            "UserPromptSubmit",
            &prompt_cmd,
            None,
            &["icm hook", "icm-post-tool"],
            force,
        )?;
        println!("[hook] Claude Code UserPromptSubmit (auto-recall): {prompt_status}");

        // SessionStart hook: `icm hook start` (inject wake-up pack of critical facts)
        let start_cmd = format!("{} hook start", icm_bin_str);
        let start_status = inject_settings_hook(
            &claude_settings_path,
            "SessionStart",
            &start_cmd,
            None,
            &["icm hook start", "icm hook", "icm-post-tool"],
            force,
        )?;
        println!("[hook] Claude Code SessionStart (wake-up pack): {start_status}");

        // SessionEnd hook: `icm hook end` (extract from transcript before /exit, /clear, etc.)
        // Catches the path PreCompact misses — compaction does not fire on /clear.
        let end_cmd = format!("{} hook end", icm_bin_str);
        let end_status = inject_settings_hook(
            &claude_settings_path,
            "SessionEnd",
            &end_cmd,
            None,
            &["icm hook end", "icm hook", "icm-post-tool"],
            force,
        )?;
        println!("[hook] Claude Code SessionEnd (transcript extract): {end_status}");

        // OpenCode plugin: install TS plugin using native @opencode-ai/plugin SDK
        let opencode_plugins_dir = PathBuf::from(&home).join(".config/opencode/plugins");
        let opencode_plugin_path = opencode_plugins_dir.join("icm.ts");
        // Remove old .js plugin if it exists
        let old_js_plugin = opencode_plugins_dir.join("icm.js");
        if old_js_plugin.exists() {
            std::fs::remove_file(&old_js_plugin).ok();
        }
        if opencode_plugin_path.exists() {
            println!("[hook] OpenCode plugin: already configured");
        } else {
            std::fs::create_dir_all(&opencode_plugins_dir).ok();
            let plugin_content = include_str!("../../../plugins/opencode-icm.ts");
            std::fs::write(&opencode_plugin_path, plugin_content)
                .with_context(|| format!("cannot write {}", opencode_plugin_path.display()))?;
            println!("[hook] OpenCode plugin: installed");
        }

        // --- Gemini CLI hooks (same settings.json format as Claude Code, different event names) ---
        let gemini_settings_path = PathBuf::from(&home).join(".gemini/settings.json");
        let detect = &["icm hook", "icm-post-tool"];

        // SessionStart → same name in Gemini CLI
        let status = inject_settings_hook(
            &gemini_settings_path,
            "SessionStart",
            &format!("{} hook start", icm_bin_str),
            None,
            &["icm hook start", "icm hook", "icm-post-tool"],
            force,
        )?;
        println!("[hook] Gemini CLI SessionStart (wake-up pack): {status}");

        // BeforeTool → equivalent of PreToolUse (auto-allow icm commands)
        // Gemini CLI uses "run_shell_command" as the Bash tool name
        let status = inject_settings_hook(
            &gemini_settings_path,
            "BeforeTool",
            &format!("{} hook pre", icm_bin_str),
            Some("run_shell_command"),
            &["icm-pretool", "icm hook pre"],
            force,
        )?;
        println!("[hook] Gemini CLI BeforeTool (auto-allow): {status}");

        // AfterTool → equivalent of PostToolUse (auto-extract context)
        let status = inject_settings_hook(
            &gemini_settings_path,
            "AfterTool",
            &format!("{} hook post", icm_bin_str),
            None,
            detect,
            force,
        )?;
        println!("[hook] Gemini CLI AfterTool (auto-extract): {status}");

        // PreCompress → equivalent of PreCompact (extract from transcript)
        let status = inject_settings_hook(
            &gemini_settings_path,
            "PreCompress",
            &format!("{} hook compact", icm_bin_str),
            None,
            detect,
            force,
        )?;
        println!("[hook] Gemini CLI PreCompress (transcript extract): {status}");

        // BeforeAgent → equivalent of UserPromptSubmit (auto-recall on each prompt)
        let status = inject_settings_hook(
            &gemini_settings_path,
            "BeforeAgent",
            &format!("{} hook prompt", icm_bin_str),
            None,
            detect,
            force,
        )?;
        println!("[hook] Gemini CLI BeforeAgent (auto-recall): {status}");

        // --- Codex CLI hooks (separate hooks.json file) ---
        let codex_hooks_path = PathBuf::from(&home).join(".codex/hooks.json");

        let status = inject_codex_hook(
            &codex_hooks_path,
            "SessionStart",
            &format!("{} hook start", icm_bin_str),
            None,
            &["icm hook start", "icm hook"],
        )?;
        println!("[hook] Codex CLI SessionStart (wake-up pack): {status}");

        let status = inject_codex_hook(
            &codex_hooks_path,
            "PreToolUse",
            &format!("{} hook pre", icm_bin_str),
            Some("Bash"),
            &["icm-pretool", "icm hook pre"],
        )?;
        println!("[hook] Codex CLI PreToolUse (auto-allow): {status}");

        let status = inject_codex_hook(
            &codex_hooks_path,
            "PostToolUse",
            &format!("{} hook post", icm_bin_str),
            None,
            detect,
        )?;
        println!("[hook] Codex CLI PostToolUse (auto-extract): {status}");

        let status = inject_codex_hook(
            &codex_hooks_path,
            "UserPromptSubmit",
            &format!("{} hook prompt", icm_bin_str),
            None,
            detect,
        )?;
        println!("[hook] Codex CLI UserPromptSubmit (auto-recall): {status}");

        // --- Copilot CLI hooks (per-repo .github/hooks/icm.json) ---
        let cwd = std::env::current_dir().context("failed to get current directory")?;
        let copilot_status = inject_copilot_hooks(&cwd, &icm_bin_str)?;
        println!("[hook] Copilot CLI (all hooks): {copilot_status}");
    }

    println!();
    println!("  binary: {icm_bin_str}");
    println!("  db:     {}", default_db_path().display());
    println!();
    println!("Restart your AI tool to activate.");

    if !do_hook {
        println!();
        println!("Tip: run `icm init --mode hook` to also install Claude Code hooks");
        println!("     for automatic memory extraction and context recall.");
    }

    Ok(())
}

/// Inject ICM instruction block into a markdown file (CLAUDE.md, AGENTS.md, GEMINI.md, etc.)
fn inject_icm_block(path: &Path, block: &str) -> Result<String> {
    if path.exists() {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        if content.contains("<!-- icm:start -->") {
            return Ok(format!("{} already configured", path.display()));
        }
        let new_content = format!("{}\n\n{}\n", content.trim_end(), block);
        std::fs::write(path, new_content)
            .with_context(|| format!("cannot write {}", path.display()))?;
        Ok(format!("{} updated", path.display()))
    } else {
        // Create parent dir if needed
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        std::fs::write(path, format!("{block}\n"))
            .with_context(|| format!("cannot create {}", path.display()))?;
        Ok(format!("{} created", path.display()))
    }
}

fn cmd_doctor() -> Result<()> {
    let home = std::env::var("HOME").context("HOME not set")?;
    let current_bin = std::env::current_exe().ok();

    let targets: Vec<(&str, PathBuf, &[&str])> = vec![
        (
            "Claude Code",
            PathBuf::from(&home).join(".claude/settings.json"),
            &[
                "PreToolUse",
                "PostToolUse",
                "PreCompact",
                "UserPromptSubmit",
                "SessionStart",
            ],
        ),
        (
            "Gemini CLI",
            PathBuf::from(&home).join(".gemini/settings.json"),
            &[
                "SessionStart",
                "BeforeTool",
                "AfterTool",
                "PreCompress",
                "BeforeAgent",
            ],
        ),
    ];

    let mut broken = 0usize;
    let mut checked = 0usize;

    for (label, path, events) in &targets {
        if !path.exists() {
            println!("[{label}] {} (no settings file, skipped)", path.display());
            continue;
        }
        let config: Value = match parse_json_config(path) {
            Ok(v) => v,
            Err(e) => {
                println!("[{label}] {}: parse error ({e})", path.display());
                broken += 1;
                continue;
            }
        };
        let Some(hooks) = config.get("hooks").and_then(|h| h.as_object()) else {
            println!("[{label}] no hooks block configured");
            continue;
        };
        for event in *events {
            let Some(arr) = hooks.get(*event).and_then(|v| v.as_array()) else {
                continue;
            };
            for entry in arr {
                let Some(hooks_arr) = entry.get("hooks").and_then(|h| h.as_array()) else {
                    continue;
                };
                for h in hooks_arr {
                    let Some(cmd) = h.get("command").and_then(|c| c.as_str()) else {
                        continue;
                    };
                    if !cmd.contains("icm hook") && !cmd.contains("icm-post-tool") {
                        continue;
                    }
                    checked += 1;
                    let bin_path = cmd.split_whitespace().next().unwrap_or("");
                    let exists = std::path::Path::new(bin_path).exists();
                    if exists {
                        println!("[{label}] {event:<17} ✓  {bin_path}");
                    } else {
                        println!("[{label}] {event:<17} ✗  {bin_path}  (missing)");
                        broken += 1;
                    }
                }
            }
        }
    }

    println!();
    if checked == 0 {
        println!("No ICM hooks found. Run `icm init --mode hook` to install them.");
    } else if broken == 0 {
        println!("All {checked} ICM hook entries are healthy.");
    } else {
        println!("{broken} of {checked} ICM hook entries point at a missing binary.");
        if let Some(bin) = current_bin {
            println!("To fix: icm init --mode hook --force");
            println!("       (will rewrite stale entries to {})", bin.display());
        } else {
            println!("To fix: icm init --mode hook --force");
        }
    }

    Ok(())
}

/// Inject ICM hook into a settings.json file (Claude Code or Gemini CLI) for a given event name.
/// Both tools use the same JSON format: `{ "hooks": { "EventName": [ { "matcher": ..., "hooks": [...] } ] } }`.
/// `matcher` is optional — if set (e.g. "Bash"), adds a matcher field to the hook entry.
/// `detect_patterns` lists substrings to detect if the hook is already present.
/// `force` rewrites stale entries (matching `detect_patterns` but with a different command) in-place.
fn inject_settings_hook(
    settings_path: &PathBuf,
    event_name: &str,
    hook_command: &str,
    matcher: Option<&str>,
    detect_patterns: &[&str],
    force: bool,
) -> Result<String> {
    let mut config: Value = if settings_path.exists() {
        parse_json_config(settings_path)?
    } else {
        serde_json::json!({})
    };

    let hooks = config
        .as_object_mut()
        .context("settings is not a JSON object")?
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}));

    let event_hooks = hooks
        .as_object_mut()
        .context("hooks is not a JSON object")?
        .entry(event_name)
        .or_insert_with(|| serde_json::json!([]));

    let event_arr = event_hooks
        .as_array_mut()
        .with_context(|| format!("{event_name} is not an array"))?;

    // Walk existing entries: classify each matching command as either
    // already-correct or stale (different binary path). With --force we
    // rewrite stale ones in-place; without --force we leave them.
    let mut updated = 0usize;
    let mut already_correct = false;
    let mut stale_present = false;

    for entry in event_arr.iter_mut() {
        let Some(hooks_arr) = entry.get_mut("hooks").and_then(|h| h.as_array_mut()) else {
            continue;
        };
        for h in hooks_arr.iter_mut() {
            let Some(current) = h.get("command").and_then(|c| c.as_str()) else {
                continue;
            };
            if !detect_patterns.iter().any(|p| current.contains(p)) {
                continue;
            }
            if current == hook_command {
                already_correct = true;
            } else if force {
                h["command"] = serde_json::json!(hook_command);
                updated += 1;
            } else {
                stale_present = true;
            }
        }
    }

    if updated > 0 {
        let output = serde_json::to_string_pretty(&config)?;
        std::fs::write(settings_path, output)
            .with_context(|| format!("cannot write {}", settings_path.display()))?;
        let plural = if updated == 1 { "entry" } else { "entries" };
        return Ok(format!("updated ({updated} stale {plural})"));
    }

    if already_correct {
        return Ok("already configured".into());
    }

    if stale_present {
        return Ok("already configured (stale path; use --force to update)".into());
    }

    // No matching entry — add a fresh one.
    let mut entry = serde_json::json!({
        "hooks": [{
            "type": "command",
            "command": hook_command
        }]
    });
    if let Some(m) = matcher {
        entry
            .as_object_mut()
            .unwrap()
            .insert("matcher".into(), serde_json::json!(m));
    }
    event_arr.push(entry);

    let output = serde_json::to_string_pretty(&config)?;
    std::fs::write(settings_path, output)
        .with_context(|| format!("cannot write {}", settings_path.display()))?;

    Ok("configured".into())
}

/// Inject ICM hook into Codex CLI hooks.json for a given event name.
/// Codex uses a separate `~/.codex/hooks.json` file (not inside config.toml).
/// Format is the same as Claude Code: `{ "hooks": { "EventName": [ { "matcher": ..., "hooks": [...] } ] } }`.
fn inject_codex_hook(
    hooks_path: &PathBuf,
    event_name: &str,
    hook_command: &str,
    matcher: Option<&str>,
    detect_patterns: &[&str],
) -> Result<String> {
    let mut config: Value = if hooks_path.exists() {
        parse_json_config(hooks_path)?
    } else {
        if let Some(parent) = hooks_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        serde_json::json!({})
    };

    let hooks = config
        .as_object_mut()
        .context("hooks.json is not a JSON object")?
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}));

    let event_hooks = hooks
        .as_object_mut()
        .context("hooks is not a JSON object")?
        .entry(event_name)
        .or_insert_with(|| serde_json::json!([]));

    let event_arr = event_hooks
        .as_array_mut()
        .with_context(|| format!("{event_name} is not an array"))?;

    // Check if ICM hook already exists
    let already = event_arr.iter().any(|entry| {
        entry
            .get("hooks")
            .and_then(|h| h.as_array())
            .map(|hooks| {
                hooks.iter().any(|h| {
                    h.get("command")
                        .and_then(|c| c.as_str())
                        .map(|c| detect_patterns.iter().any(|p| c.contains(p)))
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false)
    });

    if already {
        return Ok("already configured".into());
    }

    let mut entry = serde_json::json!({
        "hooks": [{
            "type": "command",
            "command": hook_command
        }]
    });
    if let Some(m) = matcher {
        entry
            .as_object_mut()
            .unwrap()
            .insert("matcher".into(), serde_json::json!(m));
    }
    event_arr.push(entry);

    let output = serde_json::to_string_pretty(&config)?;
    std::fs::write(hooks_path, output)
        .with_context(|| format!("cannot write {}", hooks_path.display()))?;

    Ok("configured".into())
}

/// Install a skill/rule file if it doesn't exist yet.
fn install_skill(dir: &Path, filename: &str, content: &str, label: &str) -> Result<()> {
    std::fs::create_dir_all(dir).ok();
    let path = dir.join(filename);
    if path.exists() {
        println!("[skill] {label} already configured.");
    } else {
        std::fs::write(&path, content).with_context(|| format!("cannot write {label}"))?;
        println!("[skill] {label} created.");
    }
    Ok(())
}

/// Strip JSONC comments (// and /* */) and handle empty/whitespace-only content.
/// Returns valid JSON or an empty object.
fn strip_jsonc_comments(content: &str) -> String {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return "{}".to_string();
    }
    let mut result = String::with_capacity(content.len());
    let mut chars = content.chars().peekable();
    let mut in_string = false;
    let mut escape_next = false;

    while let Some(c) = chars.next() {
        if escape_next {
            result.push(c);
            escape_next = false;
            continue;
        }
        if in_string {
            if c == '\\' {
                escape_next = true;
            } else if c == '"' {
                in_string = false;
            }
            result.push(c);
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                result.push(c);
            }
            '/' => match chars.peek() {
                Some('/') => {
                    chars.next();
                    // Skip until end of line
                    for ch in chars.by_ref() {
                        if ch == '\n' {
                            result.push('\n');
                            break;
                        }
                    }
                }
                Some('*') => {
                    chars.next();
                    // Skip until */
                    let mut prev = ' ';
                    for ch in chars.by_ref() {
                        if prev == '*' && ch == '/' {
                            break;
                        }
                        prev = ch;
                    }
                }
                _ => result.push(c),
            },
            _ => result.push(c),
        }
    }
    let r = result.trim();
    if r.is_empty() {
        "{}".to_string()
    } else {
        result
    }
}

/// Parse a JSON/JSONC config file, handling comments and empty files gracefully.
/// Parse a JSON config file with lenient parsing: accepts trailing commas
/// and JSONC comments (// and /* */). This is the only place we use lenient
/// parsing — all other JSON handling uses strict serde_json.
fn parse_json_config(config_path: &std::path::Path) -> Result<Value> {
    let content = std::fs::read_to_string(config_path)
        .with_context(|| format!("cannot read {}", config_path.display()))?;
    let clean = strip_jsonc_comments(&content);
    // Use serde_json_lenient to accept trailing commas in user-edited configs,
    // then round-trip to serde_json::Value for compatibility with the rest of the codebase.
    let lenient: serde_json_lenient::Value = serde_json_lenient::from_str(&clean)
        .with_context(|| format!("invalid JSON in {}", config_path.display()))?;
    let strict: Value = serde_json::from_str(&lenient.to_string())
        .with_context(|| format!("JSON conversion error in {}", config_path.display()))?;
    Ok(strict)
}

/// Returns true if `name` resolves to an executable file somewhere in $PATH.
fn binary_in_path(name: &str) -> bool {
    std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .any(|dir| std::path::Path::new(dir).join(name).is_file())
}

/// Heuristic: is this AI tool installed on the current machine?
///
/// Binary presence is checked first (most reliable). Directory checks are only
/// used for tools without a CLI binary (e.g. Claude Desktop, VS Code extensions).
/// Note: directory checks can yield false positives if a previous `icm init --force`
/// already created the config path — use `--force` to bypass detection entirely.
fn detect_tool(name: &str, home: &str, vscode_data: &Path) -> bool {
    let h = std::path::Path::new(home);
    let vscode_present =
        || binary_in_path("code") || binary_in_path("code-insiders") || vscode_data.exists();
    match name {
        "Claude Code" => binary_in_path("claude"),
        "Claude Desktop" => {
            // macOS-only app — always false on Linux/Windows
            cfg!(target_os = "macos")
                && (std::path::Path::new("/Applications/Claude.app").exists()
                    || h.join("Library/Application Support/Claude").exists())
        }
        "Cursor" => binary_in_path("cursor"),
        "Windsurf" => binary_in_path("windsurf"),
        "VS Code" => vscode_present(),
        "Gemini" => binary_in_path("gemini"),
        "Amp" => binary_in_path("amp"),
        "Amazon Q" => binary_in_path("q"),
        // VS Code extensions: require VS Code AND the extension's globalStorage dir
        // (globalStorage dirs are only created by VS Code when an extension is installed)
        "Cline" => {
            vscode_present()
                && vscode_data
                    .join("globalStorage/saoudrizwan.claude-dev")
                    .exists()
        }
        "Roo Code" => {
            vscode_present()
                && vscode_data
                    .join("globalStorage/rooveterinaryinc.roo-cline")
                    .exists()
        }
        "Kilo Code" => {
            vscode_present()
                && vscode_data
                    .join("globalStorage/kilocode.kilo-code")
                    .exists()
        }
        "Zed" => binary_in_path("zed"),
        "Codex CLI" => binary_in_path("codex"),
        "OpenCode" => binary_in_path("opencode"),
        // Copilot CLI is a `gh` extension — require the gh binary
        "Copilot CLI" => binary_in_path("gh"),
        // Continue.dev is a VS Code/JetBrains extension — check its globalStorage dir
        // (icm writes to ~/.continue/config.yaml, not globalStorage, so this is reliable)
        "Continue.dev" => {
            vscode_present() && vscode_data.join("globalStorage/continue.continue").exists()
        }
        _ => true,
    }
}

/// Inject ICM MCP server into a JSON config file. Returns a status string.
/// `servers_key` is the JSON key for the servers object (e.g. "mcpServers", "servers", "context_servers").
fn inject_mcp_server(
    config_path: &PathBuf,
    name: &str,
    entry: &Value,
    servers_key: &str,
) -> Result<String> {
    // Read existing config or create empty object
    let mut config: Value = if config_path.exists() {
        parse_json_config(config_path)?
    } else {
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        serde_json::json!({})
    };

    // Support nested keys like "amp.mcpServers"
    let mcp_servers = if servers_key.contains('.') {
        let parts: Vec<&str> = servers_key.split('.').collect();
        let obj = config
            .as_object_mut()
            .context("config is not a JSON object")?;
        let parent = obj.entry(parts[0]).or_insert_with(|| serde_json::json!({}));
        parent
            .as_object_mut()
            .context("nested key is not an object")?
            .entry(parts[1])
            .or_insert_with(|| serde_json::json!({}))
    } else {
        config
            .as_object_mut()
            .context("config is not a JSON object")?
            .entry(servers_key)
            .or_insert_with(|| serde_json::json!({}))
    };

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

/// Inject ICM MCP server into Zed settings.json (uses `context_servers` with nested `command` object).
fn inject_zed_mcp_server(config_path: &Path, name: &str, bin_path: &str) -> Result<String> {
    let mut config: Value = if config_path.exists() {
        parse_json_config(config_path)?
    } else {
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        serde_json::json!({})
    };

    let servers = config
        .as_object_mut()
        .context("config is not a JSON object")?
        .entry("context_servers")
        .or_insert_with(|| serde_json::json!({}));

    if servers.get(name).is_some() {
        return Ok("already configured".into());
    }

    let zed_entry = serde_json::json!({
        "command": bin_path,
        "args": ["serve"],
        "env": {},
    });

    servers
        .as_object_mut()
        .unwrap()
        .insert(name.to_string(), zed_entry);

    let output = serde_json::to_string_pretty(&config)?;
    std::fs::write(config_path, output)
        .with_context(|| format!("cannot write {}", config_path.display()))?;

    Ok("configured".into())
}

/// Inject ICM MCP server into Copilot CLI config (~/.copilot/mcp-config.json).
/// Copilot CLI uses `mcpServers` key with explicit `"type": "local"`.
fn inject_copilot_cli_mcp_server(
    config_path: &PathBuf,
    name: &str,
    icm_bin: &str,
) -> Result<String> {
    let mut config: Value = if config_path.exists() {
        parse_json_config(config_path)?
    } else {
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        serde_json::json!({})
    };

    let servers = config
        .as_object_mut()
        .context("config is not a JSON object")?
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}));

    if let Some(existing) = servers.get(name) {
        if existing.get("command").and_then(|v| v.as_str()) == Some(icm_bin) {
            return Ok("already configured".into());
        }
    }

    servers.as_object_mut().unwrap().insert(
        name.to_string(),
        serde_json::json!({
            "type": "local",
            "command": icm_bin,
            "args": ["serve"],
            "tools": ["*"]
        }),
    );

    let output = serde_json::to_string_pretty(&config)?;
    std::fs::write(config_path, output)
        .with_context(|| format!("cannot write {}", config_path.display()))?;

    Ok("configured".into())
}

/// Inject ICM MCP server into Continue.dev config (~/.continue/config.yaml).
/// Continue.dev uses YAML with a top-level `mcpServers` list.
fn inject_continue_mcp_server(config_path: &Path, name: &str, icm_bin: &str) -> Result<String> {
    if config_path.exists() {
        let content = std::fs::read_to_string(config_path)
            .with_context(|| format!("cannot read {}", config_path.display()))?;
        if content.contains(icm_bin) || content.contains(&format!("name: {name}")) {
            return Ok("already configured".into());
        }
        // Append MCP server entry to existing config
        let entry = format!(
            "\nmcpServers:\n  - name: {name}\n    command: {icm_bin}\n    args:\n      - serve\n"
        );
        let new_content = if content.contains("mcpServers:") {
            // Insert under existing mcpServers key
            content.replace(
                "mcpServers:",
                &format!(
                    "mcpServers:\n  - name: {name}\n    command: {icm_bin}\n    args:\n      - serve"
                ),
            )
        } else {
            format!("{}\n{}", content.trim_end(), entry)
        };
        std::fs::write(config_path, new_content)
            .with_context(|| format!("cannot write {}", config_path.display()))?;
    } else {
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let content = format!(
            "mcpServers:\n  - name: {name}\n    command: {icm_bin}\n    args:\n      - serve\n"
        );
        std::fs::write(config_path, content)
            .with_context(|| format!("cannot write {}", config_path.display()))?;
    }

    Ok("configured".into())
}

/// Inject ICM hooks into Copilot CLI hooks file (.github/hooks/icm.json).
/// Copilot CLI uses a per-repo hooks file with a different format:
/// `{ "version": 1, "hooks": { "eventName": [{ "type": "command", "bash": "...", "timeoutSec": N }] } }`
fn inject_copilot_hooks(cwd: &std::path::Path, icm_bin: &str) -> Result<String> {
    let hooks_dir = cwd.join(".github/hooks");
    let hooks_path = hooks_dir.join("icm.json");

    if hooks_path.exists() {
        let content = std::fs::read_to_string(&hooks_path)
            .with_context(|| format!("cannot read {}", hooks_path.display()))?;
        if content.contains("icm hook") {
            return Ok("already configured".into());
        }
    }

    std::fs::create_dir_all(&hooks_dir).ok();

    let hooks_config = serde_json::json!({
        "version": 1,
        "hooks": {
            "sessionStart": [{
                "type": "command",
                "bash": format!("{icm_bin} hook start"),
                "timeoutSec": 10
            }],
            "preToolUse": [{
                "type": "command",
                "bash": format!("{icm_bin} hook pre"),
                "timeoutSec": 5
            }],
            "postToolUse": [{
                "type": "command",
                "bash": format!("{icm_bin} hook post"),
                "timeoutSec": 10
            }],
            "userPromptSubmitted": [{
                "type": "command",
                "bash": format!("{icm_bin} hook prompt"),
                "timeoutSec": 10
            }]
        }
    });

    let output = serde_json::to_string_pretty(&hooks_config)?;
    std::fs::write(&hooks_path, output)
        .with_context(|| format!("cannot write {}", hooks_path.display()))?;

    Ok("configured".into())
}

/// Inject ICM MCP server into Codex CLI TOML config. Returns a status string.
fn inject_codex_mcp_server(config_path: &Path, name: &str, icm_bin: &str) -> Result<String> {
    let mut config: toml::Value = if config_path.exists() {
        let content = std::fs::read_to_string(config_path)
            .with_context(|| format!("cannot read {}", config_path.display()))?;
        content
            .parse::<toml::Value>()
            .with_context(|| format!("invalid TOML in {}", config_path.display()))?
    } else {
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        toml::Value::Table(toml::map::Map::new())
    };

    let root = config
        .as_table_mut()
        .context("config is not a TOML table")?;

    let mcp_servers = root
        .entry("mcp_servers")
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));

    // Check if already configured with same binary
    if let Some(existing) = mcp_servers.get(name) {
        if existing.get("command").and_then(|v| v.as_str()) == Some(icm_bin) {
            return Ok("already configured".into());
        }
    }

    let mut server = toml::map::Map::new();
    server.insert("command".into(), toml::Value::String(icm_bin.to_string()));
    server.insert(
        "args".into(),
        toml::Value::Array(vec![toml::Value::String("serve".into())]),
    );

    mcp_servers
        .as_table_mut()
        .unwrap()
        .insert(name.to_string(), toml::Value::Table(server));

    let output = toml::to_string_pretty(&config)?;
    std::fs::write(config_path, output)
        .with_context(|| format!("cannot write {}", config_path.display()))?;

    Ok("configured".into())
}

/// Inject ICM MCP server into OpenCode config (uses "mcp" key, command is array).
fn inject_opencode_mcp_server(config_path: &Path, name: &str, icm_bin: &str) -> Result<String> {
    let mut config: Value = if config_path.exists() {
        parse_json_config(config_path)?
    } else {
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        serde_json::json!({})
    };

    let mcp = config
        .as_object_mut()
        .context("config is not a JSON object")?
        .entry("mcp")
        .or_insert_with(|| serde_json::json!({}));

    if let Some(existing) = mcp.get(name) {
        if let Some(cmd) = existing.get("command").and_then(|v| v.as_array()) {
            if cmd.first().and_then(|v| v.as_str()) == Some(icm_bin) {
                return Ok("already configured".into());
            }
        }
    }

    mcp.as_object_mut().unwrap().insert(
        name.to_string(),
        serde_json::json!({
            "type": "local",
            "command": [icm_bin, "serve"],
            "enabled": true
        }),
    );

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
    println!("[embeddings]");
    println!("  model = {}", cfg.embeddings.model);
    println!();
    println!("[extraction]");
    println!("  enabled = {}", cfg.extraction.enabled);
    println!("  min_score = {}", cfg.extraction.min_score);
    println!("  max_facts = {}", cfg.extraction.max_facts);
    println!("  extract_every = {}", cfg.extraction.extract_every);
    println!("  store_raw = {}", cfg.extraction.store_raw);
    println!();
    println!("[recall]");
    println!("  enabled = {}", cfg.recall.enabled);
    println!("  limit = {}", cfg.recall.limit);
    println!();
    println!("[mcp]");
    println!("  transport = {}", cfg.mcp.transport);
    println!("  compact = {}", cfg.mcp.compact);
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
    store_raw: bool,
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
        let stored = extract::extract_and_store_with_opts(store, &input, project, store_raw)?;
        println!("Extracted and stored {stored} facts.");
    }
    Ok(())
}

fn cmd_recall_context(store: &SqliteStore, query: &str, limit: usize) -> Result<()> {
    // Explicit `recall-context` CLI invocation: no implicit project filter,
    // the user passed the query they want.
    let ctx = extract::recall_context(store, query, None, limit)?;
    if ctx.is_empty() {
        eprintln!("No relevant context found.");
    } else {
        print!("{ctx}");
    }
    Ok(())
}

/// Detect the current project name from PWD and git remote.
/// Returns the best project identifier for topic matching.
fn detect_project() -> String {
    // Try git remote first (most unique identifier)
    if let Ok(output) = std::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .output()
    {
        let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !url.is_empty() {
            // Extract repo name: "git@github.com:user/repo.git" -> "repo"
            // or "https://github.com/user/repo.git" -> "repo"
            let name = url
                .rsplit('/')
                .next()
                .unwrap_or(&url)
                .trim_end_matches(".git")
                .to_string();
            if !name.is_empty() {
                return name;
            }
        }
    }

    // Fallback: basename of current directory
    if let Ok(cwd) = std::env::current_dir() {
        if let Some(name) = cwd.file_name() {
            return name.to_string_lossy().to_string();
        }
    }

    "unknown".to_string()
}

fn cmd_recall_project(store: &SqliteStore, limit: usize) -> Result<()> {
    let project = detect_project();
    eprintln!("Project: {project}");

    // Search across project-related topics: context-<project>, decisions-<project>, errors-resolved.
    // Pass the project name as both the FTS query (so topic-name hits rank)
    // and as the hard project filter (so cross-project hits are stripped).
    let query = &project;
    let ctx = extract::recall_context(store, query, Some(project.as_str()), limit)?;
    if ctx.is_empty() {
        eprintln!("No context found for project '{project}'.");
    } else {
        print!("{ctx}");
    }
    Ok(())
}

/// Build and print a wake-up pack for LLM system-prompt injection.
///
/// Selects critical/high memories (plus preferences) optionally scoped to a
/// project, ranks by importance × recency × weight, and truncates to fit the
/// token budget.
fn cmd_wake_up(
    store: &SqliteStore,
    project: Option<String>,
    max_tokens: usize,
    format: CliWakeUpFormat,
    no_preferences: bool,
) -> Result<()> {
    // Resolve project: explicit "-" disables, None auto-detects, Some(name) uses it.
    let detected;
    let project_ref: Option<&str> = match project.as_deref() {
        Some("-") => None,
        Some(p) => Some(p),
        None => {
            detected = detect_project();
            if detected.is_empty() || detected == "unknown" {
                None
            } else {
                // Make auto-detection visible so users understand why
                // specific topics show (or don't).
                eprintln!("Project: {detected} (auto-detected; use --project - to disable)");
                Some(detected.as_str())
            }
        }
    };

    let opts = WakeUpOptions {
        project: project_ref,
        max_tokens,
        format: format.into(),
        include_preferences: !no_preferences,
    };

    let pack = build_wake_up(store, &opts)?;
    print!("{pack}");
    Ok(())
}

fn cmd_save_project(
    store: &SqliteStore,
    embedder: Option<&dyn icm_core::Embedder>,
    content: &str,
    importance: Importance,
    keywords: Option<String>,
) -> Result<()> {
    let project = detect_project();
    let topic = format!("context-{project}");
    eprintln!("Project: {project}");

    // Reuse cmd_store logic
    cmd_store(
        store,
        embedder,
        topic,
        content.to_string(),
        importance,
        keywords,
        None,
    )
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
        store.list_all()?
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
        let texts: Vec<String> = chunk.iter().map(|m| m.embed_text()).collect();
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

fn print_memory_detail(mem: &Memory, score: Option<f32>) {
    match score {
        Some(s) => println!("--- {} [score: {:.3}] ---", mem.id, s),
        None => println!("--- {} ---", mem.id),
    }
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
    if score.is_none() && mem.embedding.is_some() {
        println!("  embedding:  yes");
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
            let (_, _) = SqliteStore::new(&icm_db)?;
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
            let (store, _) = SqliteStore::new(&icm_db)?;
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
            let (store, _) = SqliteStore::new(&icm_db)?;
            let ctx = extract::recall_context(&store, q.prompt, None, 15)?;
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
                        let (store, _) = SqliteStore::new(&icm_db)?;
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
            format!("{}...", truncate_at_char_boundary(q.prompt, 35))
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
            let (_, _) = SqliteStore::new(&icm_db)?;
        }

        let mut results_with: Vec<SessionResult> = Vec::new();
        for (i, prompt) in prompts.iter().enumerate() {
            let effective_prompt = if i > 0 {
                let (store, _) = SqliteStore::new(&icm_db)?;
                let ctx = extract::recall_context(&store, prompt, None, 15)?;
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
                        let (store, _) = SqliteStore::new(&icm_db)?;
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

    let counts = store.batch_memoir_concept_counts().unwrap_or_default();
    println!("{:<25} {:<8} Description", "Name", "Concepts");
    println!("{}", "-".repeat(60));
    for m in &memoirs {
        let concept_count = counts.get(&m.id).copied().unwrap_or(0);
        println!(
            "{:<25} {:<8} {}",
            m.name,
            concept_count,
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
            let labels_str = c.format_labels();
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
            let labels_str = c.format_labels();
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

// confidence_color and confidence_bar are now methods on Concept in icm-core

fn cmd_memoir_export(store: &SqliteStore, memoir_name: &str, format: &str) -> Result<()> {
    let memoir = resolve_memoir(store, memoir_name)?;
    let concepts = store.list_concepts(&memoir.id)?;

    // Batch load all links for this memoir (single query)
    let links = store.get_links_for_memoir(&memoir.id)?;

    // Name lookup for links
    let id_to_name: std::collections::HashMap<&str, &str> = concepts
        .iter()
        .map(|c| (c.id.as_str(), c.name.as_str()))
        .collect();

    match format {
        "json" => {
            let json_concepts: Vec<serde_json::Value> = concepts
                .iter()
                .map(|c| {
                    serde_json::json!({
                        "id": c.id,
                        "name": c.name,
                        "definition": c.definition,
                        "labels": c.labels.iter().map(|l| l.to_string()).collect::<Vec<_>>(),
                        "confidence": c.confidence,
                        "revision": c.revision,
                    })
                })
                .collect();

            let json_links: Vec<serde_json::Value> = links
                .iter()
                .filter_map(|l| {
                    let src = id_to_name.get(l.source_id.as_str())?;
                    let tgt = id_to_name.get(l.target_id.as_str())?;
                    Some(serde_json::json!({
                        "id": l.id,
                        "source": src,
                        "target": tgt,
                        "relation": l.relation.to_string(),
                        "weight": l.weight,
                    }))
                })
                .collect();

            let output = serde_json::json!({
                "memoir": {
                    "name": memoir.name,
                    "description": memoir.description,
                    "created_at": memoir.created_at.to_rfc3339(),
                    "updated_at": memoir.updated_at.to_rfc3339(),
                },
                "concepts": json_concepts,
                "links": json_links,
            });

            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        "dot" => {
            println!("digraph \"{}\" {{", memoir.name);
            println!("  rankdir=LR;");
            println!("  node [shape=box, style=\"rounded,filled\", fillcolor=white];");
            println!();
            for c in &concepts {
                let escaped_def = c.definition.replace('"', "\\\"");
                let color = c.confidence_color();
                println!(
                    "  \"{}\" [tooltip=\"{}\" fillcolor=\"{}\" label=\"{}\\n({:.0}%)\"];",
                    c.name,
                    escaped_def,
                    color,
                    c.name,
                    c.confidence * 100.0
                );
            }
            println!();
            for l in &links {
                if let (Some(src), Some(tgt)) = (
                    id_to_name.get(l.source_id.as_str()),
                    id_to_name.get(l.target_id.as_str()),
                ) {
                    let pw = 0.5 + l.weight * 2.0;
                    println!(
                        "  \"{}\" -> \"{}\" [label=\"{}\" penwidth={:.1}];",
                        src, tgt, l.relation, pw
                    );
                }
            }
            println!("}}");
        }
        "ascii" => {
            println!("╔══ {} ══╗", memoir.name);
            if !memoir.description.is_empty() {
                println!("║ {}", memoir.description);
            }
            println!("║ {} concepts, {} links", concepts.len(), links.len());
            println!("╚{}╝", "═".repeat(memoir.name.len() + 6));
            println!();

            // Build incoming links map for display
            let mut incoming: std::collections::HashMap<&str, Vec<(&str, &str)>> =
                std::collections::HashMap::new();
            let mut outgoing: std::collections::HashMap<&str, Vec<(&str, &str)>> =
                std::collections::HashMap::new();
            for l in &links {
                if let (Some(&src), Some(&tgt)) = (
                    id_to_name.get(l.source_id.as_str()),
                    id_to_name.get(l.target_id.as_str()),
                ) {
                    let rel = l.relation.to_string();
                    // Leak is fine here — small, short-lived CLI output
                    let rel: &str = Box::leak(rel.into_boxed_str());
                    outgoing.entry(src).or_default().push((rel, tgt));
                    incoming.entry(tgt).or_default().push((rel, src));
                }
            }

            for c in &concepts {
                let labels_str = if c.labels.is_empty() {
                    String::new()
                } else {
                    format!(" [{}]", c.format_labels())
                };
                println!("┌─ {}{} {}", c.name, labels_str, c.confidence_bar());
                println!("│  {}", c.definition);

                if let Some(outs) = outgoing.get(c.name.as_str()) {
                    for (rel, tgt) in outs {
                        println!("│  ──{}──> {}", rel, tgt);
                    }
                }
                if let Some(ins) = incoming.get(c.name.as_str()) {
                    for (rel, src) in ins {
                        println!("│  <──{}── {}", rel, src);
                    }
                }
                println!("└─");
            }
        }
        "ai" => {
            // Compact format for LLM context injection
            println!("# Memoir: {} — {}", memoir.name, memoir.description);
            println!();
            println!("## Concepts ({})", concepts.len());
            for c in &concepts {
                let labels_str = if c.labels.is_empty() {
                    String::new()
                } else {
                    format!(" [{}]", c.format_labels())
                };
                println!(
                    "- **{}**{} (confidence: {:.0}%): {}",
                    c.name,
                    labels_str,
                    c.confidence * 100.0,
                    c.definition
                );
            }
            if !links.is_empty() {
                println!();
                println!("## Relations ({})", links.len());
                for l in &links {
                    if let (Some(src), Some(tgt)) = (
                        id_to_name.get(l.source_id.as_str()),
                        id_to_name.get(l.target_id.as_str()),
                    ) {
                        println!("- {} ──{}──> {} (w:{:.1})", src, l.relation, tgt, l.weight);
                    }
                }
            }
        }
        _ => bail!("unsupported format: {format} (use 'json', 'dot', 'ascii', or 'ai')"),
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
        let labels_str = c.format_labels();
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

// ---------------------------------------------------------------------------
// Cloud commands
// ---------------------------------------------------------------------------

fn cmd_cloud(command: CloudCommands, store: &SqliteStore) -> Result<()> {
    use icm_core::Scope;

    match command {
        CloudCommands::Login { endpoint, password } => {
            if password {
                // Email/password login (for generic emails, self-hosted, no OAuth)
                eprint!("Email: ");
                let mut email = String::new();
                std::io::stdin().read_line(&mut email)?;
                let email = email.trim().to_string();

                eprint!("Password: ");
                let pwd = rpassword::read_password().context("failed to read password")?;

                cloud::login_password(&endpoint, &email, &pwd)?;
            } else {
                cloud::login_browser(&endpoint)?;
            }
            Ok(())
        }
        CloudCommands::Logout => cloud::logout(),
        CloudCommands::Status => cloud::status(),
        CloudCommands::Push { scope, topic } => {
            let scope: Scope = scope.parse().map_err(|e: String| anyhow::anyhow!(e))?;

            let creds = cloud::require_credentials_for_scope(scope)
                .context("Cloud login required for push. Run: icm cloud login")?;

            let memories: Vec<Memory> = if let Some(ref t) = topic {
                use icm_core::MemoryStore;
                store.get_by_topic(t)?
            } else {
                use icm_core::MemoryStore;
                store.list_all()?
            };

            let mut synced = 0;
            for mut mem in memories {
                mem.scope = scope;
                if let Err(e) = cloud::sync_memory(&creds, &mem) {
                    eprintln!("Failed to sync {}: {}", mem.id, e);
                } else {
                    synced += 1;
                }
            }

            eprintln!("Pushed {} memories to cloud (scope: {})", synced, scope);
            Ok(())
        }
        CloudCommands::Pull { scope, since } => {
            let scope: Scope = scope.parse().map_err(|e: String| anyhow::anyhow!(e))?;

            let creds = cloud::require_credentials_for_scope(scope)
                .context("Cloud login required for pull. Run: icm cloud login")?;

            let memories = cloud::pull_memories(&creds, scope, since.as_deref())?;

            let mut imported = 0;
            for mem in memories {
                use icm_core::MemoryStore;
                // Upsert: if memory exists locally, update it; otherwise store it
                match store.get(&mem.id)? {
                    Some(_) => {
                        store.update(&mem)?;
                    }
                    None => {
                        store.store(mem)?;
                    }
                }
                imported += 1;
            }

            eprintln!("Pulled {} memories from cloud (scope: {})", imported, scope);
            Ok(())
        }
    }
}

#[cfg(test)]
mod truncate_tests {
    use super::truncate_at_char_boundary;

    #[test]
    fn ascii_short_is_unchanged() {
        assert_eq!(truncate_at_char_boundary("hello", 200), "hello");
    }

    #[test]
    fn ascii_long_is_cut_at_exact_byte() {
        let s = "a".repeat(300);
        let out = truncate_at_char_boundary(&s, 200);
        assert_eq!(out.len(), 200);
    }

    /// Regression: issue #110. Cyrillic chars are 2 bytes each in UTF-8.
    /// Byte 200 lands inside a 2-byte sequence for text shorter than 100 chars
    /// after some leading ASCII — bare `&s[..200]` panics.
    #[test]
    fn cyrillic_never_panics_and_cuts_at_char_boundary() {
        // 120 chars × 2 bytes = 240 bytes; 200 bytes = mid-char if not fixed.
        let s = "\u{043F}".repeat(120); // Cyrillic 'п'
        assert_eq!(s.len(), 240);
        let out = truncate_at_char_boundary(&s, 200);
        // Must not panic, and must be a valid UTF-8 prefix.
        assert!(out.len() <= 200);
        // 200 / 2 bytes-per-char = 100 chars, and the last char must fit.
        assert!(out.len().is_multiple_of(2), "boundary landed mid-char");
        // Round-trip: chars reconstructed from `out` must all be Cyrillic 'п'.
        assert!(out.chars().all(|c| c == '\u{043F}'));
    }

    /// Emoji are 4 bytes each. With a prompt of mixed ASCII + emoji, the
    /// cut at 200 bytes will often land inside an emoji.
    #[test]
    fn emoji_never_panics_and_cuts_at_char_boundary() {
        let s = "\u{1F600}".repeat(60); // 60 × 4 = 240 bytes
        let out = truncate_at_char_boundary(&s, 201);
        assert!(out.len() <= 201);
        assert_eq!(out.len() % 4, 0, "boundary landed mid-emoji");
        assert!(out.chars().all(|c| c == '\u{1F600}'));
    }

    /// Mixed ASCII prefix + Cyrillic body — the common case for prompts
    /// like "project_name посмотри в апстрим...".
    #[test]
    fn mixed_ascii_cyrillic_never_panics() {
        let s = format!("rtk {}", "\u{0430}".repeat(200)); // "rtk " + 200 × Cyrillic 'а'
        let out = truncate_at_char_boundary(&s, 200);
        assert!(out.len() <= 200);
        // The tail after "rtk " must be whole Cyrillic chars.
        assert!(out.is_char_boundary(out.len()));
    }

    /// Cut size smaller than the first char: must not panic; returns empty.
    #[test]
    fn cut_smaller_than_first_char_returns_empty() {
        let s = "\u{1F600}rest"; // first char is 4 bytes
        let out = truncate_at_char_boundary(s, 2);
        assert_eq!(out, "");
    }
}

#[cfg(test)]
mod hook_start_tests {
    use super::*;
    use icm_core::Importance;

    fn seed_store() -> SqliteStore {
        let store = SqliteStore::in_memory().unwrap();
        store
            .store(Memory::new(
                "decisions-icm".into(),
                "Use SQLite with FTS5 and sqlite-vec".into(),
                Importance::Critical,
            ))
            .unwrap();
        store
            .store(Memory::new(
                "decisions-other".into(),
                "OTHER project uses Postgres".into(),
                Importance::Critical,
            ))
            .unwrap();
        store
            .store(Memory::new(
                "preferences".into(),
                "User prefers French responses".into(),
                Importance::High,
            ))
            .unwrap();
        store
            .store(Memory::new(
                "low-noise".into(),
                "Irrelevant low-importance trivia".into(),
                Importance::Low,
            ))
            .unwrap();
        store
    }

    #[test]
    fn project_from_path_extracts_basename() {
        assert_eq!(
            project_from_path("/Users/patrick/dev/rtk-ai/icm"),
            Some("icm".into())
        );
        assert_eq!(
            project_from_path("/tmp/my-project"),
            Some("my-project".into())
        );
        assert_eq!(project_from_path(""), None);
    }

    #[test]
    fn hook_start_pack_scopes_to_cwd_project() {
        let store = seed_store();
        let stdin_json = r#"{"cwd":"/Users/patrick/dev/rtk-ai/icm","session_id":"abc"}"#;
        let pack = build_hook_start_pack(&store, stdin_json, 200).unwrap();
        assert!(pack.contains("SQLite"), "icm decision missing: {pack}");
        assert!(pack.contains("French"), "preference missing: {pack}");
        assert!(
            !pack.contains("Postgres"),
            "other project leaked into icm session: {pack}"
        );
        assert!(pack.contains("project: icm"));
    }

    #[test]
    fn hook_start_pack_empty_on_empty_store() {
        let store = SqliteStore::in_memory().unwrap();
        let stdin_json = r#"{"cwd":"/Users/patrick/dev/rtk-ai/icm"}"#;
        let pack = build_hook_start_pack(&store, stdin_json, 200).unwrap();
        assert!(
            pack.is_empty(),
            "expected empty pack for empty store, got: {pack}"
        );
    }

    #[test]
    fn hook_start_pack_tolerates_malformed_stdin() {
        let store = seed_store();
        // Not JSON at all — should fall back to project auto-detection or None
        let pack = build_hook_start_pack(&store, "garbage not json", 200).unwrap();
        // Either it auto-detected nothing (then all memories pass) or auto-detected a
        // real repo name — either way, must not panic and must produce valid output.
        assert!(!pack.is_empty());
        assert!(pack.starts_with("# ICM Wake-up"));
    }

    #[test]
    fn hook_start_pack_tolerates_missing_cwd_field() {
        let store = seed_store();
        let stdin_json = r#"{"session_id":"abc","transcript_path":"/tmp/t.jsonl"}"#;
        let pack = build_hook_start_pack(&store, stdin_json, 200).unwrap();
        // No cwd → falls back to detect_project() which will use current test
        // process PWD. We don't assert on the specific project but we do verify
        // the call doesn't fail and we get some output.
        assert!(pack.starts_with("# ICM Wake-up"));
    }

    #[test]
    fn hook_start_pack_respects_token_budget() {
        let store = SqliteStore::in_memory().unwrap();
        for i in 0..50 {
            store
                .store(Memory::new(
                    "decisions-icm".into(),
                    format!("Critical decision {i} with a reasonably long description text here"),
                    Importance::Critical,
                ))
                .unwrap();
        }
        let stdin_json = r#"{"cwd":"/path/icm"}"#;

        let small = build_hook_start_pack(&store, stdin_json, 50).unwrap();
        let large = build_hook_start_pack(&store, stdin_json, 500).unwrap();

        assert!(small.len() < large.len(), "budget should shrink output");
        assert!(
            small.len() < 500,
            "50 tok budget should stay under 500 chars"
        );
    }

    #[test]
    fn hook_start_pack_skips_placeholder_output() {
        let store = SqliteStore::in_memory().unwrap();
        // Only low-importance noise — wake-up would return the "(no critical
        // memories yet ...)" placeholder, which cmd_hook_start should suppress.
        store
            .store(Memory::new(
                "noise".into(),
                "nothing important".into(),
                Importance::Low,
            ))
            .unwrap();
        let pack = build_hook_start_pack(&store, r#"{"cwd":"/p/x"}"#, 200).unwrap();
        assert!(
            pack.is_empty(),
            "placeholder output should be suppressed to keep session clean: {pack}"
        );
    }

    #[test]
    fn hook_start_placeholder_detection_uses_exported_header() {
        // Regression guard: build an empty wake-up pack via icm_core and
        // assert it starts with the header that cmd_hook_start checks. If
        // someone reformats the placeholder in wake_up.rs, this test fails
        // and forces an update rather than silently breaking suppression.
        let empty_pack =
            icm_core::build_wake_up_from_memories(Vec::new(), &icm_core::WakeUpOptions::default());
        assert!(
            empty_pack.starts_with(icm_core::EMPTY_PACK_HEADER),
            "empty wake-up pack no longer starts with EMPTY_PACK_HEADER — \
             update the constant or adjust suppression logic: {empty_pack}"
        );
    }

    #[test]
    fn hook_start_pack_with_empty_cwd_string_falls_back() {
        let store = seed_store();
        // Edge case: cwd present but empty string — should fall through to
        // detect_project() rather than matching "" against topics.
        let stdin_json = r#"{"cwd":""}"#;
        let pack = build_hook_start_pack(&store, stdin_json, 200).unwrap();
        // We don't assert on which project was picked; we just require the
        // call does not panic and returns a valid, non-empty pack.
        assert!(!pack.is_empty());
        assert!(pack.starts_with("# ICM Wake-up"));
    }
}

#[cfg(test)]
mod inject_settings_hook_tests {
    use super::*;
    use tempfile::TempDir;

    fn read(path: &Path) -> Value {
        let raw = std::fs::read_to_string(path).unwrap();
        serde_json::from_str(&raw).unwrap()
    }

    fn extract_command(config: &Value, event: &str, idx: usize) -> String {
        config["hooks"][event][idx]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn writes_new_hook_when_settings_file_missing() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.json");

        let status = inject_settings_hook(
            &path,
            "SessionStart",
            "/opt/homebrew/bin/icm hook start",
            None,
            &["icm hook start", "icm hook"],
            false,
        )
        .unwrap();

        assert_eq!(status, "configured");
        assert!(path.exists());
        let cfg = read(&path);
        assert_eq!(
            extract_command(&cfg, "SessionStart", 0),
            "/opt/homebrew/bin/icm hook start"
        );
    }

    #[test]
    fn skips_when_already_configured_with_same_path() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.json");

        // First call installs the hook.
        inject_settings_hook(
            &path,
            "SessionStart",
            "/opt/homebrew/bin/icm hook start",
            None,
            &["icm hook start", "icm hook"],
            false,
        )
        .unwrap();

        // Second identical call must be a no-op.
        let status = inject_settings_hook(
            &path,
            "SessionStart",
            "/opt/homebrew/bin/icm hook start",
            None,
            &["icm hook start", "icm hook"],
            false,
        )
        .unwrap();

        assert_eq!(status, "already configured");
        let cfg = read(&path);
        assert_eq!(cfg["hooks"]["SessionStart"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn reports_stale_path_without_force() {
        // This is the exact bug the user hit: a previously-configured hook
        // pointing at a stale binary path is left untouched, but flagged.
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.json");

        // Pre-seed with a stale entry as if a previous `cargo run` left it.
        inject_settings_hook(
            &path,
            "SessionStart",
            "/Users/x/dev/icm/target/release/icm hook start",
            None,
            &["icm hook start", "icm hook"],
            false,
        )
        .unwrap();

        // New install with a different path but force=false must NOT overwrite.
        let status = inject_settings_hook(
            &path,
            "SessionStart",
            "/opt/homebrew/bin/icm hook start",
            None,
            &["icm hook start", "icm hook"],
            false,
        )
        .unwrap();

        assert!(
            status.contains("stale path"),
            "expected stale-path notice, got: {status}"
        );
        let cfg = read(&path);
        // The stale entry is preserved as-is.
        assert_eq!(
            extract_command(&cfg, "SessionStart", 0),
            "/Users/x/dev/icm/target/release/icm hook start"
        );
    }

    #[test]
    fn force_rewrites_stale_path_in_place() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.json");

        inject_settings_hook(
            &path,
            "SessionStart",
            "/Users/x/dev/icm/target/release/icm hook start",
            None,
            &["icm hook start", "icm hook"],
            false,
        )
        .unwrap();

        let status = inject_settings_hook(
            &path,
            "SessionStart",
            "/opt/homebrew/bin/icm hook start",
            None,
            &["icm hook start", "icm hook"],
            true,
        )
        .unwrap();

        assert!(status.starts_with("updated"), "got: {status}");
        let cfg = read(&path);
        assert_eq!(
            extract_command(&cfg, "SessionStart", 0),
            "/opt/homebrew/bin/icm hook start"
        );
        // Still exactly one entry — force updates in-place, doesn't append.
        assert_eq!(cfg["hooks"]["SessionStart"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn force_does_not_touch_unrelated_third_party_hooks() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{
                "hooks": {
                    "SessionStart": [
                        {
                            "hooks": [
                                { "type": "command", "command": "/usr/local/bin/some-other-tool" }
                            ]
                        }
                    ]
                }
            }"#,
        )
        .unwrap();

        let status = inject_settings_hook(
            &path,
            "SessionStart",
            "/opt/homebrew/bin/icm hook start",
            None,
            &["icm hook start", "icm hook"],
            true,
        )
        .unwrap();

        // No icm hook to overwrite → should append a new entry.
        assert_eq!(status, "configured");
        let cfg = read(&path);
        let arr = cfg["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(
            arr[0]["hooks"][0]["command"].as_str().unwrap(),
            "/usr/local/bin/some-other-tool"
        );
        assert_eq!(
            arr[1]["hooks"][0]["command"].as_str().unwrap(),
            "/opt/homebrew/bin/icm hook start"
        );
    }

    #[test]
    fn matcher_is_attached_when_provided() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.json");

        inject_settings_hook(
            &path,
            "PreToolUse",
            "/opt/homebrew/bin/icm hook pre",
            Some("Bash"),
            &["icm hook pre", "icm-pretool"],
            false,
        )
        .unwrap();

        let cfg = read(&path);
        assert_eq!(
            cfg["hooks"]["PreToolUse"][0]["matcher"].as_str().unwrap(),
            "Bash"
        );
    }
}
