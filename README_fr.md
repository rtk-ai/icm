[English](README.md) | **Français** | [Español](README_es.md) | [Deutsch](README_de.md) | [Italiano](README_it.md) | [Português](README_pt.md) | [Nederlands](README_nl.md) | [Polski](README_pl.md) | [Русский](README_ru.md) | [日本語](README_ja.md) | [中文](README_zh.md) | [العربية](README_ar.md) | [한국어](README_ko.md)

<p align="center">
  <img src="assets/banner.png" alt="ICM — Infinite Context Memory" width="600">
</p>

<h1 align="center">ICM</h1>

<p align="center">
  Mémoire permanente pour agents IA. Binaire unique, zéro dépendance, natif MCP.
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/icm/actions/workflows/ci.yml"><img src="https://github.com/rtk-ai/icm/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/icm/releases/latest"><img src="https://img.shields.io/github/v/release/rtk-ai/icm?color=purple" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-Source--Available-orange.svg" alt="Source-Available"></a>
</p>

---

ICM donne à votre agent IA une vraie mémoire — pas un outil de prise de notes, pas un gestionnaire de contexte, une **mémoire**.

```
                       ICM (Infinite Context Memory)
            ┌──────────────────────┬─────────────────────────┐
            │   MEMORIES (Topics)  │   MEMOIRS (Knowledge)   │
            │                      │                         │
            │  Épisodique, temporel│  Permanent, structuré   │
            │                      │                         │
            │  ┌───┐ ┌───┐ ┌───┐  │    ┌───┐               │
            │  │ m │ │ m │ │ m │  │    │ C │──depends_on──┐ │
            │  └─┬─┘ └─┬─┘ └─┬─┘  │    └───┘              │ │
            │    │decay │     │    │      │ refines      ┌─▼─┐│
            │    ▼      ▼     ▼    │    ┌─▼─┐            │ C ││
            │  le poids diminue    │    │ C │──part_of──>└───┘│
            │  avec le temps sauf  │    └───┘                 │
            │  si accédé/critique  │  Concepts + Relations    │
            ├──────────────────────┴─────────────────────────┤
            │             SQLite + FTS5 + sqlite-vec          │
            │        Recherche hybride: BM25 (30%) + cosine (70%) │
            └─────────────────────────────────────────────────┘
```

**Deux modèles de mémoire :**

- **Memories** — stockage/rappel avec décroissance temporelle par importance. Les souvenirs critiques ne s'effacent jamais, ceux de faible importance décroissent naturellement. Filtrage par topic ou mot-clé.
- **Memoirs** — graphes de connaissance permanents. Concepts reliés par des relations typées (`depends_on`, `contradicts`, `superseded_by`, ...). Filtrage par label.
- **Feedback** — enregistrer les corrections quand les prédictions IA sont fausses. Rechercher les erreurs passées avant de faire de nouvelles prédictions. Apprentissage en boucle fermée.

## Installation

```bash
# Homebrew (macOS / Linux)
brew tap rtk-ai/tap && brew install icm

# Installation rapide
curl -fsSL https://raw.githubusercontent.com/rtk-ai/icm/main/install.sh | sh

# Depuis les sources
cargo install --path crates/icm-cli
```

## Configuration

```bash
# Détection automatique et configuration de tous les outils supportés
icm init
```

