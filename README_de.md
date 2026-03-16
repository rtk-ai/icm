[English](README.md) | [Français](README_fr.md) | [Español](README_es.md) | **Deutsch** | [Italiano](README_it.md) | [Português](README_pt.md) | [Nederlands](README_nl.md) | [Polski](README_pl.md) | [Русский](README_ru.md) | [日本語](README_ja.md) | [中文](README_zh.md) | [العربية](README_ar.md) | [한국어](README_ko.md)

<p align="center">
  <img src="assets/banner.png" alt="ICM — Infinite Context Memory" width="600">
</p>

<h1 align="center">ICM</h1>

<p align="center">
  Persistentes Gedächtnis für KI-Agenten. Einzelne Binärdatei, keine Abhängigkeiten, MCP-nativ.
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/icm/actions/workflows/ci.yml"><img src="https://github.com/rtk-ai/icm/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/icm/releases/latest"><img src="https://img.shields.io/github/v/release/rtk-ai/icm?color=purple" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-Source--Available-orange.svg" alt="Source-Available"></a>
</p>

---

ICM gibt Ihrem KI-Agenten ein echtes Gedächtnis — kein Notiztool, kein Kontextmanager, sondern ein **Gedächtnis**.

```
                       ICM (Infinite Context Memory)
            ┌──────────────────────┬─────────────────────────┐
            │   MEMORIES (Topics)  │   MEMOIRS (Knowledge)   │
            │                      │                         │
            │  Episodisch, temporal│  Permanent, strukturiert│
            │                      │                         │
            │  ┌───┐ ┌───┐ ┌───┐  │    ┌───┐               │
            │  │ m │ │ m │ │ m │  │    │ C │──depends_on──┐ │
            │  └─┬─┘ └─┬─┘ └─┬─┘  │    └───┘              │ │
            │    │decay │     │    │      │ refines      ┌─▼─┐│
            │    ▼      ▼     ▼    │    ┌─▼─┐            │ C ││
            │  weight decreases    │    │ C │──part_of──>└───┘│
            │  over time unless    │    └───┘                 │
            │  accessed/critical   │  Concepts + Relations    │
            ├──────────────────────┴─────────────────────────┤
            │             SQLite + FTS5 + sqlite-vec          │
            │        Hybrid search: BM25 (30%) + cosine (70%) │
            └─────────────────────────────────────────────────┘
```

**Zwei Gedächtnismodelle:**

- **Memories** — Speichern und Abrufen mit zeitlichem Verfall nach Wichtigkeit. Kritische Erinnerungen verblassen nie, unwichtige verblassen natürlich. Nach Thema oder Schlüsselwort filtern.
- **Memoirs** — Permanente Wissensgraphen. Konzepte verknüpft durch typisierte Relationen (`depends_on`, `contradicts`, `superseded_by`, ...). Nach Label filtern.
- **Feedback** — Korrekturen aufzeichnen, wenn KI-Vorhersagen falsch sind. Vergangene Fehler durchsuchen, bevor neue Vorhersagen gemacht werden. Geschlossener Lernkreislauf.

## Installation

```bash
# Homebrew (macOS / Linux)
brew tap rtk-ai/tap && brew install icm

# Schnellinstallation
curl -fsSL https://raw.githubusercontent.com/rtk-ai/icm/main/install.sh | sh

# Aus Quellcode
cargo install --path crates/icm-cli
```

## Einrichtung

```bash
# Alle unterstützten Tools automatisch erkennen und konfigurieren
icm init
```

Konfiguriert **14 Tools** mit einem einzigen Befehl:

| Tool | Konfigurationsdatei | Format |
|------|---------------------|--------|
| Claude Code | `~/.claude.json` | JSON |
| Claude Desktop | `~/Library/.../claude_desktop_config.json` | JSON |
| Cursor | `~/.cursor/mcp.json` | JSON |
| Windsurf | `~/.codeium/windsurf/mcp_config.json` | JSON |
| VS Code / Copilot | `~/Library/.../Code/User/mcp.json` | JSON |
| Gemini Code Assist | `~/.gemini/settings.json` | JSON |
| Zed | `~/.zed/settings.json` | JSON |
| Amp | `~/.config/amp/settings.json` | JSON |
| Amazon Q | `~/.aws/amazonq/mcp.json` | JSON |
| Cline | VS Code globalStorage | JSON |
| Roo Code | VS Code globalStorage | JSON |
| Kilo Code | VS Code globalStorage | JSON |
| OpenAI Codex CLI | `~/.codex/config.toml` | TOML |
| OpenCode | `~/.config/opencode/opencode.json` | JSON |

