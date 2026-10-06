[English](README.md) | [Français](README_fr.md) | [Español](README_es.md) | [Deutsch](README_de.md) | [Italiano](README_it.md) | [Português](README_pt.md) | [Nederlands](README_nl.md) | [Polski](README_pl.md) | [Русский](README_ru.md) | [日本語](README_ja.md) | [中文](README_zh.md) | [العربية](README_ar.md) | [한국어](README_ko.md)

To jest tłumaczenie pliku README.md (po angielsku), który jest wersją wzorcową; jeśli się różnią, rację ma wersja angielska.

<h1 align="center">ICM</h1>

<p align="center">
  <b>Pamięć długoterminowa dla agentów AI do programowania, współdzielona między Twoimi narzędziami.</b><br>
  Jeden plik binarny, jeden plik SQLite. Zapisanie ani przywołanie wspomnienia nie wymaga wywołania LLM.
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/icm/actions/workflows/ci.yml"><img src="https://github.com/rtk-ai/icm/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/icm/releases/latest"><img src="https://img.shields.io/github/v/release/rtk-ai/icm?color=purple" alt="Wydanie"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-Apache--2.0-blue.svg" alt="Apache-2.0"></a>
</p>

Powiedz w poniedziałek Claude Code, jak Twój projekt obsługuje uwierzytelnianie, a wtorkowa sesja Gemini CLI już to wie. ICM przechowuje to, czego uczą się Twoi agenci programistyczni (decyzje, poprawki, konwencje, preferencje), w jednym pliku SQLite na Twoim komputerze i oddaje odpowiednią część na początku każdej sesji oraz z każdym wysłanym promptem. Tę pamięć współdzieli do 18 agentów i edytorów, więc nie musisz już ponownie objaśniać projektu za każdym razem, gdy otwierasz sesję lub zmieniasz narzędzie.

