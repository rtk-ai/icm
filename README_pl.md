[English](README.md) | [Français](README_fr.md) | [Español](README_es.md) | [Deutsch](README_de.md) | [Italiano](README_it.md) | [Português](README_pt.md) | [Nederlands](README_nl.md) | **Polski** | [Русский](README_ru.md) | [日本語](README_ja.md) | [中文](README_zh.md) | [العربية](README_ar.md) | [한국어](README_ko.md)

<p align="center">
  <img src="assets/banner.png" alt="ICM — Infinite Context Memory" width="600">
</p>

<h1 align="center">ICM</h1>

<p align="center">
  Trwała pamięć dla agentów AI. Pojedynczy plik binarny, zero zależności, natywna obsługa MCP.
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/icm/actions/workflows/ci.yml"><img src="https://github.com/rtk-ai/icm/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/icm/releases/latest"><img src="https://img.shields.io/github/v/release/rtk-ai/icm?color=purple" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-Source--Available-orange.svg" alt="Source-Available"></a>
</p>

---

ICM daje Twojemu agentowi AI prawdziwą pamięć — nie narzędzie do notatek, nie menedżer kontekstu, lecz **pamięć**.

```
                       ICM (Infinite Context Memory)
            ┌──────────────────────┬─────────────────────────┐
            │   MEMORIES (Topics)  │   MEMOIRS (Knowledge)   │
            │                      │                         │
            │  Epizodyczna, czaso. │  Trwała, ustrukturyz.   │
            │                      │                         │
            │  ┌───┐ ┌───┐ ┌───┐  │    ┌───┐               │
            │  │ m │ │ m │ │ m │  │    │ C │──depends_on──┐ │
            │  └─┬─┘ └─┬─┘ └─┬─┘  │    └───┘              │ │
            │    │decay │     │    │      │ refines      ┌─▼─┐│
            │    ▼      ▼     ▼    │    ┌─▼─┐            │ C ││
            │  waga maleje         │    │ C │──part_of──>└───┘│
            │  z czasem, chyba że  │    └───┘                 │
            │  jest dostępna/kryt. │  Koncepcje + Relacje     │
            ├──────────────────────┴─────────────────────────┤
            │             SQLite + FTS5 + sqlite-vec          │
            │        Wyszukiwanie hybrydowe: BM25 (30%) + cosine (70%) │
            └─────────────────────────────────────────────────┘
```

**Dwa modele pamięci:**

- **Memories** — przechowywanie/odwoływanie z temporalnym zanikaniem według ważności. Krytyczne wspomnienia nigdy nie zanikają, te o niskim priorytecie zanikają naturalnie. Filtrowanie według tematu lub słowa kluczowego.
- **Memoirs** — trwałe grafy wiedzy. Koncepcje powiązane typowanymi relacjami (`depends_on`, `contradicts`, `superseded_by`, ...). Filtrowanie według etykiety.
- **Feedback** — rejestrowanie korekt, gdy przewidywania AI są błędne. Przeszukiwanie przeszłych błędów przed dokonywaniem nowych przewidywań. Nauka w zamkniętej pętli.

## Instalacja

```bash
# Homebrew (macOS / Linux)
brew tap rtk-ai/tap && brew install icm

# Szybka instalacja
curl -fsSL https://raw.githubusercontent.com/rtk-ai/icm/main/install.sh | sh

# Ze źródeł
cargo install --path crates/icm-cli
```

## Konfiguracja

```bash
# Automatyczne wykrywanie i konfiguracja wszystkich obsługiwanych narzędzi
icm init
```

Konfiguruje **17 narzędzi** jednym poleceniem ([pełny przewodnik integracji](docs/integrations.md)):

