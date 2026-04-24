[English](README.md) | [Français](README_fr.md) | [Español](README_es.md) | [Deutsch](README_de.md) | [Italiano](README_it.md) | [Português](README_pt.md) | [Nederlands](README_nl.md) | [Polski](README_pl.md) | [Русский](README_ru.md) | [日本語](README_ja.md) | [中文](README_zh.md) | [العربية](README_ar.md) | **한국어**

<p align="center">
  <img src="assets/banner.png" alt="ICM — Infinite Context Memory" width="600">
</p>

<h1 align="center">ICM</h1>

<p align="center">
  AI 에이전트를 위한 영구 메모리. 단일 바이너리, 의존성 없음, MCP 네이티브.
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/icm/actions/workflows/ci.yml"><img src="https://github.com/rtk-ai/icm/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/icm/releases/latest"><img src="https://img.shields.io/github/v/release/rtk-ai/icm?color=purple" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-Source--Available-orange.svg" alt="Source-Available"></a>
</p>

---

ICM은 AI 에이전트에게 진짜 메모리를 제공합니다 — 메모 도구도, 컨텍스트 관리자도 아닌, **메모리** 그 자체입니다.

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

**두 가지 메모리 모델:**

- **Memories** — 중요도에 따른 시간적 감쇠와 함께 저장/검색. 중요 메모리는 절대 사라지지 않고, 낮은 중요도는 자연적으로 감쇠. 토픽 또는 키워드로 필터링 가능.
- **Memoirs** — 영구 지식 그래프. 타입이 지정된 관계(`depends_on`, `contradicts`, `superseded_by`, ...)로 연결된 개념. 레이블로 필터링 가능.
- **Feedback** — AI 예측이 틀렸을 때 수정 사항 기록. 새로운 예측 전에 과거 실수를 검색. 폐쇄 루프 학습.

## 설치

```bash
# Homebrew (macOS / Linux)
brew tap rtk-ai/tap && brew install icm

# 소스에서 빌드
cargo install --path crates/icm-cli
```

## 설정

```bash
# 지원되는 모든 도구를 자동 감지 및 구성
icm init
```

한 번의 명령으로 **17개 도구**를 구성합니다 ([전체 통합 가이드](docs/integrations.md)):

| 도구 | MCP | 훅 | CLI | 스킬 |
|------|:---:|:-----:|:---:|:------:|
| Claude Code | `~/.claude.json` | 5개 훅 | `CLAUDE.md` | `/recall` `/remember` |
| Claude Desktop | JSON | — | — | — |
| Gemini CLI | `~/.gemini/settings.json` | 5개 훅 | `GEMINI.md` | — |
| Codex CLI | `~/.codex/config.toml` | 4개 훅 | `AGENTS.md` | — |
| Copilot CLI | `~/.copilot/mcp-config.json` | 4개 훅 | `.github/copilot-instructions.md` | — |
| Cursor | `~/.cursor/mcp.json` | — | — | `.mdc` 규칙 |
| Windsurf | JSON | — | `.windsurfrules` | — |
| VS Code | `~/Library/.../Code/User/mcp.json` | — | — | — |
| Amp | JSON | — | — | `/icm-recall` `/icm-remember` |
| Amazon Q | JSON | — | — | — |
| Cline | VS Code globalStorage | — | — | — |
| Roo Code | VS Code globalStorage | — | — | `.md` 규칙 |
| Kilo Code | VS Code globalStorage | — | — | — |
| Zed | `~/.zed/settings.json` | — | — | — |
| OpenCode | JSON | TS 플러그인 | — | — |
| Continue.dev | `~/.continue/config.yaml` | — | — | — |
| Aider | — | — | `.aider.conventions.md` | — |

또는 수동으로:

```bash
# Claude Code
claude mcp add icm -- icm serve

# 컴팩트 모드 (더 짧은 응답, 토큰 절약)
claude mcp add icm -- icm serve --compact

# 모든 MCP 클라이언트: command = "icm", args = ["serve"]
```

### 스킬 / 규칙

```bash
icm init --mode skill
```

Claude Code(`/recall`, `/remember`), Cursor(`.mdc` 규칙), Roo Code(`.md` 규칙), Amp(`/icm-recall`, `/icm-remember`)에 슬래시 명령과 규칙을 설치합니다.

### 훅 (5개 도구)

```bash
icm init --mode hook
```

