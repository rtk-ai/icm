[English](README.md) | [Français](README_fr.md) | [Español](README_es.md) | [Deutsch](README_de.md) | [Italiano](README_it.md) | [Português](README_pt.md) | [Nederlands](README_nl.md) | [Polski](README_pl.md) | **Русский** | [日本語](README_ja.md) | [中文](README_zh.md) | [العربية](README_ar.md) | [한국어](README_ko.md)

<p align="center">
  <img src="assets/banner.png" alt="ICM — Infinite Context Memory" width="600">
</p>

<h1 align="center">ICM</h1>

<p align="center">
  Постоянная память для ИИ-агентов. Один бинарный файл, без зависимостей, нативная поддержка MCP.
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/icm/actions/workflows/ci.yml"><img src="https://github.com/rtk-ai/icm/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/icm/releases/latest"><img src="https://img.shields.io/github/v/release/rtk-ai/icm?color=purple" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-Source--Available-orange.svg" alt="Source-Available"></a>
</p>

---

ICM даёт вашему ИИ-агенту настоящую память — не инструмент для заметок, не менеджер контекста, а **память**.

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

**Две модели памяти:**

- **Memories** — хранение и извлечение с временным затуханием по важности. Критические воспоминания не исчезают никогда, маловажные затухают естественным образом. Фильтрация по теме или ключевому слову.
- **Memoirs** — постоянные графы знаний. Концепции связаны типизированными отношениями (`depends_on`, `contradicts`, `superseded_by`, ...). Фильтрация по метке.
- **Feedback** — запись исправлений при ошибочных предсказаниях ИИ. Поиск прошлых ошибок перед новыми предсказаниями. Обучение с замкнутым циклом.

## Установка

```bash
# Homebrew (macOS / Linux)
brew tap rtk-ai/tap && brew install icm

# Быстрая установка
curl -fsSL https://raw.githubusercontent.com/rtk-ai/icm/main/install.sh | sh

# Из исходного кода
cargo install --path crates/icm-cli
```

## Настройка

```bash
# Автоматическое обнаружение и настройка всех поддерживаемых инструментов
icm init
```

Настраивает **17 инструментов** одной командой ([полное руководство по интеграции](docs/integrations.md)):

| Инструмент | MCP | Хуки | CLI | Навыки |
|------------|:---:|:----:|:---:|:------:|
| Claude Code | `~/.claude.json` | 5 хуков | `CLAUDE.md` | `/recall` `/remember` |
| Claude Desktop | JSON | — | — | — |
| Gemini CLI | `~/.gemini/settings.json` | 5 хуков | `GEMINI.md` | — |
| Codex CLI | `~/.codex/config.toml` | 4 хука | `AGENTS.md` | — |
| Copilot CLI | `~/.copilot/mcp-config.json` | 4 хука | `.github/copilot-instructions.md` | — |
| Cursor | `~/.cursor/mcp.json` | — | — | правило `.mdc` |
| Windsurf | JSON | — | `.windsurfrules` | — |
| VS Code | `~/Library/.../Code/User/mcp.json` | — | — | — |
| Amp | JSON | — | — | `/icm-recall` `/icm-remember` |
| Amazon Q | JSON | — | — | — |
| Cline | VS Code globalStorage | — | — | — |
| Roo Code | VS Code globalStorage | — | — | правило `.md` |
| Kilo Code | VS Code globalStorage | — | — | — |
| Zed | `~/.zed/settings.json` | — | — | — |
| OpenCode | JSON | TS-плагин | — | — |
| Continue.dev | `~/.continue/config.yaml` | — | — | — |
| Aider | — | — | `.aider.conventions.md` | — |

Или вручную:

```bash
# Claude Code
claude mcp add icm -- icm serve

# Компактный режим (более короткие ответы, экономия токенов)
claude mcp add icm -- icm serve --compact

# Любой MCP-клиент: command = "icm", args = ["serve"]
```

### Навыки / правила

```bash
icm init --mode skill
```

Устанавливает слэш-команды и правила для Claude Code (`/recall`, `/remember`), Cursor (правило `.mdc`), Roo Code (правило `.md`) и Amp (`/icm-recall`, `/icm-remember`).

### Хуки (5 инструментов)

```bash
icm init --mode hook
```

Устанавливает хуки автоматического извлечения и автоматического вспоминания для всех поддерживаемых инструментов:

