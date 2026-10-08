#!/bin/sh
# Release artifact contract for icm.
#
# This file is the single place that says which files a release must carry
# and what each one must be able to do. The release workflows
# (.github/workflows/release-build.yml, release.yml) call it; nothing here
# publishes anything, so it is safe to run locally.
#
#   release-check.sh list <version>
#       Print the expected asset names, one per line (checksums.txt last).
#   release-check.sh smoke <artifact> [expected-version]
#       Unpack one artifact, start the binary (`icm --version`) and check it
#       has the embeddings capability the manifest promises.
#   release-check.sh version <archive.tar.gz>
#       Print the version the packaged binary reports.
#   release-check.sh assemble <dir> <version>
#       <dir> must hold exactly the expected assets; writes <dir>/checksums.txt.
#   release-check.sh verify <dir> <version>
#       <dir> must hold exactly the expected assets plus a checksums.txt whose
#       SHA256 sums all match. Used before upload and again on what GitHub
#       serves back.
#   release-check.sh notes <version>
#       Print the Markdown block that goes at the top of the release notes:
#       what each file can do for semantic search, from the same manifest.
#
# <version> is the Cargo package version (0.10.65), not the git tag.
#
# Set RELEASE_CHECK_OFFLINE=1 to skip the smoke steps that need the network
# (the ONNX Runtime download on the Linux gnu archives, and the semantic
# round trip below).
# Set RELEASE_CHECK_SEMANTIC=1 to also store a memory and find it again by
# meaning alone with every build that carries or can fetch ONNX Runtime. It
# downloads a small embedding model (about 90 MB) from Hugging Face, so the
# pull-request check asks for it and the release itself does not.

set -eu

die() {
    # ::error:: makes the line an annotation on GitHub; it is harmless elsewhere.
    printf '::error::%s\n' "$*" >&2
    exit 1
}

# name | embeddings | max glibc | extra
#
# embeddings:
#   static   ONNX Runtime is linked in: semantic search works as shipped.
#   dynamic  The embedding code is in, ONNX Runtime is loaded at run time
#            (`icm embeddings download`, or ORT_DYLIB_PATH).
#   none     Keyword search only.
# max glibc: highest glibc symbol version the binary may need ("-" = n/a).
#            install.sh installs the gnu archives on glibc >= 2.35 and every
#            `icm upgrade` in the field fetches them, so this must not move
#            without changing install.sh (select_linux_libc) with it.
# extra:
#   runtime-download  `icm embeddings download` must succeed on the build host.
#   runtime-byo       no ONNX Runtime can be downloaded for this platform:
#                     the user has to bring one (ORT_DYLIB_PATH).
#   static-binary     must not depend on any shared library.
#   alias:<name>      must be byte-identical to <name>.
#
# A "dynamic" build leaves semantic search off until the user acts, so three
# things tell them and are checked here with it: the wording of
# `icm embeddings status` that install.sh turns into a warning, the note a
# dynamic .deb prints when it is installed, and the release notes (`notes`).
#
# Why not "static" everywhere: ort 2.0.0-rc.13 ships no prebuilt ONNX Runtime
# for x86_64-apple-darwin, and its Linux prebuilts need glibc >= 2.38 and a
# GCC 13 libstdc++, which rules out the glibc 2.35 baseline. musl has no
# ONNX Runtime at all.
manifest() {
    cat <<EOF
icm-aarch64-apple-darwin.tar.gz|static|-|-
icm-x86_64-apple-darwin.tar.gz|dynamic|-|runtime-byo
icm-x86_64-unknown-linux-gnu.tar.gz|dynamic|2.35|runtime-download
icm-aarch64-unknown-linux-gnu.tar.gz|dynamic|2.35|runtime-download
icm-x86_64-unknown-linux-musl.tar.gz|none|-|static-binary
icm-x86_64-pc-windows-msvc.zip|static|-|-
icm_amd64.deb|dynamic|2.35|-
icm-cli_${1}-1_amd64.deb|dynamic|2.35|alias:icm_amd64.deb
icm.x86_64.rpm|static|-|-
icm-${1}-1.x86_64.rpm|static|-|alias:icm.x86_64.rpm
icm-cli-${1}-1.x86_64.rpm|static|-|alias:icm.x86_64.rpm
EOF
}

