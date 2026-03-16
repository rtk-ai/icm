[English](README.md) | [Français](README_fr.md) | [Español](README_es.md) | [Deutsch](README_de.md) | [Italiano](README_it.md) | **Português** | [Nederlands](README_nl.md) | [Polski](README_pl.md) | [Русский](README_ru.md) | [日本語](README_ja.md) | [中文](README_zh.md) | [العربية](README_ar.md) | [한국어](README_ko.md)

<p align="center">
  <img src="assets/banner.png" alt="ICM — Infinite Context Memory" width="600">
</p>

<h1 align="center">ICM</h1>

<p align="center">
  Memória permanente para agentes de IA. Binário único, zero dependências, MCP nativo.
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/icm/actions/workflows/ci.yml"><img src="https://github.com/rtk-ai/icm/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/icm/releases/latest"><img src="https://img.shields.io/github/v/release/rtk-ai/icm?color=purple" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-Source--Available-orange.svg" alt="Source-Available"></a>
</p>

---

O ICM dá ao seu agente de IA uma memória real — não uma ferramenta de anotações, não um gestor de contexto, uma **memória**.

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

**Dois modelos de memória:**

- **Memórias** — armazenar/recuperar com decaimento temporal por importância. Memórias críticas nunca desaparecem, as de baixa importância decaem naturalmente. Filtrar por tópico ou palavra-chave.
- **Memórias Permanentes** — grafos de conhecimento permanentes. Conceitos ligados por relações tipadas (`depends_on`, `contradicts`, `superseded_by`, ...). Filtrar por etiqueta.
- **Feedback** — registar correções quando as previsões da IA estão erradas. Pesquisar erros passados antes de fazer novas previsões. Aprendizagem em ciclo fechado.

## Instalação

```bash
# Homebrew (macOS / Linux)
brew tap rtk-ai/tap && brew install icm

# Instalação rápida
curl -fsSL https://raw.githubusercontent.com/rtk-ai/icm/main/install.sh | sh

# A partir do código-fonte
cargo install --path crates/icm-cli
```

## Configuração

```bash
# Detetar e configurar automaticamente todas as ferramentas suportadas
icm init
```

Configura **14 ferramentas** com um único comando:

| Ferramenta | Ficheiro de configuração | Formato |
|-----------|--------------------------|---------|
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

Ou manualmente:

```bash
# Claude Code
claude mcp add icm -- icm serve

# Modo compacto (respostas mais curtas, poupa tokens)
claude mcp add icm -- icm serve --compact

# Qualquer cliente MCP: command = "icm", args = ["serve"]
```

### Skills / regras

```bash
icm init --mode skill
```

Instala comandos slash e regras para Claude Code (`/recall`, `/remember`), Cursor (regra `.mdc`), Roo Code (regra `.md`), e Amp (`/icm-recall`, `/icm-remember`).

### Hooks (Claude Code)

```bash
icm init --mode hook
```

Instala todas as 3 camadas de extração como hooks do Claude Code:

**Hooks do Claude Code**:

| Hook | Evento | O que faz |
|------|--------|-----------|
| `icm hook pre` | PreToolUse | Autorizar automaticamente comandos CLI `icm` (sem pedido de permissão) |
| `icm hook post` | PostToolUse | Extrair factos da saída de ferramentas a cada 15 chamadas |
| `icm hook compact` | PreCompact | Extrair memórias do transcript antes da compressão de contexto |
| `icm hook prompt` | UserPromptSubmit | Injetar contexto recuperado no início de cada prompt |

**Plugin OpenCode** (instalado automaticamente em `~/.config/opencode/plugins/icm.js`):

| Evento OpenCode | Camada ICM | O que faz |
|----------------|------------|-----------|
| `tool.execute.after` | Camada 0 | Extrair factos da saída de ferramentas |
| `experimental.session.compacting` | Camada 1 | Extrair da conversa antes da compactação |
| `session.created` | Camada 2 | Recuperar contexto no início da sessão |

## CLI vs MCP

O ICM pode ser utilizado via CLI (comandos `icm`) ou servidor MCP (`icm serve`). Ambos acedem à mesma base de dados.

| | CLI | MCP |
|---|-----|-----|
| **Latência** | ~30ms (binário direto) | ~50ms (JSON-RPC stdio) |
| **Custo em tokens** | 0 (baseado em hooks, invisível) | ~20-50 tokens/chamada (esquema de ferramenta) |
| **Configuração** | `icm init --mode hook` | `icm init --mode mcp` |
| **Funciona com** | Claude Code, OpenCode (via hooks/plugins) | Todas as 14 ferramentas compatíveis com MCP |
| **Extração automática** | Sim (hooks acionam `icm extract`) | Sim (ferramentas MCP chamam store) |
| **Melhor para** | Utilizadores avançados, poupança de tokens | Compatibilidade universal |

