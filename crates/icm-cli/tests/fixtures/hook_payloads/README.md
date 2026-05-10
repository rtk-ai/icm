# Hook payload fixtures

Real PostToolUse / PreToolUse / etc. payloads as emitted by each AI agent
runtime. Pinned so any upstream format change breaks the regression tests
**at compile-test time** rather than silently turning auto-extraction off
for every user (issue surfaced 2026-05-10 — Claude Code 2.x switched from
top-level `tool_output` to nested `tool_response.output`, ICM kept reading
the old field, the store grew zero memories despite hooks firing).

## Capturing a fresh payload

```sh
# Wrap `icm hook post` to dump stdin to a file before running:
cat > /tmp/dump-and-fwd.sh <<'EOF'
#!/bin/sh
tee /tmp/icm-hook-stdin.json | /home/$USER/.local/bin/icm hook post
EOF
chmod +x /tmp/dump-and-fwd.sh

# Point Claude Code's PostToolUse hook at /tmp/dump-and-fwd.sh,
# trigger any tool, then move /tmp/icm-hook-stdin.json into this directory.
```

## Layout

| File | Source | Purpose |
|---|---|---|
| `claude_code_2x_post_tool.json` | Claude Code 2.x | Pin nested `tool_response.output` shape |
| `legacy_post_tool.json`         | Older clients   | Pin top-level `tool_output` shape |
| `tool_response_string.json`     | Codex variants  | Pin bare `tool_response: "string"` shape |

Add a new file when adding a new runtime — and ensure `extract_tool_output`
(in `crates/icm-cli/src/main.rs`) covers it via a regression test in
`crates/icm-cli/tests/hook_payload_fixtures.rs`.