| Инструмент | SessionStart | PreTool | PostTool | Compact | PromptRecall | Конфигурация |
|------------|:-----------:|:-------:|:--------:|:-------:|:------------:|--------|
| Claude Code | `icm hook start` | `icm hook pre` | `icm hook post` | `icm hook compact` | `icm hook prompt` | `~/.claude/settings.json` |
| Gemini CLI | `icm hook start` | `icm hook pre` | `icm hook post` | `icm hook compact` | `icm hook prompt` | `~/.gemini/settings.json` |
| Codex CLI | `icm hook start` | `icm hook pre` | `icm hook post` | — | `icm hook prompt` | `~/.codex/hooks.json` |
| Copilot CLI | `icm hook start` | `icm hook pre` | `icm hook post` | — | `icm hook prompt` | `.github/hooks/icm.json` |
| OpenCode | старт сессии | — | извлечение из инструментов | сжатие | — | `~/.config/opencode/plugins/icm.ts` |

**Что делает каждый хук:**

| Хук | Что делает |
|-----|------------|
| `icm hook start` | Внедряет пакет критических/важных воспоминаний при старте сессии (~500 токенов) |
| `icm hook pre` | Автоматическое разрешение команд `icm` CLI (без запроса подтверждения) |
| `icm hook post` | Извлечение фактов из вывода инструмента каждые N вызовов (автоматическое извлечение) |
| `icm hook compact` | Извлечение воспоминаний из транскрипта перед сжатием контекста |
| `icm hook prompt` | Внедрение извлечённого контекста в начало каждого запроса пользователя |

## CLI vs MCP

ICM можно использовать через CLI (команды `icm`) или MCP-сервер (`icm serve`). Оба варианта работают с одной и той же базой данных.

| | CLI | MCP |
|---|-----|-----|
| **Задержка** | ~30ms (прямой бинарный файл) | ~50ms (JSON-RPC stdio) |
| **Стоимость токенов** | 0 (на основе хуков, невидимо) | ~20-50 токенов/вызов (схема инструмента) |
| **Настройка** | `icm init --mode hook` | `icm init --mode mcp` |
| **Работает с** | Claude Code, Gemini, Codex, Copilot, OpenCode (через хуки) | Все 17 MCP-совместимых инструментов |
| **Авто-извлечение** | Да (хуки запускают `icm extract`) | Да (MCP-инструменты вызывают store) |
| **Лучше для** | Опытных пользователей, экономия токенов | Универсальная совместимость |

## CLI

### Memories (эпизодические, с затуханием)

```bash
# Сохранение
icm store -t "my-project" -c "Use PostgreSQL for the main DB" -i high -k "db,postgres"

# Поиск
icm recall "database choice"
icm recall "auth setup" --topic "my-project" --limit 10
icm recall "architecture" --keyword "postgres"

# Управление
icm forget <memory-id>
icm consolidate --topic "my-project"
icm topics
icm stats

# Извлечение фактов из текста (на основе правил, без затрат на LLM)
echo "The parser uses Pratt algorithm" | icm extract -p my-project
```

### Memoirs (постоянные графы знаний)

```bash
# Создание memoir
icm memoir create -n "system-architecture" -d "System design decisions"

# Добавление концепций с метками
icm memoir add-concept -m "system-architecture" -n "auth-service" \
  -d "Handles JWT tokens and OAuth2 flows" -l "domain:auth,type:service"

# Связывание концепций
icm memoir link -m "system-architecture" --from "api-gateway" --to "auth-service" -r depends-on

# Поиск с фильтром по метке
icm memoir search -m "system-architecture" "authentication"
icm memoir search -m "system-architecture" "service" --label "domain:auth"

# Просмотр окрестности
icm memoir inspect -m "system-architecture" "auth-service" -D 2

# Экспорт графа (форматы: json, dot, ascii, ai)
icm memoir export -m "system-architecture" -f ascii   # Блочная графика с индикаторами уверенности
icm memoir export -m "system-architecture" -f dot      # Graphviz DOT (цвет = уровень уверенности)
icm memoir export -m "system-architecture" -f ai       # Markdown, оптимизированный для контекста LLM
icm memoir export -m "system-architecture" -f json     # Структурированный JSON со всеми метаданными

# Генерация SVG-визуализации
icm memoir export -m "system-architecture" -f dot | dot -Tsvg > graph.svg
```