field() {
    printf '%s\n' "$1" | cut -d'|' -f"$2"
}

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | awk '{print $1}'
    else
        die "neither sha256sum nor shasum is available"
    fi
}

need_version() {
    case "${1:-}" in
        ''|*[!0-9A-Za-z.+-]*) die "a package version is required (e.g. 0.10.65), got '${1:-}'" ;;
    esac
}

cmd_list() {
    need_version "${1:-}"
    manifest "$1" | cut -d'|' -f1
    echo "checksums.txt"
}

# Unpack $1 into $2 and print the path of the icm binary it carries.
unpack() {
    artifact="$1"
    dest="$2"
    case "$artifact" in
        *.tar.gz)
            tar -xzf "$artifact" -C "$dest"
            echo "$dest/icm"
            ;;
        *.zip)
            if command -v unzip >/dev/null 2>&1; then
                unzip -oq "$artifact" -d "$dest"
            elif command -v 7z >/dev/null 2>&1; then
                7z x -y "-o$dest" "$artifact" >/dev/null
            else
                die "unzip or 7z is required to unpack $artifact"
            fi
            echo "$dest/icm.exe"
            ;;
        *.deb)
            dpkg-deb -x "$artifact" "$dest"
            echo "$dest/usr/bin/icm"
            ;;
        *.rpm)
            abs=$(cd "$(dirname "$artifact")" && pwd)/$(basename "$artifact")
            (cd "$dest" && rpm2cpio "$abs" | cpio -idm --quiet)
            echo "$dest/usr/bin/icm"
            ;;
        *)
            die "unknown artifact type: $artifact"
            ;;
    esac
}

new_workdir() {
    base="${RUNNER_TEMP:-${TMPDIR:-/tmp}}"
    # Git Bash on the Windows runner: turn D:\a\_temp into /d/a/_temp.
    if command -v cygpath >/dev/null 2>&1; then
        base=$(cygpath -u "$base")
    fi
    mktemp -d "${base%/}/icm-release-check.XXXXXX"
}

# Create the throwaway HOME the binary under test runs in, with its own
# configuration file. The only setting is a small embedding model, used by
# the semantic round trip; nothing else loads a model.
new_sandbox() {
    SANDBOX_HOME="$1"
    mkdir -p "$SANDBOX_HOME"
    printf '[embeddings]\nmodel = "Qdrant/all-MiniLM-L6-v2-onnx"\n' > "$SANDBOX_HOME/icm.toml"
}

# Run the binary under test in that sandbox so it never reads or writes the
# invoking user's icm configuration, data or model cache.
run_icm() {
    HOME="$SANDBOX_HOME" XDG_CONFIG_HOME="$SANDBOX_HOME/.config" \
        XDG_DATA_HOME="$SANDBOX_HOME/.local/share" XDG_CACHE_HOME="$SANDBOX_HOME/.cache" \
        ICM_CONFIG="$SANDBOX_HOME/icm.toml" \
        "$@"
}

# Store one memory and find it again with a query that shares no word with
# it: only vector search can return it, so a hit proves this very binary
# loaded ONNX Runtime and embedded both texts. The same query with
# embeddings switched off must come back empty, or the hit proves nothing
# (0.10.65 answers a query holding the word "a" by keywords alone).
#
# The model comes from Hugging Face. When the binary cannot embed, it still
# stores the memory and says "embedding failed" on stderr: that wording only
# decides whether to try again (a download that failed once must not cost a
# whole rebuild of the target), never whether the check passes.
semantic_round_trip() {
    text="the quick brown fox jumps over the lazy dog"
    query="speedy animal leaping above sleepy canine"
    attempt=1
    while :; do
        db="$work/semantic-$attempt.db"
        if ! run_icm "$bin" --db "$db" store -t release-check -c "$text" >/dev/null 2>"$work/store.err"; then
            cat "$work/store.err" >&2
            die "$name: could not store a memory with embeddings on"
        fi
        cat "$work/store.err" >&2
        grep -q 'embedding failed' "$work/store.err" || break
        [ "$attempt" -lt 3 ] \
            || die "$name: semantic search does not work: the binary could not embed a text in $attempt attempts (its warnings above say whether the model download or ONNX Runtime failed)"
        echo "attempt $attempt: the binary could not embed the text, trying again"
        attempt=$((attempt + 1))
        sleep 10
    done
    found=$(run_icm "$bin" --db "$db" recall "$query") \
        || die "$name: recall failed with embeddings on"
    case "$found" in
        *fox*) ;;
        *) die "$name: semantic search does not work: a query by meaning did not find the stored memory" ;;
    esac
    control=$(run_icm "$bin" --no-embeddings --db "$db" recall "$query") \
        || die "$name: recall failed with --no-embeddings"
    case "$control" in
        *fox*) die "$name: the semantic check is void: keyword search alone finds the memory" ;;
    esac
    echo "semantic round trip OK"
}

