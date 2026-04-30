[English](README.md) | [Français](README_fr.md) | [Español](README_es.md) | [Deutsch](README_de.md) | [Italiano](README_it.md) | [Português](README_pt.md) | [Nederlands](README_nl.md) | [Polski](README_pl.md) | [Русский](README_ru.md) | [日本語](README_ja.md) | [中文](README_zh.md) | **العربية** | [한국어](README_ko.md)

<p align="center">
  <img src="assets/banner.png" alt="ICM — Infinite Context Memory" width="600">
</p>

<h1 align="center">ICM</h1>

<p align="center">
  ذاكرة دائمة لعملاء الذكاء الاصطناعي. ملف تنفيذي واحد، بدون تبعيات، دعم MCP أصلي.
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/icm/actions/workflows/ci.yml"><img src="https://github.com/rtk-ai/icm/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/icm/releases/latest"><img src="https://img.shields.io/github/v/release/rtk-ai/icm?color=purple" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-Source--Available-orange.svg" alt="Source-Available"></a>
</p>

---

يمنح ICM عميلَ الذكاء الاصطناعي الخاص بك ذاكرةً حقيقية — ليست أداة تدوين ملاحظات، ولا مدير سياق، بل **ذاكرة** فعلية.

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

**نموذجان للذاكرة:**

- **Memories (الذكريات)** — تخزين واسترجاع مع تلاشٍ زمني حسب الأهمية. الذكريات الحرجة لا تتلاشى أبدًا، أما ذات الأهمية المنخفضة فتتلاشى تلقائيًا. يمكن التصفية بحسب الموضوع أو الكلمة المفتاحية.
- **Memoirs (المذكرات)** — رسوم بيانية دائمة للمعرفة. مفاهيم مرتبطة بعلاقات مكتوبة (`depends_on`، `contradicts`، `superseded_by`، ...). يمكن التصفية بحسب التصنيف.
- **Feedback (التغذية الراجعة)** — تسجيل التصحيحات عند خطأ توقعات الذكاء الاصطناعي. البحث في الأخطاء السابقة قبل إجراء تنبؤات جديدة. تعلم في حلقة مغلقة.

## التثبيت

```bash
# Homebrew (macOS / Linux)
brew tap rtk-ai/tap && brew install icm

# تثبيت سريع
curl -fsSL https://raw.githubusercontent.com/rtk-ai/icm/main/install.sh | sh

# من المصدر
cargo install --path crates/icm-cli
```

## الإعداد

```bash
# الكشف التلقائي وضبط جميع الأدوات المدعومة
icm init
```

يضبط **17 أداة** بأمر واحد ([دليل التكامل الكامل](docs/integrations.md)):

| الأداة | MCP | الخطافات | CLI | المهارات |
|--------|:---:|:--------:|:---:|:--------:|
| Claude Code | `~/.claude.json` | 5 خطافات | `CLAUDE.md` | `/recall` `/remember` |
| Claude Desktop | JSON | — | — | — |
| Gemini CLI | `~/.gemini/settings.json` | 5 خطافات | `GEMINI.md` | — |
| Codex CLI | `~/.codex/config.toml` | 4 خطافات | `AGENTS.md` | — |
| Copilot CLI | `~/.copilot/mcp-config.json` | 4 خطافات | `.github/copilot-instructions.md` | — |
| Cursor | `~/.cursor/mcp.json` | — | — | قاعدة `.mdc` |
| Windsurf | JSON | — | `.windsurfrules` | — |
| VS Code | `~/Library/.../Code/User/mcp.json` | — | — | — |
| Amp | JSON | — | — | `/icm-recall` `/icm-remember` |
| Amazon Q | JSON | — | — | — |
| Cline | VS Code globalStorage | — | — | — |
| Roo Code | VS Code globalStorage | — | — | قاعدة `.md` |
| Kilo Code | VS Code globalStorage | — | — | — |
| Zed | `~/.zed/settings.json` | — | — | — |
| OpenCode | JSON | إضافة TS | — | — |
| Continue.dev | `~/.continue/config.yaml` | — | — | — |
| Aider | — | — | `.aider.conventions.md` | — |

