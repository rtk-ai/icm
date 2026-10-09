[English](README.md) | [Français](README_fr.md) | [Español](README_es.md) | [Deutsch](README_de.md) | [Italiano](README_it.md) | [Português](README_pt.md) | [Nederlands](README_nl.md) | [Polski](README_pl.md) | [Русский](README_ru.md) | [日本語](README_ja.md) | [中文](README_zh.md) | [العربية](README_ar.md) | [한국어](README_ko.md)

本文是 README.md（英文版）的翻译，英文版为参考版本；如两者不一致，以英文版为准。

<h1 align="center">ICM</h1>

<p align="center">
  <b>面向 AI 编程智能体的长期记忆，在你的各个工具之间共享。</b><br>
  一个二进制文件，一个 SQLite 文件。存储或召回一条记忆都不需要调用 LLM。
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/icm/actions/workflows/ci.yml"><img src="https://github.com/rtk-ai/icm/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/icm/releases/latest"><img src="https://img.shields.io/github/v/release/rtk-ai/icm?color=purple" alt="发布版本"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-Apache--2.0-blue.svg" alt="Apache-2.0"></a>
</p>

周一告诉 Claude Code 你的项目如何处理认证，周二的 Gemini CLI 会话就已经知道了。ICM 把编程智能体学到的内容（决策、修复、约定、偏好）保存在你本机的一个 SQLite 文件中，并在每次会话开始时以及你发送每条提示时返回其中相关的部分。最多 18 个智能体和编辑器共享这份记忆，因此你不必在每次打开会话或切换工具时重新解释你的项目。