지원되는 모든 도구에 자동 추출 및 자동 회상 훅을 설치합니다:

| 도구 | SessionStart | PreTool | PostTool | Compact | PromptRecall | 설정 |
|------|:-----------:|:-------:|:--------:|:-------:|:------------:|--------|
| Claude Code | `icm hook start` | `icm hook pre` | `icm hook post` | `icm hook compact` | `icm hook prompt` | `~/.claude/settings.json` |
| Gemini CLI | `icm hook start` | `icm hook pre` | `icm hook post` | `icm hook compact` | `icm hook prompt` | `~/.gemini/settings.json` |
| Codex CLI | `icm hook start` | `icm hook pre` | `icm hook post` | — | `icm hook prompt` | `~/.codex/hooks.json` |
| Copilot CLI | `icm hook start` | `icm hook pre` | `icm hook post` | — | `icm hook prompt` | `.github/hooks/icm.json` |
| OpenCode | session start | — | tool extract | compaction | — | `~/.config/opencode/plugins/icm.ts` |

**각 훅의 역할:**

| 훅 | 역할 |
|------|-------------|
| `icm hook start` | 세션 시작 시 중요/높은 중요도 메모리의 웨이크업 팩 주입 (~500 토큰) |
| `icm hook pre` | `icm` CLI 명령 자동 허용 (권한 프롬프트 없음) |
| `icm hook post` | N번 호출마다 도구 출력에서 사실 추출 (자동 추출) |
| `icm hook compact` | 컨텍스트 압축 전 대화 스크립트에서 메모리 추출 |
| `icm hook prompt` | 각 사용자 프롬프트 시작 시 회상된 컨텍스트 주입 |

## CLI vs MCP

ICM은 CLI(`icm` 명령) 또는 MCP 서버(`icm serve`)를 통해 사용할 수 있습니다. 둘 다 동일한 데이터베이스에 접근합니다.

| | CLI | MCP |
|---|-----|-----|
| **지연 시간** | ~30ms (직접 바이너리) | ~50ms (JSON-RPC stdio) |
| **토큰 비용** | 0 (훅 기반, 보이지 않음) | ~20-50 토큰/호출 (도구 스키마) |
| **설정** | `icm init --mode hook` | `icm init --mode mcp` |
| **호환 도구** | Claude Code, Gemini, Codex, Copilot, OpenCode (훅 통해) | 17개 MCP 호환 도구 모두 |
| **자동 추출** | 예 (훅이 `icm extract` 트리거) | 예 (MCP 도구가 store 호출) |
| **최적 용도** | 파워 유저, 토큰 절약 | 범용 호환성 |

## CLI

### Memories (에피소드, 감쇠 포함)

```bash
# 저장
icm store -t "my-project" -c "Use PostgreSQL for the main DB" -i high -k "db,postgres"

# 회상
icm recall "database choice"
icm recall "auth setup" --topic "my-project" --limit 10
icm recall "architecture" --keyword "postgres"

# 관리
icm forget <memory-id>
icm consolidate --topic "my-project"
icm topics
icm stats

# 텍스트에서 사실 추출 (규칙 기반, LLM 비용 없음)
echo "The parser uses Pratt algorithm" | icm extract -p my-project
```

### Memoirs (영구 지식 그래프)

```bash
# memoir 생성
icm memoir create -n "system-architecture" -d "System design decisions"

# 레이블이 있는 개념 추가
icm memoir add-concept -m "system-architecture" -n "auth-service" \
  -d "Handles JWT tokens and OAuth2 flows" -l "domain:auth,type:service"

# 개념 연결
icm memoir link -m "system-architecture" --from "api-gateway" --to "auth-service" -r depends-on

# 레이블 필터로 검색
icm memoir search -m "system-architecture" "authentication"
icm memoir search -m "system-architecture" "service" --label "domain:auth"

# 이웃 검사
icm memoir inspect -m "system-architecture" "auth-service" -D 2

# 그래프 내보내기 (형식: json, dot, ascii, ai)
icm memoir export -m "system-architecture" -f ascii   # 신뢰도 바 포함 박스 드로잉
icm memoir export -m "system-architecture" -f dot      # Graphviz DOT (색상 = 신뢰도 수준)
icm memoir export -m "system-architecture" -f ai       # LLM 컨텍스트에 최적화된 마크다운
icm memoir export -m "system-architecture" -f json     # 모든 메타데이터 포함 구조화된 JSON

# SVG 시각화 생성
icm memoir export -m "system-architecture" -f dot | dot -Tsvg > graph.svg
```

