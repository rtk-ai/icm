[English](README.md) | [Français](README_fr.md) | [Español](README_es.md) | [Deutsch](README_de.md) | [Italiano](README_it.md) | [Português](README_pt.md) | [Nederlands](README_nl.md) | [Polski](README_pl.md) | [Русский](README_ru.md) | [日本語](README_ja.md) | [中文](README_zh.md) | [العربية](README_ar.md) | [한국어](README_ko.md)

これは README.md（英語）の翻訳です。英語版が基準であり、内容が異なる場合は英語版が正しいものとします。

<h1 align="center">ICM</h1>

<p align="center">
  <b>AI コーディングエージェントのための長期記憶。使っている複数のツールで共有されます。</b><br>
  単一のバイナリと単一の SQLite ファイル。記憶の保存にも想起にも LLM の呼び出しは不要です。
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/icm/actions/workflows/ci.yml"><img src="https://github.com/rtk-ai/icm/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/icm/releases/latest"><img src="https://img.shields.io/github/v/release/rtk-ai/icm?color=purple" alt="リリース"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-Apache--2.0-blue.svg" alt="Apache-2.0"></a>
</p>

月曜日に Claude Code へプロジェクトの認証の仕組みを伝えれば、火曜日の Gemini CLI セッションはすでにそれを知っています。ICM は、コーディングエージェントが学んだこと（決定事項、修正、規約、好み）を手元のマシン上の単一の SQLite ファイルに保存し、各セッションの開始時と、送信するプロンプトごとに、関連する部分を返します。最大 18 のエージェントやエディタがこの記憶を共有するため、セッションを開くたび、ツールを切り替えるたびにプロジェクトを説明し直す必要がなくなります。

