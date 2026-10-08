#!/bin/sh
# Runs INSIDE the agent container after the last turn, before the container is
# removed. Reads ICM's own telemetry so the result file can say whether the
# hooks really fired during the agent's turns, and what the agent stored.
# Read-only commands; sections start with "@@".
set -u

printf '@@hook_prompt_rows\n'
icm hook-log --event prompt --limit 200 2>/dev/null | grep -c ' prompt ' || true

printf '@@hook_start_rows\n'
icm hook-log --event start --limit 200 2>/dev/null | grep -c ' start ' || true

printf '@@hook_post_rows\n'
icm hook-log --event post --limit 1000 2>/dev/null | grep -c ' post ' || true

printf '@@hook_end_rows\n'
icm hook-log --event end --limit 200 2>/dev/null | grep -c ' end ' || true

printf '@@hook_stats\n'
icm hook-stats 2>&1 | head -n 40 || true

printf '@@stats\n'
icm stats 2>&1 || true

printf '@@topics\n'
icm topics 2>&1 | head -n 60 || true