## MCP 도구 (22개)

### 메모리 도구

| 도구 | 설명 |
|------|-------------|
| `icm_memory_store` | 자동 중복 제거와 함께 저장 (>85% 유사도 → 중복 대신 업데이트) |
| `icm_memory_recall` | 쿼리로 검색, 토픽 및/또는 키워드로 필터링 |
| `icm_memory_update` | 메모리 인플레이스 편집 (내용, 중요도, 키워드) |
| `icm_memory_forget` | ID로 메모리 삭제 |
| `icm_memory_consolidate` | 토픽의 모든 메모리를 하나의 요약으로 병합 |
| `icm_memory_list_topics` | 카운트와 함께 모든 토픽 나열 |
| `icm_memory_stats` | 글로벌 메모리 통계 |
| `icm_memory_health` | 토픽별 위생 감사 (오래됨, 통합 필요) |
| `icm_memory_embed_all` | 벡터 검색을 위한 임베딩 백필 |

### Memoir 도구 (지식 그래프)

| 도구 | 설명 |
|------|-------------|
| `icm_memoir_create` | 새 memoir 생성 (지식 컨테이너) |
| `icm_memoir_list` | 모든 memoir 나열 |
| `icm_memoir_show` | memoir 세부 정보 및 모든 개념 표시 |
| `icm_memoir_add_concept` | 레이블이 있는 개념 추가 |
| `icm_memoir_refine` | 개념의 정의 업데이트 |
| `icm_memoir_search` | 전체 텍스트 검색, 레이블로 선택적 필터링 |
| `icm_memoir_search_all` | 모든 memoir에서 검색 |
| `icm_memoir_link` | 개념 간 타입이 지정된 관계 생성 |
| `icm_memoir_inspect` | 개념 및 그래프 이웃 검사 (BFS) |
| `icm_memoir_export` | 신뢰도 수준과 함께 그래프 내보내기 (json, dot, ascii, ai) |

### 피드백 도구 (실수로부터 학습)

| 도구 | 설명 |
|------|-------------|
| `icm_feedback_record` | AI 예측이 틀렸을 때 수정 사항 기록 |
| `icm_feedback_search` | 미래 예측 정보를 위한 과거 수정 사항 검색 |
| `icm_feedback_stats` | 피드백 통계: 총 카운트, 토픽별 분류, 가장 많이 적용된 항목 |

### 관계 타입

`part_of` · `depends_on` · `related_to` · `contradicts` · `refines` · `alternative_to` · `caused_by` · `instance_of` · `superseded_by`

## 작동 방식

### 이중 메모리 모델

**에피소드 메모리 (Topics)**는 결정, 오류, 선호도를 캡처합니다. 각 메모리는 중요도에 따라 시간이 지남에 따라 감쇠하는 가중치를 가집니다:

| 중요도 | 감쇠 | 정리 | 동작 |
|-----------|-------|-------|----------|
| `critical` | 없음 | 절대 안 함 | 절대 잊혀지지 않음, 절대 정리되지 않음 |
| `high` | 느림 (0.5x 속도) | 절대 안 함 | 느리게 감쇠, 자동 삭제 없음 |
| `medium` | 보통 | 예 | 표준 감쇠, 가중치 < 임계값이면 정리됨 |
| `low` | 빠름 (2x 속도) | 예 | 빠르게 잊혀짐 |

감쇠는 **접근 인식** 방식입니다: 자주 회상되는 메모리는 더 느리게 감쇠합니다(`decay / (1 + access_count × 0.1)`). 회상 시 자동으로 적용됩니다 (마지막 감쇠 이후 >24시간인 경우).

**메모리 위생**이 내장되어 있습니다:
- **자동 중복 제거**: 같은 토픽에서 기존 메모리와 >85% 유사한 내용을 저장하면 중복 생성 대신 업데이트됨
- **통합 힌트**: 토픽이 7개 항목을 초과하면 `icm_memory_store`가 호출자에게 통합 권고
- **건강 감사**: `icm_memory_health`는 토픽별 항목 수, 평균 가중치, 오래된 항목, 통합 필요 여부 보고
- **무음 데이터 손실 없음**: critical 및 high 중요도 메모리는 자동 정리되지 않음