أو يدويًا:

```bash
# Claude Code
claude mcp add icm -- icm serve

# الوضع المضغوط (ردود أقصر، توفير رموز)
claude mcp add icm -- icm serve --compact

# أي عميل MCP: command = "icm", args = ["serve"]
```

### المهارات / القواعد

```bash
icm init --mode skill
```

يثبّت أوامر الشريطة المائلة والقواعد لـ Claude Code (`/recall`، `/remember`)، وCursor (قاعدة `.mdc`)، وRoo Code (قاعدة `.md`)، وAmp (`/icm-recall`، `/icm-remember`).

### الخطافات (5 أدوات)

```bash
icm init --mode hook
```

يثبّت خطافات الاستخراج والاسترجاع التلقائي لجميع الأدوات المدعومة:

| الأداة | SessionStart | PreTool | PostTool | Compact | PromptRecall | الضبط |
|--------|:-----------:|:-------:|:--------:|:-------:|:------------:|--------|
| Claude Code | `icm hook start` | `icm hook pre` | `icm hook post` | `icm hook compact` | `icm hook prompt` | `~/.claude/settings.json` |
| Gemini CLI | `icm hook start` | `icm hook pre` | `icm hook post` | `icm hook compact` | `icm hook prompt` | `~/.gemini/settings.json` |
| Codex CLI | `icm hook start` | `icm hook pre` | `icm hook post` | — | `icm hook prompt` | `~/.codex/hooks.json` |
| Copilot CLI | `icm hook start` | `icm hook pre` | `icm hook post` | — | `icm hook prompt` | `.github/hooks/icm.json` |
| OpenCode | session start | — | tool extract | compaction | — | `~/.config/opencode/plugins/icm.ts` |

**ما يفعله كل خطاف:**

| الخطاف | ما يفعله |
|--------|----------|
| `icm hook start` | حقن حزمة إيقاظ من الذكريات الحرجة/عالية الأهمية عند بدء الجلسة (حوالي 500 رمز) |
| `icm hook pre` | السماح تلقائيًا لأوامر `icm` CLI (بدون طلب إذن) |
| `icm hook post` | استخراج الحقائق من مخرجات الأداة كل N استدعاء (استخراج تلقائي) |
| `icm hook compact` | استخراج الذكريات من النص قبل ضغط السياق |
| `icm hook prompt` | حقن السياق المستعاد في بداية كل موجه مستخدم |

## CLI مقابل MCP

يمكن استخدام ICM عبر CLI (أوامر `icm`) أو خادم MCP (`icm serve`). كلاهما يصلان إلى نفس قاعدة البيانات.

| | CLI | MCP |
|---|-----|-----|
| **زمن الاستجابة** | ~30ms (ملف ثنائي مباشر) | ~50ms (JSON-RPC stdio) |
| **تكلفة الرموز** | 0 (قائم على الخطافات، غير مرئي) | ~20-50 رمز/استدعاء (مخطط الأداة) |
| **الإعداد** | `icm init --mode hook` | `icm init --mode mcp` |
| **يعمل مع** | Claude Code، Gemini، Codex، Copilot، OpenCode (عبر الخطافات) | جميع الأدوات الـ17 المتوافقة مع MCP |
| **الاستخراج التلقائي** | نعم (الخطافات تشغّل `icm extract`) | نعم (أدوات MCP تستدعي store) |
| **الأفضل لـ** | المستخدمين المتقدمين، توفير الرموز | التوافق الشامل |

## واجهة سطر الأوامر

### الذكريات (حلقية، مع تلاشٍ)

