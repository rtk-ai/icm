# Projet: ICM (Infinite Context Memory)

Un système de mémoire persistante pour LLMs, écrit en Rust, avec support MCP pour intégration Claude Code.

## Objectif

Créer une mémoire long-terme intelligente qui:
1. Stocke des memories avec embeddings dans Turso/SQLite
2. Fait du retrieval hybride (BM25 + vector similarity)
3. Gère le decay temporel et la consolidation
4. Expose un serveur MCP pour que Claude Code puisse stocker/récupérer des souvenirs

## Stack technique

- Rust (edition 2021)
- Turso/libsql pour le storage
- sqlite-vec pour les embeddings vectoriels
- tokio pour l'async
- serde pour la serialization
- clap pour le CLI
- MCP SDK Rust (ou implémentation manuelle du protocole JSON-RPC)

## Structure du workspace

```
icm/
├── Cargo.toml (workspace)
├── crates/
│   ├── icm-core/             # Types et traits fondamentaux
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── memory.rs     # Struct Memory, Importance, MemorySource
│   │       ├── store.rs      # Trait MemoryStore
│   │       └── embedder.rs   # Trait Embedder
│   │
│   ├── icm-store-turso/      # Implémentation Turso du store
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── turso.rs      # TursoStore impl MemoryStore
│   │       └── migrations.rs # Schema SQL
│   │
│   ├── icm-retriever/        # Recherche hybride
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── hybrid.rs     # HybridRetriever (BM25 + vector)
│   │       └── reranker.rs   # Reranking optionnel
│   │
│   ├── icm-extractor/        # Extraction de memories depuis du texte
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       └── llm.rs        # Appel LLM pour extraire facts/summaries
│   │
│   ├── icm-mcp/              # Serveur MCP pour Claude Code
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── main.rs
│   │       ├── tools.rs      # icm_store, icm_recall, icm_forget
│   │       └── protocol.rs   # JSON-RPC MCP handling
│   │
│   └── icm-cli/              # CLI standalone
│       ├── Cargo.toml
│       └── src/
│           └── main.rs       # Commands: store, recall, consolidate, prune
│
├── config/
│   └── default.toml          # Config par défaut
│
└── README.md
```

## Modèles de données

### Memory

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    pub id: String,                    // ULID ou UUID
    pub created_at: DateTime<Utc>,
    pub last_accessed: DateTime<Utc>,
    pub access_count: u32,
    pub weight: f32,                   // Calculé: decay * reinforcement
    
    pub topic: String,                 // Catégorie/tag principal
    pub summary: String,               // Résumé dense
    pub raw_excerpt: Option<String>,   // Verbatim optionnel
    pub keywords: Vec<String>,         // Pour BM25
    
    pub embedding: Option<Vec<f32>>,   // 1536 dims (OpenAI) ou 384 (local)
    pub importance: Importance,
    pub source: MemorySource,
    
    pub related_ids: Vec<String>,      // Liens vers autres memories
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Importance {
    Critical,  // Ne jamais oublier
    High,      // Decay lent
    Medium,    // Decay normal
    Low,       // Decay rapide
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MemorySource {
    ClaudeCode { session_id: String, file_path: Option<String> },
    Conversation { thread_id: String },
    Manual,
}
```

### Schema SQL (Turso)

```sql
CREATE TABLE memories (
    id TEXT PRIMARY KEY,
    created_at TEXT NOT NULL,
    last_accessed TEXT NOT NULL,
    access_count INTEGER DEFAULT 0,
    weight REAL DEFAULT 1.0,
    
    topic TEXT NOT NULL,
    summary TEXT NOT NULL,
    raw_excerpt TEXT,
    keywords TEXT, -- JSON array
    
    embedding F32_BLOB(1536),
    importance TEXT NOT NULL,
    source_type TEXT NOT NULL,
    source_data TEXT, -- JSON
    
    related_ids TEXT -- JSON array
);

CREATE INDEX idx_memories_topic ON memories(topic);
CREATE INDEX idx_memories_weight ON memories(weight);
CREATE INDEX idx_memories_created ON memories(created_at);

-- Table pour le full-text search (BM25)
CREATE VIRTUAL TABLE memories_fts USING fts5(
    id,
    topic,
    summary,
    keywords,
    content='memories',
    content_rowid='rowid'
);

-- Triggers pour sync FTS
CREATE TRIGGER memories_ai AFTER INSERT ON memories BEGIN
    INSERT INTO memories_fts(id, topic, summary, keywords)
    VALUES (new.id, new.topic, new.summary, new.keywords);
END;

CREATE TRIGGER memories_ad AFTER DELETE ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, id, topic, summary, keywords)
    VALUES('delete', old.id, old.topic, old.summary, old.keywords);
END;

