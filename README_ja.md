[English](README.md) | [Français](README_fr.md) | [Español](README_es.md) | [Deutsch](README_de.md) | [Italiano](README_it.md) | [Português](README_pt.md) | [Nederlands](README_nl.md) | [Polski](README_pl.md) | [Русский](README_ru.md) | **日本語** | [中文](README_zh.md) | [العربية](README_ar.md) | [한국어](README_ko.md)

<p align="center">
  <img src="assets/banner.png" alt="ICM — Infinite Context Memory" width="600">
</p>

<h1 align="center">ICM</h1>

<p align="center">
  AIエージェントのための永続的メモリ。シングルバイナリ、依存関係なし、MCPネイティブ。
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/icm/actions/workflows/ci.yml"><img src="https://github.com/rtk-ai/icm/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/icm/releases/latest"><img src="https://img.shields.io/github/v/release/rtk-ai/icm?color=purple" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-Source--Available-orange.svg" alt="Source-Available"></a>
</p>

---

ICMはAIエージェントに本物のメモリを与えます — メモ取りツールでもコンテキストマネージャーでもなく、**メモリ**そのものです。

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

**2つのメモリモデル:**

- **Memories（メモリ）** — 重要度に応じた時間的減衰を伴う記録・想起。重要度が「critical」のメモリは消えることなく、重要度が低いものは自然に減衰します。トピックやキーワードでフィルタリング可能。
- **Memoirs（回想録）** — 永続的ナレッジグラフ。コンセプトは型付きリレーション（`depends_on`、`contradicts`、`superseded_by` など）でリンクされます。ラベルでフィルタリング可能。
- **Feedback（フィードバック）** — AIの予測が間違っていた場合に修正を記録します。新たな予測を行う前に過去の間違いを検索できます。クローズドループ学習。

## インストール

```bash
# Homebrew (macOS / Linux)
brew tap rtk-ai/tap && brew install icm

# クイックインストール
curl -fsSL https://raw.githubusercontent.com/rtk-ai/icm/main/install.sh | sh

# ソースから
cargo install --path crates/icm-cli
```

## セットアップ

```bash
# サポートされているすべてのツールを自動検出して設定
icm init
```

1つのコマンドで **17のツール** を設定します（[完全な統合ガイド](docs/integrations.md)）:

| ツール | MCP | フック | CLI | スキル |
|--------|:---:|:-----:|:---:|:------:|
| Claude Code | `~/.claude.json` | 5フック | `CLAUDE.md` | `/recall` `/remember` |
| Claude Desktop | JSON | — | — | — |
| Gemini CLI | `~/.gemini/settings.json` | 5フック | `GEMINI.md` | — |
| Codex CLI | `~/.codex/config.toml` | 4フック | `AGENTS.md` | — |
| Copilot CLI | `~/.copilot/mcp-config.json` | 4フック | `.github/copilot-instructions.md` | — |
| Cursor | `~/.cursor/mcp.json` | — | — | `.mdc` ルール |
| Windsurf | JSON | — | `.windsurfrules` | — |
| VS Code | `~/Library/.../Code/User/mcp.json` | — | — | — |
| Amp | JSON | — | — | `/icm-recall` `/icm-remember` |
| Amazon Q | JSON | — | — | — |
| Cline | VS Code globalStorage | — | — | — |
| Roo Code | VS Code globalStorage | — | — | `.md` ルール |
| Kilo Code | VS Code globalStorage | — | — | — |
| Zed | `~/.zed/settings.json` | — | — | — |
| OpenCode | JSON | TSプラグイン | — | — |
| Continue.dev | `~/.continue/config.yaml` | — | — | — |
| Aider | — | — | `.aider.conventions.md` | — |

または手動で:

```bash
# Claude Code
claude mcp add icm -- icm serve

# コンパクトモード（短いレスポンス、トークン節約）
claude mcp add icm -- icm serve --compact

# 任意のMCPクライアント: command = "icm", args = ["serve"]
```

### スキル / ルール

```bash
icm init --mode skill
```

