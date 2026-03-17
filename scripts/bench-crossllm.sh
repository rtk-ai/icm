#!/bin/bash
# Cross-LLM Memory Benchmark
# Simulates a real fullstack project (React + Rust API)
# where multiple AI tools work on the same codebase across sessions.
#
# Session 1 (Claude): Sets up the project, makes architecture decisions
# Session 2 (Gemini): Continues development, benefits from Claude's context
# Session 3 (Claude): Comes back to debug, benefits from all previous context
#
# Measures: Can each tool answer project-specific questions
# that were only discussed in OTHER tools' sessions?

set -euo pipefail

ICM="${ICM_BIN:-/opt/homebrew/bin/icm}"
TOPIC="bench-fullstack-$(date +%s)"
SCORE_WITH=0
SCORE_WITHOUT=0
TOTAL=0

MCP_CONFIG=$(mktemp /tmp/icm-bench-mcp-XXXX.json)
cat > "$MCP_CONFIG" <<EOF
{
  "mcpServers": {
    "icm": {
      "command": "$ICM",
      "args": ["serve"]
    }
  }
}
EOF

echo "══════════════════════════════════════════════════════════"
echo "  Cross-LLM Memory Benchmark"
echo "  Project: TaskFlow (React + Bun frontend, Rust API)"
echo "══════════════════════════════════════════════════════════"
echo ""

# ─────────────────────────────────────────────────
# SESSION 1: Claude works on the project
# Architecture decisions, initial setup
# ─────────────────────────────────────────────────
echo "=== Session 1: Claude — Project setup & architecture ==="
echo ""

$ICM store -t "$TOPIC" -c "Project TaskFlow: React frontend with Bun, Rust backend with Axum. Monorepo structure: apps/web (React+Vite+Bun), apps/api (Rust+Axum+SQLx). Shared types in packages/shared-types as TypeScript." -i high -k "architecture,taskflow,react,rust,axum,bun"
echo "  [stored] Project structure: monorepo apps/web + apps/api"

$ICM store -t "$TOPIC" -c "State management: Zustand (not Redux). Reason: simpler API, no boilerplate, works well with React Server Components. Store split by domain: useAuthStore, useTaskStore, useUIStore." -i high -k "zustand,state,react,store"
echo "  [stored] State management: Zustand, split by domain"

$ICM store -t "$TOPIC" -c "API authentication: JWT with refresh tokens. Access token expires in 15min, refresh token in 7 days. Tokens stored in httpOnly cookies, not localStorage (XSS prevention). Rust backend validates with jsonwebtoken crate." -i high -k "jwt,auth,cookies,security,jsonwebtoken"
echo "  [stored] Auth: JWT + refresh tokens in httpOnly cookies"

$ICM store -t "$TOPIC" -c "Database: PostgreSQL with SQLx (compile-time checked queries). Schema uses ULID for IDs instead of UUID (sortable, URL-safe). Migrations in apps/api/migrations/." -i high -k "postgres,sqlx,ulid,database,migrations"
echo "  [stored] Database: PostgreSQL + SQLx, ULID IDs"

$ICM store -t "$TOPIC" -c "API error handling: custom AppError enum that implements IntoResponse. Maps to standard HTTP status codes. All errors return JSON {error: string, code: string, details?: any}. Error codes are prefixed by domain: AUTH_001, TASK_001, etc." -i medium -k "errors,api,appError,http"
echo "  [stored] Error handling: AppError enum with domain-prefixed codes"

echo ""

# ─────────────────────────────────────────────────
# SESSION 2: Gemini works on task features
# Builds on Claude's architecture
# ─────────────────────────────────────────────────
echo "=== Session 2: Gemini — Task CRUD & real-time ==="
echo ""

$ICM store -t "$TOPIC" -c "Task model: id (ULID), title, description (markdown), status (enum: todo/in_progress/review/done), priority (1-4), assignee_id, project_id, due_date, created_at, updated_at. Soft delete with deleted_at column." -i high -k "task,model,schema,crud"
echo "  [stored] Task model with soft delete"