- **在 LoCoMo（1,540 个问题，三次运行的平均值）上达到 92.9%，与 Hindsight（92.0%）持平**，每个问题使用的上下文少三分之一。[详情和注意事项见下文](#benchmark-comparison)。
- **存储和召回都不调用 LLM。** Hindsight、Mem0、Graphiti (Zep) 和 claude-mem 默认在存储每条记忆时都会调用 LLM。ICM 不会。只有从工具输出中自动提取事实这一步，会在已安装的情况下经过你已经在用的 LLM 命令行工具；设置 `provider = "none"` 可让这一步也留在本地（见[快速开始](#quickstart)）。
- **没有嵌入模型也能用。** 仅靠关键词召回，就能让 88.6% 的 LoCoMo 问题至少有一个正确会话进入前 5 条结果；在 Linux x86-64 上，每次召回的中位耗时为 6.8 ms。
- **在 LongMemEval-S 上达到 97.4%，检索不使用 LLM**：500 个问题中有 487 个有一个正确会话进入前 5 条结果；按同一指标，MemPalace 为 96.6%，agentmemory 为 95.2%。
- **并非处处领先。** 在 PersonaMem（用户不断变化的偏好）上，Hindsight 领先：86.6% 对 ICM 的 81.7%。

<p align="center">
  <img src="assets/demo.svg" alt="终端：用 icm store 存储三条记忆，然后由 icm recall 回答两个问题，每次都返回正确的记忆">
</p>

<a id="quickstart"></a>
## 快速开始

```bash
brew tap rtk-ai/tap && brew install icm    # or: curl -fsSL https://raw.githubusercontent.com/rtk-ai/icm/main/install.sh | sh
icm init                                   # instructions, skills and hooks for every agent it detects
```

设置到此就完成了。在 Claude Code、Codex、Gemini CLI 或 Copilot CLI 中打开一个新会话：你的智能体现在会带着一小组它所在项目中最重要的记忆（即主题中带有仓库名称的记忆，例如名为 `myapp` 的仓库中的 `decisions-myapp`，再加上你的偏好）开始工作，并在你发送每条提示时收到与之相关的记忆。它从工具输出中学到的内容会进入队列，队列会在每个 Claude Code 会话结束时转换为记忆；对于其他工具，请运行 `icm extract-pending`（例如通过 cron 任务）。

存储和召回都在你的机器上进行。自动提取会在已安装的情况下，把文本交给你已经在用的 LLM 命令行工具（Claude Code、Codex 或 Gemini CLI）；在 `[extraction.summarizer]` 下设置 `provider = "none"` 可使其完全在本地运行。

想立刻看到效果，可以手动存储和召回：

```console
$ icm store -t decisions-myapp -c "Auth uses short-lived JWTs, refreshed through /auth/refresh" -i high
Stored: 01M47YY6CHVZ48BYKQRCRNZTCF

$ icm recall "how does auth work"
memories[1]{id,topic,importance,weight,summary}:
  01M47YY6CHVZ48BYKQRCRNZTCF,decisions-myapp,high,0.975,"Auth uses short-lived JWTs, refreshed through /auth/refresh"
```

首次使用语义搜索进行存储或召回时，会下载一次多语言嵌入模型（`Qdrant/multilingual-e5-large-onnx`，约 2 GB）。如果想不用它来试用 ICM，可以加上 `--no-embeddings`（关键词召回，如上面的输出所示），或在配置中选择更轻量的模型。`icm init` 会把钩子和指令写入每个检测到的智能体的配置中；`icm uninstall --dry-run` 会显示如何移除它们。Windows、Linux、Nix 以及从源码构建：[安装](#install)。

<a id="benchmark-comparison"></a>
## 基准对比

在 [LoCoMo](https://github.com/snap-research/locomo)（10 段长对话，1,540 个问题）上的回答准确率，使用公开的 [Agent Memory Benchmark](https://github.com/vectorize-io/agent-memory-benchmark) 测试框架测量：记忆系统检索上下文，`gemini-3.1-pro-preview` 根据上下文作答，`gemini-2.5-flash-lite` 评判答案。

| 系统 | LoCoMo 准确率 | 每个问题的上下文 | 存储一条记忆时的 LLM 调用 | 运行形态 | 结果 |
|--------|:---------------:|:--------------------:|:---------------------------:|---------|--------|
| **ICM** 0.11.0（召回引擎 v2） | **92.9%** (三次运行分别为 1,430、1,433 和 1,430 / 1,540) | 24.1k tokens | 无 | 一个 Rust 二进制文件，SQLite 文件 | 我们的运行，2026-10-06 |
| Hindsight | 92.0% (1,417 / 1,540) | 36.2k tokens | LLM 事实提取 | Python 服务，PostgreSQL + pgvector | 测试框架公布 |
| 混合搜索基线（稠密 + 稀疏，RRF） | 79.1% (1,218 / 1,540) | 22.2k tokens | 无 | Qdrant | 测试框架公布 |

在 [PersonaMem](https://arxiv.org/abs/2504.14225) 32k 上（589 道关于用户不断变化的偏好的多项选择题，测试框架和作答模型相同，按选项字母匹配评分）：

| 系统 | PersonaMem 准确率 | 每个问题的上下文 | 结果 |
|--------|:-------------------:|:--------------------:|--------|
| **ICM** 0.11.0（召回引擎 v2） | **81.7%** (三次运行分别为 486、486 和 472 / 589) | 16.2k tokens | 我们的运行，2026-10-06 |
| Hindsight | 86.6% (510 / 589) | 15.8k tokens | 测试框架公布 |
| 混合搜索基线 | 84.4% (497 / 589) | 24.2k tokens | 测试框架公布 |

仅检索，在 LoCoMo 上，不使用作答模型：至少有一个标准答案会话出现在靠前结果中的问题所占的比例。这是召回引擎本身的证据。

| 召回引擎 | 搜索 | Top 5 | Top 10 | Top 20 |
|---|---|:---:|:---:|:---:|
| **0.11** (默认) | 关键词 + 嵌入模型 | **86.7%** | **93.3%** | **97.9%** |
| **0.11** (默认) | 仅关键词 | **88.6%** | **94.2%** | **97.5%** |
| 0.10 (`--engine legacy`) | 关键词 + 嵌入模型 | 76.5% | 83.0% | 87.7% |
| 0.10 (`--engine legacy`) | 仅关键词 | 12.0% | 17.4% | 29.8% |

两个引擎均使用同一个 0.11 构建进行测量。0.11 引擎的运行获得了每个会话的日期和问题的日期；0.10 引擎没有日期输入，因此其运行未获得任何日期。

LongMemEval-S，仅检索，不使用 LLM（ICM 使用其默认嵌入模型；500 个问题；每个问题附带约 48 个需要搜索的历史会话；每个会话一条记忆，仅包含用户发言，与 MemPalace 的索引方式相同；不向 ICM 提供日期）：

| 进入前 5 条结果的正确会话 | **ICM** 0.11.0 | MemPalace | agentmemory | 仅 BM25 |
|---|:---:|:---:|:---:|:---:|
| 至少一个（公开的指标） | **97.4%** (487 / 500) | 96.6% (483 / 500) | 95.2% (476 / 500) | 94.6% |
| 全部 | **88.6%** (443 / 500) | 85.0% | 81.8% | 81.2% |

MemPalace 和 agentmemory 的数字是用我们的评分脚本，根据各项目公开的结果文件重新计算的；与它们公开的数字一致。agentmemory 索引会话中的所有发言；以此为单位，仅 BM25 就达到 96.2% 和 83.0%。普通的 BM25 在第一个指标上已经能达到 95% 左右，因此第二个指标（全部正确会话）更能区分各系统。

这些数字说明了什么，又没有说明什么：

- **ICM 和 Hindsight 在 LoCoMo 上持平。** ICM 的三次运行（92.9%、93.1%、92.9%）各自比 Hindsight 公布的 92.0% 高约 1 个百分点，差距约 14 个问题，在抽样误差范围内（ICM 的 95% 区间：91.6 到 94.2）。ICM 达到这一结果所用的上下文少三分之一，并且存储记忆时不调用 LLM。
- **在 PersonaMem 上，Hindsight 领先** 4.9 个百分点，超出抽样误差范围（ICM 的 95% 区间：78.6 到 84.8）。ICM 也比混合搜索基线低 2.7 个百分点，这在该区间之内，而读取的上下文比它少三分之一。三次运行的结果分布在 80.1% 到 82.5% 之间。
- **条件并不完全相同。** 该测试框架由 Hindsight 的开发商 Vectorize 维护。已公布的 LoCoMo 结果早于一次将作答和评判温度设为 0 的改动；我们的运行使用当前版本的测试框架（提交 `f618ed7`）和 Vertex AI。
- **在 50 个分块时，每段对话的大部分内容都会被返回，** 因此这个准确率也在衡量作答模型。检索表才是召回引擎本身的证据。
- **每个数据集运行三次。** 作答模型的结果在不同运行之间有波动：LoCoMo 上为 0.2 个百分点，PersonaMem 上为 2.4 个百分点。上面的区间覆盖的是问题的抽样。

<details>
<summary>分类别结果、延迟及更多注意事项</summary>

- **按问题类型**（LoCoMo，测试框架的标签，三次运行）：open-domain 96.7% (841 个问题)，temporal 90.9% (321)，single-hop 88.8% (282)，multi-hop 78.8% (96)。
- **延迟。** 在 LoCoMo 运行期间，集群的 4-vCPU 节点上的召回延迟中位数为 137 到 149 ms；摄入 272 个会话时没有调用 LLM。在 Linux x86-64 上仅检索的运行中：使用嵌入时中位数为 136 ms，仅关键词时为 6.8 ms。
- **会话日期。** 基准适配器会把每个会话的日期写入 ICM 的记忆文本；Hindsight 以元数据形式接收相同的日期，并且每个系统都会拿到问题的日期。
- **最弱的类别是测试框架标记为 multi-hop 的 96 个问题**（78.8%）。各基准之间的类别名称并不一致：其他 LoCoMo 评测把这一类称为 open-domain，而把测试框架标记为 single-hop 的 282 个问题（此处为 88.8%）称为 multi-hop。请按问题数量比较，而不是按标签。
- **召回引擎 v2 是默认引擎**，用于 `icm recall`、MCP 工具 `icm_memory_recall`、HTTP `/recall` 和提示钩子。之前的引擎仍然可用，便于回退或对比：`icm recall --engine legacy`、HTTP `/recall` 上的 `"engine": "legacy"`，或 `ICM_RECALL_ENGINE=legacy`。

</details>

每次运行的逐题结果（每个数据集三次，外加 LongMemEval-S 的召回运行）见 [`bench/amb/results/`](bench/amb/results/)；适配器、确切设置和复现命令见 [`bench/amb/README.md`](bench/amb/README.md)。

<a id="one-memory-for-every-tool"></a>
## 所有工具共用一份记忆

所有由 `icm init` 配置的工具都读写同一个 SQLite 数据库，主题（`decisions-myapp`、`preferences`、`errors-resolved`、...）不按工具划分。从 Claude Code 存储的记忆会立即对 Codex、Gemini、Cursor、Roo、Amp、Aider、... 可见。

想要隔离？`icm init --per-project` 会在 `.icm/` 下创建项目本地数据库（并在当前目录写入智能体的指令文件，例如 `CLAUDE.md` 和 `AGENTS.md`）；`--db <path>` 或 `ICM_DB` 可指向任意其他文件。每个路径都是一个独立的语料库。

> **项目状态：beta。** ICM 尚未到 1.0：任何次版本都可能带来破坏性变更，钩子和 MCP 的配置格式也可能变化。beta 指的是 API 的稳定性，而不是日常可用性：我（维护者）每天都把 ICM 作为主要的 AI 编程记忆来使用。我的主要精力在 [rtk](https://github.com/rtk-ai/rtk) 上，因此 issue 和 pull request 会尽力而为地审阅。
>
> Apache-2.0，**按原样（as-is）提供，不附带任何形式的担保**（见 [LICENSE](LICENSE)）。在执行任何破坏性操作之前，先运行对应的只读命令（`icm uninstall --dry-run`、`icm uninstall --check`）。

<a id="install"></a>
## 安装

```bash
# macOS / Linux, Homebrew
brew tap rtk-ai/tap && brew install icm

# macOS / Linux, script (verifies SHA256 against the release checksums)
curl -fsSL https://raw.githubusercontent.com/rtk-ai/icm/main/install.sh | sh

# Windows, PowerShell
irm https://raw.githubusercontent.com/rtk-ai/icm/main/install.ps1 | iex
```

关键词搜索在所有平台上都可用。运行 `icm embeddings status` 查看语义搜索是否已开启：macOS Apple Silicon、Windows 和 `.rpm` 构建已内置；Linux glibc 压缩包和 `.deb` 需要运行一次 `icm embeddings download`；Intel Mac 构建需要你自行提供 ONNX Runtime（`ORT_DYLIB_PATH`）；静态链接的 Linux musl 构建仅支持关键词。Nix、从源码构建、版本固定及详细说明：[参考文档](docs/reference.md#install)。

<a id="setup"></a>
## 配置

```bash
icm init                  # global database, every detected agent
icm init --per-project    # database under .icm/ at the git root
icm init --mode all       # also register the MCP server in every tool that supports it
```

默认模式（`standard`）会写入指令、技能（skills）和钩子，不包含 MCP 服务器。`--mode all` 会添加 MCP 服务器；使用它（并对 Aider 加上 `--per-project`，因为 Aider 的约定文件是按项目区分的），即可覆盖下面的 19 个工具（[集成指南](docs/integrations.md)）：

| 工具 | MCP 服务器 | 钩子 |
|------|:---:|:-----:|
| Claude Code | 支持 | 支持 |
| Claude Desktop | 支持 | — |
| Gemini CLI | 支持 | 支持 |
| Codex CLI | 支持 | 支持 |
| Copilot CLI | 支持 | 支持 |
| Cursor | 支持 | — |
| Windsurf | 支持 | — |
| VS Code | 支持 | — |
| Amp | 支持 | — |
| Amazon Q | 支持 | — |
| Cline | 支持 | — |
| Roo Code | 支持 | — |
| Kilo Code | 支持 | — |
| Zed | 支持 | — |
| OpenCode | 支持 | 支持 |
| Continue.dev | 支持 | — |
| Aider | — | — |
| Pi | — | — |
| Mistral Vibe | 支持 | 支持 (pre/post tool) |

也可以手动注册 MCP 服务器：`claude mcp add icm -- icm serve`（任意 MCP 客户端：命令 `icm`，参数 `["serve"]`）。

钩子的作用：

| 钩子 | 作用 |
|------|-------------|
| `icm hook start` | 在会话开始时注入由 critical/high 记忆组成的唤醒包（约 500 tokens） |
| `icm hook pre` | 自动允许 `icm` CLI 命令（无需权限确认） |
| `icm hook post` | 每 N 次调用从工具输出中提取事实（自动提取） |
| `icm hook compact` | 在上下文压缩前从对话记录中提取记忆 |
| `icm hook prompt` | 在每条用户提示的开头注入召回的上下文 |

各工具的钩子表、技能、指令文件以及关于 Codex 的说明：[参考文档](docs/reference.md#setup)。

<a id="use"></a>
## 使用

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

ICM 还会保存 **memoirs**（由概念和带类型的关系组成的永久知识图谱）、**feedback**（可供学习的纠正）和**逐字对话记录**，并提供 **32 个 MCP 工具**（无嵌入模型时为 31 个）、一个让嵌入模型保持加载状态的 **HTTP API**，以及一个**终端仪表盘**（`icm dashboard`）。所有这些内容都在[参考文档](docs/reference.md)中。

<a id="how-it-works"></a>
## 工作原理

召回通过倒数排名融合（RRF）合并最多三个排序列表：始终启用的 **FTS5 BM25** 关键词匹配；加载了嵌入模型时通过 sqlite-vec 进行的**语义向量搜索**（默认 `Qdrant/multilingual-e5-large-onnx`，1024 维，100+ 种语言）；以及查询中提到某个时间段时（"last week"、"in March 2024"）的**日期窗口**。项目、主题和关键词过滤在截断之前应用。记忆会根据其重要性随时间衰减（`critical` 永不衰减）；加载了嵌入模型时，与同一主题中已有记忆几乎相同的新记忆（余弦相似度高于 0.95）会被合并到已有记忆中；生成已存储向量的模型会记录在数据库中，因此在配置中更改 `model` 永远不会清除这些向量（`icm embed --migrate` 是显式切换的方式）。

所有数据都存放在一个 SQLite 文件中，不依赖任何外部服务：

```
~/Library/Application Support/dev.icm.icm/memories.db     # macOS (dev.icm.icm is the app identifier, not a dev build)
~/.local/share/icm/memories.db                            # Linux
%APPDATA%\icm\icm\data\memories.db                        # Windows
<project-root>/.icm/memories.db                           # icm init --per-project
```

`icm config` 显示当前生效的配置；[config/default.toml](config/default.toml) 列出了所有选项。详情：[参考文档](docs/reference.md#how-it-works)、[架构图](docs/architecture.md#architecture-at-a-glance)。

<a id="documentation"></a>
## 文档

| 文档 | 说明 |
|----------|-------------|
| [集成指南](docs/integrations.md) | 各工具的 MCP 配置：Claude Code、Cursor、Windsurf、Zed、Amp、Codex、Cline、Roo Code 等 |
| [技术架构](docs/architecture.md) | crate 结构、搜索流水线、衰减模型、sqlite-vec 集成、测试 |
| [用户指南](docs/guide.md) | 安装、主题组织、整合、提取、故障排查 |
| [产品概览](docs/product.md) | 使用场景、基准测试、与其他方案的比较 |
| [参考文档](docs/reference.md) | 安装选项、各工具配置、CLI、32 个 MCP 工具、HTTP API、仪表盘、内部机制 |
| [基准适配器](bench/amb/README.md) | 上述对比是如何运行的，以及如何复现 |
| [演示](docs/demonstrations.md) | 存储微基准测试和小型演示 |

<a id="license"></a>
## 许可证

[Apache-2.0](LICENSE)
