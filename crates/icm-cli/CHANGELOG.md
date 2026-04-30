# Changelog

## [0.10.42](https://github.com/rtk-ai/icm/compare/icm-v0.10.41...icm-v0.10.42) (2026-04-30)


### Bug Fixes

* **audit-batch-13:** chronological transcript assembly + filter post-expand ([#163](https://github.com/rtk-ai/icm/issues/163)) ([cc74a5c](https://github.com/rtk-ai/icm/commit/cc74a5c44603ec11a23191eea1b863b78b0687ca))

## [0.10.41](https://github.com/rtk-ai/icm/compare/icm-v0.10.40...icm-v0.10.41) (2026-04-30)


### Bug Fixes

* **audit-batch-12:** prune dry-run alignment + transcript code-fence + recall --project ([#161](https://github.com/rtk-ai/icm/issues/161)) ([8918304](https://github.com/rtk-ai/icm/commit/8918304613799f095b9a89f73f77f3749b94d4c8))

## [0.10.40](https://github.com/rtk-ai/icm/compare/icm-v0.10.39...icm-v0.10.40) (2026-04-30)


### Features

* **init:** default to CLI-only integration (no MCP) ([#158](https://github.com/rtk-ai/icm/issues/158)) ([fcfbb73](https://github.com/rtk-ai/icm/commit/fcfbb73b72383ea75b0f050c1cb4adea0c02f362))

## [0.10.39](https://github.com/rtk-ai/icm/compare/icm-v0.10.38...icm-v0.10.39) (2026-04-30)


### Bug Fixes

* **deps:** restore vendored-openssl feature for cross-build pipeline ([#156](https://github.com/rtk-ai/icm/issues/156)) ([6493e87](https://github.com/rtk-ai/icm/commit/6493e8702ad284bd18bc8f5de54b45579e862aaf))

## [0.10.38](https://github.com/rtk-ai/icm/compare/icm-v0.10.37...icm-v0.10.38) (2026-04-30)


### Bug Fixes

* **deps:** audit batch 10 — drop tokio "full" + remove dead openssl dep ([#154](https://github.com/rtk-ai/icm/issues/154)) ([fbc3aa5](https://github.com/rtk-ai/icm/commit/fbc3aa514d56d2b56ebde133cff973a9f03c9ee8))

## [0.10.37](https://github.com/rtk-ai/icm/compare/icm-v0.10.36...icm-v0.10.37) (2026-04-30)


### Bug Fixes

* **docs,cli:** audit batch 9 — extract_every help text + 12 README count drifts ([#152](https://github.com/rtk-ai/icm/issues/152)) ([834ddd3](https://github.com/rtk-ai/icm/commit/834ddd3197c7ed259d746eccd485e650fdc0da2e))

## [0.10.36](https://github.com/rtk-ai/icm/compare/icm-v0.10.35...icm-v0.10.36) (2026-04-30)


### Bug Fixes

* **consistency:** audit batch 8 — auto-consolidate paths, embedding, cycles, feedback list ([#150](https://github.com/rtk-ai/icm/issues/150)) ([004458a](https://github.com/rtk-ai/icm/commit/004458a1831e757c630f475f345ddaf6abd5e671))

## [0.10.35](https://github.com/rtk-ai/icm/compare/icm-v0.10.34...icm-v0.10.35) (2026-04-30)


### Bug Fixes

* **robustness:** audit batch 7 — versions, honorifics, wake-up cap, UTF-8 slice ([#148](https://github.com/rtk-ai/icm/issues/148)) ([edc0d3b](https://github.com/rtk-ai/icm/commit/edc0d3b6404c65759813c0ac7566a1810c8ced9d))

## [0.10.34](https://github.com/rtk-ai/icm/compare/icm-v0.10.33...icm-v0.10.34) (2026-04-30)


### Bug Fixes

* **security,robustness:** audit batch 6 — PreToolUse chain bypass + 4 robustness gaps ([#146](https://github.com/rtk-ai/icm/issues/146)) ([49c9789](https://github.com/rtk-ai/icm/commit/49c978960ad7f5e878ffc44f74771b35fe20ac84))

## [0.10.33](https://github.com/rtk-ai/icm/compare/icm-v0.10.32...icm-v0.10.33) (2026-04-30)


### Bug Fixes

* **extract:** URL/path/version-aware sentence splitter ([#144](https://github.com/rtk-ai/icm/issues/144)) ([cde26df](https://github.com/rtk-ai/icm/commit/cde26df910adc5a92fe6f22af782e42f42146a8e))

## [0.10.32](https://github.com/rtk-ai/icm/compare/icm-v0.10.31...icm-v0.10.32) (2026-04-29)


### Bug Fixes

* audit batch 1 — security, panics, and cross-platform paths ([#141](https://github.com/rtk-ai/icm/issues/141)) ([181c6dd](https://github.com/rtk-ai/icm/commit/181c6dd4b051b48165ef1eae4132710da576013b))

## [0.10.31](https://github.com/rtk-ai/icm/compare/icm-v0.10.30...icm-v0.10.31) (2026-04-29)


### Bug Fixes

* **cli:** honor XXX_HOME env vars and move Copilot hooks to user dir ([#138](https://github.com/rtk-ai/icm/issues/138)) ([0647926](https://github.com/rtk-ai/icm/commit/0647926e5c8969f5cd0c21455bcafe7a6b66070c))

## [0.10.30](https://github.com/rtk-ai/icm/compare/icm-v0.10.29...icm-v0.10.30) (2026-04-29)


### Features

* **hook:** add SessionEnd handler so /exit and /clear flush memories ([#132](https://github.com/rtk-ai/icm/issues/132)) ([95cd969](https://github.com/rtk-ai/icm/commit/95cd969b2ef5663dab5c8c2fcdc085f8f5c59561))

## [0.10.29](https://github.com/rtk-ai/icm/compare/icm-v0.10.28...icm-v0.10.29) (2026-04-29)


### Bug Fixes

* **cli:** convert remaining &PathBuf params to &Path (clippy::ptr_arg) ([dcf45fb](https://github.com/rtk-ai/icm/commit/dcf45fbbe2ea70475ab25f670e46468cbeebd6a3))
* **cli:** list 'hook' as a valid --mode value in icm init --help ([9a54782](https://github.com/rtk-ai/icm/commit/9a5478296777e30f2fa91eb6fa68725efec41154))
* **cli:** list 'hook' as a valid --mode value in icm init --help ([10a7aaf](https://github.com/rtk-ai/icm/commit/10a7aaf8d7e74d7106084f4430bd25514c504b06))
* **cli:** satisfy clippy::ptr_arg on detect_tool's vscode_data param ([d733e54](https://github.com/rtk-ai/icm/commit/d733e541c3e00c9225fdae19d134e0125a0b3b3e))
* **hook:** filter recall_context by project to stop cross-project leakage ([#130](https://github.com/rtk-ai/icm/issues/130)) ([3af4e91](https://github.com/rtk-ai/icm/commit/3af4e91986c1c0f90464d2e3f0925abb66776b26))

## [0.10.26](https://github.com/rtk-ai/icm/compare/icm-v0.10.25...icm-v0.10.26) (2026-04-18)


### Features

* **init:** add --force flag and doctor command for hook hygiene ([#116](https://github.com/rtk-ai/icm/issues/116)) ([2c769f9](https://github.com/rtk-ai/icm/commit/2c769f94958e30ba8a211e73b6c7e361cf3ad901))

## [0.10.25](https://github.com/rtk-ai/icm/compare/icm-v0.10.24...icm-v0.10.25) (2026-04-16)


### Bug Fixes

* **cli:** apply truncate_at_char_boundary to remaining byte-slicing sites ([#113](https://github.com/rtk-ai/icm/issues/113)) ([17ba38c](https://github.com/rtk-ai/icm/commit/17ba38cd1b640794b357b0b28aab563690003ef1))
* **hook-prompt:** don't panic on multi-byte UTF-8 query truncation ([#111](https://github.com/rtk-ai/icm/issues/111)) ([16b9631](https://github.com/rtk-ai/icm/commit/16b963127f1be87f482eb0a609507a6dd010e17f)), closes [#110](https://github.com/rtk-ai/icm/issues/110)

## [0.10.24](https://github.com/rtk-ai/icm/compare/icm-v0.10.23...icm-v0.10.24) (2026-04-13)


### Features

* verbatim transcripts — sessions + messages (refs [#107](https://github.com/rtk-ai/icm/issues/107)) ([#108](https://github.com/rtk-ai/icm/issues/108)) ([85cad66](https://github.com/rtk-ai/icm/commit/85cad663a36d05c23a71f8180a1fee85d038119b))

## [0.10.23](https://github.com/rtk-ai/icm/compare/icm-v0.10.22...icm-v0.10.23) (2026-04-12)


### Bug Fixes

* **upgrade:** refuse to upgrade Homebrew-managed binary ([#105](https://github.com/rtk-ai/icm/issues/105)) ([d6a90e5](https://github.com/rtk-ai/icm/commit/d6a90e53616f39062db48d42804797b79ece049d))

## [0.10.22](https://github.com/rtk-ai/icm/compare/icm-v0.10.21...icm-v0.10.22) (2026-04-12)


### Features

* **security:** add SHA256 checksum verification + icm upgrade --apply ([#103](https://github.com/rtk-ai/icm/issues/103)) ([3aa18b1](https://github.com/rtk-ai/icm/commit/3aa18b12c78ce63d16d411df9d2751365742e38d))

## [0.10.21](https://github.com/rtk-ai/icm/compare/icm-v0.10.20...icm-v0.10.21) (2026-04-12)


### Bug Fixes

* persist hook counter in SQLite instead of /tmp file ([#101](https://github.com/rtk-ai/icm/issues/101)) ([a256a85](https://github.com/rtk-ai/icm/commit/a256a85e49bcdb0bebe2d325110bb9f58b9d9790))
* **zed:** use correct settings format ([#88](https://github.com/rtk-ai/icm/issues/88)) ([0dd366d](https://github.com/rtk-ai/icm/commit/0dd366d7834acebe662593c4640bbb0ccc8aaa5c))

## [0.10.20](https://github.com/rtk-ai/icm/compare/icm-v0.10.19...icm-v0.10.20) (2026-04-12)


### Features

* add web dashboard with Svelte frontend ([#99](https://github.com/rtk-ai/icm/issues/99)) ([d3ea043](https://github.com/rtk-ai/icm/commit/d3ea04317127248359bd16724b43a72b3eb35348))

## [0.10.19](https://github.com/rtk-ai/icm/compare/icm-v0.10.18...icm-v0.10.19) (2026-04-12)


### Features

* add Continue.dev MCP + Aider CLI support ([#95](https://github.com/rtk-ai/icm/issues/95)) ([1087917](https://github.com/rtk-ai/icm/commit/10879172c4a390143f9ca0d03a213124471e31f5))

## [0.10.18](https://github.com/rtk-ai/icm/compare/icm-v0.10.17...icm-v0.10.18) (2026-04-12)


### Bug Fixes

* improve recall coverage for Claude and Codex agents ([#93](https://github.com/rtk-ai/icm/issues/93)) ([1e7c562](https://github.com/rtk-ai/icm/commit/1e7c562e4134c9f447334190802f6c6b044526b4))

## [0.10.17](https://github.com/rtk-ai/icm/compare/icm-v0.10.16...icm-v0.10.17) (2026-04-12)


### Features

* add graph-aware recall, auto-linking, and multi-tool hooks ([#91](https://github.com/rtk-ai/icm/issues/91)) ([20ae926](https://github.com/rtk-ai/icm/commit/20ae9264e44509109a38d38f9fbc5734c3e4a597))

## [0.10.16](https://github.com/rtk-ai/icm/compare/icm-v0.10.15...icm-v0.10.16) (2026-04-11)


### Features

* add SessionStart Claude Code hook that injects wake-up pack ([#86](https://github.com/rtk-ai/icm/issues/86)) ([23692e2](https://github.com/rtk-ai/icm/commit/23692e2caebb9f27bb06655a739e04c3e6e785ac))

## [0.10.15](https://github.com/rtk-ai/icm/compare/icm-v0.10.14...icm-v0.10.15) (2026-04-11)


### Features

* add `icm wake-up` command + `icm_wake_up` MCP tool ([#84](https://github.com/rtk-ai/icm/issues/84)) ([51a1081](https://github.com/rtk-ai/icm/commit/51a1081a1b988a02686acc1a9d3b9988e26359a7))

## [0.10.14](https://github.com/rtk-ai/icm/compare/icm-v0.10.13...icm-v0.10.14) (2026-04-09)


### Bug Fixes

* preserve JSON key order in config files ([#80](https://github.com/rtk-ai/icm/issues/80)) ([4dc4e83](https://github.com/rtk-ai/icm/commit/4dc4e83bbfe00cd125f780229cbbcddb3ad6bbea))

## [0.10.13](https://github.com/rtk-ai/icm/compare/icm-v0.10.12...icm-v0.10.13) (2026-04-07)


### Features

* icm import — conversations from Claude, ChatGPT, Slack ([#78](https://github.com/rtk-ai/icm/issues/78)) ([6f036c8](https://github.com/rtk-ai/icm/commit/6f036c80cc120dd2561d8c8a8eb6200410f92c7e))

## [0.10.12](https://github.com/rtk-ai/icm/compare/icm-v0.10.11...icm-v0.10.12) (2026-04-07)


### Features

* add MCP icm_learn tool + forget --topic ([#72](https://github.com/rtk-ai/icm/issues/72)) ([#76](https://github.com/rtk-ai/icm/issues/76)) ([60420a9](https://github.com/rtk-ai/icm/commit/60420a96fc4757c7cd51cd4dfef37f7bb3ed62d8))

## [0.10.11](https://github.com/rtk-ai/icm/compare/icm-v0.10.10...icm-v0.10.11) (2026-04-07)


### Features

* add icm learn command — scan project into memoir knowledge graph ([#72](https://github.com/rtk-ai/icm/issues/72)) ([#74](https://github.com/rtk-ai/icm/issues/74)) ([0801e7f](https://github.com/rtk-ai/icm/commit/0801e7ff9239040bc0b69030fbb8a5237a8e422b))

## [0.10.10](https://github.com/rtk-ai/icm/compare/icm-v0.10.9...icm-v0.10.10) (2026-04-06)


### Features

* add recall-project and save-project commands ([#70](https://github.com/rtk-ai/icm/issues/70)) ([51185fe](https://github.com/rtk-ai/icm/commit/51185fe4614764794ae9b5fd7b0f028ae9a70c3e)), closes [#69](https://github.com/rtk-ai/icm/issues/69)

## [0.10.9](https://github.com/rtk-ai/icm/compare/icm-v0.10.8...icm-v0.10.9) (2026-04-06)


### Bug Fixes

* ensure OpenCode plugin update triggers release ([2f33879](https://github.com/rtk-ai/icm/commit/2f338791436c8f1ce4164348a835e0e5e95f8bf5))

## [0.10.8](https://github.com/rtk-ai/icm/compare/icm-v0.10.7...icm-v0.10.8) (2026-04-06)


### Bug Fixes

* rewrite OpenCode plugin with native @opencode-ai/plugin SDK ([#64](https://github.com/rtk-ai/icm/issues/64)) ([#65](https://github.com/rtk-ai/icm/issues/65)) ([a74a94d](https://github.com/rtk-ai/icm/commit/a74a94df5e878358b211b3968fa1e4f43c26373d))

## [0.10.7](https://github.com/rtk-ai/icm/compare/icm-v0.10.6...icm-v0.10.7) (2026-04-06)


### Bug Fixes

* lower extraction threshold + configurable settings ([#61](https://github.com/rtk-ai/icm/issues/61)) ([#62](https://github.com/rtk-ai/icm/issues/62)) ([f8f904d](https://github.com/rtk-ai/icm/commit/f8f904de3c631a9218da5f77b4c65c687886ab78))

## [0.10.6](https://github.com/rtk-ai/icm/compare/icm-v0.10.5...icm-v0.10.6) (2026-04-06)


### Features

* add Copilot/Windsurf CLI instructions + integration docs ([#55](https://github.com/rtk-ai/icm/issues/55)) ([344df6a](https://github.com/rtk-ai/icm/commit/344df6a3935a4da2157c33868708a4a27d9d321b))


### Bug Fixes

* JSONC parsing in init + OpenCode plugin extraction ([#57](https://github.com/rtk-ai/icm/issues/57), [#58](https://github.com/rtk-ai/icm/issues/58)) ([#60](https://github.com/rtk-ai/icm/issues/60)) ([a3a7967](https://github.com/rtk-ai/icm/commit/a3a796717ff815dcc8456fb41811169b8b2d56ef))

## [0.10.5](https://github.com/rtk-ai/icm/compare/icm-v0.10.4...icm-v0.10.5) (2026-03-21)


### Bug Fixes

* comprehensive audit fixes — security, performance, tech debt, tests ([#51](https://github.com/rtk-ai/icm/issues/51)) ([c3555c9](https://github.com/rtk-ai/icm/commit/c3555c9bdceb2d66829a724a7cd4aeb6bb0c6c52))

## [0.10.4](https://github.com/rtk-ai/icm/compare/icm-v0.10.3...icm-v0.10.4) (2026-03-21)


### Features

* add .deb and .rpm packages, drop Windows ARM64 ([#47](https://github.com/rtk-ai/icm/issues/47)) ([c7f8775](https://github.com/rtk-ai/icm/commit/c7f87750a78d91907757e02a3e0a7fa2caa79d5e))

## [0.10.3](https://github.com/rtk-ai/icm/compare/icm-v0.10.2...icm-v0.10.3) (2026-03-17)


### Features

* make ICM store instructions mandatory with explicit triggers ([7a60af3](https://github.com/rtk-ai/icm/commit/7a60af33d3bbc87760e89886f71e631c882166a2))
* mandatory ICM store triggers for all AI tools ([2d0298a](https://github.com/rtk-ai/icm/commit/2d0298a19bbe549f07aff96ab9ced11c6432f58d))

## [0.10.2](https://github.com/rtk-ai/icm/compare/icm-v0.10.1...icm-v0.10.2) (2026-03-17)


### Bug Fixes

* blob validation, Vec pre-alloc, replace collect-all-topics N+1 ([9a8a435](https://github.com/rtk-ai/icm/commit/9a8a435016d3a7ae4c098457d2203786b09df0bf))
* harden input bounds, clamp confidence, deduplicate helpers ([faa3651](https://github.com/rtk-ai/icm/commit/faa36514392c0e95c36543199bf53b95e5fea2be))


### Performance Improvements

* fix N+1 queries in memoir operations and dedup inject_claude_hook ([161900b](https://github.com/rtk-ai/icm/commit/161900b71a40cd5e41584305fb244757bf77c0b1))

## [0.10.1](https://github.com/rtk-ai/icm/compare/icm-v0.10.0...icm-v0.10.1) (2026-03-16)


### Features

* add CLI commands for all MCP-only tools (update, health, feedback) ([#16](https://github.com/rtk-ai/icm/issues/16)) ([157b3e0](https://github.com/rtk-ai/icm/commit/157b3e0a7439f0e05950c3b0d9b2a26fa9075bc8))
* add conversational extraction patterns for PreCompact ([#27](https://github.com/rtk-ai/icm/issues/27)) ([b621895](https://github.com/rtk-ai/icm/commit/b6218952dab5960d713b1e3b05370a4591b90f72)), closes [#11](https://github.com/rtk-ai/icm/issues/11)
* add executable actions to TUI dashboard ([#31](https://github.com/rtk-ai/icm/issues/31)) ([56a0811](https://github.com/rtk-ai/icm/commit/56a0811858a2a7c2ed5166d1b4ba1c380cf2627c))
* add interactive TUI dashboard (icm dashboard / icm tui) ([#30](https://github.com/rtk-ai/icm/issues/30)) ([085204a](https://github.com/rtk-ai/icm/commit/085204aeaa227325d03f077132a8369afe8bb9c6)), closes [#21](https://github.com/rtk-ai/icm/issues/21)
* add OpenCode plugin for auto-extraction (layers 0/1/2) ([#28](https://github.com/rtk-ai/icm/issues/28)) ([1764a22](https://github.com/rtk-ai/icm/commit/1764a22e4ab4b594fbc46a0d91d8ad1b3bbe34e1)), closes [#14](https://github.com/rtk-ai/icm/issues/14)
* add PreToolUse hook for CLI-first multi-tool support ([#17](https://github.com/rtk-ai/icm/issues/17)) ([f6f32c7](https://github.com/rtk-ai/icm/commit/f6f32c7407155725e239a50d6a6fd2115ed28767))
* add RTK Cloud sync for shared memories ([4943141](https://github.com/rtk-ai/icm/commit/4943141b075629ad0c7a5a97bb6627136091a4f2))
* auto-consolidation, agent memory scope, pattern extraction ([c2cbea9](https://github.com/rtk-ai/icm/commit/c2cbea94c074a86a9806a641af9d00eef48e706a))
* compact mode on by default, shorter MCP instructions ([60cc469](https://github.com/rtk-ai/icm/commit/60cc46984f6533f0153d239b3e22906794136190))
* full Rust hooks, no shell scripts ([#18](https://github.com/rtk-ai/icm/issues/18)) ([caebfd1](https://github.com/rtk-ai/icm/commit/caebfd1226956fa2a01e8a1cc63f9bbe8801da6c))
* install all 3 hook layers via icm init --mode hook ([#26](https://github.com/rtk-ai/icm/issues/26)) ([d79e1bc](https://github.com/rtk-ai/icm/commit/d79e1bc0c6acda4816852e3b5d0de45f27c189ee)), closes [#9](https://github.com/rtk-ai/icm/issues/9)
* memoir graph export (JSON, DOT, ASCII, AI) with confidence levels ([#24](https://github.com/rtk-ai/icm/issues/24)) ([f548fe5](https://github.com/rtk-ai/icm/commit/f548fe5bd267ac9f875003d85ef0458446b3b6f3))
* RTK Cloud compat — X-Org-Id header, rtk-pro credential fallback, email login ([d13a469](https://github.com/rtk-ai/icm/commit/d13a4691a6af3963ea344b20296f29b7ca063296))
* RTK Cloud sync compatibility ([bb1b2dc](https://github.com/rtk-ai/icm/commit/bb1b2dcc65b495c102690d3279b753de10f704ec))
* write ICM instructions to each tool's native file ([#19](https://github.com/rtk-ai/icm/issues/19)) ([b786240](https://github.com/rtk-ai/icm/commit/b7862406cbe4e11cc6c717bc3277ee8d8393b446))


### Bug Fixes

* add opt-out for embedding model download ([#25](https://github.com/rtk-ai/icm/issues/25)) ([5334d45](https://github.com/rtk-ai/icm/commit/5334d45e220914a2f993b600672a8761dccd1b41)), closes [#8](https://github.com/rtk-ai/icm/issues/8)
* cross-platform config path (Windows/Linux/macOS) ([12aede9](https://github.com/rtk-ai/icm/commit/12aede99a9e2cfc109a1906d7576409aff37f026))
* cross-platform credentials path and password input ([09188f3](https://github.com/rtk-ai/icm/commit/09188f331fbb55ea514aa87b2a97dc4a061cda9f))
* improve extract fact scoring for dev tool outputs ([#3](https://github.com/rtk-ai/icm/issues/3)) ([037dfc4](https://github.com/rtk-ai/icm/commit/037dfc4994a2f5aea1381584e0a216d2d057e738))
* use correct Zed context_servers format (command.path instead of flat command) ([f2c0ea6](https://github.com/rtk-ai/icm/commit/f2c0ea6dba465d4cc4193cc28dbfbca955da2a16))
* use cross-platform config path via directories crate ([a292ee9](https://github.com/rtk-ai/icm/commit/a292ee9b2303893224acea802ac94f78decd05c4)), closes [#22](https://github.com/rtk-ai/icm/issues/22)