Oder manuell:

```bash
# Claude Code
claude mcp add icm -- icm serve

# Kompaktmodus (kürzere Antworten, spart Tokens)
claude mcp add icm -- icm serve --compact

# Beliebiger MCP-Client: command = "icm", args = ["serve"]
```

### Skills / Regeln

```bash
icm init --mode skill
```

Installiert Slash-Befehle und Regeln für Claude Code (`/recall`, `/remember`), Cursor (`.mdc`-Regel), Roo Code (`.md`-Regel) und Amp (`/icm-recall`, `/icm-remember`).

### Hooks (Claude Code)

```bash
icm init --mode hook
```

Installiert alle 3 Extraktionsebenen als Claude Code Hooks:

**Claude Code** Hooks:

| Hook | Ereignis | Funktion |
|------|----------|----------|
| `icm hook pre` | PreToolUse | `icm` CLI-Befehle automatisch erlauben (keine Berechtigungsabfrage) |
| `icm hook post` | PostToolUse | Fakten aus Tool-Ausgaben alle 15 Aufrufe extrahieren |
| `icm hook compact` | PreCompact | Erinnerungen aus Transkript vor der Kontextkomprimierung extrahieren |
| `icm hook prompt` | UserPromptSubmit | Abgerufenen Kontext am Anfang jeder Eingabe einfügen |

**OpenCode** Plugin (automatisch installiert nach `~/.config/opencode/plugins/icm.js`):

| OpenCode-Ereignis | ICM-Ebene | Funktion |
|-------------------|-----------|----------|
| `tool.execute.after` | Layer 0 | Fakten aus Tool-Ausgaben extrahieren |
| `experimental.session.compacting` | Layer 1 | Aus Konversation vor der Komprimierung extrahieren |
| `session.created` | Layer 2 | Kontext beim Sitzungsstart abrufen |

## CLI vs MCP

ICM kann über die CLI (`icm`-Befehle) oder den MCP-Server (`icm serve`) verwendet werden. Beide greifen auf dieselbe Datenbank zu.

| | CLI | MCP |
|---|-----|-----|
| **Latenz** | ~30ms (direkte Binärdatei) | ~50ms (JSON-RPC stdio) |
| **Token-Kosten** | 0 (hook-basiert, unsichtbar) | ~20-50 Tokens/Aufruf (Tool-Schema) |
| **Einrichtung** | `icm init --mode hook` | `icm init --mode mcp` |
| **Kompatibel mit** | Claude Code, OpenCode (über Hooks/Plugins) | Allen 14 MCP-kompatiblen Tools |
| **Auto-Extraktion** | Ja (Hooks lösen `icm extract` aus) | Ja (MCP-Tools rufen store auf) |
| **Geeignet für** | Power-User, Token-Einsparung | Universelle Kompatibilität |

## CLI

### Memories (episodisch, mit Verfall)

```bash
# Speichern
icm store -t "my-project" -c "Use PostgreSQL for the main DB" -i high -k "db,postgres"

# Abrufen
icm recall "database choice"
icm recall "auth setup" --topic "my-project" --limit 10
icm recall "architecture" --keyword "postgres"

# Verwalten
icm forget <memory-id>
icm consolidate --topic "my-project"
icm topics
icm stats

# Fakten aus Text extrahieren (regelbasiert, keine LLM-Kosten)
echo "The parser uses Pratt algorithm" | icm extract -p my-project
```

### Memoirs (permanente Wissensgraphen)

```bash
# Memoir erstellen
icm memoir create -n "system-architecture" -d "System design decisions"

# Konzepte mit Labels hinzufügen
icm memoir add-concept -m "system-architecture" -n "auth-service" \
  -d "Handles JWT tokens and OAuth2 flows" -l "domain:auth,type:service"

# Konzepte verknüpfen
icm memoir link -m "system-architecture" --from "api-gateway" --to "auth-service" -r depends-on

# Mit Label-Filter suchen
icm memoir search -m "system-architecture" "authentication"
icm memoir search -m "system-architecture" "service" --label "domain:auth"

# Nachbarschaft inspizieren
icm memoir inspect -m "system-architecture" "auth-service" -D 2

# Graphen exportieren (Formate: json, dot, ascii, ai)
icm memoir export -m "system-architecture" -f ascii   # Boxzeichnungen mit Konfidenzbalkenz
icm memoir export -m "system-architecture" -f dot      # Graphviz DOT (Farbe = Konfidenzgrad)
icm memoir export -m "system-architecture" -f ai       # Markdown optimiert für LLM-Kontext
icm memoir export -m "system-architecture" -f json     # Strukturiertes JSON mit allen Metadaten

# SVG-Visualisierung erzeugen
icm memoir export -m "system-architecture" -f dot | dot -Tsvg > graph.svg
```

