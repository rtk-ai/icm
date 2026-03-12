# ICM -- Documentation fonctionnelle complete

## Table des matieres

- [Vue d'ensemble](#vue-densemble)
- [Commandes CLI (29)](#commandes-cli-29)
  - [Memoires (episodiques)](#memoires-episodiques)
  - [Memoir (graphes de connaissances)](#memoir-graphes-de-connaissances)
  - [Administration et maintenance](#administration-et-maintenance)
  - [Configuration et setup](#configuration-et-setup)
  - [Benchmarks](#benchmarks)
- [Outils MCP (21)](#outils-mcp-21)
  - [Outils Memory (9)](#outils-memory-9)
  - [Outils Memoir (9)](#outils-memoir-9)
  - [Outils Feedback (3)](#outils-feedback-3)
- [Memory vs Memoir : quand utiliser quoi](#memory-vs-memoir--quand-utiliser-quoi)
- [Workflow multi-session](#workflow-multi-session)
- [Organisation des topics](#organisation-des-topics)
- [Guide de consolidation](#guide-de-consolidation)
- [Guide des niveaux d'importance](#guide-des-niveaux-dimportance)
- [Modele de decay explique](#modele-de-decay-explique)
- [Configuration complete](#configuration-complete)

---

## Vue d'ensemble

ICM (Infinite Context Memory) offre deux systemes de memoire complementaires :

- **Memories** (episodiques) : stockage temporel avec decay. Les souvenirs importants persistent, les triviaux s'effacent naturellement. Organises par **topic**.
- **Memoirs** (semantiques) : graphes de connaissances permanents. Les concepts sont raffines, jamais declines. Organises par **memoir** contenant des **concepts** relies par des **relations typees**.

Le CLI offre 29 commandes. Le serveur MCP expose 18 outils. Les deux accedent a la meme base SQLite.

---

## Commandes CLI (29)

Option globale disponible sur toutes les commandes :

```
--db <chemin>    Chemin vers la base SQLite (defaut : chemin plateforme)
```

### Memoires (episodiques)

#### `icm store` -- Stocker un souvenir

```
icm store -t <topic> -c <contenu> [-i <importance>] [-k <mots-cles>] [-r <extrait-brut>]
```

| Option | Court | Obligatoire | Defaut | Description |
|--------|-------|-------------|--------|-------------|
| `--topic` | `-t` | oui | -- | Categorie/namespace du souvenir |
| `--content` | `-c` | oui | -- | Contenu a memoriser |
| `--importance` | `-i` | non | `medium` | `critical`, `high`, `medium`, `low` |
| `--keywords` | `-k` | non | -- | Mots-cles separes par virgules |
| `--raw` | `-r` | non | -- | Extrait verbatim (code, message d'erreur) |

**Exemples :**

```bash
# Decision d'architecture
icm store -t "decisions-api" -c "Choix de REST plutot que GraphQL pour la v1" -i high

# Erreur resolue avec mots-cles
icm store -t "erreurs" -c "CORS fixe en ajoutant le header Origin dans nginx" -i medium -k "cors,nginx,fix"

# Fait critique (jamais oublie)
icm store -t "infra" -c "La DB de prod est sur le port 5433, pas 5432" -i critical

# Avec extrait brut
icm store -t "erreurs" -c "Erreur de compilation corrigee" -r "error[E0382]: borrow of moved value"
```

Si les embeddings sont actives, le souvenir est automatiquement vectorise au stockage.

---

#### `icm recall` -- Rechercher des souvenirs

```
icm recall <requete> [-t <topic>] [-l <limite>] [-k <mot-cle>]
```

| Option | Court | Obligatoire | Defaut | Description |
|--------|-------|-------------|--------|-------------|
| `query` | -- | oui (positionnel) | -- | Requete en langage naturel |
| `--topic` | `-t` | non | -- | Filtrer par topic |
| `--limit` | `-l` | non | `5` | Nombre max de resultats |
| `--keyword` | `-k` | non | -- | Filtrer par mot-cle exact |

**Exemples :**

```bash
# Recherche large
icm recall "choix de base de donnees"

# Filtree par topic
icm recall "authentification" --topic "decisions-api" --limit 10

# Filtree par mot-cle
icm recall "erreur nginx" --keyword "cors"
```

**Comportement automatique :**
- Applique le decay si >24h depuis la derniere execution
- Met a jour le compteur d'acces de chaque resultat
- Pipeline de recherche : hybrid (si embeddings) -> FTS5 -> keyword LIKE

---

#### `icm list` -- Lister les souvenirs

```
icm list [-t <topic>] [-a] [-s <tri>]
```

| Option | Court | Obligatoire | Defaut | Description |
|--------|-------|-------------|--------|-------------|
| `--topic` | `-t` | non | -- | Filtrer par topic |
| `--all` | `-a` | non | false | Lister tous les souvenirs |
| `--sort` | `-s` | non | `weight` | Tri : `weight`, `created`, `accessed` |

**Exemples :**

```bash
# Lister un topic
icm list -t "decisions-api"

# Tous les souvenirs tries par date de creation
icm list --all --sort created

# Tries par dernier acces
icm list -t "erreurs" --sort accessed
```

---

#### `icm forget` -- Supprimer un souvenir

```
icm forget <id>
```

| Argument | Obligatoire | Description |
|----------|-------------|-------------|
| `id` | oui (positionnel) | ID ULID du souvenir a supprimer |

**Exemple :**

```bash
icm forget 01HWXYZ123456789ABCDEF
```

---

#### `icm extract` -- Extraction de faits (zero cout LLM)

```
icm extract [-p <projet>] [-t <texte>] [--dry-run]
```

| Option | Court | Obligatoire | Defaut | Description |
|--------|-------|-------------|--------|-------------|
| `--project` | `-p` | non | `project` | Nom du projet pour le namespace des topics |
| `--text` | `-t` | non | stdin | Texte source (lit stdin si omis) |
| `--dry-run` | -- | non | false | Afficher sans stocker |

**Exemples :**

```bash
# Depuis stdin
echo "Le parser utilise l'algorithme de Pratt" | icm extract -p mon-projet

# Depuis un fichier
cat session-log.txt | icm extract -p backend

# Apercu sans stockage
echo "Migre de MySQL vers PostgreSQL pour le support JSONB" | icm extract -p api --dry-run
```

**Signaux detectes :**

| Signal | Mots-cles | Score |
|--------|-----------|-------|
| Architecture | `uses`, `architecture`, `pattern`, `algorithm` | +3 |
| Erreur/Fix | `error`, `fixed`, `bug`, `workaround` | +3 |
| Decision | `decided`, `chose`, `prefer`, `switched to` | +4 |
| Config | `configured`, `setup`, `installed`, `enabled` | +2 |
| Dev | `commit`, `deploy`, `migrate`, `refactor` | +2 |

---

#### `icm recall-context` -- Injection de contexte

```
icm recall-context <requete> [-l <limite>]
```

| Option | Court | Obligatoire | Defaut | Description |
|--------|-------|-------------|--------|-------------|
| `query` | -- | oui (positionnel) | -- | Requete de recherche |
| `--limit` | `-l` | non | `10` | Nombre max de souvenirs |

Retourne un bloc formate pret pour l'injection dans un prompt. Utilise par le hook SessionStart pour le chargement automatique du contexte.

```bash
icm recall-context "mon-projet backend API"
icm recall-context "authentification" --limit 20
```

---

### Memoir (graphes de connaissances)

#### `icm memoir create` -- Creer un memoir

```
icm memoir create -n <nom> [-d <description>]
```

| Option | Court | Obligatoire | Defaut | Description |
|--------|-------|-------------|--------|-------------|
| `--name` | `-n` | oui | -- | Nom unique du memoir |
| `--description` | `-d` | non | `""` | Description du memoir |

```bash
icm memoir create -n "archi-backend" -d "Decisions d'architecture backend"
```

---

#### `icm memoir list` -- Lister les memoirs

```
icm memoir list
```

Aucun argument. Affiche tous les memoirs avec leur nombre de concepts.

---

#### `icm memoir show` -- Afficher un memoir

```
icm memoir show <nom>
```

| Argument | Obligatoire | Description |
|----------|-------------|-------------|
| `name` | oui (positionnel) | Nom du memoir |

```bash
icm memoir show archi-backend
```

Affiche les stats, labels utilises, et tous les concepts du memoir.

---

#### `icm memoir delete` -- Supprimer un memoir

```
icm memoir delete <nom>
```

| Argument | Obligatoire | Description |
|----------|-------------|-------------|
| `name` | oui (positionnel) | Nom du memoir |

**Attention :** Supprime en cascade tous les concepts et liens du memoir.

```bash
icm memoir delete ancien-projet
```

---

#### `icm memoir add-concept` -- Ajouter un concept

```
icm memoir add-concept -m <memoir> -n <nom> -d <definition> [-l <labels>]
```

| Option | Court | Obligatoire | Defaut | Description |
|--------|-------|-------------|--------|-------------|
| `--memoir` | `-m` | oui | -- | Nom du memoir |
| `--name` | `-n` | oui | -- | Nom du concept (unique dans le memoir) |
| `--definition` | `-d` | oui | -- | Definition dense du concept |
| `--labels` | `-l` | non | -- | Labels comma-separated (`namespace:valeur` ou tag simple) |

```bash
icm memoir add-concept -m "archi-backend" -n "user-service" \
  -d "Gere l'inscription, l'authentification (JWT + OAuth2) et les profils" \
  -l "domain:auth,type:microservice"

icm memoir add-concept -m "archi-backend" -n "postgres" \
  -d "Base de donnees principale pour users et transactions" \
  -l "type:database"
```

---

#### `icm memoir refine` -- Raffiner un concept

```
icm memoir refine -m <memoir> -n <nom> -d <nouvelle-definition>
```

| Option | Court | Obligatoire | Description |
|--------|-------|-------------|-------------|
| `--memoir` | `-m` | oui | Nom du memoir |
| `--name` | `-n` | oui | Nom du concept existant |
| `--definition` | `-d` | oui | Nouvelle definition (remplace l'ancienne) |

Incremente la revision et augmente la confiance du concept.

```bash
icm memoir refine -m "archi-backend" -n "user-service" \
  -d "Gere inscription, auth (JWT + OAuth2), profils et 2FA. Rate limiting via Redis."
```

---

#### `icm memoir search` -- Rechercher dans un memoir

```
icm memoir search -m <memoir> <requete> [-L <label>] [-l <limite>]
```

| Option | Court | Obligatoire | Defaut | Description |
|--------|-------|-------------|--------|-------------|
| `--memoir` | `-m` | oui | -- | Nom du memoir |
| `query` | -- | oui (positionnel) | -- | Requete de recherche |
| `--label` | `-L` | non | -- | Filtrer par label (ex: `domain:auth`) |
| `--limit` | `-l` | non | `10` | Nombre max de resultats |

```bash
icm memoir search -m "archi-backend" "authentification"
icm memoir search -m "archi-backend" "service" --label "domain:auth"
```

---

#### `icm memoir search-all` -- Rechercher dans tous les memoirs

```
icm memoir search-all <requete> [-l <limite>]
```

| Option | Court | Obligatoire | Defaut | Description |
|--------|-------|-------------|--------|-------------|
| `query` | -- | oui (positionnel) | -- | Requete de recherche |
| `--limit` | `-l` | non | `10` | Nombre max de resultats |

```bash
icm memoir search-all "database"
```

---

#### `icm memoir link` -- Lier deux concepts

```
icm memoir link -m <memoir> --from <source> --to <cible> -r <relation>
```

| Option | Court | Obligatoire | Description |
|--------|-------|-------------|-------------|
| `--memoir` | `-m` | oui | Nom du memoir |
| `--from` | -- | oui | Nom du concept source |
| `--to` | -- | oui | Nom du concept cible |
| `--relation` | `-r` | oui | Type de relation (voir ci-dessous) |

**9 types de relations :**

| Relation | Signification | Exemple |
|----------|---------------|---------|
| `part-of` | A fait partie de B | `cache-layer` part-of `api-gateway` |
| `depends-on` | A necessite B | `user-service` depends-on `postgres` |
| `related-to` | A est associe a B | `auth` related-to `session-mgmt` |
| `contradicts` | A contredit B | `rest-api` contradicts `graphql-api` |
| `refines` | A precise B | `jwt-auth-v2` refines `jwt-auth` |
| `alternative-to` | A peut remplacer B | `redis` alternative-to `memcached` |
| `caused-by` | A est cause par B | `perf-issue` caused-by `n-plus-one` |
| `instance-of` | A est une instance de B | `user-db` instance-of `postgres` |
| `superseded-by` | A est remplace par B | `mysql-setup` superseded-by `postgres-setup` |

```bash
icm memoir link -m "archi-backend" --from "user-service" --to "postgres" -r depends-on
icm memoir link -m "archi-backend" --from "user-service" --to "redis" -r depends-on
```

**Utiliser `superseded-by`** pour marquer les faits obsoletes au lieu de les supprimer -- l'historique a de la valeur.

---

#### `icm memoir inspect` -- Inspecter un concept et son voisinage

```
icm memoir inspect -m <memoir> <nom> [-D <profondeur>]
```

| Option | Court | Obligatoire | Defaut | Description |
|--------|-------|-------------|--------|-------------|
| `--memoir` | `-m` | oui | -- | Nom du memoir |
| `name` | -- | oui (positionnel) | -- | Nom du concept |
| `--depth` | `-D` | non | `1` | Profondeur BFS pour l'exploration du graphe |

```bash
# Voisins directs
icm memoir inspect -m "archi-backend" "user-service"

# Voisinage a 2 sauts
icm memoir inspect -m "archi-backend" "user-service" -D 2
```

---

#### `icm memoir distill` -- Distiller des souvenirs en concepts

```
icm memoir distill --from-topic <topic> --into <memoir>
```

| Option | Obligatoire | Description |
|--------|-------------|-------------|
| `--from-topic` | oui | Topic source (memories) |
| `--into` | oui | Memoir cible (doit exister) |

Transforme les souvenirs d'un topic en concepts dans un memoir. Le premier mot-cle devient le nom du concept. Si un concept du meme nom existe deja, la definition est fusionnee (refine).

```bash
# Creer le memoir d'abord
icm memoir create -n "archi-v2" -d "Architecture v2"

# Distiller les decisions dans le memoir
icm memoir distill --from-topic "decisions-api" --into "archi-v2"
```

---

### Administration et maintenance

#### `icm topics` -- Lister les topics

```
icm topics
```

Aucun argument. Affiche tous les topics avec le nombre d'entrees.

```
Topic                          Count
----------------------------------------
decisions-api                  12
erreurs-resolues               8
preferences                    3
```

---

#### `icm stats` -- Statistiques globales

```
icm stats
```

```
Memories:  23
Topics:    3
Avg weight: 0.847
Oldest:    2024-01-15 09:30
Newest:    2024-03-05 14:22
```

---

#### `icm decay` -- Appliquer le decay manuellement

```
icm decay [-f <facteur>]
```

| Option | Court | Obligatoire | Defaut | Description |
|--------|-------|-------------|--------|-------------|
| `--factor` | `-f` | non | `0.95` | Facteur de decay (0.0 a 1.0) |

```bash
# Decay standard
icm decay

# Decay agressif
icm decay --factor 0.8
```

Normalement, le decay s'execute automatiquement lors d'un `recall` si >24h depuis la derniere execution.

---

#### `icm prune` -- Supprimer les souvenirs a faible poids

```
icm prune [-t <seuil>] [--dry-run]
```

| Option | Court | Obligatoire | Defaut | Description |
|--------|-------|-------------|--------|-------------|
| `--threshold` | `-t` | non | `0.1` | Seuil de poids (en dessous = supprime) |
| `--dry-run` | -- | non | false | Apercu sans supprimer |

**Important :** Les souvenirs `critical` et `high` ne sont jamais prunes, quel que soit leur poids.

```bash
# Apercu
icm prune --threshold 0.2 --dry-run

# Execution
icm prune --threshold 0.1
```

---

#### `icm consolidate` -- Consolider un topic

```
icm consolidate -t <topic> [--keep-originals]
```

| Option | Court | Obligatoire | Defaut | Description |
|--------|-------|-------------|--------|-------------|
| `--topic` | `-t` | oui | -- | Topic a consolider |
| `--keep-originals` | -- | non | false | Garder les originaux apres consolidation |

La consolidation fusionne tous les souvenirs d'un topic en un seul resume. L'importance du resume consolide est la plus haute des originaux. Les mots-cles sont fusionnes.

```bash
# Remplacer tous les souvenirs par un resume
icm consolidate --topic "erreurs-resolues"

# Garder les originaux
icm consolidate --topic "erreurs-resolues" --keep-originals
```

---

#### `icm embed` -- Generer les embeddings

```
icm embed [-t <topic>] [--force] [-b <taille-batch>]
```

| Option | Court | Obligatoire | Defaut | Description |
|--------|-------|-------------|--------|-------------|
| `--topic` | `-t` | non | -- | Limiter a un topic |
| `--force` | -- | non | false | Re-embedder meme ceux qui ont deja un embedding |
| `--batch-size` | `-b` | non | `32` | Taille du batch d'embedding |

Necessite le feature `embeddings`. Si compile sans, la commande echoue avec un message explicite.

```bash
# Tous les souvenirs sans embedding
icm embed

# Re-embedder tout (apres changement de modele)
icm embed --force

# Un seul topic
icm embed --topic "decisions-api"
```

---

### Configuration et setup

#### `icm init` -- Configuration automatique

```
icm init [-m <mode>]
```

| Option | Court | Obligatoire | Defaut | Description |
|--------|-------|-------------|--------|-------------|
| `--mode` | `-m` | non | `mcp` | Mode : `mcp`, `cli`, `skill`, `hook`, `all` |

**Modes :**

| Mode | Action | Description |
|------|--------|-------------|
| `mcp` | Configure le serveur MCP | Auto-detecte et configure 14 outils IA |
| `cli` | Injecte dans CLAUDE.md | Ajoute les instructions `icm store`/`icm recall` |
| `skill` | Installe les slash commands | `/recall`, `/remember` pour Claude Code, `.mdc` pour Cursor, etc. |
| `hook` | Installe le hook PostToolUse | Extraction automatique apres chaque outil |
| `all` | Tout ci-dessus | Configure MCP + CLI + Skills + Hook |

**14 outils supportes (mode MCP) :**

| Outil | Fichier de config |
|-------|-------------------|
| Claude Code | `~/.claude.json` |
| Claude Desktop | `~/Library/.../claude_desktop_config.json` |
| Cursor | `~/.cursor/mcp.json` |
| Windsurf | `~/.codeium/windsurf/mcp_config.json` |
| VS Code / Copilot | `~/Library/.../Code/User/mcp.json` |
| Gemini Code Assist | `~/.gemini/settings.json` |
| Zed | `~/.zed/settings.json` |
| Amp | `~/.config/amp/settings.json` |
| Amazon Q | `~/.aws/amazonq/mcp.json` |
| Cline | VS Code globalStorage |
| Roo Code | VS Code globalStorage |
| Kilo Code | VS Code globalStorage |
| OpenAI Codex CLI | `~/.codex/config.toml` |
| OpenCode | `~/.config/opencode/opencode.json` |

```bash
# Setup standard
icm init

# Tout installer
icm init --mode all

# Juste les slash commands
icm init --mode skill
```

---

#### `icm config` -- Afficher la configuration

```
icm config
```

Aucun argument. Affiche la configuration active avec toutes les sections.

```
Config: ~/.config/icm/config.toml (loaded)

[store]
  path = (default platform path)

[memory]
  default_importance = medium
  decay_rate = 0.95
  prune_threshold = 0.1

[embeddings]
  model = intfloat/multilingual-e5-base

[extraction]
  enabled = true
  min_score = 3.0
  max_facts = 10

[recall]
  enabled = true
  limit = 15

[mcp]
  transport = stdio
  compact = true
```

---

#### `icm serve` -- Lancer le serveur MCP

```
icm serve [--compact]
```

| Option | Obligatoire | Defaut | Description |
|--------|-------------|--------|-------------|
| `--compact` | non | false | Reponses courtes (~40% de tokens en moins) |

Le flag `--compact` prend precedence. Sinon, la valeur de `config.toml` (`[mcp] compact = true`) est utilisee.

```bash
# Standard
icm serve

# Mode compact (economise ~40% de tokens)
icm serve --compact

# Test rapide
echo '{"jsonrpc":"2.0","id":1,"method":"initialize"}' | icm serve
```

---

### Benchmarks

#### `icm bench` -- Benchmark de performance stockage

```
icm bench [-c <nombre>]
```

| Option | Court | Obligatoire | Defaut | Description |
|--------|-------|-------------|--------|-------------|
| `--count` | `-c` | non | `1000` | Nombre de souvenirs a generer |

```bash
icm bench --count 1000
```

Resultat type :
```
Store (no embeddings)      1000 ops      34.2 ms      34.2 us/op
Store (with embeddings)    1000 ops      51.6 ms      51.6 us/op
FTS5 search                 100 ops       4.7 ms      46.6 us/op
Vector search (KNN)         100 ops      59.0 ms     590.0 us/op
Hybrid search               100 ops      95.1 ms     951.1 us/op
Decay (batch)                 1 ops       5.8 ms       5.8 ms/op
```

---

#### `icm bench-recall` -- Benchmark de retention de connaissances

```
icm bench-recall [-m <modele>] [-r <runs>] [-v]
```

| Option | Court | Obligatoire | Defaut | Description |
|--------|-------|-------------|--------|-------------|
| `--model` | `-m` | non | `sonnet` | Modele a utiliser |
| `--runs` | `-r` | non | `1` | Nombre de runs a moyenner |
| `--verbose` | `-v` | non | false | Afficher le contexte injecte |

Mesure la capacite de l'agent a rappeler des faits d'un document technique entre sessions. Utilise de vrais appels API.

```bash
icm bench-recall --model haiku --runs 5
```

---

#### `icm bench-agent` -- Benchmark d'efficacite agent

```
icm bench-agent [-s <sessions>] [-m <modele>] [-r <runs>] [-v]
```

| Option | Court | Obligatoire | Defaut | Description |
|--------|-------|-------------|--------|-------------|
| `--sessions` | `-s` | non | `10` | Nombre de sessions par mode |
| `--model` | `-m` | non | `sonnet` | Modele a utiliser |
| `--runs` | `-r` | non | `1` | Nombre de runs a moyenner |
| `--verbose` | `-v` | non | false | Afficher les faits extraits et le contexte |

Compare les tours, tokens et couts avec et sans ICM sur un vrai projet Rust.

```bash
icm bench-agent --sessions 10 --model haiku --runs 3
```

---

## Outils MCP (21)

Le serveur MCP expose 18 outils via le protocole JSON-RPC 2.0 sur stdio. Tous les outils sont appeles par l'agent IA (Claude, Cursor, etc.) de maniere transparente.

### Outils Memory (9)

#### `icm_memory_store` -- Stocker un souvenir

**Parametres :**

| Parametre | Type | Obligatoire | Defaut | Description |
|-----------|------|-------------|--------|-------------|
| `topic` | string | oui | -- | Categorie (ex: `projet-kexa`, `decisions-architecture`) |
| `content` | string | oui | -- | Information a memoriser |
| `importance` | string (enum) | non | `medium` | `critical`, `high`, `medium`, `low` |
| `keywords` | string[] | non | -- | Mots-cles pour ameliorer la recherche |
| `raw_excerpt` | string | non | -- | Extrait verbatim (code, message d'erreur) |

**Comportements automatiques :**
- **Auto-dedup** : si un souvenir similaire a >85% existe dans le meme topic, il est mis a jour au lieu de creer un doublon
- **Auto-embed** : si l'embedder est disponible, le souvenir est vectorise automatiquement
- **Alerte consolidation** : si le topic depasse 7 entrees, un avertissement est ajoute a la reponse

**Exemple de requete :**
```json
{
  "topic": "decisions-api",
  "content": "Utilisation de JWT pour l'authentification API",
  "importance": "high",
  "keywords": ["jwt", "auth", "api"]
}
```

**Exemple de reponse (mode normal) :**
```
Stored memory: 01HWXYZ123456789ABCDEF
[Note: topic 'decisions-api' has 8 entries. Consider consolidating.]
```

**Exemple de reponse (mode compact) :**
```
ok:01HWXYZ123456789ABCDEF
```

---

#### `icm_memory_recall` -- Rechercher des souvenirs

**Parametres :**

| Parametre | Type | Obligatoire | Defaut | Description |
|-----------|------|-------------|--------|-------------|
| `query` | string | oui | -- | Requete en langage naturel |
| `topic` | string | non | -- | Filtrer par topic |
| `limit` | integer | non | `5` | Max resultats (1-20) |
| `keyword` | string | non | -- | Filtrer par mot-cle exact |

**Comportements automatiques :**
- **Auto-decay** : applique le decay si >24h depuis la derniere execution
- **Mise a jour acces** : incremente le compteur d'acces de chaque resultat

**Exemple de requete :**
```json
{
  "query": "choix base de donnees",
  "topic": "decisions-api",
  "limit": 3
}
```

**Exemple de reponse (mode normal) :**
```
--- 01HWXYZ123456789ABCDEF ---
  topic:      decisions-api
  importance: high
  weight:     0.950
  summary:    Utilisation de PostgreSQL pour le support JSONB
  keywords:   postgres, jsonb, database
```

**Exemple de reponse (mode compact) :**
```
[decisions-api] Utilisation de PostgreSQL pour le support JSONB
```

---

#### `icm_memory_update` -- Mettre a jour un souvenir

**Parametres :**

| Parametre | Type | Obligatoire | Defaut | Description |
|-----------|------|-------------|--------|-------------|
| `id` | string | oui | -- | ID du souvenir a mettre a jour |
| `content` | string | oui | -- | Nouveau contenu (remplace le summary) |
| `importance` | string (enum) | non | (conserve) | Nouvelle importance |
| `keywords` | string[] | non | (conserve) | Nouveaux mots-cles |

**Exemple de requete :**
```json
{
  "id": "01HWXYZ123456789ABCDEF",
  "content": "PostgreSQL pour JSONB + PostGIS pour les donnees geo",
  "importance": "critical"
}
```

---

#### `icm_memory_forget` -- Supprimer un souvenir

**Parametres :**

| Parametre | Type | Obligatoire | Description |
|-----------|------|-------------|-------------|
| `id` | string | oui | ID du souvenir a supprimer |

**Exemple :**
```json
{ "id": "01HWXYZ123456789ABCDEF" }
```

---

#### `icm_memory_consolidate` -- Consolider un topic

**Parametres :**

| Parametre | Type | Obligatoire | Description |
|-----------|------|-------------|-------------|
| `topic` | string | oui | Topic a consolider |
| `summary` | string | oui | Resume consolide (remplace tous les souvenirs du topic) |

**Important :** Contrairement au CLI, le MCP necessite que l'agent fournisse le resume. L'agent doit d'abord rappeler les souvenirs du topic, puis les synthetiser.

**Exemple :**
```json
{
  "topic": "erreurs-resolues",
  "summary": "CORS fixe via nginx header. Memory leak fixe en fermeant les connexions DB. Rate limiting ajoute sur /api/auth."
}
```

---

#### `icm_memory_list_topics` -- Lister les topics

**Parametres :** Aucun

**Exemple de reponse :**
```
decisions-api: 5
erreurs-resolues: 12
preferences: 3
```

---

#### `icm_memory_stats` -- Statistiques globales

**Parametres :** Aucun

**Exemple de reponse :**
```
Memories: 20, Topics: 3, Avg weight: 0.847, Oldest: 2024-01-15 09:30, Newest: 2024-03-05 14:22
```

---

#### `icm_memory_health` -- Audit d'hygiene

**Parametres :**

| Parametre | Type | Obligatoire | Defaut | Description |
|-----------|------|-------------|--------|-------------|
| `topic` | string | non | (tous) | Topic specifique a auditer |

Rapporte par topic : nombre d'entrees, poids moyen, entrees perimees, besoin de consolidation.

**Exemple de reponse :**
```
decisions-api: 5 entries, avg_weight=0.92, stale=0, needs_consolidation=false
erreurs-resolues: 12 entries, avg_weight=0.65, stale=3, needs_consolidation=true
```

---

#### `icm_memory_embed_all` -- Backfill des embeddings

**Parametres :**

| Parametre | Type | Obligatoire | Defaut | Description |
|-----------|------|-------------|--------|-------------|
| `topic` | string | non | (tous) | Limiter a un topic |

Disponible uniquement si le feature `embeddings` est active. Genere les vecteurs pour les souvenirs qui n'en ont pas encore.

---

### Outils Memoir (9)

#### `icm_memoir_create` -- Creer un memoir

**Parametres :**

| Parametre | Type | Obligatoire | Description |
|-----------|------|-------------|-------------|
| `name` | string | oui | Nom unique du memoir |
| `description` | string | non | Description |

**Exemple :**
```json
{ "name": "system-architecture", "description": "Design decisions and component relationships" }
```

---

#### `icm_memoir_list` -- Lister les memoirs

**Parametres :** Aucun

Retourne tous les memoirs avec leurs nombres de concepts.

---

#### `icm_memoir_show` -- Afficher un memoir

**Parametres :**

| Parametre | Type | Obligatoire | Description |
|-----------|------|-------------|-------------|
| `name` | string | oui | Nom du memoir |

Retourne les stats, labels, et tous les concepts du memoir.

---

#### `icm_memoir_add_concept` -- Ajouter un concept

**Parametres :**

| Parametre | Type | Obligatoire | Description |
|-----------|------|-------------|-------------|
| `memoir` | string | oui | Nom du memoir |
| `name` | string | oui | Nom du concept (unique dans le memoir) |
| `definition` | string | oui | Description dense du concept |
| `labels` | string | non | Labels comma-separated (ex: `domain:arch,type:decision`) |

**Exemple :**
```json
{
  "memoir": "system-architecture",
  "name": "auth-service",
  "definition": "Gere JWT et OAuth2 flows",
  "labels": "domain:auth,type:service"
}
```

---

#### `icm_memoir_refine` -- Raffiner un concept

**Parametres :**

| Parametre | Type | Obligatoire | Description |
|-----------|------|-------------|-------------|
| `memoir` | string | oui | Nom du memoir |
| `name` | string | oui | Nom du concept |
| `definition` | string | oui | Nouvelle definition (remplace l'ancienne) |

Incremente la revision et augmente la confiance.

---

#### `icm_memoir_search` -- Rechercher dans un memoir

**Parametres :**

| Parametre | Type | Obligatoire | Defaut | Description |
|-----------|------|-------------|--------|-------------|
| `memoir` | string | oui | -- | Nom du memoir |
| `query` | string | oui | -- | Requete de recherche |
| `label` | string | non | -- | Filtrer par label (ex: `domain:tech`) |
| `limit` | integer | non | `10` | Max resultats |

---

#### `icm_memoir_search_all` -- Rechercher dans tous les memoirs

**Parametres :**

| Parametre | Type | Obligatoire | Defaut | Description |
|-----------|------|-------------|--------|-------------|
| `query` | string | oui | -- | Requete de recherche |
| `limit` | integer | non | `10` | Max resultats |

---

#### `icm_memoir_link` -- Lier deux concepts

**Parametres :**

| Parametre | Type | Obligatoire | Description |
|-----------|------|-------------|-------------|
| `memoir` | string | oui | Nom du memoir |
| `from` | string | oui | Nom du concept source |
| `to` | string | oui | Nom du concept cible |
| `relation` | string (enum) | oui | Type de relation |

**Valeurs de `relation` :**
`part_of`, `depends_on`, `related_to`, `contradicts`, `refines`, `alternative_to`, `caused_by`, `instance_of`, `superseded_by`

**Exemple :**
```json
{
  "memoir": "system-architecture",
  "from": "api-gateway",
  "to": "auth-service",
  "relation": "depends_on"
}
```

---

#### `icm_memoir_inspect` -- Inspecter le voisinage d'un concept

**Parametres :**

| Parametre | Type | Obligatoire | Defaut | Description |
|-----------|------|-------------|--------|-------------|
| `memoir` | string | oui | -- | Nom du memoir |
| `name` | string | oui | -- | Nom du concept |
| `depth` | integer | non | `1` | Profondeur BFS |

**Exemple :**
```json
{
  "memoir": "system-architecture",
  "name": "auth-service",
  "depth": 2
}
```

Retourne le concept et tous les concepts atteignables en N sauts, avec les liens entre eux.

---

### Outils Feedback (3)

Les outils de feedback permettent l'apprentissage en boucle fermee : quand une prediction AI est fausse, on enregistre la correction pour ameliorer les predictions futures.

#### `icm_feedback_record` -- Enregistrer une correction

**Parametres :**

| Parametre | Type | Obligatoire | Description |
|-----------|------|-------------|-------------|
| `topic` | string | oui | Categorie/namespace (ex: `triage-owner/repo`, `pr-analysis`) |
| `context` | string | oui | Situation / input qui a mene a la prediction |
| `predicted` | string | oui | Ce que l'AI a predit ou fait |
| `corrected` | string | oui | La bonne reponse/action |
| `reason` | string | non | Pourquoi la correction a ete faite |
| `source` | string | non | Quel outil/pipeline a genere la prediction |

**Exemple :**
```json
{
  "topic": "triage-myorg/myrepo",
  "context": "Issue: 'App crashes when clicking save button'",
  "predicted": "feature",
  "corrected": "bug",
  "reason": "The issue describes a crash, which is a bug not a feature request"
}
```

En mode compact, retourne uniquement l'ID. En mode normal, retourne l'objet complet.

#### `icm_feedback_search` -- Rechercher des corrections passees

**Parametres :**

| Parametre | Type | Obligatoire | Description |
|-----------|------|-------------|-------------|
| `query` | string | oui | Requete de recherche |
| `topic` | string | non | Filtrer par topic |
| `limit` | integer | non | Nombre max de resultats (defaut: 5, max: 20) |

**Exemple :**
```json
{ "query": "crash bug classification", "topic": "triage-myorg/myrepo", "limit": 5 }
```

Utilise la recherche full-text (FTS5) sur les champs context, predicted, corrected et reason.

#### `icm_feedback_stats` -- Statistiques de feedback

**Parametres :** aucun

Retourne :
- `total` : nombre total de corrections enregistrees
- `by_topic` : ventilation par topic
- `most_applied` : les corrections les plus souvent referencees

---

## Memory vs Memoir : quand utiliser quoi

### Comparaison rapide

| Aspect | Memory (episodique) | Memoir (semantique) |
|--------|--------------------|--------------------|
| **Duree de vie** | Temporaire (decay) | Permanente |
| **Organisation** | Par topic (plat) | Par memoir (graphe) |
| **Granularite** | Un fait, une decision | Un concept avec relations |
| **Recherche** | FTS + vecteur | FTS + labels + graphe BFS |
| **Evolution** | Le souvenir decay ou est prune | Le concept est raffine (revision++) |
| **Meilleur pour** | Evenements, erreurs, decisions ponctuelles | Architecture, modeles de domaine, connaissances structurees |

### Exemples concrets

**Utiliser Memory quand...**

```bash
# Une erreur vient d'etre resolue
icm store -t "erreurs" -c "Timeout fixe en augmentant pool_max_size a 20" -i medium -k "timeout,pool"

# Une preference est decouverte
icm store -t "preferences" -c "L'utilisateur prefere les imports absolus" -i high

# Un fait temporaire
icm store -t "contexte-sprint" -c "Le sprint actuel se concentre sur la facturation" -i low
```

**Utiliser Memoir quand...**

```bash
# Modeliser l'architecture comme un graphe
icm memoir create -n "archi" -d "Architecture du systeme"
icm memoir add-concept -m "archi" -n "api-gateway" -d "Point d'entree unique, routing, rate limiting" -l "type:service"
icm memoir add-concept -m "archi" -n "user-db" -d "PostgreSQL 16, schema users/sessions" -l "type:database"
icm memoir link -m "archi" --from "api-gateway" --to "user-db" -r depends-on

# Documenter un domaine metier
icm memoir create -n "domaine-facturation" -d "Concepts du domaine facturation"
icm memoir add-concept -m "domaine-facturation" -n "invoice" -d "Facture avec lignes, TVA, echeance"
icm memoir add-concept -m "domaine-facturation" -n "payment" -d "Paiement lie a une ou plusieurs factures"
icm memoir link -m "domaine-facturation" --from "payment" --to "invoice" -r related-to
```

### Workflow combine : du souvenir au graphe

1. **Session 1-5** : Stocker des decisions en memories
   ```bash
   icm store -t "decisions-api" -c "Choix PostgreSQL pour JSONB" -i high
   icm store -t "decisions-api" -c "Redis pour le cache de sessions" -i high
   icm store -t "decisions-api" -c "JWT pour l'auth API" -i high
   ```

2. **Session 6** : Quand les patterns emergent, distiller en memoir
   ```bash
   icm memoir create -n "archi-api" -d "Architecture API v1"
   icm memoir distill --from-topic "decisions-api" --into "archi-api"
   ```

3. **Session 7+** : Raffiner le graphe et continuer a stocker des events
   ```bash
   icm memoir refine -m "archi-api" -n "postgres" -d "PostgreSQL 16 + JSONB + PostGIS"
   icm store -t "decisions-api" -c "Ajout de PostGIS pour la geolocalisation" -i high
   ```

---

## Workflow multi-session

### Scenario concret : projet API sur 3 jours

#### Session 1 -- Demarrage du projet

L'agent demarre et n'a aucun contexte.

```
Agent: icm_memory_recall("API project setup")
ICM: No memories found.

Agent: (lit le code, fait des decisions)

Agent: icm_memory_store({
  topic: "projet-api",
  content: "Nouveau projet API REST avec Actix-web. Structure: src/routes/, src/models/, src/db/",
  importance: "high",
  keywords: ["actix", "rust", "api", "structure"]
})
ICM: Stored memory: 01HW...A1

Agent: icm_memory_store({
  topic: "projet-api",
  content: "PostgreSQL choisi pour le support JSONB et la maturite de l'ecosysteme Rust",
  importance: "high",
  keywords: ["postgres", "database", "decision"]
})
ICM: Stored memory: 01HW...A2
```

#### Session 2 -- Reprise le lendemain

L'agent demarre et rappelle le contexte.

```
Agent: icm_memory_recall("API project")
ICM:
  [projet-api] Nouveau projet API REST avec Actix-web. Structure: src/routes/, src/models/, src/db/
  [projet-api] PostgreSQL choisi pour le support JSONB et la maturite de l'ecosysteme Rust

Agent: (n'a pas besoin de relire le code pour comprendre le contexte)
Agent: (corrige un bug et stocke la resolution)

Agent: icm_memory_store({
  topic: "erreurs-resolues",
  content: "Le middleware d'auth bloquait les routes publiques -- ajoute un filtre d'exclusion par prefix",
  importance: "medium",
  keywords: ["auth", "middleware", "routing", "fix"]
})
```

#### Session 3 -- Meme probleme revient

```
Agent: icm_memory_recall("auth middleware issue")
ICM:
  [erreurs-resolues] Le middleware d'auth bloquait les routes publiques -- ajoute un filtre d'exclusion par prefix

Agent: (applique directement la solution connue sans re-debugger)
```

### Points cles du workflow multi-session

1. **En debut de session** : toujours `icm_memory_recall` avec le contexte du projet
2. **Apres chaque decision importante** : `icm_memory_store` avec importance `high`
3. **Apres chaque bug fixe** : `icm_memory_store` avec mots-cles specifiques
4. **Periodiquement** : `icm_memory_health` pour verifier l'hygiene des topics
5. **Quand un topic grossit** : `icm_memory_consolidate` pour densifier

---

## Organisation des topics

### Bonnes pratiques

#### Nommage

| Pattern | Exemple | Quand utiliser |
|---------|---------|----------------|
| `{projet}` | `mon-api` | Contexte general d'un projet |
| `decisions-{projet}` | `decisions-api` | Decisions d'architecture et design |
| `erreurs-resolues` | `erreurs-resolues` | Bugs corriges et leurs solutions |
| `preferences` | `preferences` | Style de code, preferences d'outils |
| `conventions-{projet}` | `conventions-api` | Conventions de code, nommage, structure |
| `infra` | `infra` | URLs, ports, configuration serveur |
| `contexte-{sprint}` | `contexte-sprint-3` | Contexte temporaire d'un sprint |

#### Regles de base

1. **Un topic par preoccupation** -- Ne pas melanger decisions et erreurs dans le meme topic
2. **Prefixer par projet** quand on travaille sur plusieurs projets
3. **Utiliser `critical`** pour les faits invariants (ports, URLs, credentials)
4. **Utiliser `low`** pour les notes temporaires qui n'ont pas besoin de persister
5. **Consolider regulierement** quand un topic depasse 7-10 entrees
6. **Ne pas creer de topics trop granulaires** -- `erreurs-cors` est trop fin, `erreurs-resolues` suffit

#### Anti-patterns

- `todo` -- ICM n'est pas un gestionnaire de taches
- `misc` / `divers` -- Trop vague, impossible a rappeler efficacement
- Un topic par fichier -- Granularite excessive, utiliser les mots-cles plutot
- Tout en `critical` -- Defait l'objectif du decay, tout sera garde indefiniment

---

## Guide de consolidation

### Quand consolider ?

- **ICM le dit** : le MCP avertit quand un topic depasse 7 entrees
- **L'audit le montre** : `icm health` rapporte `needs_consolidation=true`
- **Manuellement** : quand on sent qu'un topic est devenu bruyant

### Comment consolider ?

#### Via le CLI (consolidation automatique)

```bash
# Voit l'etat
icm health

# Consolide en remplacant tous les souvenirs
icm consolidate --topic "erreurs-resolues"

# Ou garde les originaux (le resume est ajoute, pas remplace)
icm consolidate --topic "erreurs-resolues" --keep-originals
```

Le CLI fusionne automatiquement : il concatene les summaries avec ` | `, fusionne les mots-cles, et prend l'importance la plus haute.

#### Via le MCP (consolidation guidee par l'agent)

L'agent fait un meilleur travail car il comprend le contenu :

```
Agent: icm_memory_recall("erreurs-resolues" topic, limit 20)
ICM: (retourne 12 souvenirs)

Agent: (synthetise un resume intelligent)

Agent: icm_memory_consolidate({
  topic: "erreurs-resolues",
  summary: "Erreurs principales resolues: 1) CORS fixe via nginx proxy_set_header 2) Memory leak DB corrige en fermant les connexions avec defer 3) Rate limiting ajoute sur /api/auth pour contrer le brute force 4) Timeout Actix augmente a 30s pour les uploads"
})
```

### Impact de la consolidation

| Avant | Apres |
|-------|-------|
| 12 entrees dans le topic | 1 entree dense |
| Recherche retourne du bruit | Recherche retourne le resume pertinent |
| Poids variables (certains declines) | Poids = 1.0 (frais) |
| Importance mixte | Importance = la plus haute des originaux |

### Quand ne PAS consolider

- Topic avec <5 entrees -- pas encore necessaire
- Topic dont les entrees sont tres differentes (pas consolidable en un resume coherent)
- Quand les souvenirs individuels ont des mots-cles importants pour la recherche specifique

---

## Guide des niveaux d'importance

### Les 4 niveaux

#### `critical` -- Jamais oublie, jamais prune

**Decay :** 0 (aucun)
**Pruning :** jamais

**Utiliser pour :**
- Ports, URLs et credentials de production
- Contraintes de securite absolues
- Faits invariants du projet

**Exemples :**
```bash
icm store -t "infra" -c "DB prod sur port 5433, pas 5432" -i critical
icm store -t "securite" -c "Jamais stocker de PII dans les logs" -i critical
icm store -t "infra" -c "L'API de prod est derriere Cloudflare, IP directe interdite" -i critical
```

#### `high` -- Decay lent, jamais prune

**Decay :** 0.5x le taux normal
**Pruning :** jamais

**Utiliser pour :**
- Decisions d'architecture
- Patterns recurrents
- Preferences utilisateur confirmees

**Exemples :**
```bash
icm store -t "decisions" -c "REST plutot que GraphQL pour la v1" -i high
icm store -t "preferences" -c "Toujours utiliser des imports absolus" -i high
icm store -t "conventions" -c "Noms de fichiers en kebab-case" -i high
```

#### `medium` -- Decay normal, peut etre prune

**Decay :** 1.0x (taux standard)
**Pruning :** oui, quand poids < seuil (defaut 0.1)

Valeur par defaut si non specifie.

**Utiliser pour :**
- Configurations ponctuelles
- Contexte de session
- Corrections de bugs standard

**Exemples :**
```bash
icm store -t "erreurs" -c "CORS fixe en ajoutant le header dans nginx" -i medium
icm store -t "config" -c "Variable REDIS_URL configuree dans .env.local" -i medium
```

#### `low` -- Decay rapide, prune rapidement

**Decay :** 2.0x le taux normal
**Pruning :** oui, quand poids < seuil

**Utiliser pour :**
- Notes d'exploration temporaires
- Hypotheses non confirmees
- Contexte ephemere

**Exemples :**
```bash
icm store -t "exploration" -c "Test de la lib XYZ -- ne semble pas compatible" -i low
icm store -t "contexte" -c "Actuellement en train de debugger le module auth" -i low
```

### Tableau recapitulatif

| Niveau | Decay rate | Prune | Duree de vie typique | Usage |
|--------|-----------|-------|---------------------|-------|
| `critical` | 0 | jamais | infinie | Faits invariants |
| `high` | 0.5x | jamais | mois | Decisions importantes |
| `medium` | 1.0x | oui | semaines | Contexte standard |
| `low` | 2.0x | oui | jours | Notes temporaires |

---

## Modele de decay explique

### Le principe

Chaque souvenir a un **poids** (weight) qui demarre a 1.0 et diminue dans le temps. Plus le poids est bas, moins le souvenir est pertinent. Quand le poids tombe en dessous d'un seuil (defaut 0.1), le souvenir peut etre prune automatiquement.

### La formule

```
taux_effectif = taux_base x multiplicateur_importance / (1 + compteur_acces x 0.1)

nouveau_poids = poids x (1 - taux_effectif)
```

**Ou :**
- `taux_base` = taux de decay configure (defaut 0.95, signifie 5% de perte par cycle)
- `multiplicateur_importance` = voir tableau ci-dessous
- `compteur_acces` = nombre de fois que le souvenir a ete rappele

### Multiplicateurs d'importance

| Importance | Multiplicateur | Effet |
|-----------|---------------|-------|
| `critical` | 0.0 | **Aucun decay** -- le poids reste a 1.0 pour toujours |
| `high` | 0.5 | Decay a moitie vitesse |
| `medium` | 1.0 | Decay normal |
| `low` | 2.0 | Decay a double vitesse |

### L'effet de l'acces

Plus un souvenir est rappele, plus il resiste au decay. Le denominateur `(1 + access_count x 0.1)` reduit le taux effectif :

| Nombre d'acces | Diviseur | Taux effectif (medium) |
|----------------|----------|----------------------|
| 0 | 1.0 | 5.0% par cycle |
| 5 | 1.5 | 3.3% par cycle |
| 10 | 2.0 | 2.5% par cycle |
| 20 | 3.0 | 1.7% par cycle |

**Interpretation :** un souvenir rappele 10 fois decay 2x plus lentement qu'un souvenir jamais rappele.

### Quand le decay s'execute

- **Automatiquement** : a chaque `icm recall` ou `icm_memory_recall`, si >24h depuis la derniere execution
- **Manuellement** : via `icm decay`
- Le timestamp du dernier decay est stocke dans `icm_metadata.last_decay_at`

### Exemple concret

Un souvenir `medium`, jamais rappele, avec le taux de decay par defaut (0.95) :

| Jour | Poids | Statut |
|------|-------|--------|
| 0 | 1.000 | Frais |
| 7 | 0.698 | Encore pertinent |
| 14 | 0.488 | Commence a vieillir |
| 21 | 0.341 | Vieilli |
| 30 | 0.214 | Presque prune |
| 46 | 0.099 | **Prune** (< 0.1) |

Le meme souvenir en `high` (0.5x decay) :

| Jour | Poids | Statut |
|------|-------|--------|
| 0 | 1.000 | Frais |
| 30 | 0.463 | Encore solide |
| 60 | 0.214 | Commence a vieillir |
| 90 | 0.099 | Jamais prune (high = pas de prune) |

Et en `critical` : poids = 1.000 pour toujours.

### Protection contre la perte de donnees

- Les souvenirs `critical` ne declient **jamais**
- Les souvenirs `high` ne sont **jamais prunes** (meme si leur poids baisse)
- Le decay est `access-aware` : rappeler un souvenir le renforce
- Le pruning ne supprime que `medium` et `low` sous le seuil

---

## Configuration complete

### Fichier de configuration

Emplacement : `~/.config/icm/config.toml` (ou `$ICM_CONFIG`)

```toml
[store]
# Chemin de la base SQLite (defaut : chemin plateforme)
# path = "~/Library/Application Support/dev.icm.icm/memories.db"

[memory]
# Importance par defaut si non specifiee
default_importance = "medium"

# Taux de decay par jour (0.95 = perd 5% par jour)
decay_rate = 0.95

# Seuil de pruning automatique
prune_threshold = 0.1

[embeddings]
# Modele d'embedding (code fastembed)
model = "intfloat/multilingual-e5-base"

# Alternatives :
# "intfloat/multilingual-e5-small"              # 384d, multilingue, plus leger
# "intfloat/multilingual-e5-large"              # 1024d, multilingue, meilleure precision
# "Xenova/bge-small-en-v1.5"                    # 384d, anglais seul, le plus rapide
# "jinaai/jina-embeddings-v2-base-code"         # 768d, optimise pour le code

[extraction]
# Layer 0 : extraction de faits par regles (zero cout LLM)
enabled = true

# Score minimum pour garder un fait
min_score = 3.0

# Maximum de faits par passe d'extraction
max_facts = 10

[recall]
# Layer 2 : injection de contexte avant les sessions
enabled = true

# Maximum de souvenirs a injecter
limit = 15

[mcp]
# Transport du serveur MCP
transport = "stdio"

# Mode compact : reponses courtes pour economiser des tokens
compact = true

# Instructions personnalisees ajoutees a la description du serveur MCP
# instructions = "Toujours recall avant de commencer a travailler"
```

### Variables d'environnement

| Variable | Description |
|----------|-------------|
| `ICM_CONFIG` | Chemin vers le fichier de configuration |
| `ICM_DB` | Chemin vers la base SQLite |
| `ICM_LOG` | Niveau de log (`debug`, `info`, `warn`, `error`) |

### Emplacement de la base de donnees

| Plateforme | Chemin |
|------------|--------|
| macOS | `~/Library/Application Support/dev.icm.icm/memories.db` |
| Linux | `~/.local/share/dev.icm.icm/memories.db` |

Surcharge possible via `--db <chemin>` ou `ICM_DB`.

### Changement de modele d'embedding

Quand on change le modele dans `config.toml` :
1. Au prochain demarrage, ICM detecte le changement de dimensions
2. La table `vec_memories` est supprimee et recreee
3. Tous les embeddings existants sont effaces
4. Regrenerer avec `icm embed --force`
