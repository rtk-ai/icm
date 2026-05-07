[English](README.md) | [Français](README_fr.md) | **Español** | [Deutsch](README_de.md) | [Italiano](README_it.md) | [Português](README_pt.md) | [Nederlands](README_nl.md) | [Polski](README_pl.md) | [Русский](README_ru.md) | [日本語](README_ja.md) | [中文](README_zh.md) | [العربية](README_ar.md) | [한국어](README_ko.md)

<p align="center">
  <img src="assets/banner.png" alt="ICM — Infinite Context Memory" width="600">
</p>

<h1 align="center">ICM</h1>

<p align="center">
  Memoria permanente para agentes de IA. Binario único, sin dependencias, MCP nativo.
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/icm/actions/workflows/ci.yml"><img src="https://github.com/rtk-ai/icm/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/icm/releases/latest"><img src="https://img.shields.io/github/v/release/rtk-ai/icm?color=purple" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-Source--Available-orange.svg" alt="Source-Available"></a>
</p>

---

ICM le da a tu agente de IA una memoria real — no una herramienta de notas, no un gestor de contexto, una **memoria**.

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

**Dos modelos de memoria:**

- **Memories** — almacena/recupera con decaimiento temporal por importancia. Las memorias críticas nunca desaparecen, las de baja importancia decaen de forma natural. Filtra por tema o palabra clave.
- **Memoirs** — grafos de conocimiento permanentes. Conceptos vinculados por relaciones tipadas (`depends_on`, `contradicts`, `superseded_by`, ...). Filtra por etiqueta.
- **Feedback** — registra correcciones cuando las predicciones de la IA son incorrectas. Busca errores pasados antes de hacer nuevas predicciones. Aprendizaje de bucle cerrado.

## Instalación

```bash
# Homebrew (macOS / Linux)
brew tap rtk-ai/tap && brew install icm

# Instalación rápida
curl -fsSL https://raw.githubusercontent.com/rtk-ai/icm/main/install.sh | sh

# Desde el código fuente
cargo install --path crates/icm-cli
```

## Configuración inicial

```bash
# Detectar y configurar automáticamente todas las herramientas soportadas
icm init
```

Configura **17 herramientas** con un solo comando ([guia de integración completa](docs/integrations.md)):

| Herramienta | MCP | Hooks | CLI | Skills |
|-------------|:---:|:-----:|:---:|:------:|
| Claude Code | `~/.claude.json` | 5 hooks | `CLAUDE.md` | `/recall` `/remember` |
| Claude Desktop | JSON | — | — | — |
| Gemini CLI | `~/.gemini/settings.json` | 5 hooks | `GEMINI.md` | — |
| Codex CLI | `~/.codex/config.toml` | 4 hooks | `AGENTS.md` | — |
| Copilot CLI | `~/.copilot/mcp-config.json` | 4 hooks | `.github/copilot-instructions.md` | — |
| Cursor | `~/.cursor/mcp.json` | — | — | regla `.mdc` |
| Windsurf | JSON | — | `.windsurfrules` | — |
| VS Code | `~/Library/.../Code/User/mcp.json` | — | — | — |
| Amp | JSON | — | — | `/icm-recall` `/icm-remember` |
| Amazon Q | JSON | — | — | — |
| Cline | VS Code globalStorage | — | — | — |
| Roo Code | VS Code globalStorage | — | — | regla `.md` |
| Kilo Code | VS Code globalStorage | — | — | — |
| Zed | `~/.zed/settings.json` | — | — | — |
| OpenCode | JSON | plugin TS | — | — |
| Continue.dev | `~/.continue/config.yaml` | — | — | — |
| Aider | — | — | `.aider.conventions.md` | — |

O manualmente:

```bash
# Claude Code
claude mcp add icm -- icm serve

# Modo compacto (respuestas más cortas, ahorra tokens)
claude mcp add icm -- icm serve --compact

# Cualquier cliente MCP: command = "icm", args = ["serve"]
```

### Skills / rules

```bash
icm init --mode skill
```

Instala comandos slash y reglas para Claude Code (`/recall`, `/remember`), Cursor (regla `.mdc`), Roo Code (regla `.md`) y Amp (`/icm-recall`, `/icm-remember`).

### Hooks (5 herramientas)

