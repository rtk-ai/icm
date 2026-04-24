[English](README.md) | [Français](README_fr.md) | [Español](README_es.md) | [Deutsch](README_de.md) | **Italiano** | [Português](README_pt.md) | [Nederlands](README_nl.md) | [Polski](README_pl.md) | [Русский](README_ru.md) | [日本語](README_ja.md) | [中文](README_zh.md) | [العربية](README_ar.md) | [한국어](README_ko.md)

<p align="center">
  <img src="assets/banner.png" alt="ICM — Infinite Context Memory" width="600">
</p>

<h1 align="center">ICM</h1>

<p align="center">
  Memoria permanente per agenti AI. Binario singolo, zero dipendenze, nativo MCP.
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/icm/actions/workflows/ci.yml"><img src="https://github.com/rtk-ai/icm/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/icm/releases/latest"><img src="https://img.shields.io/github/v/release/rtk-ai/icm?color=purple" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-Source--Available-orange.svg" alt="Source-Available"></a>
</p>

---

ICM offre al tuo agente AI una vera memoria — non uno strumento per prendere appunti, non un gestore di contesto, ma una **memoria**.

```
                       ICM (Infinite Context Memory)
            ┌──────────────────────┬─────────────────────────┐
            │   MEMORIES (Topics)  │   MEMOIRS (Knowledge)   │
            │                      │                         │
            │  Episodic, temporal  │  Permanent, structured  │
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

**Due modelli di memoria:**

- **Memories** — archivia/recupera con decadimento temporale basato sull'importanza. Le memorie critiche non svaniscono mai, quelle a bassa importanza decadono naturalmente. Filtra per topic o parola chiave.
- **Memoirs** — grafi di conoscenza permanenti. Concetti collegati da relazioni tipizzate (`depends_on`, `contradicts`, `superseded_by`, ...). Filtra per etichetta.
- **Feedback** — registra le correzioni quando le previsioni AI sono errate. Cerca gli errori passati prima di fare nuove previsioni. Apprendimento a ciclo chiuso.

## Installazione

```bash
# Homebrew (macOS / Linux)
brew tap rtk-ai/tap && brew install icm

# Installazione rapida

# Dal sorgente
cargo install --path crates/icm-cli
```

## Configurazione

```bash
# Rileva e configura automaticamente tutti gli strumenti supportati
icm init
```

Configura **17 strumenti** con un solo comando ([guida completa alle integrazioni](docs/integrations.md)):

| Strumento | MCP | Hook | CLI | Skills |
|-----------|:---:|:----:|:---:|:------:|
| Claude Code | `~/.claude.json` | 5 hook | `CLAUDE.md` | `/recall` `/remember` |
| Claude Desktop | JSON | — | — | — |
| Gemini CLI | `~/.gemini/settings.json` | 5 hook | `GEMINI.md` | — |
| Codex CLI | `~/.codex/config.toml` | 4 hook | `AGENTS.md` | — |
| Copilot CLI | `~/.copilot/mcp-config.json` | 4 hook | `.github/copilot-instructions.md` | — |
| Cursor | `~/.cursor/mcp.json` | — | — | regola `.mdc` |
| Windsurf | JSON | — | `.windsurfrules` | — |
| VS Code | `~/Library/.../Code/User/mcp.json` | — | — | — |
| Amp | JSON | — | — | `/icm-recall` `/icm-remember` |
| Amazon Q | JSON | — | — | — |
| Cline | VS Code globalStorage | — | — | — |
| Roo Code | VS Code globalStorage | — | — | regola `.md` |
| Kilo Code | VS Code globalStorage | — | — | — |
| Zed | `~/.zed/settings.json` | — | — | — |
| OpenCode | JSON | plugin TS | — | — |
| Continue.dev | `~/.continue/config.yaml` | — | — | — |
| Aider | — | — | `.aider.conventions.md` | — |

Oppure manualmente:

```bash
# Claude Code
claude mcp add icm -- icm serve

# Modalità compatta (risposte più brevi, risparmio di token)
claude mcp add icm -- icm serve --compact

