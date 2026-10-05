#!/bin/sh
# Runs INSIDE a throwaway container of the agent image, once per task, before
# any agent starts. Builds the task's memory database from the task's corpus
# with ICM's own import command: no LLM, no key, no network.
#
#   /corpus/NNNN.jsonl   a past developer conversation (the task's own chat or
#                        chats, the dataset's decoy conversations), in the
#                        Claude Code session format `icm import` reads
#   /corpus/NNNN.md      a commit message (the documented decision commit of a
#                        history task, host-history noise), plain text
#   /out                 receives memories.db, seed-export.jsonl, seed-report.txt
#
# NNNN is the import rank chosen on the host (icm_corpus.py). The names say
# nothing else: not which conversation is the task's, not which is a decoy.
#
# ICM_SDE_IMPORT selects the extractor of `icm import`:
#   default   ICM's own default: sentences scored with the local embedding
#             model, stored with their embeddings. Minutes per task.
#   rules     `--no-embeddings`: ICM's keyword rules, nothing embedded. Seconds
#             per task; the agent's own recalls are then keyword-only too.
# Neither calls an LLM.
#
# The database is built on the container's own filesystem and exported with
# `icm backup` (SQLite online backup: one checkpointed file, no -wal/-shm), so
# no SQLite file is ever written through a bind mount.
set -eu

PROJECT="${ICM_SDE_PROJECT:?ICM_SDE_PROJECT is required}"
case "${ICM_SDE_IMPORT:-default}" in
  default) NO_EMBEDDINGS="" ;;
  rules) NO_EMBEDDINGS="--no-embeddings" ;;
  *) printf 'ICM_SDE_IMPORT must be default or rules\n' >&2; exit 1 ;;
esac
WORK=/tmp/icm-sde-seed
DB="$WORK/memories.db"

command -v icm >/dev/null 2>&1 || { printf 'icm binary not found\n' >&2; exit 1; }
[ -d /corpus ] || { printf '/corpus is not mounted\n' >&2; exit 1; }
[ -d /out ] || { printf '/out is not mounted\n' >&2; exit 1; }

rm -rf "$WORK"
mkdir -p "$WORK"
REPORT=/out/seed-report.txt
: > "$REPORT"

{
  printf '@@icm_version\n'
  icm --version
  printf '@@project\n%s\n' "$PROJECT"
  printf '@@import_mode\n%s\n' "${ICM_SDE_IMPORT:-default}"
} >> "$REPORT"

# One pass over one directory, in file-name order: the order is the host's
# (icm_corpus.py), conversations and commit messages interleaved. The parser is
# picked per file from its extension (.jsonl session, .md text).
if [ -n "$(ls -A /corpus 2>/dev/null)" ]; then
  printf '@@import\n' >> "$REPORT"
  # shellcheck disable=SC2086  # one optional flag, or nothing
  icm --db "$DB" $NO_EMBEDDINGS import /corpus --project "$PROJECT" >> "$REPORT" 2>&1
fi

{
  printf '@@stats\n'
  icm --db "$DB" stats 2>&1 || true
  printf '@@topics\n'
  icm --db "$DB" topics 2>&1 | head -n 80 || true
} >> "$REPORT"

if [ -f "$DB" ]; then
  rm -f /out/memories.db /out/seed-export.jsonl
  icm --db "$DB" backup --output /out/memories.db >> "$REPORT" 2>&1
  # Every stored note, for the host to count the ones that come from the task's
  # own documents. Not needed by the agent: a failed export costs the count only.
  icm --db "$DB" export --output /out/seed-export.jsonl > /dev/null 2>&1 || rm -f /out/seed-export.jsonl
fi
[ -f /out/memories.db ] || { printf '@@error\nno database produced\n' >> "$REPORT"; exit 3; }
printf '@@ok\n1\n' >> "$REPORT"