| Narzędzie | MCP | Hooki | CLI | Umiejętności |
|-----------|:---:|:-----:|:---:|:------------:|
| Claude Code | `~/.claude.json` | 5 hooków | `CLAUDE.md` | `/recall` `/remember` |
| Claude Desktop | JSON | — | — | — |
| Gemini CLI | `~/.gemini/settings.json` | 5 hooków | `GEMINI.md` | — |
| Codex CLI | `~/.codex/config.toml` | 4 hooki | `AGENTS.md` | — |
| Copilot CLI | `~/.copilot/mcp-config.json` | 4 hooki | `.github/copilot-instructions.md` | — |
| Cursor | `~/.cursor/mcp.json` | — | — | reguła `.mdc` |
| Windsurf | JSON | — | `.windsurfrules` | — |
| VS Code | `~/Library/.../Code/User/mcp.json` | — | — | — |
| Amp | JSON | — | — | `/icm-recall` `/icm-remember` |
| Amazon Q | JSON | — | — | — |
| Cline | VS Code globalStorage | — | — | — |
| Roo Code | VS Code globalStorage | — | — | reguła `.md` |
| Kilo Code | VS Code globalStorage | — | — | — |
| Zed | `~/.zed/settings.json` | — | — | — |
| OpenCode | JSON | wtyczka TS | — | — |
| Continue.dev | `~/.continue/config.yaml` | — | — | — |
| Aider | — | — | `.aider.conventions.md` | — |

Lub ręcznie:

```bash
# Claude Code
claude mcp add icm -- icm serve

# Tryb kompaktowy (krótsze odpowiedzi, oszczędność tokenów)
claude mcp add icm -- icm serve --compact

# Dowolny klient MCP: command = "icm", args = ["serve"]
```

### Umiejętności / reguły

```bash
icm init --mode skill
```

Instaluje polecenia slash i reguły dla Claude Code (`/recall`, `/remember`), Cursor (reguła `.mdc`), Roo Code (reguła `.md`) oraz Amp (`/icm-recall`, `/icm-remember`).

### Hooki (5 narzędzi)

```bash
icm init --mode hook
```

Instaluje hooki automatycznej ekstrakcji i automatycznego przywoływania dla wszystkich obsługiwanych narzędzi:

| Narzędzie | SessionStart | PreTool | PostTool | Compact | PromptRecall | Konfiguracja |
|-----------|:-----------:|:-------:|:--------:|:-------:|:------------:|--------|
| Claude Code | `icm hook start` | `icm hook pre` | `icm hook post` | `icm hook compact` | `icm hook prompt` | `~/.claude/settings.json` |
| Gemini CLI | `icm hook start` | `icm hook pre` | `icm hook post` | `icm hook compact` | `icm hook prompt` | `~/.gemini/settings.json` |
| Codex CLI | `icm hook start` | `icm hook pre` | `icm hook post` | — | `icm hook prompt` | `~/.codex/hooks.json` |
| Copilot CLI | `icm hook start` | `icm hook pre` | `icm hook post` | — | `icm hook prompt` | `.github/hooks/icm.json` |
| OpenCode | start sesji | — | ekstrakcja narzędzi | kompakcja | — | `~/.config/opencode/plugins/icm.ts` |

**Co robi każdy hook:**

| Hook | Co robi |
|------|---------|
| `icm hook start` | Wstrzykuje pakiet startowy krytycznych/ważnych wspomnień na początku sesji (~500 tokenów) |
| `icm hook pre` | Automatyczne zezwalanie na polecenia CLI `icm` (bez monitu o uprawnienia) |
| `icm hook post` | Ekstrakcja faktów z wyjścia narzędzia co N wywołań (automatyczna ekstrakcja) |
| `icm hook compact` | Ekstrakcja wspomnień z transkryptu przed kompresją kontekstu |
| `icm hook prompt` | Wstrzykiwanie przypomnianego kontekstu na początku każdego monitu użytkownika |

## CLI vs MCP

ICM może być używany przez CLI (polecenia `icm`) lub serwer MCP (`icm serve`). Oba sposoby korzystają z tej samej bazy danych.

