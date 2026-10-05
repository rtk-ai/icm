#!/bin/sh
# Container entrypoint: one benchmark run (or one shard of it), results to GCS.
#
# Required:  DATASET, SPLIT           e.g. longmemeval / s
# Optional:  RUN_NAME                 output directory name (default: icm)
#            MEMORY                   provider (default: icm; bm25 for the baseline; full-context
#                                     for the no-retrieval baseline)
#            ICM_AMB_ENGINE           required with MEMORY=icm: v2, legacy or
#                                     binary-default-no-dates (see icm_provider.py). The pod stops
#                                     before any download or model call without it; run_amb.py and
#                                     recall_only.py write the engine in the run description.
#            MODE                     harness mode (default: rag). `recall` is not a harness mode:
#                                     it runs recall_only.py instead of run_amb.py (ingestion and
#                                     retrieval only, no model is ever called; MEMORY icm or bm25,
#                                     indexed unit in ICM_AMB_RECALL_UNIT). Same arguments, same
#                                     sharding, same sync, one save per isolation unit.
#            SHARDS                   total shards; the shard index comes from
#                                     JOB_COMPLETION_INDEX (Kubernetes Indexed Job)
#            ICM_AMB_SHARD_BY         hash (default) or rank, see run_amb.py; passed through
#            RESULTS_BUCKET, RUN_ID   results go to gs://RESULTS_BUCKET/RUN_ID/...
#            RUN_DESCRIPTION          free text stored in the result file (`--description`);
#                                     may contain spaces. ICM_AMB_IMAGE, when set, is appended
#                                     so the result file names the image that produced it.
#            EXTRA_ARGS               extra flags for `amb run`, split on spaces
#                                     (e.g. "--query-limit 20"); no quoting inside
#            SYNC_INTERVAL            seconds between checks for a changed result file (15)
#            TERM_WAIT                seconds left to the benchmark process after SIGTERM (20)
#            DOWN_ATTEMPTS            tries of the initial download before the pod fails (5)
#            RESULTS_REMOTE           replaces gs://RESULTS_BUCKET; a local directory works
#                                     (tests and dry runs)
#            LONGMEMEVAL_DATA_PATH    the LongMemEval-S file. The image carries it; when the file
#                                     is absent it is fetched here, with retries and resume,
#                                     instead of by the harness (one attempt, no resume).
#
# Kept with the results, next to the result file: `usage-*.jsonl` (the token counts
# the API reports for every model response, thinking tokens included: what the run
# cost) and, with MEMORY=icm, `http-trace-*.jsonl` (every request sent to ICM: the
# engine named and the dates given).
# LLM settings (OMB_ANSWER_*, OMB_JUDGE_*, GOOGLE_*) are passed through untouched.
#
# What a restarted pod gets back: the result file of its previous attempts. The
# harness saves it after each finished isolation unit, so a dataset with units
# (LoCoMo: one conversation; PersonaMem only with ICM_AMB_UNIT_CHECKPOINT) resumes
# at the first unfinished unit. A dataset without units saves once, at the end:
# a restart replays the whole shard.
set -eu

: "${DATASET:?DATASET is required}"
: "${SPLIT:?SPLIT is required}"
RUN_NAME="${RUN_NAME:-icm}"
MEMORY="${MEMORY:-icm}"
MODE="${MODE:-rag}"
AMB_HOME="${AMB_HOME:-/work/amb}"
AMB_REF="${AMB_REF:-f618ed7b1f0eb9cad7b42e876f91a42f0eadb150}"
TOOLS="${TOOLS:-/opt/icm-amb}"
WORK_DIR="${WORK_DIR:-/work}"
SYNC_INTERVAL="${SYNC_INTERVAL:-15}"
TERM_WAIT="${TERM_WAIT:-20}"
DOWN_ATTEMPTS="${DOWN_ATTEMPTS:-5}"
DOWN_RETRY_DELAY="${DOWN_RETRY_DELAY:-5}"

log() { echo "[entrypoint] $*"; }

if [ "${MEMORY}" = "icm" ]; then
    # No implicit engine: what a binary picks by itself changed between builds, and
    # a result file must say which engine answered.
    case "${ICM_AMB_ENGINE:-}" in
        v2|legacy|binary-default-no-dates) ;;
        '') log "FATAL: MEMORY=icm needs ICM_AMB_ENGINE (v2, legacy or binary-default-no-dates)"; exit 2 ;;
        *) log "FATAL: ICM_AMB_ENGINE='${ICM_AMB_ENGINE}' is not v2, legacy or binary-default-no-dates"; exit 2 ;;
    esac
