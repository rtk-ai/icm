[English](README.md) | [Français](README_fr.md) | [Español](README_es.md) | [Deutsch](README_de.md) | [Italiano](README_it.md) | [Português](README_pt.md) | [Nederlands](README_nl.md) | [Polski](README_pl.md) | [Русский](README_ru.md) | [日本語](README_ja.md) | **中文** | [العربية](README_ar.md) | [한국어](README_ko.md)

<p align="center">
  <img src="assets/banner.png" alt="ICM — Infinite Context Memory" width="600">
</p>

<h1 align="center">ICM</h1>

<p align="center">
  AI 智能体的永久记忆。单一二进制文件，零依赖，原生 MCP 支持。
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/icm/actions/workflows/ci.yml"><img src="https://github.com/rtk-ai/icm/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/icm/releases/latest"><img src="https://img.shields.io/github/v/release/rtk-ai/icm?color=purple" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-Source--Available-orange.svg" alt="Source-Available"></a>
</p>

---

ICM 为您的 AI 智能体提供真正的记忆——不是笔记工具，不是上下文管理器，而是真正的**记忆**。

```
                       ICM (Infinite Context Memory)
            ┌──────────────────────┬─────────────────────────┐
            │   MEMORIES（主题）    │   MEMOIRS（知识）        │
            │                      │                         │
            │  情节性，具有时间性   │  永久性，结构化          │
            │                      │                         │
            │  ┌───┐ ┌───┐ ┌───┐  │    ┌───┐               │
            │  │ m │ │ m │ │ m │  │    │ C │──depends_on──┐ │
            │  └─┬─┘ └─┬─┘ └─┬─┘  │    └───┘              │ │
            │    │decay │     │    │      │ refines      ┌─▼─┐│
            │    ▼      ▼     ▼    │    ┌─▼─┐            │ C ││
            │  权重随时间衰减       │    │ C │──part_of──>└───┘│
            │  除非被访问或        │    └───┘                 │
            │  标记为关键          │  概念 + 关系              │
            ├──────────────────────┴─────────────────────────┤
            │             SQLite + FTS5 + sqlite-vec          │
            │        混合搜索：BM25（30%）+ 余弦（70%）        │
            └─────────────────────────────────────────────────┘
```

**两种记忆模型：**

- **Memories（记忆）** — 按重要性进行时间衰减的存储与召回。关键记忆永不消逝，低重要性记忆自然衰减。可按主题或关键词过滤。
- **Memoirs（回忆录）** — 永久知识图谱。概念通过有类型的关系相互连接（`depends_on`、`contradicts`、`superseded_by` 等）。可按标签过滤。
- **Feedback（反馈）** — 记录 AI 预测错误时的纠正。在做出新预测前搜索过去的错误。闭环学习。

## 安装

```bash
# Homebrew（macOS / Linux）
brew tap rtk-ai/tap && brew install icm

# 快速安装
curl -fsSL https://raw.githubusercontent.com/rtk-ai/icm/main/install.sh | sh

# 从源码编译
cargo install --path crates/icm-cli
```

## 配置

```bash
# 自动检测并配置所有支持的工具
icm init
```

一条命令配置 **14 个工具**：

| 工具 | 配置文件 | 格式 |
|------|------------|--------|
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

或手动配置：

```bash
# Claude Code
claude mcp add icm -- icm serve

# 紧凑模式（更短的响应，节省 token）
claude mcp add icm -- icm serve --compact

# 任意 MCP 客户端：command = "icm"，args = ["serve"]
```

### 技能 / 规则

```bash
icm init --mode skill
```

为 Claude Code 安装斜杠命令和规则（`/recall`、`/remember`），为 Cursor 安装（`.mdc` 规则），为 Roo Code 安装（`.md` 规则），为 Amp 安装（`/icm-recall`、`/icm-remember`）。

### 钩子（Claude Code）

```bash
icm init --mode hook
```

将所有 3 个提取层作为 Claude Code 钩子安装：

**Claude Code** 钩子：

| 钩子 | 事件 | 功能 |
|------|-------|-------------|
| `icm hook pre` | PreToolUse | 自动允许 `icm` CLI 命令（无需权限提示） |
| `icm hook post` | PostToolUse | 每 15 次调用从工具输出中提取事实 |
| `icm hook compact` | PreCompact | 在上下文压缩前从对话记录中提取记忆 |
| `icm hook prompt` | UserPromptSubmit | 在每次提示开始时注入召回的上下文 |