CREATE TRIGGER memories_au AFTER UPDATE ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, id, topic, summary, keywords)
    VALUES('delete', old.id, old.topic, old.summary, old.keywords);
    INSERT INTO memories_fts(id, topic, summary, keywords)
    VALUES (new.id, new.topic, new.summary, new.keywords);
END;
```

## Traits principaux

### MemoryStore

```rust
use async_trait::async_trait;
use anyhow::Result;

#[async_trait]
pub trait MemoryStore: Send + Sync {
    // CRUD basique
    async fn store(&self, memory: Memory) -> Result<String>;
    async fn get(&self, id: &str) -> Result<Option<Memory>>;
    async fn update(&self, memory: Memory) -> Result<()>;
    async fn delete(&self, id: &str) -> Result<()>;
    
    // Recherche
    async fn search_by_embedding(&self, embedding: &[f32], limit: usize) -> Result<Vec<Memory>>;
    async fn search_by_keywords(&self, keywords: &[&str], limit: usize) -> Result<Vec<Memory>>;
    async fn search_hybrid(&self, query: &str, embedding: &[f32], limit: usize) -> Result<Vec<Memory>>;
    
    // Gestion du lifecycle
    async fn update_access(&self, id: &str) -> Result<()>;
    async fn apply_decay(&self, decay_factor: f32) -> Result<usize>;
    async fn prune(&self, weight_threshold: f32) -> Result<usize>;
    
    // Organisation
    async fn get_by_topic(&self, topic: &str) -> Result<Vec<Memory>>;
    async fn list_topics(&self) -> Result<Vec<String>>;
    async fn consolidate_topic(&self, topic: &str, consolidated: Memory) -> Result<()>;
    
    // Stats
    async fn count(&self) -> Result<usize>;
    async fn stats(&self) -> Result<StoreStats>;
}

#[derive(Debug, Clone)]
pub struct StoreStats {
    pub total_memories: usize,
    pub total_topics: usize,
    pub avg_weight: f32,
    pub oldest_memory: Option<DateTime<Utc>>,
    pub newest_memory: Option<DateTime<Utc>>,
}
```

### Embedder

```rust
#[async_trait]
pub trait Embedder: Send + Sync {
    async fn embed(&self, text: &str) -> Result<Vec<f32>>;
    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;
    fn dimensions(&self) -> usize;
    fn model_name(&self) -> &str;
}
```

### Extractor

```rust
#[async_trait]
pub trait Extractor: Send + Sync {
    /// Extrait des memories depuis une conversation/texte
    async fn extract(&self, text: &str, source: MemorySource) -> Result<Vec<Memory>>;
    
    /// Génère un résumé consolidé de plusieurs memories
    async fn consolidate(&self, memories: &[Memory]) -> Result<Memory>;
    
    /// Détermine l'importance d'une information
    async fn assess_importance(&self, content: &str) -> Result<Importance>;
}
```

## MCP Server Tools

Le serveur MCP doit exposer ces tools pour Claude Code:

### icm_store

```json
{
  "name": "icm_store",
  "description": "Stocker une information importante dans la mémoire long-terme ICM. Utiliser pour sauvegarder des décisions, préférences, contextes de projet, ou tout ce qui devrait persister entre les sessions.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "topic": {
        "type": "string",
        "description": "Catégorie/namespace (ex: 'projet-kexa', 'preferences-user', 'decisions-architecture', 'erreurs-resolues')"
      },
      "content": {
        "type": "string",
        "description": "Information à mémoriser - être concis mais complet"
      },
      "importance": {
        "type": "string",
        "enum": ["critical", "high", "medium", "low"],
        "default": "medium",
        "description": "critical=jamais oublié, high=decay lent, medium=normal, low=decay rapide"
      },
      "keywords": {
        "type": "array",
        "items": { "type": "string" },
        "description": "Mots-clés pour améliorer la recherche"
      },
      "raw_excerpt": {
        "type": "string",
        "description": "Verbatim optionnel (code, message d'erreur exact, etc.)"
      }
    },
    "required": ["topic", "content"]
  }
}
```

### icm_recall

```json
{
  "name": "icm_recall",
  "description": "Rechercher dans la mémoire long-terme ICM. Utiliser pour retrouver des décisions passées, du contexte de projet, des préférences, ou des solutions à des problèmes déjà rencontrés.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "query": {
        "type": "string",
        "description": "Requête de recherche en langage naturel"
      },
      "topic": {
        "type": "string",
        "description": "Filtrer par topic spécifique (optionnel)"
      },
      "limit": {
        "type": "integer",
        "default": 5,
        "minimum": 1,
        "maximum": 20,
        "description": "Nombre max de résultats"
      },
      "min_weight": {
        "type": "number",
        "default": 0.0,
        "description": "Poids minimum des memories à retourner"
      }
    },
    "required": ["query"]
  }
}
```

### icm_forget

```json
{
  "name": "icm_forget",
  "description": "Oublier une mémoire spécifique par son ID. Utiliser quand une information est obsolète ou incorrecte.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "id": {
        "type": "string",
        "description": "ID de la mémoire à supprimer"
      }
    },
    "required": ["id"]
  }
}
```

### icm_consolidate

```json
{
  "name": "icm_consolidate",
  "description": "Consolider toutes les mémoires d'un topic en un résumé unique. Utile quand un topic accumule trop d'entrées.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "topic": {
        "type": "string",
        "description": "Topic à consolider"
      },
      "keep_originals": {
        "type": "boolean",
        "default": false,
        "description": "Garder les mémoires originales après consolidation"
      }
    },
    "required": ["topic"]
  }
}
```

### icm_list_topics

```json
{
  "name": "icm_list_topics",
  "description": "Lister tous les topics disponibles dans la mémoire avec leurs stats.",
  "inputSchema": {
    "type": "object",
    "properties": {}
  }
}
```

### icm_stats

```json
{
  "name": "icm_stats",
  "description": "Obtenir les statistiques globales de la mémoire ICM.",
  "inputSchema": {
    "type": "object",
    "properties": {}
  }
}
```

## Configuration

```toml
# config/default.toml