## MCP-инструменты (22)

### Инструменты памяти

| Инструмент | Описание |
|------------|----------|
| `icm_memory_store` | Сохранение с авто-дедупликацией (сходство >85% → обновление вместо дубликата) |
| `icm_memory_recall` | Поиск по запросу, фильтрация по теме и/или ключевому слову |
| `icm_memory_update` | Редактирование воспоминания на месте (содержимое, важность, ключевые слова) |
| `icm_memory_forget` | Удаление воспоминания по ID |
| `icm_memory_consolidate` | Объединение всех воспоминаний темы в одно резюме |
| `icm_memory_list_topics` | Список всех тем с количеством записей |
| `icm_memory_stats` | Глобальная статистика памяти |
| `icm_memory_health` | Аудит гигиены по темам (устарелость, необходимость консолидации) |
| `icm_memory_embed_all` | Заполнение эмбеддингов для векторного поиска |

### Инструменты Memoir (графы знаний)

| Инструмент | Описание |
|------------|----------|
| `icm_memoir_create` | Создание нового memoir (контейнер знаний) |
| `icm_memoir_list` | Список всех memoir |
| `icm_memoir_show` | Просмотр деталей memoir и всех концепций |
| `icm_memoir_add_concept` | Добавление концепции с метками |
| `icm_memoir_refine` | Обновление определения концепции |
| `icm_memoir_search` | Полнотекстовый поиск, опционально фильтруется по метке |
| `icm_memoir_search_all` | Поиск по всем memoir |
| `icm_memoir_link` | Создание типизированного отношения между концепциями |
| `icm_memoir_inspect` | Просмотр концепции и окрестности графа (BFS) |
| `icm_memoir_export` | Экспорт графа (json, dot, ascii, ai) с уровнями уверенности |

### Инструменты обратной связи (обучение на ошибках)

| Инструмент | Описание |
|------------|----------|
| `icm_feedback_record` | Запись исправления при ошибочном предсказании ИИ |
| `icm_feedback_search` | Поиск прошлых исправлений для информирования будущих предсказаний |
| `icm_feedback_stats` | Статистика обратной связи: общее количество, разбивка по темам, наиболее применяемые |

### Типы отношений

`part_of` · `depends_on` · `related_to` · `contradicts` · `refines` · `alternative_to` · `caused_by` · `instance_of` · `superseded_by`

## Принцип работы

### Двойная модель памяти

**Эпизодическая память (Темы)** фиксирует решения, ошибки, предпочтения. Каждое воспоминание имеет вес, который затухает со временем в зависимости от важности:

| Важность | Затухание | Очистка | Поведение |
|----------|-----------|---------|-----------|
| `critical` | нет | никогда | Никогда не забывается, никогда не очищается |
| `high` | медленное (0.5x скорость) | никогда | Затухает медленно, никогда не удаляется автоматически |
| `medium` | нормальное | да | Стандартное затухание, очищается при весе ниже порога |
| `low` | быстрое (2x скорость) | да | Быстро забывается |

Затухание **учитывает обращения**: часто извлекаемые воспоминания затухают медленнее (`decay / (1 + access_count × 0.1)`). Применяется автоматически при извлечении (если прошло >24 часов с последнего затухания).

**Гигиена памяти** встроена:
- **Авто-дедупликация**: сохранение содержимого со сходством >85% с существующим воспоминанием в той же теме обновляет его вместо создания дубликата
- **Подсказки консолидации**: когда тема превышает 7 записей, `icm_memory_store` предупреждает вызывающую сторону о необходимости консолидации
- **Аудит состояния**: `icm_memory_health` сообщает количество записей по темам, средний вес, устаревшие записи и необходимость консолидации
- **Без тихой потери данных**: критические воспоминания и воспоминания высокой важности никогда не очищаются автоматически

**Семантическая память (Memoirs)** фиксирует структурированные знания в виде графа. Концепции постоянны — они уточняются, но не затухают. Используйте `superseded_by` для пометки устаревших фактов вместо их удаления.

### Гибридный поиск

При включённых эмбеддингах ICM использует гибридный поиск:
- **FTS5 BM25** (30%) — полнотекстовое сопоставление по ключевым словам
- **Косинусное сходство** (70%) — семантический векторный поиск через sqlite-vec

