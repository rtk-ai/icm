[English](README.md) | [Français](README_fr.md) | [Español](README_es.md) | [Deutsch](README_de.md) | [Italiano](README_it.md) | [Português](README_pt.md) | **Nederlands** | [Polski](README_pl.md) | [Русский](README_ru.md) | [日本語](README_ja.md) | [中文](README_zh.md) | [العربية](README_ar.md) | [한국어](README_ko.md)

<p align="center">
  <img src="assets/banner.png" alt="ICM — Infinite Context Memory" width="600">
</p>

<h1 align="center">ICM</h1>

<p align="center">
  Permanent geheugen voor AI-agenten. Één binair bestand, geen afhankelijkheden, native MCP.
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/icm/actions/workflows/ci.yml"><img src="https://github.com/rtk-ai/icm/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/icm/releases/latest"><img src="https://img.shields.io/github/v/release/rtk-ai/icm?color=purple" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-Source--Available-orange.svg" alt="Source-Available"></a>
</p>

---

ICM geeft uw AI-agent een echt geheugen — geen notitietool, geen contextbeheerder, maar een **geheugen**.

```
                       ICM (Infinite Context Memory)
            ┌──────────────────────┬─────────────────────────┐
            │   MEMORIES (Topics)  │   MEMOIRS (Knowledge)   │
            │                      │                         │
            │  Episodisch, tijdel. │  Permanent, gestructur. │
            │                      │                         │
            │  ┌───┐ ┌───┐ ┌───┐  │    ┌───┐               │
            │  │ m │ │ m │ │ m │  │    │ C │──depends_on──┐ │
            │  └─┬─┘ └─┬─┘ └─┬─┘  │    └───┘              │ │
            │    │decay │     │    │      │ refines      ┌─▼─┐│
            │    ▼      ▼     ▼    │    ┌─▼─┐            │ C ││
            │  gewicht neemt af    │    │ C │──part_of──>└───┘│
            │  over tijd tenzij    │    └───┘                 │
            │  benaderd/kritiek    │  Concepten + Relaties    │
            ├──────────────────────┴─────────────────────────┤
            │             SQLite + FTS5 + sqlite-vec          │
            │        Hybride zoekopdracht: BM25 (30%) + cosinus (70%) │
            └─────────────────────────────────────────────────┘
```

**Twee geheugenmodellen:**

- **Memories** — opslaan/ophalen met tijdelijk verval op basis van belang. Kritieke herinneringen vervagen nooit, herinneringen met lage prioriteit vervagen vanzelf. Filter op onderwerp of trefwoord.
- **Memoirs** — permanente kennisgrafen. Concepten gekoppeld door getypeerde relaties (`depends_on`, `contradicts`, `superseded_by`, ...). Filter op label.
- **Feedback** — correcies vastleggen wanneer AI-voorspellingen fout zijn. Zoek eerdere fouten op voordat nieuwe voorspellingen worden gedaan. Gesloten-lus leren.

## Installatie

```bash
# Homebrew (macOS / Linux)
brew tap rtk-ai/tap && brew install icm

# Vanuit broncode
cargo install --path crates/icm-cli
```

## Instelling

```bash
# Automatisch detecteren en alle ondersteunde tools configureren
icm init
```

Configureert **17 tools** in één opdracht ([volledige integratiegids](docs/integrations.md)):

| Tool | MCP | Hooks | CLI | Skills |
|------|:---:|:-----:|:---:|:------:|
| Claude Code | `~/.claude.json` | 5 hooks | `CLAUDE.md` | `/recall` `/remember` |
| Claude Desktop | JSON | — | — | — |
| Gemini CLI | `~/.gemini/settings.json` | 5 hooks | `GEMINI.md` | — |
| Codex CLI | `~/.codex/config.toml` | 4 hooks | `AGENTS.md` | — |
| Copilot CLI | `~/.copilot/mcp-config.json` | 4 hooks | `.github/copilot-instructions.md` | — |
| Cursor | `~/.cursor/mcp.json` | — | — | `.mdc`-regel |
| Windsurf | JSON | — | `.windsurfrules` | — |
| VS Code | `~/Library/.../Code/User/mcp.json` | — | — | — |
| Amp | JSON | — | — | `/icm-recall` `/icm-remember` |
| Amazon Q | JSON | — | — | — |
| Cline | VS Code globalStorage | — | — | — |
| Roo Code | VS Code globalStorage | — | — | `.md`-regel |
| Kilo Code | VS Code globalStorage | — | — | — |
| Zed | `~/.zed/settings.json` | — | — | — |
| OpenCode | JSON | TS-plugin | — | — |
| Continue.dev | `~/.continue/config.yaml` | — | — | — |
| Aider | — | — | `.aider.conventions.md` | — |

