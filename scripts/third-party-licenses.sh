#!/bin/sh
# Write THIRD-PARTY-LICENSES.md: the licenses of everything built into the
# icm binary, shipped next to LICENSE in every release archive and package
# (.github/workflows/release-build.yml, issue #514).
#
#   third-party-licenses.sh <output-file>
#
# Rust crates come from cargo-about (licenses/about.toml, about.hbs), with
# every feature on so one file covers all release builds. Components that are
# not crates (ONNX Runtime, SQLite, the web dashboard's bundle) are in
# licenses/other-components.md, kept by hand.
set -eu

out="${1:?usage: third-party-licenses.sh <output-file>}"
root=$(cd "$(dirname "$0")/.." && pwd)

cargo about --version >/dev/null 2>&1 \
    || { echo "cargo-about is required (cargo install cargo-about --locked --features cli)" >&2; exit 1; }

crates=$(mktemp)
trap 'rm -f "$crates"' EXIT
cargo about generate --manifest-path "$root/Cargo.toml" \
    -c "$root/licenses/about.toml" --all-features \
    "$root/licenses/about.hbs" -o "$crates"

{
    printf '# Third-party licenses\n\n'
    printf 'icm is licensed under the Apache License 2.0 (see LICENSE). It is built with the open-source components below, each under its own license.\n\n'
    cat "$root/licenses/other-components.md"
    printf '\n'
    cat "$crates"
} > "$out"
