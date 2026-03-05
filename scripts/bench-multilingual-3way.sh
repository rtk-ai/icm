#!/usr/bin/env bash
# Benchmark: 3-way comparison of embedding models
set -euo pipefail

ICM="./target/release/icm"
TMPDIR=$(mktemp -d)

MODELS=(
  "Xenova/bge-small-en-v1.5|EN-small-Q|384"
  "intfloat/multilingual-e5-small|ML-e5-small|384"
  "intfloat/multilingual-e5-base|ML-e5-base|768"
)

echo "=== ICM Embedding Model Comparison (3-way) ==="
echo ""

MEMORIES=(
  "decisions-arch|Utiliser PostgreSQL comme base de donnees principale pour la persistence des objets metier|high|postgres,db"
  "errors-resolved|Erreur SQLite WAL: le fichier est verrouille quand 2 processus ecrivent simultanement. Solution: utiliser un seul writer avec queue.|high|sqlite,wal,lock"
  "context-project|Le serveur MCP utilise le transport stdio avec JSON-RPC 2.0 exposant 18 outils|medium|mcp,stdio"
  "preferences|L'utilisateur prefere Rust a Go pour les projets systemes car il aime le typage fort|medium|rust,go"
  "decisions-arch|L'authentification utilise des JWT tokens avec refresh rotation toutes les 15 minutes|high|auth,jwt"
  "errors-resolved|Probleme de compilation croisee aarch64: il faut ajouter g++ pour le linking libstdc++|medium|cross-compile,aarch64"
  "context-project|Le workspace Rust contient 4 crates: icm-core, icm-store, icm-mcp, icm-cli|high|workspace,crates"
  "preferences|Toujours utiliser bun au lieu de npm pour JavaScript, c'est plus rapide|medium|bun,npm"
  "decisions-arch|Use SQLite with FTS5 for full-text search and sqlite-vec for vector similarity|high|fts5,vector"
  "errors-resolved|fastembed ORT session crash on M1 when using quantized model. Fix: set intra_op_threads to 1|high|fastembed,ort,m1"
  "context-project|Hybrid search combines 30% BM25 with 70% cosine vector similarity|medium|bm25,cosine"
  "preferences|Prefer compact CLI output to save tokens in AI agent context|low|compact,tokens"
)

QUERIES=(
  # Pure semantic FR->FR (no shared words)
  "quel SGBD relationnel choisir pour persister les donnees|postgres"
  "acces concurrent fichier qui bloque les ecritures|wal,lock"
  "compilateur de langages systemes favori|rust,go"
  "renouvellement automatique des jetons de securite|auth,jwt"
  # Pure semantic EN->EN (no shared words)
  "relational database management system choice|postgres"
  "file locking concurrent writes issue|wal,lock"
  "favorite systems programming language|rust,go"
  "security credential renewal rotation|auth,jwt"
  # Cross-language FR->EN
  "moteur de recherche plein texte dans la base|fts5,vector"
  "plantage librairie IA puce Apple|ort,m1"
  "algorithme de classement par pertinence|bm25,cosine"
  # Cross-language EN->FR
  "protocol for AI tool communication|mcp,stdio"
  "javascript package manager preference|bun,npm"
  "cross compilation arm linux build|aarch64"
  "project code organization modules|workspace,crates"
)

total=${#QUERIES[@]}

for model_entry in "${MODELS[@]}"; do
  IFS='|' read -r model_name label dims <<< "$model_entry"
  db="$TMPDIR/db-${label}.db"
  conf="$TMPDIR/conf-${label}.toml"

  echo "[embeddings]" > "$conf"
  echo "model = \"$model_name\"" >> "$conf"

  # Seed
  for entry in "${MEMORIES[@]}"; do
    IFS='|' read -r topic content importance keywords <<< "$entry"
    "$ICM" --db "$db" store -t "$topic" -c "$content" -i "$importance" -k "$keywords" > /dev/null 2>&1
  done

  # Embed
  echo "Embedding with $label ($model_name, ${dims}d)..."
  ICM_CONFIG="$conf" "$ICM" --db "$db" embed --force 2>/dev/null
done

echo ""
printf "%-50s" "Query"
for model_entry in "${MODELS[@]}"; do
  IFS='|' read -r _ label _ <<< "$model_entry"
  printf "  %12s" "$label"
done
echo ""
printf "%-50s" "$(printf '%0.s-' {1..50})"
for _ in "${MODELS[@]}"; do printf "  %12s" "------------"; done
echo ""

declare -A scores
for model_entry in "${MODELS[@]}"; do
  IFS='|' read -r _ label _ <<< "$model_entry"
  scores[$label]=0
done

for entry in "${QUERIES[@]}"; do
  IFS='|' read -r query expected_kw <<< "$entry"
  display_q="${query:0:48}"
  printf "%-50s" "$display_q"

  for model_entry in "${MODELS[@]}"; do
    IFS='|' read -r _ label _ <<< "$model_entry"
    db="$TMPDIR/db-${label}.db"
    conf="$TMPDIR/conf-${label}.toml"

    result=$(ICM_CONFIG="$conf" "$ICM" --db "$db" recall "$query" --limit 3 2>/dev/null || echo "")

    hit=0
    IFS=',' read -ra kws <<< "$expected_kw"
    for kw in "${kws[@]}"; do
      if echo "$result" | grep -qi "$kw"; then hit=1; break; fi
    done

    scores[$label]=$((${scores[$label]} + hit))
    mark="miss"
    [ "$hit" -eq 1 ] && mark="HIT"
    printf "  %12s" "$mark"
  done
  echo ""
done

echo ""
printf "%-50s" "SCORE"
for model_entry in "${MODELS[@]}"; do
  IFS='|' read -r _ label _ <<< "$model_entry"
  printf "  %8d/%d  " "${scores[$label]}" "$total"
done
echo ""

printf "%-50s" "ACCURACY"
for model_entry in "${MODELS[@]}"; do
  IFS='|' read -r _ label _ <<< "$model_entry"
  pct=$((${scores[$label]} * 100 / total))
  printf "  %10d%%  " "$pct"
done
echo ""

echo ""
echo "================================================================"
for model_entry in "${MODELS[@]}"; do
  IFS='|' read -r model_name label dims <<< "$model_entry"
  printf "  %-14s: %s (%sd)\n" "$label" "$model_name" "$dims"
done
echo "================================================================"

rm -rf "$TMPDIR"