**OpenCode** 插件（自动安装至 `~/.config/opencode/plugins/icm.js`）：

| OpenCode 事件 | ICM 层 | 功能 |
|---------------|-----------|-------------|
| `tool.execute.after` | 第 0 层 | 从工具输出中提取事实 |
| `experimental.session.compacting` | 第 1 层 | 压缩前从对话中提取 |
| `session.created` | 第 2 层 | 会话开始时召回上下文 |

## CLI 与 MCP 对比

ICM 可通过 CLI（`icm` 命令）或 MCP 服务器（`icm serve`）使用。两者访问同一数据库。

| | CLI | MCP |
|---|-----|-----|
| **延迟** | ~30ms（直接二进制） | ~50ms（JSON-RPC stdio） |
| **Token 开销** | 0（基于钩子，透明） | ~20-50 token/调用（工具 schema） |
| **配置** | `icm init --mode hook` | `icm init --mode mcp` |
| **适用工具** | Claude Code、OpenCode（通过钩子/插件） | 全部 14 个兼容 MCP 的工具 |
| **自动提取** | 是（钩子触发 `icm extract`） | 是（MCP 工具调用 store） |
| **最适合** | 高级用户，节省 token | 通用兼容性 |

## CLI

### Memories（情节性，带衰减）

```bash
# 存储
icm store -t "my-project" -c "Use PostgreSQL for the main DB" -i high -k "db,postgres"

# 召回
icm recall "database choice"
icm recall "auth setup" --topic "my-project" --limit 10
icm recall "architecture" --keyword "postgres"

# 管理
icm forget <memory-id>
icm consolidate --topic "my-project"
icm topics
icm stats

# 从文本中提取事实（基于规则，零 LLM 开销）
echo "The parser uses Pratt algorithm" | icm extract -p my-project
```

### Memoirs（永久知识图谱）

```bash
# 创建回忆录
icm memoir create -n "system-architecture" -d "System design decisions"

# 添加带标签的概念
icm memoir add-concept -m "system-architecture" -n "auth-service" \
  -d "Handles JWT tokens and OAuth2 flows" -l "domain:auth,type:service"

# 连接概念
icm memoir link -m "system-architecture" --from "api-gateway" --to "auth-service" -r depends-on

# 按标签过滤搜索
icm memoir search -m "system-architecture" "authentication"
icm memoir search -m "system-architecture" "service" --label "domain:auth"

# 检视邻域
icm memoir inspect -m "system-architecture" "auth-service" -D 2

# 导出图谱（格式：json、dot、ascii、ai）
icm memoir export -m "system-architecture" -f ascii   # 带置信度条的框图
icm memoir export -m "system-architecture" -f dot      # Graphviz DOT（颜色 = 置信度）
icm memoir export -m "system-architecture" -f ai       # 为 LLM 上下文优化的 Markdown
icm memoir export -m "system-architecture" -f json     # 包含所有元数据的结构化 JSON

# 生成 SVG 可视化
icm memoir export -m "system-architecture" -f dot | dot -Tsvg > graph.svg
```

## MCP 工具（22 个）

### 记忆工具

| 工具 | 描述 |
|------|-------------|
| `icm_memory_store` | 存储，自动去重（相似度 >85% → 更新而非重复创建） |
| `icm_memory_recall` | 按查询搜索，可按主题和/或关键词过滤 |
| `icm_memory_update` | 就地编辑记忆（内容、重要性、关键词） |
| `icm_memory_forget` | 按 ID 删除记忆 |
| `icm_memory_consolidate` | 将某主题的所有记忆合并为一个摘要 |
| `icm_memory_list_topics` | 列出所有主题及其数量 |
| `icm_memory_stats` | 全局记忆统计 |
| `icm_memory_health` | 按主题的健康审计（陈旧度、是否需要整合） |
| `icm_memory_embed_all` | 为向量搜索补全嵌入向量 |

### 回忆录工具（知识图谱）

| 工具 | 描述 |
|------|-------------|
| `icm_memoir_create` | 创建新的回忆录（知识容器） |
| `icm_memoir_list` | 列出所有回忆录 |
| `icm_memoir_show` | 显示回忆录详情及所有概念 |
| `icm_memoir_add_concept` | 添加带标签的概念 |
| `icm_memoir_refine` | 更新概念的定义 |
| `icm_memoir_search` | 全文搜索，可选按标签过滤 |
| `icm_memoir_search_all` | 跨所有回忆录搜索 |
| `icm_memoir_link` | 在概念之间创建有类型的关系 |
| `icm_memoir_inspect` | 检视概念及图谱邻域（BFS） |
| `icm_memoir_export` | 导出图谱（json、dot、ascii、ai），含置信度级别 |