Of handmatig:

```bash
# Claude Code
claude mcp add icm -- icm serve

# Compacte modus (kortere antwoorden, bespaart tokens)
claude mcp add icm -- icm serve --compact

# Elke MCP-client: command = "icm", args = ["serve"]
```

### Skills / regels

```bash
icm init --mode skill
```

Installeert slash-commando's en regels voor Claude Code (`/recall`, `/remember`), Cursor (`.mdc`-regel), Roo Code (`.md`-regel) en Amp (`/icm-recall`, `/icm-remember`).

### CLI-instructies

```bash
icm init --mode cli
```

Injecteert ICM-instructies in het instructiebestand van elke tool:

| Tool | Bestand |
|------|---------|
| Claude Code | `CLAUDE.md` |
| GitHub Copilot | `.github/copilot-instructions.md` |
| Windsurf | `.windsurfrules` |
| OpenAI Codex | `AGENTS.md` |
| Gemini | `~/.gemini/GEMINI.md` |

### Hooks (5 tools)

```bash
icm init --mode hook
```

Installeert hooks voor automatische extractie en ophaling voor alle ondersteunde tools:

| Tool | SessionStart | PreTool | PostTool | Compact | PromptRecall | Config |
|------|:-----------:|:-------:|:--------:|:-------:|:------------:|--------|
| Claude Code | `icm hook start` | `icm hook pre` | `icm hook post` | `icm hook compact` | `icm hook prompt` | `~/.claude/settings.json` |
| Gemini CLI | `icm hook start` | `icm hook pre` | `icm hook post` | `icm hook compact` | `icm hook prompt` | `~/.gemini/settings.json` |
| Codex CLI | `icm hook start` | `icm hook pre` | `icm hook post` | — | `icm hook prompt` | `~/.codex/hooks.json` |
| Copilot CLI | `icm hook start` | `icm hook pre` | `icm hook post` | — | `icm hook prompt` | `.github/hooks/icm.json` |
| OpenCode | sessiestart | — | tool-extractie | compactie | — | `~/.config/opencode/plugins/icm.ts` |

**Wat elke hook doet:**

| Hook | Wat het doet |
|------|--------------|
| `icm hook start` | Injecteert een opstartpakket met kritieke/belangrijke herinneringen bij sessiestart (~500 tokens) |
| `icm hook pre` | Automatisch `icm` CLI-opdrachten toestaan (geen toestemmingsprompt) |
| `icm hook post` | Feiten extraheren uit tool-uitvoer elke N aanroepen (automatische extractie) |
| `icm hook compact` | Herinneringen extraheren uit transcript vóór contextcompressie |
| `icm hook prompt` | Opgehaalde context injecteren aan het begin van elke gebruikersprompt |

## CLI versus MCP

ICM kan worden gebruikt via CLI (`icm`-opdrachten) of MCP-server (`icm serve`). Beide hebben toegang tot dezelfde database.

| | CLI | MCP |
|---|-----|-----|
| **Latentie** | ~30ms (direct binair bestand) | ~50ms (JSON-RPC stdio) |
| **Tokenkosten** | 0 (hook-gebaseerd, onzichtbaar) | ~20-50 tokens/aanroep (tool-schema) |
| **Instelling** | `icm init --mode hook` | `icm init --mode mcp` |
| **Werkt met** | Claude Code, Gemini, Codex, Copilot, OpenCode (via hooks) | Alle 17 MCP-compatibele tools |
| **Automatische extractie** | Ja (hooks activeren `icm extract`) | Ja (MCP-tools roepen store aan) |
| **Het beste voor** | Geavanceerde gebruikers, tokenbesparing | Universele compatibiliteit |

