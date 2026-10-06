[English](README.md) | [Français](README_fr.md) | [Español](README_es.md) | [Deutsch](README_de.md) | [Italiano](README_it.md) | [Português](README_pt.md) | [Nederlands](README_nl.md) | [Polski](README_pl.md) | [Русский](README_ru.md) | [日本語](README_ja.md) | [中文](README_zh.md) | [العربية](README_ar.md) | [한국어](README_ko.md)

Esta es una traducción de README.md (inglés), que es la referencia; si difieren, la versión en inglés es la correcta.

<h1 align="center">ICM</h1>

<p align="center">
  <b>Memoria a largo plazo para agentes de programación con IA, compartida entre tus herramientas.</b><br>
  Un binario, un archivo SQLite. Ninguna llamada a un LLM para guardar o recuperar una memoria.
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/icm/actions/workflows/ci.yml"><img src="https://github.com/rtk-ai/icm/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/icm/releases/latest"><img src="https://img.shields.io/github/v/release/rtk-ai/icm?color=purple" alt="Versión"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-Apache--2.0-blue.svg" alt="Apache-2.0"></a>
</p>

Explícale a Claude Code el lunes cómo gestiona tu proyecto la autenticación, y la sesión de Gemini CLI del martes ya lo sabe. ICM guarda lo que aprenden tus agentes de programación (decisiones, correcciones, convenciones, preferencias) en un único archivo SQLite en tu máquina, y les devuelve la parte relevante al inicio de cada sesión y con cada prompt que envías. Hasta 18 agentes y editores comparten esa memoria, así que dejas de volver a explicar tu proyecto cada vez que abres una sesión o cambias de herramienta.

