#!/bin/sh
# Runs INSIDE the agent container, piped by icm_sde_run.py right after the
# container starts and before the agent's first turn.
#
# It installs ICM the way a user does: `icm init`. Nothing here writes a hook,
# an MCP entry or an instruction file by hand; the script only checks that
# `icm init` produced them, and fails the task when it did not, so an arm
# without memory wiring is never scored as a memory arm.
#
# Output: sections introduced by a line starting with "@@", parsed by the
# harness wrapper. Exit status 0 = wiring verified.
#
# Usage: sh -s -- <claude-code|codex|opencode> < setup.sh
set -eu

AGENT="${1:-${ICM_SDE_AGENT:-claude-code}}"
MODE="${ICM_SDE_INIT_MODE:-all}"

fail() {
  printf '@@error\n%s\n' "$1"
  exit 1
}

has() {
  # has <file> <fixed string>
  [ -f "$1" ] && grep -qF -- "$2" "$1"
}

command -v icm >/dev/null 2>&1 || fail "icm binary not found in the agent image"
[ -n "${ICM_DB:-}" ] || fail "ICM_DB is not set in the container environment"
[ -n "${HOME:-}" ] || fail "HOME is not set"
mkdir -p "$(dirname "$ICM_DB")"

case "$AGENT" in
  claude-code) BIN=claude ;;
  codex) BIN=codex ;;
  opencode) BIN=opencode ;;
  *) fail "unknown agent: $AGENT" ;;
esac
# `icm init` configures the tools it detects on PATH. Without the agent binary
# it would print "skipped (not detected)" and exit 0.
command -v "$BIN" >/dev/null 2>&1 || fail "$BIN is not on PATH: icm init would skip it"

printf '@@icm_version\n'
icm --version

printf '@@agent_version\n'
"$BIN" --version 2>/dev/null | head -n 1 || printf 'unknown\n'

printf '@@seed\n'
if [ -f "$ICM_DB" ]; then
  printf 'present %s bytes\n' "$(wc -c < "$ICM_DB" | tr -d ' ')"
else
  printf 'absent\n'
fi

printf '@@init\n'
icm init --mode "$MODE" 2>&1 || fail "icm init --mode $MODE failed"

# Codex only runs hooks when its feature flag is on. The upstream harness sets
# the same flag for its reference memory arm; `icm init` does not write it.
# It must be a top-level key, so it goes BEFORE the [mcp_servers.icm] table
# that `icm init` appended (a key appended after a table belongs to that table).
if [ "$AGENT" = codex ]; then
  CFG="$HOME/.codex/config.toml"
  mkdir -p "$HOME/.codex"
  if ! has "$CFG" "codex_hooks"; then
    { printf 'codex_hooks = true\n'; [ -f "$CFG" ] && cat "$CFG"; } > "$CFG.icm-sde" || true
    mv "$CFG.icm-sde" "$CFG"
  fi
fi

# Who extracts facts in ICM's write hooks. Anything but `none` makes the
# SessionEnd hook call the agent's CLI on its own (a model call the harness
# does not count); the image sets `none` (Dockerfile.agent-icm).
WANT_EXTRACTION="${ICM_SDE_EXTRACTION:-none}"
EXTRACTION="$(icm config 2>/dev/null \
  | sed -n '/^\[extraction\.summarizer\]/,/^$/s/^ *provider *= *//p' | head -n 1)"
printf '@@extraction\n%s\n' "${EXTRACTION:-unknown}"

printf '@@wiring\n'
MISSING=""
if [ "$EXTRACTION" = "$WANT_EXTRACTION" ]; then
  printf 'ok extraction-%s\n' "$WANT_EXTRACTION"
else
  printf 'MISSING extraction-%s (icm config says: %s)\n' "$WANT_EXTRACTION" "${EXTRACTION:-unknown}"
  MISSING="$MISSING extraction-$WANT_EXTRACTION"
fi
need() {
  # need <label> <file> <fixed string>
  if has "$2" "$3"; then
    printf 'ok %s\n' "$1"
  else
    printf 'MISSING %s (%s)\n' "$1" "$2"
    MISSING="$MISSING $1"
  fi
}
case "$AGENT" in
  claude-code)
    S="$HOME/.claude/settings.json"
    need hook-prompt "$S" "hook prompt"
    need hook-start "$S" "hook start"
    need hook-post "$S" "hook post"
    need hook-pre "$S" "hook pre"
    need hook-end "$S" "hook end"
    # The image's allow-list must survive `icm init`. Not `"Bash"`: `icm init`
    # writes that string itself, as the matcher of its PreToolUse hook.
    need permissions-kept "$S" '"defaultMode"'
    need instructions "$HOME/.claude/CLAUDE.md" "icm:start"
    case "$MODE" in
      all|mcp)
        need mcp-server "$HOME/.claude.json" '"icm"'
        need mcp-allowed "$S" "mcp__icm"
        ;;
    esac
    ;;
  codex)
    need hook-prompt "$HOME/.codex/hooks.json" "hook prompt"
    need hook-start "$HOME/.codex/hooks.json" "hook start"
    need hooks-enabled "$HOME/.codex/config.toml" "codex_hooks"
    need instructions "$HOME/.codex/AGENTS.md" "icm:start"
    case "$MODE" in
      all|mcp) need mcp-server "$HOME/.codex/config.toml" "mcp_servers.icm" ;;
    esac
    ;;
  opencode)
    need plugin "$HOME/.config/opencode/plugins/icm.ts" "icm"
    case "$MODE" in
      all|mcp) need mcp-server "$HOME/.config/opencode/opencode.json" '"icm"' ;;
    esac
    ;;
esac

# MCP handshake without any model: initialize, then list the tools.
case "$MODE" in
  all|mcp)
    printf '@@mcp_tools\n'
    printf '%s\n' \
      '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"icm-sde-preflight","version":"0"}}}' \
      '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
      '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' \
      | timeout 120 icm serve 2>/dev/null \
      | grep -o '"name": *"icm_[a-z_]*"' | tr -d ' ' | sort -u | tr '\n' ' ' || true
    printf '\n'
    ;;
esac

printf '@@stats\n'
icm stats 2>&1 || true

printf '@@topics\n'
icm topics 2>&1 | head -n 60 || true

[ -z "$MISSING" ] || fail "icm init left the wiring incomplete:$MISSING"
printf '@@ok\n1\n'