## MCP-Tools (22)

### Gedächtnis-Tools

| Tool | Beschreibung |
|------|--------------|
| `icm_memory_store` | Speichern mit Auto-Deduplizierung (>85% Ähnlichkeit → Aktualisierung statt Duplikat) |
| `icm_memory_recall` | Suche nach Abfrage, Filter nach Thema und/oder Schlüsselwort |
| `icm_memory_update` | Erinnerung direkt bearbeiten (Inhalt, Wichtigkeit, Schlüsselwörter) |
| `icm_memory_forget` | Erinnerung anhand ID löschen |
| `icm_memory_consolidate` | Alle Erinnerungen eines Themas zu einer Zusammenfassung zusammenführen |
| `icm_memory_list_topics` | Alle Themen mit Anzahl auflisten |
| `icm_memory_stats` | Globale Gedächtnisstatistiken |
| `icm_memory_health` | Hygiene-Prüfung pro Thema (Veralterung, Konsolidierungsbedarf) |
| `icm_memory_embed_all` | Fehlende Embeddings für die Vektorsuche nachträglich erzeugen |

### Memoir-Tools (Wissensgraphen)

| Tool | Beschreibung |
|------|--------------|
| `icm_memoir_create` | Neues Memoir erstellen (Wissenscontainer) |
| `icm_memoir_list` | Alle Memoirs auflisten |
| `icm_memoir_show` | Memoir-Details und alle Konzepte anzeigen |
| `icm_memoir_add_concept` | Konzept mit Labels hinzufügen |
| `icm_memoir_refine` | Definition eines Konzepts aktualisieren |
| `icm_memoir_search` | Volltextsuche, optional nach Label gefiltert |
| `icm_memoir_search_all` | Über alle Memoirs hinweg suchen |
| `icm_memoir_link` | Typisierte Relation zwischen Konzepten erstellen |
| `icm_memoir_inspect` | Konzept und Graph-Nachbarschaft inspizieren (BFS) |
| `icm_memoir_export` | Graphen exportieren (json, dot, ascii, ai) mit Konfidenzgraden |

### Feedback-Tools (Lernen aus Fehlern)

| Tool | Beschreibung |
|------|--------------|
| `icm_feedback_record` | Korrektur aufzeichnen, wenn eine KI-Vorhersage falsch war |
| `icm_feedback_search` | Vergangene Korrekturen durchsuchen, um künftige Vorhersagen zu verbessern |
| `icm_feedback_stats` | Feedback-Statistiken: Gesamtanzahl, Aufschlüsselung nach Thema, am häufigsten angewandt |

### Relationstypen

`part_of` · `depends_on` · `related_to` · `contradicts` · `refines` · `alternative_to` · `caused_by` · `instance_of` · `superseded_by`

## Funktionsweise

### Duales Gedächtnismodell

Das **episodische Gedächtnis (Topics)** erfasst Entscheidungen, Fehler und Präferenzen. Jede Erinnerung hat ein Gewicht, das im Laufe der Zeit je nach Wichtigkeit abnimmt:

| Wichtigkeit | Verfall | Bereinigung | Verhalten |
|-------------|---------|-------------|-----------|
| `critical` | keiner | nie | Wird nie vergessen, nie bereinigt |
| `high` | langsam (0,5× Rate) | nie | Verblasst langsam, wird nie automatisch gelöscht |
| `medium` | normal | ja | Standardverfall, bereinigt wenn Gewicht < Schwellenwert |
| `low` | schnell (2× Rate) | ja | Wird schnell vergessen |

Der Verfall ist **zugriffsbewusst**: häufig abgerufene Erinnerungen verblassen langsamer (`decay / (1 + access_count × 0.1)`). Wird automatisch beim Abrufen angewendet (wenn >24h seit dem letzten Verfall).