- **92.8% en LoCoMo (1,540 preguntas), al nivel de Hindsight (92.0%)**, con un tercio menos de contexto por pregunta. [Detalles y salvedades más abajo](#benchmark-comparison).
- **Ninguna llamada a un LLM para guardar o recuperar.** Hindsight, Mem0, Graphiti (Zep) y claude-mem llaman por defecto a un LLM por cada memoria que guardan. ICM no. Solo su extracción automática de hechos a partir de la salida de las herramientas pasa por la herramienta de línea de comandos de LLM que ya usas, cuando hay una instalada; `provider = "none"` también la mantiene local (ver [Inicio rápido](#quickstart)).
- **Útil sin modelo de embeddings.** La recuperación solo por palabras clave sitúa al menos una de las sesiones correctas en el top 5 para el 88.6% de las preguntas de LoCoMo, con una mediana de 6.8 ms por consulta en Linux x86-64.
- **No va por delante en todo.** En PersonaMem (las preferencias cambiantes de un usuario), Hindsight lidera: 86.6% frente al 82.9% de ICM.

<p align="center">
  <img src="assets/demo.svg" alt="Terminal: tres memorias guardadas con icm store y luego dos preguntas respondidas por icm recall, cada una devolviendo la memoria correcta">
</p>

<a id="quickstart"></a>

## Inicio rápido

```bash
brew tap rtk-ai/tap && brew install icm    # or: curl -fsSL https://raw.githubusercontent.com/rtk-ai/icm/main/install.sh | sh
icm init                                   # instructions, skills and hooks for every agent it detects
```

Eso es toda la configuración. Abre una nueva sesión en Claude Code, Codex, Gemini CLI o Copilot CLI: tu agente empieza ahora con un paquete corto de sus memorias más importantes y recibe las memorias relevantes para cada prompt que envías. Lo que aprende de la salida de sus herramientas se pone en cola, y la cola se convierte en memorias al final de cada sesión de Claude Code; con las demás herramientas, ejecuta `icm extract-pending` (desde un cron job, por ejemplo).

Guardar y recuperar se queda en tu máquina. La extracción automática pasa texto a la herramienta de línea de comandos de LLM que ya usas (Claude Code, Codex o Gemini CLI) cuando hay una instalada; define `provider = "none"` en `[extraction.summarizer]` para que sea totalmente local.

Para verlo funcionar ahora mismo, guarda y recupera a mano:

```console
$ icm store -t decisions-myapp -c "Auth uses short-lived JWTs, refreshed through /auth/refresh" -i high
Stored: 01M47YY6CHVZ48BYKQRCRNZTCF

$ icm recall "how does auth work"
memories[1]{id,topic,importance,weight,summary}:
  01M47YY6CHVZ48BYKQRCRNZTCF,decisions-myapp,high,0.975,"Auth uses short-lived JWTs, refreshed through /auth/refresh"
```

La primera vez que guardas o recuperas con búsqueda semántica se descarga una sola vez el modelo de embeddings multilingüe (`Qdrant/multilingual-e5-large-onnx`, unos 2 GB). Para probar ICM sin él, añade `--no-embeddings` (recuperación por palabras clave, como en la salida de arriba) o elige un modelo más ligero en la configuración. `icm init` escribe hooks e instrucciones en la configuración de cada agente detectado; `icm uninstall --dry-run` muestra cómo quitarlos. Windows, Linux, Nix y compilación desde el código fuente: [Instalación](#install).

<a id="benchmark-comparison"></a>

## Comparación de benchmarks

Precisión de las respuestas en [LoCoMo](https://github.com/snap-research/locomo) (10 conversaciones largas, 1,540 preguntas), medida con el arnés de evaluación público [Agent Memory Benchmark](https://github.com/vectorize-io/agent-memory-benchmark): el sistema de memoria recupera el contexto, `gemini-3.1-pro-preview` responde a partir de él y `gemini-2.5-flash-lite` juzga la respuesta.

| Sistema | Precisión en LoCoMo | Contexto por pregunta | Llamadas a un LLM para guardar una memoria | Se ejecuta como | Resultado |
|--------|:---------------:|:--------------------:|:---------------------------:|---------|--------|
| **ICM** 0.10.65 (motor de recall v2) | **92.8%** (1,429 / 1,540) | 24.1k tokens | ninguna | un binario Rust, un archivo SQLite | nuestra ejecución, 2026-10-05 |
| Hindsight | 92.0% (1,417 / 1,540) | 36.2k tokens | extracción de hechos con LLM | servicio Python, PostgreSQL + pgvector | publicado por el arnés |
| Línea base de búsqueda híbrida (dense + sparse, RRF) | 79.1% (1,218 / 1,540) | 22.2k tokens | ninguna | Qdrant | publicado por el arnés |

En [PersonaMem](https://arxiv.org/abs/2504.14225) 32k (589 preguntas de opción múltiple sobre las preferencias cambiantes de un usuario, mismo arnés y mismo modelo de respuesta, puntuado por coincidencia de la letra):

| Sistema | Precisión en PersonaMem | Contexto por pregunta | Resultado |
|--------|:-------------------:|:--------------------:|--------|
| **ICM** 0.10.65 (motor de recall v2) | **82.9%** (488 / 589) | 16.2k tokens | nuestra ejecución, 2026-10-05 |
| Hindsight | 86.6% (510 / 589) | 15.8k tokens | publicado por el arnés |
| Línea base de búsqueda híbrida | 84.4% (497 / 589) | 24.2k tokens | publicado por el arnés |

Solo la recuperación, en LoCoMo, sin modelo de respuesta: la proporción de preguntas para las que al menos una sesión de referencia (gold) está entre los primeros resultados. Esta es la evidencia sobre el motor de recall en sí. Las ejecuciones v2 recibieron la fecha de cada sesión y la fecha de la pregunta; el motor anterior no tiene entrada de fecha, así que sus ejecuciones no recibieron ninguna.

| Primeros resultados | Motor anterior (`legacy`) | Motor de recall v2 | Motor anterior, sin modelo de embeddings | Motor de recall v2, sin modelo de embeddings |
|:-----------:|:--------------:|:----------------:|:--------------:|:----------------:|
| 5 | 76.5% | **86.7%** | 12.0% | **88.6%** |
| 10 | 83.0% | **93.3%** | 17.4% | **94.2%** |
| 20 | 87.7% | **97.9%** | 29.8% | **97.5%** |

Lo que muestran y lo que no muestran estas cifras:

- **ICM y Hindsight están empatados en LoCoMo.** La diferencia de 0.8 puntos son 12 preguntas, dentro del error de muestreo (intervalo del 95% para ICM: 91.5 a 94.1). ICM llega ahí con un tercio menos de contexto y sin llamar a un LLM cuando se guarda una memoria.
- **En PersonaMem, Hindsight va por delante** por 3.7 puntos; ICM está al nivel de la línea base de búsqueda híbrida (intervalo del 95% para ICM: 79.8 a 85.9) leyendo un tercio menos de contexto que ella. La categoría más débil de ICM allí es sugerir ideas nuevas a partir de preferencias conocidas (59.1%).
- **Las condiciones no son idénticas.** El arnés lo mantiene Vectorize, el proveedor de Hindsight. Los resultados publicados de LoCoMo son anteriores a un cambio que fijó en 0 la temperatura de la respuesta y del juez; nuestra ejecución usa el arnés actual (commit `f618ed7`) y Vertex AI.
- **Con 50 chunks se devuelve gran parte de cada conversación,** así que esta precisión también mide el modelo de respuesta. La tabla de recuperación es la evidencia sobre el motor de recall en sí.
- **Por ahora, una sola ejecución por conjunto de datos.** Los intervalos del 95% de arriba cubren el muestreo de las preguntas, no la variación de una ejecución a otra.

<details>
<summary>Resultados por categoría, latencia y otras salvedades</summary>

- **Por tipo de pregunta** (LoCoMo, etiquetas del arnés): open-domain 96.7% (813 / 841), temporal 91.0% (292 / 321), single-hop 89.0% (251 / 282), multi-hop 76.0% (73 / 96).
- **Latencia.** Latencia mediana de recuperación de 141 ms en una VM en la nube de 4 vCPU durante la ejecución anterior; 272 sesiones ingeridas sin ninguna llamada a un LLM. En las ejecuciones de solo recuperación en Linux x86-64: mediana de 136 ms con embeddings y 6.8 ms solo con palabras clave.
- **Fechas de las sesiones.** El adaptador del benchmark escribe la fecha de cada sesión en el texto de la memoria de ICM; Hindsight recibe las mismas fechas como metadatos, y todos los sistemas reciben la fecha de la pregunta.
- **La categoría más débil son las 96 preguntas que el arnés etiqueta como multi-hop** (76.0%). Los nombres de las categorías no coinciden entre benchmarks: otras evaluaciones de LoCoMo llaman open-domain a esta categoría, y llaman multi-hop a las 282 preguntas que el arnés etiqueta como single-hop (aquí, 89.0%). Compara por número de preguntas, no por etiqueta.
- **El motor de recall v2 es el predeterminado** para `icm recall`, la herramienta MCP `icm_memory_recall`, `/recall` por HTTP y el hook de prompt. El motor anterior sigue disponible para volver atrás o para comparar: `icm recall --engine legacy`, `"engine": "legacy"` en `/recall` por HTTP, o `ICM_RECALL_ENGINE=legacy`.

</details>

Los resultados por pregunta de ambos conjuntos de datos están en [`bench/amb/results/`](bench/amb/results/); el adaptador, los ajustes exactos y los comandos para reproducirlo están en [`bench/amb/README.md`](bench/amb/README.md).

<a id="one-memory-for-every-tool"></a>

## Una sola memoria para todas las herramientas

Todas las herramientas configuradas por `icm init` leen y escriben en la misma base de datos SQLite, y los topics (`decisions-myapp`, `preferences`, `errors-resolved`, ...) no están separados por herramienta. Una memoria guardada desde Claude Code es visible de inmediato para Codex, Gemini, Cursor, Roo, Amp, Aider, ...

¿Prefieres aislamiento? `icm init --per-project` crea una base de datos local al proyecto en `.icm/` (y escribe los archivos de instrucciones de los agentes, como `CLAUDE.md` y `AGENTS.md`, en el directorio actual); `--db <path>` o `ICM_DB` apuntan a cualquier otro archivo. Cada ruta es un corpus independiente.

> **Estado del proyecto: beta.** ICM es pre-1.0: puede haber cambios incompatibles en cualquier versión menor, y los formatos de configuración de los hooks y del MCP pueden cambiar. Beta se refiere a la estabilidad de la API, no a su utilidad en el día a día: yo (el mantenedor) uso ICM todos los días como mi memoria principal para programar con IA. Mi foco principal es [rtk](https://github.com/rtk-ai/rtk), así que las issues y los pull requests se revisan según mi disponibilidad.
>
> Apache-2.0, distribuido **tal cual, sin garantía de ningún tipo** (ver [LICENSE](LICENSE)). Antes de cualquier operación destructiva, ejecuta primero su equivalente de solo lectura (`icm uninstall --dry-run`, `icm uninstall --check`).

<a id="install"></a>

## Instalación

```bash
# macOS / Linux, Homebrew
brew tap rtk-ai/tap && brew install icm

# macOS / Linux, script (verifies SHA256 against the release checksums)
curl -fsSL https://raw.githubusercontent.com/rtk-ai/icm/main/install.sh | sh

# Windows, PowerShell
irm https://raw.githubusercontent.com/rtk-ai/icm/main/install.ps1 | iex
```

La búsqueda por palabras clave funciona en todas partes. Ejecuta `icm embeddings status` para ver si la búsqueda semántica está activada: viene integrada en las builds de macOS Apple Silicon, Windows y `.rpm`; los archivos de Linux glibc y el `.deb` necesitan un `icm embeddings download`; la build para Mac Intel necesita tu propio ONNX Runtime (`ORT_DYLIB_PATH`); la build estática de Linux musl es solo por palabras clave. Nix, compilación desde el código fuente, fijación de versión y los detalles: [referencia](docs/reference.md#install).

<a id="setup"></a>

## Configuración

```bash
icm init                  # global database, every detected agent
icm init --per-project    # database under .icm/ at the git root
icm init --mode all       # also register the MCP server in every tool that supports it
```

El modo predeterminado (`standard`) escribe instrucciones, skills y hooks, sin servidor MCP. `--mode all` añade el servidor MCP; con él (más `--per-project` para Aider, cuyo archivo de convenciones es por proyecto), eso cubre las 18 herramientas siguientes ([guía de integración](docs/integrations.md)):

| Herramienta | Servidor MCP | Hooks |
|------|:---:|:-----:|
| Claude Code | sí | sí |
| Claude Desktop | sí | — |
| Gemini CLI | sí | sí |
| Codex CLI | sí | sí |
| Copilot CLI | sí | sí |
| Cursor | sí | — |
| Windsurf | sí | — |
| VS Code | sí | — |
| Amp | sí | — |
| Amazon Q | sí | — |
| Cline | sí | — |
| Roo Code | sí | — |
| Kilo Code | sí | — |
| Zed | sí | — |
| OpenCode | sí | sí |
| Continue.dev | sí | — |
| Aider | — | — |
| Pi | — | — |

O registra el servidor MCP a mano: `claude mcp add icm -- icm serve` (cualquier cliente MCP: comando `icm`, argumentos `["serve"]`).

Lo que hacen los hooks:

| Hook | Qué hace |
|------|-------------|
| `icm hook start` | Inyecta al inicio de la sesión un paquete de arranque con memorias critical/high (~500 tokens) |
| `icm hook pre` | Permite automáticamente los comandos CLI de `icm` (sin solicitud de permiso) |
| `icm hook post` | Extrae hechos de la salida de las herramientas cada N llamadas (extracción automática) |
| `icm hook compact` | Extrae memorias de la transcripción antes de la compresión del contexto |
| `icm hook prompt` | Inyecta el contexto recuperado al inicio de cada prompt del usuario |

Tablas de hooks por herramienta, skills, archivos de instrucciones y la nota sobre Codex: [referencia](docs/reference.md#setup).

<a id="use"></a>

## Uso

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

ICM también guarda **memoirs** (grafos de conocimiento permanentes de conceptos y relaciones tipadas), **feedback** (correcciones de las que aprender) y **transcripciones literales**, y expone **31 herramientas MCP** (30 sin modelo de embeddings), una **API HTTP** que mantiene cargado el modelo de embeddings y un **panel en la terminal** (`icm dashboard`). Todo está en la [referencia](docs/reference.md).

<a id="how-it-works"></a>

## Cómo funciona

La recuperación fusiona hasta tres listas ordenadas por rango recíproco (RRF): coincidencia de palabras clave con **FTS5 BM25**, siempre activa; **búsqueda vectorial semántica** mediante sqlite-vec cuando hay un modelo de embeddings cargado (por defecto `Qdrant/multilingual-e5-large-onnx`, 1024 dimensiones, 100+ idiomas); y una **ventana de fechas** cuando la consulta nombra un periodo ("last week", "in March 2024"). Los filtros de proyecto, topic y palabra clave se aplican antes del corte. Las memorias se desvanecen con el tiempo según su importancia (`critical` nunca se desvanece); con un modelo de embeddings cargado, una memoria nueva casi idéntica a otra del mismo topic (similitud coseno superior a 0.95) se fusiona con ella; y el modelo que produjo los vectores guardados queda registrado en la base de datos, así que cambiar `model` en la configuración nunca los borra (`icm embed --migrate` es la forma explícita de cambiar de modelo).

Todo vive en un único archivo SQLite, sin ningún servicio externo:

```
~/Library/Application Support/dev.icm.icm/memories.db     # macOS
~/.local/share/icm/memories.db                            # Linux
%APPDATA%\icm\icm\data\memories.db                        # Windows
<project-root>/.icm/memories.db                           # icm init --per-project
```

`icm config` muestra la configuración activa; [config/default.toml](config/default.toml) enumera todas las opciones. Detalles: [referencia](docs/reference.md#how-it-works).

<a id="documentation"></a>

## Documentación

| Documento | Descripción |
|----------|-------------|
| [Guía de integración](docs/integrations.md) | Configuración MCP por herramienta: Claude Code, Cursor, Windsurf, Zed, Amp, Codex, Cline, Roo Code, etc. |
| [Arquitectura técnica](docs/architecture.md) | Estructura de crates, pipeline de búsqueda, modelo de decaimiento, integración de sqlite-vec, pruebas |
| [Guía de usuario](docs/guide.md) | Instalación, organización de topics, consolidación, extracción, resolución de problemas |
| [Visión general del producto](docs/product.md) | Casos de uso, benchmarks, comparación con alternativas |
| [Referencia](docs/reference.md) | Opciones de instalación, configuración por herramienta, CLI, 31 herramientas MCP, API HTTP, panel, funcionamiento interno |
| [Adaptador del benchmark](bench/amb/README.md) | Cómo se hizo la comparación anterior y cómo reproducirla |
| [Demostraciones](docs/demonstrations.md) | Micro-benchmarks de almacenamiento y pequeñas demos |

<a id="license"></a>

## Licencia

[Apache-2.0](LICENSE)
