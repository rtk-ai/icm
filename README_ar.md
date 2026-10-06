[English](README.md) | [Français](README_fr.md) | [Español](README_es.md) | [Deutsch](README_de.md) | [Italiano](README_it.md) | [Português](README_pt.md) | [Nederlands](README_nl.md) | [Polski](README_pl.md) | [Русский](README_ru.md) | [日本語](README_ja.md) | [中文](README_zh.md) | [العربية](README_ar.md) | [한국어](README_ko.md)

هذه ترجمة لملف README.md (بالإنجليزية)، وهو المرجع؛ وإذا اختلفا، فالنسخة الإنجليزية هي الصحيحة.

<h1 align="center">ICM</h1>

<p align="center">
  <b>ذاكرة طويلة المدى لوكلاء البرمجة بالذكاء الاصطناعي، مشتركة بين أدواتك.</b><br>
  ملف تنفيذي واحد، وملف SQLite واحد. لا حاجة إلى استدعاء LLM لتخزين ذكرى أو استرجاعها.
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/icm/actions/workflows/ci.yml"><img src="https://github.com/rtk-ai/icm/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/icm/releases/latest"><img src="https://img.shields.io/github/v/release/rtk-ai/icm?color=purple" alt="الإصدار"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-Apache--2.0-blue.svg" alt="Apache-2.0"></a>
</p>

أخبر Claude Code يوم الاثنين كيف يتعامل مشروعك مع المصادقة، وستكون جلسة Gemini CLI يوم الثلاثاء على علم بذلك. يحفظ ICM ما يتعلّمه وكلاء البرمجة لديك (القرارات، والإصلاحات، والاصطلاحات، والتفضيلات) في ملف SQLite واحد على جهازك، ويعيد الجزء المتعلق منه في بداية كل جلسة ومع كل موجّه (prompt) ترسله. يتشارك ما يصل إلى 18 وكيلًا ومحررًا هذه الذاكرة، فلا تعود مضطرًا إلى إعادة شرح مشروعك في كل مرة تفتح فيها جلسة أو تنتقل إلى أداة أخرى.