**Gedächtnis-Hygiene** ist eingebaut:
- **Auto-Deduplizierung**: Wird Inhalt mit >85% Ähnlichkeit zu einer bestehenden Erinnerung im selben Thema gespeichert, wird diese aktualisiert statt ein Duplikat zu erstellen
- **Konsolidierungshinweise**: Überschreitet ein Thema 7 Einträge, warnt `icm_memory_store` den Aufrufer zur Konsolidierung
- **Hygiene-Prüfung**: `icm_memory_health` meldet Eintragsanzahl, durchschnittliches Gewicht, veraltete Einträge und Konsolidierungsbedarf pro Thema
- **Kein stiller Datenverlust**: Kritische und hochwertige Erinnerungen werden nie automatisch bereinigt

Das **semantische Gedächtnis (Memoirs)** erfasst strukturiertes Wissen als Graphen. Konzepte sind permanent — sie werden verfeinert, nicht veraltet. Verwenden Sie `superseded_by`, um veraltete Fakten zu kennzeichnen, anstatt sie zu löschen.

### Hybridsuche

Mit aktivierten Embeddings verwendet ICM Hybridsuche:
- **FTS5 BM25** (30%) — Volltextstichwortsuche
- **Kosinus-Ähnlichkeit** (70%) — Semantische Vektorsuche via sqlite-vec

