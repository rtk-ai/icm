#!/bin/sh
# Render k8s/job.yaml on stdout: defaults filled, required variables checked,
# free text made safe for the double-quoted YAML scalars it lands in.
#
#   export IMAGE=... RUN_ID=... DATASET=... SPLIT=... SHARDS=... PARALLELISM=... RESULTS_BUCKET=...
#   bench/amb/k8s/render.sh | kubectl --context gke_rtk-ai-labs-01_europe-west9-a_rtk-bench apply -f -
#
# Variables and their meaning: header of job.yaml. Nothing here talks to a cluster.
set -eu
LC_ALL=C; export LC_ALL  # [a-z] below must mean ASCII lowercase, whatever the caller's locale

: "${IMAGE:?IMAGE is required (prefer a digest: .../icm-amb@sha256:...; a tag can move between two shards)}"
: "${RUN_ID:?RUN_ID is required}"
: "${DATASET:?DATASET is required}"
: "${SPLIT:?SPLIT is required}"
: "${SHARDS:?SHARDS is required}"
: "${PARALLELISM:?PARALLELISM is required}"
: "${RESULTS_BUCKET:?RESULTS_BUCKET is required (without it nothing leaves the pod)}"
export RUN_NAME="${RUN_NAME:-icm}" MEMORY="${MEMORY:-icm}" MODE="${MODE:-rag}"
export ICM_AMB_RECALL_UNIT="${ICM_AMB_RECALL_UNIT:-}"
export BACKOFF_LIMIT_PER_INDEX="${BACKOFF_LIMIT_PER_INDEX:-1}"
export ICM_AMB_ENGINE="${ICM_AMB_ENGINE:-}" ICM_AMB_MAX_TOKENS="${ICM_AMB_MAX_TOKENS:-}"
export ICM_AMB_K="${ICM_AMB_K:-}" ICM_AMB_CHUNK_TOKENS="${ICM_AMB_CHUNK_TOKENS:-}"
export ICM_AMB_HEADER="${ICM_AMB_HEADER:-}" ICM_AMB_QUERY_NOW="${ICM_AMB_QUERY_NOW:-}"
export ICM_AMB_STORE_DATE="${ICM_AMB_STORE_DATE:-}" ICM_AMB_NO_EMBEDDINGS="${ICM_AMB_NO_EMBEDDINGS:-}"
export ICM_AMB_MAX_SERVERS="${ICM_AMB_MAX_SERVERS:-}" ICM_AMB_UNIT_CHECKPOINT="${ICM_AMB_UNIT_CHECKPOINT:-}"
export ICM_AMB_SHARD_BY="${ICM_AMB_SHARD_BY:-}"

die() { echo "render.sh: $*" >&2; exit 1; }

for pair in "SHARDS=${SHARDS}" "PARALLELISM=${PARALLELISM}" "BACKOFF_LIMIT_PER_INDEX=${BACKOFF_LIMIT_PER_INDEX}"; do
    case "${pair#*=}" in
        ''|*[!0-9]*) die "${pair%%=*} must be a whole number, got '${pair#*=}'" ;;
    esac
done
[ "${SHARDS}" -ge 1 ] || die "SHARDS must be at least 1"
case "${ICM_AMB_SHARD_BY}" in
    ''|hash|rank) ;;
    *) die "ICM_AMB_SHARD_BY must be hash, rank or empty, got '${ICM_AMB_SHARD_BY}'" ;;
esac
[ "${PARALLELISM}" -ge 1 ] || die "PARALLELISM must be at least 1"

# The recall engine is never implicit: what a binary picks by itself changed between
# builds, so a Job that names none would measure neither the baseline nor v2.
if [ "${MEMORY}" = "icm" ]; then
    case "${ICM_AMB_ENGINE}" in
        v2|legacy|binary-default-no-dates) ;;
        '') die "ICM_AMB_ENGINE is required with MEMORY=icm: v2 (dates sent unless ICM_AMB_STORE_DATE=0 / ICM_AMB_QUERY_NOW=0), legacy (the baseline, also for a binary older than v2) or binary-default-no-dates (whatever the binary picks, no date)" ;;
        *) die "ICM_AMB_ENGINE must be v2, legacy or binary-default-no-dates, got '${ICM_AMB_ENGINE}'" ;;
    esac
    for pair in "ICM_AMB_STORE_DATE=${ICM_AMB_STORE_DATE}" "ICM_AMB_QUERY_NOW=${ICM_AMB_QUERY_NOW}"; do
        case "${pair#*=}" in
            '') ;;
            0|1) [ "${ICM_AMB_ENGINE}" = "v2" ] || die "${pair%%=*} is only read with ICM_AMB_ENGINE=v2: ${ICM_AMB_ENGINE} sends no date" ;;
            *) die "${pair%%=*} must be 0, 1 or empty, got '${pair#*=}'" ;;
        esac
    done
    case "${ICM_AMB_NO_EMBEDDINGS}" in
        ''|0|1) ;;
        *) die "ICM_AMB_NO_EMBEDDINGS must be 0, 1 or empty, got '${ICM_AMB_NO_EMBEDDINGS}'" ;;
    esac
    if [ -n "${ICM_AMB_MAX_TOKENS}" ] && [ "${ICM_AMB_ENGINE}" != "v2" ]; then
        die "ICM_AMB_MAX_TOKENS needs ICM_AMB_ENGINE=v2, got '${ICM_AMB_ENGINE}'"
    fi