[store]
# Type de store: "turso" ou "sqlite"
type = "turso"

# Pour Turso (remote)
url = "libsql://your-db.turso.io"
auth_token = ""  # Ou via env: TURSO_AUTH_TOKEN

# Pour SQLite local (alternative)
# type = "sqlite"
# path = "~/.icm/memories.db"

[embedder]
# Type: "openai", "anthropic", "local", "none"
type = "openai"
model = "text-embedding-3-small"  # 1536 dims
api_key = ""  # Ou via env: OPENAI_API_KEY

# Alternative locale (plus lent mais gratuit)
# type = "local"
# model = "all-MiniLM-L6-v2"  # 384 dims

[memory]
# Importance par défaut pour les nouvelles memories
default_importance = "medium"

# Taux de decay par jour (0.95 = perd 5% par jour)
decay_rate = 0.95

# Seuil de poids pour le pruning automatique
prune_threshold = 0.1

# Nombre de memories dans un topic avant consolidation auto
consolidation_threshold = 10

# Multiplicateurs de decay par importance
[memory.decay_multipliers]
critical = 0.0    # Jamais de decay
high = 0.5        # Decay 2x plus lent
medium = 1.0      # Decay normal
low = 2.0         # Decay 2x plus rapide

[retriever]
# Poids pour la recherche hybride (BM25 vs vector)
bm25_weight = 0.3
vector_weight = 0.7

# Nombre de candidats pour le reranking
rerank_candidates = 20

[mcp]
# Configuration du serveur MCP
host = "127.0.0.1"
port = 3000
# Transport: "stdio" ou "http"
transport = "stdio"

[logging]
level = "info"  # debug, info, warn, error
format = "pretty"  # pretty, json
```

## CLI Commands

```bash
# === CRUD ===

# Stocker une mémoire
icm store --topic "projet-kexa" --content "Architecture: utiliser Turso pour la DB" --importance high
icm store -t "preferences" -c "User préfère Rust à Go" -i medium -k "rust,go,language"

# Rechercher
icm recall "architecture database"
icm recall "préférences langage" --topic "preferences" --limit 10

# Supprimer
icm forget <memory-id>

# === ORGANISATION ===

# Lister les topics
icm topics

# Voir les memories d'un topic
icm list --topic "projet-kexa"
icm list --all --sort weight  # Toutes, triées par poids

# Consolider un topic
icm consolidate --topic "projet-kexa"
icm consolidate --topic "erreurs" --keep-originals

# === MAINTENANCE ===

# Appliquer le decay (à mettre en cron daily)
icm decay
icm decay --factor 0.9  # Override le config

# Pruner les memories à faible poids
icm prune
icm prune --threshold 0.2 --dry-run  # Preview

# Stats globales
icm stats

# === SERVEUR MCP ===

# Lancer le serveur (mode stdio pour Claude Code)
icm serve

# Lancer en mode HTTP (pour debug/autres clients)
icm serve --transport http --port 3000

# === CONFIG ===

# Voir la config active
icm config show

# Init une nouvelle config
icm config init