fi

SHARD_DIR="all"
if [ -n "${SHARDS:-}" ] && [ "${SHARDS}" -gt 1 ]; then
    INDEX="${JOB_COMPLETION_INDEX:?SHARDS is set but JOB_COMPLETION_INDEX is not}"
    export ICM_AMB_SHARD="${INDEX}/${SHARDS}"
    SHARD_DIR="shard-${INDEX}-of-${SHARDS}"
fi
OUT="${WORK_DIR}/outputs/${SHARD_DIR}"
mkdir -p "${OUT}"
export ICM_AMB_TRACE="${OUT}/trace-${DATASET}-${SPLIT}-${RUN_NAME}.jsonl"
export ICM_AMB_USAGE="${ICM_AMB_USAGE:-${OUT}/usage-${DATASET}-${SPLIT}-${RUN_NAME}.jsonl}"
if [ "${MEMORY}" = "icm" ]; then
    export ICM_AMB_HTTP_TRACE="${ICM_AMB_HTTP_TRACE:-${OUT}/http-trace-${DATASET}-${SPLIT}-${RUN_NAME}.jsonl}"
fi

# One line per pod start and per pod end, kept with the results: merge_shards.py
# reads it to tell a shard that was resumed (its ingestion time covers only the
# last attempt) and one that did not finish.
note() {
    # shellcheck disable=SC3028  # HOSTNAME is the pod name, set by the container runtime
    printf '{"time":"%s","host":"%s","event":"%s"%s}\n' \
        "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "${HOSTNAME:-unknown}" "$1" "${2:-}" >> "${OUT}/attempts.jsonl"
}

# SIGTERM (eviction, node drain, `kubectl delete`) reaches only this shell, PID 1
# of the container. It is forwarded to the benchmark process; the final upload
# below then runs inside the termination grace period.
PY_PID=""
TERMINATED=0
# shellcheck disable=SC2329  # invoked by the trap below
on_term() {
    TERMINATED=1
    if [ -n "${PY_PID}" ]; then
        log "SIGTERM: forwarding to the benchmark process (pid ${PY_PID})"
        kill -TERM "${PY_PID}" 2>/dev/null || true
    fi
}
trap on_term TERM INT

if [ ! -d "${AMB_HOME}/src/memory_bench" ]; then
    git clone --quiet --filter=blob:none --no-checkout \
        https://github.com/vectorize-io/agent-memory-benchmark.git "${AMB_HOME}"
    git -C "${AMB_HOME}" sparse-checkout set src
    git -C "${AMB_HOME}" checkout --quiet "${AMB_REF}"
fi
export AMB_HOME

if [ "${DATASET}" = "longmemeval" ]; then
    # 277 MB from Hugging Face. The harness fetches it with a single urlretrieve: one
    # dropped connection among the pods of a run would cost a pod attempt each time.
    if [ -z "${LONGMEMEVAL_DATA_PATH:-}" ] || [ ! -s "${LONGMEMEVAL_DATA_PATH}" ]; then
        LONGMEMEVAL_DATA_PATH="${WORK_DIR}/datasets/longmemeval_s_cleaned.json"
        if ! python "${TOOLS}/fetch_dataset.py" longmemeval "${LONGMEMEVAL_DATA_PATH}"; then
            log "FATAL: cannot fetch the LongMemEval-S file"
            exit 1
        fi
    fi
    export LONGMEMEVAL_DATA_PATH
fi

RESULT="${OUT}/${DATASET}/${RUN_NAME}/${MODE}/${SPLIT}.json"
REMOTE=""
if [ -n "${RESULTS_REMOTE:-}" ]; then
    REMOTE="${RESULTS_REMOTE}/${RUN_ID:-adhoc}/${SHARD_DIR}"
elif [ -n "${RESULTS_BUCKET:-}" ]; then
    REMOTE="gs://${RESULTS_BUCKET}/${RUN_ID:-adhoc}/${SHARD_DIR}"
fi