$ICM store -t "$TOPIC" -c "Real-time updates: Server-Sent Events (SSE) via Axum, not WebSockets. Reason: simpler, works through proxies, sufficient for task updates. Endpoint: GET /api/tasks/stream. Frontend uses EventSource API wrapped in a custom useTaskStream hook." -i high -k "sse,realtime,eventsource,stream"
echo "  [stored] Real-time: SSE (not WebSockets), useTaskStream hook"

$ICM store -t "$TOPIC" -c "Task filtering: backend supports query params ?status=todo&priority=3&assignee=me&search=keyword. Frontend TaskFilter component uses URL search params for shareable filter links. Debounced search input (300ms)." -i medium -k "filter,search,query,debounce"
echo "  [stored] Task filtering via URL search params, debounced"

echo ""

# ─────────────────────────────────────────────────
# SESSION 3: Claude debugs a production issue
# ─────────────────────────────────────────────────
echo "=== Session 3: Claude — Bug fix ==="
echo ""

$ICM store -t "$TOPIC" -c "Bug fix: SSE connection was dropping after 60s due to nginx proxy_read_timeout default. Fix: set proxy_read_timeout 3600s in nginx config AND added heartbeat ping every 30s from Axum SSE handler. Also added auto-reconnect in useTaskStream with exponential backoff (1s, 2s, 4s, max 30s)." -i high -k "bug,sse,nginx,timeout,heartbeat,reconnect"
echo "  [stored] Bug fix: SSE timeout + heartbeat + auto-reconnect"

$ICM store -t "$TOPIC" -c "Performance issue: task list was re-rendering on every SSE event. Fix: used React.memo on TaskCard, added selector to Zustand store (useTaskStore.use.taskById(id)) to prevent unnecessary re-renders. Reduced re-renders from 50+ to 2 per SSE event." -i medium -k "performance,react,memo,zustand,rerender"
echo "  [stored] Perf fix: React.memo + Zustand selectors"

echo ""

# Embed all memories
echo "  Embedding all memories..."
$ICM embed --force > /dev/null 2>&1
echo "  Done. $(echo 10) memories stored across 3 simulated sessions."
echo ""

# ─────────────────────────────────────────────────
# QUESTIONS: Test cross-session recall
# Each question requires knowledge from a DIFFERENT session
# ─────────────────────────────────────────────────

declare -a QUESTIONS=(
  # From Session 1 (Claude's architecture)
  "What state management library does TaskFlow use and why?"
  "How are authentication tokens stored in TaskFlow?"
  "What ID format does the database use instead of UUID?"
  # From Session 2 (Gemini's features)
  "How does TaskFlow handle real-time updates — WebSockets or SSE?"
  "What are the possible task statuses in TaskFlow?"
  # From Session 3 (Claude's bug fixes)
  "Why was the SSE connection dropping and how was it fixed?"
  "What caused excessive re-renders in the task list?"
)

declare -a EXPECTED=(
  "Zustand"
  "httpOnly cookies"
  "ULID"
  "SSE"
  "todo.*in_progress.*review.*done"
  "nginx.*timeout.*heartbeat"
  "React.memo.*Zustand.*selector"
)

declare -a SESSION_SOURCE=(
  "Session 1 (Claude)"
  "Session 1 (Claude)"
  "Session 1 (Claude)"
  "Session 2 (Gemini)"
  "Session 2 (Gemini)"
  "Session 3 (Claude)"
  "Session 3 (Claude)"
)

echo "══════════════════════════════════════════════════════════"
echo "  Testing: Can each LLM recall context from OTHER sessions?"
echo "══════════════════════════════════════════════════════════"
echo ""

# --- Gemini WITH ICM ---
echo "=== Gemini WITH ICM ==="
GEMINI_SCORE=0
for i in "${!QUESTIONS[@]}"; do
  echo -n "  Q$((i+1)) [${SESSION_SOURCE[$i]}]: ${QUESTIONS[$i]}"
  echo ""
  ANSWER=$(timeout 60 gemini -p "Use icm_memory_recall to search your memory about the TaskFlow project, then answer concisely (1-2 sentences max): ${QUESTIONS[$i]}" \
    --approval-mode yolo --allowed-mcp-server-names icm 2>/dev/null || echo "TIMEOUT")
  ANSWER=$(echo "$ANSWER" | grep -v '^$' | tail -3)
  echo "    → $ANSWER"
  if echo "$ANSWER" | grep -qiE "${EXPECTED[$i]}"; then
    GEMINI_SCORE=$((GEMINI_SCORE + 1))
    echo "    ✓ correct"
  else
    echo "    ✗ missed"
  fi
  echo ""