cmd_version() {
    artifact="${1:-}"
    [ -s "$artifact" ] || die "artifact missing or empty: $artifact"
    work=$(new_workdir)
    trap 'rm -rf "$work"' EXIT
    new_sandbox "$work/home"
    mkdir -p "$work/x"
    bin=$(unpack "$artifact" "$work/x")
    out=$(run_icm "$bin" --version) || die "$(basename "$artifact"): 'icm --version' did not run"
    printf '%s\n' "$out" | awk 'NR == 1 {print $2}'
}

cmd_smoke() {
    artifact="${1:-}"
    expected="${2:-}"
    [ -s "$artifact" ] || die "artifact missing or empty: $artifact"
    name=$(basename "$artifact")
    # Only version-free names are smoke-tested; the versioned .deb/.rpm names
    # are aliases whose identity `assemble` checks byte for byte.
    row=$(manifest VERSION | awk -F'|' -v n="$name" '$1 == n')
    [ -n "$row" ] || die "$name is not in the release manifest (scripts/release-check.sh)"
    mode=$(field "$row" 2)
    glibc_max=$(field "$row" 3)
    extra=$(field "$row" 4)

    work=$(new_workdir)
    trap 'rm -rf "$work"' EXIT
    new_sandbox "$work/home"
    mkdir -p "$work/x"
    bin=$(unpack "$artifact" "$work/x")
    [ -f "$bin" ] || die "$name does not contain ${bin#"$work"/x/}"

    # 1. The binary starts.
    out=$(run_icm "$bin" --version) || die "$name: 'icm --version' did not run on $(uname -sm)"
    case "$out" in
        "icm "[0-9]*) ;;
        *) die "$name: unexpected --version output: $out" ;;
    esac
    version=$(printf '%s\n' "$out" | awk 'NR == 1 {print $2}')
    if [ -n "$expected" ] && [ "$version" != "$expected" ]; then
        die "$name reports version $version but the release is $expected (built from the wrong commit?)"
    fi

    # 2. It carries the embeddings capability the manifest promises. The
    #    phrases come from `icm embeddings status` (crates/icm-cli/src/main.rs,
    #    ort_runtime.rs); if that wording changes, update the patterns here.
    status=$(run_icm "$bin" embeddings status 2>&1) || die "$name: 'icm embeddings status' failed: $status"
    case "$mode" in
        static)
            printf '%s\n' "$status" | grep -qi 'statically linked' \
                || die "$name must link ONNX Runtime statically, but reports: $status"
            ;;
        dynamic)
            if printf '%s\n' "$status" | grep -qi 'statically linked' \
                || ! printf '%s\n' "$status" | grep -qi 'onnxruntime'; then
                die "$name must load ONNX Runtime at run time, but reports: $status"
            fi
            # In a fresh home the runtime is not there yet. install.sh keys
            # its warning on these exact words (print_embeddings_status).
            if [ "$extra" = "runtime-byo" ]; then
                printf '%s\n' "$status" | grep -q 'no prebuilt runtime' \
                    || die "$name: install.sh expects 'no prebuilt runtime' in the status, got: $status"
            else
                if ! printf '%s\n' "$status" | grep -q 'not installed' \
                    || ! printf '%s\n' "$status" | grep -q 'icm embeddings download'; then
                    die "$name: install.sh expects 'not installed' and 'icm embeddings download' in the status, got: $status"
                fi
            fi
            # A package manager runs no icm command for the user: the
            # package itself has to say that semantic search needs one.
            case "$name" in
                *.deb)
                    mkdir -p "$work/control"
                    dpkg-deb -e "$artifact" "$work/control"
                    [ -f "$work/control/postinst" ] \
                        || die "$name has no postinst: installing it would say nothing about 'icm embeddings download'"
                    note=$(sh "$work/control/postinst" configure) \
                        || die "$name: its postinst fails, the package would not install"
                    printf '%s\n' "$note" | grep -q 'icm embeddings download' \
                        || die "$name: its postinst does not mention 'icm embeddings download'"
                    ;;
            esac
            ;;
        none)
            printf '%s\n' "$status" | grep -qi 'without embeddings' \
                || die "$name must be a keyword-only build, but reports: $status"
            ;;
        *)
            die "manifest error: unknown embeddings mode '$mode' for $name"
            ;;
    esac

    # 3. It does not need a newer glibc than install.sh assumes.
    glibc_need="-"
    if [ "$glibc_max" != "-" ]; then
        command -v objdump >/dev/null 2>&1 || die "objdump is required to check the glibc baseline of $name"
        glibc_need=$(objdump -T "$bin" 2>/dev/null \
            | grep -oE 'GLIBC_[0-9]+(\.[0-9]+)+' | sed 's/^GLIBC_//' | sort -u -V | tail -1)
        [ -n "$glibc_need" ] || die "$name: could not read the glibc symbol versions"
        highest=$(printf '%s\n%s\n' "$glibc_need" "$glibc_max" | sort -V | tail -1)
        [ "$highest" = "$glibc_max" ] \
            || die "$name needs glibc $glibc_need, above the $glibc_max baseline install.sh and 'icm upgrade' rely on"
    fi

    case "$extra" in
        static-binary)
            ldd "$bin" 2>&1 | grep -Eq 'statically linked|not a dynamic executable' \
                || die "$name must be fully static, but ldd reports shared libraries"
            ;;
        runtime-download)
            # 4. The runtime this build asks for can be fetched and unpacked
            #    here. This shows the file is in place, not that the binary
            #    can load it: only the round trip below does.
            if [ "${RELEASE_CHECK_OFFLINE:-0}" = "1" ]; then
                echo "skipped (RELEASE_CHECK_OFFLINE=1): icm embeddings download"
            else
                run_icm "$bin" embeddings download \
                    || die "$name: 'icm embeddings download' failed, semantic search cannot be enabled"
                run_icm "$bin" embeddings status | grep -q ': installed' \
                    || die "$name: ONNX Runtime not reported as installed after the download"
            fi
            ;;
    esac

    # 5. On request: semantic search works end to end with this binary, for
    #    the builds that link the runtime and the ones that just fetched it.
    semantic="not run"
    if [ "${RELEASE_CHECK_SEMANTIC:-0}" = "1" ] && [ "${RELEASE_CHECK_OFFLINE:-0}" != "1" ]; then
        if [ "$mode" = "static" ] || [ "$extra" = "runtime-download" ]; then
            semantic_round_trip
            semantic="ok"
        fi
    fi

    echo "OK $name: $out, embeddings=$mode, glibc<=$glibc_need, semantic round trip: $semantic"
    if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
        echo "- \`$name\`: $out, embeddings **$mode**, glibc needed: $glibc_need, semantic round trip: $semantic" >> "$GITHUB_STEP_SUMMARY"
    fi
}

