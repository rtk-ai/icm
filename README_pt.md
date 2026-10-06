[English](README.md) | [Français](README_fr.md) | [Español](README_es.md) | [Deutsch](README_de.md) | [Italiano](README_it.md) | [Português](README_pt.md) | [Nederlands](README_nl.md) | [Polski](README_pl.md) | [Русский](README_ru.md) | [日本語](README_ja.md) | [中文](README_zh.md) | [العربية](README_ar.md) | [한국어](README_ko.md)

Esta é uma tradução de README.md (inglês), que é a referência; se houver diferenças, a versão em inglês é a correta.

<h1 align="center">ICM</h1>

<p align="center">
  <b>Memória de longo prazo para agentes de programação com IA, compartilhada entre suas ferramentas.</b><br>
  Um binário, um arquivo SQLite. Nenhuma chamada a um LLM para armazenar ou recuperar uma memória.
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/icm/actions/workflows/ci.yml"><img src="https://github.com/rtk-ai/icm/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/icm/releases/latest"><img src="https://img.shields.io/github/v/release/rtk-ai/icm?color=purple" alt="Versão"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-Apache--2.0-blue.svg" alt="Apache-2.0"></a>
</p>

Explique ao Claude Code na segunda-feira como seu projeto lida com autenticação, e a sessão do Gemini CLI de terça-feira já sabe disso. O ICM guarda o que seus agentes de programação aprendem (decisões, correções, convenções, preferências) em um único arquivo SQLite na sua máquina e devolve a parte relevante no início de cada sessão e a cada prompt que você envia. Até 18 agentes e editores compartilham essa memória, então você para de reexplicar seu projeto toda vez que abre uma sessão ou troca de ferramenta.

