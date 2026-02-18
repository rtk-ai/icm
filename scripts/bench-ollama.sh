#!/usr/bin/env bash
# Benchmark ICM recall with local LLMs via ollama
# Tests Layer 2 (context injection) — no MCP tools needed
set -euo pipefail

OLLAMA_HOST="admn@maria"
MODEL="${1:-qwen2.5:7b}"
VERBOSE="${2:-}"  # pass "verbose" as 2nd arg to see answers
ICM_BIN="./target/release/icm"
TMPDIR=$(mktemp -d /tmp/icm-ollama-bench-XXXXXX)
ICM_DB="$TMPDIR/icm-bench.db"

trap "rm -rf $TMPDIR" EXIT

echo "ICM Recall Benchmark (ollama, model: $MODEL, host: maria)"
echo "================================================================"
echo "Tests context injection only (Layer 0+2). No MCP tools."
echo ""

# Questions and expected keywords
declare -a QUESTIONS=(
  "Who proposed the Meridian Protocol, at which conference, and in what year?"
  "What are the three phases of Meridian and their timeouts?"
  "What is the maximum cluster size for Meridian and why?"
  "What ports does the Meridian gossip protocol use?"
  "What throughput did Meridian achieve on a 64-node cluster?"
  "What is the Byzantine fault tolerance formula for Meridian?"
  "Name the three implementations of Meridian and their languages."
  "Which companies deployed Meridian in production?"
  "What was Dr. Tanaka's prior work that influenced Meridian?"
  "What is the BLAME threshold and what happens when it's reached?"
)

declare -a KEYWORDS=(
  "Vasquez|Tanaka|SIGCOMM|2019|Beijing"
  "Propose|150|Validate|300|Commit|50"
  "127|7-bit|node.ID"
  "9471|UDP|9472|TCP"
  "47.000|47000|47ms|47,000"
  "3f|Byzantine|2f|crash"
  "libmeridian|meridian-rs|Rust|PyMeridian|Python"
  "Cloudflare|Akamai|Fastly"
  "Firefly|gossip|SOSP|2017"
  "f...1|blacklist|10 epochs|leader|rotation"
)

# Pre-populate ICM DB with extracted facts from the document
echo "=== Populating ICM with extracted facts ==="
$ICM_BIN --db "$ICM_DB" store -t "init" -c "init" -i low 2>/dev/null
$ICM_BIN --db "$ICM_DB" store -t "context-meridian" -c 'The Meridian Protocol was proposed by Dr. Elena Vasquez and Dr. Kenji Tanaka at SIGCOMM 2019 in Beijing.' -i high -k "proposed,authors,conference,SIGCOMM,Vasquez,Tanaka,origin" 2>/dev/null
$ICM_BIN --db "$ICM_DB" store -t "context-meridian" -c 'Their paper "Meridian: Sub-millisecond Consensus at the Edge" won Best Paper and introduced a three-phase commit protocol.' -i high -k "paper,best-paper,consensus,edge" 2>/dev/null
$ICM_BIN --db "$ICM_DB" store -t "context-meridian" -c 'Meridian uses three phases: Propose (150ms timeout), Validate (300ms timeout), Commit (50ms timeout). Total worst-case: 500ms.' -i high -k "phases,timeout,propose,validate,commit" 2>/dev/null
$ICM_BIN --db "$ICM_DB" store -t "context-meridian" -c 'Maximum cluster size: 127 nodes (limited by 7-bit node ID in header). Minimum: 5 nodes.' -i medium -k "cluster,size,maximum,nodes,limit" 2>/dev/null
$ICM_BIN --db "$ICM_DB" store -t "context-meridian" -c 'Gossip protocol port: 9471 (UDP) for peer discovery, 9472 (TCP) for state sync.' -i medium -k "gossip,port,UDP,TCP,network" 2>/dev/null
$ICM_BIN --db "$ICM_DB" store -t "context-meridian" -c 'Benchmark: 64 nodes cluster achieved 47,000 TPS with 47ms median latency.' -i high -k "benchmark,throughput,TPS,latency,performance" 2>/dev/null
$ICM_BIN --db "$ICM_DB" store -t "context-meridian" -c 'Meridian tolerates up to f Byzantine faults in a cluster of n = 3f + 1 nodes. For crash-only: n = 2f + 1.' -i high -k "byzantine,fault,tolerance,crash,formula" 2>/dev/null
$ICM_BIN --db "$ICM_DB" store -t "context-meridian" -c 'Three implementations: libmeridian (C++, 47k lines, Stanford), meridian-rs (Rust, 12k lines, Constellation Labs), PyMeridian (Python, 3.2k lines, MIT).' -i high -k "implementations,languages,C++,Rust,Python" 2>/dev/null
$ICM_BIN --db "$ICM_DB" store -t "context-meridian" -c 'Deployments: Cloudflare Workers KV (2020), Akamai EdgeDB (2021), Fastly Compute@Edge (2022).' -i medium -k "deployments,production,companies,Cloudflare,Akamai,Fastly" 2>/dev/null
$ICM_BIN --db "$ICM_DB" store -t "context-meridian" -c "Tanaka prior work: the Firefly gossip protocol, published at SOSP 2017, provided foundation for Meridian peer discovery layer." -i high -k "Tanaka,Firefly,prior-work,SOSP,gossip" 2>/dev/null
$ICM_BIN --db "$ICM_DB" store -t "context-meridian" -c 'BLAME threshold: f + 1 messages. When reached, the leader is blacklisted for 10 epochs and leader rotation is triggered.' -i high -k "BLAME,threshold,blacklist,leader,rotation" 2>/dev/null
echo "  Stored 11 facts in ICM"

