[English](README.md) | [Français](README_fr.md) | [Español](README_es.md) | [Deutsch](README_de.md) | [Italiano](README_it.md) | [Português](README_pt.md) | [Nederlands](README_nl.md) | [Polski](README_pl.md) | [Русский](README_ru.md) | [日本語](README_ja.md) | [中文](README_zh.md) | [العربية](README_ar.md) | [한국어](README_ko.md)

이 문서는 README.md(영어)의 번역본이며, 기준 문서는 영어 원문입니다. 두 문서가 다를 경우 영어판이 맞습니다.

<h1 align="center">ICM</h1>

<p align="center">
  <b>AI 코딩 에이전트를 위한 장기 메모리, 사용하는 여러 도구가 함께 공유합니다.</b><br>
  바이너리 하나, SQLite 파일 하나. 메모리를 저장하거나 검색할 때 LLM을 호출하지 않습니다.
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/icm/actions/workflows/ci.yml"><img src="https://github.com/rtk-ai/icm/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/icm/releases/latest"><img src="https://img.shields.io/github/v/release/rtk-ai/icm?color=purple" alt="릴리스"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-Apache--2.0-blue.svg" alt="Apache-2.0"></a>
</p>

월요일에 Claude Code에게 프로젝트의 인증 처리 방식을 알려 주면, 화요일의 Gemini CLI 세션은 이미 그 내용을 알고 있습니다. ICM은 코딩 에이전트가 배운 것(결정, 수정, 규칙, 선호)을 사용자 머신의 SQLite 파일 하나에 보관하고, 각 세션이 시작될 때와 프롬프트를 보낼 때마다 관련된 부분을 다시 제공합니다. 최대 18개의 에이전트와 에디터가 이 메모리를 공유하므로, 세션을 열거나 도구를 바꿀 때마다 프로젝트를 다시 설명할 필요가 없습니다.