```bash
icm init --mode hook
```

Instala hooks de auto-extracción y auto-recuperación para todas las herramientas soportadas:

| Herramienta | SessionStart | PreTool | PostTool | Compact | PromptRecall | Config |
|-------------|:-----------:|:-------:|:--------:|:-------:|:------------:|--------|
| Claude Code | `icm hook start` | `icm hook pre` | `icm hook post` | `icm hook compact` | `icm hook prompt` | `~/.claude/settings.json` |
| Gemini CLI | `icm hook start` | `icm hook pre` | `icm hook post` | `icm hook compact` | `icm hook prompt` | `~/.gemini/settings.json` |
| Codex CLI | `icm hook start` | `icm hook pre` | `icm hook post` | — | `icm hook prompt` | `~/.codex/hooks.json` |
| Copilot CLI | `icm hook start` | `icm hook pre` | `icm hook post` | — | `icm hook prompt` | `.github/hooks/icm.json` |
| OpenCode | session start | — | tool extract | compaction | — | `~/.config/opencode/plugins/icm.ts` |

**Qué hace cada hook:**

| Hook | Qué hace |
|------|----------|
| `icm hook start` | Inyecta un paquete de inicio con memorias críticas/alta importancia al inicio de sesión (~500 tokens) |
| `icm hook pre` | Permite automáticamente los comandos CLI `icm` (sin prompt de permiso) |
| `icm hook post` | Extrae hechos de la salida de herramientas cada N llamadas (auto-extracción) |
| `icm hook compact` | Extrae memorias del transcript antes de la compresión de contexto |
| `icm hook prompt` | Inyecta contexto recuperado al inicio de cada prompt del usuario |

## CLI vs MCP

ICM puede usarse vía CLI (comandos `icm`) o servidor MCP (`icm serve`). Ambos acceden a la misma base de datos.

| | CLI | MCP |
|---|-----|-----|
| **Latencia** | ~30ms (binario directo) | ~50ms (JSON-RPC stdio) |
| **Coste en tokens** | 0 (basado en hooks, invisible) | ~20-50 tokens/llamada (esquema de herramienta) |
| **Configuración** | `icm init --mode hook` | `icm init --mode mcp` |
| **Compatible con** | Claude Code, Gemini, Codex, Copilot, OpenCode (vía hooks) | Las 17 herramientas compatibles con MCP |
| **Auto-extracción** | Sí (los hooks lanzan `icm extract`) | Sí (las herramientas MCP llaman a store) |
| **Ideal para** | Usuarios avanzados, ahorro de tokens | Compatibilidad universal |

## CLI

### Memories (episódicas, con decaimiento)

```bash
# Almacenar
icm store -t "my-project" -c "Use PostgreSQL for the main DB" -i high -k "db,postgres"

# Recuperar
icm recall "database choice"
icm recall "auth setup" --topic "my-project" --limit 10
icm recall "architecture" --keyword "postgres"

# Gestionar
icm forget <memory-id>
icm consolidate --topic "my-project"
icm topics
icm stats

# Extraer hechos de texto (basado en reglas, sin coste LLM)
echo "The parser uses Pratt algorithm" | icm extract -p my-project
```

### Memoirs (grafos de conocimiento permanentes)

```bash
# Crear un memoir
icm memoir create -n "system-architecture" -d "System design decisions"

# Añadir conceptos con etiquetas
icm memoir add-concept -m "system-architecture" -n "auth-service" \
  -d "Handles JWT tokens and OAuth2 flows" -l "domain:auth,type:service"

# Vincular conceptos
icm memoir link -m "system-architecture" --from "api-gateway" --to "auth-service" -r depends-on

# Buscar con filtro de etiqueta
icm memoir search -m "system-architecture" "authentication"
icm memoir search -m "system-architecture" "service" --label "domain:auth"

# Inspeccionar vecindad
icm memoir inspect -m "system-architecture" "auth-service" -D 2

# Exportar grafo (formatos: json, dot, ascii, ai)
icm memoir export -m "system-architecture" -f ascii   # Dibujo en caja con barras de confianza
icm memoir export -m "system-architecture" -f dot      # Graphviz DOT (color = nivel de confianza)
icm memoir export -m "system-architecture" -f ai       # Markdown optimizado para contexto LLM
icm memoir export -m "system-architecture" -f json     # JSON estructurado con todos los metadatos

# Generar visualización SVG
icm memoir export -m "system-architecture" -f dot | dot -Tsvg > graph.svg
```

