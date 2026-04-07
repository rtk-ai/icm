# Changelog

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