Claude Code（`/recall`、`/remember`）、Cursor（`.mdc` ルール）、Roo Code（`.md` ルール）、Amp（`/icm-recall`、`/icm-remember`）用のスラッシュコマンドとルールをインストールします。

### フック（5ツール）

```bash
icm init --mode hook
```

サポートされているすべてのツールに自動抽出・自動想起フックをインストールします:

| ツール | SessionStart | PreTool | PostTool | Compact | PromptRecall | 設定 |
|--------|:-----------:|:-------:|:--------:|:-------:|:------------:|--------|
| Claude Code | `icm hook start` | `icm hook pre` | `icm hook post` | `icm hook compact` | `icm hook prompt` | `~/.claude/settings.json` |
| Gemini CLI | `icm hook start` | `icm hook pre` | `icm hook post` | `icm hook compact` | `icm hook prompt` | `~/.gemini/settings.json` |
| Codex CLI | `icm hook start` | `icm hook pre` | `icm hook post` | — | `icm hook prompt` | `~/.codex/hooks.json` |
| Copilot CLI | `icm hook start` | `icm hook pre` | `icm hook post` | — | `icm hook prompt` | `.github/hooks/icm.json` |
| OpenCode | セッション開始 | — | ツール抽出 | コンパクション | — | `~/.config/opencode/plugins/icm.ts` |

**各フックの動作:**

| フック | 動作 |
|--------|-------------|
| `icm hook start` | セッション開始時にcritical/highメモリのウェイクアップパックを注入（約500トークン） |
| `icm hook pre` | `icm` CLIコマンドを自動許可（許可プロンプトなし） |
| `icm hook post` | N回のツール呼び出しごとにツール出力からファクトを抽出（自動抽出） |
| `icm hook compact` | コンテキスト圧縮前にトランスクリプトからメモリを抽出 |
| `icm hook prompt` | 各ユーザープロンプトの先頭に想起したコンテキストを注入 |

## CLI vs MCP

ICMはCLI（`icm` コマンド）またはMCPサーバー（`icm serve`）経由で使用できます。どちらも同じデータベースにアクセスします。

| | CLI | MCP |
|---|-----|-----|
| **レイテンシ** | ~30ms（直接バイナリ） | ~50ms（JSON-RPC stdio） |
| **トークンコスト** | 0（フックベース、不可視） | ~20-50トークン/呼び出し（ツールスキーマ） |
| **セットアップ** | `icm init --mode hook` | `icm init --mode mcp` |
| **対応ツール** | Claude Code、Gemini、Codex、Copilot、OpenCode（フック経由） | MCP対応の全17ツール |
| **自動抽出** | あり（フックが `icm extract` を起動） | あり（MCPツールがstoreを呼び出し） |
| **最適用途** | パワーユーザー、トークン節約 | ユニバーサル互換性 |

## CLI

### Memories（エピソード記憶、減衰あり）

```bash
# 記録
icm store -t "my-project" -c "Use PostgreSQL for the main DB" -i high -k "db,postgres"

# 想起
icm recall "database choice"
icm recall "auth setup" --topic "my-project" --limit 10
icm recall "architecture" --keyword "postgres"

# 管理
icm forget <memory-id>
icm consolidate --topic "my-project"
icm topics
icm stats

# テキストからファクトを抽出（ルールベース、LLMコストなし）
echo "The parser uses Pratt algorithm" | icm extract -p my-project
```

### Memoirs（永続的ナレッジグラフ）

```bash
# 回想録を作成
icm memoir create -n "system-architecture" -d "System design decisions"

# ラベル付きコンセプトを追加
icm memoir add-concept -m "system-architecture" -n "auth-service" \
  -d "Handles JWT tokens and OAuth2 flows" -l "domain:auth,type:service"

# コンセプトをリンク
icm memoir link -m "system-architecture" --from "api-gateway" --to "auth-service" -r depends-on

# ラベルフィルターで検索
icm memoir search -m "system-architecture" "authentication"
icm memoir search -m "system-architecture" "service" --label "domain:auth"

# 近傍を調査
icm memoir inspect -m "system-architecture" "auth-service" -D 2

# グラフをエクスポート（フォーマット: json, dot, ascii, ai）
icm memoir export -m "system-architecture" -f ascii   # 信頼度バー付きボックス描画
icm memoir export -m "system-architecture" -f dot      # Graphviz DOT（色 = 信頼度レベル）
icm memoir export -m "system-architecture" -f ai       # LLMコンテキスト最適化Markdown
icm memoir export -m "system-architecture" -f json     # 全メタデータ付き構造化JSON

# SVGビジュアライゼーションを生成
icm memoir export -m "system-architecture" -f dot | dot -Tsvg > graph.svg
```

