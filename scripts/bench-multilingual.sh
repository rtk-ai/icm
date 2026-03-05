#!/usr/bin/env bash
# Benchmark: bge-small-en-v1.5 (English-only) vs multilingual-e5-small
# Tests recall quality with mixed FR/EN content, focusing on semantic search
set -euo pipefail

ICM="./target/release/icm"
TMPDIR=$(mktemp -d)
DB_EN="$TMPDIR/en.db"
DB_ML="$TMPDIR/ml.db"

echo "=== ICM Multilingual Embedding Benchmark ==="
echo ""

# --- Seed memories (mixed FR/EN) ---
MEMORIES=(
  # French content
  "decisions-arch|Utiliser PostgreSQL comme base de donnees principale pour la persistence des objets metier|high|postgres,db,architecture"
  "errors-resolved|Erreur SQLite WAL: le fichier est verrouille quand 2 processus ecrivent simultanement. Solution: utiliser un seul writer avec queue.|high|sqlite,wal,lock"
  "context-project|Le serveur MCP utilise le transport stdio avec JSON-RPC 2.0, exposant 18 outils pour la memoire persistante|medium|mcp,stdio,jsonrpc"
  "preferences|L'utilisateur prefere Rust a Go pour les projets systemes car il aime le typage fort et la gestion memoire|medium|rust,go,language"
  "decisions-arch|L'authentification utilise des JWT tokens avec refresh rotation toutes les 15 minutes|high|auth,jwt,tokens"
  "errors-resolved|Probleme de compilation croisee aarch64: il faut ajouter g++ pour le linking libstdc++|medium|cross-compile,aarch64,linux"
  "context-project|Le workspace Rust contient 4 crates: icm-core pour les types, icm-store pour SQLite, icm-mcp pour le protocole, icm-cli pour le terminal|high|rust,workspace,crates"
  "preferences|Toujours utiliser bun au lieu de npm pour les projets JavaScript, c'est plus rapide|medium|bun,npm,javascript"
  # English content
  "decisions-arch|Use SQLite with FTS5 for full-text search and sqlite-vec extension for vector similarity matching|high|sqlite,fts5,vector"
  "errors-resolved|fastembed ORT session crash on M1 when using quantized model with wrong thread count. Fix: set intra_op_threads to 1|high|fastembed,ort,m1,crash"
  "context-project|Hybrid search combines 30 percent BM25 keyword matching with 70 percent cosine vector similarity for best results|medium|hybrid,bm25,cosine"
  "preferences|Prefer compact CLI output to save tokens in AI agent context windows|low|compact,tokens,cli"
)

# ==========================================
# TEST 1: Standard queries (FTS5 + vectors)
# ==========================================
QUERIES_STANDARD=(
  # French queries for French content
  "quelle base de donnees utiliser|postgres"
  "erreur sqlite verrouillage|wal,lock"
  "serveur MCP transport|mcp,stdio"
  "preference langage programmation|rust,go"
  # English queries for English content
  "full text search setup|fts5,vector"
  "fastembed crash apple silicon|ort,m1"
  "search algorithm weights|bm25,cosine"
  # Cross-language: FR query -> EN content
  "recherche semantique vectorielle|fts5,vector"
  "crash du modele embedding|ort,m1"
  # Cross-language: EN query -> FR content
  "database choice for persistence|postgres"
  "authentication tokens refresh|auth,jwt"
  "rust workspace project structure|workspace,crates"
  "javascript package manager preference|bun,npm"
  "cross compilation arm linux|aarch64,linux"
)

# ==========================================
# TEST 2: Pure semantic (NO shared words)
# Queries that have ZERO lexical overlap with content
# ==========================================
QUERIES_SEMANTIC=(
  # FR query -> FR content (no shared words)
  "quel SGBD relationnel choisir|postgres"
  "acces concurrent fichier bloque|wal,lock"
  "quel outil de scripting rapide cote client|bun,npm"
  "compilateur de langages systemes favori|rust,go"
  # FR query -> EN content (cross-language, no shared words)
  "moteur de recherche plein texte|fts5,vector"
  "plantage librairie intelligence artificielle puce Apple|ort,m1"
  "algorithme de classement par pertinence|bm25,cosine"
  # EN query -> FR content (cross-language, no shared words)
  "relational database management system choice|postgres"
  "file locking concurrent writes issue|wal,lock"
  "protocol for AI tool communication|mcp,stdio"
  "favorite systems programming language|rust,go"
  "security credential renewal rotation|auth,jwt"
)