- **LoCoMo(1,540문항, 세 번 실행한 평균)에서 92.9%, Hindsight(92.0%)와 같은 수준**이며, 질문당 컨텍스트는 3분의 1 적습니다. [자세한 내용과 주의 사항은 아래에](#benchmark-comparison).
- **저장과 검색에 LLM 호출이 없습니다.** Hindsight, Mem0, Graphiti (Zep), claude-mem은 기본적으로 메모리를 저장할 때마다 LLM을 호출합니다. ICM은 그렇지 않습니다. 도구 출력에서 사실을 자동으로 추출하는 기능만, 이미 사용 중인 LLM 명령줄 도구가 설치되어 있을 때 그 도구를 거칩니다. `provider = "none"`으로 설정하면 이것도 로컬에 머뭅니다([빠른 시작](#quickstart) 참고).
- **임베딩 모델 없이도 쓸 수 있습니다.** 키워드 검색만으로도 LoCoMo 질문의 88.6%에서 정답 세션 중 하나 이상이 상위 5개 결과 안에 들며, Linux x86-64에서 검색 1회당 중앙값은 6.8 ms입니다.
- **LongMemEval-S에서 97.4%, LLM 없는 검색**: 500문항 중 487문항에서 정답 세션 중 하나가 상위 5개 결과 안에 들며, 같은 기준에서 MemPalace는 96.6%, agentmemory는 95.2%입니다.
- **모든 면에서 앞서지는 않습니다.** PersonaMem(사용자의 변화하는 선호)에서는 Hindsight가 앞섭니다: 86.6% 대 ICM의 81.7%.

<p align="center">
  <img src="assets/demo.svg" alt="터미널: icm store로 메모리 세 개를 저장한 뒤 icm recall이 두 질문에 답하며 매번 올바른 메모리를 반환하는 모습">
</p>

<a id="quickstart"></a>
## 빠른 시작

```bash
brew tap rtk-ai/tap && brew install icm    # or: curl -fsSL https://raw.githubusercontent.com/rtk-ai/icm/main/install.sh | sh
icm init                                   # instructions, skills and hooks for every agent it detects
```

설정은 이것으로 끝입니다. Claude Code, Codex, Gemini CLI 또는 Copilot CLI에서 새 세션을 열면, 에이전트는 현재 있는 프로젝트의 가장 중요한 메모리(`myapp`이라는 저장소의 `decisions-myapp`처럼 토픽에 저장소 이름이 들어 있는 메모리와 사용자의 선호)를 모은 짧은 팩을 가지고 시작하며, 보내는 프롬프트마다 관련된 메모리를 받습니다. 에이전트가 도구 출력에서 배운 내용은 큐에 쌓이고, 이 큐는 각 Claude Code 세션이 끝날 때 메모리로 변환됩니다. 다른 도구에서는 `icm extract-pending`을 실행하세요(예를 들어 cron 작업으로).

저장과 검색은 사용자의 머신에서 이루어집니다. 자동 추출은 이미 사용 중인 LLM 명령줄 도구(Claude Code, Codex 또는 Gemini CLI)가 설치되어 있으면 그 도구에 텍스트를 넘깁니다. 완전히 로컬로 유지하려면 `[extraction.summarizer]` 아래에 `provider = "none"`을 설정하세요.

바로 동작을 확인하려면 직접 저장하고 검색해 보세요:

```console
$ icm store -t decisions-myapp -c "Auth uses short-lived JWTs, refreshed through /auth/refresh" -i high
Stored: 01M47YY6CHVZ48BYKQRCRNZTCF

$ icm recall "how does auth work"
memories[1]{id,topic,importance,weight,summary}:
  01M47YY6CHVZ48BYKQRCRNZTCF,decisions-myapp,high,0.975,"Auth uses short-lived JWTs, refreshed through /auth/refresh"
```

시맨틱 검색을 사용하는 첫 저장 또는 검색 때 다국어 임베딩 모델(`Qdrant/multilingual-e5-large-onnx`, 약 2 GB)을 한 번 다운로드합니다. 이 모델 없이 ICM을 써 보려면 `--no-embeddings`를 추가하거나(위 출력처럼 키워드 검색) 설정에서 더 가벼운 모델을 고르세요. `icm init`은 감지된 각 에이전트의 설정에 훅과 지침을 기록합니다. `icm uninstall --dry-run`으로 이를 제거하는 방법을 확인할 수 있습니다. Windows, Linux, Nix, 소스에서 빌드: [설치](#install).

<a id="benchmark-comparison"></a>
## 벤치마크 비교

[LoCoMo](https://github.com/snap-research/locomo)(긴 대화 10개, 1,540문항)에서의 답변 정확도이며, 공개된 [Agent Memory Benchmark](https://github.com/vectorize-io/agent-memory-benchmark) 하네스로 측정했습니다. 메모리 시스템이 컨텍스트를 가져오고, `gemini-3.1-pro-preview`가 이를 바탕으로 답하며, `gemini-2.5-flash-lite`가 답을 채점합니다.

| 시스템 | LoCoMo 정확도 | 질문당 컨텍스트 | 메모리 저장 시 LLM 호출 | 실행 형태 | 결과 |
|--------|:---------------:|:--------------------:|:---------------------------:|---------|--------|
| **ICM** 0.11.0 (검색 엔진 v2) | **92.9%** (세 번 실행에서 1,430, 1,433, 1,430 / 1,540) | 24.1k tokens | 없음 | Rust 바이너리 하나, SQLite 파일 | 자체 실행, 2026-10-06 |
| Hindsight | 92.0% (1,417 / 1,540) | 36.2k tokens | LLM 사실 추출 | Python 서비스, PostgreSQL + pgvector | 하네스 공개 결과 |
| 하이브리드 검색 기준선 (dense + sparse, RRF) | 79.1% (1,218 / 1,540) | 22.2k tokens | 없음 | Qdrant | 하네스 공개 결과 |

[PersonaMem](https://arxiv.org/abs/2504.14225) 32k(사용자의 변화하는 선호에 관한 객관식 589문항, 같은 하네스와 답변 모델, 선택지 문자 일치로 채점)에서는:

| 시스템 | PersonaMem 정확도 | 질문당 컨텍스트 | 결과 |
|--------|:-------------------:|:--------------------:|--------|
| **ICM** 0.11.0 (검색 엔진 v2) | **81.7%** (세 번 실행에서 486, 486, 472 / 589) | 16.2k tokens | 자체 실행, 2026-10-06 |
| Hindsight | 86.6% (510 / 589) | 15.8k tokens | 하네스 공개 결과 |
| 하이브리드 검색 기준선 | 84.4% (497 / 589) | 24.2k tokens | 하네스 공개 결과 |

답변 모델 없이 LoCoMo에서 검색만 평가한 결과입니다. 정답 세션 중 하나 이상이 상위 결과에 포함된 질문의 비율이며, 이것이 검색 엔진 자체에 대한 근거입니다. v2 실행에는 각 세션의 날짜와 질문의 날짜를 제공했습니다. 이전 엔진은 날짜 입력이 없으므로 이전 엔진 실행에는 날짜를 제공하지 않았습니다.

| 상위 결과 수 | 이전 엔진 (`legacy`) | 검색 엔진 v2 | 이전 엔진, 임베딩 모델 없음 | 검색 엔진 v2, 임베딩 모델 없음 |
|:-----------:|:--------------:|:----------------:|:--------------:|:----------------:|
| 5 | 76.5% | **86.7%** | 12.0% | **88.6%** |
| 10 | 83.0% | **93.3%** | 17.4% | **94.2%** |
| 20 | 87.7% | **97.9%** | 29.8% | **97.5%** |

LongMemEval-S, 검색만 평가, LLM 없음(ICM은 기본 임베딩 모델 사용, 500문항, 질문마다 검색 대상인 과거 세션 약 48개, MemPalace의 인덱싱 방식대로 세션당 메모리 하나에 사용자 발화만 포함, ICM에는 날짜를 제공하지 않음):

| 상위 5개 결과 안의 정답 세션 | **ICM** 0.11.0 | MemPalace | agentmemory | BM25 단독 |
|---|:---:|:---:|:---:|:---:|
| 하나 이상 (공개된 기준) | **97.4%** (487 / 500) | 96.6% (483 / 500) | 95.2% (476 / 500) | 94.6% |
| 전부 | **88.6%** (443 / 500) | 85.0% | 81.8% | 81.2% |

MemPalace와 agentmemory 수치는 각 프로젝트가 공개한 결과 파일에서 자체 채점 스크립트로 다시 계산한 것이며, 각자의 공개 수치와 일치합니다. agentmemory는 세션의 모든 발화를 인덱싱하며, 그 단위에서는 BM25 단독으로 96.2%와 83.0%에 이릅니다. 단순한 BM25도 첫 번째 기준에서는 이미 95% 안팎의 점수를 내므로, 두 번째 기준인 정답 세션 전부가 시스템 간 차이를 더 잘 드러냅니다.

이 수치가 보여 주는 것과 보여 주지 않는 것:

- **LoCoMo에서 ICM과 Hindsight는 동률입니다.** ICM의 세 번 실행(92.9%, 93.1%, 92.9%)은 각각 Hindsight가 공개한 92.0%보다 약 1포인트 높으며, 이는 약 14문항 차이로 표본 오차 범위 안에 있습니다(ICM의 95% 구간: 91.6에서 94.2). ICM은 3분의 1 적은 컨텍스트로, 그리고 메모리를 저장할 때 LLM을 호출하지 않고 이 결과를 냅니다.
- **PersonaMem에서는 Hindsight가 앞섭니다.** 차이는 4.9포인트로, 표본 오차 범위 밖입니다(ICM의 95% 구간: 78.6에서 84.8). ICM은 하이브리드 검색 기준선보다도 2.7포인트 낮지만 이는 그 구간 안이며, 읽는 컨텍스트는 그보다 3분의 1 적습니다. 세 번 실행의 결과는 80.1%에서 82.5% 사이입니다.
- **조건이 동일하지 않습니다.** 하네스는 Hindsight의 개발사인 Vectorize가 관리합니다. 공개된 LoCoMo 결과는 답변 및 채점 temperature를 0으로 설정한 변경 이전에 나온 것입니다. 자체 실행은 현재 하네스(커밋 `f618ed7`)와 Vertex AI를 사용합니다.
- **청크 50개에서는 각 대화의 상당 부분이 반환되므로,** 이 정확도는 답변 모델도 함께 측정합니다. 검색 엔진 자체에 대한 근거는 검색 표입니다.
- **데이터셋당 세 번 실행했습니다.** 답변 모델의 결과는 실행마다 달라지며, 그 폭은 LoCoMo에서 0.2포인트, PersonaMem에서 2.4포인트입니다. 위의 구간은 질문 표본 추출을 반영합니다.

<details>
<summary>범주별 결과, 지연 시간, 추가 주의 사항</summary>

- **질문 유형별** (LoCoMo, 하네스 레이블, 세 번 실행): open-domain 96.7% (841문항), temporal 90.9% (321), single-hop 88.8% (282), multi-hop 78.8% (96).
- **지연 시간.** LoCoMo 실행 중 클러스터의 4-vCPU 노드에서 검색 지연 시간 중앙값은 137에서 149 ms 사이였고, 272개 세션을 LLM 호출 없이 수집했습니다. Linux x86-64에서 검색만 수행한 실행에서는 중앙값이 임베딩 사용 시 136 ms, 키워드만 사용 시 6.8 ms였습니다.
- **세션 날짜.** 벤치마크 어댑터는 각 세션의 날짜를 ICM의 메모리 텍스트에 기록합니다. Hindsight는 같은 날짜를 메타데이터로 받으며, 모든 시스템이 질문의 날짜를 받습니다.
- **가장 약한 범주는 하네스가 multi-hop으로 레이블을 붙인 96문항입니다** (78.8%). 범주 이름은 벤치마크마다 일치하지 않습니다. 다른 LoCoMo 평가에서는 이 범주를 open-domain이라 부르고, 하네스가 single-hop으로 레이블을 붙인 282문항(여기서는 88.8%)을 multi-hop이라 부릅니다. 레이블이 아니라 문항 수로 비교하세요.
- **검색 엔진 v2가 기본값입니다.** 적용 대상은 `icm recall`, MCP `icm_memory_recall` 도구, HTTP `/recall`, 프롬프트 훅입니다. 이전 엔진은 롤백이나 비교를 위해 계속 사용할 수 있습니다: `icm recall --engine legacy`, HTTP `/recall`의 `"engine": "legacy"`, 또는 `ICM_RECALL_ENGINE=legacy`.

</details>

모든 실행(데이터셋당 세 번, 그리고 LongMemEval-S 검색 실행)의 문항별 결과는 [`bench/amb/results/`](bench/amb/results/)에 있고, 어댑터, 정확한 설정, 재현 명령은 [`bench/amb/README.md`](bench/amb/README.md)에 있습니다.

<a id="one-memory-for-every-tool"></a>
## 모든 도구를 위한 하나의 메모리

`icm init`으로 설정된 모든 도구는 같은 SQLite 데이터베이스를 읽고 쓰며, 토픽(`decisions-myapp`, `preferences`, `errors-resolved`, ...)은 도구별로 나뉘지 않습니다. Claude Code에서 저장한 메모리는 Codex, Gemini, Cursor, Roo, Amp, Aider, ...에서 바로 보입니다.

격리가 필요한가요? `icm init --per-project`는 `.icm/` 아래에 프로젝트 로컬 데이터베이스를 만들고(또한 `CLAUDE.md`, `AGENTS.md` 같은 에이전트 지침 파일을 현재 디렉터리에 기록합니다), `--db <path>` 또는 `ICM_DB`로 다른 파일을 지정할 수 있습니다. 경로마다 독립된 코퍼스입니다.

> **프로젝트 상태: 베타.** ICM은 1.0 이전 버전입니다. 어떤 마이너 릴리스에서도 호환성을 깨는 변경이 들어올 수 있고, 훅과 MCP 설정 형식이 바뀔 수 있습니다. 베타는 API 안정성을 뜻하며, 일상적인 유용성을 뜻하지 않습니다. 저(메인테이너)는 ICM을 주된 AI 코딩 메모리로 매일 사용합니다. 제 주된 관심사는 [rtk](https://github.com/rtk-ai/rtk)이므로, 이슈와 풀 리퀘스트는 가능한 범위에서 검토합니다.
>
> Apache-2.0이며, **있는 그대로(as-is), 어떠한 종류의 보증도 없이** 제공됩니다([LICENSE](LICENSE) 참고). 파괴적인 작업을 하기 전에 먼저 그에 해당하는 읽기 전용 명령(`icm uninstall --dry-run`, `icm uninstall --check`)을 실행하세요.

<a id="install"></a>
## 설치

```bash
# macOS / Linux, Homebrew
brew tap rtk-ai/tap && brew install icm

# macOS / Linux, script (verifies SHA256 against the release checksums)
curl -fsSL https://raw.githubusercontent.com/rtk-ai/icm/main/install.sh | sh

# Windows, PowerShell
irm https://raw.githubusercontent.com/rtk-ai/icm/main/install.ps1 | iex
```

키워드 검색은 어디서나 동작합니다. 시맨틱 검색이 켜져 있는지는 `icm embeddings status`로 확인하세요. macOS Apple Silicon, Windows, `.rpm` 빌드에는 내장되어 있고, Linux glibc 아카이브와 `.deb`는 `icm embeddings download`를 한 번 실행해야 하며, Intel Mac 빌드는 ONNX Runtime을 직접 준비해야 하고(`ORT_DYLIB_PATH`), 정적 Linux musl 빌드는 키워드 검색만 지원합니다. Nix, 소스에서 빌드, 버전 고정 및 자세한 내용: [레퍼런스](docs/reference.md#install).

<a id="setup"></a>
## 설정

```bash
icm init                  # global database, every detected agent
icm init --per-project    # database under .icm/ at the git root
icm init --mode all       # also register the MCP server in every tool that supports it
```

기본 모드(`standard`)는 MCP 서버 없이 지침, 스킬, 훅을 기록합니다. `--mode all`은 MCP 서버를 추가합니다. 이 옵션을 쓰면(그리고 규칙 파일이 프로젝트별로 있는 Aider에는 `--per-project`도 함께 쓰면) 아래 18개 도구를 지원합니다([통합 가이드](docs/integrations.md)):

| 도구 | MCP 서버 | 훅 |
|------|:---:|:-----:|
| Claude Code | 지원 | 지원 |
| Claude Desktop | 지원 | — |
| Gemini CLI | 지원 | 지원 |
| Codex CLI | 지원 | 지원 |
| Copilot CLI | 지원 | 지원 |
| Cursor | 지원 | — |
| Windsurf | 지원 | — |
| VS Code | 지원 | — |
| Amp | 지원 | — |
| Amazon Q | 지원 | — |
| Cline | 지원 | — |
| Roo Code | 지원 | — |
| Kilo Code | 지원 | — |
| Zed | 지원 | — |
| OpenCode | 지원 | 지원 |
| Continue.dev | 지원 | — |
| Aider | — | — |
| Pi | — | — |

또는 MCP 서버를 직접 등록하세요: `claude mcp add icm -- icm serve` (모든 MCP 클라이언트: 명령 `icm`, 인수 `["serve"]`).

훅이 하는 일:

| 훅 | 하는 일 |
|------|-------------|
| `icm hook start` | 세션 시작 시 critical/high 메모리로 구성된 웨이크업 팩을 주입(~500 토큰) |
| `icm hook pre` | `icm` CLI 명령을 자동 허용(권한 확인 없음) |
| `icm hook post` | N회 호출마다 도구 출력에서 사실을 추출(자동 추출) |
| `icm hook compact` | 컨텍스트 압축 전에 대화 기록에서 메모리를 추출 |
| `icm hook prompt` | 각 사용자 프롬프트의 시작 부분에 검색된 컨텍스트를 주입 |

도구별 훅 표, 스킬, 지침 파일, Codex 관련 참고 사항: [레퍼런스](docs/reference.md#setup).

<a id="use"></a>
## 사용법

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

ICM은 **memoirs**(개념과 타입이 있는 관계로 이루어진 영구 지식 그래프), **feedback**(학습에 쓰는 수정 사항), **원문 그대로의 대화 기록**도 보관하며, **31개의 MCP 도구**(임베딩 모델이 없으면 30개), 임베딩 모델을 로드된 상태로 유지하는 **HTTP API**, **터미널 대시보드**(`icm dashboard`)를 제공합니다. 모든 내용은 [레퍼런스](docs/reference.md)에 있습니다.

<a id="how-it-works"></a>
## 작동 방식

검색은 최대 세 개의 순위 목록을 상호 순위 융합(RRF)으로 합칩니다. 항상 켜져 있는 **FTS5 BM25** 키워드 매칭, 임베딩 모델이 로드되어 있을 때 sqlite-vec를 통한 **시맨틱 벡터 검색**(기본값 `Qdrant/multilingual-e5-large-onnx`, 1024차원, 100개 이상 언어), 그리고 쿼리가 기간을 지정할 때("last week", "in March 2024")의 **날짜 창**입니다. 프로젝트, 토픽, 키워드 필터는 잘라내기 전에 적용됩니다. 메모리는 중요도에 따라 시간이 지나며 감쇠하고(`critical`은 감쇠하지 않음), 임베딩 모델이 로드되어 있으면 같은 토픽의 기존 메모리와 거의 같은 새 메모리(코사인 유사도 0.95 초과)는 기존 메모리에 병합됩니다. 또한 저장된 벡터를 만든 모델이 데이터베이스에 기록되므로, 설정에서 `model`을 바꿔도 벡터가 지워지지 않습니다(전환하는 명시적인 방법은 `icm embed --migrate`입니다).

모든 것은 외부 서비스 없이 SQLite 파일 하나에 저장됩니다:

```
~/Library/Application Support/dev.icm.icm/memories.db     # macOS
~/.local/share/icm/memories.db                            # Linux
%APPDATA%\icm\icm\data\memories.db                        # Windows
<project-root>/.icm/memories.db                           # icm init --per-project
```

`icm config`는 현재 적용된 설정을 보여 주며, [config/default.toml](config/default.toml)에 모든 옵션이 나열되어 있습니다. 자세한 내용: [레퍼런스](docs/reference.md#how-it-works).

<a id="documentation"></a>
## 문서

| 문서 | 설명 |
|----------|-------------|
| [통합 가이드](docs/integrations.md) | 도구별 MCP 설정: Claude Code, Cursor, Windsurf, Zed, Amp, Codex, Cline, Roo Code 등 |
| [기술 아키텍처](docs/architecture.md) | 크레이트 구조, 검색 파이프라인, 감쇠 모델, sqlite-vec 통합, 테스트 |
| [사용자 가이드](docs/guide.md) | 설치, 토픽 구성, 통합 정리(consolidation), 추출, 문제 해결 |
| [제품 개요](docs/product.md) | 사용 사례, 벤치마크, 대안과의 비교 |
| [레퍼런스](docs/reference.md) | 설치 옵션, 도구별 설정, CLI, 31개 MCP 도구, HTTP API, 대시보드, 내부 구조 |
| [벤치마크 어댑터](bench/amb/README.md) | 위 비교를 어떻게 실행했는지와 재현 방법 |
| [데모](docs/demonstrations.md) | 저장소 마이크로벤치마크와 작은 데모 |

<a id="license"></a>
## 라이선스

[Apache-2.0](LICENSE)
