[English](README.md) | [Français](README_fr.md) | [Español](README_es.md) | [Deutsch](README_de.md) | [Italiano](README_it.md) | [Português](README_pt.md) | [Nederlands](README_nl.md) | [Polski](README_pl.md) | [Русский](README_ru.md) | [日本語](README_ja.md) | [中文](README_zh.md) | [العربية](README_ar.md) | [한국어](README_ko.md)

Questa è una traduzione di README.md (inglese), che fa da riferimento; in caso di differenze, vale la versione inglese.

<h1 align="center">ICM</h1>

<p align="center">
  <b>Memoria a lungo termine per agenti di coding IA, condivisa tra i tuoi strumenti.</b><br>
  Un binario, un file SQLite. Nessuna chiamata a un LLM per salvare o richiamare una memoria.
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/icm/actions/workflows/ci.yml"><img src="https://github.com/rtk-ai/icm/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/icm/releases/latest"><img src="https://img.shields.io/github/v/release/rtk-ai/icm?color=purple" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-Apache--2.0-blue.svg" alt="Apache-2.0"></a>
</p>

Spiega lunedì a Claude Code come il tuo progetto gestisce l'autenticazione, e la sessione di Gemini CLI di martedì lo sa già. ICM conserva ciò che i tuoi agenti di coding imparano (decisioni, fix, convenzioni, preferenze) in un unico file SQLite sulla tua macchina, e ne restituisce la parte rilevante all'inizio di ogni sessione e a ogni prompt che invii. Fino a 18 agenti ed editor condividono questa memoria, così smetti di rispiegare il tuo progetto ogni volta che apri una sessione o cambi strumento.