| | CLI | MCP |
|---|-----|-----|
| **Opóźnienie** | ~30ms (bezpośredni plik binarny) | ~50ms (JSON-RPC stdio) |
| **Koszt tokenów** | 0 (oparte na hookach, niewidoczne) | ~20-50 tokenów/wywołanie (schemat narzędzia) |
| **Konfiguracja** | `icm init --mode hook` | `icm init --mode mcp` |
| **Współpracuje z** | Claude Code, Gemini, Codex, Copilot, OpenCode (przez hooki) | Wszystkie 17 narzędzi kompatybilnych z MCP |
| **Automatyczna ekstrakcja** | Tak (hooki wywołują `icm extract`) | Tak (narzędzia MCP wywołują store) |
| **Najlepsze dla** | Zaawansowanych użytkowników, oszczędności tokenów | Uniwersalna kompatybilność |

## CLI

### Memories (epizodyczne, z zanikaniem)

```bash
# Przechowywanie
icm store -t "mój-projekt" -c "Użyj PostgreSQL jako głównej bazy danych" -i high -k "db,postgres"

# Przywoływanie
icm recall "wybór bazy danych"
icm recall "konfiguracja uwierzytelniania" --topic "mój-projekt" --limit 10
icm recall "architektura" --keyword "postgres"

# Zarządzanie
icm forget <memory-id>
icm consolidate --topic "mój-projekt"
icm topics
icm stats

# Ekstrakcja faktów z tekstu (regułowa, zero kosztów LLM)
echo "Parser używa algorytmu Pratt" | icm extract -p mój-projekt
```

### Memoirs (trwałe grafy wiedzy)

```bash
# Tworzenie wspomnienia
icm memoir create -n "architektura-systemu" -d "Decyzje dotyczące projektu systemu"

# Dodawanie koncepcji z etykietami
icm memoir add-concept -m "architektura-systemu" -n "usługa-uwierzytelniania" \
  -d "Obsługuje tokeny JWT i przepływy OAuth2" -l "domain:auth,type:service"

# Łączenie koncepcji
icm memoir link -m "architektura-systemu" --from "brama-api" --to "usługa-uwierzytelniania" -r depends-on

# Wyszukiwanie z filtrem etykiet
icm memoir search -m "architektura-systemu" "uwierzytelnianie"
icm memoir search -m "architektura-systemu" "usługa" --label "domain:auth"

# Inspekcja sąsiedztwa
icm memoir inspect -m "architektura-systemu" "usługa-uwierzytelniania" -D 2

# Eksport grafu (formaty: json, dot, ascii, ai)
icm memoir export -m "architektura-systemu" -f ascii   # Ramki z paskami pewności
icm memoir export -m "architektura-systemu" -f dot      # Graphviz DOT (kolor = poziom pewności)
icm memoir export -m "architektura-systemu" -f ai       # Markdown zoptymalizowany dla kontekstu LLM
icm memoir export -m "architektura-systemu" -f json     # Strukturalny JSON ze wszystkimi metadanymi

# Generowanie wizualizacji SVG
icm memoir export -m "architektura-systemu" -f dot | dot -Tsvg > graph.svg
```

## Narzędzia MCP (31)

### Narzędzia pamięci

| Narzędzie | Opis |
|-----------|------|
| `icm_memory_store` | Przechowywanie z automatyczną deduplikacją (podobieństwo >85% → aktualizacja zamiast duplikatu) |
| `icm_memory_recall` | Wyszukiwanie według zapytania, filtrowanie według tematu i/lub słowa kluczowego |
| `icm_memory_update` | Edycja wspomnienia w miejscu (treść, ważność, słowa kluczowe) |
| `icm_memory_forget` | Usuwanie wspomnienia według ID |
| `icm_memory_consolidate` | Scalanie wszystkich wspomnień tematu w jedno podsumowanie |
| `icm_memory_list_topics` | Lista wszystkich tematów z liczbą wpisów |
| `icm_memory_stats` | Globalne statystyki pamięci |
| `icm_memory_health` | Audyt higieny według tematu (nieaktualność, potrzeba konsolidacji) |
| `icm_memory_embed_all` | Uzupełnianie osadzeń dla wyszukiwania wektorowego |

