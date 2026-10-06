[English](README.md) | [Français](README_fr.md) | [Español](README_es.md) | [Deutsch](README_de.md) | [Italiano](README_it.md) | [Português](README_pt.md) | [Nederlands](README_nl.md) | [Polski](README_pl.md) | [Русский](README_ru.md) | [日本語](README_ja.md) | [中文](README_zh.md) | [العربية](README_ar.md) | [한국어](README_ko.md)

Ceci est une traduction de README.md (anglais), qui fait référence ; en cas de différence, c'est la version anglaise qui a raison.

<h1 align="center">ICM</h1>

<p align="center">
  <b>Une mémoire à long terme pour les agents de code IA, partagée entre vos outils.</b><br>
  Un binaire, un fichier SQLite. Aucun appel LLM pour stocker ou rappeler une mémoire.
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/icm/actions/workflows/ci.yml"><img src="https://github.com/rtk-ai/icm/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/icm/releases/latest"><img src="https://img.shields.io/github/v/release/rtk-ai/icm?color=purple" alt="Version"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-Apache--2.0-blue.svg" alt="Apache-2.0"></a>
</p>

Expliquez lundi à Claude Code comment votre projet gère l'authentification, et la session Gemini CLI de mardi le sait déjà. ICM conserve ce que vos agents de code apprennent (décisions, correctifs, conventions, préférences) dans un seul fichier SQLite sur votre machine, et leur en restitue la partie pertinente au début de chaque session et à chaque prompt que vous envoyez. Jusqu'à 18 agents et éditeurs partagent cette mémoire : vous n'avez plus à réexpliquer votre projet chaque fois que vous ouvrez une session ou changez d'outil.