# Tester la connexion
icm config test
```

## Intégration Claude Code

### Configuration MCP dans Claude Code

Ajouter dans `~/.config/claude-code/mcp.json` (ou équivalent):

```json
{
  "mcpServers": {
    "icm": {
      "command": "icm",
      "args": ["serve"],
      "env": {
        "TURSO_AUTH_TOKEN": "your-token",
        "OPENAI_API_KEY": "your-key"
      }
    }
  }
}
```

### Comportement attendu dans Claude Code

Quand ICM est configuré, Claude Code devrait:

1. **Au début d'une session**: Faire un `icm_recall` avec le contexte du projet pour charger les memories pertinentes

2. **Pendant la session**: 
   - `icm_store` les décisions importantes
   - `icm_store` les erreurs résolues et leurs solutions
   - `icm_recall` quand une question similaire à un problème passé survient

3. **En fin de session** (optionnel): Proposer de `icm_consolidate` si beaucoup de nouvelles memories

### Exemple de prompts système pour Claude Code

```
Tu as accès à ICM (Infinite Context Memory), un système de mémoire persistante.

UTILISE icm_recall au début de chaque tâche pour vérifier si des informations pertinentes existent.

UTILISE icm_store pour sauvegarder:
- Les décisions d'architecture importantes
- Les préférences de l'utilisateur
- Les erreurs résolues et leurs solutions
- Le contexte spécifique au projet

Topics suggérés:
- "decisions-{projet}" : Choix d'architecture, libs, patterns
- "preferences" : Style de code, conventions, outils préférés
- "erreurs-resolues" : Problèmes rencontrés et solutions
- "contexte-{projet}" : Infos spécifiques au projet
```

## Implémentation prioritaire

### Phase 1: Core + Storage (MVP)
- [ ] `icm-core`: Structs Memory, Importance, MemorySource
- [ ] `icm-core`: Traits MemoryStore, Embedder
- [ ] `icm-store-turso`: Implémentation basique CRUD
- [ ] `icm-cli`: Commands store, recall, list, forget
- [ ] Tests unitaires core

### Phase 2: Recherche intelligente
- [ ] `icm-store-turso`: Full-text search avec FTS5
- [ ] `icm-retriever`: Recherche hybride BM25 + vector
- [ ] Intégration embeddings OpenAI
- [ ] `icm-cli`: Amélioration de recall avec scores

### Phase 3: Serveur MCP
- [ ] `icm-mcp`: Protocol JSON-RPC basique
- [ ] `icm-mcp`: 6 tools (store, recall, forget, consolidate, list_topics, stats)
- [ ] Transport stdio pour Claude Code
- [ ] Documentation intégration

### Phase 4: Intelligence
- [ ] `icm-extractor`: Extraction automatique via LLM
- [ ] Decay automatique (cron ou daemon)
- [ ] Consolidation intelligente
- [ ] Détection de contradictions
- [ ] Linking automatique entre memories

### Phase 5: Polish
- [ ] Transport HTTP pour `icm-mcp`
- [ ] Dashboard web (optionnel)
- [ ] Embeddings locaux (alternative à OpenAI)
- [ ] Export/Import
- [ ] Multi-user support

## Contraintes techniques

- **Pas de `unwrap()`** - Gestion d'erreur propre avec `thiserror` pour les erreurs typées et `anyhow` pour le CLI
- **Tests**: Unit tests pour chaque module, integration tests pour le store
- **Documentation**: Rustdoc complète, exemples dans les docstrings
- **CI-ready**: `cargo fmt`, `cargo clippy -- -D warnings`, `cargo test`
- **Async-first**: Tout le I/O est async avec tokio
- **Config flexible**: Support fichier TOML + env vars + CLI flags

## Dépendances suggérées

```toml
# Workspace Cargo.toml
[workspace]
resolver = "2"
members = [
    "crates/icm-core",
    "crates/icm-store-turso",
    "crates/icm-retriever",
    "crates/icm-extractor",
    "crates/icm-mcp",
    "crates/icm-cli",
]

[workspace.dependencies]
# Async
tokio = { version = "1", features = ["full"] }
async-trait = "0.1"

# Serialization
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# Database
libsql = "0.6"

# Error handling
thiserror = "2"
anyhow = "1"

# CLI
clap = { version = "4", features = ["derive"] }

# Utils
chrono = { version = "0.4", features = ["serde"] }
ulid = "1"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }

# Config
config = "0.14"
directories = "5"

# HTTP client (pour embeddings API)
reqwest = { version = "0.12", features = ["json"] }

# MCP
# Note: utiliser rmcp ou implémenter le protocol manuellement
```

## Pour commencer

Lance Claude Code et donne-lui ce fichier comme contexte, puis demande:

```
Génère le workspace ICM complet en suivant la spec. 
Commence par:
1. Créer la structure des dossiers
2. Tous les Cargo.toml
3. icm-core avec les types et traits
4. icm-store-turso avec le CRUD basique
5. icm-cli minimal (store et recall)

Ensuite on itérera sur les autres phases.
```

utilise la commande rtk cli pour économiser les tokens.
utilise vox pour me parler à la fin des tâches par résumé.
ils sont sur : https://github.com/rtk-ai