## CLI

### Memórias (episódicas, com decaimento)

```bash
# Armazenar
icm store -t "meu-projeto" -c "Usar PostgreSQL para a BD principal" -i high -k "bd,postgres"

# Recuperar
icm recall "escolha de base de dados"
icm recall "configuração auth" --topic "meu-projeto" --limit 10
icm recall "arquitetura" --keyword "postgres"

# Gerir
icm forget <memory-id>
icm consolidate --topic "meu-projeto"
icm topics
icm stats

# Extrair factos de texto (baseado em regras, custo zero em LLM)
echo "O parser usa o algoritmo Pratt" | icm extract -p meu-projeto
```

### Memórias Permanentes (grafos de conhecimento permanentes)

```bash
# Criar uma memória permanente
icm memoir create -n "system-architecture" -d "Decisões de design do sistema"

# Adicionar conceitos com etiquetas
icm memoir add-concept -m "system-architecture" -n "auth-service" \
  -d "Gere tokens JWT e fluxos OAuth2" -l "domain:auth,type:service"

# Ligar conceitos
icm memoir link -m "system-architecture" --from "api-gateway" --to "auth-service" -r depends-on

# Pesquisar com filtro de etiqueta
icm memoir search -m "system-architecture" "autenticação"
icm memoir search -m "system-architecture" "service" --label "domain:auth"

# Inspecionar vizinhança
icm memoir inspect -m "system-architecture" "auth-service" -D 2

# Exportar grafo (formatos: json, dot, ascii, ai)
icm memoir export -m "system-architecture" -f ascii   # Caixas com barras de confiança
icm memoir export -m "system-architecture" -f dot      # Graphviz DOT (cor = nível de confiança)
icm memoir export -m "system-architecture" -f ai       # Markdown otimizado para contexto LLM
icm memoir export -m "system-architecture" -f json     # JSON estruturado com todos os metadados

# Gerar visualização SVG
icm memoir export -m "system-architecture" -f dot | dot -Tsvg > graph.svg
```

## Ferramentas MCP (22)

### Ferramentas de memória

| Ferramenta | Descrição |
|-----------|-----------|
| `icm_memory_store` | Armazenar com deduplicação automática (>85% de similaridade → atualizar em vez de duplicar) |
| `icm_memory_recall` | Pesquisar por consulta, filtrar por tópico e/ou palavra-chave |
| `icm_memory_update` | Editar uma memória no local (conteúdo, importância, palavras-chave) |
| `icm_memory_forget` | Apagar uma memória por ID |
| `icm_memory_consolidate` | Fundir todas as memórias de um tópico num único resumo |
| `icm_memory_list_topics` | Listar todos os tópicos com contagens |
| `icm_memory_stats` | Estatísticas globais de memória |
| `icm_memory_health` | Auditoria de higiene por tópico (antiguidade, necessidades de consolidação) |
| `icm_memory_embed_all` | Preencher embeddings retroativamente para pesquisa vetorial |

### Ferramentas de memórias permanentes (grafos de conhecimento)

| Ferramenta | Descrição |
|-----------|-----------|
| `icm_memoir_create` | Criar uma nova memória permanente (contentor de conhecimento) |
| `icm_memoir_list` | Listar todas as memórias permanentes |
| `icm_memoir_show` | Mostrar detalhes e todos os conceitos de uma memória permanente |
| `icm_memoir_add_concept` | Adicionar um conceito com etiquetas |
| `icm_memoir_refine` | Atualizar a definição de um conceito |
| `icm_memoir_search` | Pesquisa de texto completo, opcionalmente filtrada por etiqueta |
| `icm_memoir_search_all` | Pesquisar em todas as memórias permanentes |
| `icm_memoir_link` | Criar relação tipada entre conceitos |
| `icm_memoir_inspect` | Inspecionar conceito e vizinhança do grafo (BFS) |
| `icm_memoir_export` | Exportar grafo (json, dot, ascii, ai) com níveis de confiança |

### Ferramentas de feedback (aprender com os erros)

| Ferramenta | Descrição |
|-----------|-----------|
| `icm_feedback_record` | Registar uma correção quando uma previsão da IA estava errada |
| `icm_feedback_search` | Pesquisar correções passadas para informar previsões futuras |
| `icm_feedback_stats` | Estatísticas de feedback: contagem total, distribuição por tópico, mais aplicadas |