### 反馈工具（从错误中学习）

| 工具 | 描述 |
|------|-------------|
| `icm_feedback_record` | 当 AI 预测错误时记录纠正 |
| `icm_feedback_search` | 搜索过去的纠正以指导未来的预测 |
| `icm_feedback_stats` | 反馈统计：总数、按主题细分、应用最多的条目 |

### 关系类型

`part_of` · `depends_on` · `related_to` · `contradicts` · `refines` · `alternative_to` · `caused_by` · `instance_of` · `superseded_by`

## 工作原理

### 双重记忆模型

**情节记忆（主题）** 捕获决策、错误、偏好。每条记忆都有一个随时间按重要性衰减的权重：

| 重要性 | 衰减 | 修剪 | 行为 |
|-----------|-------|-------|----------|
| `critical` | 无 | 永不 | 永不遗忘，永不修剪 |
| `high` | 慢（0.5x 速率） | 永不 | 缓慢消退，永不自动删除 |
| `medium` | 正常 | 是 | 标准衰减，权重低于阈值时修剪 |
| `low` | 快（2x 速率） | 是 | 迅速遗忘 |

衰减是**访问感知**的：频繁召回的记忆衰减更慢（`decay / (1 + access_count × 0.1)`）。在召回时自动应用（如果距上次衰减超过 24 小时）。

**记忆健康**内置功能：
- **自动去重**：存储与同主题现有记忆相似度 >85% 的内容时，更新而非创建重复项
- **整合提示**：当主题超过 7 条记录时，`icm_memory_store` 会提示调用者进行整合
- **健康审计**：`icm_memory_health` 报告每个主题的条目数、平均权重、陈旧条目及整合需求
- **无静默数据丢失**：关键和高重要性记忆永不自动修剪

**语义记忆（回忆录）** 将结构化知识捕获为图谱。概念是永久性的——它们被精炼，永不衰减。使用 `superseded_by` 标记过时事实，而不是删除它们。

### 混合搜索

启用嵌入时，ICM 使用混合搜索：
- **FTS5 BM25**（30%）— 全文关键词匹配
- **余弦相似度**（70%）— 通过 sqlite-vec 进行语义向量搜索

