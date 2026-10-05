#!/bin/sh
# Runs INSIDE the agent container, before the agent's first turn. Replays one of
# ICM's two context-injecting hooks and prints exactly what the hook would hand
# to the agent:
#
#   start    SessionStart: the wake-up pack. Selected by importance and insertion
#            order, not by search; payload /tmp/icm-sde-probe-start.json
#   prompt   UserPromptSubmit: a search on the head of the prompt;
#            payload /tmp/icm-sde-probe-prompt.json
#
# The hook runs on a COPY of the database: the replay leaves no telemetry row and
# no access count in the database the agent will use, so the rows read at the
# end of the task (collect.sh) are the agent's own.
#
# Usage: sh -s -- <start|prompt> < probe.sh
set -eu

EVENT="${1:?usage: probe.sh start|prompt}"
case "$EVENT" in
  start|prompt) ;;
  *) printf 'unknown hook event: %s\n' "$EVENT" >&2; exit 2 ;;
esac
PAYLOAD="/tmp/icm-sde-probe-$EVENT.json"
COPY=/tmp/icm-sde-probe.db

[ -f "$PAYLOAD" ] || { printf 'probe payload missing: %s\n' "$PAYLOAD" >&2; exit 2; }
rm -f "$COPY" "$COPY-wal" "$COPY-shm"
for suffix in "" -wal -shm; do
  if [ -f "$ICM_DB$suffix" ]; then
    cp "$ICM_DB$suffix" "$COPY$suffix"
  fi
done

status=0
ICM_DB="$COPY" icm hook "$EVENT" < "$PAYLOAD" || status=$?
rm -f "$COPY" "$COPY-wal" "$COPY-shm"
exit "$status"