else
    # Settings only the ICM provider reads. A value left exported from an ICM run must
    # not travel in the manifest of a baseline, where it would read as a setting of it.
    unread() { [ -z "$2" ] || echo "render.sh: note: $1=$2 is not read with MEMORY=${MEMORY}; rendered empty" >&2; }
    unread ICM_AMB_ENGINE "${ICM_AMB_ENGINE}"
    unread ICM_AMB_MAX_TOKENS "${ICM_AMB_MAX_TOKENS}"
    unread ICM_AMB_STORE_DATE "${ICM_AMB_STORE_DATE}"
    unread ICM_AMB_QUERY_NOW "${ICM_AMB_QUERY_NOW}"
    unread ICM_AMB_NO_EMBEDDINGS "${ICM_AMB_NO_EMBEDDINGS}"
    ICM_AMB_ENGINE='' ICM_AMB_MAX_TOKENS='' ICM_AMB_STORE_DATE='' ICM_AMB_QUERY_NOW='' ICM_AMB_NO_EMBEDDINGS=''
fi
case "${MODE}" in
    rag|agentic-rag|agent|coding|retrieval) [ -z "${ICM_AMB_RECALL_UNIT}" ] || die "ICM_AMB_RECALL_UNIT is only read with MODE=recall" ;;
    recall)
        case "${MEMORY}" in
            icm|bm25) ;;
            *) die "MODE=recall runs MEMORY=icm or MEMORY=bm25, got '${MEMORY}'" ;;
        esac
        case "${ICM_AMB_RECALL_UNIT}" in
            ''|session-user|session-all|chunk) ;;
            *) die "ICM_AMB_RECALL_UNIT must be session-user, session-all, chunk or empty, got '${ICM_AMB_RECALL_UNIT}'" ;;
        esac
        [ -z "${ICM_AMB_MAX_TOKENS}" ] || die "MODE=recall refuses ICM_AMB_MAX_TOKENS (a token budget would cut the ranked list)"
        ;;
    *) die "MODE must be rag, agentic-rag, agent, coding, retrieval or recall, got '${MODE}'" ;;
esac

# The Job name is a DNS label: lowercase letters, digits, hyphens, 63 characters.
name="amb-${RUN_ID}-${DATASET}-${SPLIT}-${MEMORY}"
case "${name}" in
    *[!a-z0-9-]*) die "Job name '${name}' may only hold lowercase letters, digits and hyphens (RUN_ID, DATASET, SPLIT, MEMORY)" ;;
esac
[ "${#name}" -le 63 ] || die "Job name '${name}' is longer than 63 characters; shorten RUN_ID"

# Text that lands inside "..." in the template: one line, backslash and quote escaped.
yaml_text() {
    case "$2" in
        *"
"*) die "$1 must be a single line" ;;
    esac
    printf '%s' "$2" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g'
}
RUN_DESCRIPTION="$(yaml_text RUN_DESCRIPTION "${RUN_DESCRIPTION:-}")"
EXTRA_ARGS="$(yaml_text EXTRA_ARGS "${EXTRA_ARGS:-}")"
export RUN_DESCRIPTION EXTRA_ARGS
case "${EXTRA_ARGS}" in
    *--description*|*\"*|*\'*) die "EXTRA_ARGS is split on spaces and cannot carry quoted text: put the description in RUN_DESCRIPTION" ;;
esac

# Only the template's own variables are substituted.
# shellcheck disable=SC2016
envsubst '${IMAGE} ${RUN_ID} ${RUN_NAME} ${MEMORY} ${MODE} ${ICM_AMB_RECALL_UNIT} ${DATASET} ${SPLIT} ${SHARDS} ${PARALLELISM}
${BACKOFF_LIMIT_PER_INDEX} ${RESULTS_BUCKET} ${EXTRA_ARGS} ${RUN_DESCRIPTION} ${ICM_AMB_ENGINE}
${ICM_AMB_MAX_TOKENS} ${ICM_AMB_K} ${ICM_AMB_CHUNK_TOKENS} ${ICM_AMB_HEADER} ${ICM_AMB_QUERY_NOW}
${ICM_AMB_STORE_DATE} ${ICM_AMB_NO_EMBEDDINGS}
${ICM_AMB_MAX_SERVERS} ${ICM_AMB_UNIT_CHECKPOINT} ${ICM_AMB_SHARD_BY}' < "$(dirname "$0")/job.yaml"