## MCPツール（31個）

### メモリツール

| ツール | 説明 |
|--------|-------------|
| `icm_memory_store` | 自動重複排除付きで記録（類似度85%超 → 複製ではなく更新） |
| `icm_memory_recall` | クエリで検索、トピックやキーワードでフィルタリング |
| `icm_memory_update` | メモリをインプレースで編集（内容、重要度、キーワード） |
| `icm_memory_forget` | IDでメモリを削除 |
| `icm_memory_consolidate` | トピックのすべてのメモリを1つのサマリーにマージ |
| `icm_memory_list_topics` | 件数付きで全トピックを一覧表示 |
| `icm_memory_stats` | グローバルメモリ統計 |
| `icm_memory_health` | トピック別衛生監査（陳腐化、統合の必要性） |
| `icm_memory_embed_all` | ベクター検索用に埋め込みをバックフィル |

### 回想録ツール（ナレッジグラフ）

| ツール | 説明 |
|--------|-------------|
| `icm_memoir_create` | 新しい回想録（ナレッジコンテナ）を作成 |
| `icm_memoir_list` | 全回想録を一覧表示 |
| `icm_memoir_show` | 回想録の詳細と全コンセプトを表示 |
| `icm_memoir_add_concept` | ラベル付きコンセプトを追加 |
| `icm_memoir_refine` | コンセプトの定義を更新 |
| `icm_memoir_search` | 全文検索、オプションでラベルフィルター |
| `icm_memoir_search_all` | 全回想録を横断して検索 |
| `icm_memoir_link` | コンセプト間に型付きリレーションを作成 |
| `icm_memoir_inspect` | コンセプトとグラフ近傍を調査（BFS） |
| `icm_memoir_export` | 信頼度レベル付きグラフをエクスポート（json, dot, ascii, ai） |

### フィードバックツール（間違いから学ぶ）

| ツール | 説明 |
|--------|-------------|
| `icm_feedback_record` | AIの予測が間違っていた際に修正を記録 |
| `icm_feedback_search` | 将来の予測に活かすため過去の修正を検索 |
| `icm_feedback_stats` | フィードバック統計：総件数、トピック別内訳、最も適用された修正 |

### リレーションタイプ

`part_of` · `depends_on` · `related_to` · `contradicts` · `refines` · `alternative_to` · `caused_by` · `instance_of` · `superseded_by`

## 仕組み

### デュアルメモリモデル

**エピソード記憶（トピック）** は意思決定、エラー、好みを記録します。各メモリは重要度に基づいて時間とともに減衰するウェイトを持ちます:

| 重要度 | 減衰 | 削除 | 動作 |
|-----------|-------|-------|----------|
| `critical` | なし | しない | 忘れられず、削除されることもない |
| `high` | 遅い（0.5倍のレート） | しない | ゆっくり減衰し、自動削除されることはない |
| `medium` | 通常 | あり | 標準的な減衰、ウェイトが閾値を下回ると削除 |
| `low` | 速い（2倍のレート） | あり | 急速に忘れられる |

減衰は**アクセスを考慮**します：頻繁に想起されるメモリはより遅く減衰します（`decay / (1 + access_count × 0.1)`）。想起時に自動適用されます（最終減衰から24時間以上経過している場合）。

**メモリ衛生管理** が組み込まれています:
- **自動重複排除**: 同じトピック内の既存メモリとの類似度が85%を超える内容を記録すると、複製を作成する代わりに更新されます
- **統合ヒント**: トピックが7件を超えると、`icm_memory_store` が呼び出し元に統合を促します
- **衛生監査**: `icm_memory_health` はトピック別のエントリ数、平均ウェイト、陳腐化エントリ、統合の必要性を報告します
- **サイレントなデータ損失なし**: 重要度が「critical」と「high」のメモリは自動削除されることはありません