Standardmodell: `intfloat/multilingual-e5-base` (768d, 100+ Sprachen). Konfigurierbar in der [Konfigurationsdatei](#konfiguration):

```toml
[embeddings]
# enabled = false                          # Vollständig deaktivieren (kein Modell-Download)
model = "intfloat/multilingual-e5-base"    # 768d, mehrsprachig (Standard)
# model = "intfloat/multilingual-e5-small" # 384d, mehrsprachig (leichter)
# model = "intfloat/multilingual-e5-large" # 1024d, mehrsprachig (beste Genauigkeit)
# model = "Xenova/bge-small-en-v1.5"      # 384d, nur Englisch (schnellstes)
# model = "jinaai/jina-embeddings-v2-base-code"  # 768d, code-optimiert
```

Um den Embedding-Modell-Download vollständig zu überspringen, verwenden Sie eine der folgenden Optionen:
```bash
icm --no-embeddings serve          # CLI-Flag
ICM_NO_EMBEDDINGS=1 icm serve     # Umgebungsvariable
```
Oder setzen Sie `enabled = false` in Ihrer Konfigurationsdatei. ICM fällt auf FTS5-Schlüsselwortsuche zurück (funktioniert weiterhin, nur ohne semantisches Matching).

Durch Ändern des Modells wird der Vektorindex automatisch neu erstellt (vorhandene Embeddings werden gelöscht und können mit `icm_memory_embed_all` neu erzeugt werden).

### Speicherung

Einzelne SQLite-Datei. Keine externen Dienste, keine Netzwerkabhängigkeit.

```
~/Library/Application Support/dev.icm.icm/memories.db                    # macOS
~/.local/share/dev.icm.icm/memories.db                                   # Linux
C:\Users\<user>\AppData\Local\icm\icm\data\memories.db                   # Windows
```

### Konfiguration

```bash
icm config                    # Aktive Konfiguration anzeigen
```

Speicherort der Konfigurationsdatei (plattformspezifisch oder `$ICM_CONFIG`):

```
~/Library/Application Support/dev.icm.icm/config.toml                    # macOS
~/.config/icm/config.toml                                                # Linux
C:\Users\<user>\AppData\Roaming\icm\icm\config\config.toml              # Windows
```

Alle Optionen finden Sie unter [config/default.toml](config/default.toml).

## Auto-Extraktion

ICM extrahiert Erinnerungen automatisch über drei Ebenen:

```
  Ebene 0: Pattern-Hooks            Ebene 1: PreCompact           Ebene 2: UserPromptSubmit
  (keine LLM-Kosten)                (keine LLM-Kosten)            (keine LLM-Kosten)
  ┌──────────────────┐                ┌──────────────────┐          ┌──────────────────┐
  │ PostToolUse hook  │                │ PreCompact hook   │          │ UserPromptSubmit  │
  │                   │                │                   │          │                   │
  │ • Bash-Fehler     │                │ Kontext wird      │          │ Nutzer sendet     │
  │ • git commits     │                │ komprimiert →     │          │ Eingabe           │
  │ • Konfig-Änder.   │                │ Erinnerungen aus  │          │ → icm recall      │
  │ • Entscheidungen  │                │ Transkript         │          │ → Kontext einfügen│
  │ • Präferenzen     │                │ extrahieren bevor │          │                   │
  │ • Erkenntnisse    │                │ sie verloren gehen│          │ Agent startet mit  │
  │ • Einschränkungen │                │                   │          │ relevanten Erinne-│
  │                   │                │ Gleiche Muster +  │          │ rungen geladen    │
  │ Regelbasiert,     │                │ --store-raw Fallbk│          │                   │
  │ kein LLM          │                │                   │          │                   │
  └──────────────────┘                └──────────────────┘          └──────────────────┘
```

| Ebene | Status | LLM-Kosten | Hook-Befehl | Beschreibung |
|-------|--------|------------|-------------|--------------|
| Ebene 0 | Implementiert | 0 | `icm hook post` | Regelbasierte Schlüsselwortextraktion aus Tool-Ausgaben |
| Ebene 1 | Implementiert | 0 | `icm hook compact` | Extraktion aus Transkript vor der Kontextkomprimierung |
| Ebene 2 | Implementiert | 0 | `icm hook prompt` | Erinnerungen bei jeder Nutzereingabe einfügen |

Alle 3 Ebenen werden automatisch durch `icm init --mode hook` installiert.

### Vergleich mit Alternativen

| System | Methode | LLM-Kosten | Latenz | Erfasst Komprimierung? |
|--------|---------|------------|--------|------------------------|
| **ICM** | 3-Ebenen-Extraktion | 0 bis ~500 Tok/Sitzung | 0ms | **Ja (PreCompact)** |
| Mem0 | 2 LLM-Aufrufe/Nachricht | ~2k Tok/Nachricht | 200-2000ms | Nein |
| claude-mem | PostToolUse + async | ~1-5k Tok/Sitzung | 8ms Hook | Nein |
| MemGPT/Letta | Agent selbstverwaltend | 0 marginal | 0ms | Nein |
| DiffMem | Git-basierte Diffs | 0 | 0ms | Nein |

## Benchmarks

### Speicherleistung

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

Apple M1 Pro, In-Memory-SQLite, single-threaded. `icm bench --count 1000`

### Agenteneffizienz

Mehrere Sitzungen mit einem echten Rust-Projekt (12 Dateien, ~550 Zeilen). Ab Sitzung 2 zeigen sich die größten Gewinne, da ICM abruft statt Dateien erneut zu lesen.

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

### Wissenserhalt

Der Agent ruft spezifische Fakten aus einem dichten technischen Dokument über mehrere Sitzungen ab. Sitzung 1 liest und memoriert; Sitzung 2+ beantwortet 10 Sachfragen **ohne** den Quellentext.

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

### Lokale LLMs (ollama)

Gleicher Test mit lokalen Modellen — reine Kontextinjektion, keine Tool-Nutzung erforderlich.

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

### Testprotokoll

Alle Benchmarks verwenden **echte API-Aufrufe** — keine Mocks, keine simulierten Antworten, keine gecachten Ergebnisse.

- **Agenten-Benchmark**: Erstellt ein echtes Rust-Projekt in einem temporären Verzeichnis. Führt N Sitzungen mit `claude -p --output-format json` aus. Ohne ICM: leere MCP-Konfiguration. Mit ICM: echter MCP-Server + Auto-Extraktion + Kontextinjektion.
- **Wissenserhalt**: Verwendet ein fiktives technisches Dokument (das „Meridian Protocol"). Bewertet Antworten durch Schlüsselwortabgleich mit erwarteten Fakten. 120s Zeitlimit pro Aufruf.
- **Isolation**: Jeder Lauf verwendet ein eigenes temporäres Verzeichnis und eine frische SQLite-Datenbank. Keine Sitzungspersistenz.

## Dokumentation

| Dokument | Beschreibung |
|----------|--------------|
| [Technische Architektur](docs/architecture.md) | Crate-Struktur, Such-Pipeline, Verfallsmodell, sqlite-vec-Integration, Tests |
| [Benutzerhandbuch](docs/guide.md) | Installation, Themenorganisation, Konsolidierung, Extraktion, Fehlerbehebung |
| [Produktübersicht](docs/product.md) | Anwendungsfälle, Benchmarks, Vergleich mit Alternativen |

## Lizenz

[Source-Available](LICENSE) — Kostenlos für Einzelpersonen und Teams mit bis zu 20 Personen. Für größere Organisationen ist eine Unternehmenslizenz erforderlich. Kontakt: contact@rtk-ai.app