```bash
# تخزين
icm store -t "my-project" -c "Use PostgreSQL for the main DB" -i high -k "db,postgres"

# استرجاع
icm recall "database choice"
icm recall "auth setup" --topic "my-project" --limit 10
icm recall "architecture" --keyword "postgres"

# إدارة
icm forget <memory-id>
icm consolidate --topic "my-project"
icm topics
icm stats

# استخراج حقائق من النص (قائم على القواعد، بدون تكلفة LLM)
echo "The parser uses Pratt algorithm" | icm extract -p my-project
```

### المذكرات (رسوم بيانية دائمة للمعرفة)

```bash
# إنشاء مذكرة
icm memoir create -n "system-architecture" -d "System design decisions"

# إضافة مفاهيم مع تصنيفات
icm memoir add-concept -m "system-architecture" -n "auth-service" \
  -d "Handles JWT tokens and OAuth2 flows" -l "domain:auth,type:service"

# ربط المفاهيم
icm memoir link -m "system-architecture" --from "api-gateway" --to "auth-service" -r depends-on

# البحث مع تصفية التصنيف
icm memoir search -m "system-architecture" "authentication"
icm memoir search -m "system-architecture" "service" --label "domain:auth"

# فحص الجوار
icm memoir inspect -m "system-architecture" "auth-service" -D 2

# تصدير الرسم البياني (الصيغ: json, dot, ascii, ai)
icm memoir export -m "system-architecture" -f ascii   # رسم بالأحرف مع أشرطة الثقة
icm memoir export -m "system-architecture" -f dot      # Graphviz DOT (اللون = مستوى الثقة)
icm memoir export -m "system-architecture" -f ai       # Markdown محسّن لسياق LLM
icm memoir export -m "system-architecture" -f json     # JSON منظم مع جميع البيانات الوصفية

# توليد تصور SVG
icm memoir export -m "system-architecture" -f dot | dot -Tsvg > graph.svg
```

## أدوات MCP (31 أداة)

### أدوات الذاكرة

| الأداة | الوصف |
|--------|-------|
| `icm_memory_store` | التخزين مع إزالة التكرار التلقائي (تشابه >85% → تحديث بدلًا من تكرار) |
| `icm_memory_recall` | البحث بالاستعلام، التصفية بحسب الموضوع و/أو الكلمة المفتاحية |
| `icm_memory_update` | تعديل ذاكرة في مكانها (المحتوى، الأهمية، الكلمات المفتاحية) |
| `icm_memory_forget` | حذف ذاكرة بالمعرّف |
| `icm_memory_consolidate` | دمج جميع ذكريات موضوع واحد في ملخص |
| `icm_memory_list_topics` | سرد جميع المواضيع مع الأعداد |
| `icm_memory_stats` | إحصاءات الذاكرة الإجمالية |
| `icm_memory_health` | تدقيق نظافة المواضيع (القِدَم، الحاجة للدمج) |
| `icm_memory_embed_all` | ملء التضمينات للبحث الشعاعي بأثر رجعي |

### أدوات المذكرات (الرسوم البيانية للمعرفة)

| الأداة | الوصف |
|--------|-------|
| `icm_memoir_create` | إنشاء مذكرة جديدة (حاوية المعرفة) |
| `icm_memoir_list` | سرد جميع المذكرات |
| `icm_memoir_show` | عرض تفاصيل المذكرة وجميع المفاهيم |
| `icm_memoir_add_concept` | إضافة مفهوم مع تصنيفات |
| `icm_memoir_refine` | تحديث تعريف مفهوم |
| `icm_memoir_search` | بحث نصي كامل، مع تصفية اختيارية بحسب التصنيف |
| `icm_memoir_search_all` | البحث عبر جميع المذكرات |
| `icm_memoir_link` | إنشاء علاقة مكتوبة بين مفهومين |
| `icm_memoir_inspect` | فحص المفهوم والجوار في الرسم البياني (BFS) |
| `icm_memoir_export` | تصدير الرسم البياني (json, dot, ascii, ai) مع مستويات الثقة |

### أدوات التغذية الراجعة (التعلم من الأخطاء)