**시맨틱 메모리 (Memoirs)**는 구조화된 지식을 그래프로 캡처합니다. 개념은 영구적입니다 — 정제되며, 감쇠되지 않습니다. 사실을 삭제하는 대신 `superseded_by`를 사용하여 오래된 사실을 표시합니다.

### 하이브리드 검색

임베딩이 활성화되면 ICM은 하이브리드 검색을 사용합니다:
- **FTS5 BM25** (30%) — 전체 텍스트 키워드 매칭
- **코사인 유사도** (70%) — sqlite-vec를 통한 시맨틱 벡터 검색

기본 모델: `intfloat/multilingual-e5-base` (768d, 100+ 언어). [설정 파일](#설정)에서 구성 가능:

```toml
[embeddings]
# enabled = false                          # 완전히 비활성화 (모델 다운로드 없음)
model = "intfloat/multilingual-e5-base"    # 768d, 다국어 (기본값)
# model = "intfloat/multilingual-e5-small" # 384d, 다국어 (더 가벼움)
# model = "intfloat/multilingual-e5-large" # 1024d, 다국어 (최고 정확도)
# model = "Xenova/bge-small-en-v1.5"      # 384d, 영어 전용 (가장 빠름)
# model = "jinaai/jina-embeddings-v2-base-code"  # 768d, 코드 최적화
```

임베딩 모델 다운로드를 완전히 건너뛰려면 다음 중 하나를 사용합니다:
```bash
icm --no-embeddings serve          # CLI 플래그
ICM_NO_EMBEDDINGS=1 icm serve     # 환경 변수
```
또는 설정 파일에서 `enabled = false`로 설정합니다. ICM은 FTS5 키워드 검색으로 대체됩니다 (여전히 작동하지만 시맨틱 매칭 없음).

모델을 변경하면 자동으로 벡터 인덱스가 재생성됩니다 (기존 임베딩이 지워지며 `icm_memory_embed_all`로 재생성 가능).

### 저장소

단일 SQLite 파일. 외부 서비스, 네트워크 의존성 없음.

```
~/Library/Application Support/dev.icm.icm/memories.db                    # macOS
~/.local/share/dev.icm.icm/memories.db                                   # Linux
C:\Users\<user>\AppData\Local\icm\icm\data\memories.db                   # Windows
```

### 설정

```bash
icm config                    # 활성 설정 표시
```

설정 파일 위치 (플랫폼별, 또는 `$ICM_CONFIG`):

```
~/Library/Application Support/dev.icm.icm/config.toml                    # macOS
~/.config/icm/config.toml                                                # Linux
C:\Users\<user>\AppData\Roaming\icm\icm\config\config.toml              # Windows
```

모든 옵션은 [config/default.toml](config/default.toml)을 참조하세요.

## 자동 추출

ICM은 세 가지 레이어를 통해 메모리를 자동으로 추출합니다:

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

| 레이어 | 상태 | LLM 비용 | 훅 명령 | 설명 |
|-------|--------|----------|-------------|-------------|
| Layer 0 | 구현됨 | 0 | `icm hook post` | 도구 출력에서 규칙 기반 키워드 추출 |
| Layer 1 | 구현됨 | 0 | `icm hook compact` | 컨텍스트 압축 전 스크립트에서 추출 |
| Layer 2 | 구현됨 | 0 | `icm hook prompt` | 각 사용자 프롬프트에서 회상된 메모리 주입 |

3개 레이어 모두 `icm init --mode hook`으로 자동 설치됩니다.

### 대안과의 비교

| 시스템 | 방법 | LLM 비용 | 지연 시간 | 압축 캡처? |
|--------|--------|----------|---------|---------------------|
| **ICM** | 3레이어 추출 | 0 ~ ~500 토큰/세션 | 0ms | **예 (PreCompact)** |
| Mem0 | 메시지당 2번 LLM 호출 | ~2k 토큰/메시지 | 200-2000ms | 아니오 |
| claude-mem | PostToolUse + 비동기 | ~1-5k 토큰/세션 | 8ms 훅 | 아니오 |
| MemGPT/Letta | 에이전트 자체 관리 | 추가 비용 없음 | 0ms | 아니오 |
| DiffMem | Git 기반 diff | 0 | 0ms | 아니오 |

## 벤치마크

### 저장 성능

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

Apple M1 Pro, 인메모리 SQLite, 단일 스레드. `icm bench --count 1000`

### 에이전트 효율성

실제 Rust 프로젝트(12개 파일, ~550줄)를 사용한 다중 세션 워크플로. 세션 2+에서 ICM이 파일을 다시 읽는 대신 회상하면서 가장 큰 효율을 보입니다.

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

### 지식 보존

에이전트가 세션 간에 밀도 높은 기술 문서에서 특정 사실을 회상합니다. 세션 1은 읽고 기억하며; 세션 2+는 소스 텍스트 **없이** 10개의 사실적 질문에 답합니다.

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

### 로컬 LLM (ollama)

로컬 모델로 동일한 테스트 — 순수 컨텍스트 주입, 도구 사용 필요 없음.

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

### 테스트 프로토콜

모든 벤치마크는 **실제 API 호출**을 사용합니다 — 목(mock) 없음, 시뮬레이션된 응답 없음, 캐시된 답변 없음.

- **에이전트 벤치마크**: 임시 디렉터리에 실제 Rust 프로젝트를 생성합니다. `claude -p --output-format json`으로 N개의 세션을 실행합니다. ICM 없이: 빈 MCP 설정. ICM 있이: 실제 MCP 서버 + 자동 추출 + 컨텍스트 주입.
- **지식 보존**: 가상의 기술 문서("Meridian Protocol")를 사용합니다. 예상 사실에 대한 키워드 매칭으로 답변 채점. 호출당 120초 타임아웃.
- **격리**: 각 실행은 자체 임시 디렉터리와 새로운 SQLite DB를 사용합니다. 세션 지속성 없음.

### 멀티 에이전트 통합 메모리

17개 도구 모두 동일한 SQLite 데이터베이스를 공유합니다. Claude가 저장한 메모리는 즉시 Gemini, Codex, Copilot, Cursor 및 모든 다른 도구에서 사용할 수 있습니다.

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

점수 = 60% 회상 정확도 + 30% 사실 세부 정보 + 10% 속도. **98% 멀티 에이전트 효율성.**

## ICM을 선택하는 이유

| 기능 | ICM | Mem0 | Engram | AgentMemory |
|-----------|:---:|:----:|:------:|:-----------:|
| 도구 지원 | **17** | SDK만 | ~6-8 | ~10 |
| 원커맨드 설정 | `icm init` | 수동 SDK | 수동 | 수동 |
| 훅 (시작 시 자동 회상) | 5개 도구 | 없음 | MCP 통해 | 1개 도구 |
| 하이브리드 검색 (FTS5 + 벡터) | 30/70 가중치 | 벡터만 | FTS5만 | FTS5+벡터 |
| 다국어 임베딩 | 100+ 언어 (768d) | 상황에 따라 | 없음 | 영어 384d |
| 지식 그래프 | Memoir 시스템 | 없음 | 없음 | 없음 |
| 시간적 감쇠 + 통합 | 접근 인식 | 없음 | 기본 | 기본 |
| TUI 대시보드 | `icm dashboard` | 없음 | 있음 | 웹 뷰어 |
| 도구 출력에서 자동 추출 | 3 레이어, 제로 LLM | 없음 | 없음 | 없음 |
| 피드백/수정 루프 | `icm_feedback_*` | 없음 | 없음 | 없음 |
| 런타임 | Rust 단일 바이너리 | Python | Go | Node.js |
| 로컬 우선, 제로 의존성 | SQLite 파일 | 클라우드 우선 | SQLite | SQLite |
| 멀티 에이전트 회상 정확도 | **98%** | N/A | N/A | 95.2% |

## 문서

| 문서 | 설명 |
|----------|-------------|
| [통합 가이드](docs/integrations.md) | 17개 도구 모두 설정: Claude Code, Copilot, Cursor, Windsurf, Zed, Amp 등 |
| [기술 아키텍처](docs/architecture.md) | 크레이트 구조, 검색 파이프라인, 감쇠 모델, sqlite-vec 통합, 테스트 |
| [사용자 가이드](docs/guide.md) | 설치, 토픽 구성, 통합, 추출, 문제 해결 |
| [제품 개요](docs/product.md) | 사용 사례, 벤치마크, 대안과의 비교 |

## 라이선스

[Source-Available](LICENSE) — 개인 및 20명 이하 팀에게 무료. 대규모 조직은 엔터프라이즈 라이선스 필요. 문의: contact@rtk-ai.app