Configure **17 outils** en une seule commande ([guide d'intégration complet](docs/integrations.md)) :

| Outil | MCP | Hooks | CLI | Skills |
|-------|:---:|:-----:|:---:|:------:|
| Claude Code | `~/.claude.json` | 5 hooks | `CLAUDE.md` | `/recall` `/remember` |
| Claude Desktop | JSON | — | — | — |
| Gemini CLI | `~/.gemini/settings.json` | 5 hooks | `GEMINI.md` | — |
| Codex CLI | `~/.codex/config.toml` | 4 hooks | `AGENTS.md` | — |
| Copilot CLI | `~/.copilot/mcp-config.json` | 4 hooks | `.github/copilot-instructions.md` | — |
| Cursor | `~/.cursor/mcp.json` | — | — | règle `.mdc` |
| Windsurf | JSON | — | `.windsurfrules` | — |
| VS Code | `~/Library/.../Code/User/mcp.json` | — | — | — |
| Amp | JSON | — | — | `/icm-recall` `/icm-remember` |
| Amazon Q | JSON | — | — | — |
| Cline | VS Code globalStorage | — | — | — |
| Roo Code | VS Code globalStorage | — | — | règle `.md` |
| Kilo Code | VS Code globalStorage | — | — | — |
| Zed | `~/.zed/settings.json` | — | — | — |
| OpenCode | JSON | plugin TS | — | — |
| Continue.dev | `~/.continue/config.yaml` | — | — | — |
| Aider | — | — | `.aider.conventions.md` | — |

Ou manuellement :

```bash
# Claude Code
claude mcp add icm -- icm serve

# Mode compact (réponses plus courtes, économise des tokens)
claude mcp add icm -- icm serve --compact

# N'importe quel client MCP : command = "icm", args = ["serve"]
```

### Skills / règles

```bash
icm init --mode skill
```

Installe des commandes slash et des règles pour Claude Code (`/recall`, `/remember`), Cursor (règle `.mdc`), Roo Code (règle `.md`), et Amp (`/icm-recall`, `/icm-remember`).

### Hooks (5 outils)

```bash
icm init --mode hook
```

Installe les hooks d'auto-extraction et d'auto-rappel pour tous les outils supportés :

| Outil | SessionStart | PreTool | PostTool | Compact | PromptRecall | Config |
|-------|:-----------:|:-------:|:--------:|:-------:|:------------:|--------|
| Claude Code | `icm hook start` | `icm hook pre` | `icm hook post` | `icm hook compact` | `icm hook prompt` | `~/.claude/settings.json` |
| Gemini CLI | `icm hook start` | `icm hook pre` | `icm hook post` | `icm hook compact` | `icm hook prompt` | `~/.gemini/settings.json` |
| Codex CLI | `icm hook start` | `icm hook pre` | `icm hook post` | — | `icm hook prompt` | `~/.codex/hooks.json` |
| Copilot CLI | `icm hook start` | `icm hook pre` | `icm hook post` | — | `icm hook prompt` | `.github/hooks/icm.json` |
| OpenCode | session start | — | tool extract | compaction | — | `~/.config/opencode/plugins/icm.ts` |

**Ce que fait chaque hook :**

| Hook | Ce qu'il fait |
|------|---------------|
| `icm hook start` | Injecte un pack de démarrage de souvenirs critiques/haute importance au début de session (~500 tokens) |
| `icm hook pre` | Autorise automatiquement les commandes `icm` CLI (sans invite de permission) |
| `icm hook post` | Extrait des faits depuis la sortie des outils toutes les N invocations (auto-extraction) |
| `icm hook compact` | Extrait des souvenirs depuis la transcription avant la compression du contexte |
| `icm hook prompt` | Injecte le contexte rappelé au début de chaque prompt utilisateur |

## CLI vs MCP

ICM peut être utilisé via CLI (commandes `icm`) ou serveur MCP (`icm serve`). Les deux accèdent à la même base de données.

| | CLI | MCP |
|---|-----|-----|
| **Latence** | ~30ms (binaire direct) | ~50ms (JSON-RPC stdio) |
| **Coût en tokens** | 0 (basé sur hooks, invisible) | ~20-50 tokens/appel (schéma d'outil) |
| **Configuration** | `icm init --mode hook` | `icm init --mode mcp` |
| **Compatible avec** | Claude Code, Gemini, Codex, Copilot, OpenCode (via hooks) | Les 17 outils compatibles MCP |
| **Auto-extraction** | Oui (hooks déclenchent `icm extract`) | Oui (outils MCP appellent store) |
| **Idéal pour** | Utilisateurs avancés, économie de tokens | Compatibilité universelle |

## CLI

### Memories (épisodiques, avec décroissance)

```bash
# Stocker
icm store -t "my-project" -c "Use PostgreSQL for the main DB" -i high -k "db,postgres"

# Rappeler
icm recall "database choice"
icm recall "auth setup" --topic "my-project" --limit 10
icm recall "architecture" --keyword "postgres"

# Gérer
icm forget <memory-id>
icm consolidate --topic "my-project"
icm topics
icm stats

# Extraire des faits depuis du texte (basé sur des règles, zéro coût LLM)
echo "The parser uses Pratt algorithm" | icm extract -p my-project
```

### Memoirs (graphes de connaissance permanents)

```bash
# Créer un memoir
icm memoir create -n "system-architecture" -d "System design decisions"

# Ajouter des concepts avec labels
icm memoir add-concept -m "system-architecture" -n "auth-service" \
  -d "Handles JWT tokens and OAuth2 flows" -l "domain:auth,type:service"

# Relier des concepts
icm memoir link -m "system-architecture" --from "api-gateway" --to "auth-service" -r depends-on

# Rechercher avec filtre par label
icm memoir search -m "system-architecture" "authentication"
icm memoir search -m "system-architecture" "service" --label "domain:auth"

# Inspecter le voisinage
icm memoir inspect -m "system-architecture" "auth-service" -D 2

# Exporter le graphe (formats : json, dot, ascii, ai)
icm memoir export -m "system-architecture" -f ascii   # Tracé en boîtes avec barres de confiance
icm memoir export -m "system-architecture" -f dot      # Graphviz DOT (couleur = niveau de confiance)
icm memoir export -m "system-architecture" -f ai       # Markdown optimisé pour contexte LLM
icm memoir export -m "system-architecture" -f json     # JSON structuré avec toutes les métadonnées

# Générer une visualisation SVG
icm memoir export -m "system-architecture" -f dot | dot -Tsvg > graph.svg
```

## Outils MCP (22)

### Outils Memory

| Outil | Description |
|-------|-------------|
| `icm_memory_store` | Stocker avec déduplication automatique (>85% de similarité → mise à jour au lieu de dupliquer) |
| `icm_memory_recall` | Rechercher par requête, filtrer par topic et/ou mot-clé |
| `icm_memory_update` | Modifier un souvenir sur place (contenu, importance, mots-clés) |
| `icm_memory_forget` | Supprimer un souvenir par ID |
| `icm_memory_consolidate` | Fusionner tous les souvenirs d'un topic en un résumé |
| `icm_memory_list_topics` | Lister tous les topics avec leurs compteurs |
| `icm_memory_stats` | Statistiques globales de la mémoire |
| `icm_memory_health` | Audit d'hygiène par topic (obsolescence, besoins de consolidation) |
| `icm_memory_embed_all` | Regénérer les embeddings pour la recherche vectorielle |

### Outils Memoir (graphes de connaissance)

| Outil | Description |
|-------|-------------|
| `icm_memoir_create` | Créer un nouveau memoir (conteneur de connaissance) |
| `icm_memoir_list` | Lister tous les memoirs |
| `icm_memoir_show` | Afficher les détails d'un memoir et tous ses concepts |
| `icm_memoir_add_concept` | Ajouter un concept avec des labels |
| `icm_memoir_refine` | Mettre à jour la définition d'un concept |
| `icm_memoir_search` | Recherche plein texte, optionnellement filtrée par label |
| `icm_memoir_search_all` | Rechercher dans tous les memoirs |
| `icm_memoir_link` | Créer une relation typée entre concepts |
| `icm_memoir_inspect` | Inspecter un concept et son voisinage dans le graphe (BFS) |
| `icm_memoir_export` | Exporter le graphe (json, dot, ascii, ai) avec niveaux de confiance |

### Outils Feedback (apprentissage par les erreurs)

| Outil | Description |
|-------|-------------|
| `icm_feedback_record` | Enregistrer une correction quand une prédiction IA est fausse |
| `icm_feedback_search` | Rechercher des corrections passées pour éclairer les prédictions futures |
| `icm_feedback_stats` | Statistiques de feedback : nombre total, répartition par topic, les plus appliquées |

### Types de relations

`part_of` · `depends_on` · `related_to` · `contradicts` · `refines` · `alternative_to` · `caused_by` · `instance_of` · `superseded_by`

## Fonctionnement

### Modèle de mémoire dual

**La mémoire épisodique (Topics)** capture les décisions, erreurs et préférences. Chaque souvenir a un poids qui décroît dans le temps selon son importance :

| Importance | Décroissance | Élagage | Comportement |
|------------|--------------|---------|--------------|
| `critical` | aucune | jamais | Jamais oublié, jamais élagué |
| `high` | lente (0,5x) | jamais | S'efface lentement, jamais supprimé automatiquement |
| `medium` | normale | oui | Décroissance standard, élagué quand le poids < seuil |
| `low` | rapide (2x) | oui | Oublié rapidement |

La décroissance est **sensible aux accès** : les souvenirs fréquemment rappelés décroissent plus lentement (`decay / (1 + access_count × 0.1)`). Appliquée automatiquement au rappel (si >24h depuis la dernière décroissance).

**L'hygiène mémorielle** est intégrée :
- **Déduplication automatique** : stocker un contenu à plus de 85% de similarité avec un souvenir existant dans le même topic le met à jour au lieu de créer un doublon
- **Conseils de consolidation** : quand un topic dépasse 7 entrées, `icm_memory_store` avertit l'appelant de consolider
- **Audit de santé** : `icm_memory_health` rapporte le nombre d'entrées par topic, le poids moyen, les entrées obsolètes et les besoins de consolidation
- **Pas de perte de données silencieuse** : les souvenirs critiques et à haute importance ne sont jamais élagués automatiquement

**La mémoire sémantique (Memoirs)** capture des connaissances structurées sous forme de graphe. Les concepts sont permanents — ils se raffinent, ne décroissent jamais. Utilisez `superseded_by` pour marquer les faits obsolètes plutôt que de les supprimer.

### Recherche hybride

Avec les embeddings activés, ICM utilise la recherche hybride :
- **FTS5 BM25** (30%) — correspondance de mots-clés en plein texte
- **Similarité cosinus** (70%) — recherche vectorielle sémantique via sqlite-vec

Modèle par défaut : `intfloat/multilingual-e5-base` (768d, 100+ langues). Configurable dans votre [fichier de configuration](#configuration) :

```toml
[embeddings]
# enabled = false                          # Désactiver entièrement (pas de téléchargement de modèle)
model = "intfloat/multilingual-e5-base"    # 768d, multilingue (défaut)
# model = "intfloat/multilingual-e5-small" # 384d, multilingue (plus léger)
# model = "intfloat/multilingual-e5-large" # 1024d, multilingue (meilleure précision)
# model = "Xenova/bge-small-en-v1.5"      # 384d, anglais uniquement (plus rapide)
# model = "jinaai/jina-embeddings-v2-base-code"  # 768d, optimisé pour le code
```

Pour ignorer entièrement le téléchargement du modèle d'embedding, utilisez l'une de ces options :
```bash
icm --no-embeddings serve          # Flag CLI
ICM_NO_EMBEDDINGS=1 icm serve     # Variable d'environnement
```
Ou définissez `enabled = false` dans votre fichier de configuration. ICM bascule alors sur la recherche par mots-clés FTS5 (fonctionne toujours, sans correspondance sémantique).

Changer de modèle recrée automatiquement l'index vectoriel (les embeddings existants sont effacés et peuvent être régénérés avec `icm_memory_embed_all`).

### Stockage

Fichier SQLite unique. Aucun service externe, aucune dépendance réseau.

```
~/Library/Application Support/dev.icm.icm/memories.db                    # macOS
~/.local/share/dev.icm.icm/memories.db                                   # Linux
C:\Users\<user>\AppData\Local\icm\icm\data\memories.db                   # Windows
```

### Configuration

```bash
icm config                    # Afficher la configuration active
```

Emplacement du fichier de configuration (spécifique à la plateforme, ou `$ICM_CONFIG`) :

```
~/Library/Application Support/dev.icm.icm/config.toml                    # macOS
~/.config/icm/config.toml                                                # Linux
C:\Users\<user>\AppData\Roaming\icm\icm\config\config.toml              # Windows
```

Voir [config/default.toml](config/default.toml) pour toutes les options.

## Auto-extraction

ICM extrait des souvenirs automatiquement via trois couches :

```
  Couche 0 : Hooks de patterns      Couche 1 : PreCompact         Couche 2 : UserPromptSubmit
  (zéro coût LLM)                   (zéro coût LLM)               (zéro coût LLM)
  ┌──────────────────┐              ┌──────────────────┐          ┌──────────────────┐
  │ Hook PostToolUse  │              │ Hook PreCompact   │          │ UserPromptSubmit  │
  │                   │              │                   │          │                   │
  │ • Erreurs Bash    │              │ Contexte sur le   │          │ Utilisateur envoie│
  │ • commits git     │              │ point d'être      │          │ un prompt         │
  │ • changements cfg │              │ compressé →       │          │ → icm recall      │
  │ • décisions       │              │ extraire souvenirs │          │ → injecter contexte│
  │ • préférences     │              │ depuis transcript  │          │                   │
  │ • apprentissages  │              │ avant qu'ils       │          │ L'agent démarre   │
  │ • contraintes     │              │ soient perdus      │          │ avec les souvenirs │
  │                   │              │                   │          │ pertinents déjà   │
  │ Basé sur règles,  │              │ Mêmes patterns +  │          │ chargés            │
  │ sans LLM          │              │ --store-raw fallbk│          │                   │
  └──────────────────┘              └──────────────────┘          └──────────────────┘
```

| Couche | Statut | Coût LLM | Commande hook | Description |
|--------|--------|----------|---------------|-------------|
| Couche 0 | Implémentée | 0 | `icm hook post` | Extraction par mots-clés basée sur règles depuis la sortie des outils |
| Couche 1 | Implémentée | 0 | `icm hook compact` | Extraction depuis la transcription avant compression du contexte |
| Couche 2 | Implémentée | 0 | `icm hook prompt` | Injection des souvenirs rappelés à chaque prompt utilisateur |

Les 3 couches sont installées automatiquement par `icm init --mode hook`.

### Comparaison avec les alternatives

| Système | Méthode | Coût LLM | Latence | Capture la compaction ? |
|---------|---------|----------|---------|------------------------|
| **ICM** | Extraction 3 couches | 0 à ~500 tok/session | 0ms | **Oui (PreCompact)** |
| Mem0 | 2 appels LLM/message | ~2k tok/message | 200-2000ms | Non |
| claude-mem | PostToolUse + async | ~1-5k tok/session | 8ms hook | Non |
| MemGPT/Letta | L'agent se gère lui-même | 0 marginal | 0ms | Non |
| DiffMem | Diffs basés sur Git | 0 | 0ms | Non |

## Benchmarks

### Performance de stockage

```
ICM Benchmark (1000 memories, 384d embeddings)
──────────────────────────────────────────────────────────
Store (no embeddings)      1000 ops      34.2 ms      34.2 µs/op
Store (with embeddings)    1000 ops      51.6 ms      51.6 µs/op
FTS5 search                 100 ops       4.7 ms      46.6 µs/op
Vector search (KNN)         100 ops      59.0 ms     590.0 µs/op
Hybrid search               100 ops      95.1 ms     951.1 µs/op
Decay (batch)                 1 ops       5.8 ms       5.8 ms/op
──────────────────────────────────────────────────────────
```

Apple M1 Pro, SQLite en mémoire, mono-thread. `icm bench --count 1000`

### Efficacité agent

Flux de travail multi-session sur un vrai projet Rust (12 fichiers, ~550 lignes). Les sessions 2+ montrent les gains les plus importants car ICM rappelle au lieu de relire les fichiers.

```
ICM Agent Benchmark (10 sessions, model: haiku, 3 runs averaged)
══════════════════════════════════════════════════════════════════
                            Without ICM         With ICM      Delta
Session 2 (recall)
  Turns                             5.7              4.0       -29%
  Context (input)                 99.9k            67.5k       -32%
  Cost                          $0.0298          $0.0249       -17%

Session 3 (recall)
  Turns                             3.3              2.0       -40%
  Context (input)                 74.7k            41.6k       -44%
  Cost                          $0.0249          $0.0194       -22%
══════════════════════════════════════════════════════════════════
```

`icm bench-agent --sessions 10 --model haiku`

### Rétention de connaissance

L'agent rappelle des faits spécifiques depuis un document technique dense à travers les sessions. La session 1 lit et mémorise ; les sessions 2+ répondent à 10 questions factuelles **sans** le texte source.

```
ICM Recall Benchmark (10 questions, model: haiku, 5 runs averaged)
══════════════════════════════════════════════════════════════════════
                                               No ICM     With ICM
──────────────────────────────────────────────────────────────────────
Average score                                      5%          68%
Questions passed                                 0/10         5/10
══════════════════════════════════════════════════════════════════════
```

`icm bench-recall --model haiku`

### LLMs locaux (ollama)

Même test avec des modèles locaux — injection de contexte pure, sans besoin d'appels d'outils.

```
Model               Params   No ICM   With ICM     Delta
─────────────────────────────────────────────────────────
qwen2.5:14b           14B       4%       97%       +93%
mistral:7b             7B       4%       93%       +89%
llama3.1:8b            8B       4%       93%       +89%
qwen2.5:7b             7B       4%       90%       +86%
phi4:14b              14B       6%       79%       +73%
llama3.2:3b            3B       0%       76%       +76%
gemma2:9b              9B       4%       76%       +72%
qwen2.5:3b             3B       2%       58%       +56%
─────────────────────────────────────────────────────────
```

`scripts/bench-ollama.sh qwen2.5:14b`

### Protocole de test

Tous les benchmarks utilisent de **vrais appels API** — pas de mocks, pas de réponses simulées, pas de réponses en cache.

- **Benchmark agent** : crée un vrai projet Rust dans un répertoire temporaire. Lance N sessions avec `claude -p --output-format json`. Sans ICM : configuration MCP vide. Avec ICM : vrai serveur MCP + auto-extraction + injection de contexte.
- **Rétention de connaissance** : utilise un document technique fictif (le « Protocole Meridian »). Évalue les réponses par correspondance de mots-clés avec les faits attendus. Timeout de 120s par invocation.
- **Isolation** : chaque exécution utilise son propre répertoire temporaire et une nouvelle base SQLite. Pas de persistance entre sessions.

### Mémoire unifiée multi-agents

Les 17 outils partagent la même base SQLite. Un souvenir stocké par Claude est instantanément disponible pour Gemini, Codex, Copilot, Cursor et tous les autres outils.

```
ICM Multi-Agent Efficiency Benchmark (10 seeded facts, 5 CLI agents)
╔══════════════╦═══════╦══════════╦════════╦═══════════╦═══════╗
║ Agent        ║ Facts ║ Accuracy ║ Detail ║ Latency   ║ Score ║
╠══════════════╬═══════╬══════════╬════════╬═══════════╬═══════╣
║ Claude Code  ║ 10/10 ║   100%   ║  5/5   ║    ~15s   ║   99  ║
║ Gemini CLI   ║ 10/10 ║   100%   ║  5/5   ║    ~33s   ║   94  ║
║ Copilot CLI  ║ 10/10 ║   100%   ║  5/5   ║    ~10s   ║  100  ║
║ Cursor Agent ║ 10/10 ║   100%   ║  5/5   ║    ~16s   ║   99  ║
║ Aider        ║ 10/10 ║   100%   ║  5/5   ║     ~5s   ║  100  ║
╠══════════════╬═══════╬══════════╬════════╬═══════════╬═══════╣
║ AVERAGE      ║       ║          ║        ║           ║   98  ║
╚══════════════╩═══════╩══════════╩════════╩═══════════╩═══════╝
```

Score = 60% précision de rappel + 30% détail des faits + 10% vitesse. **98% d'efficacité multi-agents.**

## Pourquoi ICM

| Capacité | ICM | Mem0 | Engram | AgentMemory |
|----------|:---:|:----:|:------:|:-----------:|
| Support d'outils | **17** | SDK uniquement | ~6-8 | ~10 |
| Configuration en une commande | `icm init` | SDK manuel | manuel | manuel |
| Hooks (auto-rappel au démarrage) | 5 outils | aucun | via MCP | 1 outil |
| Recherche hybride (FTS5 + vector) | 30/70 pondéré | vector uniquement | FTS5 uniquement | FTS5+vector |
| Embeddings multilingues | 100+ langues (768d) | dépend | aucun | Anglais 384d |
| Graphe de connaissance | Système Memoir | aucun | aucun | aucun |
| Décroissance temporelle + consolidation | sensible aux accès | aucun | basique | basique |
| Dashboard TUI | `icm dashboard` | aucun | oui | visualiseur web |
| Auto-extraction depuis la sortie des outils | 3 couches, zéro LLM | aucun | aucun | aucun |
| Boucle de feedback/correction | `icm_feedback_*` | aucun | aucun | aucun |
| Runtime | Binaire Rust unique | Python | Go | Node.js |
| Local-first, zéro dépendance | Fichier SQLite | cloud-first | SQLite | SQLite |
| Précision de rappel multi-agents | **98%** | N/A | N/A | 95.2% |

## Documentation

| Document | Description |
|----------|-------------|
| [Guide d'intégration](docs/integrations.md) | Configuration pour les 17 outils : Claude Code, Copilot, Cursor, Windsurf, Zed, Amp, etc. |
| [Architecture technique](docs/architecture.md) | Structure des crates, pipeline de recherche, modèle de décroissance, intégration sqlite-vec, tests |
| [Guide utilisateur](docs/guide.md) | Installation, organisation des topics, consolidation, extraction, dépannage |
| [Vue d'ensemble produit](docs/product.md) | Cas d'usage, benchmarks, comparaison avec les alternatives |

## Licence

[Source disponible](LICENSE) — Gratuit pour les particuliers et les équipes de 20 personnes ou moins. Licence entreprise requise pour les organisations plus grandes. Contact : contact@rtk-ai.app