| الأداة | الوصف |
|--------|-------|
| `icm_feedback_record` | تسجيل تصحيح عند خطأ توقع الذكاء الاصطناعي |
| `icm_feedback_search` | البحث في التصحيحات السابقة لتوجيه التنبؤات المستقبلية |
| `icm_feedback_stats` | إحصاءات التغذية الراجعة: العدد الإجمالي، التوزيع بحسب الموضوع، الأكثر تطبيقًا |

### أنواع العلاقات

`part_of` · `depends_on` · `related_to` · `contradicts` · `refines` · `alternative_to` · `caused_by` · `instance_of` · `superseded_by`

## كيف يعمل

### نموذج الذاكرة المزدوج

**الذاكرة الحلقية (المواضيع)** تلتقط القرارات والأخطاء والتفضيلات. لكل ذكرى وزن يتلاشى مع الوقت بناءً على الأهمية:

| الأهمية | التلاشي | الحذف | السلوك |
|---------|--------|-------|--------|
| `critical` | لا يوجد | أبدًا | لا تُنسى أبدًا، ولا تُحذف |
| `high` | بطيء (0.5× المعدل) | أبدًا | تتلاشى ببطء، ولا تُحذف تلقائيًا |
| `medium` | عادي | نعم | تلاشٍ قياسي، تُحذف عند انخفاض الوزن عن الحد |
| `low` | سريع (2× المعدل) | نعم | تُنسى بسرعة |

التلاشي **واعٍ بالوصول**: الذكريات المُستعادة بكثرة تتلاشى بشكل أبطأ (`decay / (1 + access_count × 0.1)`). يُطبَّق تلقائيًا عند الاسترجاع (إذا مضى >24 ساعة منذ آخر تلاشٍ).

**نظافة الذاكرة** مدمجة:
- **إزالة التكرار التلقائي**: تخزين محتوى بتشابه >85% مع ذكرى موجودة في نفس الموضوع يُحدّثها بدلًا من إنشاء نسخة مكررة
- **تلميحات الدمج**: عندما يتجاوز موضوعٌ ما 7 مدخلات، يُنبّه `icm_memory_store` المستدعي بالدمج
- **تدقيق الصحة**: يُقدّم `icm_memory_health` تقريرًا بعدد المدخلات في كل موضوع، ومتوسط الوزن، والمدخلات القديمة، والحاجة للدمج
- **لا فقدان صامت للبيانات**: الذكريات الحرجة وعالية الأهمية لا تُحذف تلقائيًا أبدًا

**الذاكرة الدلالية (المذكرات)** تلتقط المعرفة المنظمة كرسم بياني. المفاهيم دائمة — تُحسَّن ولا تتلاشى. استخدم `superseded_by` للإشارة إلى الحقائق المتقادمة بدلًا من حذفها.

### البحث الهجين

عند تفعيل التضمينات، يستخدم ICM البحث الهجين:
- **FTS5 BM25** (30%) — مطابقة كلمات مفتاحية نصية كاملة
- **تشابه جيب التمام** (70%) — بحث شعاعي دلالي عبر sqlite-vec