- **92.9% no LoCoMo (1,540 perguntas, média de três execuções), no mesmo nível do Hindsight (92.0%)**, com um terço a menos de contexto por pergunta. [Detalhes e ressalvas abaixo](#benchmark-comparison).
- **Nenhuma chamada a um LLM para armazenar ou recuperar.** Hindsight, Mem0, Graphiti (Zep) e claude-mem chamam, por padrão, um LLM para cada memória que armazenam. O ICM não. Apenas a extração automática de fatos a partir da saída das ferramentas passa pela ferramenta de linha de comando de LLM que você já usa, quando há uma instalada; `provider = "none"` mantém isso local também (veja [Início rápido](#quickstart)).
- **Útil sem modelo de embeddings.** Só a recuperação por palavras-chave coloca pelo menos uma das sessões certas no top 5 para 88.6% das perguntas do LoCoMo, com mediana de 6.8 ms por consulta no Linux x86-64.
- **97.4% no LongMemEval-S, recuperação sem LLM**: uma das sessões certas no top 5 para 487 de 500 perguntas, contra 96.6% do MemPalace e 95.2% do agentmemory na mesma medida.
- **Não está à frente em tudo.** No PersonaMem (as preferências de um usuário que mudam com o tempo), o Hindsight lidera: 86.6% contra 81.7% do ICM.

<p align="center">
  <img src="assets/demo.svg" alt="Terminal: três memórias armazenadas com icm store e depois duas perguntas respondidas por icm recall, cada uma retornando a memória certa">
</p>

<a id="quickstart"></a>

## Início rápido

```bash
brew tap rtk-ai/tap && brew install icm    # or: curl -fsSL https://raw.githubusercontent.com/rtk-ai/icm/main/install.sh | sh
icm init                                   # instructions, skills and hooks for every agent it detects
```

Essa é toda a configuração. Abra uma nova sessão no Claude Code, Codex, Gemini CLI ou Copilot CLI: seu agente agora começa com um pacote curto das suas memórias mais importantes e recebe as memórias relevantes para cada prompt que você envia. O que ele aprende com a saída das ferramentas vai para uma fila, e a fila é transformada em memórias no fim de cada sessão do Claude Code; com as outras ferramentas, execute `icm extract-pending` (a partir de um cron job, por exemplo).

Armazenar e recuperar ficam na sua máquina. A extração automática passa texto para a ferramenta de linha de comando de LLM que você já usa (Claude Code, Codex ou Gemini CLI) quando há uma instalada; defina `provider = "none"` em `[extraction.summarizer]` para mantê-la totalmente local.

Para ver funcionando agora mesmo, armazene e recupere à mão:

```console
$ icm store -t decisions-myapp -c "Auth uses short-lived JWTs, refreshed through /auth/refresh" -i high
Stored: 01M47YY6CHVZ48BYKQRCRNZTCF

$ icm recall "how does auth work"
memories[1]{id,topic,importance,weight,summary}:
  01M47YY6CHVZ48BYKQRCRNZTCF,decisions-myapp,high,0.975,"Auth uses short-lived JWTs, refreshed through /auth/refresh"
```

O primeiro armazenamento ou recuperação com busca semântica baixa uma única vez o modelo de embeddings multilíngue (`Qdrant/multilingual-e5-large-onnx`, cerca de 2 GB). Para testar o ICM sem ele, adicione `--no-embeddings` (recuperação por palavras-chave, como na saída acima) ou escolha um modelo mais leve na configuração. `icm init` grava hooks e instruções na configuração de cada agente detectado; `icm uninstall --dry-run` mostra como removê-los. Windows, Linux, Nix e compilação a partir do código-fonte: [Instalação](#install).

<a id="benchmark-comparison"></a>

## Comparação de benchmarks

Precisão das respostas no [LoCoMo](https://github.com/snap-research/locomo) (10 conversas longas, 1,540 perguntas), medida com o harness público [Agent Memory Benchmark](https://github.com/vectorize-io/agent-memory-benchmark): o sistema de memória recupera o contexto, `gemini-3.1-pro-preview` responde a partir dele e `gemini-2.5-flash-lite` avalia a resposta.

| Sistema | Precisão no LoCoMo | Contexto por pergunta | Chamadas a LLM para armazenar uma memória | Roda como | Resultado |
|--------|:---------------:|:--------------------:|:---------------------------:|---------|--------|
| **ICM** 0.11.0 (motor de recall v2) | **92.9%** (1,430, 1,433 e 1,430 / 1,540 em três execuções) | 24.1k tokens | nenhuma | um binário Rust, arquivo SQLite | nossas execuções, 2026-10-06 |
| Hindsight | 92.0% (1,417 / 1,540) | 36.2k tokens | extração de fatos via LLM | serviço Python, PostgreSQL + pgvector | publicado pelo harness |
| Baseline de busca híbrida (dense + sparse, RRF) | 79.1% (1,218 / 1,540) | 22.2k tokens | nenhuma | Qdrant | publicado pelo harness |

No [PersonaMem](https://arxiv.org/abs/2504.14225) 32k (589 perguntas de múltipla escolha sobre as preferências de um usuário que mudam com o tempo, mesmo harness e mesmo modelo de resposta, pontuadas pela correspondência da letra):

| Sistema | Precisão no PersonaMem | Contexto por pergunta | Resultado |
|--------|:-------------------:|:--------------------:|--------|
| **ICM** 0.11.0 (motor de recall v2) | **81.7%** (486, 486 e 472 / 589 em três execuções) | 16.2k tokens | nossas execuções, 2026-10-06 |
| Hindsight | 86.6% (510 / 589) | 15.8k tokens | publicado pelo harness |
| Baseline de busca híbrida | 84.4% (497 / 589) | 24.2k tokens | publicado pelo harness |

Só a recuperação, no LoCoMo, sem modelo de resposta: a proporção de perguntas para as quais pelo menos uma sessão de referência (gold) está entre os primeiros resultados. Esta é a evidência sobre o motor de recall em si. As execuções v2 receberam a data de cada sessão e a data da pergunta; o motor anterior não tem entrada de data, então suas execuções não receberam nenhuma.

| Primeiros resultados | Motor anterior (`legacy`) | Motor de recall v2 | Motor anterior, sem modelo de embeddings | Motor de recall v2, sem modelo de embeddings |
|:-----------:|:--------------:|:----------------:|:--------------:|:----------------:|
| 5 | 76.5% | **86.7%** | 12.0% | **88.6%** |
| 10 | 83.0% | **93.3%** | 17.4% | **94.2%** |
| 20 | 87.7% | **97.9%** | 29.8% | **97.5%** |

LongMemEval-S, só recuperação, sem LLM (ICM com seu modelo de embeddings padrão; 500 perguntas; cada pergunta vem com cerca de 48 sessões passadas para pesquisar; uma memória por sessão, só os turnos do usuário, como o MemPalace as indexa; nenhuma data fornecida ao ICM):

| Sessões certas no top 5 | **ICM** 0.11.0 | MemPalace | agentmemory | Só BM25 |
|---|:---:|:---:|:---:|:---:|
| Pelo menos uma (a medida publicada) | **97.4%** (487 / 500) | 96.6% (483 / 500) | 95.2% (476 / 500) | 94.6% |
| Todas | **88.6%** (443 / 500) | 85.0% | 81.8% | 81.2% |

Os números do MemPalace e do agentmemory foram recalculados com o nosso avaliador a partir dos arquivos de resultados que cada projeto publica; eles batem com os números publicados. O agentmemory indexa todos os turnos de uma sessão; nessa unidade, só o BM25 chega a 96.2% e 83.0%. Um BM25 simples já fica em torno de 95% na primeira medida, e é por isso que a segunda, todas as sessões certas, separa melhor os sistemas.

O que esses números mostram e o que não mostram:

- **ICM e Hindsight estão empatados no LoCoMo.** As três execuções do ICM (92.9%, 93.1%, 92.9%) ficam cada uma cerca de 1 ponto acima dos 92.0% publicados do Hindsight, uma diferença de cerca de 14 perguntas, dentro do erro de amostragem (intervalo de 95% para o ICM: 91.6 a 94.2). O ICM chega lá com um terço a menos de contexto e sem chamar um LLM quando uma memória é armazenada.
- **No PersonaMem, o Hindsight está à frente** por 4.9 pontos, fora do erro de amostragem (intervalo de 95% para o ICM: 78.6 a 84.8). O ICM também fica 2.7 pontos abaixo do baseline de busca híbrida, dentro desse intervalo, lendo um terço a menos de contexto que ele. As três execuções variam de 80.1% a 82.5%.
- **Condições não idênticas.** O harness é mantido pela Vectorize, a empresa por trás do Hindsight. Os resultados publicados do LoCoMo são anteriores a uma mudança que fixou em 0 a temperatura da resposta e do avaliador; nossa execução usa o harness atual (commit `f618ed7`) e o Vertex AI.
- **Com 50 chunks, grande parte de cada conversa é retornada,** então essa precisão também mede o modelo de resposta. A tabela de recuperação é a evidência sobre o motor de recall em si.
- **Três execuções por conjunto de dados.** O modelo de resposta varia de uma execução para outra: 0.2 ponto no LoCoMo, 2.4 pontos no PersonaMem. Os intervalos acima cobrem a amostragem das perguntas.

<details>
<summary>Resultados por categoria, latência e outras ressalvas</summary>

- **Por tipo de pergunta** (LoCoMo, rótulos do harness, três execuções): open-domain 96.7% (841 perguntas), temporal 90.9% (321), single-hop 88.8% (282), multi-hop 78.8% (96).
- **Latência.** Latência mediana de recuperação de 137 a 149 ms nos nós de 4 vCPUs do cluster durante as execuções do LoCoMo; 272 sessões ingeridas sem nenhuma chamada a LLM. Nas execuções só de recuperação no Linux x86-64: mediana de 136 ms com embeddings e 6.8 ms só com palavras-chave.
- **Datas das sessões.** O adaptador do benchmark grava a data de cada sessão no texto da memória do ICM; o Hindsight recebe as mesmas datas como metadados, e todo sistema recebe a data da pergunta.
- **A categoria mais fraca são as 96 perguntas que o harness rotula como multi-hop** (78.8%). Os nomes das categorias não batem entre benchmarks: outras avaliações do LoCoMo chamam essa categoria de open-domain e chamam de multi-hop as 282 perguntas que o harness rotula como single-hop (88.8% aqui). Compare pelo número de perguntas, não pelo rótulo.
- **O motor de recall v2 é o padrão** para `icm recall`, a ferramenta MCP `icm_memory_recall`, `/recall` via HTTP e o hook de prompt. O motor anterior continua disponível para voltar atrás ou comparar: `icm recall --engine legacy`, `"engine": "legacy"` em `/recall` via HTTP, ou `ICM_RECALL_ENGINE=legacy`.

</details>

Os resultados por pergunta de cada execução (três por conjunto de dados, mais a execução de recall no LongMemEval-S) estão em [`bench/amb/results/`](bench/amb/results/); o adaptador, as configurações exatas e os comandos para reproduzir estão em [`bench/amb/README.md`](bench/amb/README.md).

<a id="one-memory-for-every-tool"></a>

## Uma memória para todas as ferramentas

Toda ferramenta configurada por `icm init` lê e grava no mesmo banco de dados SQLite, e os topics (`decisions-myapp`, `preferences`, `errors-resolved`, ...) não são separados por ferramenta. Uma memória armazenada a partir do Claude Code fica visível imediatamente para Codex, Gemini, Cursor, Roo, Amp, Aider, ...

Prefere isolamento? `icm init --per-project` cria um banco de dados local do projeto em `.icm/` (e grava os arquivos de instruções dos agentes, como `CLAUDE.md` e `AGENTS.md`, no diretório atual); `--db <path>` ou `ICM_DB` apontam para qualquer outro arquivo. Cada caminho é um corpus independente.

> **Status do projeto: beta.** O ICM é pré-1.0: mudanças incompatíveis podem chegar em qualquer versão menor, e os formatos de configuração de hooks e do MCP podem mudar. Beta se refere à estabilidade da API, não à utilidade no dia a dia: eu (o mantenedor) uso o ICM todos os dias como minha memória principal para programar com IA. Meu foco principal é o [rtk](https://github.com/rtk-ai/rtk), então issues e pull requests são revisados conforme a disponibilidade.
>
> Apache-2.0, distribuído **no estado em que se encontra, sem garantia de qualquer tipo** (veja [LICENSE](LICENSE)). Antes de qualquer operação destrutiva, execute primeiro o equivalente somente leitura (`icm uninstall --dry-run`, `icm uninstall --check`).

<a id="install"></a>

## Instalação

```bash
# macOS / Linux, Homebrew
brew tap rtk-ai/tap && brew install icm

# macOS / Linux, script (verifies SHA256 against the release checksums)
curl -fsSL https://raw.githubusercontent.com/rtk-ai/icm/main/install.sh | sh

# Windows, PowerShell
irm https://raw.githubusercontent.com/rtk-ai/icm/main/install.ps1 | iex
```

A busca por palavras-chave funciona em qualquer lugar. Execute `icm embeddings status` para ver se a busca semântica está ativa: ela vem embutida nos builds para macOS Apple Silicon, Windows e `.rpm`; os arquivos Linux glibc e o `.deb` precisam de um `icm embeddings download`; o build para Mac Intel precisa do seu próprio ONNX Runtime (`ORT_DYLIB_PATH`); o build estático Linux musl só faz busca por palavras-chave. Nix, compilação a partir do código-fonte, fixação de versão e os detalhes: [referência](docs/reference.md#install).

<a id="setup"></a>

## Configuração

```bash
icm init                  # global database, every detected agent
icm init --per-project    # database under .icm/ at the git root
icm init --mode all       # also register the MCP server in every tool that supports it
```

O modo padrão (`standard`) grava instruções, skills e hooks, sem servidor MCP. `--mode all` adiciona o servidor MCP; com ele (mais `--per-project` para o Aider, cujo arquivo de convenções é por projeto), isso cobre as 18 ferramentas abaixo ([guia de integração](docs/integrations.md)):

| Ferramenta | Servidor MCP | Hooks |
|------|:---:|:-----:|
| Claude Code | sim | sim |
| Claude Desktop | sim | — |
| Gemini CLI | sim | sim |
| Codex CLI | sim | sim |
| Copilot CLI | sim | sim |
| Cursor | sim | — |
| Windsurf | sim | — |
| VS Code | sim | — |
| Amp | sim | — |
| Amazon Q | sim | — |
| Cline | sim | — |
| Roo Code | sim | — |
| Kilo Code | sim | — |
| Zed | sim | — |
| OpenCode | sim | sim |
| Continue.dev | sim | — |
| Aider | — | — |
| Pi | — | — |

Ou registre o servidor MCP à mão: `claude mcp add icm -- icm serve` (qualquer cliente MCP: comando `icm`, argumentos `["serve"]`).

O que os hooks fazem:

| Hook | O que faz |
|------|-------------|
| `icm hook start` | Injeta no início da sessão um pacote de despertar com memórias critical/high (~500 tokens) |
| `icm hook pre` | Permite automaticamente os comandos CLI do `icm` (sem pedido de permissão) |
| `icm hook post` | Extrai fatos da saída das ferramentas a cada N chamadas (extração automática) |
| `icm hook compact` | Extrai memórias da transcrição antes da compressão do contexto |
| `icm hook prompt` | Injeta o contexto recuperado no início de cada prompt do usuário |

Tabelas de hooks por ferramenta, skills, arquivos de instruções e a nota sobre o Codex: [referência](docs/reference.md#setup).

<a id="use"></a>

## Uso

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

O ICM também guarda **memoirs** (grafos de conhecimento permanentes de conceitos e relações tipadas), **feedback** (correções com as quais aprender) e **transcrições literais**, e expõe **31 ferramentas MCP** (30 sem modelo de embeddings), uma **API HTTP** que mantém o modelo de embeddings carregado e um **painel no terminal** (`icm dashboard`). Tudo isso está na [referência](docs/reference.md).

<a id="how-it-works"></a>

## Como funciona

A recuperação funde até três listas ordenadas por posição recíproca (RRF): correspondência por palavras-chave com **FTS5 BM25**, sempre ativa; **busca vetorial semântica** via sqlite-vec quando um modelo de embeddings está carregado (padrão `Qdrant/multilingual-e5-large-onnx`, 1024 dimensões, 100+ idiomas); e uma **janela de datas** quando a consulta nomeia um período ("last week", "in March 2024"). Os filtros de projeto, topic e palavra-chave são aplicados antes do corte. As memórias se desgastam com o tempo de acordo com sua importância (`critical` nunca se desgasta); com um modelo de embeddings carregado, uma memória nova quase idêntica a outra do mesmo topic (similaridade de cosseno acima de 0.95) é mesclada a ela; e o modelo que produziu os vetores armazenados fica registrado no banco de dados, então mudar `model` na configuração nunca os apaga (`icm embed --migrate` é a forma explícita de trocar de modelo).

Tudo fica em um único arquivo SQLite, sem serviço externo:

```
~/Library/Application Support/dev.icm.icm/memories.db     # macOS
~/.local/share/icm/memories.db                            # Linux
%APPDATA%\icm\icm\data\memories.db                        # Windows
<project-root>/.icm/memories.db                           # icm init --per-project
```

`icm config` mostra a configuração ativa; [config/default.toml](config/default.toml) lista todas as opções. Detalhes: [referência](docs/reference.md#how-it-works).

<a id="documentation"></a>

## Documentação

| Documento | Descrição |
|----------|-------------|
| [Guia de integração](docs/integrations.md) | Configuração MCP por ferramenta: Claude Code, Cursor, Windsurf, Zed, Amp, Codex, Cline, Roo Code etc. |
| [Arquitetura técnica](docs/architecture.md) | Estrutura dos crates, pipeline de busca, modelo de decaimento, integração com sqlite-vec, testes |
| [Guia do usuário](docs/guide.md) | Instalação, organização dos topics, consolidação, extração, solução de problemas |
| [Visão geral do produto](docs/product.md) | Casos de uso, benchmarks, comparação com alternativas |
| [Referência](docs/reference.md) | Opções de instalação, configuração por ferramenta, CLI, 31 ferramentas MCP, API HTTP, painel, funcionamento interno |
| [Adaptador do benchmark](bench/amb/README.md) | Como a comparação acima foi feita e como reproduzi-la |
| [Demonstrações](docs/demonstrations.md) | Microbenchmarks de armazenamento e pequenas demos |

<a id="license"></a>

## Licença

[Apache-2.0](LICENSE)
