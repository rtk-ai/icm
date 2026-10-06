[English](README.md) | [Français](README_fr.md) | [Español](README_es.md) | [Deutsch](README_de.md) | [Italiano](README_it.md) | [Português](README_pt.md) | [Nederlands](README_nl.md) | [Polski](README_pl.md) | [Русский](README_ru.md) | [日本語](README_ja.md) | [中文](README_zh.md) | [العربية](README_ar.md) | [한국어](README_ko.md)

Dies ist eine Übersetzung von README.md (Englisch), die als Referenz gilt; bei Abweichungen ist die englische Fassung maßgeblich.

<h1 align="center">ICM</h1>

<p align="center">
  <b>Langzeitgedächtnis für KI-Coding-Agenten, gemeinsam genutzt von all deinen Tools.</b><br>
  Ein Binary, eine SQLite-Datei. Kein LLM-Aufruf, um eine Erinnerung zu speichern oder abzurufen.
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/icm/actions/workflows/ci.yml"><img src="https://github.com/rtk-ai/icm/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/icm/releases/latest"><img src="https://img.shields.io/github/v/release/rtk-ai/icm?color=purple" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-Apache--2.0-blue.svg" alt="Apache-2.0"></a>
</p>

Erkläre Claude Code am Montag, wie dein Projekt die Authentifizierung handhabt, und die Gemini-CLI-Session am Dienstag weiß es bereits. ICM speichert, was deine Coding-Agenten lernen (Entscheidungen, Fixes, Konventionen, Präferenzen), in einer einzigen SQLite-Datei auf deinem Rechner und gibt den relevanten Teil zu Beginn jeder Session und mit jedem Prompt, den du sendest, wieder zurück. Bis zu 18 Agenten und Editoren teilen sich dieses Gedächtnis, sodass du dein Projekt nicht jedes Mal neu erklären musst, wenn du eine Session öffnest oder das Tool wechselst.