النموذج الافتراضي: `intfloat/multilingual-e5-base` (768d، أكثر من 100 لغة). قابل للضبط في [ملف الضبط](#الضبط):

```toml
[embeddings]
# enabled = false                          # تعطيل كليًا (بدون تنزيل نموذج)
model = "intfloat/multilingual-e5-base"    # 768d، متعدد اللغات (الافتراضي)
# model = "intfloat/multilingual-e5-small" # 384d، متعدد اللغات (أخف)
# model = "intfloat/multilingual-e5-large" # 1024d، متعدد اللغات (أعلى دقة)
# model = "Xenova/bge-small-en-v1.5"      # 384d، إنجليزي فقط (الأسرع)
# model = "jinaai/jina-embeddings-v2-base-code"  # 768d، محسّن للكود
```

لتخطي تنزيل نموذج التضمين كليًا، استخدم أيًا مما يلي:
```bash
icm --no-embeddings serve          # علامة CLI
ICM_NO_EMBEDDINGS=1 icm serve     # متغير بيئة
```
أو اضبط `enabled = false` في ملف الضبط. سيعود ICM إلى البحث بكلمات مفتاحية FTS5 (يعمل مع ذلك، لكن بدون مطابقة دلالية).

تغيير النموذج يُعيد إنشاء فهرس الشعاعيات تلقائيًا (تُمسح التضمينات الموجودة ويمكن إعادة توليدها بـ `icm_memory_embed_all`).

### التخزين

ملف SQLite واحد. بدون خدمات خارجية، بدون تبعية على الشبكة.

```
~/Library/Application Support/dev.icm.icm/memories.db                    # macOS
~/.local/share/dev.icm.icm/memories.db                                   # Linux
C:\Users\<user>\AppData\Local\icm\icm\data\memories.db                   # Windows
```

### الضبط

```bash
icm config                    # عرض الضبط النشط
```

موقع ملف الضبط (خاص بكل منصة، أو `$ICM_CONFIG`):

```
~/Library/Application Support/dev.icm.icm/config.toml                    # macOS
~/.config/icm/config.toml                                                # Linux
C:\Users\<user>\AppData\Roaming\icm\icm\config\config.toml              # Windows
```

راجع [config/default.toml](config/default.toml) لجميع الخيارات.

## الاستخراج التلقائي

يستخرج ICM الذكريات تلقائيًا عبر ثلاث طبقات:

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

| الطبقة | الحالة | تكلفة LLM | أمر الخطاف | الوصف |
|--------|--------|-----------|-----------|-------|
| الطبقة 0 | مُنفَّذة | 0 | `icm hook post` | استخراج كلمات مفتاحية قائم على القواعد من مخرجات الأداة |
| الطبقة 1 | مُنفَّذة | 0 | `icm hook compact` | استخراج من النص قبل ضغط السياق |
| الطبقة 2 | مُنفَّذة | 0 | `icm hook prompt` | حقن الذكريات المُستعادة عند كل موجه مستخدم |

تُثبَّت الطبقات الثلاث تلقائيًا بـ `icm init --mode hook`.

### مقارنة مع البدائل

| النظام | الطريقة | تكلفة LLM | زمن الاستجابة | يلتقط الضغط؟ |
|--------|--------|-----------|---------|---------------------|
| **ICM** | استخراج بـ3 طبقات | 0 إلى ~500 رمز/جلسة | 0ms | **نعم (PreCompact)** |
| Mem0 | استدعاءان LLM/رسالة | ~2k رمز/رسالة | 200-2000ms | لا |
| claude-mem | PostToolUse + غير متزامن | ~1-5k رمز/جلسة | 8ms خطاف | لا |
| MemGPT/Letta | العميل يدير نفسه | 0 هامشية | 0ms | لا |
| DiffMem | فروق Git | 0 | 0ms | لا |

## المعايير

### أداء التخزين

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

Apple M1 Pro، SQLite في الذاكرة، خيط واحد. `icm bench --count 1000`

### كفاءة العميل

سير عمل متعدد الجلسات مع مشروع Rust حقيقي (12 ملفًا، ~550 سطرًا). الجلسات الثانية وما بعدها تُظهر أكبر المكاسب حيث يستعيد ICM بدلًا من إعادة قراءة الملفات.

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

### احتفاظ المعرفة

يستعيد العميل حقائق محددة من وثيقة تقنية مكثفة عبر الجلسات. الجلسة 1 تقرأ وتحفظ؛ الجلسات الثانية وما بعدها تجيب على 10 أسئلة واقعية **بدون** النص المصدر.

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

### النماذج المحلية (ollama)

نفس الاختبار مع النماذج المحلية — حقن السياق البحت، بدون حاجة لاستخدام الأدوات.

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

### بروتوكول الاختبار

جميع المعايير تستخدم **استدعاءات API حقيقية** — بدون محاكاة، بدون ردود وهمية، بدون إجابات مخزنة مؤقتًا.

- **معيار العميل**: يُنشئ مشروع Rust حقيقيًا في مجلد مؤقت. يُشغّل N جلسات مع `claude -p --output-format json`. بدون ICM: ضبط MCP فارغ. مع ICM: خادم MCP حقيقي + استخراج تلقائي + حقن السياق.
- **احتفاظ المعرفة**: يستخدم وثيقة تقنية خيالية ("بروتوكول ميريديان"). يُسجّل الإجابات بمطابقة الكلمات المفتاحية مع الحقائق المتوقعة. مهلة 120 ثانية لكل استدعاء.
- **العزل**: كل تشغيل يستخدم مجلده المؤقت الخاص وقاعدة بيانات SQLite جديدة. لا استمرارية للجلسة.

### ذاكرة موحدة متعددة العملاء

تتشارك جميع الأدوات الـ17 نفس قاعدة بيانات SQLite. الذاكرة المُخزَّنة بواسطة Claude تصبح متاحة فورًا لـ Gemini وCodex وCopilot وCursor وجميع الأدوات الأخرى.

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

النتيجة = 60% دقة الاسترجاع + 30% تفاصيل الحقائق + 10% السرعة. **98% كفاءة متعددة العملاء.**

## لماذا ICM

| القدرة | ICM | Mem0 | Engram | AgentMemory |
|--------|:---:|:----:|:------:|:-----------:|
| دعم الأدوات | **17** | SDK فقط | ~6-8 | ~10 |
| إعداد بأمر واحد | `icm init` | SDK يدوي | يدوي | يدوي |
| الخطافات (استرجاع تلقائي عند البدء) | 5 أدوات | لا يوجد | عبر MCP | أداة واحدة |
| بحث هجين (FTS5 + شعاعي) | 30/70 موزون | شعاعي فقط | FTS5 فقط | FTS5+شعاعي |
| تضمينات متعددة اللغات | 100+ لغة (768d) | حسب الحالة | لا يوجد | إنجليزي 384d |
| رسم بياني للمعرفة | نظام Memoir | لا يوجد | لا يوجد | لا يوجد |
| تلاشٍ زمني + دمج | واعٍ بالوصول | لا يوجد | أساسي | أساسي |
| لوحة تحكم TUI | `icm dashboard` | لا يوجد | نعم | عارض ويب |
| استخراج تلقائي من مخرجات الأداة | 3 طبقات، صفر LLM | لا يوجد | لا يوجد | لا يوجد |
| حلقة تغذية راجعة/تصحيح | `icm_feedback_*` | لا يوجد | لا يوجد | لا يوجد |
| بيئة التشغيل | Rust ملف تنفيذي واحد | Python | Go | Node.js |
| محلي أولًا، صفر تبعيات | ملف SQLite | سحابي أولًا | SQLite | SQLite |
| دقة استرجاع متعددة العملاء | **98%** | غ/م | غ/م | 95.2% |

## التوثيق

| الوثيقة | الوصف |
|---------|-------|
| [دليل التكامل](docs/integrations.md) | إعداد جميع الأدوات الـ17: Claude Code، Copilot، Cursor، Windsurf، Zed، Amp، إلخ |
| [البنية التقنية](docs/architecture.md) | هيكل الحزم، مسار البحث، نموذج التلاشي، تكامل sqlite-vec، الاختبار |
| [دليل المستخدم](docs/guide.md) | التثبيت، تنظيم المواضيع، الدمج، الاستخراج، استكشاف الأخطاء |
| [نظرة عامة على المنتج](docs/product.md) | حالات الاستخدام، المعايير، المقارنة مع البدائل |

## الترخيص

[المصدر المتاح](LICENSE) — مجاني للأفراد والفرق التي لا يتجاوز عددها 20 شخصًا. يُشترط ترخيص المؤسسات للمنظمات الأكبر. التواصل: contact@rtk-ai.app