- **92.9% على LoCoMo (1,540 سؤالًا، متوسط ثلاثة تشغيلات)، بمستوى Hindsight (92.0%)**، مع سياق أقل بمقدار الثلث لكل سؤال. [التفاصيل والتحفظات أدناه](#benchmark-comparison).
- **لا استدعاء لـ LLM عند التخزين أو الاسترجاع.** تستدعي Hindsight و Mem0 و Graphiti (Zep) و claude-mem نموذج LLM افتراضيًا لكل ذكرى تخزّنها. أما ICM فلا يفعل. وحده الاستخراج التلقائي للحقائق من مخرجات الأدوات يمر عبر أداة سطر أوامر LLM التي تستخدمها أصلًا، إن كانت مثبّتة؛ والإعداد `provider = "none"` يُبقي هذا أيضًا محليًا (انظر [البدء السريع](#quickstart)).
- **مفيد من دون نموذج تضمين (embedding).** الاسترجاع بالكلمات المفتاحية وحده يضع جلسة صحيحة واحدة على الأقل ضمن أفضل 5 نتائج في 88.6% من أسئلة LoCoMo، بوسيط 6.8 ms لكل عملية استرجاع على Linux x86-64.
- **97.4% على LongMemEval-S، استرجاع من دون LLM**: إحدى الجلسات الصحيحة ضمن أفضل 5 نتائج في 487 من 500 سؤال، مقابل 96.6% لـ MemPalace و 95.2% لـ agentmemory بالمقياس نفسه.
- **ليس متقدمًا في كل شيء.** على PersonaMem (تفضيلات المستخدم المتغيرة)، يتقدم Hindsight: 86.6% مقابل 81.7% لـ ICM.

<p align="center">
  <img src="assets/demo.svg" alt="الطرفية: تخزين ثلاث ذكريات باستخدام icm store، ثم إجابة icm recall عن سؤالين، مع إعادة الذكرى الصحيحة في كل مرة">
</p>

<a id="quickstart"></a>
## البدء السريع

```bash
brew tap rtk-ai/tap && brew install icm    # or: curl -fsSL https://raw.githubusercontent.com/rtk-ai/icm/main/install.sh | sh
icm init                                   # instructions, skills and hooks for every agent it detects
```

هذا كل الإعداد. افتح جلسة جديدة في Claude Code أو Codex أو Gemini CLI أو Copilot CLI: سيبدأ وكيلك الآن بحزمة قصيرة من أهم ذكرياته، وسيتلقى الذكريات المتعلقة بكل موجّه ترسله. ما يتعلمه من مخرجات أدواته يوضع في قائمة انتظار، وتُحوَّل هذه القائمة إلى ذكريات في نهاية كل جلسة Claude Code؛ أما مع الأدوات الأخرى، فشغّل `icm extract-pending` (من مهمة cron مثلًا).

يبقى التخزين والاسترجاع على جهازك. يسلّم الاستخراج التلقائي النص إلى أداة سطر أوامر LLM التي تستخدمها أصلًا (Claude Code أو Codex أو Gemini CLI) إن كانت مثبّتة؛ اضبط `provider = "none"` تحت `[extraction.summarizer]` لإبقائه محليًا بالكامل.

لترى ICM يعمل فورًا، خزّن واسترجع يدويًا:

```console
$ icm store -t decisions-myapp -c "Auth uses short-lived JWTs, refreshed through /auth/refresh" -i high
Stored: 01M47YY6CHVZ48BYKQRCRNZTCF

$ icm recall "how does auth work"
memories[1]{id,topic,importance,weight,summary}:
  01M47YY6CHVZ48BYKQRCRNZTCF,decisions-myapp,high,0.975,"Auth uses short-lived JWTs, refreshed through /auth/refresh"
```

أول عملية تخزين أو استرجاع تستخدم البحث الدلالي تنزّل نموذج التضمين متعدد اللغات مرة واحدة (`Qdrant/multilingual-e5-large-onnx`، نحو 2 GB). لتجربة ICM من دونه، أضف `--no-embeddings` (استرجاع بالكلمات المفتاحية، كما في المخرجات أعلاه) أو اختر نموذجًا أخف في الإعدادات. يكتب `icm init` الخطافات (hooks) والتعليمات في إعدادات كل وكيل يكتشفه؛ ويعرض `icm uninstall --dry-run` كيفية إزالتها. Windows و Linux و Nix والبناء من المصدر: [التثبيت](#install).

<a id="benchmark-comparison"></a>
## مقارنة نتائج الاختبارات المعيارية

دقة الإجابات على [LoCoMo](https://github.com/snap-research/locomo) (10 محادثات طويلة، 1,540 سؤالًا)، مقيسة باستخدام منصة الاختبار العامة [Agent Memory Benchmark](https://github.com/vectorize-io/agent-memory-benchmark): يسترجع نظام الذاكرة السياق، ويجيب `gemini-3.1-pro-preview` استنادًا إليه، ويحكم `gemini-2.5-flash-lite` على الإجابة.

| النظام | الدقة على LoCoMo | السياق لكل سؤال | استدعاءات LLM لتخزين ذكرى | يعمل بوصفه | النتيجة |
|--------|:---------------:|:--------------------:|:---------------------------:|---------|--------|
| **ICM** 0.11.0 (محرك الاسترجاع v2) | **92.9%** (1,430 و 1,433 و 1,430 / 1,540 في ثلاثة تشغيلات) | 24.1k tokens | لا شيء | ملف تنفيذي واحد بلغة Rust، ملف SQLite | تشغيلاتنا، 2026-10-06 |
| Hindsight | 92.0% (1,417 / 1,540) | 36.2k tokens | استخراج الحقائق عبر LLM | خدمة Python، PostgreSQL + pgvector | منشورة من منصة الاختبار |
| خط الأساس للبحث الهجين (كثيف + متناثر، RRF) | 79.1% (1,218 / 1,540) | 22.2k tokens | لا شيء | Qdrant | منشورة من منصة الاختبار |

على [PersonaMem](https://arxiv.org/abs/2504.14225) 32k (589 سؤال اختيار من متعدد حول تفضيلات المستخدم المتغيرة، بالمنصة نفسها ونموذج الإجابة نفسه، مع التقييم بمطابقة الحرف):

| النظام | الدقة على PersonaMem | السياق لكل سؤال | النتيجة |
|--------|:-------------------:|:--------------------:|--------|
| **ICM** 0.11.0 (محرك الاسترجاع v2) | **81.7%** (486 و 486 و 472 / 589 في ثلاثة تشغيلات) | 16.2k tokens | تشغيلاتنا، 2026-10-06 |
| Hindsight | 86.6% (510 / 589) | 15.8k tokens | منشورة من منصة الاختبار |
| خط الأساس للبحث الهجين | 84.4% (497 / 589) | 24.2k tokens | منشورة من منصة الاختبار |

الاسترجاع وحده، على LoCoMo، من دون نموذج إجابة: نسبة الأسئلة التي تكون فيها جلسة مرجعية واحدة على الأقل ضمن أعلى النتائج. هذا هو الدليل على أداء محرك الاسترجاع نفسه. تلقّت تشغيلات v2 تاريخ كل جلسة وتاريخ السؤال؛ أما المحرك السابق فلا يقبل التاريخ مدخلًا، لذا لم تتلقَّ تشغيلاته أي تاريخ.

| أعلى النتائج | المحرك السابق (`legacy`) | محرك الاسترجاع v2 | المحرك السابق، دون نموذج تضمين | محرك الاسترجاع v2، دون نموذج تضمين |
|:-----------:|:--------------:|:----------------:|:--------------:|:----------------:|
| 5 | 76.5% | **86.7%** | 12.0% | **88.6%** |
| 10 | 83.0% | **93.3%** | 17.4% | **94.2%** |
| 20 | 87.7% | **97.9%** | 29.8% | **97.5%** |

LongMemEval-S، الاسترجاع وحده، من دون LLM (ICM مع نموذج التضمين الافتراضي لديه؛ 500 سؤال؛ يأتي مع كل سؤال نحو 48 جلسة سابقة للبحث فيها؛ ذكرى واحدة لكل جلسة، أدوار المستخدم فقط، كما يفهرسها MemPalace؛ من دون تزويد ICM بأي تاريخ):

| الجلسات الصحيحة ضمن أفضل 5 نتائج | **ICM** 0.11.0 | MemPalace | agentmemory | BM25 وحده |
|---|:---:|:---:|:---:|:---:|
| واحدة على الأقل (المقياس المنشور) | **97.4%** (487 / 500) | 96.6% (483 / 500) | 95.2% (476 / 500) | 94.6% |
| جميعها | **88.6%** (443 / 500) | 85.0% | 81.8% | 81.2% |

أُعيد حساب أرقام MemPalace و agentmemory بأداة التقييم لدينا انطلاقًا من ملفات النتائج التي ينشرها كل مشروع؛ وهي تطابق أرقامهما المنشورة. يفهرس agentmemory جميع أدوار الجلسة؛ وبهذه الوحدة يبلغ BM25 وحده 96.2% و 83.0%. يحقق BM25 بسيط بالفعل نحو 95% في المقياس الأول، ولهذا فإن المقياس الثاني، أي جميع الجلسات الصحيحة، يميّز بين الأنظمة على نحو أفضل.

ما تُظهره هذه الأرقام وما لا تُظهره:

- **ICM و Hindsight متعادلان على LoCoMo.** كل واحد من تشغيلات ICM الثلاثة (92.9%، 93.1%، 92.9%) أعلى بنحو 1 نقطة من نتيجة Hindsight المنشورة 92.0%، أي بفارق نحو 14 سؤالًا، وهو ضمن خطأ المعاينة (فترة الثقة 95% لـ ICM: من 91.6 إلى 94.2). يصل ICM إلى ذلك بسياق أقل بمقدار الثلث ومن دون استدعاء LLM عند تخزين ذكرى.
- **على PersonaMem، يتقدم Hindsight** بفارق 4.9 نقطة، خارج خطأ المعاينة (فترة الثقة 95% لـ ICM: من 78.6 إلى 84.8). ويقل ICM أيضًا بـ 2.7 نقطة عن خط الأساس للبحث الهجين، وهذا ضمن تلك الفترة، مع قراءة سياق أقل منه بمقدار الثلث. تتراوح التشغيلات الثلاثة بين 80.1% و 82.5%.
- **الظروف ليست متطابقة.** تتولى صيانة منصة الاختبار شركة Vectorize، مطوّرة Hindsight. نتائج LoCoMo المنشورة سابقة لتغيير ضبط درجة حرارة نموذجي الإجابة والتحكيم على 0؛ أما تشغيلنا فيستخدم الإصدار الحالي من المنصة (commit `f618ed7`) و Vertex AI.
- **عند 50 مقطعًا (chunk)، يُعاد جزء كبير من كل محادثة،** لذا تقيس هذه الدقة نموذج الإجابة أيضًا. جدول الاسترجاع هو الدليل على أداء محرك الاسترجاع نفسه.
- **ثلاثة تشغيلات لكل مجموعة بيانات.** تتباين نتائج نموذج الإجابة من تشغيل إلى آخر: 0.2 نقطة على LoCoMo، و 2.4 نقطة على PersonaMem. تغطي الفترات أعلاه معاينة الأسئلة.

<details>
<summary>النتائج حسب الفئة، وزمن الاستجابة، وتحفظات إضافية</summary>

- **حسب نوع السؤال** (LoCoMo، تسميات منصة الاختبار، ثلاثة تشغيلات): open-domain 96.7% (841 سؤالًا)، temporal 90.9% (321)، single-hop 88.8% (282)، multi-hop 78.8% (96).
- **زمن الاستجابة.** بلغ وسيط زمن الاسترجاع من 137 إلى 149 ms على عُقد العنقود ذات 4 vCPU أثناء تشغيلات LoCoMo؛ وجرى إدخال 272 جلسة من دون أي استدعاء LLM. في تشغيلات الاسترجاع وحده على Linux x86-64: الوسيط 136 ms مع التضمينات، و 6.8 ms بالكلمات المفتاحية فقط.
- **تواريخ الجلسات.** يكتب مُحوّل الاختبار (adapter) تاريخ كل جلسة في نص الذكرى في ICM؛ ويتلقى Hindsight التواريخ نفسها كبيانات وصفية، ويحصل كل نظام على تاريخ السؤال.
- **أضعف فئة هي الأسئلة الـ 96 التي تسمّيها منصة الاختبار multi-hop** (78.8%). أسماء الفئات لا تتطابق بين الاختبارات المعيارية: تقييمات LoCoMo الأخرى تسمّي هذه الفئة open-domain، وتسمّي multi-hop الأسئلةَ الـ 282 التي تسمّيها المنصة single-hop (88.8% هنا). قارن حسب عدد الأسئلة، لا حسب التسمية.
- **محرك الاسترجاع v2 هو الافتراضي** في `icm recall`، وأداة MCP `icm_memory_recall`، و HTTP `/recall`، وخطاف الموجّه. يبقى المحرك السابق متاحًا للرجوع إليه أو للمقارنة: `icm recall --engine legacy`، أو `"engine": "legacy"` على HTTP `/recall`، أو `ICM_RECALL_ENGINE=legacy`.

</details>

نتائج كل سؤال لكل تشغيل (ثلاثة لكل مجموعة بيانات، إضافة إلى تشغيل الاسترجاع على LongMemEval-S) موجودة في [`bench/amb/results/`](bench/amb/results/)؛ والمُحوّل والإعدادات الدقيقة وأوامر إعادة الإنتاج موجودة في [`bench/amb/README.md`](bench/amb/README.md).

<a id="one-memory-for-every-tool"></a>
## ذاكرة واحدة لكل الأدوات

كل أداة يضبطها `icm init` تقرأ قاعدة بيانات SQLite نفسها وتكتب فيها، والمواضيع (`decisions-myapp`، `preferences`، `errors-resolved`، ...) غير مقسّمة حسب الأداة. الذكرى المخزّنة من Claude Code تصبح مرئية فورًا لـ Codex و Gemini و Cursor و Roo و Amp و Aider، ...

تريد العزل بدلًا من ذلك؟ ينشئ `icm init --per-project` قاعدة بيانات محلية للمشروع تحت `.icm/` (ويكتب ملفات تعليمات الوكلاء، مثل `CLAUDE.md` و `AGENTS.md`، في المجلد الحالي)؛ ويشير `--db <path>` أو `ICM_DB` إلى أي ملف آخر. كل مسار مجموعة نصوص (corpus) مستقلة.

> **حالة المشروع: بيتا.** ICM في مرحلة ما قبل 1.0: قد تصل تغييرات غير متوافقة مع ما سبق في أي إصدار فرعي (minor)، وقد تتغير صيغ إعدادات الخطافات و MCP. تشير بيتا إلى استقرار الواجهة البرمجية (API)، لا إلى الفائدة في الاستخدام اليومي: أنا (المشرف على المشروع) أستخدم ICM كل يوم ذاكرتي الأساسية للبرمجة بالذكاء الاصطناعي. تركيزي الأساسي على [rtk](https://github.com/rtk-ai/rtk)، لذا تُراجَع البلاغات (issues) وطلبات الدمج (pull requests) بحسب ما يتيحه الوقت.
>
> مرخّص بموجب Apache-2.0، ويُقدَّم **كما هو (as-is)، من دون أي ضمان من أي نوع** (انظر [LICENSE](LICENSE)). قبل أي عملية مدمِّرة، شغّل أولًا ما يعادلها للقراءة فقط (`icm uninstall --dry-run`، `icm uninstall --check`).

<a id="install"></a>
## التثبيت

```bash
# macOS / Linux, Homebrew
brew tap rtk-ai/tap && brew install icm

# macOS / Linux, script (verifies SHA256 against the release checksums)
curl -fsSL https://raw.githubusercontent.com/rtk-ai/icm/main/install.sh | sh

# Windows, PowerShell
irm https://raw.githubusercontent.com/rtk-ai/icm/main/install.ps1 | iex
```

البحث بالكلمات المفتاحية يعمل في كل مكان. شغّل `icm embeddings status` لمعرفة ما إذا كان البحث الدلالي مفعّلًا: فهو مدمج في إصدارات macOS Apple Silicon و Windows و `.rpm`؛ وتحتاج أرشيفات Linux glibc وحزمة `.deb` إلى تشغيل `icm embeddings download` مرة واحدة؛ ويحتاج إصدار Mac بمعالج Intel إلى ONNX Runtime خاص بك (`ORT_DYLIB_PATH`)؛ أما إصدار Linux musl الثابت فيقتصر على الكلمات المفتاحية. Nix، والبناء من المصدر، وتثبيت الإصدار، والتفاصيل: [المرجع](docs/reference.md#install).

<a id="setup"></a>
## الإعداد

```bash
icm init                  # global database, every detected agent
icm init --per-project    # database under .icm/ at the git root
icm init --mode all       # also register the MCP server in every tool that supports it
```

الوضع الافتراضي (`standard`) يكتب التعليمات والمهارات (skills) والخطافات، من دون خادم MCP. يضيف `--mode all` خادم MCP؛ ومعه (إضافةً إلى `--per-project` لـ Aider، الذي يكون ملف اصطلاحاته خاصًا بكل مشروع) يغطي ذلك الأدوات الـ 18 أدناه ([دليل التكامل](docs/integrations.md)):

| الأداة | خادم MCP | الخطافات |
|------|:---:|:-----:|
| Claude Code | نعم | نعم |
| Claude Desktop | نعم | — |
| Gemini CLI | نعم | نعم |
| Codex CLI | نعم | نعم |
| Copilot CLI | نعم | نعم |
| Cursor | نعم | — |
| Windsurf | نعم | — |
| VS Code | نعم | — |
| Amp | نعم | — |
| Amazon Q | نعم | — |
| Cline | نعم | — |
| Roo Code | نعم | — |
| Kilo Code | نعم | — |
| Zed | نعم | — |
| OpenCode | نعم | نعم |
| Continue.dev | نعم | — |
| Aider | — | — |
| Pi | — | — |

أو سجّل خادم MCP يدويًا: `claude mcp add icm -- icm serve` (أي عميل MCP: الأمر `icm`، والوسائط `["serve"]`).

ما تفعله الخطافات:

| الخطاف | ما يفعله |
|------|-------------|
| `icm hook start` | يحقن حزمة إيقاظ من ذكريات critical/high عند بدء الجلسة (~500 رمز) |
| `icm hook pre` | يسمح تلقائيًا بأوامر `icm` في سطر الأوامر (من دون طلب إذن) |
| `icm hook post` | يستخرج الحقائق من مخرجات الأدوات كل N استدعاء (استخراج تلقائي) |
| `icm hook compact` | يستخرج الذكريات من نص المحادثة قبل ضغط السياق |
| `icm hook prompt` | يحقن السياق المسترجع في بداية كل موجّه من المستخدم |

جداول الخطافات لكل أداة، والمهارات، وملفات التعليمات، وملاحظة Codex: [المرجع](docs/reference.md#setup).

<a id="use"></a>
## الاستخدام

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

يحتفظ ICM أيضًا بـ **memoirs** (رسوم بيانية معرفية دائمة من المفاهيم والعلاقات ذات الأنواع)، و **feedback** (تصحيحات يُتعلَّم منها)، و **نصوص المحادثات الحرفية**، ويوفّر **31 أداة MCP** (30 من دون نموذج تضمين)، و **واجهة HTTP API** تُبقي نموذج التضمين محمّلًا، و **لوحة معلومات في الطرفية** (`icm dashboard`). كل ذلك موثّق في [المرجع](docs/reference.md).

<a id="how-it-works"></a>
## كيف يعمل

يدمج الاسترجاع ما يصل إلى ثلاث قوائم مرتّبة بطريقة دمج الرتب التبادلية (RRF): مطابقة الكلمات المفتاحية **FTS5 BM25**، المفعّلة دائمًا؛ و **البحث المتجهي الدلالي** عبر sqlite-vec عند تحميل نموذج تضمين (الافتراضي `Qdrant/multilingual-e5-large-onnx`، 1024 بُعدًا، 100+ لغة)؛ و **نافذة زمنية** حين يذكر الاستعلام فترة ("last week"، "in March 2024"). تُطبَّق مرشّحات المشروع والموضوع والكلمات المفتاحية قبل الاقتطاع. تتلاشى الذكريات مع الوقت بحسب أهميتها (`critical` لا تتلاشى أبدًا)؛ ومع تحميل نموذج تضمين، تُدمج الذكرى الجديدة شبه المطابقة لذكرى موجودة في الموضوع نفسه (تشابه جيب التمام أعلى من 0.95) في تلك الذكرى؛ ويُسجَّل في قاعدة البيانات النموذج الذي أنتج المتجهات المخزّنة، لذا فإن تغيير `model` في الإعدادات لا يمسحها أبدًا (`icm embed --migrate` هي الطريقة الصريحة للتبديل).

يوجد كل شيء في ملف SQLite واحد، من دون أي خدمة خارجية:

```
~/Library/Application Support/dev.icm.icm/memories.db     # macOS
~/.local/share/icm/memories.db                            # Linux
%APPDATA%\icm\icm\data\memories.db                        # Windows
<project-root>/.icm/memories.db                           # icm init --per-project
```

يعرض `icm config` الإعدادات النشطة؛ ويسرد [config/default.toml](config/default.toml) كل الخيارات. التفاصيل: [المرجع](docs/reference.md#how-it-works).

<a id="documentation"></a>
## التوثيق

| المستند | الوصف |
|----------|-------------|
| [دليل التكامل](docs/integrations.md) | إعداد MCP لكل أداة: Claude Code، Cursor، Windsurf، Zed، Amp، Codex، Cline، Roo Code، إلخ. |
| [البنية التقنية](docs/architecture.md) | بنية الـ crates، ومسار البحث، ونموذج التلاشي، وتكامل sqlite-vec، والاختبارات |
| [دليل المستخدم](docs/guide.md) | التثبيت، وتنظيم المواضيع، والدمج، والاستخراج، واستكشاف الأخطاء وإصلاحها |
| [نظرة عامة على المنتج](docs/product.md) | حالات الاستخدام، والاختبارات المعيارية، والمقارنة مع البدائل |
| [المرجع](docs/reference.md) | خيارات التثبيت، والإعداد لكل أداة، و CLI، و 31 أداة MCP، و HTTP API، ولوحة المعلومات، والتفاصيل الداخلية |
| [مُحوّل الاختبار](bench/amb/README.md) | كيف أُجريت المقارنة أعلاه، وكيف يمكن إعادة إنتاجها |
| [العروض التوضيحية](docs/demonstrations.md) | اختبارات أداء مصغّرة للتخزين وعروض صغيرة |

<a id="license"></a>
## الترخيص

[Apache-2.0](LICENSE)