- **92.8% na LoCoMo (1,540 pytań), na poziomie Hindsight (92.0%)**, przy kontekście mniejszym o jedną trzecią na pytanie. [Szczegóły i zastrzeżenia poniżej](#benchmark-comparison).
- **Bez wywołania LLM przy zapisie i przywołaniu.** Hindsight, Mem0, Graphiti (Zep) i claude-mem domyślnie wywołują LLM dla każdego zapisywanego wspomnienia. ICM tego nie robi. Jedynie automatyczne wydobywanie faktów z wyników narzędzi przechodzi przez narzędzie wiersza poleceń LLM, którego już używasz, jeśli jest zainstalowane; `provider = "none"` pozostawia również to lokalnie (zob. [Szybki start](#quickstart)).
- **Przydatny bez modelu embeddingów.** Samo przywoływanie po słowach kluczowych umieszcza co najmniej jedną z właściwych sesji wśród 5 najwyższych wyników dla 88.6% pytań LoCoMo, z medianą 6.8 ms na przywołanie na Linux x86-64.
- **Nie wszędzie na prowadzeniu.** Na PersonaMem (zmieniające się preferencje użytkownika) prowadzi Hindsight: 86.6% wobec 82.9% dla ICM.

<p align="center">
  <img src="assets/demo.svg" alt="Terminal: trzy wspomnienia zapisane przez icm store, a potem dwa pytania, na które odpowiada icm recall, za każdym razem zwracając właściwe wspomnienie">
</p>

<a id="quickstart"></a>
## Szybki start

```bash
brew tap rtk-ai/tap && brew install icm    # or: curl -fsSL https://raw.githubusercontent.com/rtk-ai/icm/main/install.sh | sh
icm init                                   # instructions, skills and hooks for every agent it detects
```

To cała konfiguracja. Otwórz nową sesję w Claude Code, Codex, Gemini CLI lub Copilot CLI: agent zaczyna teraz od krótkiego pakietu swoich najważniejszych wspomnień i otrzymuje wspomnienia związane z każdym wysłanym przez Ciebie promptem. To, czego uczy się z wyników swoich narzędzi, trafia do kolejki, a kolejka jest zamieniana na wspomnienia na końcu każdej sesji Claude Code; w przypadku pozostałych narzędzi uruchom `icm extract-pending` (na przykład z zadania cron).

Zapisywanie i przywoływanie odbywa się na Twoim komputerze. Automatyczne wydobywanie przekazuje tekst do narzędzia wiersza poleceń LLM, którego już używasz (Claude Code, Codex lub Gemini CLI), jeśli jest zainstalowane; ustaw `provider = "none"` w sekcji `[extraction.summarizer]`, aby wszystko działało w pełni lokalnie.

Aby od razu zobaczyć, jak to działa, zapisz i przywołaj wspomnienie ręcznie:

```console
$ icm store -t decisions-myapp -c "Auth uses short-lived JWTs, refreshed through /auth/refresh" -i high
Stored: 01M47YY6CHVZ48BYKQRCRNZTCF

$ icm recall "how does auth work"
memories[1]{id,topic,importance,weight,summary}:
  01M47YY6CHVZ48BYKQRCRNZTCF,decisions-myapp,high,0.975,"Auth uses short-lived JWTs, refreshed through /auth/refresh"
```

Pierwszy zapis lub pierwsze przywołanie z wyszukiwaniem semantycznym jednorazowo pobiera wielojęzyczny model embeddingów (`Qdrant/multilingual-e5-large-onnx`, około 2 GB). Aby wypróbować ICM bez niego, dodaj `--no-embeddings` (przywoływanie po słowach kluczowych, jak w wyniku powyżej) lub wybierz lżejszy model w konfiguracji. `icm init` zapisuje hooki i instrukcje w konfiguracji każdego wykrytego agenta; `icm uninstall --dry-run` pokazuje, jak je usunąć. Windows, Linux, Nix i budowanie ze źródeł: [Instalacja](#install).

<a id="benchmark-comparison"></a>
## Porównanie w benchmarkach

Dokładność odpowiedzi na [LoCoMo](https://github.com/snap-research/locomo) (10 długich rozmów, 1,540 pytań), mierzona publicznym zestawem testowym [Agent Memory Benchmark](https://github.com/vectorize-io/agent-memory-benchmark): system pamięci pobiera kontekst, `gemini-3.1-pro-preview` odpowiada na jego podstawie, `gemini-2.5-flash-lite` ocenia odpowiedź.

| System | Dokładność LoCoMo | Kontekst na pytanie | Wywołania LLM przy zapisie wspomnienia | Działa jako | Wynik |
|--------|:---------------:|:--------------------:|:---------------------------:|---------|--------|
| **ICM** 0.10.65 (silnik przywoływania v2) | **92.8%** (1,429 / 1,540) | 24.1k tokens | brak | jeden plik binarny Rust, plik SQLite | nasz przebieg, 2026-10-05 |
| Hindsight | 92.0% (1,417 / 1,540) | 36.2k tokens | wydobywanie faktów przez LLM | usługa w Pythonie, PostgreSQL + pgvector | opublikowany przez zestaw testowy |
| Bazowe wyszukiwanie hybrydowe (gęste + rzadkie, RRF) | 79.1% (1,218 / 1,540) | 22.2k tokens | brak | Qdrant | opublikowany przez zestaw testowy |

Na [PersonaMem](https://arxiv.org/abs/2504.14225) 32k (589 pytań wielokrotnego wyboru o zmieniających się preferencjach użytkownika, ten sam zestaw testowy i ten sam model odpowiadający, ocena według zgodności litery):

| System | Dokładność PersonaMem | Kontekst na pytanie | Wynik |
|--------|:-------------------:|:--------------------:|--------|
| **ICM** 0.10.65 (silnik przywoływania v2) | **82.9%** (488 / 589) | 16.2k tokens | nasz przebieg, 2026-10-05 |
| Hindsight | 86.6% (510 / 589) | 15.8k tokens | opublikowany przez zestaw testowy |
| Bazowe wyszukiwanie hybrydowe | 84.4% (497 / 589) | 24.2k tokens | opublikowany przez zestaw testowy |

Samo wyszukiwanie, na LoCoMo, bez modelu odpowiadającego: odsetek pytań, dla których co najmniej jedna sesja wzorcowa (gold) znajduje się wśród najwyższych wyników. To jest dowód dotyczący samego silnika przywoływania. Przebiegi v2 otrzymały datę każdej sesji i datę pytania; poprzedni silnik nie przyjmuje daty na wejściu, więc jego przebiegi nie otrzymały żadnej.

| Najwyższe wyniki | Poprzedni silnik (`legacy`) | Silnik przywoływania v2 | Poprzedni silnik, bez modelu embeddingów | Silnik przywoływania v2, bez modelu embeddingów |
|:-----------:|:--------------:|:----------------:|:--------------:|:----------------:|
| 5 | 76.5% | **86.7%** | 12.0% | **88.6%** |
| 10 | 83.0% | **93.3%** | 17.4% | **94.2%** |
| 20 | 87.7% | **97.9%** | 29.8% | **97.5%** |

Co te liczby pokazują, a czego nie:

- **ICM i Hindsight remisują na LoCoMo.** Różnica 0.8 punktu to 12 pytań, w granicach błędu próbkowania (przedział 95% dla ICM: od 91.5 do 94.1). ICM osiąga to przy kontekście mniejszym o jedną trzecią i bez wywoływania LLM przy zapisie wspomnienia.
- **Na PersonaMem prowadzi Hindsight**, o 3.7 punktu; ICM jest na poziomie bazowego wyszukiwania hybrydowego (przedział 95% dla ICM: od 79.8 do 85.9), czytając o jedną trzecią mniej kontekstu niż ono. Najsłabsza kategoria ICM w tym zbiorze to proponowanie nowych pomysłów na podstawie znanych preferencji (59.1%).
- **Warunki nie są identyczne.** Zestaw testowy utrzymuje Vectorize, dostawca Hindsight. Opublikowane wyniki LoCoMo pochodzą sprzed zmiany, która ustawiła temperaturę modelu odpowiadającego i oceniającego na 0; nasz przebieg używa obecnej wersji zestawu (commit `f618ed7`) i Vertex AI.
- **Przy 50 fragmentach zwracana jest duża część każdej rozmowy,** więc ta dokładność mierzy także model odpowiadający. Dowodem dotyczącym samego silnika przywoływania jest tabela wyszukiwania.
- **Na razie jeden przebieg na zbiór danych.** Powyższe przedziały 95% obejmują losowanie pytań, a nie zmienność między kolejnymi przebiegami.

<details>
<summary>Wyniki według kategorii, opóźnienia i dalsze zastrzeżenia</summary>

- **Według typu pytania** (LoCoMo, etykiety zestawu testowego): open-domain 96.7% (813 / 841), temporal 91.0% (292 / 321), single-hop 89.0% (251 / 282), multi-hop 76.0% (73 / 96).
- **Opóźnienie.** Mediana opóźnienia przywołania wyniosła 141 ms na chmurowej maszynie wirtualnej z 4 vCPU podczas powyższego przebiegu; 272 sesje wczytano bez żadnego wywołania LLM. W przebiegach samego wyszukiwania na Linux x86-64: mediana 136 ms z embeddingami, 6.8 ms tylko ze słowami kluczowymi.
- **Daty sesji.** Adapter benchmarku zapisuje datę każdej sesji w tekście wspomnienia ICM; Hindsight otrzymuje te same daty jako metadane, a każdy system dostaje datę pytania.
- **Najsłabsza kategoria to 96 pytań, które zestaw testowy oznacza jako multi-hop** (76.0%). Nazwy kategorii nie pokrywają się między benchmarkami: inne ewaluacje LoCoMo nazywają tę kategorię open-domain, a jako multi-hop określają 282 pytania, które zestaw testowy oznacza jako single-hop (tutaj 89.0%). Porównuj według liczby pytań, nie według etykiety.
- **Silnik przywoływania v2 jest domyślny** dla `icm recall`, narzędzia MCP `icm_memory_recall`, HTTP `/recall` i hooka promptu. Poprzedni silnik pozostaje dostępny, aby do niego wrócić lub porównać wyniki: `icm recall --engine legacy`, `"engine": "legacy"` w HTTP `/recall` albo `ICM_RECALL_ENGINE=legacy`.

</details>

Wyniki dla poszczególnych pytań z obu zbiorów danych są w [`bench/amb/results/`](bench/amb/results/); adapter, dokładne ustawienia i polecenia do odtworzenia wyników są w [`bench/amb/README.md`](bench/amb/README.md).

<a id="one-memory-for-every-tool"></a>
## Jedna pamięć dla wszystkich narzędzi

Każde narzędzie skonfigurowane przez `icm init` odczytuje i zapisuje tę samą bazę danych SQLite, a tematy (`decisions-myapp`, `preferences`, `errors-resolved`, ...) nie są podzielone według narzędzi. Wspomnienie zapisane z Claude Code jest od razu widoczne dla Codex, Gemini, Cursor, Roo, Amp, Aider, ...

Wolisz izolację? `icm init --per-project` tworzy lokalną bazę danych projektu w `.icm/` (i zapisuje pliki instrukcji agentów, takie jak `CLAUDE.md` i `AGENTS.md`, w bieżącym katalogu); `--db <path>` lub `ICM_DB` wskazują dowolny inny plik. Każda ścieżka to niezależny korpus.

> **Status projektu: beta.** ICM jest przed wersją 1.0: zmiany niezgodne wstecz mogą pojawić się w dowolnym wydaniu minor, a formaty konfiguracji hooków i MCP mogą się zmieniać. Beta dotyczy stabilności API, a nie codziennej przydatności: ja (opiekun projektu) używam ICM codziennie jako głównej pamięci przy programowaniu z AI. Skupiam się głównie na [rtk](https://github.com/rtk-ai/rtk), więc zgłoszenia i pull requesty są przeglądane w miarę możliwości.
>
> Apache-2.0, dostarczany **w stanie, w jakim jest („as-is”), bez jakiejkolwiek gwarancji** (zob. [LICENSE](LICENSE)). Przed każdą operacją destrukcyjną najpierw uruchom jej odpowiednik tylko do odczytu (`icm uninstall --dry-run`, `icm uninstall --check`).

<a id="install"></a>
## Instalacja

```bash
# macOS / Linux, Homebrew
brew tap rtk-ai/tap && brew install icm

# macOS / Linux, script (verifies SHA256 against the release checksums)
curl -fsSL https://raw.githubusercontent.com/rtk-ai/icm/main/install.sh | sh

# Windows, PowerShell
irm https://raw.githubusercontent.com/rtk-ai/icm/main/install.ps1 | iex
```

Wyszukiwanie po słowach kluczowych działa wszędzie. Uruchom `icm embeddings status`, aby sprawdzić, czy wyszukiwanie semantyczne jest włączone: jest wbudowane w kompilacje dla macOS Apple Silicon, Windows i `.rpm`; archiwa Linux glibc i pakiet `.deb` wymagają jednorazowego `icm embeddings download`; kompilacja dla Maców z procesorem Intel wymaga własnego ONNX Runtime (`ORT_DYLIB_PATH`); statyczna kompilacja Linux musl obsługuje tylko słowa kluczowe. Nix, budowanie ze źródeł, przypinanie wersji i szczegóły: [dokumentacja referencyjna](docs/reference.md#install).

<a id="setup"></a>
## Konfiguracja

```bash
icm init                  # global database, every detected agent
icm init --per-project    # database under .icm/ at the git root
icm init --mode all       # also register the MCP server in every tool that supports it
```

Tryb domyślny (`standard`) zapisuje instrukcje, umiejętności (skills) i hooki, bez serwera MCP. `--mode all` dodaje serwer MCP; z nim (oraz z `--per-project` dla Aider, którego plik konwencji jest osobny dla każdego projektu) obejmuje to 18 narzędzi poniżej ([przewodnik integracji](docs/integrations.md)):

| Narzędzie | Serwer MCP | Hooki |
|------|:---:|:-----:|
| Claude Code | tak | tak |
| Claude Desktop | tak | — |
| Gemini CLI | tak | tak |
| Codex CLI | tak | tak |
| Copilot CLI | tak | tak |
| Cursor | tak | — |
| Windsurf | tak | — |
| VS Code | tak | — |
| Amp | tak | — |
| Amazon Q | tak | — |
| Cline | tak | — |
| Roo Code | tak | — |
| Kilo Code | tak | — |
| Zed | tak | — |
| OpenCode | tak | tak |
| Continue.dev | tak | — |
| Aider | — | — |
| Pi | — | — |

Możesz też zarejestrować serwer MCP ręcznie: `claude mcp add icm -- icm serve` (dowolny klient MCP: polecenie `icm`, argumenty `["serve"]`).

Co robią hooki:

| Hook | Co robi |
|------|-------------|
| `icm hook start` | Wstrzykuje na początku sesji pakiet startowy wspomnień critical/high (~500 tokenów) |
| `icm hook pre` | Automatycznie zezwala na polecenia CLI `icm` (bez pytania o uprawnienia) |
| `icm hook post` | Wydobywa fakty z wyników narzędzi co N wywołań (automatyczne wydobywanie) |
| `icm hook compact` | Wydobywa wspomnienia z transkryptu przed kompresją kontekstu |
| `icm hook prompt` | Wstrzykuje przywołany kontekst na początku każdego promptu użytkownika |

Tabele hooków dla poszczególnych narzędzi, umiejętności, pliki instrukcji i uwaga dotycząca Codex: [dokumentacja referencyjna](docs/reference.md#setup).

<a id="use"></a>
## Użycie

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

ICM przechowuje też **memoirs** (trwałe grafy wiedzy złożone z pojęć i typowanych relacji), **feedback** (poprawki, z których można się uczyć) i **dosłowne transkrypty**, udostępnia **31 narzędzi MCP** (30 bez modelu embeddingów), **HTTP API**, które utrzymuje model embeddingów załadowany, oraz **panel w terminalu** (`icm dashboard`). Wszystko to opisuje [dokumentacja referencyjna](docs/reference.md).

<a id="how-it-works"></a>
## Jak to działa

Przywoływanie łączy do trzech rankingów metodą reciprocal rank fusion (RRF): dopasowanie słów kluczowych **FTS5 BM25**, zawsze włączone; **semantyczne wyszukiwanie wektorowe** przez sqlite-vec, gdy załadowany jest model embeddingów (domyślnie `Qdrant/multilingual-e5-large-onnx`, 1024 wymiary, 100+ języków); oraz **okno dat**, gdy zapytanie wskazuje okres ("last week", "in March 2024"). Filtry projektu, tematu i słów kluczowych są stosowane przed odcięciem. Wspomnienia z czasem tracą wagę zgodnie ze swoją ważnością (`critical` nigdy nie zanika); gdy załadowany jest model embeddingów, nowe wspomnienie niemal identyczne z istniejącym w tym samym temacie (podobieństwo cosinusowe powyżej 0.95) zostaje z nim scalone; a model, który wygenerował zapisane wektory, jest zapisywany w bazie danych, więc zmiana `model` w konfiguracji nigdy ich nie usuwa (`icm embed --migrate` to jawny sposób przejścia na inny model).

Wszystko znajduje się w jednym pliku SQLite, bez zewnętrznej usługi:

```
~/Library/Application Support/dev.icm.icm/memories.db     # macOS
~/.local/share/icm/memories.db                            # Linux
%APPDATA%\icm\icm\data\memories.db                        # Windows
<project-root>/.icm/memories.db                           # icm init --per-project
```

`icm config` pokazuje aktywną konfigurację; [config/default.toml](config/default.toml) zawiera listę wszystkich opcji. Szczegóły: [dokumentacja referencyjna](docs/reference.md#how-it-works).

<a id="documentation"></a>
## Dokumentacja

| Dokument | Opis |
|----------|-------------|
| [Przewodnik integracji](docs/integrations.md) | Konfiguracja MCP dla poszczególnych narzędzi: Claude Code, Cursor, Windsurf, Zed, Amp, Codex, Cline, Roo Code itd. |
| [Architektura techniczna](docs/architecture.md) | Struktura crate'ów, potok wyszukiwania, model zaniku, integracja sqlite-vec, testy |
| [Przewodnik użytkownika](docs/guide.md) | Instalacja, organizacja tematów, konsolidacja, wydobywanie, rozwiązywanie problemów |
| [Przegląd produktu](docs/product.md) | Przypadki użycia, benchmarki, porównanie z alternatywami |
| [Dokumentacja referencyjna](docs/reference.md) | Opcje instalacji, konfiguracja dla poszczególnych narzędzi, CLI, 31 narzędzi MCP, HTTP API, panel, mechanizmy wewnętrzne |
| [Adapter benchmarku](bench/amb/README.md) | Jak przeprowadzono powyższe porównanie i jak je odtworzyć |
| [Demonstracje](docs/demonstrations.md) | Mikrobenchmarki przechowywania i małe demonstracje |

<a id="license"></a>
## Licencja

[Apache-2.0](LICENSE)