seed_db() {
  local db="$1"
  for entry in "${MEMORIES[@]}"; do
    IFS='|' read -r topic content importance keywords <<< "$entry"
    "$ICM" --db "$db" store -t "$topic" -c "$content" -i "$importance" -k "$keywords" > /dev/null 2>&1
  done
}

echo "Seeding databases..."
seed_db "$DB_EN"
seed_db "$DB_ML"

CONF_EN="$TMPDIR/config-en.toml"
CONF_ML="$TMPDIR/config-ml.toml"

cat > "$CONF_EN" << 'EOF'
[embeddings]
model = "Xenova/bge-small-en-v1.5"
EOF

cat > "$CONF_ML" << 'EOF'
[embeddings]
model = "intfloat/multilingual-e5-small"
EOF

echo "Embedding with bge-small-en-v1.5 (English-only)..."
ICM_CONFIG="$CONF_EN" "$ICM" --db "$DB_EN" embed --force 2>/dev/null

echo "Embedding with multilingual-e5-small..."
ICM_CONFIG="$CONF_ML" "$ICM" --db "$DB_ML" embed --force 2>/dev/null
echo ""

run_test() {
  local test_name="$1"
  shift
  local -n queries_ref=$1

  local score_en=0
  local score_ml=0
  local total=${#queries_ref[@]}

  echo "--- $test_name ($total queries) ---"
  echo ""
  printf "%-50s  %8s  %8s\n" "Query" "EN-only" "Multi"
  printf "%-50s  %8s  %8s\n" "$(printf '%0.s-' {1..50})" "-------" "-----"

  for entry in "${queries_ref[@]}"; do
    IFS='|' read -r query expected_kw <<< "$entry"

    result_en=$(ICM_CONFIG="$CONF_EN" "$ICM" --db "$DB_EN" recall "$query" --limit 3 2>/dev/null || echo "")
    result_ml=$(ICM_CONFIG="$CONF_ML" "$ICM" --db "$DB_ML" recall "$query" --limit 3 2>/dev/null || echo "")

    hit_en=0
    hit_ml=0
    IFS=',' read -ra kws <<< "$expected_kw"
    for kw in "${kws[@]}"; do
      if echo "$result_en" | grep -qi "$kw"; then hit_en=1; break; fi
    done
    for kw in "${kws[@]}"; do
      if echo "$result_ml" | grep -qi "$kw"; then hit_ml=1; break; fi
    done

    score_en=$((score_en + hit_en))
    score_ml=$((score_ml + hit_ml))

    en_mark="miss"; ml_mark="miss"
    [ "$hit_en" -eq 1 ] && en_mark="HIT"
    [ "$hit_ml" -eq 1 ] && ml_mark="HIT"

    display_q="${query:0:48}"
    printf "%-50s  %8s  %8s\n" "$display_q" "$en_mark" "$ml_mark"
  done

  pct_en=$((score_en * 100 / total))
  pct_ml=$((score_ml * 100 / total))

  echo ""
  printf "%-50s  %5d/%d  %5d/%d\n" "Score" "$score_en" "$total" "$score_ml" "$total"
  printf "%-50s  %6d%%  %6d%%\n" "Accuracy" "$pct_en" "$pct_ml"
  echo ""
}

run_test "TEST 1: Standard queries (hybrid FTS5 + vector)" QUERIES_STANDARD
run_test "TEST 2: Pure semantic (zero lexical overlap)" QUERIES_SEMANTIC

echo "================================================================"
echo "  bge-small-en-v1.5    : English-only, 384d, quantized"
echo "  multilingual-e5-small: 100+ languages, 384d"
echo "================================================================"

rm -rf "$TMPDIR"