默认模型：`intfloat/multilingual-e5-base`（768 维，支持 100+ 种语言）。可在[配置文件](#配置)中设置：

```toml
[embeddings]
# enabled = false                          # 完全禁用（不下载模型）
model = "intfloat/multilingual-e5-base"    # 768d，多语言（默认）
# model = "intfloat/multilingual-e5-small" # 384d，多语言（更轻量）
# model = "intfloat/multilingual-e5-large" # 1024d，多语言（最高精度）
# model = "Xenova/bge-small-en-v1.5"      # 384d，仅英文（最快）
# model = "jinaai/jina-embeddings-v2-base-code"  # 768d，代码优化
```

若要完全跳过嵌入模型下载，可使用以下任一方式：
```bash
icm --no-embeddings serve          # CLI 标志
ICM_NO_EMBEDDINGS=1 icm serve     # 环境变量
```
或在配置文件中设置 `enabled = false`。ICM 将回退到 FTS5 关键词搜索（仍然有效，只是没有语义匹配）。

更换模型会自动重建向量索引（现有嵌入将被清除，可通过 `icm_memory_embed_all` 重新生成）。

### 存储

单一 SQLite 文件。无外部服务，无网络依赖。

```
~/Library/Application Support/dev.icm.icm/memories.db                    # macOS
~/.local/share/dev.icm.icm/memories.db                                   # Linux
C:\Users\<user>\AppData\Local\icm\icm\data\memories.db                   # Windows
```

### 配置

```bash
icm config                    # 显示当前配置
```

配置文件位置（平台特定，或使用 `$ICM_CONFIG`）：

```
~/Library/Application Support/dev.icm.icm/config.toml                    # macOS
~/.config/icm/config.toml                                                # Linux
C:\Users\<user>\AppData\Roaming\icm\icm\config\config.toml              # Windows
```

所有选项请参见 [config/default.toml](config/default.toml)。

## 自动提取

ICM 通过三个层次自动提取记忆：

```
  第 0 层：模式钩子              第 1 层：PreCompact           第 2 层：UserPromptSubmit
  （零 LLM 开销）                 （零 LLM 开销）               （零 LLM 开销）
  ┌──────────────────┐                ┌──────────────────┐          ┌──────────────────┐
  │ PostToolUse 钩子  │                │ PreCompact 钩子   │          │ UserPromptSubmit  │
  │                   │                │                   │          │                   │
  │ • Bash 错误       │                │ 上下文即将被压缩→  │          │ 用户发送提示      │
  │ • git 提交        │                │ 在压缩前从对话    │          │ → icm recall      │
  │ • 配置变更        │                │ 记录中提取记忆    │          │ → 注入上下文      │
  │ • 决策            │                │ 以免永久丢失      │          │                   │
  │ • 偏好            │                │                   │          │ 智能体启动时      │
  │ • 学习            │                │ 相同模式 +        │          │ 已加载相关记忆    │
  │ • 约束            │                │ --store-raw 回退  │          │                   │
  │                   │                │                   │          │                   │
  │ 基于规则，无 LLM  │                └──────────────────┘          └──────────────────┘
  └──────────────────┘
```

| 层次 | 状态 | LLM 开销 | 钩子命令 | 描述 |
|-------|--------|----------|-------------|-------------|
| 第 0 层 | 已实现 | 0 | `icm hook post` | 从工具输出中进行基于规则的关键词提取 |
| 第 1 层 | 已实现 | 0 | `icm hook compact` | 在上下文压缩前从对话记录中提取 |
| 第 2 层 | 已实现 | 0 | `icm hook prompt` | 在每次用户提示时注入召回的记忆 |

所有 3 个层次均通过 `icm init --mode hook` 自动安装。

### 与其他方案的对比

| 系统 | 方法 | LLM 开销 | 延迟 | 是否捕获压缩？ |
|--------|--------|----------|---------|---------------------|
| **ICM** | 3 层提取 | 0 至 ~500 token/会话 | 0ms | **是（PreCompact）** |
| Mem0 | 每条消息 2 次 LLM 调用 | ~2k token/消息 | 200-2000ms | 否 |
| claude-mem | PostToolUse + 异步 | ~1-5k token/会话 | 8ms 钩子 | 否 |
| MemGPT/Letta | 智能体自管理 | 0 边际成本 | 0ms | 否 |
| DiffMem | 基于 Git 差异 | 0 | 0ms | 否 |

## 基准测试

### 存储性能

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

Apple M1 Pro，内存 SQLite，单线程。`icm bench --count 1000`

### 智能体效率

使用真实 Rust 项目（12 个文件，约 550 行）的多会话工作流。第 2 次及之后的会话收益最大，因为 ICM 召回记忆而无需重新读取文件。

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

### 知识留存

智能体跨会话从密集技术文档中召回特定事实。第 1 次会话读取并记忆；第 2 次及之后的会话**不依赖**源文本回答 10 个事实性问题。

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

### 本地 LLM（ollama）

使用本地模型进行相同测试——纯上下文注入，无需工具调用。

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

### 测试协议

所有基准测试均使用**真实 API 调用**——无模拟，无模拟响应，无缓存答案。

- **智能体基准**：在临时目录中创建真实 Rust 项目。使用 `claude -p --output-format json` 运行 N 个会话。不使用 ICM：空 MCP 配置。使用 ICM：真实 MCP 服务器 + 自动提取 + 上下文注入。
- **知识留存**：使用虚构技术文档（"Meridian Protocol"）。通过关键词匹配预期事实对答案评分。每次调用超时 120 秒。
- **隔离**：每次运行使用独立的临时目录和全新的 SQLite 数据库。无会话持久化。

## 文档

| 文档 | 描述 |
|----------|-------------|
| [技术架构](docs/architecture.md) | Crate 结构、搜索流水线、衰减模型、sqlite-vec 集成、测试 |
| [用户指南](docs/guide.md) | 安装、主题组织、整合、提取、故障排除 |
| [产品概述](docs/product.md) | 使用场景、基准测试、与其他方案的对比 |

## 许可证

[Source-Available](LICENSE) — 个人及 20 人以下团队免费使用。20 人以上的组织需要企业许可证。联系方式：contact@rtk-ai.app