## Herramientas MCP (31)

### Herramientas de Memory

| Herramienta | Descripción |
|-------------|-------------|
| `icm_memory_store` | Almacena con deduplicación automática (>85% similitud → actualiza en lugar de duplicar) |
| `icm_memory_recall` | Busca por consulta, filtra por tema y/o palabra clave |
| `icm_memory_update` | Edita una memoria en el lugar (contenido, importancia, palabras clave) |
| `icm_memory_forget` | Elimina una memoria por ID |
| `icm_memory_consolidate` | Fusiona todas las memorias de un tema en un único resumen |
| `icm_memory_list_topics` | Lista todos los temas con sus conteos |
| `icm_memory_stats` | Estadísticas globales de memoria |
| `icm_memory_health` | Auditoría de higiene por tema (antigüedad, necesidades de consolidación) |
| `icm_memory_embed_all` | Rellena embeddings para búsqueda vectorial |

### Herramientas de Memoir (grafos de conocimiento)

| Herramienta | Descripción |
|-------------|-------------|
| `icm_memoir_create` | Crea un nuevo memoir (contenedor de conocimiento) |
| `icm_memoir_list` | Lista todos los memoirs |
| `icm_memoir_show` | Muestra los detalles del memoir y todos los conceptos |
| `icm_memoir_add_concept` | Añade un concepto con etiquetas |
| `icm_memoir_refine` | Actualiza la definición de un concepto |
| `icm_memoir_search` | Búsqueda de texto completo, opcionalmente filtrada por etiqueta |
| `icm_memoir_search_all` | Busca en todos los memoirs |
| `icm_memoir_link` | Crea una relación tipada entre conceptos |
| `icm_memoir_inspect` | Inspecciona el concepto y la vecindad del grafo (BFS) |
| `icm_memoir_export` | Exporta el grafo (json, dot, ascii, ai) con niveles de confianza |

### Herramientas de Feedback (aprendizaje de errores)

| Herramienta | Descripción |
|-------------|-------------|
| `icm_feedback_record` | Registra una corrección cuando una predicción de IA fue incorrecta |
| `icm_feedback_search` | Busca correcciones pasadas para informar predicciones futuras |
| `icm_feedback_stats` | Estadísticas de feedback: total, desglose por tema, más aplicados |

### Tipos de relación

`part_of` · `depends_on` · `related_to` · `contradicts` · `refines` · `alternative_to` · `caused_by` · `instance_of` · `superseded_by`

## Cómo funciona

### Modelo de memoria dual

**Memoria episódica (Topics)** captura decisiones, errores, preferencias. Cada memoria tiene un peso que decae con el tiempo según la importancia:

| Importancia | Decaimiento | Poda | Comportamiento |
|-------------|-------------|------|----------------|
| `critical` | ninguno | nunca | Nunca olvidada, nunca podada |
| `high` | lento (0.5x tasa) | nunca | Desvanece lentamente, nunca se elimina automáticamente |
| `medium` | normal | sí | Decaimiento estándar, podada cuando el peso < umbral |
| `low` | rápido (2x tasa) | sí | Olvidada rápidamente |

El decaimiento es **consciente del acceso**: las memorias frecuentemente recuperadas decaen más lento (`decay / (1 + access_count × 0.1)`). Se aplica automáticamente al recuperar (si han pasado >24h desde el último decaimiento).

**La higiene de memoria** está incorporada:
- **Deduplicación automática**: almacenar contenido >85% similar a una memoria existente en el mismo tema la actualiza en lugar de crear un duplicado
- **Avisos de consolidación**: cuando un tema supera las 7 entradas, `icm_memory_store` advierte al llamador que consolide
- **Auditoría de salud**: `icm_memory_health` reporta el número de entradas por tema, peso promedio, entradas antiguas y necesidades de consolidación
- **Sin pérdida silenciosa de datos**: las memorias críticas y de alta importancia nunca se podan automáticamente

**Memoria semántica (Memoirs)** captura conocimiento estructurado como un grafo. Los conceptos son permanentes — se refinan, nunca decaen. Usa `superseded_by` para marcar hechos obsoletos en lugar de eliminarlos.