- **LoCoMo（1,540 問、3 回の実行の平均）で 92.9%、Hindsight（92.0%）と同水準**で、質問あたりのコンテキストは 3 分の 1 少なくなっています。[詳細と注意点は後述](#benchmark-comparison)。
- **保存にも想起にも LLM 呼び出しは不要です。** Hindsight、Mem0、Graphiti (Zep)、claude-mem は、デフォルトでは記憶を保存するたびに LLM を呼び出します。ICM は呼び出しません。ツール出力からの事実の自動抽出だけが、すでに使っている LLM のコマンドラインツールがインストールされている場合に、それを経由します。`provider = "none"` にすれば、これもローカルに留まります（[クイックスタート](#quickstart)を参照）。
- **埋め込みモデルなしでも使えます。** キーワード想起だけで、LoCoMo の質問の 88.6% について、正しいセッションのうち少なくとも 1 つが上位 5 件に入ります。Linux x86-64 での想起 1 回あたりの中央値は 6.8 ms です。
- **LongMemEval-S で 97.4%（LLM を使わない検索）**: 500 問中 487 問で、正しいセッションのうち 1 つが上位 5 件に入ります。同じ指標で MemPalace は 96.6%、agentmemory は 95.2% です。
- **すべての面で上回っているわけではありません。** PersonaMem（ユーザーの変化していく好み）では Hindsight が上回っており、86.6% 対 ICM の 81.7% です。

<p align="center">
  <img src="assets/demo.svg" alt="ターミナル: icm store で 3 件の記憶を保存し、続いて icm recall が 2 つの質問に答え、それぞれ正しい記憶を返している様子">
</p>

<a id="quickstart"></a>
## クイックスタート

```bash
brew tap rtk-ai/tap && brew install icm    # or: curl -fsSL https://raw.githubusercontent.com/rtk-ai/icm/main/install.sh | sh
icm init                                   # instructions, skills and hooks for every agent it detects
```

セットアップはこれだけです。Claude Code、Codex、Gemini CLI、Copilot CLI のいずれかで新しいセッションを開くと、エージェントは、作業しているプロジェクトの最も重要な記憶（`myapp` という名前のリポジトリでの `decisions-myapp` のように、トピックにリポジトリ名が含まれる記憶と、ユーザーの好み）をまとめた短いパックを持って開始し、送信するプロンプトごとに関連する記憶を受け取ります。ツール出力から学んだことはキューに入れられ、そのキューは Claude Code の各セッションの終了時に記憶に変換されます。他のツールでは `icm extract-pending` を実行してください（たとえば cron ジョブから）。

保存と想起は手元のマシン上で行われます。自動抽出は、すでに使っている LLM のコマンドラインツール（Claude Code、Codex、Gemini CLI）がインストールされている場合、そこにテキストを渡します。完全にローカルで動かすには、`[extraction.summarizer]` の下で `provider = "none"` を設定してください。

すぐに動作を確認するには、手動で保存と想起を行います:

```console
$ icm store -t decisions-myapp -c "Auth uses short-lived JWTs, refreshed through /auth/refresh" -i high
Stored: 01M47YY6CHVZ48BYKQRCRNZTCF

$ icm recall "how does auth work"
memories[1]{id,topic,importance,weight,summary}:
  01M47YY6CHVZ48BYKQRCRNZTCF,decisions-myapp,high,0.975,"Auth uses short-lived JWTs, refreshed through /auth/refresh"
```

セマンティック検索を使う最初の保存または想起で、多言語埋め込みモデル（`Qdrant/multilingual-e5-large-onnx`、約 2 GB）が一度だけダウンロードされます。これなしで ICM を試すには、`--no-embeddings` を付ける（上の出力のようなキーワード想起になります）か、設定でより軽いモデルを選んでください。`icm init` は検出した各エージェントの設定にフックと指示を書き込みます。`icm uninstall --dry-run` でそれらの削除方法を確認できます。Windows、Linux、Nix、ソースからのビルド: [インストール](#install)。

<a id="benchmark-comparison"></a>
## ベンチマーク比較

[LoCoMo](https://github.com/snap-research/locomo)（10 件の長い会話、1,540 問）での回答精度を、公開されている [Agent Memory Benchmark](https://github.com/vectorize-io/agent-memory-benchmark) ハーネスで測定しました。メモリシステムがコンテキストを取得し、`gemini-3.1-pro-preview` がそれをもとに回答し、`gemini-2.5-flash-lite` が回答を判定します。

| システム | LoCoMo 精度 | 質問あたりのコンテキスト | 記憶の保存時の LLM 呼び出し | 実行形態 | 結果 |
|--------|:---------------:|:--------------------:|:---------------------------:|---------|--------|
| **ICM** 0.11.0（想起エンジン v2） | **92.9%** (3 回の実行で 1,430、1,433、1,430 / 1,540) | 24.1k tokens | なし | 単一の Rust バイナリ、SQLite ファイル | 当方の実行、2026-10-06 |
| Hindsight | 92.0% (1,417 / 1,540) | 36.2k tokens | LLM による事実抽出 | Python サービス、PostgreSQL + pgvector | ハーネスによる公開値 |
| ハイブリッド検索ベースライン（dense + sparse、RRF） | 79.1% (1,218 / 1,540) | 22.2k tokens | なし | Qdrant | ハーネスによる公開値 |

[PersonaMem](https://arxiv.org/abs/2504.14225) 32k（ユーザーの変化していく好みに関する 589 問の多肢選択式問題。ハーネスと回答モデルは同じで、選択肢の文字の一致で採点）:

| システム | PersonaMem 精度 | 質問あたりのコンテキスト | 結果 |
|--------|:-------------------:|:--------------------:|--------|
| **ICM** 0.11.0（想起エンジン v2） | **81.7%** (3 回の実行で 486、486、472 / 589) | 16.2k tokens | 当方の実行、2026-10-06 |
| Hindsight | 86.6% (510 / 589) | 15.8k tokens | ハーネスによる公開値 |
| ハイブリッド検索ベースライン | 84.4% (497 / 589) | 24.2k tokens | ハーネスによる公開値 |

LoCoMo での検索のみの結果（回答モデルなし）: 正解セッションのうち少なくとも 1 つが上位の結果に含まれる質問の割合です。これが想起エンジンそのものの根拠となる数値です。

| 想起エンジン | 検索 | Top 5 | Top 10 | Top 20 |
|---|---|:---:|:---:|:---:|
| **0.11** (デフォルト) | キーワード + 埋め込みモデル | **86.7%** | **93.3%** | **97.9%** |
| **0.11** (デフォルト) | キーワードのみ | **88.6%** | **94.2%** | **97.5%** |
| 0.10 (`--engine legacy`) | キーワード + 埋め込みモデル | 76.5% | 83.0% | 87.7% |
| 0.10 (`--engine legacy`) | キーワードのみ | 12.0% | 17.4% | 29.8% |

どちらのエンジンも同じ 0.11 のビルドで測定しました。0.11 エンジンの実行には各セッションの日付と質問の日付を与えました。0.10 エンジンには日付の入力がないため、その実行には日付を与えていません。

LongMemEval-S での検索のみの結果（LLM なし。ICM はデフォルトの埋め込みモデルを使用。500 問。各質問には検索対象となる過去のセッションが約 48 件付いています。MemPalace のインデックス方法に合わせ、セッションごとに 1 件の記憶、ユーザーの発話のみ。ICM には日付を与えていません）:

| 上位 5 件に入った正しいセッション | **ICM** 0.11.0 | MemPalace | agentmemory | BM25 のみ |
|---|:---:|:---:|:---:|:---:|
| 少なくとも 1 つ（公開されている指標） | **97.4%** (487 / 500) | 96.6% (483 / 500) | 95.2% (476 / 500) | 94.6% |
| すべて | **88.6%** (443 / 500) | 85.0% | 81.8% | 81.2% |

MemPalace と agentmemory の数値は、各プロジェクトが公開している結果ファイルから当方の採点スクリプトで再計算したもので、それぞれの公開値と一致します。agentmemory はセッションのすべての発話をインデックスします。その単位では BM25 のみで 96.2% と 83.0% に達します。単純な BM25 でも最初の指標ではすでに 95% 前後に達するため、2 つ目の指標（正しいセッションすべて）のほうがシステム間の差をよく示します。

これらの数値が示すこと、示さないこと:

- **LoCoMo では ICM と Hindsight は互角です。** ICM の 3 回の実行（92.9%、93.1%、92.9%）はいずれも Hindsight の公開値 92.0% を約 1 ポイント上回っており、差は約 14 問分で、サンプリング誤差の範囲内です（ICM の 95% 区間: 91.6 から 94.2）。ICM はこれを 3 分の 1 少ないコンテキストで、記憶の保存時に LLM を呼び出さずに達成しています。
- **PersonaMem では Hindsight が上回っています。** 差は 4.9 ポイントで、サンプリング誤差の範囲外です（ICM の 95% 区間: 78.6 から 84.8）。ICM はハイブリッド検索ベースラインも 2.7 ポイント下回っていますが、これはこの区間の範囲内で、読むコンテキストはそれより 3 分の 1 少なくなっています。3 回の実行の結果は 80.1% から 82.5% の範囲にあります。
- **条件は同一ではありません。** ハーネスは Hindsight の開発元である Vectorize が保守しています。公開されている LoCoMo の結果は、回答と判定の temperature を 0 に設定した変更より前のものです。当方の実行は現在のハーネス（コミット `f618ed7`）と Vertex AI を使用しています。
- **50 チャンクでは各会話の大部分が返されます。** そのため、この精度は回答モデルの性能も測っています。想起エンジンそのものの根拠は検索の表です。
- **データセットごとに 3 回実行しています。** 回答モデルの結果は実行ごとにばらつき、その幅は LoCoMo で 0.2 ポイント、PersonaMem で 2.4 ポイントです。上記の区間は質問のサンプリングによるばらつきをカバーするものです。

<details>
<summary>カテゴリ別の結果、レイテンシ、その他の注意点</summary>

- **質問タイプ別**（LoCoMo、ハーネスのラベル、3 回の実行）: open-domain 96.7% (841 問)、temporal 90.9% (321)、single-hop 88.8% (282)、multi-hop 78.8% (96)。
- **レイテンシ。** LoCoMo の実行中、クラスタの 4 vCPU ノードでの想起レイテンシの中央値は 137 から 149 ms でした。272 セッションを LLM 呼び出しなしで取り込んでいます。Linux x86-64 での検索のみの実行では、中央値は埋め込みありで 136 ms、キーワードのみで 6.8 ms です。
- **セッションの日付。** ベンチマークアダプタは各セッションの日付を ICM の記憶テキストに書き込みます。Hindsight は同じ日付をメタデータとして受け取り、すべてのシステムが質問の日付を受け取ります。
- **最も弱いカテゴリは、ハーネスが multi-hop とラベル付けした 96 問です**（78.8%）。カテゴリ名はベンチマーク間で一致しません。LoCoMo の他の評価ではこのカテゴリを open-domain と呼び、ハーネスが single-hop とラベル付けした 282 問（ここでは 88.8%）を multi-hop と呼んでいます。ラベルではなく問題数で比較してください。
- **想起エンジン v2 がデフォルトです。** 対象は `icm recall`、MCP の `icm_memory_recall` ツール、HTTP `/recall`、プロンプトフックです。以前のエンジンも、ロールバックや比較のために引き続き利用できます: `icm recall --engine legacy`、HTTP `/recall` での `"engine": "legacy"`、または `ICM_RECALL_ENGINE=legacy`。

</details>

すべての実行（データセットごとに 3 回、および LongMemEval-S での想起の実行）の質問ごとの結果は [`bench/amb/results/`](bench/amb/results/) にあります。アダプタ、正確な設定、再現用のコマンドは [`bench/amb/README.md`](bench/amb/README.md) にあります。

<a id="one-memory-for-every-tool"></a>
## すべてのツールで共通の記憶

`icm init` で設定されたすべてのツールが同じ SQLite データベースを読み書きし、トピック（`decisions-myapp`、`preferences`、`errors-resolved`、...）はツールごとに分割されません。Claude Code から保存した記憶は、Codex、Gemini、Cursor、Roo、Amp、Aider、... からすぐに参照できます。

分離したい場合は、`icm init --per-project` で `.icm/` の下にプロジェクトローカルなデータベースを作成します（あわせて `CLAUDE.md` や `AGENTS.md` などエージェントの指示ファイルをカレントディレクトリに書き込みます）。`--db <path>` または `ICM_DB` で他の任意のファイルを指定できます。パスごとに独立したコーパスになります。

> **プロジェクトの状態: ベータ。** ICM は 1.0 より前の段階にあり、どのマイナーリリースでも互換性のない変更が入る可能性があり、フックと MCP の設定形式も変わることがあります。ベータは API の安定性を指すもので、日常的な有用性を指すものではありません。私（メンテナ）は ICM を主要な AI コーディング用の記憶として毎日使っています。私の主な関心は [rtk](https://github.com/rtk-ai/rtk) にあるため、issue や pull request のレビューはできる範囲で行います。
>
> Apache-2.0 で、**現状のまま（as-is）、いかなる種類の保証もなく**提供されます（[LICENSE](LICENSE) を参照）。破壊的な操作の前には、まず読み取り専用の同等コマンド（`icm uninstall --dry-run`、`icm uninstall --check`）を実行してください。

<a id="install"></a>
## インストール

```bash
# macOS / Linux, Homebrew
brew tap rtk-ai/tap && brew install icm

# macOS / Linux, script (verifies SHA256 against the release checksums)
curl -fsSL https://raw.githubusercontent.com/rtk-ai/icm/main/install.sh | sh

# Windows, PowerShell
irm https://raw.githubusercontent.com/rtk-ai/icm/main/install.ps1 | iex
```

キーワード検索はどの環境でも動作します。セマンティック検索が有効かどうかは `icm embeddings status` で確認できます。macOS Apple Silicon、Windows、`.rpm` のビルドには組み込まれています。Linux glibc のアーカイブと `.deb` では `icm embeddings download` を一度実行する必要があります。Intel Mac のビルドでは ONNX Runtime を自分で用意する必要があります（`ORT_DYLIB_PATH`）。静的リンクの Linux musl ビルドはキーワード検索のみです。Nix、ソースからのビルド、バージョンの固定、詳細: [リファレンス](docs/reference.md#install)。

<a id="setup"></a>
## セットアップ

```bash
icm init                  # global database, every detected agent
icm init --per-project    # database under .icm/ at the git root
icm init --mode all       # also register the MCP server in every tool that supports it
```

デフォルトのモード（`standard`）は、MCP サーバーなしで指示、スキル、フックを書き込みます。`--mode all` は MCP サーバーを追加します。これを使うと（さらに、規約ファイルがプロジェクトごとにある Aider には `--per-project` も使うと）、以下の 18 のツールをカバーします（[統合ガイド](docs/integrations.md)）:

| ツール | MCP サーバー | フック |
|------|:---:|:-----:|
| Claude Code | 対応 | 対応 |
| Claude Desktop | 対応 | — |
| Gemini CLI | 対応 | 対応 |
| Codex CLI | 対応 | 対応 |
| Copilot CLI | 対応 | 対応 |
| Cursor | 対応 | — |
| Windsurf | 対応 | — |
| VS Code | 対応 | — |
| Amp | 対応 | — |
| Amazon Q | 対応 | — |
| Cline | 対応 | — |
| Roo Code | 対応 | — |
| Kilo Code | 対応 | — |
| Zed | 対応 | — |
| OpenCode | 対応 | 対応 |
| Continue.dev | 対応 | — |
| Aider | — | — |
| Pi | — | — |

または MCP サーバーを手動で登録します: `claude mcp add icm -- icm serve`（任意の MCP クライアント: コマンド `icm`、引数 `["serve"]`）。

フックの役割:

| フック | 内容 |
|------|-------------|
| `icm hook start` | セッション開始時に critical/high の記憶からなるウェイクアップパックを注入（約 500 トークン） |
| `icm hook pre` | `icm` CLI コマンドを自動許可（権限確認なし） |
| `icm hook post` | N 回の呼び出しごとにツール出力から事実を抽出（自動抽出） |
| `icm hook compact` | コンテキスト圧縮の前にトランスクリプトから記憶を抽出 |
| `icm hook prompt` | ユーザーの各プロンプトの先頭に想起したコンテキストを注入 |

ツールごとのフック一覧、スキル、指示ファイル、Codex に関する注意: [リファレンス](docs/reference.md#setup)。

<a id="use"></a>
## 使い方

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

ICM はさらに、**memoirs**（概念と型付きの関係からなる永続的なナレッジグラフ）、**feedback**（学習に使う修正）、**逐語的なトランスクリプト**も保持し、**32 個の MCP ツール**（埋め込みモデルなしでは 31 個）、埋め込みモデルを読み込んだ状態に保つ **HTTP API**、**ターミナルダッシュボード**（`icm dashboard`）を提供します。すべて[リファレンス](docs/reference.md)に記載されています。

<a id="how-it-works"></a>
## 仕組み

想起は、最大 3 つのランク付きリストを相互順位融合（RRF）で統合します。常に有効な **FTS5 BM25** のキーワード照合、埋め込みモデルが読み込まれている場合の sqlite-vec による**セマンティックベクトル検索**（デフォルトは `Qdrant/multilingual-e5-large-onnx`、1024 次元、100 以上の言語）、そしてクエリが期間を指定している場合（"last week"、"in March 2024"）の**日付ウィンドウ**です。プロジェクト、トピック、キーワードのフィルタは切り捨ての前に適用されます。記憶は重要度に応じて時間とともに減衰します（`critical` は減衰しません）。埋め込みモデルが読み込まれている場合、同じトピック内の既存の記憶とほぼ同一の新しい記憶（コサイン類似度が 0.95 超）は、その既存の記憶に統合されます。また、保存されたベクトルを生成したモデルはデータベースに記録されるため、設定の `model` を変更してもベクトルが消えることはありません（切り替えの明示的な方法は `icm embed --migrate` です）。

すべては外部サービスなしで単一の SQLite ファイルに保存されます:

```
~/Library/Application Support/dev.icm.icm/memories.db     # macOS (dev.icm.icm is the app identifier, not a dev build)
~/.local/share/icm/memories.db                            # Linux
%APPDATA%\icm\icm\data\memories.db                        # Windows
<project-root>/.icm/memories.db                           # icm init --per-project
```

`icm config` は有効な設定を表示します。[config/default.toml](config/default.toml) にすべてのオプションが記載されています。詳細: [リファレンス](docs/reference.md#how-it-works)、[アーキテクチャ図](docs/architecture.md#architecture-at-a-glance)。

<a id="documentation"></a>
## ドキュメント

| ドキュメント | 説明 |
|----------|-------------|
| [統合ガイド](docs/integrations.md) | ツールごとの MCP セットアップ: Claude Code、Cursor、Windsurf、Zed、Amp、Codex、Cline、Roo Code など |
| [技術アーキテクチャ](docs/architecture.md) | クレート構成、検索パイプライン、減衰モデル、sqlite-vec の統合、テスト |
| [ユーザーガイド](docs/guide.md) | インストール、トピックの整理、集約、抽出、トラブルシューティング |
| [製品概要](docs/product.md) | ユースケース、ベンチマーク、代替手段との比較 |
| [リファレンス](docs/reference.md) | インストールオプション、ツールごとのセットアップ、CLI、32 個の MCP ツール、HTTP API、ダッシュボード、内部構造 |
| [ベンチマークアダプタ](bench/amb/README.md) | 上記の比較の実施方法と再現方法 |
| [デモ](docs/demonstrations.md) | ストレージのマイクロベンチマークと小さなデモ |

<a id="license"></a>
## ライセンス

[Apache-2.0](LICENSE)