## CLI

### Memories (episodisch, met verval)

```bash
# Opslaan
icm store -t "mijn-project" -c "Gebruik PostgreSQL voor de hoofd-DB" -i high -k "db,postgres"

# Ophalen
icm recall "databasekeuze"
icm recall "auth-instelling" --topic "mijn-project" --limit 10
icm recall "architectuur" --keyword "postgres"

# Beheren
icm forget <memory-id>
icm consolidate --topic "mijn-project"
icm topics
icm stats

# Feiten extraheren uit tekst (regelgebaseerd, geen LLM-kosten)
echo "The parser uses Pratt algorithm" | icm extract -p mijn-project
```

### Memoirs (permanente kennisgrafen)

```bash
# Een memoir aanmaken
icm memoir create -n "systeem-architectuur" -d "Ontwerpbeslissingen voor het systeem"

# Concepten toevoegen met labels
icm memoir add-concept -m "systeem-architectuur" -n "auth-service" \
  -d "Verwerkt JWT-tokens en OAuth2-stromen" -l "domain:auth,type:service"

# Concepten koppelen
icm memoir link -m "systeem-architectuur" --from "api-gateway" --to "auth-service" -r depends-on

# Zoeken met labelfilter
icm memoir search -m "systeem-architectuur" "authenticatie"
icm memoir search -m "systeem-architectuur" "service" --label "domain:auth"

# Buurt inspecteren
icm memoir inspect -m "systeem-architectuur" "auth-service" -D 2

# Graaf exporteren (formaten: json, dot, ascii, ai)
icm memoir export -m "systeem-architectuur" -f ascii   # Lijntekening met betrouwbaarheidsbalken
icm memoir export -m "systeem-architectuur" -f dot      # Graphviz DOT (kleur = betrouwbaarheidsniveau)
icm memoir export -m "systeem-architectuur" -f ai       # Markdown geoptimaliseerd voor LLM-context
icm memoir export -m "systeem-architectuur" -f json     # Gestructureerde JSON met alle metadata

# SVG-visualisatie genereren
icm memoir export -m "systeem-architectuur" -f dot | dot -Tsvg > graph.svg
```

## MCP-tools (22)

### Geheugentools

| Tool | Beschrijving |
|------|--------------|
| `icm_memory_store` | Opslaan met automatische deduplicatie (>85% gelijkenis → bijwerken in plaats van dupliceren) |
| `icm_memory_recall` | Zoeken op query, filteren op onderwerp en/of trefwoord |
| `icm_memory_update` | Een herinnering ter plaatse bewerken (inhoud, belang, trefwoorden) |
| `icm_memory_forget` | Een herinnering verwijderen op ID |
| `icm_memory_consolidate` | Alle herinneringen van een onderwerp samenvoegen tot één samenvatting |
| `icm_memory_list_topics` | Alle onderwerpen met aantallen weergeven |
| `icm_memory_stats` | Globale geheugenstatistieken |
| `icm_memory_health` | Hygiëne-audit per onderwerp (veroudering, consolidatiebehoeften) |
| `icm_memory_embed_all` | Embeddings aanvullen voor vectorzoekopdrachten |

### Memoir-tools (kennisgrafen)

| Tool | Beschrijving |
|------|--------------|
| `icm_memoir_create` | Een nieuw memoir aanmaken (kenniscontainer) |
| `icm_memoir_list` | Alle memoirs weergeven |
| `icm_memoir_show` | Memoir-details en alle concepten weergeven |
| `icm_memoir_add_concept` | Een concept toevoegen met labels |
| `icm_memoir_refine` | De definitie van een concept bijwerken |
| `icm_memoir_search` | Volledige tekst zoeken, optioneel gefilterd op label |
| `icm_memoir_search_all` | Zoeken in alle memoirs |
| `icm_memoir_link` | Getypeerde relatie aanmaken tussen concepten |
| `icm_memoir_inspect` | Concept en grafiekbuurt inspecteren (BFS) |
| `icm_memoir_export` | Graaf exporteren (json, dot, ascii, ai) met betrouwbaarheidsniveaus |