Модель по умолчанию: `intfloat/multilingual-e5-base` (768d, 100+ языков). Настраивается в [файле конфигурации](#конфигурация):

```toml
[embeddings]
# enabled = false                          # Полное отключение (без загрузки модели)
model = "intfloat/multilingual-e5-base"    # 768d, многоязычная (по умолчанию)
# model = "intfloat/multilingual-e5-small" # 384d, многоязычная (легче)
# model = "intfloat/multilingual-e5-large" # 1024d, многоязычная (лучшая точность)
# model = "Xenova/bge-small-en-v1.5"      # 384d, только английский (быстрее всего)
# model = "jinaai/jina-embeddings-v2-base-code"  # 768d, оптимизирована для кода
```

Чтобы полностью пропустить загрузку модели эмбеддингов, используйте любое из следующего:
```bash
icm --no-embeddings serve          # Флаг CLI
ICM_NO_EMBEDDINGS=1 icm serve     # Переменная окружения
```
Или установите `enabled = false` в файле конфигурации. ICM перейдёт к ключевому поиску FTS5 (работает, но без семантического сопоставления).

Изменение модели автоматически пересоздаёт векторный индекс (существующие эмбеддинги очищаются и могут быть перегенерированы с помощью `icm_memory_embed_all`).

### Хранилище

Единый файл SQLite. Без внешних сервисов, без сетевых зависимостей.

```
~/Library/Application Support/dev.icm.icm/memories.db                    # macOS
~/.local/share/dev.icm.icm/memories.db                                   # Linux
C:\Users\<user>\AppData\Local\icm\icm\data\memories.db                   # Windows
```

### Конфигурация

```bash
icm config                    # Показать активную конфигурацию
```

Расположение файла конфигурации (зависит от платформы или `$ICM_CONFIG`):

```
~/Library/Application Support/dev.icm.icm/config.toml                    # macOS
~/.config/icm/config.toml                                                # Linux
C:\Users\<user>\AppData\Roaming\icm\icm\config\config.toml              # Windows
```

Смотрите [config/default.toml](config/default.toml) для всех параметров.

## Авто-извлечение

ICM автоматически извлекает воспоминания через три слоя:

```
  Layer 0: Pattern hooks              Layer 1: PreCompact           Layer 2: UserPromptSubmit
  (zero LLM cost)                     (zero LLM cost)               (zero LLM cost)
  ┌──────────────────┐                ┌──────────────────┐          ┌──────────────────┐
  │ PostToolUse hook  │                │ PreCompact hook   │          │ UserPromptSubmit  │
  │                   │                │                   │          │                   │
  │ • Bash errors     │                │ Context about to  │          │ User sends prompt │
  │ • git commits     │                │ be compressed →   │          │ → icm recall      │
  │ • config changes  │                │ extract memories  │          │ → inject context  │
  │ • decisions       │                │ from transcript   │          │                   │
  │ • preferences     │                │ before they're    │          │ Agent starts with  │
  │ • learnings       │                │ lost forever      │          │ relevant memories  │
  │ • constraints     │                │                   │          │ already loaded     │
  │                   │                │ Same patterns +   │          │                   │
  │ Rule-based, no LLM│                │ --store-raw fallbk│          │                   │
  └──────────────────┘                └──────────────────┘          └──────────────────┘
```

| Слой | Статус | Стоимость LLM | Команда хука | Описание |
|------|--------|---------------|-------------|----------|
| Layer 0 | Реализован | 0 | `icm hook post` | Извлечение по правилам из вывода инструмента |
| Layer 1 | Реализован | 0 | `icm hook compact` | Извлечение из транскрипта перед сжатием контекста |
| Layer 2 | Реализован | 0 | `icm hook prompt` | Внедрение извлечённых воспоминаний на каждый запрос пользователя |

Все 3 слоя устанавливаются автоматически командой `icm init --mode hook`.

### Сравнение с альтернативами

| Система | Метод | Стоимость LLM | Задержка | Перехватывает сжатие? |
|---------|-------|---------------|---------|----------------------|
| **ICM** | 3-слойное извлечение | 0 до ~500 tok/сессия | 0ms | **Да (PreCompact)** |
| Mem0 | 2 вызова LLM/сообщение | ~2k tok/сообщение | 200-2000ms | Нет |
| claude-mem | PostToolUse + async | ~1-5k tok/сессия | 8ms хук | Нет |
| MemGPT/Letta | Агент управляет сам | 0 доп. | 0ms | Нет |
| DiffMem | Git-based diffs | 0 | 0ms | Нет |

## Бенчмарки

### Производительность хранилища

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

Apple M1 Pro, SQLite в памяти, однопоточный режим. `icm bench --count 1000`

### Эффективность агента

Многосессионный рабочий процесс с реальным Rust-проектом (12 файлов, ~550 строк). Сессии 2+ показывают наибольший выигрыш, так как ICM извлекает информацию вместо повторного чтения файлов.

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

### Сохранение знаний

Агент вспоминает конкретные факты из насыщенного технического документа между сессиями. Сессия 1 читает и запоминает; сессии 2+ отвечают на 10 фактических вопросов **без** исходного текста.

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

### Локальные LLM (ollama)

Тот же тест с локальными моделями — чистое внедрение контекста, без необходимости использования инструментов.

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

### Протокол тестирования

Все бенчмарки используют **реальные API-вызовы** — без заглушек, без симулированных ответов, без кэшированных результатов.

- **Бенчмарк агента**: Создаёт реальный Rust-проект во временной директории. Запускает N сессий с `claude -p --output-format json`. Без ICM: пустая конфигурация MCP. С ICM: реальный MCP-сервер + авто-извлечение + внедрение контекста.
- **Сохранение знаний**: Использует вымышленный технический документ («Протокол Меридиан»). Оценивает ответы по совпадению ключевых слов с ожидаемыми фактами. Таймаут 120 секунд на вызов.
- **Изоляция**: Каждый запуск использует собственную временную директорию и чистую базу данных SQLite. Без сохранения между сессиями.

### Единая память для нескольких агентов

Все 17 инструментов используют одну и ту же базу данных SQLite. Воспоминание, сохранённое Claude, мгновенно доступно Gemini, Codex, Copilot, Cursor и любому другому инструменту.

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

Оценка = 60% точность вспоминания + 30% детальность фактов + 10% скорость. **98% эффективности мультиагентной работы.**

## Почему ICM

| Возможность | ICM | Mem0 | Engram | AgentMemory |
|-------------|:---:|:----:|:------:|:-----------:|
| Поддержка инструментов | **17** | только SDK | ~6-8 | ~10 |
| Настройка одной командой | `icm init` | SDK вручную | вручную | вручную |
| Хуки (авто-вспоминание при запуске) | 5 инструментов | нет | через MCP | 1 инструмент |
| Гибридный поиск (FTS5 + вектор) | 30/70 взвешенный | только вектор | только FTS5 | FTS5+вектор |
| Многоязычные эмбеддинги | 100+ языков (768d) | зависит | нет | английский 384d |
| Граф знаний | Система Memoir | нет | нет | нет |
| Временное затухание + консолидация | с учётом обращений | нет | базовое | базовое |
| TUI-дашборд | `icm dashboard` | нет | да | веб-просмотрщик |
| Автоматическое извлечение из вывода инструмента | 3 слоя, без LLM | нет | нет | нет |
| Цикл обратной связи/коррекций | `icm_feedback_*` | нет | нет | нет |
| Среда исполнения | Rust, один бинарный файл | Python | Go | Node.js |
| Локальное, без зависимостей | файл SQLite | облачное | SQLite | SQLite |
| Точность вспоминания нескольких агентов | **98%** | Н/Д | Н/Д | 95,2% |

## Документация

| Документ | Описание |
|----------|----------|
| [Руководство по интеграции](docs/integrations.md) | Настройка для всех 17 инструментов: Claude Code, Copilot, Cursor, Windsurf, Zed, Amp и др. |
| [Техническая архитектура](docs/architecture.md) | Структура крейтов, конвейер поиска, модель затухания, интеграция sqlite-vec, тестирование |
| [Руководство пользователя](docs/guide.md) | Установка, организация тем, консолидация, извлечение, устранение неполадок |
| [Обзор продукта](docs/product.md) | Сценарии использования, бенчмарки, сравнение с альтернативами |

## Лицензия

[Source-Available](LICENSE) — Бесплатно для физических лиц и команд ≤ 20 человек. Для более крупных организаций требуется корпоративная лицензия. Контакт: contact@rtk-ai.app