### Tipos de relação

`part_of` · `depends_on` · `related_to` · `contradicts` · `refines` · `alternative_to` · `caused_by` · `instance_of` · `superseded_by`

## Como funciona

### Modelo de memória dual

**Memória episódica (Tópicos)** captura decisões, erros, preferências. Cada memória tem um peso que decai com o tempo com base na importância:

| Importância | Decaimento | Pruning | Comportamento |
|------------|------------|---------|---------------|
| `critical` | nenhum | nunca | Nunca esquecida, nunca removida |
| `high` | lento (0.5x taxa) | nunca | Desvanece lentamente, nunca auto-eliminada |
| `medium` | normal | sim | Decaimento padrão, removida quando peso < limiar |
| `low` | rápido (2x taxa) | sim | Esquecida rapidamente |

O decaimento é **consciente do acesso**: memórias frequentemente recuperadas decaem mais lentamente (`decay / (1 + access_count × 0.1)`). Aplicado automaticamente na recuperação (se >24h desde o último decaimento).

**A higiene de memória** está incorporada:
- **Deduplicação automática**: armazenar conteúdo >85% similar a uma memória existente no mesmo tópico atualiza-a em vez de criar um duplicado
- **Dicas de consolidação**: quando um tópico ultrapassa 7 entradas, `icm_memory_store` avisa o chamador para consolidar
- **Auditoria de saúde**: `icm_memory_health` reporta contagem de entradas por tópico, peso médio, entradas obsoletas e necessidades de consolidação
- **Sem perda silenciosa de dados**: memórias críticas e de alta importância nunca são removidas automaticamente

**Memória semântica (Memórias Permanentes)** captura conhecimento estruturado como um grafo. Os conceitos são permanentes — são refinados, nunca decaem. Use `superseded_by` para marcar factos obsoletos em vez de os eliminar.

### Pesquisa híbrida

Com embeddings ativados, o ICM utiliza pesquisa híbrida:
- **FTS5 BM25** (30%) — correspondência de palavras-chave em texto completo
- **Similaridade cosseno** (70%) — pesquisa vetorial semântica via sqlite-vec