# Function: query ollama via SSH — builds JSON with python to avoid escaping hell
ask_ollama() {
  local system_prompt="$1"
  local user_prompt="$2"
  local payload_file="$TMPDIR/payload.json"

  # Build JSON payload safely with python
  python3 -c "
import json, sys
payload = {
    'model': sys.argv[1],
    'system': sys.argv[2],
    'prompt': sys.argv[3],
    'stream': False,
    'options': {'num_predict': 256, 'temperature': 0.1}
}
print(json.dumps(payload))
" "$MODEL" "$system_prompt" "$user_prompt" > "$payload_file"

  # Send via SSH: copy payload, curl, cleanup
  local response
  response=$(cat "$payload_file" | ssh "$OLLAMA_HOST" "cat > /tmp/ollama-payload.json && curl -s --max-time 120 http://localhost:11434/api/generate -d @/tmp/ollama-payload.json && rm -f /tmp/ollama-payload.json" 2>/dev/null)
  echo "$response" | python3 -c "import sys,json; print(json.loads(sys.stdin.read()).get('response',''))" 2>/dev/null || echo ""
}

# Function: count keyword matches
count_matches() {
  local answer="$1"
  local keywords="$2"
  local answer_lower
  answer_lower=$(echo "$answer" | tr '[:upper:]' '[:lower:]')
  local count=0
  IFS='|' read -ra KWS <<< "$keywords"
  for kw in "${KWS[@]}"; do
    local kw_lower
    kw_lower=$(echo "$kw" | tr '[:upper:]' '[:lower:]')
    if echo "$answer_lower" | grep -qi "$kw_lower"; then
      count=$((count + 1))
    fi
  done
  echo "$count"
}

# === Questions WITHOUT context ===
echo ""
echo "=== WITHOUT ICM (no context) ==="
declare -a SCORES_WO=()
declare -a TOTALS=()
for i in "${!QUESTIONS[@]}"; do
  Q="${QUESTIONS[$i]}"
  SYSTEM="You are a helpful assistant. Answer questions concisely and accurately. If you don't know the answer, say so."

  printf "  Q%d/10..." "$((i+1))"
  ANSWER=$(ask_ollama "$SYSTEM" "$Q")
  MATCHES=$(count_matches "$ANSWER" "${KEYWORDS[$i]}")
  TOTAL=$(echo "${KEYWORDS[$i]}" | tr '|' '\n' | wc -l | tr -d ' ')
  SCORES_WO+=("$MATCHES")
  TOTALS+=("$TOTAL")
  printf " %s/%s keywords\n" "$MATCHES" "$TOTAL"
  if [ "$VERBOSE" = "verbose" ]; then
    echo "    >>> $(echo "$ANSWER" | head -3 | tr '\n' ' ')"
  fi
done

# === Questions WITH context injection ===
echo ""
echo "=== WITH ICM (Layer 2 context injection) ==="
declare -a SCORES_WI=()
for i in "${!QUESTIONS[@]}"; do
  Q="${QUESTIONS[$i]}"

  # Get raw facts from ICM recall (extract summary lines)
  FACTS=$($ICM_BIN --db "$ICM_DB" recall "$Q" --limit 15 2>/dev/null | grep '  summary:' | sed 's/^  summary:  *//' || echo "")

  SYSTEM="You are a helpful assistant. Answer the question using ONLY the reference information below. Be concise and accurate.

Reference information:
$FACTS"

  printf "  Q%d/10..." "$((i+1))"
  ANSWER=$(ask_ollama "$SYSTEM" "$Q")
  MATCHES=$(count_matches "$ANSWER" "${KEYWORDS[$i]}")
  TOTAL="${TOTALS[$i]}"
  SCORES_WI+=("$MATCHES")
  printf " %s/%s keywords\n" "$MATCHES" "$TOTAL"
  if [ "$VERBOSE" = "verbose" ]; then
    echo "    CTX: $(echo "$FACTS" | head -2 | tr '\n' ' | ')"
    echo "    >>> $(echo "$ANSWER" | head -3 | tr '\n' ' ')"
  fi
done

# === Results table ===
echo ""
echo "================================================================"
printf "%-45s %10s %10s\n" "Question" "No ICM" "With ICM"
echo "----------------------------------------------------------------"
TOTAL_WO=0
TOTAL_WI=0
TOTAL_KW=0
for i in "${!QUESTIONS[@]}"; do
  Q="${QUESTIONS[$i]}"
  Q_SHORT="${Q:0:42}"
  [ ${#Q} -gt 42 ] && Q_SHORT="${Q_SHORT}..."
  printf "%-45s %7s/%-2s %7s/%-2s\n" "$Q_SHORT" "${SCORES_WO[$i]}" "${TOTALS[$i]}" "${SCORES_WI[$i]}" "${TOTALS[$i]}"
  TOTAL_WO=$((TOTAL_WO + SCORES_WO[$i]))
  TOTAL_WI=$((TOTAL_WI + SCORES_WI[$i]))
  TOTAL_KW=$((TOTAL_KW + TOTALS[$i]))
done
echo "----------------------------------------------------------------"
PCT_WO=$((TOTAL_WO * 100 / TOTAL_KW))
PCT_WI=$((TOTAL_WI * 100 / TOTAL_KW))
printf "%-45s %7s/%-2s %7s/%-2s\n" "Total keywords matched" "$TOTAL_WO" "$TOTAL_KW" "$TOTAL_WI" "$TOTAL_KW"
printf "%-45s %9s%% %9s%%\n" "Score" "$PCT_WO" "$PCT_WI"
echo "================================================================"
echo "Model: $MODEL (ollama on maria) | No MCP tools | Pure Layer 0+2"
