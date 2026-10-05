# ICM x Agent Memory Benchmark runner image (linux/amd64).
#
# Build context is the repository root:
#   docker build --platform linux/amd64 -f bench/amb/Dockerfile -t icm-amb:<tag> .
# On an Apple Silicon laptop this cross-build runs under emulation and is very
# slow; prefer Cloud Build (see cloudbuild.yaml).
#
# The image contains: the `icm` binary built from this checkout (default feature
# set, i.e. what a release binary ships), the ICM provider + launcher, the light
# Python dependencies, and the embedding model pre-downloaded. The upstream
# harness is NOT baked in (it declares no license): entrypoint.sh clones it at
# the pinned commit AMB_REF when the container starts.

# trixie, not bookworm: the prebuilt ONNX Runtime that `ort` links needs
# glibc >= 2.38 and a recent libstdc++ (bookworm fails at link time with
# `undefined symbol: __isoc23_strtol`). CI builds on ubuntu-latest for the
# same reason. The runtime stage must be at least as recent.
FROM --platform=linux/amd64 rust:1-trixie AS icm-build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
# main.rs embeds plugins/opencode-icm.ts via include_str!
COPY plugins ./plugins
# No --locked: the committed Cargo.lock can lag the workspace version by one
# release (it did at 0.10.65), which --locked turns into a build failure.
RUN cargo build --release -p icm-cli

FROM --platform=linux/amd64 python:3.12-slim-trixie
# 1: the image carries the LongMemEval-S file (277 MB), so the pods of a recall-only
# run do not each download it. 0: entrypoint.sh fetches it at pod start instead.
ARG LONGMEMEVAL=1
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates git libssl3t64 libstdc++6 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 10001 bench \
    && mkdir -p /work && chown bench:bench /work

COPY --from=icm-build /src/target/release/icm /usr/local/bin/icm
COPY bench/amb/requirements.txt /opt/icm-amb/requirements.txt
RUN pip install --no-cache-dir -r /opt/icm-amb/requirements.txt google-cloud-storage
COPY bench/amb/icm_provider.py bench/amb/full_context_provider.py bench/amb/run_amb.py \
     bench/amb/recall_at_k.py bench/amb/recall_only.py bench/amb/merge_recall.py \
     bench/amb/merge_shards.py bench/amb/compare_builds.py bench/amb/dry_run_tokens.py \
     bench/amb/gcs_sync.py bench/amb/fetch_dataset.py bench/amb/estimate_cost.py \
     bench/amb/entrypoint.sh /opt/icm-amb/
RUN chmod 0755 /opt/icm-amb/entrypoint.sh \
    && mkdir -p /opt/icm-amb/data \
    && if [ "${LONGMEMEVAL}" = "1" ]; then \
         python /opt/icm-amb/fetch_dataset.py longmemeval /opt/icm-amb/data/longmemeval_s_cleaned.json; \
       fi

USER bench
ENV HOME=/home/bench \
    TIKTOKEN_CACHE_DIR=/home/bench/.cache/tiktoken \
    ICM_AMB_BIN=/usr/local/bin/icm \
    AMB_HOME=/work/amb \
    AMB_REF=f618ed7b1f0eb9cad7b42e876f91a42f0eadb150 \
    LONGMEMEVAL_DATA_PATH=/opt/icm-amb/data/longmemeval_s_cleaned.json \
    PYTHONUNBUFFERED=1

# Pre-download what would otherwise be fetched by every pod on first use:
# the tokenizer used for chunking, and ICM's default embedding model (~2 GB,
# lands in ~/.cache/icm/models). The throwaway database is deleted.
RUN python -c "import tiktoken; tiktoken.get_encoding('cl100k_base')" \
    && icm --db /tmp/warm.db store -t warm -c "warm the embedding model cache" \
    && rm -f /tmp/warm.db*

WORKDIR /work
ENTRYPOINT ["/opt/icm-amb/entrypoint.sh"]