# $1 = dir, $2 = version, $3 = "with-checksums" when checksums.txt must be there.
check_set() {
    dir="$1"
    version="$2"
    with_checksums="${3:-}"
    [ -d "$dir" ] || die "not a directory: $dir"
    expected=$(manifest "$version" | cut -d'|' -f1)
    [ "$with_checksums" = "with-checksums" ] && expected="$expected
checksums.txt"

    missing=""
    for name in $expected; do
        [ -s "$dir/$name" ] || missing="$missing $name"
    done
    unexpected=""
    for path in "$dir"/* "$dir"/.[!.]*; do
        [ -e "$path" ] || continue
        name=$(basename "$path")
        printf '%s\n' "$expected" | grep -Fxq -- "$name" || unexpected="$unexpected $name"
    done
    if [ -n "$missing" ] || [ -n "$unexpected" ]; then
        [ -n "$missing" ] && printf '::error::missing or empty release asset:%s\n' "$missing" >&2
        [ -n "$unexpected" ] && printf '::error::file not in the release manifest:%s\n' "$unexpected" >&2
        die "the release asset set is not the expected one (scripts/release-check.sh)"
    fi

    # Aliases must be the very same bytes as the file they stand for.
    manifest "$version" | while IFS='|' read -r name _ _ extra; do
        case "$extra" in
            alias:*)
                target=${extra#alias:}
                cmp -s "$dir/$name" "$dir/$target" || die "$name differs from $target"
                ;;
        esac
    done || exit 1
}

cmd_assemble() {
    dir="${1:-}"
    need_version "${2:-}"
    rm -f "$dir/checksums.txt"
    check_set "$dir" "$2"
    # "<sha256>  <name>": the format install.sh, install.ps1, `icm upgrade`
    # and the Homebrew job all parse.
    : > "$dir/checksums.txt.tmp"
    for name in $(manifest "$2" | cut -d'|' -f1 | LC_ALL=C sort); do
        printf '%s  %s\n' "$(sha256_of "$dir/$name")" "$name" >> "$dir/checksums.txt.tmp"
    done
    mv "$dir/checksums.txt.tmp" "$dir/checksums.txt"
    echo "OK $(manifest "$2" | wc -l | tr -d ' ') assets + checksums.txt in $dir"
}

cmd_verify() {
    dir="${1:-}"
    need_version "${2:-}"
    check_set "$dir" "$2" with-checksums
    for name in $(manifest "$2" | cut -d'|' -f1); do
        want=$(awk -v n="$name" '$2 == n {print $1; exit}' "$dir/checksums.txt")
        [ -n "$want" ] || die "checksums.txt has no entry for $name"
        got=$(sha256_of "$dir/$name")
        [ "$want" = "$got" ] || die "SHA256 mismatch for $name: checksums.txt says $want, file is $got"
    done
    lines=$(grep -c . "$dir/checksums.txt")
    count=$(manifest "$2" | wc -l | tr -d ' ')
    [ "$lines" -eq "$count" ] || die "checksums.txt lists $lines files, expected $count"
    echo "OK $count assets match checksums.txt in $dir"
}

# The block release.yml puts at the top of the release notes. One line per
# file a user can pick (aliases are the same bytes under another name).
cmd_notes() {
    need_version "${1:-}"
    cat <<'EOF'
<!-- icm:semantic-search -->
### Semantic search: check the build you install

Keyword search works in every build. Semantic (vector) search needs ONNX Runtime, and not every file below carries it:

| File | Semantic search |
|---|---|
EOF
    manifest "$1" | while IFS='|' read -r name mode _ extra; do
        case "$extra" in alias:*) continue ;; esac
        case "$mode:$extra" in
            static:*) what="Built in, nothing to do" ;;
            dynamic:runtime-byo) what="Only with your own ONNX Runtime 1.24 or newer, through \`ORT_DYLIB_PATH\`" ;;
            dynamic:*) what="After \`icm embeddings download\`, once per user (about 11 MB)" ;;
            none:*) what="Not available, keyword search only" ;;
            *) die "manifest error: unknown embeddings mode '$mode' for $name" ;;
        esac
        # shellcheck disable=SC2016  # the backticks are Markdown
        printf '| `%s` | %s |\n' "$name" "$what"
    done || exit 1
    cat <<'EOF'

`install.sh`, `icm upgrade` and the Homebrew tap install these same archives.

**Updating on Linux (archive, `.deb`, `icm upgrade`, Homebrew) or on an Intel Mac from 0.10.63 or older:** those builds carried the runtime, these do not. Run `icm embeddings download` once after the update (on an Intel Mac, set `ORT_DYLIB_PATH`). Until then `icm` answers with keyword search only, also as an MCP server and from hooks, where it cannot ask you. `icm embeddings status` shows where you stand.
EOF
}

command="${1:-}"
[ $# -gt 0 ] && shift
case "$command" in
    list) cmd_list "$@" ;;
    notes) cmd_notes "$@" ;;
    smoke) cmd_smoke "$@" ;;
    version) cmd_version "$@" ;;
    assemble) cmd_assemble "$@" ;;
    verify) cmd_verify "$@" ;;
    *)
        sed -n '2,/^$/p' "$0" | sed 's/^# \{0,1\}//'
        exit 2
        ;;
esac