**セマンティック記憶（回想録）** は構造化されたナレッジをグラフとして記録します。コンセプトは永続的で、洗練されるだけで減衰しません。古くなったファクトを削除する代わりに `superseded_by` を使用してマークします。

### ハイブリッド検索

埋め込みが有効な場合、ICMはハイブリッド検索を使用します:
- **FTS5 BM25**（30%）— 全文キーワードマッチング
- **コサイン類似度**（70%）— sqlite-vecによるセマンティックベクター検索

デフォルトモデル: `intfloat/multilingual-e5-base`（768次元、100以上の言語）。[設定ファイル](#設定)で変更可能:

```toml
[embeddings]
# enabled = false                          # 完全に無効化（モデルのダウンロードなし）
model = "intfloat/multilingual-e5-base"    # 768次元、多言語（デフォルト）
# model = "intfloat/multilingual-e5-small" # 384次元、多言語（軽量）
# model = "intfloat/multilingual-e5-large" # 1024次元、多言語（最高精度）
# model = "Xenova/bge-small-en-v1.5"      # 384次元、英語のみ（最速）
# model = "jinaai/jina-embeddings-v2-base-code"  # 768次元、コード最適化
```

埋め込みモデルのダウンロードを完全にスキップするには、以下のいずれかを使用します:
```bash
icm --no-embeddings serve          # CLIフラグ
ICM_NO_EMBEDDINGS=1 icm serve     # 環境変数
```
または設定ファイルで `enabled = false` を設定します。ICMはFTS5キーワード検索にフォールバックします（動作しますが、セマンティックマッチングはありません）。

モデルを変更すると、自動的にベクターインデックスが再作成されます（既存の埋め込みはクリアされ、`icm_memory_embed_all` で再生成できます）。

### ストレージ

単一のSQLiteファイル。外部サービスやネットワーク依存なし。

```
~/Library/Application Support/dev.icm.icm/memories.db                    # macOS
~/.local/share/dev.icm.icm/memories.db                                   # Linux
C:\Users\<user>\AppData\Local\icm\icm\data\memories.db                   # Windows
```

### 設定

```bash
icm config                    # アクティブな設定を表示
```

設定ファイルの場所（プラットフォーム別、または `$ICM_CONFIG`）:

```
~/Library/Application Support/dev.icm.icm/config.toml                    # macOS
~/.config/icm/config.toml                                                # Linux
C:\Users\<user>\AppData\Roaming\icm\icm\config\config.toml              # Windows
```

すべてのオプションは [config/default.toml](config/default.toml) を参照してください。

## マルチプロジェクト・マルチエージェント

ICM は、一人のユーザーが複数のプロジェクトにまたがって複数のエージェントと協働するケースを想定して構築されています。メモリは関連性を保つ必要があります。プロジェクト A の決定がプロジェクト B に漏れてはならず、`dev` エージェントが `mentor` エージェントの保存内容で初期化されることもあってはなりません。

### プロジェクトの分離

ICM はメモリを別カラムではなく **トピックの命名規則** によってスコープ化します。規則は以下の通りです。

```
{kind}-{project}              # e.g. decisions-icm, errors-resolved-icm, contexte-rtk-cloud
preferences                   # global, always included
identity                      # global, always included
```

`icm_wake_up { project: "icm" }` は **セグメント単位** でマッチングを行います。`"icm"` は `decisions-icm`、`errors-icm-core`、`contexte-icm` にはマッチしますが、`icmp-notes` には決してマッチしません(誤検出なし)。トピックは `-`、`.`、`_`、`/`、`:` で分割されます。`preferences` と `identity` のトピックは設計上クロスプロジェクトです。ユーザーレベルのガイダンスが取り除かれることはありません。

`UserPromptSubmit` フック (`icm hook prompt`) と `SessionStart` フック (`icm hook start`) はどちらも、フック JSON の `cwd` フィールド(作業ディレクトリの `basename`)からプロジェクトを導出します。各プロジェクトを専用のディレクトリから実行すれば、分離は自動的に行われます。

### 良いメモリの書き方

`icm_memory_store` ではエージェントが `topic` と `content` を選ぶ必要があります。自動分類器はありません。ベストプラクティス:

| フィールド | ガイダンス |
|------|----------|
| `topic` | `{kind}-{project}`。種別: `decisions`、`errors-resolved`、`contexte`、`preferences`。 |
| `content` | 1 回の保存につき 1 つの事実。密度の高い英語の要約 — `topic + content` が埋め込み対象テキストになります。 |
| `raw_excerpt` | 逐語的なもののみ(コード、正確なエラーメッセージ、コマンド出力)。 |
| `keywords` | BM25 検索を強化するための 3〜5 個の語。 |
| `importance` | 絶対に忘れたくないものは `critical`、プロジェクトの決定は `high`、デフォルトは `medium`、一時的なものは `low`。 |

残りは ICM が処理します。**85% 類似度での重複排除**、意味的に近いメモリ間の **自動リンク**、トピックあたり 10 件を超えた場合の **自動統合**、そしてアクセス回数で重み付けされた **減衰** です。1 呼び出しあたり 1 つの事実とするほうがバッチ投入よりも優れています。リトリーバは個別に保存された事実をより上位にランク付けします。

### マルチエージェントのロール

ICM にはまだ第一級の `role` カラムはありません。現状では、ロールはトピックの接尾辞とエージェントごとの作業ディレクトリでエミュレートされます。

```
decisions-icm-dev             # dev agent: code patterns, library choices, refactors
decisions-icm-architect       # architect: design, workflows, subtask decomposition
decisions-icm-mentor          # mentor / BA: business goals, non-technical context
```

各エージェントは自分専用の作業ディレクトリ(`~/projects/icm-dev/`、`~/projects/icm-architect/`、…)で動作します。これにより `icm hook prompt` と `icm hook start` が `cwd` から異なるプロジェクトセグメントを導出し、対応するメモリのみを呼び出します。`preferences` はグローバルのままです。ユーザーのアイデンティティはすべてのロールにまたがって引き継がれます。

単一のエージェント内でも、手動で recall を絞り込めます。

```jsonc
// icm_memory_recall
{ "query": "auth flow", "topic": "decisions-icm-architect", "limit": 5 }
```

第一級の `role` フィールド(wake-up と recall でのネイティブフィルタリング付き)はロードマップに含まれています。それまでは、トピック接尾辞の規則がサポートされるパターンです。

## 自動抽出

ICMは3つのレイヤーを通じてメモリを自動的に抽出します:

```
  Layer 0: パターンフック           Layer 1: PreCompact           Layer 2: UserPromptSubmit
  （LLMコストなし）                  （LLMコストなし）               （LLMコストなし）
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

| レイヤー | 状態 | LLMコスト | フックコマンド | 説明 |
|-------|--------|----------|-------------|-------------|
| Layer 0 | 実装済み | 0 | `icm hook post` | ツール出力からのルールベースキーワード抽出 |
| Layer 1 | 実装済み | 0 | `icm hook compact` | コンテキスト圧縮前にトランスクリプトから抽出 |
| Layer 2 | 実装済み | 0 | `icm hook prompt` | 各ユーザープロンプト時に想起メモリを注入 |

3つのレイヤーすべては `icm init --mode hook` によって自動的にインストールされます。

### 代替手段との比較

| システム | 方法 | LLMコスト | レイテンシ | 圧縮をキャプチャ？ |
|--------|--------|----------|---------|---------------------|
| **ICM** | 3層抽出 | 0〜約500トークン/セッション | 0ms | **あり（PreCompact）** |
| Mem0 | メッセージごとに2回のLLM呼び出し | 約2kトークン/メッセージ | 200〜2000ms | なし |
| claude-mem | PostToolUse + 非同期 | 約1〜5kトークン/セッション | 8msフック | なし |
| MemGPT/Letta | エージェント自己管理 | 追加コスト0 | 0ms | なし |
| DiffMem | Gitベースdiff | 0 | 0ms | なし |

## ベンチマーク

### ストレージパフォーマンス

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

Apple M1 Pro、インメモリSQLite、シングルスレッド。`icm bench --count 1000`

### エージェント効率

実際のRustプロジェクト（12ファイル、約550行）を使用したマルチセッションワークフロー。セッション2以降は、ICMがファイルを再読み込みする代わりに想起することで最大の効果を発揮します。

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

### 知識保持

エージェントがセッションをまたいで密度の高い技術文書から特定のファクトを想起します。セッション1で読み込んで記憶し、セッション2以降はソーステキスト**なし**で10の事実質問に回答します。

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

### ローカルLLM（ollama）

ローカルモデルを使用した同じテスト — 純粋なコンテキスト注入、ツール使用不要。

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

### テストプロトコル

すべてのベンチマークは**実際のAPI呼び出し**を使用 — モックなし、シミュレートされたレスポンスなし、キャッシュされた回答なし。

- **エージェントベンチマーク**: tempdir内に実際のRustプロジェクトを作成。`claude -p --output-format json` でNセッションを実行。ICMなし: 空のMCP設定。ICMあり: 実際のMCPサーバー + 自動抽出 + コンテキスト注入。
- **知識保持**: 架空の技術文書（「Meridian Protocol」）を使用。期待されるファクトに対するキーワードマッチングで回答を採点。呼び出しごとに120秒タイムアウト。
- **分離**: 各実行は独自のtempdir と新鮮なSQLite DBを使用。セッション持続性なし。

### マルチエージェント統合メモリ

17のツールすべてが同じSQLiteデータベースを共有します。Claudeが保存したメモリは、Gemini、Codex、Copilot、Cursor、その他すべてのツールから即座にアクセスできます。

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

スコア = 想起精度60% + ファクト詳細度30% + 速度10%。**マルチエージェント効率98%。**

## ICMを選ぶ理由

| 機能 | ICM | Mem0 | Engram | AgentMemory |
|------|:---:|:----:|:------:|:-----------:|
| ツールサポート | **17** | SDKのみ | ~6-8 | ~10 |
| ワンコマンドセットアップ | `icm init` | 手動SDK | 手動 | 手動 |
| フック（起動時の自動想起） | 5ツール | なし | MCP経由 | 1ツール |
| ハイブリッド検索（FTS5 + ベクター） | 30/70加重 | ベクターのみ | FTS5のみ | FTS5+ベクター |
| 多言語埋め込み | 100+言語（768次元） | 依存 | なし | 英語384次元 |
| ナレッジグラフ | Memoirシステム | なし | なし | なし |
| 時間的減衰 + 統合 | アクセス考慮型 | なし | 基本的 | 基本的 |
| TUIダッシュボード | `icm dashboard` | なし | あり | Webビューア |
| ツール出力からの自動抽出 | 3レイヤー、LLMコストなし | なし | なし | なし |
| フィードバック/修正ループ | `icm_feedback_*` | なし | なし | なし |
| ランタイム | Rustシングルバイナリ | Python | Go | Node.js |
| ローカルファースト、依存関係なし | SQLiteファイル | クラウドファースト | SQLite | SQLite |
| マルチエージェント想起精度 | **98%** | N/A | N/A | 95.2% |

## ドキュメント

| ドキュメント | 説明 |
|----------|-------------|
| [統合ガイド](docs/integrations.md) | 全17ツールのセットアップ: Claude Code、Copilot、Cursor、Windsurf、Zed、Ampなど |
| [技術アーキテクチャ](docs/architecture.md) | クレート構造、検索パイプライン、減衰モデル、sqlite-vec統合、テスト |
| [ユーザーガイド](docs/guide.md) | インストール、トピック整理、統合、抽出、トラブルシューティング |
| [製品概要](docs/product.md) | ユースケース、ベンチマーク、代替手段との比較 |

## ライセンス

[Source-Available](LICENSE) — 個人および20人以下のチームは無料。それ以上の組織にはエンタープライズライセンスが必要です。お問い合わせ: contact@rtk-ai.app
