# Changelog

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