### Feedback-tools (leren van fouten)

| Tool | Beschrijving |
|------|--------------|
| `icm_feedback_record` | Een correctie vastleggen wanneer een AI-voorspelling fout was |
| `icm_feedback_search` | Eerdere correcties zoeken om toekomstige voorspellingen te informeren |
| `icm_feedback_stats` | Feedbackstatistieken: totaal aantal, uitsplitsing per onderwerp, meest toegepast |

### Relatietypen

`part_of` · `depends_on` · `related_to` · `contradicts` · `refines` · `alternative_to` · `caused_by` · `instance_of` · `superseded_by`

## Hoe het werkt

### Dubbel geheugenmodel

**Episodisch geheugen (Onderwerpen)** legt beslissingen, fouten en voorkeuren vast. Elke herinnering heeft een gewicht dat na verloop van tijd vervalt op basis van belang:

| Belang | Verval | Verwijdering | Gedrag |
|--------|--------|--------------|--------|
| `critical` | geen | nooit | Nooit vergeten, nooit verwijderd |
| `high` | langzaam (0,5x snelheid) | nooit | Vervagt langzaam, nooit automatisch verwijderd |
| `medium` | normaal | ja | Standaard verval, verwijderd wanneer gewicht < drempelwaarde |
| `low` | snel (2x snelheid) | ja | Snel vergeten |

Verval is **toegangsbewust**: vaak opgehaalde herinneringen vervallen langzamer (`decay / (1 + access_count × 0.1)`). Automatisch toegepast bij ophalen (als >24u verstreken sinds laatste verval).

**Geheugen­hygiëne** is ingebouwd:
- **Automatische deduplicatie**: inhoud opslaan die >85% vergelijkbaar is met een bestaande herinnering in hetzelfde onderwerp werkt deze bij in plaats van een duplicaat aan te maken
- **Consolidatiehints**: wanneer een onderwerp meer dan 7 vermeldingen heeft, waarschuwt `icm_memory_store` de aanroeper om te consolideren
- **Hygiëne-audit**: `icm_memory_health` rapporteert per onderwerp het aantal vermeldingen, het gemiddelde gewicht, verouderde vermeldingen en consolidatiebehoeften
- **Geen stille gegevensverlies**: kritieke herinneringen en herinneringen met hoog belang worden nooit automatisch verwijderd

**Semantisch geheugen (Memoirs)** legt gestructureerde kennis vast als een graaf. Concepten zijn permanent — ze worden verfijnd, nooit afgebroken. Gebruik `superseded_by` om verouderde feiten te markeren in plaats van ze te verwijderen.

### Hybride zoeken

Met embeddings ingeschakeld gebruikt ICM hybride zoeken:
- **FTS5 BM25** (30%) — volledige tekst trefwoordmatch
- **Cosinus-gelijkenis** (70%) — semantisch vectorzoeken via sqlite-vec