### Narzędzia Memoir (grafy wiedzy)

| Narzędzie | Opis |
|-----------|------|
| `icm_memoir_create` | Tworzenie nowego wspomnienia (kontener wiedzy) |
| `icm_memoir_list` | Lista wszystkich wspomnień |
| `icm_memoir_show` | Wyświetlanie szczegółów wspomnienia i wszystkich koncepcji |
| `icm_memoir_add_concept` | Dodawanie koncepcji z etykietami |
| `icm_memoir_refine` | Aktualizacja definicji koncepcji |
| `icm_memoir_search` | Wyszukiwanie pełnotekstowe, opcjonalnie filtrowane według etykiety |
| `icm_memoir_search_all` | Wyszukiwanie we wszystkich wspomnieniach |
| `icm_memoir_link` | Tworzenie typowanej relacji między koncepcjami |
| `icm_memoir_inspect` | Inspekcja koncepcji i sąsiedztwa grafu (BFS) |
| `icm_memoir_export` | Eksport grafu (json, dot, ascii, ai) z poziomami pewności |

### Narzędzia Feedback (nauka na błędach)

| Narzędzie | Opis |
|-----------|------|
| `icm_feedback_record` | Rejestrowanie korekty, gdy przewidywanie AI było błędne |
| `icm_feedback_search` | Wyszukiwanie przeszłych korekt w celu informowania przyszłych przewidywań |
| `icm_feedback_stats` | Statystyki informacji zwrotnych: łączna liczba, podział według tematu, najczęściej stosowane |

### Typy relacji

`part_of` · `depends_on` · `related_to` · `contradicts` · `refines` · `alternative_to` · `caused_by` · `instance_of` · `superseded_by`

## Jak to działa

### Dualny model pamięci

**Pamięć epizodyczna (Topics)** rejestruje decyzje, błędy, preferencje. Każde wspomnienie ma wagę, która zanika z czasem w zależności od ważności:

| Ważność | Zanikanie | Przycinanie | Zachowanie |
|---------|-----------|-------------|------------|
| `critical` | brak | nigdy | Nigdy nie zapomniane, nigdy nie przycinane |
| `high` | powolne (0,5x szybkości) | nigdy | Zanika powoli, nigdy nie usuwane automatycznie |
| `medium` | normalne | tak | Standardowe zanikanie, przycinane gdy waga < próg |
| `low` | szybkie (2x szybkości) | tak | Szybko zapomniane |

Zanikanie jest **świadome dostępu**: często przywoływane wspomnienia zanikają wolniej (`decay / (1 + access_count × 0.1)`). Stosowane automatycznie przy przywoływaniu (jeśli >24h od ostatniego zanikania).

**Higiena pamięci** jest wbudowana:
- **Automatyczna deduplikacja**: przechowywanie treści o podobieństwie >85% do istniejącego wspomnienia w tym samym temacie aktualizuje je zamiast tworzyć duplikat
- **Wskazówki konsolidacji**: gdy temat przekracza 7 wpisów, `icm_memory_store` ostrzega rozmówcę o potrzebie konsolidacji
- **Audyt zdrowia**: `icm_memory_health` raportuje liczbę wpisów na temat, średnią wagę, nieaktualne wpisy i potrzeby konsolidacji
- **Bez cichej utraty danych**: wspomnienia krytyczne i o wysokiej ważności nigdy nie są automatycznie przycinane

**Pamięć semantyczna (Memoirs)** rejestruje ustrukturyzowaną wiedzę jako graf. Koncepcje są trwałe — są udoskonalane, nigdy nie zanikają. Użyj `superseded_by` do oznaczania przestarzałych faktów zamiast ich usuwania.

### Wyszukiwanie hybrydowe

Przy włączonych osadzeniach ICM używa wyszukiwania hybrydowego:
- **FTS5 BM25** (30%) — dopasowywanie słów kluczowych pełnotekstowe
- **Podobieństwo kosinusowe** (70%) — semantyczne wyszukiwanie wektorowe przez sqlite-vec

