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

1つのコマンドで **14のツール** を設定します:

| ツール | 設定ファイル | フォーマット |
|--------|------------|--------|
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

### フック（Claude Code）

```bash
icm init --mode hook
```

3つの抽出レイヤーすべてをClaude Codeフックとしてインストールします:

**Claude Code** フック:

| フック | イベント | 動作 |
|--------|-------|-------------|
| `icm hook pre` | PreToolUse | `icm` CLIコマンドを自動許可（許可プロンプトなし） |
| `icm hook post` | PostToolUse | 15回のツール呼び出しごとにツール出力からファクトを抽出 |
| `icm hook compact` | PreCompact | コンテキスト圧縮前にトランスクリプトからメモリを抽出 |
| `icm hook prompt` | UserPromptSubmit | 各プロンプトの先頭に想起したコンテキストを注入 |

**OpenCode** プラグイン（`~/.config/opencode/plugins/icm.js` に自動インストール）:

| OpenCodeイベント | ICMレイヤー | 動作 |
|---------------|-----------|-------------|
| `tool.execute.after` | Layer 0 | ツール出力からファクトを抽出 |
| `experimental.session.compacting` | Layer 1 | 圧縮前に会話から抽出 |
| `session.created` | Layer 2 | セッション開始時にコンテキストを想起 |

## CLI vs MCP

ICMはCLI（`icm` コマンド）またはMCPサーバー（`icm serve`）経由で使用できます。どちらも同じデータベースにアクセスします。

| | CLI | MCP |
|---|-----|-----|
| **レイテンシ** | ~30ms（直接バイナリ） | ~50ms（JSON-RPC stdio） |
| **トークンコスト** | 0（フックベース、不可視） | ~20-50トークン/呼び出し（ツールスキーマ） |
| **セットアップ** | `icm init --mode hook` | `icm init --mode mcp` |
| **対応ツール** | Claude Code、OpenCode（フック/プラグイン経由） | MCP対応の全14ツール |
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

## MCPツール（22個）

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

## ドキュメント

| ドキュメント | 説明 |
|----------|-------------|
| [技術アーキテクチャ](docs/architecture.md) | クレート構造、検索パイプライン、減衰モデル、sqlite-vec統合、テスト |
| [ユーザーガイド](docs/guide.md) | インストール、トピック整理、統合、抽出、トラブルシューティング |
| [製品概要](docs/product.md) | ユースケース、ベンチマーク、代替手段との比較 |

## ライセンス

[Source-Available](LICENSE) — 個人および20人以下のチームは無料。それ以上の組織にはエンタープライズライセンスが必要です。お問い合わせ: contact@rtk-ai.app