Standaardmodel: `intfloat/multilingual-e5-base` (768d, 100+ talen). Configureerbaar in uw [configuratiebestand](#configuratie):

```toml
[embeddings]
# enabled = false                          # Volledig uitschakelen (geen model downloaden)
model = "intfloat/multilingual-e5-base"    # 768d, meertalig (standaard)
# model = "intfloat/multilingual-e5-small" # 384d, meertalig (lichter)
# model = "intfloat/multilingual-e5-large" # 1024d, meertalig (beste nauwkeurigheid)
# model = "Xenova/bge-small-en-v1.5"      # 384d, alleen Engels (snelste)
# model = "jinaai/jina-embeddings-v2-base-code"  # 768d, geoptimaliseerd voor code
```

Om het downloaden van het embedding-model volledig over te slaan, gebruik een van deze:
```bash
icm --no-embeddings serve          # CLI-vlag
ICM_NO_EMBEDDINGS=1 icm serve     # Omgevingsvariabele
```
Of stel `enabled = false` in in uw configuratiebestand. ICM valt terug op FTS5-trefwoordzoeken (werkt nog steeds, maar zonder semantische matching).

Het wijzigen van het model maakt de vectorindex automatisch opnieuw aan (bestaande embeddings worden gewist en kunnen opnieuw worden gegenereerd met `icm_memory_embed_all`).

### Opslag

Één SQLite-bestand. Geen externe diensten, geen netwerkafhankelijkheid.

```
~/Library/Application Support/dev.icm.icm/memories.db                    # macOS
~/.local/share/dev.icm.icm/memories.db                                   # Linux
C:\Users\<user>\AppData\Local\icm\icm\data\memories.db                   # Windows
```

### Configuratie

```bash
icm config                    # Actieve configuratie weergeven
```

Locatie van het configuratiebestand (platformspecifiek, of `$ICM_CONFIG`):

```
~/Library/Application Support/dev.icm.icm/config.toml                    # macOS
~/.config/icm/config.toml                                                # Linux
C:\Users\<user>\AppData\Roaming\icm\icm\config\config.toml              # Windows
```

Zie [config/default.toml](config/default.toml) voor alle opties.

## Automatische extractie

ICM extraheert herinneringen automatisch via drie lagen:

```
  Laag 0: Patroonhooks             Laag 1: PreCompact            Laag 2: UserPromptSubmit
  (geen LLM-kosten)                (geen LLM-kosten)             (geen LLM-kosten)
  ┌──────────────────┐                ┌──────────────────┐          ┌──────────────────┐
  │ PostToolUse-hook  │                │ PreCompact-hook   │          │ UserPromptSubmit  │
  │                   │                │                   │          │                   │
  │ • Bash-fouten     │                │ Context staat op  │          │ Gebruiker stuurt  │
  │ • git-commits     │                │ punt gecomprimeerd│          │ prompt → icm      │
  │ • config-wijzig.  │                │ te worden →       │          │ recall → context  │
  │ • beslissingen    │                │ herinneringen     │          │ injecteren        │
  │ • voorkeuren      │                │ extraheren uit    │          │                   │
  │ • lessen          │                │ transcript vóór   │          │ Agent begint met  │
  │ • beperkingen     │                │ ze voor altijd    │          │ relevante         │
  │                   │                │ verloren gaan     │          │ herinneringen     │
  │ Regelgebaseerd,   │                │                   │          │ al geladen        │
  │ geen LLM          │                │ Zelfde patronen + │          │                   │
  └──────────────────┘                │ --store-raw fallbk│          └──────────────────┘
                                      └──────────────────┘
```

| Laag | Status | LLM-kosten | Hook-opdracht | Beschrijving |
|------|--------|------------|---------------|--------------|
| Laag 0 | Geïmplementeerd | 0 | `icm hook post` | Regelgebaseerde trefwoordextractie uit tool-uitvoer |
| Laag 1 | Geïmplementeerd | 0 | `icm hook compact` | Extraheren uit transcript vóór contextcompressie |
| Laag 2 | Geïmplementeerd | 0 | `icm hook prompt` | Opgehaalde herinneringen injecteren bij elke gebruikersprompt |

Alle 3 lagen worden automatisch geïnstalleerd door `icm init --mode hook`.

### Vergelijking met alternatieven

| Systeem | Methode | LLM-kosten | Latentie | Legt compactie vast? |
|---------|---------|------------|----------|----------------------|
| **ICM** | 3-laags extractie | 0 tot ~500 tok/sessie | 0ms | **Ja (PreCompact)** |
| Mem0 | 2 LLM-aanroepen/bericht | ~2k tok/bericht | 200-2000ms | Nee |
| claude-mem | PostToolUse + async | ~1-5k tok/sessie | 8ms hook | Nee |
| MemGPT/Letta | Agent beheert zichzelf | 0 marginaal | 0ms | Nee |
| DiffMem | Git-gebaseerde diffs | 0 | 0ms | Nee |

## Benchmarks

### Opslagprestaties

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

Apple M1 Pro, in-memory SQLite, enkeldraads. `icm bench --count 1000`

### Agent-efficiëntie

Meersessie-workflow met een echt Rust-project (12 bestanden, ~550 regels). Sessie 2 en later tonen de grootste winsten naarmate ICM ophaalt in plaats van bestanden opnieuw te lezen.

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

### Kennisbehoud

Agent haalt specifieke feiten op uit een technisch document in meerdere sessies. Sessie 1 leest en onthoudt; sessies 2 en later beantwoorden 10 feitelijke vragen **zonder** de brontekst.

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

### Lokale LLM's (ollama)

Dezelfde test met lokale modellen — pure contextinjectie, geen tool-gebruik vereist.

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

### Testprotocol

Alle benchmarks gebruiken **echte API-aanroepen** — geen mocks, geen gesimuleerde antwoorden, geen gecachede antwoorden.

- **Agent-benchmark**: Maakt een echt Rust-project aan in een tijdelijke map. Voert N sessies uit met `claude -p --output-format json`. Zonder ICM: lege MCP-configuratie. Met ICM: echte MCP-server + automatische extractie + contextinjectie.
- **Kennisbehoud**: Gebruikt een fictief technisch document (het "Meridian Protocol"). Beoordeelt antwoorden op trefwoordmatch met verwachte feiten. Time-out van 120s per aanroep.
- **Isolatie**: Elke run gebruikt zijn eigen tijdelijke map en verse SQLite-database. Geen sessiepersistentie.

### Multi-agent gedeeld geheugen

Alle 17 tools delen dezelfde SQLite-database. Een herinnering opgeslagen door Claude is direct beschikbaar voor Gemini, Codex, Copilot, Cursor en elke andere tool.

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

Score = 60% ophaalnauwkeurigheid + 30% feitendetail + 10% snelheid. **98% multi-agent efficiëntie.**

## Waarom ICM

| Functionaliteit | ICM | Mem0 | Engram | AgentMemory |
|----------------|:---:|:----:|:------:|:-----------:|
| Toolondersteuning | **17** | Alleen SDK | ~6-8 | ~10 |
| Instelling met één opdracht | `icm init` | handmatige SDK | handmatig | handmatig |
| Hooks (automatisch ophalen bij start) | 5 tools | geen | via MCP | 1 tool |
| Hybride zoeken (FTS5 + vector) | 30/70 gewogen | alleen vector | alleen FTS5 | FTS5+vector |
| Meertalige embeddings | 100+ talen (768d) | afhankelijk | geen | Engels 384d |
| Kennisgraaf | Memoir-systeem | geen | geen | geen |
| Tijdelijk verval + consolidatie | toegangsbewust | geen | basis | basis |
| TUI-dashboard | `icm dashboard` | geen | ja | webviewer |
| Automatische extractie uit tool-uitvoer | 3 lagen, nul LLM | geen | geen | geen |
| Feedback-/correctielus | `icm_feedback_*` | geen | geen | geen |
| Runtime | Rust enkel binair bestand | Python | Go | Node.js |
| Local-first, geen afhankelijkheden | SQLite-bestand | cloud-first | SQLite | SQLite |
| Multi-agent ophaalnauwkeurigheid | **98%** | N/A | N/A | 95.2% |

## Documentatie

| Document | Beschrijving |
|----------|--------------|
| [Integratiegids](docs/integrations.md) | Instelling voor alle 17 tools: Claude Code, Copilot, Cursor, Windsurf, Zed, Amp, enz. |
| [Technische architectuur](docs/architecture.md) | Crate-structuur, zoekpijplijn, vervalmodel, sqlite-vec-integratie, testen |
| [Gebruikersgids](docs/guide.md) | Installatie, onderwerporganisatie, consolidatie, extractie, probleemoplossing |
| [Productoverzicht](docs/product.md) | Gebruiksscenario's, benchmarks, vergelijking met alternatieven |

## Licentie

[Apache-2.0](LICENSE)