# Qualsiasi client MCP: command = "icm", args = ["serve"]
```

### Skills / regole

```bash
icm init --mode skill
```

Installa comandi slash e regole per Claude Code (`/recall`, `/remember`), Cursor (regola `.mdc`), Roo Code (regola `.md`), e Amp (`/icm-recall`, `/icm-remember`).

### Istruzioni CLI

```bash
icm init --mode cli
```

Inietta le istruzioni ICM nel file di istruzioni di ogni strumento:

| Strumento | File |
|-----------|------|
| Claude Code | `CLAUDE.md` |
| GitHub Copilot | `.github/copilot-instructions.md` |
| Windsurf | `.windsurfrules` |
| OpenAI Codex | `AGENTS.md` |
| Gemini | `~/.gemini/GEMINI.md` |

### Hook (5 strumenti)

```bash
icm init --mode hook
```

Installa hook di estrazione automatica e richiamo automatico per tutti gli strumenti supportati:

| Strumento | SessionStart | PreTool | PostTool | Compact | PromptRecall | Config |
|-----------|:-----------:|:-------:|:--------:|:-------:|:------------:|--------|
| Claude Code | `icm hook start` | `icm hook pre` | `icm hook post` | `icm hook compact` | `icm hook prompt` | `~/.claude/settings.json` |
| Gemini CLI | `icm hook start` | `icm hook pre` | `icm hook post` | `icm hook compact` | `icm hook prompt` | `~/.gemini/settings.json` |
| Codex CLI | `icm hook start` | `icm hook pre` | `icm hook post` | — | `icm hook prompt` | `~/.codex/hooks.json` |
| Copilot CLI | `icm hook start` | `icm hook pre` | `icm hook post` | — | `icm hook prompt` | `.github/hooks/icm.json` |
| OpenCode | avvio sessione | — | estrazione tool | compattazione | — | `~/.config/opencode/plugins/icm.ts` |

**Cosa fa ogni hook:**

| Hook | Funzione |
|------|----------|
| `icm hook start` | Inietta un pacchetto di avvio con le memorie critiche/importanti all'inizio della sessione (~500 token) |
| `icm hook pre` | Autorizza automaticamente i comandi CLI `icm` (senza richiesta di permesso) |
| `icm hook post` | Estrae fatti dall'output degli strumenti ogni N chiamate (estrazione automatica) |
| `icm hook compact` | Estrae memorie dalla trascrizione prima della compressione del contesto |
| `icm hook prompt` | Inietta il contesto recuperato all'inizio di ogni prompt utente |

## CLI vs MCP

ICM può essere usato tramite CLI (comandi `icm`) o server MCP (`icm serve`). Entrambi accedono allo stesso database.

| | CLI | MCP |
|---|-----|-----|
| **Latenza** | ~30ms (binario diretto) | ~50ms (JSON-RPC stdio) |
| **Costo in token** | 0 (basato su hook, invisibile) | ~20-50 token/chiamata (schema tool) |
| **Configurazione** | `icm init --mode hook` | `icm init --mode mcp` |
| **Compatibile con** | Claude Code, Gemini, Codex, Copilot, OpenCode (tramite hook) | Tutti i 17 strumenti compatibili MCP |
| **Estrazione automatica** | Sì (gli hook attivano `icm extract`) | Sì (i tool MCP chiamano store) |
| **Ideale per** | Utenti avanzati, risparmio di token | Compatibilità universale |

## CLI

### Memories (episodiche, con decadimento)

```bash
# Archivia
icm store -t "my-project" -c "Use PostgreSQL for the main DB" -i high -k "db,postgres"

# Recupera
icm recall "database choice"
icm recall "auth setup" --topic "my-project" --limit 10
icm recall "architecture" --keyword "postgres"

# Gestione
icm forget <memory-id>
icm consolidate --topic "my-project"
icm topics
icm stats

# Estrai fatti dal testo (basato su regole, zero costo LLM)
echo "The parser uses Pratt algorithm" | icm extract -p my-project
```

### Memoirs (grafi di conoscenza permanenti)

```bash
# Crea un memoir
icm memoir create -n "system-architecture" -d "System design decisions"

# Aggiungi concetti con etichette
icm memoir add-concept -m "system-architecture" -n "auth-service" \
  -d "Handles JWT tokens and OAuth2 flows" -l "domain:auth,type:service"

# Collega concetti
icm memoir link -m "system-architecture" --from "api-gateway" --to "auth-service" -r depends-on

# Cerca con filtro per etichetta
icm memoir search -m "system-architecture" "authentication"
icm memoir search -m "system-architecture" "service" --label "domain:auth"

# Ispeziona il vicinato
icm memoir inspect -m "system-architecture" "auth-service" -D 2

# Esporta grafo (formati: json, dot, ascii, ai)
icm memoir export -m "system-architecture" -f ascii   # Box-drawing con barre di confidenza
icm memoir export -m "system-architecture" -f dot      # Graphviz DOT (colore = livello di confidenza)
icm memoir export -m "system-architecture" -f ai       # Markdown ottimizzato per contesto LLM
icm memoir export -m "system-architecture" -f json     # JSON strutturato con tutti i metadati