Modelo padrão: `intfloat/multilingual-e5-base` (768d, mais de 100 idiomas). Configurável no seu [ficheiro de configuração](#configuração):

```toml
[embeddings]
# enabled = false                          # Desativar completamente (sem download do modelo)
model = "intfloat/multilingual-e5-base"    # 768d, multilíngue (padrão)
# model = "intfloat/multilingual-e5-small" # 384d, multilíngue (mais leve)
# model = "intfloat/multilingual-e5-large" # 1024d, multilíngue (melhor precisão)
# model = "Xenova/bge-small-en-v1.5"      # 384d, apenas inglês (mais rápido)
# model = "jinaai/jina-embeddings-v2-base-code"  # 768d, otimizado para código
```

Para ignorar completamente o download do modelo de embeddings, use um destes:
```bash
icm --no-embeddings serve          # Flag CLI
ICM_NO_EMBEDDINGS=1 icm serve     # Variável de ambiente
```
Ou defina `enabled = false` no seu ficheiro de configuração. O ICM recorrerá à pesquisa por palavras-chave FTS5 (ainda funciona, apenas sem correspondência semântica).

Alterar o modelo recria automaticamente o índice vetorial (os embeddings existentes são limpos e podem ser regenerados com `icm_memory_embed_all`).

### Armazenamento

Ficheiro SQLite único. Sem serviços externos, sem dependência de rede.

```
~/Library/Application Support/dev.icm.icm/memories.db                    # macOS
~/.local/share/dev.icm.icm/memories.db                                   # Linux
C:\Users\<user>\AppData\Local\icm\icm\data\memories.db                   # Windows
```

### Configuração

```bash
icm config                    # Mostrar configuração ativa
```

Localização do ficheiro de configuração (específico por plataforma, ou `$ICM_CONFIG`):

```
~/Library/Application Support/dev.icm.icm/config.toml                    # macOS
~/.config/icm/config.toml                                                # Linux
C:\Users\<user>\AppData\Roaming\icm\icm\config\config.toml              # Windows
```

Consulte [config/default.toml](config/default.toml) para todas as opções.

## Extração automática

O ICM extrai memórias automaticamente através de três camadas:

```
  Camada 0: Hooks de padrões     Camada 1: PreCompact          Camada 2: UserPromptSubmit
  (custo zero em LLM)            (custo zero em LLM)           (custo zero em LLM)
  ┌──────────────────┐                ┌──────────────────┐          ┌──────────────────┐
  │ Hook PostToolUse  │                │ Hook PreCompact   │          │ UserPromptSubmit  │
  │                   │                │                   │          │                   │
  │ • Erros Bash      │                │ Contexto prestes  │          │ Utilizador envia  │
  │ • commits git     │                │ a ser comprimido→ │          │ prompt → icm      │
  │ • mudanças config │                │ extrair memórias  │          │ recall → injetar  │
  │ • decisões        │                │ do transcript     │          │ contexto          │
  │ • preferências    │                │ antes de serem    │          │                   │
  │ • aprendizagens   │                │ perdidas para     │          │ Agente inicia com │
  │ • restrições      │                │ sempre            │          │ memórias relevantes│
  │                   │                │                   │          │ já carregadas     │
  │ Baseado em regras,│                │ Mesmos padrões +  │          │                   │
  │ sem LLM           │                │ --store-raw fallbk│          │                   │
  └──────────────────┘                └──────────────────┘          └──────────────────┘
```

| Camada | Estado | Custo LLM | Comando hook | Descrição |
|--------|--------|-----------|--------------|-----------|
| Camada 0 | Implementada | 0 | `icm hook post` | Extração de palavras-chave baseada em regras da saída de ferramentas |
| Camada 1 | Implementada | 0 | `icm hook compact` | Extrair do transcript antes da compressão de contexto |
| Camada 2 | Implementada | 0 | `icm hook prompt` | Injetar memórias recuperadas em cada prompt do utilizador |

As 3 camadas são instaladas automaticamente por `icm init --mode hook`.

### Comparação com alternativas

| Sistema | Método | Custo LLM | Latência | Captura compactação? |
|---------|--------|-----------|----------|---------------------|
| **ICM** | Extração em 3 camadas | 0 a ~500 tok/sessão | 0ms | **Sim (PreCompact)** |
| Mem0 | 2 chamadas LLM/mensagem | ~2k tok/mensagem | 200-2000ms | Não |
| claude-mem | PostToolUse + async | ~1-5k tok/sessão | 8ms hook | Não |
| MemGPT/Letta | Agente auto-gere | 0 marginal | 0ms | Não |
| DiffMem | Diffs baseados em Git | 0 | 0ms | Não |

## Benchmarks

### Desempenho de armazenamento

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

Apple M1 Pro, SQLite em memória, single-threaded. `icm bench --count 1000`

### Eficiência do agente

Fluxo de trabalho multi-sessão com um projeto Rust real (12 ficheiros, ~550 linhas). As sessões 2+ mostram os maiores ganhos à medida que o ICM recupera em vez de reler ficheiros.

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

### Retenção de conhecimento

O agente recupera factos específicos de um documento técnico denso ao longo de sessões. A sessão 1 lê e memoriza; as sessões 2+ respondem a 10 perguntas factuais **sem** o texto de origem.

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

### LLMs locais (ollama)

Mesmo teste com modelos locais — injeção de contexto puro, sem necessidade de uso de ferramentas.

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

### Protocolo de teste

Todos os benchmarks utilizam **chamadas API reais** — sem mocks, sem respostas simuladas, sem respostas em cache.

- **Benchmark de agente**: Cria um projeto Rust real num diretório temporário. Executa N sessões com `claude -p --output-format json`. Sem ICM: configuração MCP vazia. Com ICM: servidor MCP real + extração automática + injeção de contexto.
- **Retenção de conhecimento**: Utiliza um documento técnico fictício (o "Protocolo Meridian"). Pontua as respostas por correspondência de palavras-chave com factos esperados. Timeout de 120s por invocação.
- **Isolamento**: Cada execução utiliza o seu próprio diretório temporário e uma BD SQLite nova. Sem persistência de sessão.

## Documentação

| Documento | Descrição |
|-----------|-----------|
| [Arquitetura Técnica](docs/architecture.md) | Estrutura de crates, pipeline de pesquisa, modelo de decaimento, integração sqlite-vec, testes |
| [Guia do Utilizador](docs/guide.md) | Instalação, organização de tópicos, consolidação, extração, resolução de problemas |
| [Visão Geral do Produto](docs/product.md) | Casos de uso, benchmarks, comparação com alternativas |

## Licença

[Source-Available](LICENSE) — Gratuito para indivíduos e equipas ≤ 20 pessoas. Licença empresarial necessária para organizações maiores. Contacto: contact@rtk-ai.app