- **92.9% sur LoCoMo (1,540 questions, moyenne de trois runs), au niveau de Hindsight (92.0%)**, avec un tiers de contexte en moins par question. [Détails et réserves plus bas](#benchmark-comparison).
- **Aucun appel LLM pour stocker ou rappeler.** Hindsight, Mem0, Graphiti (Zep) et claude-mem appellent par défaut un LLM pour chaque mémoire qu'ils stockent. ICM, non. Seule son extraction automatique de faits à partir de la sortie des outils passe par l'outil LLM en ligne de commande que vous utilisez déjà, lorsqu'il y en a un d'installé ; `provider = "none"` la garde elle aussi en local (voir [Démarrage rapide](#quickstart)).
- **Utile sans modèle d'embedding.** Le rappel par mots-clés seul place au moins une des bonnes sessions dans le top 5 pour 88.6% des questions LoCoMo, avec une médiane de 6.8 ms par rappel sous Linux x86-64.
- **97.4% sur LongMemEval-S, en récupération sans LLM** : une des bonnes sessions dans le top 5 pour 487 questions sur 500, contre 96.6% pour MemPalace et 95.2% pour agentmemory sur la même mesure.
- **Pas en tête partout.** Sur PersonaMem (les préférences d'un utilisateur qui évoluent), Hindsight mène : 86.6% contre 81.7% pour ICM.

<p align="center">
  <img src="assets/demo.svg" alt="Terminal : trois mémoires stockées avec icm store, puis deux questions auxquelles icm recall répond, chacune en renvoyant la bonne mémoire">
</p>

<a id="quickstart"></a>

## Démarrage rapide

```bash
brew tap rtk-ai/tap && brew install icm    # or: curl -fsSL https://raw.githubusercontent.com/rtk-ai/icm/main/install.sh | sh
icm init                                   # instructions, skills and hooks for every agent it detects
```

C'est toute la mise en place. Ouvrez une nouvelle session dans Claude Code, Codex, Gemini CLI ou Copilot CLI : votre agent démarre désormais avec un court paquet de ses mémoires les plus importantes et reçoit les mémoires pertinentes pour chaque prompt que vous envoyez. Ce qu'il apprend de la sortie de ses outils est mis en file d'attente, et cette file est transformée en mémoires à la fin de chaque session Claude Code ; avec les autres outils, lancez `icm extract-pending` (depuis une tâche cron, par exemple).

Le stockage et le rappel restent sur votre machine. L'extraction automatique transmet du texte à l'outil LLM en ligne de commande que vous utilisez déjà (Claude Code, Codex ou Gemini CLI), lorsqu'il y en a un d'installé ; définissez `provider = "none"` sous `[extraction.summarizer]` pour qu'elle reste entièrement locale.

Pour le voir fonctionner tout de suite, stockez et rappelez à la main :

```console
$ icm store -t decisions-myapp -c "Auth uses short-lived JWTs, refreshed through /auth/refresh" -i high
Stored: 01M47YY6CHVZ48BYKQRCRNZTCF

$ icm recall "how does auth work"
memories[1]{id,topic,importance,weight,summary}:
  01M47YY6CHVZ48BYKQRCRNZTCF,decisions-myapp,high,0.975,"Auth uses short-lived JWTs, refreshed through /auth/refresh"
```

Le premier stockage ou rappel avec recherche sémantique télécharge une fois le modèle d'embedding multilingue (`Qdrant/multilingual-e5-large-onnx`, environ 2 GB). Pour essayer ICM sans lui, ajoutez `--no-embeddings` (rappel par mots-clés, comme dans la sortie ci-dessus) ou choisissez un modèle plus léger dans la config. `icm init` écrit des hooks et des instructions dans la configuration de chaque agent détecté ; `icm uninstall --dry-run` montre comment les retirer. Windows, Linux, Nix et compilation depuis les sources : [Installation](#install).

<a id="benchmark-comparison"></a>

## Comparaison des benchmarks

Précision des réponses sur [LoCoMo](https://github.com/snap-research/locomo) (10 longues conversations, 1,540 questions), mesurée avec le banc d'évaluation public [Agent Memory Benchmark](https://github.com/vectorize-io/agent-memory-benchmark) : le système de mémoire récupère le contexte, `gemini-3.1-pro-preview` répond à partir de ce contexte, `gemini-2.5-flash-lite` juge la réponse.

| Système | Précision LoCoMo | Contexte par question | Appels LLM pour stocker une mémoire | Forme | Résultat |
|--------|:---------------:|:--------------------:|:---------------------------:|---------|--------|
| **ICM** 0.11.0 (moteur de rappel v2) | **92.9%** (1,430, 1,433 et 1,430 / 1,540 sur trois runs) | 24.1k tokens | aucun | un binaire Rust, un fichier SQLite | nos runs, 2026-10-06 |
| Hindsight | 92.0% (1,417 / 1,540) | 36.2k tokens | extraction de faits par LLM | service Python, PostgreSQL + pgvector | publié par le banc d'évaluation |
| Référence de recherche hybride (dense + sparse, RRF) | 79.1% (1,218 / 1,540) | 22.2k tokens | aucun | Qdrant | publié par le banc d'évaluation |

Sur [PersonaMem](https://arxiv.org/abs/2504.14225) 32k (589 questions à choix multiples sur les préférences évolutives d'un utilisateur, même banc d'évaluation et même modèle de réponse, notation par correspondance de la lettre) :

| Système | Précision PersonaMem | Contexte par question | Résultat |
|--------|:-------------------:|:--------------------:|--------|
| **ICM** 0.11.0 (moteur de rappel v2) | **81.7%** (486, 486 et 472 / 589 sur trois runs) | 16.2k tokens | nos runs, 2026-10-06 |
| Hindsight | 86.6% (510 / 589) | 15.8k tokens | publié par le banc d'évaluation |
| Référence de recherche hybride | 84.4% (497 / 589) | 24.2k tokens | publié par le banc d'évaluation |

La récupération seule, sur LoCoMo, sans modèle de réponse : la part des questions pour lesquelles au moins une session de référence figure dans les premiers résultats. C'est la preuve qui porte sur le moteur de rappel lui-même. Les runs v2 ont reçu la date de chaque session et la date de la question ; le moteur précédent n'a pas d'entrée de date, ses runs n'en ont donc reçu aucune.

| Premiers résultats | Moteur précédent (`legacy`) | Moteur de rappel v2 | Moteur précédent, sans modèle d'embedding | Moteur de rappel v2, sans modèle d'embedding |
|:-----------:|:--------------:|:----------------:|:--------------:|:----------------:|
| 5 | 76.5% | **86.7%** | 12.0% | **88.6%** |
| 10 | 83.0% | **93.3%** | 17.4% | **94.2%** |
| 20 | 87.7% | **97.9%** | 29.8% | **97.5%** |

LongMemEval-S, récupération seule, sans LLM (ICM avec son modèle d'embedding par défaut ; 500 questions ; chaque question est accompagnée d'environ 48 sessions passées dans lesquelles chercher ; une mémoire par session, tours de l'utilisateur seulement, comme MemPalace les indexe ; aucune date fournie à ICM) :

| Bonnes sessions dans le top 5 | **ICM** 0.11.0 | MemPalace | agentmemory | BM25 seul |
|---|:---:|:---:|:---:|:---:|
| Au moins une (la mesure publiée) | **97.4%** (487 / 500) | 96.6% (483 / 500) | 95.2% (476 / 500) | 94.6% |
| Toutes | **88.6%** (443 / 500) | 85.0% | 81.8% | 81.2% |

Les chiffres de MemPalace et d'agentmemory sont recalculés avec notre script de notation à partir des fichiers de résultats que chaque projet publie ; ils correspondent à leurs chiffres publiés. agentmemory indexe tous les tours d'une session ; sur cette unité, BM25 seul atteint 96.2% et 83.0%. Un BM25 simple obtient déjà autour de 95% sur la première mesure, c'est pourquoi la seconde, toutes les bonnes sessions, départage mieux les systèmes.

Ce que ces chiffres montrent et ne montrent pas :

- **ICM et Hindsight sont à égalité sur LoCoMo.** Les trois runs d'ICM (92.9%, 93.1%, 92.9%) sont chacun environ 1 point au-dessus des 92.0% publiés par Hindsight, un écart d'environ 14 questions, dans la marge d'erreur d'échantillonnage (intervalle à 95% pour ICM : 91.6 à 94.2). ICM y parvient avec un tiers de contexte en moins et sans appeler de LLM lorsqu'une mémoire est stockée.
- **Sur PersonaMem, Hindsight est devant** de 4.9 points, hors de la marge d'erreur d'échantillonnage (intervalle à 95% pour ICM : 78.6 à 84.8). ICM est aussi 2.7 points sous la référence de recherche hybride, à l'intérieur de cet intervalle, tout en lisant un tiers de contexte en moins qu'elle. Les trois runs s'étalent de 80.1% à 82.5%.
- **Des conditions qui ne sont pas identiques.** Le banc d'évaluation est maintenu par Vectorize, l'éditeur de Hindsight. Les résultats LoCoMo publiés sont antérieurs à un changement qui a fixé à 0 la température de la réponse et du juge ; notre run utilise la version actuelle du banc (commit `f618ed7`) et Vertex AI.
- **À 50 chunks, une grande partie de chaque conversation est renvoyée,** si bien que cette précision mesure aussi le modèle de réponse. Le tableau de récupération est la preuve qui porte sur le moteur de rappel lui-même.
- **Trois runs par jeu de données.** Le modèle de réponse varie d'un run à l'autre : 0.2 point sur LoCoMo, 2.4 points sur PersonaMem. Les intervalles ci-dessus couvrent l'échantillonnage des questions.

<details>
<summary>Résultats par catégorie, latence et autres réserves</summary>

- **Par type de question** (LoCoMo, libellés du banc d'évaluation, trois runs) : open-domain 96.7% (841 questions), temporal 90.9% (321), single-hop 88.8% (282), multi-hop 78.8% (96).
- **Latence.** Latence médiane de rappel de 137 à 149 ms sur les nœuds à 4 vCPU du cluster pendant les runs LoCoMo ; 272 sessions ingérées sans aucun appel LLM. Dans les runs de récupération seule sous Linux x86-64 : médiane de 136 ms avec embeddings, 6.8 ms en mots-clés seuls.
- **Dates des sessions.** L'adaptateur du benchmark écrit la date de chaque session dans le texte de la mémoire ICM ; Hindsight reçoit les mêmes dates sous forme de métadonnées, et chaque système reçoit la date de la question.
- **La catégorie la plus faible est celle des 96 questions que le banc d'évaluation étiquette multi-hop** (78.8%). Les noms de catégories ne concordent pas d'un benchmark à l'autre : d'autres évaluations LoCoMo appellent cette catégorie open-domain, et appellent multi-hop les 282 questions que le banc étiquette single-hop (88.8% ici). Comparez par nombre de questions, pas par libellé.
- **Le moteur de rappel v2 est celui par défaut** pour `icm recall`, l'outil MCP `icm_memory_recall`, `/recall` en HTTP et le hook de prompt. Le moteur précédent reste disponible pour revenir en arrière ou pour comparer : `icm recall --engine legacy`, `"engine": "legacy"` sur `/recall` en HTTP, ou `ICM_RECALL_ENGINE=legacy`.

</details>

Les résultats question par question de chaque run (trois par jeu de données, plus le run de rappel LongMemEval-S) sont dans [`bench/amb/results/`](bench/amb/results/) ; l'adaptateur, les réglages exacts et les commandes pour reproduire sont dans [`bench/amb/README.md`](bench/amb/README.md).

<a id="one-memory-for-every-tool"></a>

## Une mémoire pour tous les outils

Chaque outil configuré par `icm init` lit et écrit dans la même base SQLite, et les topics (`decisions-myapp`, `preferences`, `errors-resolved`, ...) ne sont pas cloisonnés par outil. Une mémoire stockée depuis Claude Code est immédiatement visible par Codex, Gemini, Cursor, Roo, Amp, Aider, ...

Vous préférez l'isolation ? `icm init --per-project` crée une base propre au projet sous `.icm/` (et écrit les fichiers d'instructions des agents, comme `CLAUDE.md` et `AGENTS.md`, dans le répertoire courant) ; `--db <path>` ou `ICM_DB` pointent vers n'importe quel autre fichier. Chaque chemin est un corpus indépendant.

> **État du projet : bêta.** ICM est en pré-1.0 : des changements incompatibles peuvent arriver dans n'importe quelle version mineure, et les formats de configuration des hooks et du MCP peuvent évoluer. Bêta désigne la stabilité de l'API, pas l'utilité au quotidien : moi (le mainteneur), j'utilise ICM tous les jours comme mémoire principale pour coder avec l'IA. Mon projet principal est [rtk](https://github.com/rtk-ai/rtk), donc les issues et les pull requests sont examinées au mieux, selon mes disponibilités.
>
> Apache-2.0, fourni **tel quel, sans garantie d'aucune sorte** (voir [LICENSE](LICENSE)). Avant toute opération destructive, lancez d'abord l'équivalent en lecture seule (`icm uninstall --dry-run`, `icm uninstall --check`).

<a id="install"></a>

## Installation

```bash
# macOS / Linux, Homebrew
brew tap rtk-ai/tap && brew install icm

# macOS / Linux, script (verifies SHA256 against the release checksums)
curl -fsSL https://raw.githubusercontent.com/rtk-ai/icm/main/install.sh | sh

# Windows, PowerShell
irm https://raw.githubusercontent.com/rtk-ai/icm/main/install.ps1 | iex
```

La recherche par mots-clés fonctionne partout. Lancez `icm embeddings status` pour savoir si la recherche sémantique est active : elle est intégrée aux builds macOS Apple Silicon, Windows et `.rpm` ; les archives Linux glibc et le `.deb` demandent un `icm embeddings download` ; le build Mac Intel a besoin de votre propre ONNX Runtime (`ORT_DYLIB_PATH`) ; le build Linux musl statique fonctionne en mots-clés seulement. Nix, compilation depuis les sources, épinglage de version et détails : [référence](docs/reference.md#install).

<a id="setup"></a>

## Mise en place

```bash
icm init                  # global database, every detected agent
icm init --per-project    # database under .icm/ at the git root
icm init --mode all       # also register the MCP server in every tool that supports it
```

Le mode par défaut (`standard`) écrit les instructions, les skills et les hooks, sans serveur MCP. `--mode all` ajoute le serveur MCP ; avec lui (plus `--per-project` pour Aider, dont le fichier de conventions est propre à chaque projet), cela couvre les 18 outils ci-dessous ([guide d'intégration](docs/integrations.md)) :

| Outil | Serveur MCP | Hooks |
|------|:---:|:-----:|
| Claude Code | oui | oui |
| Claude Desktop | oui | — |
| Gemini CLI | oui | oui |
| Codex CLI | oui | oui |
| Copilot CLI | oui | oui |
| Cursor | oui | — |
| Windsurf | oui | — |
| VS Code | oui | — |
| Amp | oui | — |
| Amazon Q | oui | — |
| Cline | oui | — |
| Roo Code | oui | — |
| Kilo Code | oui | — |
| Zed | oui | — |
| OpenCode | oui | oui |
| Continue.dev | oui | — |
| Aider | — | — |
| Pi | — | — |

Ou enregistrez le serveur MCP à la main : `claude mcp add icm -- icm serve` (pour tout client MCP : commande `icm`, arguments `["serve"]`).

Ce que font les hooks :

| Hook | Ce qu'il fait |
|------|-------------|
| `icm hook start` | Injecte au début de la session un paquet de réveil de mémoires critical/high (~500 tokens) |
| `icm hook pre` | Autorise automatiquement les commandes CLI `icm` (sans demande de permission) |
| `icm hook post` | Extrait des faits de la sortie des outils tous les N appels (extraction automatique) |
| `icm hook compact` | Extrait des mémoires de la transcription avant la compression du contexte |
| `icm hook prompt` | Injecte le contexte rappelé au début de chaque prompt de l'utilisateur |

Tableaux de hooks par outil, skills, fichiers d'instructions et note sur Codex : [référence](docs/reference.md#setup).

<a id="use"></a>

## Utilisation

```bash
# Store
icm store -t "my-project" -c "Use PostgreSQL for the main DB" -i high -k "db,postgres"

# Recall
icm recall "database choice"
icm recall "auth setup" --topic "my-project" --limit 10
icm recall "architecture" --keyword "postgres"

# Manage
icm forget <memory-id>
icm consolidate --topic "my-project"
icm topics
icm stats

# Extract facts from text (rule-based, no LLM call)
echo "The parser uses Pratt algorithm" | icm extract -p my-project
```

ICM conserve aussi des **memoirs** (des graphes de connaissances permanents de concepts et de relations typées), du **feedback** (des corrections dont tirer des leçons) et des **transcriptions verbatim**, et expose **31 outils MCP** (30 sans modèle d'embedding), une **API HTTP** qui garde le modèle d'embedding chargé et un **tableau de bord en terminal** (`icm dashboard`). Tout cela est décrit dans la [référence](docs/reference.md).

<a id="how-it-works"></a>

## Fonctionnement

Le rappel fusionne jusqu'à trois listes classées par rang réciproque (RRF) : la correspondance par mots-clés **FTS5 BM25**, toujours active ; la **recherche vectorielle sémantique** via sqlite-vec lorsqu'un modèle d'embedding est chargé (par défaut `Qdrant/multilingual-e5-large-onnx`, 1024 dimensions, 100+ langues) ; et une **fenêtre de dates** lorsque la requête nomme une période ("last week", "in March 2024"). Les filtres de projet, de topic et de mot-clé s'appliquent avant la coupe. Les mémoires s'estompent avec le temps selon leur importance (`critical` ne s'estompe jamais) ; avec un modèle d'embedding chargé, une nouvelle mémoire presque identique à une autre du même topic (similarité cosinus supérieure à 0.95) est fusionnée avec elle ; et le modèle qui a produit les vecteurs stockés est enregistré dans la base, si bien que changer `model` dans la config ne les efface jamais (`icm embed --migrate` est la façon explicite de changer de modèle).

Tout tient dans un seul fichier SQLite, sans service externe :

```
~/Library/Application Support/dev.icm.icm/memories.db     # macOS
~/.local/share/icm/memories.db                            # Linux
%APPDATA%\icm\icm\data\memories.db                        # Windows
<project-root>/.icm/memories.db                           # icm init --per-project
```

`icm config` affiche la configuration active ; [config/default.toml](config/default.toml) liste toutes les options. Détails : [référence](docs/reference.md#how-it-works).

<a id="documentation"></a>

## Documentation

| Document | Description |
|----------|-------------|
| [Guide d'intégration](docs/integrations.md) | Configuration MCP par outil : Claude Code, Cursor, Windsurf, Zed, Amp, Codex, Cline, Roo Code, etc. |
| [Architecture technique](docs/architecture.md) | Structure des crates, pipeline de recherche, modèle de decay, intégration de sqlite-vec, tests |
| [Guide utilisateur](docs/guide.md) | Installation, organisation des topics, consolidation, extraction, dépannage |
| [Présentation du produit](docs/product.md) | Cas d'usage, benchmarks, comparaison avec les alternatives |
| [Référence](docs/reference.md) | Options d'installation, configuration par outil, CLI, 31 outils MCP, API HTTP, tableau de bord, fonctionnement interne |
| [Adaptateur de benchmark](bench/amb/README.md) | Comment la comparaison ci-dessus a été menée, et comment la reproduire |
| [Démonstrations](docs/demonstrations.md) | Micro-benchmarks de stockage et petites démos |

<a id="license"></a>

## Licence

[Apache-2.0](LICENSE)