# Genera visualizzazione SVG
icm memoir export -m "system-architecture" -f dot | dot -Tsvg > graph.svg
```

## Tool MCP (22)

### Tool per le memorie

| Tool | Descrizione |
|------|-------------|
| `icm_memory_store` | Archivia con deduplicazione automatica (similarità >85% → aggiorna invece di duplicare) |
| `icm_memory_recall` | Cerca per query, filtra per topic e/o parola chiave |
| `icm_memory_update` | Modifica una memoria in-place (contenuto, importanza, parole chiave) |
| `icm_memory_forget` | Elimina una memoria tramite ID |
| `icm_memory_consolidate` | Unisce tutte le memorie di un topic in un unico riepilogo |
| `icm_memory_list_topics` | Elenca tutti i topic con i relativi conteggi |
| `icm_memory_stats` | Statistiche globali della memoria |
| `icm_memory_health` | Controllo igienico per topic (obsolescenza, necessità di consolidazione) |
| `icm_memory_embed_all` | Ricalcola gli embedding per la ricerca vettoriale |

### Tool per i memoir (grafi di conoscenza)

| Tool | Descrizione |
|------|-------------|
| `icm_memoir_create` | Crea un nuovo memoir (contenitore di conoscenza) |
| `icm_memoir_list` | Elenca tutti i memoir |
| `icm_memoir_show` | Mostra i dettagli del memoir e tutti i concetti |
| `icm_memoir_add_concept` | Aggiunge un concetto con etichette |
| `icm_memoir_refine` | Aggiorna la definizione di un concetto |
| `icm_memoir_search` | Ricerca full-text, opzionalmente filtrata per etichetta |
| `icm_memoir_search_all` | Cerca in tutti i memoir |
| `icm_memoir_link` | Crea una relazione tipizzata tra concetti |
| `icm_memoir_inspect` | Ispeziona il concetto e il vicinato nel grafo (BFS) |
| `icm_memoir_export` | Esporta il grafo (json, dot, ascii, ai) con livelli di confidenza |

### Tool per il feedback (apprendere dagli errori)

| Tool | Descrizione |
|------|-------------|
| `icm_feedback_record` | Registra una correzione quando una previsione AI era errata |
| `icm_feedback_search` | Cerca le correzioni passate per informare le previsioni future |
| `icm_feedback_stats` | Statistiche del feedback: conteggio totale, dettaglio per topic, più applicate |

### Tipi di relazione

`part_of` · `depends_on` · `related_to` · `contradicts` · `refines` · `alternative_to` · `caused_by` · `instance_of` · `superseded_by`

## Come funziona

### Modello di memoria duale

**Memoria episodica (Topics)** cattura decisioni, errori, preferenze. Ogni memoria ha un peso che decade nel tempo in base all'importanza:

| Importanza | Decadimento | Eliminazione | Comportamento |
|-----------|-------------|--------------|---------------|
| `critical` | nessuno | mai | Mai dimenticata, mai eliminata |
| `high` | lento (0.5x tasso) | mai | Svanisce lentamente, mai eliminata automaticamente |
| `medium` | normale | sì | Decadimento standard, eliminata quando il peso è sotto la soglia |
| `low` | rapido (2x tasso) | sì | Dimenticata rapidamente |

Il decadimento è **consapevole degli accessi**: le memorie richiamate frequentemente decadono più lentamente (`decay / (1 + access_count × 0.1)`). Applicato automaticamente al recupero (se >24h dall'ultimo decadimento).

**L'igiene della memoria** è integrata:
- **Deduplicazione automatica**: archiviare contenuti con similarità >85% rispetto a una memoria esistente nello stesso topic la aggiorna invece di creare un duplicato
- **Suggerimenti di consolidazione**: quando un topic supera le 7 voci, `icm_memory_store` avvisa il chiamante di consolidare
- **Controllo dello stato**: `icm_memory_health` riporta il numero di voci per topic, il peso medio, le voci obsolete e le necessità di consolidazione
- **Nessuna perdita silenziosa di dati**: le memorie critiche e ad alta importanza non vengono mai eliminate automaticamente

**Memoria semantica (Memoirs)** cattura la conoscenza strutturata come un grafo. I concetti sono permanenti — vengono raffinati, mai fatti decadere. Usa `superseded_by` per contrassegnare i fatti obsoleti invece di eliminarli.

### Ricerca ibrida

Con gli embedding abilitati, ICM utilizza la ricerca ibrida:
- **FTS5 BM25** (30%) — corrispondenza full-text per parole chiave
- **Similarità coseno** (70%) — ricerca vettoriale semantica tramite sqlite-vec

Modello predefinito: `intfloat/multilingual-e5-base` (768d, 100+ lingue). Configurabile nel tuo [file di configurazione](#configurazione):

```toml
[embeddings]
# enabled = false                          # Disabilita completamente (nessun download del modello)
model = "intfloat/multilingual-e5-base"    # 768d, multilingue (predefinito)
# model = "intfloat/multilingual-e5-small" # 384d, multilingue (più leggero)
# model = "intfloat/multilingual-e5-large" # 1024d, multilingue (migliore precisione)
# model = "Xenova/bge-small-en-v1.5"      # 384d, solo inglese (più veloce)
# model = "jinaai/jina-embeddings-v2-base-code"  # 768d, ottimizzato per codice
```

Per saltare completamente il download del modello di embedding, usa uno di questi:
```bash
icm --no-embeddings serve          # Flag CLI
ICM_NO_EMBEDDINGS=1 icm serve     # Variabile d'ambiente
```
Oppure imposta `enabled = false` nel tuo file di configurazione. ICM tornerà alla ricerca per parole chiave FTS5 (funziona comunque, ma senza corrispondenza semantica).

La modifica del modello ricrea automaticamente l'indice vettoriale (gli embedding esistenti vengono cancellati e possono essere rigenerati con `icm_memory_embed_all`).

### Storage

Un singolo file SQLite. Nessun servizio esterno, nessuna dipendenza di rete.

```
~/Library/Application Support/dev.icm.icm/memories.db                    # macOS
~/.local/share/dev.icm.icm/memories.db                                   # Linux
C:\Users\<user>\AppData\Local\icm\icm\data\memories.db                   # Windows
```

### Configurazione

```bash
icm config                    # Mostra la configurazione attiva
```

Posizione del file di configurazione (specifica per piattaforma, o `$ICM_CONFIG`):

```
~/Library/Application Support/dev.icm.icm/config.toml                    # macOS
~/.config/icm/config.toml                                                # Linux
C:\Users\<user>\AppData\Roaming\icm\icm\config\config.toml              # Windows
```

Vedi [config/default.toml](config/default.toml) per tutte le opzioni.

## Estrazione automatica

ICM estrae le memorie automaticamente tramite tre livelli:

```
  Livello 0: Hook a pattern       Livello 1: PreCompact          Livello 2: UserPromptSubmit
  (zero costo LLM)                (zero costo LLM)               (zero costo LLM)
  ┌──────────────────┐            ┌──────────────────┐          ┌──────────────────┐
  │ PostToolUse hook  │            │ PreCompact hook   │          │ UserPromptSubmit  │
  │                   │            │                   │          │                   │
  │ • Errori Bash     │            │ Contesto sul      │          │ L'utente invia    │
  │ • commit git      │            │ punto di essere   │          │ un prompt →       │
  │ • cambiam. config │            │ compresso →       │          │ icm recall        │
  │ • decisioni       │            │ estrai memorie    │          │ → inietta context │
  │ • preferenze      │            │ dalla trascrizione│          │                   │
  │ • apprendimenti   │            │ prima che vadano  │          │ L'agente inizia   │
  │ • vincoli         │            │ perdute per sempre│          │ con le memorie    │
  │                   │            │                   │          │ pertinenti già    │
  │ Basato su regole, │            │ Stessi pattern +  │          │ caricate          │
  │ nessun LLM        │            │ --store-raw fallbk│          │                   │
  └──────────────────┘            └──────────────────┘          └──────────────────┘