done

# --- Claude WITH ICM ---
echo "=== Claude WITH ICM ==="
CLAUDE_SCORE=0
for i in "${!QUESTIONS[@]}"; do
  echo -n "  Q$((i+1)) [${SESSION_SOURCE[$i]}]: ${QUESTIONS[$i]}"
  echo ""
  unset CLAUDECODE CLAUDE_CODE_SESSION
  ANSWER=$(timeout 60 claude -p "Use icm_memory_recall to search your memory about the TaskFlow project, then answer concisely (1-2 sentences max): ${QUESTIONS[$i]}" \
    --mcp-config "$MCP_CONFIG" --allowedTools "mcp__icm__icm_memory_recall" 2>/dev/null || echo "TIMEOUT")
  ANSWER=$(echo "$ANSWER" | grep -v '^$' | tail -3)
  echo "    → $ANSWER"
  if echo "$ANSWER" | grep -qiE "${EXPECTED[$i]}"; then
    CLAUDE_SCORE=$((CLAUDE_SCORE + 1))
    echo "    ✓ correct"
  else
    echo "    ✗ missed"
  fi
  echo ""
done

# --- Baseline: Claude WITHOUT ICM ---
echo "=== Claude WITHOUT ICM (baseline) ==="
BASELINE_SCORE=0
for i in "${!QUESTIONS[@]}"; do
  echo -n "  Q$((i+1)): ${QUESTIONS[$i]}"
  echo ""
  unset CLAUDECODE CLAUDE_CODE_SESSION
  ANSWER=$(timeout 30 claude -p "Answer concisely. If you don't know the specific answer for this project, say 'I don't know': ${QUESTIONS[$i]}" 2>/dev/null || echo "TIMEOUT")
  ANSWER=$(echo "$ANSWER" | grep -v '^$' | tail -3)
  echo "    → $ANSWER"
  if echo "$ANSWER" | grep -qiE "${EXPECTED[$i]}"; then
    BASELINE_SCORE=$((BASELINE_SCORE + 1))
    echo "    ✓ correct"
  else
    echo "    ✗ missed"
  fi
  echo ""
done

# --- Results ---
TOTAL=${#QUESTIONS[@]}
echo ""
echo "══════════════════════════════════════════════════════════"
echo "  Results — Cross-LLM Memory Benchmark"
echo "══════════════════════════════════════════════════════════"
echo ""
echo "  Project: TaskFlow (React/Bun + Rust/Axum)"
echo "  Sessions: 3 (Claude → Gemini → Claude)"
echo "  Questions: $TOTAL (each requires context from another session)"
echo ""
echo "  ┌─────────────────────────┬───────────┐"
echo "  │ Configuration           │ Score     │"
echo "  ├─────────────────────────┼───────────┤"
printf "  │ Claude WITHOUT ICM      │ %d/%d       │\n" $BASELINE_SCORE $TOTAL
printf "  │ Claude WITH ICM         │ %d/%d       │\n" $CLAUDE_SCORE $TOTAL
printf "  │ Gemini WITH ICM         │ %d/%d       │\n" $GEMINI_SCORE $TOTAL
echo "  └─────────────────────────┴───────────┘"
echo ""
echo "  Memories stored by: Claude (sessions 1,3), Gemini (session 2)"
echo "  Recalled by: Both — same DB, same binary, different LLMs."
echo ""
echo "══════════════════════════════════════════════════════════"

# Cleanup
rm -f "$MCP_CONFIG"
# Don't delete memories — let user inspect if needed
echo "  Topic: $TOPIC (run 'icm recall --topic $TOPIC' to inspect)"