- **92.8% auf LoCoMo (1,540 Fragen), gleichauf mit Hindsight (92.0%)**, mit einem Drittel weniger Kontext pro Frage. [Details und Einschränkungen weiter unten](#benchmark-comparison).
- **Kein LLM-Aufruf zum Speichern oder Abrufen.** Hindsight, Mem0, Graphiti (Zep) und claude-mem rufen standardmäßig für jede gespeicherte Erinnerung ein LLM auf. ICM nicht. Nur die automatische Extraktion von Fakten aus Tool-Ausgaben läuft über das LLM-Kommandozeilentool, das du bereits verwendest, sofern eines installiert ist; `provider = "none"` hält auch das lokal (siehe [Schnellstart](#quickstart)).
- **Auch ohne Embedding-Modell nützlich.** Allein der Keyword-Recall bringt für 88.6% der LoCoMo-Fragen mindestens eine der richtigen Sessions in die Top 5, mit einem Median von 6.8 ms pro Abruf unter Linux x86-64.
- **Nicht überall vorn.** Auf PersonaMem (die sich verändernden Präferenzen eines Nutzers) liegt Hindsight vorn: 86.6% gegenüber 82.9% für ICM.

<p align="center">
  <img src="assets/demo.svg" alt="Terminal: drei Erinnerungen werden mit icm store gespeichert, dann beantwortet icm recall zwei Fragen und liefert jeweils die richtige Erinnerung">
</p>

<a id="quickstart"></a>

## Schnellstart

```bash
brew tap rtk-ai/tap && brew install icm    # or: curl -fsSL https://raw.githubusercontent.com/rtk-ai/icm/main/install.sh | sh
icm init                                   # instructions, skills and hooks for every agent it detects
```

Das ist die gesamte Einrichtung. Öffne eine neue Session in Claude Code, Codex, Gemini CLI oder Copilot CLI: Dein Agent startet jetzt mit einem kurzen Paket seiner wichtigsten Erinnerungen und erhält zu jedem Prompt, den du sendest, die relevanten Erinnerungen. Was er aus den Ausgaben seiner Tools lernt, kommt in eine Warteschlange, und diese wird am Ende jeder Claude-Code-Session in Erinnerungen umgewandelt; bei den anderen Tools führst du `icm extract-pending` aus (zum Beispiel per Cronjob).

Speichern und Abrufen bleiben auf deinem Rechner. Die automatische Extraktion übergibt Text an das LLM-Kommandozeilentool, das du bereits verwendest (Claude Code, Codex oder Gemini CLI), sofern eines installiert ist; setze `provider = "none"` unter `[extraction.summarizer]`, damit sie vollständig lokal bleibt.

Um es sofort in Aktion zu sehen, speichere und rufe von Hand ab:

```console
$ icm store -t decisions-myapp -c "Auth uses short-lived JWTs, refreshed through /auth/refresh" -i high
Stored: 01M47YY6CHVZ48BYKQRCRNZTCF

$ icm recall "how does auth work"
memories[1]{id,topic,importance,weight,summary}:
  01M47YY6CHVZ48BYKQRCRNZTCF,decisions-myapp,high,0.975,"Auth uses short-lived JWTs, refreshed through /auth/refresh"
```

Das erste Speichern oder Abrufen mit semantischer Suche lädt einmalig das mehrsprachige Embedding-Modell herunter (`Qdrant/multilingual-e5-large-onnx`, etwa 2 GB). Um ICM ohne das Modell auszuprobieren, füge `--no-embeddings` hinzu (Keyword-Recall wie in der Ausgabe oben) oder wähle in der Konfiguration ein leichteres Modell. `icm init` schreibt Hooks und Anweisungen in die Konfiguration jedes erkannten Agenten; `icm uninstall --dry-run` zeigt, wie man sie wieder entfernt. Windows, Linux, Nix und Bauen aus dem Quellcode: [Installation](#install).

<a id="benchmark-comparison"></a>

## Benchmark-Vergleich

Antwortgenauigkeit auf [LoCoMo](https://github.com/snap-research/locomo) (10 lange Konversationen, 1,540 Fragen), gemessen mit dem öffentlichen Harness [Agent Memory Benchmark](https://github.com/vectorize-io/agent-memory-benchmark): Das Gedächtnissystem ruft den Kontext ab, `gemini-3.1-pro-preview` antwortet auf dieser Grundlage, `gemini-2.5-flash-lite` bewertet die Antwort.

| System | LoCoMo-Genauigkeit | Kontext pro Frage | LLM-Aufrufe zum Speichern einer Erinnerung | Läuft als | Ergebnis |
|--------|:---------------:|:--------------------:|:---------------------------:|---------|--------|
| **ICM** 0.10.65 (Recall-Engine v2) | **92.8%** (1,429 / 1,540) | 24.1k Tokens | keine | ein Rust-Binary, SQLite-Datei | unser Lauf, 2026-10-05 |
| Hindsight | 92.0% (1,417 / 1,540) | 36.2k Tokens | Faktenextraktion per LLM | Python-Dienst, PostgreSQL + pgvector | vom Harness veröffentlicht |
| Baseline hybride Suche (dense + sparse, RRF) | 79.1% (1,218 / 1,540) | 22.2k Tokens | keine | Qdrant | vom Harness veröffentlicht |

Auf [PersonaMem](https://arxiv.org/abs/2504.14225) 32k (589 Multiple-Choice-Fragen zu den sich verändernden Präferenzen eines Nutzers, gleicher Harness und gleiches Antwortmodell, bewertet per Buchstabenabgleich):

| System | PersonaMem-Genauigkeit | Kontext pro Frage | Ergebnis |
|--------|:-------------------:|:--------------------:|--------|
| **ICM** 0.10.65 (Recall-Engine v2) | **82.9%** (488 / 589) | 16.2k Tokens | unser Lauf, 2026-10-05 |
| Hindsight | 86.6% (510 / 589) | 15.8k Tokens | vom Harness veröffentlicht |
| Baseline hybride Suche | 84.4% (497 / 589) | 24.2k Tokens | vom Harness veröffentlicht |

Nur das Retrieval, auf LoCoMo, ohne Antwortmodell: der Anteil der Fragen, bei denen mindestens eine Gold-Session unter den Top-Ergebnissen ist. Das ist der Beleg für die Recall-Engine selbst. Die v2-Läufe erhielten das Datum jeder Session und das Datum der Frage; die vorherige Engine hat keinen Datums-Input, daher erhielten ihre Läufe keines.

| Top-Ergebnisse | Vorherige Engine (`legacy`) | Recall-Engine v2 | Vorherige Engine, ohne Embedding-Modell | Recall-Engine v2, ohne Embedding-Modell |
|:-----------:|:--------------:|:----------------:|:--------------:|:----------------:|
| 5 | 76.5% | **86.7%** | 12.0% | **88.6%** |
| 10 | 83.0% | **93.3%** | 17.4% | **94.2%** |
| 20 | 87.7% | **97.9%** | 29.8% | **97.5%** |

Was diese Zahlen zeigen und was nicht:

- **ICM und Hindsight liegen auf LoCoMo gleichauf.** Der Abstand von 0.8 Punkten entspricht 12 Fragen und liegt innerhalb des Stichprobenfehlers (95%-Intervall für ICM: 91.5 bis 94.1). ICM erreicht das mit einem Drittel weniger Kontext und ohne LLM-Aufruf beim Speichern einer Erinnerung.
- **Auf PersonaMem liegt Hindsight vorn**, um 3.7 Punkte; ICM liegt gleichauf mit der Baseline der hybriden Suche (95%-Intervall für ICM: 79.8 bis 85.9) und liest dabei ein Drittel weniger Kontext als diese. Die schwächste Kategorie von ICM ist dort das Vorschlagen neuer Ideen aus bekannten Präferenzen (59.1%).
- **Keine identischen Bedingungen.** Der Harness wird von Vectorize gepflegt, dem Anbieter von Hindsight. Die veröffentlichten LoCoMo-Ergebnisse stammen aus der Zeit vor einer Änderung, die die Temperatur für Antwort und Bewertung auf 0 gesetzt hat; unser Lauf verwendet den aktuellen Harness (Commit `f618ed7`) und Vertex AI.
- **Bei 50 Chunks wird ein großer Teil jeder Konversation zurückgegeben,** daher misst diese Genauigkeit auch das Antwortmodell. Die Retrieval-Tabelle ist der Beleg für die Recall-Engine selbst.
- **Bisher ein Lauf pro Datensatz.** Die 95%-Intervalle oben decken die Stichprobe der Fragen ab, nicht die Schwankung von einem Lauf zum nächsten.

<details>
<summary>Ergebnisse pro Kategorie, Latenz und weitere Einschränkungen</summary>

- **Nach Fragetyp** (LoCoMo, Labels des Harness): open-domain 96.7% (813 / 841), temporal 91.0% (292 / 321), single-hop 89.0% (251 / 282), multi-hop 76.0% (73 / 96).
- **Latenz.** Median-Latenz beim Abruf 141 ms auf einer Cloud-VM mit 4 vCPUs während des obigen Laufs; 272 Sessions ohne LLM-Aufruf eingelesen. In den reinen Retrieval-Läufen unter Linux x86-64: Median 136 ms mit Embeddings, 6.8 ms nur mit Keywords.
- **Session-Daten.** Der Benchmark-Adapter schreibt das Datum jeder Session in den Erinnerungstext von ICM; Hindsight erhält dieselben Daten als Metadaten, und jedes System bekommt das Datum der Frage.
- **Die schwächste Kategorie sind die 96 Fragen, die der Harness als multi-hop kennzeichnet** (76.0%). Die Kategorienamen stimmen zwischen Benchmarks nicht überein: Andere LoCoMo-Auswertungen nennen diese Kategorie open-domain und nennen multi-hop die 282 Fragen, die der Harness als single-hop kennzeichnet (hier 89.0%). Vergleiche nach Anzahl der Fragen, nicht nach Label.
- **Recall-Engine v2 ist der Standard** für `icm recall`, das MCP-Tool `icm_memory_recall`, HTTP `/recall` und den Prompt-Hook. Die vorherige Engine bleibt verfügbar, um zurückzuwechseln oder zu vergleichen: `icm recall --engine legacy`, `"engine": "legacy"` bei HTTP `/recall` oder `ICM_RECALL_ENGINE=legacy`.

</details>

Die Ergebnisse pro Frage für beide Datensätze liegen in [`bench/amb/results/`](bench/amb/results/); der Adapter, die genauen Einstellungen und die Befehle zum Reproduzieren stehen in [`bench/amb/README.md`](bench/amb/README.md).

<a id="one-memory-for-every-tool"></a>

## Ein Gedächtnis für alle Tools

Jedes von `icm init` konfigurierte Tool liest und schreibt dieselbe SQLite-Datenbank, und Topics (`decisions-myapp`, `preferences`, `errors-resolved`, ...) sind nicht nach Tool getrennt. Eine aus Claude Code gespeicherte Erinnerung ist sofort für Codex, Gemini, Cursor, Roo, Amp, Aider, ... sichtbar.

Lieber isoliert? `icm init --per-project` legt eine projektlokale Datenbank unter `.icm/` an (und schreibt die Anweisungsdateien der Agenten, etwa `CLAUDE.md` und `AGENTS.md`, ins aktuelle Verzeichnis); `--db <path>` oder `ICM_DB` verweisen auf eine beliebige andere Datei. Jeder Pfad ist ein unabhängiger Korpus.

> **Projektstatus: Beta.** ICM ist pre-1.0: Breaking Changes können in jedem Minor-Release kommen, und die Konfigurationsformate für Hooks und MCP können sich ändern. Beta bezieht sich auf die API-Stabilität, nicht auf den Nutzen im Alltag: Ich (der Maintainer) nutze ICM jeden Tag als mein primäres Gedächtnis beim Programmieren mit KI. Mein Hauptfokus ist [rtk](https://github.com/rtk-ai/rtk), daher werden Issues und Pull Requests nach bestem Bemühen geprüft.
>
> Apache-2.0, ausgeliefert **wie besehen, ohne jegliche Gewährleistung** (siehe [LICENSE](LICENSE)). Führe vor jeder destruktiven Operation zuerst das schreibgeschützte Gegenstück aus (`icm uninstall --dry-run`, `icm uninstall --check`).

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

Die Keyword-Suche funktioniert überall. Führe `icm embeddings status` aus, um zu sehen, ob die semantische Suche aktiv ist: Sie ist in den Builds für macOS Apple Silicon, Windows und `.rpm` enthalten; die Linux-glibc-Archive und das `.deb` brauchen einmal `icm embeddings download`; der Build für Intel-Macs braucht eine eigene ONNX Runtime (`ORT_DYLIB_PATH`); der statische Linux-musl-Build unterstützt nur die Keyword-Suche. Nix, Bauen aus dem Quellcode, Versions-Pinning und die Details: [Referenz](docs/reference.md#install).

<a id="setup"></a>

## Einrichtung

```bash
icm init                  # global database, every detected agent
icm init --per-project    # database under .icm/ at the git root
icm init --mode all       # also register the MCP server in every tool that supports it
```

Der Standardmodus (`standard`) schreibt Anweisungen, Skills und Hooks, ohne MCP-Server. `--mode all` fügt den MCP-Server hinzu; damit (plus `--per-project` für Aider, dessen Konventionsdatei pro Projekt gilt) sind die 18 Tools unten abgedeckt ([Integrationsleitfaden](docs/integrations.md)):

| Tool | MCP-Server | Hooks |
|------|:---:|:-----:|
| Claude Code | ja | ja |
| Claude Desktop | ja | — |
| Gemini CLI | ja | ja |
| Codex CLI | ja | ja |
| Copilot CLI | ja | ja |
| Cursor | ja | — |
| Windsurf | ja | — |
| VS Code | ja | — |
| Amp | ja | — |
| Amazon Q | ja | — |
| Cline | ja | — |
| Roo Code | ja | — |
| Kilo Code | ja | — |
| Zed | ja | — |
| OpenCode | ja | ja |
| Continue.dev | ja | — |
| Aider | — | — |
| Pi | — | — |

Oder registriere den MCP-Server von Hand: `claude mcp add icm -- icm serve` (jeder MCP-Client: Befehl `icm`, Argumente `["serve"]`).

Was die Hooks tun:

| Hook | Was er tut |
|------|-------------|
| `icm hook start` | Fügt zu Beginn der Session ein Wake-up-Paket mit critical/high-Erinnerungen ein (~500 Tokens) |
| `icm hook pre` | Erlaubt `icm`-CLI-Befehle automatisch (keine Berechtigungsabfrage) |
| `icm hook post` | Extrahiert alle N Aufrufe Fakten aus Tool-Ausgaben (automatische Extraktion) |
| `icm hook compact` | Extrahiert Erinnerungen aus dem Transkript vor der Kontextkomprimierung |
| `icm hook prompt` | Fügt zu Beginn jedes Nutzer-Prompts abgerufenen Kontext ein |

Hook-Tabellen pro Tool, Skills, Anweisungsdateien und der Hinweis zu Codex: [Referenz](docs/reference.md#setup).

<a id="use"></a>

## Verwendung

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

ICM verwaltet außerdem **Memoirs** (dauerhafte Wissensgraphen aus Konzepten und typisierten Beziehungen), **Feedback** (Korrekturen, aus denen gelernt wird) und **wörtliche Transkripte**, und stellt **31 MCP-Tools** (30 ohne Embedding-Modell), eine **HTTP-API**, die das Embedding-Modell geladen hält, sowie ein **Terminal-Dashboard** (`icm dashboard`) bereit. All das steht in der [Referenz](docs/reference.md).

<a id="how-it-works"></a>

## Funktionsweise

Der Recall führt bis zu drei Ranglisten per Reciprocal Rank Fusion (RRF) zusammen: **FTS5 BM25**-Keyword-Matching, immer aktiv; **semantische Vektorsuche** über sqlite-vec, wenn ein Embedding-Modell geladen ist (standardmäßig `Qdrant/multilingual-e5-large-onnx`, 1024 Dimensionen, 100+ Sprachen); und ein **Datumsfenster**, wenn die Anfrage einen Zeitraum nennt ("last week", "in March 2024"). Projekt-, Topic- und Keyword-Filter greifen vor dem Abschneiden. Erinnerungen verblassen mit der Zeit je nach ihrer Wichtigkeit (`critical` verblasst nie); mit geladenem Embedding-Modell wird eine neue Erinnerung, die einer anderen im selben Topic fast gleicht (Kosinus-Ähnlichkeit über 0.95), mit dieser zusammengeführt; und das Modell, das die gespeicherten Vektoren erzeugt hat, wird in der Datenbank vermerkt, sodass eine Änderung von `model` in der Konfiguration sie nie löscht (`icm embed --migrate` ist der explizite Weg zum Wechseln).

Alles liegt in einer einzigen SQLite-Datei, ohne externen Dienst:

```
~/Library/Application Support/dev.icm.icm/memories.db     # macOS
~/.local/share/icm/memories.db                            # Linux
%APPDATA%\icm\icm\data\memories.db                        # Windows
<project-root>/.icm/memories.db                           # icm init --per-project
```

`icm config` zeigt die aktive Konfiguration; [config/default.toml](config/default.toml) listet alle Optionen auf. Details: [Referenz](docs/reference.md#how-it-works).

<a id="documentation"></a>

## Dokumentation

| Dokument | Beschreibung |
|----------|-------------|
| [Integrationsleitfaden](docs/integrations.md) | MCP-Einrichtung pro Tool: Claude Code, Cursor, Windsurf, Zed, Amp, Codex, Cline, Roo Code usw. |
| [Technische Architektur](docs/architecture.md) | Crate-Struktur, Such-Pipeline, Decay-Modell, sqlite-vec-Integration, Tests |
| [Benutzerhandbuch](docs/guide.md) | Installation, Organisation der Topics, Konsolidierung, Extraktion, Fehlerbehebung |
| [Produktüberblick](docs/product.md) | Anwendungsfälle, Benchmarks, Vergleich mit Alternativen |
| [Referenz](docs/reference.md) | Installationsoptionen, Einrichtung pro Tool, CLI, 31 MCP-Tools, HTTP-API, Dashboard, Interna |
| [Benchmark-Adapter](bench/amb/README.md) | Wie der Vergleich oben durchgeführt wurde und wie man ihn reproduziert |
| [Demonstrationen](docs/demonstrations.md) | Speicher-Mikrobenchmarks und kleine Demos |

<a id="license"></a>

## Lizenz

[Apache-2.0](LICENSE)