### Búsqueda híbrida

Con embeddings habilitados, ICM usa búsqueda híbrida:
- **FTS5 BM25** (30%) — coincidencia de texto completo por palabras clave
- **Similitud coseno** (70%) — búsqueda vectorial semántica vía sqlite-vec

Modelo por defecto: `intfloat/multilingual-e5-base` (768d, más de 100 idiomas). Configurable en tu [archivo de configuración](#configuración):

```toml
[embeddings]
# enabled = false                          # Deshabilitar completamente (sin descarga de modelo)
model = "intfloat/multilingual-e5-base"    # 768d, multilingüe (por defecto)
# model = "intfloat/multilingual-e5-small" # 384d, multilingüe (más ligero)
# model = "intfloat/multilingual-e5-large" # 1024d, multilingüe (mejor precisión)
# model = "Xenova/bge-small-en-v1.5"      # 384d, solo inglés (más rápido)
# model = "jinaai/jina-embeddings-v2-base-code"  # 768d, optimizado para código
```

Para omitir completamente la descarga del modelo de embeddings, usa cualquiera de estos:
```bash
icm --no-embeddings serve          # Flag CLI
ICM_NO_EMBEDDINGS=1 icm serve     # Variable de entorno
```
O establece `enabled = false` en tu archivo de configuración. ICM recurrirá a la búsqueda por palabras clave FTS5 (sigue funcionando, simplemente sin coincidencia semántica).

Cambiar el modelo recrea automáticamente el índice vectorial (los embeddings existentes se borran y pueden regenerarse con `icm_memory_embed_all`).

### Almacenamiento

Archivo SQLite único. Sin servicios externos, sin dependencia de red.

```
~/Library/Application Support/dev.icm.icm/memories.db                    # macOS
~/.local/share/dev.icm.icm/memories.db                                   # Linux
C:\Users\<user>\AppData\Local\icm\icm\data\memories.db                   # Windows
```

### Configuración

```bash
icm config                    # Mostrar configuración activa
```

Ubicación del archivo de configuración (específico por plataforma, o `$ICM_CONFIG`):

```
~/Library/Application Support/dev.icm.icm/config.toml                    # macOS
~/.config/icm/config.toml                                                # Linux
C:\Users\<user>\AppData\Roaming\icm\icm\config\config.toml              # Windows
```

Ver [config/default.toml](config/default.toml) para todas las opciones.

## Multi-proyecto y multi-agente

ICM está diseñado para el caso en el que un usuario colabora con muchos agentes a través de muchos proyectos. Las memorias deben mantenerse relevantes: una decisión del proyecto A nunca debería filtrarse al proyecto B, y un agente `dev` no debería ser hidratado con lo que un agente `mentor` almacenó.

### Aislamiento por proyecto

ICM delimita las memorias mediante una **convención de nomenclatura de topics**, no mediante una columna separada. La convención:

```
{kind}-{project}              # e.g. decisions-icm, errors-resolved-icm, contexte-rtk-cloud
preferences                   # global, always included
identity                      # global, always included
```

`icm_wake_up { project: "icm" }` realiza una correspondencia **consciente de segmentos**: `"icm"` coincide con `decisions-icm`, `errors-icm-core`, `contexte-icm` — pero nunca con `icmp-notes` (sin falsos positivos). Los topics se dividen por `-`, `.`, `_`, `/`, `:`. Los topics de preferencias e identidad son entre proyectos por diseño — la guía a nivel de usuario nunca se elimina.

El hook `UserPromptSubmit` (`icm hook prompt`) y el hook `SessionStart` (`icm hook start`) derivan ambos el proyecto a partir del campo `cwd` en el JSON del hook (`basename` del directorio de trabajo). Ejecuta cada proyecto desde su propio directorio y el aislamiento es automático.

### Cómo escribir buenas memorias

`icm_memory_store` requiere que el agente elija `topic` y `content` — no hay clasificador automático. Mejores prácticas:

| Campo | Recomendación |
|------|----------|
| `topic` | `{kind}-{project}`. Tipos: `decisions`, `errors-resolved`, `contexte`, `preferences`. |
| `content` | Un hecho por almacenamiento. Resumen denso en inglés — `topic + content` es el texto del embedding. |
| `raw_excerpt` | Solo verbatim (código, mensaje de error exacto, salida de comando). |
| `keywords` | 3–5 términos para potenciar la recuperación BM25. |
| `importance` | `critical` para nunca olvidar, `high` para decisiones de proyecto, `medium` por defecto, `low` para efímeros. |

ICM se encarga del resto: **deduplicación al 85% de similitud**, **enlace automático** entre memorias semánticamente cercanas, **consolidación automática** por encima de 10 entradas por topic, y **decay** ponderado por número de accesos. Un hecho por llamada supera los volcados por lotes — el recuperador clasifica más alto los hechos almacenados individualmente.

### Roles multi-agente

ICM aún no dispone de una columna `role` de primera clase. Hoy en día, los roles se emulan mediante sufijos de topic más directorios de trabajo por agente:

```
decisions-icm-dev             # dev agent: code patterns, library choices, refactors
decisions-icm-architect       # architect: design, workflows, subtask decomposition
decisions-icm-mentor          # mentor / BA: business goals, non-technical context
```

Cada agente se ejecuta en su propio directorio de trabajo (`~/projects/icm-dev/`, `~/projects/icm-architect/`, ...) de modo que `icm hook prompt` e `icm hook start` deriven un segmento de proyecto distinto a partir de `cwd` y solo recuperen las memorias correspondientes. Las preferencias siguen siendo globales — la identidad del usuario se mantiene a través de todos los roles.

Dentro de un mismo agente, también puedes restringir manualmente el recall:

```jsonc
// icm_memory_recall
{ "query": "auth flow", "topic": "decisions-icm-architect", "limit": 5 }
```

Un campo `role` de primera clase (con filtrado nativo en wake-up y recall) está en la hoja de ruta. Hasta entonces, la convención de sufijos de topic es el patrón soportado.

## Auto-extracción

ICM extrae memorias automáticamente mediante tres capas:

```
  Capa 0: Hooks de patrones       Capa 1: PreCompact           Capa 2: UserPromptSubmit
  (sin coste LLM)                 (sin coste LLM)               (sin coste LLM)
  ┌──────────────────┐                ┌──────────────────┐          ┌──────────────────┐
  │ Hook PostToolUse  │                │ Hook PreCompact   │          │ UserPromptSubmit  │
  │                   │                │                   │          │                   │
  │ • Errores Bash    │                │ El contexto está  │          │ El usuario envía  │
  │ • git commits     │                │ a punto de        │          │ un prompt         │
  │ • cambios config  │                │ comprimirse →     │          │ → icm recall      │
  │ • decisiones      │                │ extraer memorias  │          │ → inyectar cont.  │
  │ • preferencias    │                │ del transcript    │          │                   │
  │ • aprendizajes    │                │ antes de perderlos│          │ El agente empieza  │
  │ • restricciones   │                │ para siempre      │          │ con memorias       │
  │                   │                │                   │          │ relevantes ya      │
  │ Basado en reglas, │                │ Mismos patrones + │          │ cargadas           │
  │ sin LLM           │                │ --store-raw fallbk│          │                   │
  └──────────────────┘                └──────────────────┘          └──────────────────┘
```

| Capa | Estado | Coste LLM | Comando hook | Descripción |
|------|--------|-----------|-------------|-------------|
| Capa 0 | Implementada | 0 | `icm hook post` | Extracción de palabras clave basada en reglas de la salida de herramientas |
| Capa 1 | Implementada | 0 | `icm hook compact` | Extrae del transcript antes de la compresión de contexto |
| Capa 2 | Implementada | 0 | `icm hook prompt` | Inyecta memorias recuperadas en cada prompt del usuario |

Las 3 capas se instalan automáticamente con `icm init --mode hook`.

### Comparación con alternativas

| Sistema | Método | Coste LLM | Latencia | ¿Captura compactación? |
|---------|--------|-----------|---------|------------------------|
| **ICM** | Extracción 3 capas | 0 a ~500 tok/sesión | 0ms | **Sí (PreCompact)** |
| Mem0 | 2 llamadas LLM/mensaje | ~2k tok/mensaje | 200-2000ms | No |
| claude-mem | PostToolUse + async | ~1-5k tok/sesión | 8ms hook | No |
| MemGPT/Letta | El agente se gestiona solo | 0 marginal | 0ms | No |
| DiffMem | Diffs basados en Git | 0 | 0ms | No |

## Benchmarks

### Rendimiento de almacenamiento

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

Apple M1 Pro, SQLite en memoria, monohilo. `icm bench --count 1000`

### Eficiencia del agente

Flujo de trabajo multisesión con un proyecto Rust real (12 archivos, ~550 líneas). Las sesiones 2+ muestran las mayores ganancias ya que ICM recupera en lugar de releer archivos.

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

### Retención de conocimiento

El agente recupera hechos específicos de un documento técnico denso entre sesiones. La sesión 1 lee y memoriza; las sesiones 2+ responden 10 preguntas de hecho **sin** el texto fuente.

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

### LLMs locales (ollama)

La misma prueba con modelos locales — inyección de contexto puro, sin necesidad de uso de herramientas.

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

### Protocolo de pruebas

Todos los benchmarks usan **llamadas API reales** — sin mocks, sin respuestas simuladas, sin respuestas en caché.

- **Benchmark de agente**: Crea un proyecto Rust real en un directorio temporal. Ejecuta N sesiones con `claude -p --output-format json`. Sin ICM: configuración MCP vacía. Con ICM: servidor MCP real + auto-extracción + inyección de contexto.
- **Retención de conocimiento**: Usa un documento técnico ficticio (el "Protocolo Meridian"). Puntúa las respuestas por coincidencia de palabras clave contra hechos esperados. Tiempo límite de 120s por invocación.
- **Aislamiento**: Cada ejecución usa su propio directorio temporal y base de datos SQLite nueva. Sin persistencia de sesión.

### Memoria unificada multi-agente

Las 17 herramientas comparten la misma base de datos SQLite. Una memoria almacenada por Claude está disponible instantáneamente para Gemini, Codex, Copilot, Cursor y todas las demás herramientas.

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

Score = 60% precisión de recuperación + 30% detalle de hechos + 10% velocidad. **98% de eficiencia multi-agente.**

## Por qué ICM

| Capacidad | ICM | Mem0 | Engram | AgentMemory |
|-----------|:---:|:----:|:------:|:-----------:|
| Soporte de herramientas | **17** | Solo SDK | ~6-8 | ~10 |
| Configuración en un comando | `icm init` | SDK manual | manual | manual |
| Hooks (auto-recuperación al inicio) | 5 herramientas | ninguno | vía MCP | 1 herramienta |
| Búsqueda híbrida (FTS5 + vector) | 30/70 ponderado | solo vector | solo FTS5 | FTS5+vector |
| Embeddings multilingües | 100+ idiomas (768d) | depende | ninguno | Inglés 384d |
| Grafo de conocimiento | Sistema Memoir | ninguno | ninguno | ninguno |
| Decaimiento temporal + consolidación | sensible al acceso | ninguno | básico | básico |
| Dashboard TUI | `icm dashboard` | ninguno | sí | visor web |
| Auto-extracción desde salida de herramientas | 3 capas, cero LLM | ninguno | ninguno | ninguno |
| Bucle de feedback/corrección | `icm_feedback_*` | ninguno | ninguno | ninguno |
| Runtime | Binario Rust único | Python | Go | Node.js |
| Local-first, sin dependencias | Archivo SQLite | cloud-first | SQLite | SQLite |
| Precisión de recuperación multi-agente | **98%** | N/A | N/A | 95.2% |

## Documentación

| Documento | Descripción |
|-----------|-------------|
| [Guía de integración](docs/integrations.md) | Configuración para las 17 herramientas: Claude Code, Copilot, Cursor, Windsurf, Zed, Amp, etc. |
| [Arquitectura técnica](docs/architecture.md) | Estructura de crates, pipeline de búsqueda, modelo de decaimiento, integración sqlite-vec, pruebas |
| [Guía de usuario](docs/guide.md) | Instalación, organización de temas, consolidación, extracción, resolución de problemas |
| [Descripción del producto](docs/product.md) | Casos de uso, benchmarks, comparación con alternativas |

## Licencia

[Source-Available](LICENSE) — Gratuito para individuos y equipos de ≤ 20 personas. Se requiere licencia empresarial para organizaciones más grandes. Contacto: contact@rtk-ai.app