RESUME=""
if [ -n "${REMOTE}" ]; then
    # The previous attempt's answers are paid for: starting without them is not an
    # option. An empty prefix (first attempt) is a success with 0 files; a failed
    # listing or download is retried, then fails the pod before any LLM call.
    attempt=1
    delay="${DOWN_RETRY_DELAY}"
    until python "${TOOLS}/gcs_sync.py" down "${REMOTE}" "${OUT}"; do
        if [ "${attempt}" -ge "${DOWN_ATTEMPTS}" ]; then
            log "FATAL: cannot read ${REMOTE} after ${attempt} attempts; not starting from scratch"
            exit 1
        fi
        log "download from ${REMOTE} failed (attempt ${attempt}/${DOWN_ATTEMPTS}); retrying in ${delay}s"
        sleep "${delay}"
        attempt=$((attempt + 1))
        delay=$((delay * 2))
    done
    if [ -f "${RESULT}" ]; then
        # The harness reads an unreadable previous result as "nothing done" and
        # would replay every unit without a word.
        if ! python "${TOOLS}/gcs_sync.py" check "${RESULT}"; then
            log "FATAL: ${REMOTE} holds an unreadable result file; delete it or use a new RUN_ID"
            exit 1
        fi
        RESUME="--skip-ingested"
    fi
fi

DESCRIPTION="${RUN_DESCRIPTION:-}"
if [ -n "${ICM_AMB_IMAGE:-}" ]; then
    DESCRIPTION="${DESCRIPTION:+${DESCRIPTION} | }image ${ICM_AMB_IMAGE}"
fi
set -- run --dataset "${DATASET}" --split "${SPLIT}" --memory "${MEMORY}" --mode "${MODE}" \
    --name "${RUN_NAME}" --output-dir "${OUT}"
if [ -n "${DESCRIPTION}" ]; then
    set -- "$@" --description "${DESCRIPTION}"
fi

if [ "${TERMINATED}" -eq 1 ]; then
    log "SIGTERM before the run started: nothing to save"
    exit 143
fi

if [ -n "${RESUME}" ]; then note start ',"resume":true' || true; else note start ',"resume":false' || true; fi

SYNC_PID=""
if [ -n "${REMOTE}" ]; then
    # Uploads as soon as the harness saves a unit, so a pod killed without notice
    # (OOM, lost node) loses at most the unit in progress.
    python "${TOOLS}/gcs_sync.py" watch "${OUT}" "${REMOTE}" --interval "${SYNC_INTERVAL}" &
    SYNC_PID=$!
fi

RUNNER="${TOOLS}/run_amb.py"
if [ "${MODE}" = "recall" ]; then
    RUNNER="${TOOLS}/recall_only.py"
fi
# shellcheck disable=SC2086  # EXTRA_ARGS and RESUME are intentionally word-split
python "${RUNNER}" "$@" ${RESUME} ${EXTRA_ARGS:-} &
PY_PID=$!
if [ "${TERMINATED}" -eq 1 ]; then kill -TERM "${PY_PID}" 2>/dev/null || true; fi

status=0
wait "${PY_PID}" || status=$?
if [ "${TERMINATED}" -eq 1 ]; then
    # `wait` returned because of the trap, not because the process ended. After
    # SIGTERM the process saves nothing new (it only finishes a save already under
    # way), but it may sit on in-flight LLM calls for minutes: do not wait for those.
    waited=0
    while kill -0 "${PY_PID}" 2>/dev/null && [ "${waited}" -lt "${TERM_WAIT}" ]; do
        sleep 1
        waited=$((waited + 1))
    done
    if kill -0 "${PY_PID}" 2>/dev/null; then
        log "benchmark process still busy ${TERM_WAIT}s after SIGTERM; stopping it and syncing what is saved"
        kill -KILL "${PY_PID}" 2>/dev/null || true
        wait "${PY_PID}" 2>/dev/null || true
        status=143
    else
        status=0
        wait "${PY_PID}" || status=$?
    fi
fi

if [ -n "${SYNC_PID}" ]; then
    kill "${SYNC_PID}" 2>/dev/null || true
    wait "${SYNC_PID}" 2>/dev/null || true
fi
note exit ",\"status\":${status}" || true

if [ -n "${REMOTE}" ]; then
    # Final upload. A refusal (2: the remote copy is more complete) or an invalid
    # file (3) will not change on a second try; anything else is retried.
    sync=1
    attempt=1
    while [ "${attempt}" -le 3 ]; do
        sync=0
        python "${TOOLS}/gcs_sync.py" up "${OUT}" "${REMOTE}" || sync=$?
        if [ "${sync}" -eq 0 ] || [ "${sync}" -eq 2 ] || [ "${sync}" -eq 3 ]; then break; fi
        log "final upload failed (exit ${sync}, attempt ${attempt}/3)"
        attempt=$((attempt + 1))
        sleep 3
    done
    if [ "${sync}" -ne 0 ]; then
        log "ERROR: final upload to ${REMOTE} did not complete (gcs_sync exit ${sync})"
        if [ "${status}" -eq 0 ]; then status=1; fi
    fi
fi
exit "${status}"