```

| Livello | Stato | Costo LLM | Comando hook | Descrizione |
|---------|-------|-----------|-------------|-------------|
| Livello 0 | Implementato | 0 | `icm hook post` | Estrazione di parole chiave basata su regole dall'output degli strumenti |
| Livello 1 | Implementato | 0 | `icm hook compact` | Estrazione dalla trascrizione prima della compressione del contesto |
| Livello 2 | Implementato | 0 | `icm hook prompt` | Iniezione delle memorie recuperate ad ogni prompt utente |

Tutti e 3 i livelli vengono installati automaticamente da `icm init --mode hook`.

### Confronto con le alternative

| Sistema | Metodo | Costo LLM | Latenza | Cattura la compattazione? |
|---------|--------|-----------|---------|--------------------------|
| **ICM** | Estrazione a 3 livelli | da 0 a ~500 tok/sessione | 0ms | **Sì (PreCompact)** |
| Mem0 | 2 chiamate LLM/messaggio | ~2k tok/messaggio | 200-2000ms | No |
| claude-mem | PostToolUse + async | ~1-5k tok/sessione | 8ms hook | No |
| MemGPT/Letta | Autogestione dell'agente | 0 marginale | 0ms | No |
| DiffMem | Diff basati su Git | 0 | 0ms | No |

## Benchmark

### Performance di archiviazione

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

Apple M1 Pro, SQLite in-memory, single-threaded. `icm bench --count 1000`

### Efficienza degli agenti

Flusso di lavoro multi-sessione con un progetto Rust reale (12 file, ~550 righe). Le sessioni dalla 2 in poi mostrano i maggiori guadagni mentre ICM recupera invece di rileggere i file.

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

### Ritenzione della conoscenza

L'agente recupera fatti specifici da un documento tecnico denso attraverso le sessioni. La sessione 1 legge e memorizza; le sessioni 2+ rispondono a 10 domande fattuali **senza** il testo sorgente.

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

### LLM locali (ollama)

Stesso test con modelli locali — pura iniezione di contesto, nessun uso di strumenti necessario.

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

### Protocollo di test

Tutti i benchmark utilizzano **chiamate API reali** — nessun mock, nessuna risposta simulata, nessuna risposta in cache.

- **Benchmark degli agenti**: Crea un progetto Rust reale in una directory temporanea. Esegue N sessioni con `claude -p --output-format json`. Senza ICM: configurazione MCP vuota. Con ICM: server MCP reale + estrazione automatica + iniezione di contesto.
- **Ritenzione della conoscenza**: Usa un documento tecnico fittizio (il "Protocollo Meridian"). Valuta le risposte tramite corrispondenza di parole chiave rispetto ai fatti attesi. Timeout di 120s per invocazione.
- **Isolamento**: Ogni esecuzione utilizza la propria directory temporanea e un database SQLite nuovo. Nessuna persistenza tra sessioni.

### Memoria unificata multi-agente

Tutti i 17 strumenti condividono lo stesso database SQLite. Una memoria archiviata da Claude è immediatamente disponibile per Gemini, Codex, Copilot, Cursor e tutti gli altri strumenti.

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

Punteggio = 60% accuratezza richiamo + 30% dettaglio fatti + 10% velocità. **98% efficienza multi-agente.**

## Perché ICM

| Funzionalità | ICM | Mem0 | Engram | AgentMemory |
|-------------|:---:|:----:|:------:|:-----------:|
| Strumenti supportati | **17** | Solo SDK | ~6-8 | ~10 |
| Configurazione con un comando | `icm init` | SDK manuale | manuale | manuale |
| Hook (richiamo automatico all'avvio) | 5 strumenti | nessuno | via MCP | 1 strumento |
| Ricerca ibrida (FTS5 + vettoriale) | 30/70 ponderata | solo vettoriale | solo FTS5 | FTS5+vettoriale |
| Embedding multilingue | 100+ lingue (768d) | dipende | nessuno | inglese 384d |
| Grafo di conoscenza | Sistema Memoir | nessuno | nessuno | nessuno |
| Decadimento temporale + consolidazione | consapevole degli accessi | nessuno | base | base |
| Dashboard TUI | `icm dashboard` | nessuno | sì | visualizzatore web |
| Estrazione automatica dall'output degli strumenti | 3 livelli, zero LLM | nessuno | nessuno | nessuno |
| Ciclo di feedback/correzione | `icm_feedback_*` | nessuno | nessuno | nessuno |
| Runtime | Binario singolo Rust | Python | Go | Node.js |
| Local-first, zero dipendenze | File SQLite | cloud-first | SQLite | SQLite |
| Accuratezza richiamo multi-agente | **98%** | N/A | N/A | 95.2% |

## Documentazione

| Documento | Descrizione |
|-----------|-------------|
| [Guida alle integrazioni](docs/integrations.md) | Configurazione per tutti i 17 strumenti: Claude Code, Copilot, Cursor, Windsurf, Zed, Amp, ecc. |
| [Architettura tecnica](docs/architecture.md) | Struttura dei crate, pipeline di ricerca, modello di decadimento, integrazione sqlite-vec, test |
| [Guida utente](docs/guide.md) | Installazione, organizzazione dei topic, consolidazione, estrazione, risoluzione dei problemi |
| [Panoramica del prodotto](docs/product.md) | Casi d'uso, benchmark, confronto con le alternative |

## Licenza

[Apache-2.0](LICENSE)