- **92.9% su LoCoMo (1,540 domande, media di tre esecuzioni), alla pari con Hindsight (92.0%)**, con un terzo di contesto in meno per domanda. [Dettagli e avvertenze più sotto](#benchmark-comparison).
- **Nessuna chiamata a un LLM per salvare o richiamare.** Hindsight, Mem0, Graphiti (Zep) e claude-mem chiamano per impostazione predefinita un LLM per ogni memoria che salvano. ICM no. Solo la sua estrazione automatica di fatti dall'output degli strumenti passa per lo strumento LLM da riga di comando che usi già, quando ce n'è uno installato; `provider = "none"` mantiene locale anche quella (vedi [Avvio rapido](#quickstart)).
- **Utile anche senza modello di embedding.** Il recall per sole parole chiave mette almeno una delle sessioni giuste nella top 5 per l'88.6% delle domande di LoCoMo, con una mediana di 6.8 ms per richiamo su Linux x86-64.
- **97.4% su LongMemEval-S, retrieval senza LLM**: una delle sessioni giuste nella top 5 per 487 domande su 500, contro il 96.6% di MemPalace e il 95.2% di agentmemory sulla stessa misura.
- **Non è in testa ovunque.** Su PersonaMem (le preferenze di un utente che cambiano nel tempo) è in testa Hindsight: 86.6% contro l'81.7% di ICM.

<p align="center">
  <img src="assets/demo.svg" alt="Terminale: tre memorie salvate con icm store, poi due domande a cui risponde icm recall, ognuna restituendo la memoria giusta">
</p>

<a id="quickstart"></a>

## Avvio rapido

```bash
brew tap rtk-ai/tap && brew install icm    # or: curl -fsSL https://raw.githubusercontent.com/rtk-ai/icm/main/install.sh | sh
icm init                                   # instructions, skills and hooks for every agent it detects
```

Questa è tutta la configurazione. Apri una nuova sessione in Claude Code, Codex, Gemini CLI o Copilot CLI: il tuo agente ora parte con un breve pacchetto delle memorie più importanti del progetto in cui si trova (quelle il cui topic porta il nome del repository, come `decisions-myapp` in un repository chiamato `myapp`, più le tue preferenze) e riceve le memorie rilevanti per ogni prompt che invii. Ciò che impara dall'output dei suoi strumenti viene messo in coda, e la coda viene trasformata in memorie alla fine di ogni sessione di Claude Code; con gli altri strumenti, esegui `icm extract-pending` (da un cron job, per esempio).

Il salvataggio e il richiamo restano sulla tua macchina. L'estrazione automatica passa del testo allo strumento LLM da riga di comando che usi già (Claude Code, Codex o Gemini CLI), quando ce n'è uno installato; imposta `provider = "none"` sotto `[extraction.summarizer]` per mantenerla del tutto locale.

Per vederlo funzionare subito, salva e richiama a mano:

```console
$ icm store -t decisions-myapp -c "Auth uses short-lived JWTs, refreshed through /auth/refresh" -i high
Stored: 01M47YY6CHVZ48BYKQRCRNZTCF

$ icm recall "how does auth work"
memories[1]{id,topic,importance,weight,summary}:
  01M47YY6CHVZ48BYKQRCRNZTCF,decisions-myapp,high,0.975,"Auth uses short-lived JWTs, refreshed through /auth/refresh"
```

Il primo salvataggio o richiamo con ricerca semantica scarica una sola volta il modello di embedding multilingue (`Qdrant/multilingual-e5-large-onnx`, circa 2 GB). Per provare ICM senza, aggiungi `--no-embeddings` (recall per parole chiave, come nell'output qui sopra) oppure scegli un modello più leggero nella configurazione. `icm init` scrive hook e istruzioni nella configurazione di ogni agente rilevato; `icm uninstall --dry-run` mostra come rimuoverli. Windows, Linux, Nix e compilazione dai sorgenti: [Installazione](#install).

<a id="benchmark-comparison"></a>

## Confronto dei benchmark

Accuratezza delle risposte su [LoCoMo](https://github.com/snap-research/locomo) (10 conversazioni lunghe, 1,540 domande), misurata con l'harness pubblico [Agent Memory Benchmark](https://github.com/vectorize-io/agent-memory-benchmark): il sistema di memoria recupera il contesto, `gemini-3.1-pro-preview` risponde a partire da quello, `gemini-2.5-flash-lite` giudica la risposta.

| Sistema | Accuratezza LoCoMo | Contesto per domanda | Chiamate LLM per salvare una memoria | Si esegue come | Risultato |
|--------|:---------------:|:--------------------:|:---------------------------:|---------|--------|
| **ICM** 0.11.0 (motore di recall v2) | **92.9%** (1,430, 1,433 e 1,430 / 1,540 in tre esecuzioni) | 24.1k token | nessuna | un binario Rust, file SQLite | nostre esecuzioni, 2026-10-06 |
| Hindsight | 92.0% (1,417 / 1,540) | 36.2k token | estrazione di fatti via LLM | servizio Python, PostgreSQL + pgvector | pubblicato dall'harness |
| Baseline di ricerca ibrida (dense + sparse, RRF) | 79.1% (1,218 / 1,540) | 22.2k token | nessuna | Qdrant | pubblicato dall'harness |

Su [PersonaMem](https://arxiv.org/abs/2504.14225) 32k (589 domande a scelta multipla sulle preferenze di un utente che cambiano nel tempo, stesso harness e stesso modello di risposta, valutate per corrispondenza della lettera):

| Sistema | Accuratezza PersonaMem | Contesto per domanda | Risultato |
|--------|:-------------------:|:--------------------:|--------|
| **ICM** 0.11.0 (motore di recall v2) | **81.7%** (486, 486 e 472 / 589 in tre esecuzioni) | 16.2k token | nostre esecuzioni, 2026-10-06 |
| Hindsight | 86.6% (510 / 589) | 15.8k token | pubblicato dall'harness |
| Baseline di ricerca ibrida | 84.4% (497 / 589) | 24.2k token | pubblicato dall'harness |

Il solo retrieval, su LoCoMo, senza modello di risposta: la quota di domande per cui almeno una sessione gold è tra i primi risultati. Questa è la prova che riguarda il motore di recall in sé. Le esecuzioni v2 hanno ricevuto la data di ogni sessione e la data della domanda; il motore precedente non ha un input di data, quindi le sue esecuzioni non ne hanno ricevuta nessuna.

| Primi risultati | Motore precedente (`legacy`) | Motore di recall v2 | Motore precedente, senza modello di embedding | Motore di recall v2, senza modello di embedding |
|:-----------:|:--------------:|:----------------:|:--------------:|:----------------:|
| 5 | 76.5% | **86.7%** | 12.0% | **88.6%** |
| 10 | 83.0% | **93.3%** | 17.4% | **94.2%** |
| 20 | 87.7% | **97.9%** | 29.8% | **97.5%** |

LongMemEval-S, solo retrieval, senza LLM (ICM con il suo modello di embedding predefinito; 500 domande; ogni domanda è accompagnata da circa 48 sessioni passate in cui cercare; una memoria per sessione, solo i turni dell'utente, come li indicizza MemPalace; nessuna data fornita a ICM):

| Sessioni giuste nella top 5 | **ICM** 0.11.0 | MemPalace | agentmemory | Solo BM25 |
|---|:---:|:---:|:---:|:---:|
| Almeno una (la misura pubblicata) | **97.4%** (487 / 500) | 96.6% (483 / 500) | 95.2% (476 / 500) | 94.6% |
| Tutte | **88.6%** (443 / 500) | 85.0% | 81.8% | 81.2% |

I valori di MemPalace e agentmemory sono ricalcolati con il nostro script di valutazione a partire dai file di risultati che ciascun progetto pubblica; corrispondono ai numeri pubblicati. agentmemory indicizza tutti i turni di una sessione; su quell'unità BM25 da solo raggiunge il 96.2% e l'83.0%. Un semplice BM25 arriva già intorno al 95% sulla prima misura, ed è per questo che la seconda, tutte le sessioni giuste, distingue meglio i sistemi.

Cosa mostrano e cosa non mostrano questi numeri:

- **Su LoCoMo ICM e Hindsight sono alla pari.** Le tre esecuzioni di ICM (92.9%, 93.1%, 92.9%) sono ciascuna circa 1 punto sopra il 92.0% pubblicato da Hindsight, un distacco di circa 14 domande, entro l'errore di campionamento (intervallo al 95% per ICM: da 91.6 a 94.2). ICM ci arriva con un terzo di contesto in meno e senza chiamare un LLM quando salva una memoria.
- **Su PersonaMem Hindsight è avanti** di 4.9 punti, al di fuori dell'errore di campionamento (intervallo al 95% per ICM: da 78.6 a 84.8). ICM è anche 2.7 punti sotto la baseline di ricerca ibrida, entro quell'intervallo, leggendo un terzo di contesto in meno. Le tre esecuzioni vanno dall'80.1% all'82.5%.
- **Condizioni non identiche.** L'harness è mantenuto da Vectorize, il fornitore di Hindsight. I risultati LoCoMo pubblicati sono precedenti a una modifica che ha impostato a 0 la temperatura della risposta e del giudice; la nostra esecuzione usa l'harness attuale (commit `f618ed7`) e Vertex AI.
- **Con 50 chunk viene restituita gran parte di ogni conversazione,** quindi questa accuratezza misura anche il modello di risposta. La tabella del retrieval è la prova che riguarda il motore di recall in sé.
- **Tre esecuzioni per dataset.** Il modello di risposta varia da un'esecuzione all'altra: 0.2 punti su LoCoMo, 2.4 punti su PersonaMem. Gli intervalli qui sopra coprono il campionamento delle domande.

<details>
<summary>Risultati per categoria, latenza e altre avvertenze</summary>

- **Per tipo di domanda** (LoCoMo, etichette dell'harness, tre esecuzioni): open-domain 96.7% (841 domande), temporal 90.9% (321), single-hop 88.8% (282), multi-hop 78.8% (96).
- **Latenza.** Latenza mediana di richiamo tra 137 e 149 ms sui nodi con 4 vCPU del cluster durante le esecuzioni su LoCoMo; 272 sessioni acquisite senza alcuna chiamata LLM. Nelle esecuzioni di solo retrieval su Linux x86-64: mediana di 136 ms con gli embedding, 6.8 ms con le sole parole chiave.
- **Date delle sessioni.** L'adattatore del benchmark scrive la data di ogni sessione nel testo della memoria di ICM; Hindsight riceve le stesse date come metadati, e ogni sistema riceve la data della domanda.
- **La categoria più debole sono le 96 domande che l'harness etichetta come multi-hop** (78.8%). I nomi delle categorie non coincidono tra i benchmark: altre valutazioni di LoCoMo chiamano questa categoria open-domain, e chiamano multi-hop le 282 domande che l'harness etichetta come single-hop (qui 88.8%). Confronta in base al numero di domande, non all'etichetta.
- **Il motore di recall v2 è quello predefinito** per `icm recall`, lo strumento MCP `icm_memory_recall`, `/recall` via HTTP e l'hook del prompt. Il motore precedente resta disponibile per tornare indietro o per confrontare: `icm recall --engine legacy`, `"engine": "legacy"` su `/recall` via HTTP, oppure `ICM_RECALL_ENGINE=legacy`.

</details>

I risultati domanda per domanda di ogni esecuzione (tre per dataset, più l'esecuzione di recall su LongMemEval-S) sono in [`bench/amb/results/`](bench/amb/results/); l'adattatore, le impostazioni esatte e i comandi per riprodurli sono in [`bench/amb/README.md`](bench/amb/README.md).

<a id="one-memory-for-every-tool"></a>

## Una sola memoria per tutti gli strumenti

Ogni strumento configurato da `icm init` legge e scrive lo stesso database SQLite, e i topic (`decisions-myapp`, `preferences`, `errors-resolved`, ...) non sono separati per strumento. Una memoria salvata da Claude Code è subito visibile a Codex, Gemini, Cursor, Roo, Amp, Aider, ...

Preferisci l'isolamento? `icm init --per-project` crea un database locale al progetto sotto `.icm/` (e scrive i file di istruzioni degli agenti, come `CLAUDE.md` e `AGENTS.md`, nella directory corrente); `--db <path>` o `ICM_DB` puntano a qualsiasi altro file. Ogni percorso è un corpus indipendente.

> **Stato del progetto: beta.** ICM è pre-1.0: modifiche incompatibili possono arrivare in qualsiasi release minore, e i formati di configurazione di hook e MCP possono cambiare. Beta si riferisce alla stabilità dell'API, non all'utilità quotidiana: io (il maintainer) uso ICM ogni giorno come memoria principale per programmare con l'IA. Il mio focus principale è [rtk](https://github.com/rtk-ai/rtk), quindi issue e pull request vengono esaminate secondo la disponibilità.
>
> Apache-2.0, distribuito **così com'è, senza garanzie di alcun tipo** (vedi [LICENSE](LICENSE)). Prima di qualsiasi operazione distruttiva, esegui prima l'equivalente in sola lettura (`icm uninstall --dry-run`, `icm uninstall --check`).

<a id="install"></a>

## Installazione

```bash
# macOS / Linux, Homebrew
brew tap rtk-ai/tap && brew install icm

# macOS / Linux, script (verifies SHA256 against the release checksums)
curl -fsSL https://raw.githubusercontent.com/rtk-ai/icm/main/install.sh | sh

# Windows, PowerShell
irm https://raw.githubusercontent.com/rtk-ai/icm/main/install.ps1 | iex
```

La ricerca per parole chiave funziona ovunque. Esegui `icm embeddings status` per vedere se la ricerca semantica è attiva: è inclusa nelle build per macOS Apple Silicon, Windows e `.rpm`; gli archivi Linux glibc e il `.deb` richiedono un `icm embeddings download`; la build per Mac Intel richiede un tuo ONNX Runtime (`ORT_DYLIB_PATH`); la build statica Linux musl supporta solo le parole chiave. Nix, compilazione dai sorgenti, pinning della versione e i dettagli: [riferimento](docs/reference.md#install).

<a id="setup"></a>

## Configurazione

```bash
icm init                  # global database, every detected agent
icm init --per-project    # database under .icm/ at the git root
icm init --mode all       # also register the MCP server in every tool that supports it
```

La modalità predefinita (`standard`) scrive istruzioni, skill e hook, senza server MCP. `--mode all` aggiunge il server MCP; con questa (più `--per-project` per Aider, il cui file di convenzioni è per progetto) si coprono i 18 strumenti qui sotto ([guida all'integrazione](docs/integrations.md)):

| Strumento | Server MCP | Hook |
|------|:---:|:-----:|
| Claude Code | sì | sì |
| Claude Desktop | sì | — |
| Gemini CLI | sì | sì |
| Codex CLI | sì | sì |
| Copilot CLI | sì | sì |
| Cursor | sì | — |
| Windsurf | sì | — |
| VS Code | sì | — |
| Amp | sì | — |
| Amazon Q | sì | — |
| Cline | sì | — |
| Roo Code | sì | — |
| Kilo Code | sì | — |
| Zed | sì | — |
| OpenCode | sì | sì |
| Continue.dev | sì | — |
| Aider | — | — |
| Pi | — | — |

Oppure registra il server MCP a mano: `claude mcp add icm -- icm serve` (qualsiasi client MCP: comando `icm`, argomenti `["serve"]`).

Cosa fanno gli hook:

| Hook | Cosa fa |
|------|-------------|
| `icm hook start` | Inietta all'inizio della sessione un pacchetto di risveglio con le memorie critical/high (~500 token) |
| `icm hook pre` | Consente automaticamente i comandi CLI di `icm` (nessuna richiesta di permesso) |
| `icm hook post` | Estrae fatti dall'output degli strumenti ogni N chiamate (estrazione automatica) |
| `icm hook compact` | Estrae memorie dalla trascrizione prima della compressione del contesto |
| `icm hook prompt` | Inietta il contesto richiamato all'inizio di ogni prompt dell'utente |

Tabelle degli hook per strumento, skill, file di istruzioni e la nota su Codex: [riferimento](docs/reference.md#setup).

<a id="use"></a>

## Utilizzo

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

ICM conserva anche **memoirs** (grafi di conoscenza permanenti di concetti e relazioni tipizzate), **feedback** (correzioni da cui imparare) e **trascrizioni letterali**, ed espone **31 strumenti MCP** (30 senza modello di embedding), una **API HTTP** che mantiene caricato il modello di embedding e una **dashboard da terminale** (`icm dashboard`). Tutto è descritto nel [riferimento](docs/reference.md).

<a id="how-it-works"></a>

## Come funziona

Il recall fonde fino a tre liste ordinate tramite reciprocal rank fusion (RRF): il matching per parole chiave **FTS5 BM25**, sempre attivo; la **ricerca vettoriale semantica** tramite sqlite-vec quando è caricato un modello di embedding (predefinito `Qdrant/multilingual-e5-large-onnx`, 1024 dimensioni, 100+ lingue); e una **finestra di date** quando la query nomina un periodo ("last week", "in March 2024"). I filtri per progetto, topic e parola chiave si applicano prima del taglio. Le memorie sbiadiscono nel tempo in base alla loro importanza (`critical` non sbiadisce mai); con un modello di embedding caricato, una nuova memoria quasi identica a un'altra dello stesso topic (similarità del coseno superiore a 0.95) viene fusa con essa; e il modello che ha prodotto i vettori salvati è registrato nel database, quindi cambiare `model` nella configurazione non li cancella mai (`icm embed --migrate` è il modo esplicito per cambiare modello).

Tutto sta in un unico file SQLite, senza servizi esterni:

```
~/Library/Application Support/dev.icm.icm/memories.db     # macOS
~/.local/share/icm/memories.db                            # Linux
%APPDATA%\icm\icm\data\memories.db                        # Windows
<project-root>/.icm/memories.db                           # icm init --per-project
```

`icm config` mostra la configurazione attiva; [config/default.toml](config/default.toml) elenca tutte le opzioni. Dettagli: [riferimento](docs/reference.md#how-it-works).

<a id="documentation"></a>

## Documentazione

| Documento | Descrizione |
|----------|-------------|
| [Guida all'integrazione](docs/integrations.md) | Configurazione MCP per strumento: Claude Code, Cursor, Windsurf, Zed, Amp, Codex, Cline, Roo Code, ecc. |
| [Architettura tecnica](docs/architecture.md) | Struttura dei crate, pipeline di ricerca, modello di decay, integrazione di sqlite-vec, test |
| [Guida utente](docs/guide.md) | Installazione, organizzazione dei topic, consolidamento, estrazione, risoluzione dei problemi |
| [Panoramica del prodotto](docs/product.md) | Casi d'uso, benchmark, confronto con le alternative |
| [Riferimento](docs/reference.md) | Opzioni di installazione, configurazione per strumento, CLI, 31 strumenti MCP, API HTTP, dashboard, funzionamento interno |
| [Adattatore del benchmark](bench/amb/README.md) | Come è stato eseguito il confronto qui sopra e come riprodurlo |
| [Dimostrazioni](docs/demonstrations.md) | Micro-benchmark di archiviazione e piccole demo |

<a id="license"></a>

## Licenza

[Apache-2.0](LICENSE)
