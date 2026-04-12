#!/usr/bin/env bash
# ICM PostToolUse hook for Claude Code
# Counts tool calls and auto-extracts context every N calls.
# Install: icm init --mode hook (or manually add to ~/.claude/settings.json)
#
# Input (stdin): JSON with tool_name, tool_input, tool_output, etc.
# Output: nothing (PostToolUse hooks are fire-and-forget)

# NOTE: The Rust binary (icm hook post) now persists the hook counter in SQLite
# (icm_metadata table) for reliable, atomic, reboot-safe counting.
# This shell script retains the file-based /tmp counter for standalone/legacy usage.

set -euo pipefail

# Config
EXTRACT_EVERY=15           # Extract every N tool calls
COUNTER_FILE="${ICM_HOOK_COUNTER:-/tmp/icm-hook-counter}"
ICM_BIN="${ICM_BIN:-icm}"

# Read hook input
INPUT=$(cat)
TOOL_NAME=$(echo "$INPUT" | jq -r '.tool_name // empty' 2>/dev/null)

# Skip ICM's own tools (avoid infinite loop)
case "$TOOL_NAME" in
  icm_*|mcp__icm__*) exit 0 ;;
esac

# Increment counter
COUNT=0
if [ -f "$COUNTER_FILE" ]; then
  COUNT=$(cat "$COUNTER_FILE" 2>/dev/null || echo "0")
fi
COUNT=$((COUNT + 1))
echo "$COUNT" > "$COUNTER_FILE"

# Reset counter on icm_memory_store (agent stored voluntarily)
if [ "$TOOL_NAME" = "icm_memory_store" ] || [ "$TOOL_NAME" = "mcp__icm__icm_memory_store" ]; then
  echo "0" > "$COUNTER_FILE"
  exit 0
fi

# Not time to extract yet
if [ "$COUNT" -lt "$EXTRACT_EVERY" ]; then
  exit 0
fi

# Reset counter
echo "0" > "$COUNTER_FILE"

# Extract from tool output if available
TOOL_OUTPUT=$(echo "$INPUT" | jq -r '.tool_output // empty' 2>/dev/null)
if [ -z "$TOOL_OUTPUT" ]; then
  exit 0
fi

# Get project name from cwd
PROJECT=$(basename "$(pwd)" 2>/dev/null || echo "project")

# Extract facts and store (async, don't block the agent)
echo "$TOOL_OUTPUT" | "$ICM_BIN" extract -p "$PROJECT" 2>/dev/null &

exit 0