Domyślny model: `intfloat/multilingual-e5-base` (768d, ponad 100 języków). Konfigurowalny w [pliku konfiguracyjnym](#konfiguracja):

```toml
[embeddings]
# enabled = false                          # Wyłącz całkowicie (bez pobierania modelu)
model = "intfloat/multilingual-e5-base"    # 768d, wielojęzyczny (domyślny)
# model = "intfloat/multilingual-e5-small" # 384d, wielojęzyczny (lżejszy)
# model = "intfloat/multilingual-e5-large" # 1024d, wielojęzyczny (najlepsza dokładność)
# model = "Xenova/bge-small-en-v1.5"      # 384d, tylko angielski (najszybszy)
# model = "jinaai/jina-embeddings-v2-base-code"  # 768d, zoptymalizowany pod kod
```

Aby całkowicie pominąć pobieranie modelu osadzenia, użyj jednego z poniższych:
```bash
icm --no-embeddings serve          # Flaga CLI
ICM_NO_EMBEDDINGS=1 icm serve     # Zmienna środowiskowa
```
Lub ustaw `enabled = false` w pliku konfiguracyjnym. ICM przełączy się na wyszukiwanie słów kluczowych FTS5 (nadal działa, tylko bez dopasowywania semantycznego).

Zmiana modelu automatycznie odtwarza indeks wektorowy (istniejące osadzenia są czyszczone i można je regenerować za pomocą `icm_memory_embed_all`).

### Przechowywanie

Pojedynczy plik SQLite. Brak zewnętrznych usług, brak zależności sieciowych.

```
~/Library/Application Support/dev.icm.icm/memories.db                    # macOS
~/.local/share/dev.icm.icm/memories.db                                   # Linux
C:\Users\<user>\AppData\Local\icm\icm\data\memories.db                   # Windows
```

### Konfiguracja

```bash
icm config                    # Wyświetl aktywną konfigurację
```

Lokalizacja pliku konfiguracyjnego (zależna od platformy lub `$ICM_CONFIG`):

```
~/Library/Application Support/dev.icm.icm/config.toml                    # macOS
~/.config/icm/config.toml                                                # Linux
C:\Users\<user>\AppData\Roaming\icm\icm\config\config.toml              # Windows
```

Zobacz [config/default.toml](config/default.toml) dla wszystkich opcji.

## Wieloprojektowość i wieloagentowość

ICM jest zaprojektowany pod kątem sytuacji, w której jeden użytkownik współpracuje z wieloma agentami w wielu projektach. Wspomnienia muszą pozostać trafne: decyzja z projektu A nigdy nie powinna przeniknąć do projektu B, a agent `dev` nie powinien być nawadniany tym, co zapisał agent `mentor`.

### Izolacja projektów

ICM rozdziela wspomnienia poprzez **konwencję nazewnictwa topiców**, a nie osobną kolumnę. Konwencja:

```
{kind}-{project}              # np. decisions-icm, errors-resolved-icm, contexte-rtk-cloud
preferences                   # globalne, zawsze dołączane
identity                      # globalne, zawsze dołączane
```

`icm_wake_up { project: "icm" }` wykonuje dopasowanie **świadome segmentów**: `"icm"` pasuje do `decisions-icm`, `errors-icm-core`, `contexte-icm` — ale nigdy do `icmp-notes` (brak fałszywych dopasowań). Topiki są dzielone po `-`, `.`, `_`, `/`, `:`. Topiki preferencji i tożsamości są z założenia międzyprojektowe — wskazówki na poziomie użytkownika nigdy nie są usuwane.

Hook `UserPromptSubmit` (`icm hook prompt`) oraz hook `SessionStart` (`icm hook start`) wyprowadzają projekt z pola `cwd` w JSON-ie hooka (`basename` katalogu roboczego). Uruchamiaj każdy projekt z jego własnego katalogu, a izolacja zadziała automatycznie.

### Pisanie dobrych wspomnień

`icm_memory_store` wymaga, by agent wybrał `topic` i `content` — nie ma automatycznego klasyfikatora. Najlepsze praktyki:

| Pole | Wskazówka |
|------|-----------|
| `topic` | `{kind}-{project}`. Rodzaje: `decisions`, `errors-resolved`, `contexte`, `preferences`. |
| `content` | Jeden fakt na zapis. Zwięzłe streszczenie po angielsku — `topic + content` to tekst osadzenia (embeddingu). |
| `raw_excerpt` | Wyłącznie cytat dosłowny (kod, dokładny komunikat błędu, wynik polecenia). |
| `keywords` | 3–5 terminów wzmacniających wyszukiwanie BM25. |
| `importance` | `critical` dla nigdy-nie-zapominaj, `high` dla decyzji projektowych, `medium` domyślnie, `low` dla efemerycznych. |

ICM zajmuje się resztą: **deduplikacją przy 85% podobieństwa**, **automatycznym łączeniem** semantycznie bliskich wspomnień, **automatyczną konsolidacją** powyżej 10 wpisów na topik oraz **decay** ważonym liczbą dostępów. Jeden fakt na wywołanie wygrywa z paczkowymi zrzutami — retriever wyżej rankuje fakty zapisane indywidualnie.

### Role wieloagentowe

ICM nie ma jeszcze pierwszoklasowej kolumny `role`. Obecnie role są emulowane przez przyrostki topiców plus dedykowane katalogi robocze dla każdego agenta:

```
decisions-icm-dev             # agent dev: wzorce kodu, wybory bibliotek, refaktoryzacje
decisions-icm-architect       # architekt: projekt, workflow, dekompozycja podzadań
decisions-icm-mentor          # mentor / BA: cele biznesowe, kontekst pozatechniczny
```

Każdy agent działa we własnym katalogu roboczym (`~/projects/icm-dev/`, `~/projects/icm-architect/`, ...), aby `icm hook prompt` i `icm hook start` wyprowadzały inny segment projektu z `cwd` i przywoływały tylko pasujące wspomnienia. Preferencje pozostają globalne — tożsamość użytkownika przenosi się przez wszystkie role.

W obrębie pojedynczego agenta możesz też ręcznie zawęzić recall:

```jsonc
// icm_memory_recall
{ "query": "auth flow", "topic": "decisions-icm-architect", "limit": 5 }
```

Pierwszoklasowe pole `role` (z natywnym filtrowaniem w wake-up i recall) jest na roadmapie. Do tego czasu konwencja przyrostka topiku jest wzorcem wspieranym.

## Automatyczna ekstrakcja

ICM automatycznie wyodrębnia wspomnienia przez trzy warstwy:

```
  Warstwa 0: Hooki wzorców      Warstwa 1: PreCompact         Warstwa 2: UserPromptSubmit
  (zero kosztów LLM)            (zero kosztów LLM)            (zero kosztów LLM)
  ┌──────────────────┐          ┌──────────────────┐          ┌──────────────────┐
  │ Hook PostToolUse  │          │ Hook PreCompact   │          │ UserPromptSubmit  │
  │                   │          │                   │          │                   │
  │ • Błędy Bash      │          │ Kontekst ma być   │          │ Użytkownik wysyła │
  │ • Commity git     │          │ skompresowany →   │          │ monit → icm recall│
  │ • Zmiany config   │          │ wyodrębniaj       │          │ → wstrzyknij kont.│
  │ • Decyzje         │          │ wspomnienia       │          │                   │
  │ • Preferencje     │          │ z transkryptu     │          │ Agent zaczyna z   │
  │ • Wnioski         │          │ zanim zostaną     │          │ załadowanymi      │
  │ • Ograniczenia    │          │ utracone na zawsze│          │ odpowiednimi      │
  │                   │          │                   │          │ wspomnieniami     │
  │ Regułowe, bez LLM │          │ Te same wzorce +  │          │                   │
  └──────────────────┘          │ --store-raw fallbk│          └──────────────────┘
                                 └──────────────────┘
```

| Warstwa | Status | Koszt LLM | Polecenie hooka | Opis |
|---------|--------|-----------|-----------------|------|
| Warstwa 0 | Zaimplementowana | 0 | `icm hook post` | Regułowa ekstrakcja słów kluczowych z wyjścia narzędzia |
| Warstwa 1 | Zaimplementowana | 0 | `icm hook compact` | Ekstrakcja z transkryptu przed kompresją kontekstu |
| Warstwa 2 | Zaimplementowana | 0 | `icm hook prompt` | Wstrzykiwanie przywoływanych wspomnień przy każdym monicie użytkownika |

Wszystkie 3 warstwy są instalowane automatycznie przez `icm init --mode hook`.

### Porównanie z alternatywami

| System | Metoda | Koszt LLM | Opóźnienie | Rejestruje kompakcję? |
|--------|--------|-----------|------------|----------------------|
| **ICM** | Ekstrakcja 3-warstwowa | 0 do ~500 tok/sesję | 0ms | **Tak (PreCompact)** |
| Mem0 | 2 wywołania LLM/wiadomość | ~2k tok/wiadomość | 200-2000ms | Nie |
| claude-mem | PostToolUse + async | ~1-5k tok/sesję | 8ms hook | Nie |
| MemGPT/Letta | Agent zarządza samodzielnie | 0 marginalnie | 0ms | Nie |
| DiffMem | Diffs oparte na Git | 0 | 0ms | Nie |

## Benchmarki

### Wydajność przechowywania

```
ICM Benchmark (1000 wspomnień, osadzenia 384d)
──────────────────────────────────────────────────────────
Store (bez osadzeń)        1000 ops      34,2 ms      34,2 µs/op
Store (z osadzeniami)      1000 ops      51,6 ms      51,6 µs/op
Wyszukiwanie FTS5           100 ops       4,7 ms      46,6 µs/op
Wyszukiwanie wektorowe (KNN) 100 ops     59,0 ms     590,0 µs/op
Wyszukiwanie hybrydowe      100 ops      95,1 ms     951,1 µs/op
Zanikanie (wsadowe)           1 ops       5,8 ms       5,8 ms/op
──────────────────────────────────────────────────────────
```

Apple M1 Pro, SQLite w pamięci, jednowątkowy. `icm bench --count 1000`

### Efektywność agenta

Wielosesyjny przepływ pracy z prawdziwym projektem Rust (12 plików, ~550 linii). Sesje 2+ pokazują największe zyski, gdy ICM przywołuje zamiast ponownie czytać pliki.

```
ICM Agent Benchmark (10 sesji, model: haiku, uśrednione 3 przebiegi)
══════════════════════════════════════════════════════════════════
                            Bez ICM          Z ICM       Delta
Sesja 2 (przywoływanie)
  Tury                              5,7              4,0       -29%
  Kontekst (wejście)              99,9k            67,5k       -32%
  Koszt                          $0,0298          $0,0249       -17%

Sesja 3 (przywoływanie)
  Tury                              3,3              2,0       -40%
  Kontekst (wejście)              74,7k            41,6k       -44%
  Koszt                          $0,0249          $0,0194       -22%
══════════════════════════════════════════════════════════════════
```

`icm bench-agent --sessions 10 --model haiku`

### Retencja wiedzy

Agent przywołuje konkretne fakty z gęstego dokumentu technicznego między sesjami. Sesja 1 czyta i zapamiętuje; sesje 2+ odpowiadają na 10 pytań faktycznych **bez** tekstu źródłowego.

```
ICM Recall Benchmark (10 pytań, model: haiku, uśrednione 5 przebiegów)
══════════════════════════════════════════════════════════════════════
                                               Bez ICM     Z ICM
──────────────────────────────────────────────────────────────────────
Średni wynik                                       5%         68%
Pytania zaliczone                                0/10        5/10
══════════════════════════════════════════════════════════════════════
```

`icm bench-recall --model haiku`

### Lokalne modele LLM (ollama)

Ten sam test z lokalnymi modelami — czyste wstrzykiwanie kontekstu, bez potrzeby użycia narzędzi.

```
Model               Params   Bez ICM   Z ICM     Delta
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

### Protokół testowy

Wszystkie benchmarki używają **prawdziwych wywołań API** — bez atrap, bez symulowanych odpowiedzi, bez buforowanych odpowiedzi.

- **Benchmark agenta**: Tworzy prawdziwy projekt Rust w katalogu tymczasowym. Uruchamia N sesji z `claude -p --output-format json`. Bez ICM: pusta konfiguracja MCP. Z ICM: prawdziwy serwer MCP + automatyczna ekstrakcja + wstrzykiwanie kontekstu.
- **Retencja wiedzy**: Używa fikcyjnego dokumentu technicznego ("Protokół Meridian"). Ocenia odpowiedzi przez dopasowywanie słów kluczowych do oczekiwanych faktów. Limit 120s na wywołanie.
- **Izolacja**: Każdy przebieg używa własnego katalogu tymczasowego i świeżej bazy danych SQLite. Brak trwałości sesji.

### Zunifikowana pamięć wielu agentów

Wszystkie 17 narzędzi współdzieli tę samą bazę danych SQLite. Wspomnienie zapisane przez Claude jest natychmiast dostępne dla Gemini, Codex, Copilot, Cursor i każdego innego narzędzia.

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

Wynik = 60% dokładność przywoływania + 30% szczegółowość faktów + 10% szybkość. **98% efektywności wielu agentów.**

## Dlaczego ICM

| Możliwość | ICM | Mem0 | Engram | AgentMemory |
|-----------|:---:|:----:|:------:|:-----------:|
| Obsługa narzędzi | **17** | tylko SDK | ~6-8 | ~10 |
| Konfiguracja jednym poleceniem | `icm init` | ręcznie SDK | ręcznie | ręcznie |
| Hooki (auto-przywoływanie na starcie) | 5 narzędzi | brak | przez MCP | 1 narzędzie |
| Wyszukiwanie hybrydowe (FTS5 + wektor) | 30/70 ważone | tylko wektor | tylko FTS5 | FTS5+wektor |
| Wielojęzyczne osadzenia | 100+ języków (768d) | zależne | brak | angielski 384d |
| Graf wiedzy | System Memoir | brak | brak | brak |
| Zanikanie temporalne + konsolidacja | świadome dostępu | brak | podstawowe | podstawowe |
| Dashboard TUI | `icm dashboard` | brak | tak | przeglądarka web |
| Automatyczna ekstrakcja z wyjścia narzędzia | 3 warstwy, zero LLM | brak | brak | brak |
| Pętla feedbacku/korekt | `icm_feedback_*` | brak | brak | brak |
| Runtime | Rust, pojedynczy plik binarny | Python | Go | Node.js |
| Lokalne, zero zależności | plik SQLite | cloud-first | SQLite | SQLite |
| Dokładność przywoływania wielu agentów | **98%** | N/A | N/A | 95,2% |

## Dokumentacja

| Dokument | Opis |
|----------|------|
| [Przewodnik integracji](docs/integrations.md) | Konfiguracja dla wszystkich 17 narzędzi: Claude Code, Copilot, Cursor, Windsurf, Zed, Amp, itp. |
| [Architektura techniczna](docs/architecture.md) | Struktura crate, potok wyszukiwania, model zanikania, integracja sqlite-vec, testowanie |
| [Przewodnik użytkownika](docs/guide.md) | Instalacja, organizacja tematów, konsolidacja, ekstrakcja, rozwiązywanie problemów |
| [Przegląd produktu](docs/product.md) | Przypadki użycia, benchmarki, porównanie z alternatywami |

## Licencja

[Source-Available](LICENSE) — Bezpłatna dla osób prywatnych i zespołów liczących ≤ 20 osób. Licencja korporacyjna wymagana dla większych organizacji. Kontakt: contact@rtk-ai.app
