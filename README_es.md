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

Configura **14 herramientas** con un solo comando:

| Herramienta | Archivo de configuración | Formato |
|-------------|--------------------------|---------|
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

### Hooks (Claude Code)

```bash
icm init --mode hook
```

Instala las 3 capas de extracción como hooks de Claude Code:

**Hooks de Claude Code**:

| Hook | Evento | Qué hace |
|------|--------|----------|
| `icm hook pre` | PreToolUse | Permite automáticamente los comandos CLI `icm` (sin prompt de permiso) |
| `icm hook post` | PostToolUse | Extrae hechos de la salida de herramientas cada 15 llamadas |
| `icm hook compact` | PreCompact | Extrae memorias del transcript antes de la compresión de contexto |
| `icm hook prompt` | UserPromptSubmit | Inyecta contexto recuperado al inicio de cada prompt |

**Plugin de OpenCode** (instalado automáticamente en `~/.config/opencode/plugins/icm.js`):

| Evento de OpenCode | Capa ICM | Qué hace |
|--------------------|----------|----------|
| `tool.execute.after` | Capa 0 | Extrae hechos de la salida de herramientas |
| `experimental.session.compacting` | Capa 1 | Extrae de la conversación antes de la compactación |
| `session.created` | Capa 2 | Recupera contexto al inicio de la sesión |

## CLI vs MCP

ICM puede usarse vía CLI (comandos `icm`) o servidor MCP (`icm serve`). Ambos acceden a la misma base de datos.

| | CLI | MCP |
|---|-----|-----|
| **Latencia** | ~30ms (binario directo) | ~50ms (JSON-RPC stdio) |
| **Coste en tokens** | 0 (basado en hooks, invisible) | ~20-50 tokens/llamada (esquema de herramienta) |
| **Configuración** | `icm init --mode hook` | `icm init --mode mcp` |
| **Compatible con** | Claude Code, OpenCode (vía hooks/plugins) | Las 14 herramientas compatibles con MCP |
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

## Herramientas MCP (22)

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

## Documentación

| Documento | Descripción |
|-----------|-------------|
| [Arquitectura técnica](docs/architecture.md) | Estructura de crates, pipeline de búsqueda, modelo de decaimiento, integración sqlite-vec, pruebas |
| [Guía de usuario](docs/guide.md) | Instalación, organización de temas, consolidación, extracción, resolución de problemas |
| [Descripción del producto](docs/product.md) | Casos de uso, benchmarks, comparación con alternativas |

## Licencia

[Source-Available](LICENSE) — Gratuito para individuos y equipos de ≤ 20 personas. Se requiere licencia empresarial para organizaciones más grandes. Contacto: license@rtk.ai
