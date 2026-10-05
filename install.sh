#!/bin/sh
# icm installer — https://github.com/rtk-ai/icm
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/rtk-ai/icm/main/install.sh | sh
#
# Re-run to upgrade. Pass flags via `sh -s --`:
#   curl -fsSL https://raw.githubusercontent.com/rtk-ai/icm/main/install.sh | sh -s -- --version icm-v0.10.28
#   curl -fsSL https://raw.githubusercontent.com/rtk-ai/icm/main/install.sh | sh -s -- --dir /usr/local/bin
#
# Every download is verified against the release's checksums.txt (SHA256).

set -eu

REPO="rtk-ai/icm"
BINARY_NAME="icm"
INSTALL_DIR="${HOME}/.local/bin"
VERSION=""

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

info() { printf "${GREEN}[INFO]${NC} %s\n" "$1"; }
warn() { printf "${YELLOW}[WARN]${NC} %s\n" "$1"; }
error() { printf "${RED}[ERROR]${NC} %s\n" "$1" >&2; exit 1; }

usage() {
    cat <<EOF
icm installer — installs or upgrades icm from GitHub releases.

Usage: install.sh [--dir <path>] [--version <tag>] [--help]

Options:
  --dir <path>      Install directory (default: \$HOME/.local/bin)
  --version <tag>   Release tag to install (default: latest, e.g. icm-v0.10.28)
  -h, --help        Show this help

Re-running this script upgrades an existing installation in place.
Each download is verified against the release's checksums.txt (SHA256).
EOF
}

parse_args() {
    while [ $# -gt 0 ]; do
        case "$1" in
            --dir)
                [ $# -ge 2 ] || error "--dir requires a path"
                INSTALL_DIR="$2"
                shift 2
                ;;
            --version)
                [ $# -ge 2 ] || error "--version requires a tag"
                VERSION="$2"
                shift 2
                ;;
            -h|--help)
                usage
                exit 0
                ;;
            *)
                error "Unknown argument: $1 (use --help)"
                ;;
        esac
    done
}

detect_os() {
    case "$(uname -s)" in
        Darwin*) OS="darwin"; TARGET_SUFFIX="apple-darwin";;
        Linux*)  OS="linux";  TARGET_SUFFIX="unknown-linux-gnu";;
        MINGW*|MSYS*|CYGWIN*) OS="windows"; TARGET_SUFFIX="pc-windows-msvc";;
        *)       error "Unsupported OS: $(uname -s). icm supports macOS, Linux and Windows.";;
    esac
}

detect_arch() {
    case "$(uname -m)" in
        x86_64|amd64)  ARCH="x86_64";;
        arm64|aarch64) ARCH="aarch64";;
        *)             error "Unsupported architecture: $(uname -m)";;
    esac
}

# Choose the Linux libc flavor (issue #330). The default `unknown-linux-gnu`
# binaries are built on glibc 2.35 (the release build fails if they ever need
# more: scripts/release-check.sh) and carry the embedding code; ONNX Runtime
# itself is fetched once with `icm embeddings download`. Systems older than
# that — or musl distros like Alpine — can't run them, so fall back to the
# fully-static `unknown-linux-musl` build (keyword search only, no embeddings).
# musl artifacts are published for x86_64 only; on other architectures the
# gnu build is tried and install_binary refuses it if it does not start.
select_linux_libc() {
    if [ "$ARCH" != "x86_64" ]; then
        TARGET_SUFFIX="unknown-linux-gnu"
        return
    fi
    ldd_out=$(ldd --version 2>&1 | head -1)
    if printf '%s' "$ldd_out" | grep -qi musl; then
        TARGET_SUFFIX="unknown-linux-musl"
        return
    fi
    glibc_ver=$(printf '%s' "$ldd_out" | grep -oE '[0-9]+\.[0-9]+' | tail -1)
    if [ -n "$glibc_ver" ]; then
        glibc_major=${glibc_ver%%.*}
        glibc_minor=${glibc_ver#*.}
        if [ "$glibc_major" -lt 2 ] || { [ "$glibc_major" -eq 2 ] && [ "$glibc_minor" -lt 35 ]; }; then
            TARGET_SUFFIX="unknown-linux-musl"
            return
        fi
    fi
    # Modern glibc (>= 2.35, e.g. Debian 12+/Ubuntu 22.04+) or undetectable:
    # keep the full-featured gnu build.
    TARGET_SUFFIX="unknown-linux-gnu"
}

require() {
    command -v "$1" >/dev/null 2>&1 || error "$1 is required but not installed"
}

get_latest_version() {
    if [ -n "$VERSION" ]; then
        return
    fi
    VERSION=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
        | grep '"tag_name":' \
        | head -1 \
        | sed -E 's/.*"tag_name"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/')
    [ -n "$VERSION" ] || error "Failed to determine latest release from GitHub API"
}

# Print currently installed version, or empty if not installed.
current_version() {
    if [ -x "${INSTALL_DIR}/${BINARY_NAME}" ]; then
        "${INSTALL_DIR}/${BINARY_NAME}" --version 2>/dev/null | awk '{print $2}'
    elif command -v "$BINARY_NAME" >/dev/null 2>&1; then
        "$BINARY_NAME" --version 2>/dev/null | awk '{print $2}'
    fi
}

verify_sha256() {
    archive="$1"
    expected="$2"
    if command -v sha256sum >/dev/null 2>&1; then
        actual=$(sha256sum "$archive" | awk '{print $1}')
    elif command -v shasum >/dev/null 2>&1; then
        actual=$(shasum -a 256 "$archive" | awk '{print $1}')
    else
        error "Neither sha256sum nor shasum found — cannot verify integrity. Install one and retry."
    fi
    if [ "$expected" != "$actual" ]; then
        error "SHA256 mismatch — refusing to install.
  expected: ${expected}
  got:      ${actual}
The download was tampered with or corrupted."
    fi
    info "SHA256 verified: ${actual}"
}

install_binary() {
    # On Linux, pick gnu vs static-musl based on the running libc (#330).
    if [ "$OS" = "linux" ]; then
        select_linux_libc
        if [ "$TARGET_SUFFIX" = "unknown-linux-musl" ]; then
            info "Old or musl libc detected — installing the fully-static build (keyword search only; no embeddings)."
        fi
    fi
    TARGET="${ARCH}-${TARGET_SUFFIX}"

    if [ "$OS" = "windows" ]; then
        # Only x86_64 is published for Windows (same rule as install.ps1).
        [ "$ARCH" = "x86_64" ] || error "No prebuilt icm for ${ARCH} Windows. Build from source: cargo install --git https://github.com/${REPO} icm-cli"
        EXT="zip"
        : "${INSTALL_DIR:=${LOCALAPPDATA:-$HOME}/icm/bin}"
    else
        EXT="tar.gz"
    fi

    mkdir -p "$INSTALL_DIR" || error "Cannot create install directory: $INSTALL_DIR"

    ARCHIVE_NAME="${BINARY_NAME}-${TARGET}.${EXT}"
    BASE_URL="https://github.com/${REPO}/releases/download/${VERSION}"
    TEMP_DIR=$(mktemp -d)
    # Staged next to the destination so the final move is a rename on the
    # same filesystem, and so the test run below works even when the temp
    # directory is mounted noexec.
    STAGED="${INSTALL_DIR}/.${BINARY_NAME}.install.$$"
    trap 'rm -rf "$TEMP_DIR" "$STAGED"' EXIT

    ARCHIVE="${TEMP_DIR}/${ARCHIVE_NAME}"
    info "Downloading ${ARCHIVE_NAME}"
    curl -fsSL "${BASE_URL}/${ARCHIVE_NAME}" -o "$ARCHIVE" \
        || error "Failed to download ${BASE_URL}/${ARCHIVE_NAME}"

    # SHA256 verification — mandatory, never skipped.
    CHECKSUMS_FILE="${TEMP_DIR}/checksums.txt"
    info "Downloading checksums.txt"
    curl -fsSL "${BASE_URL}/checksums.txt" -o "$CHECKSUMS_FILE" \
        || error "Failed to download checksums.txt (required for integrity verification)"

    # checksums.txt format: "<sha256>  <filename>" (two spaces, sha256sum default).
    EXPECTED_SHA=$(awk -v name="$ARCHIVE_NAME" '$2 == name {print $1; exit}' "$CHECKSUMS_FILE")
    [ -n "$EXPECTED_SHA" ] || error "No checksum entry for ${ARCHIVE_NAME} in checksums.txt"
    verify_sha256 "$ARCHIVE" "$EXPECTED_SHA"

    info "Extracting"
    if [ "$OS" = "windows" ]; then
        require unzip
        unzip -oq "$ARCHIVE" -d "$TEMP_DIR"
        DEST="${INSTALL_DIR}/${BINARY_NAME}.exe"
        STAGED="${STAGED}.exe"
        EXTRACTED="${TEMP_DIR}/${BINARY_NAME}.exe"
    else
        tar -xzf "$ARCHIVE" -C "$TEMP_DIR"
        DEST="${INSTALL_DIR}/${BINARY_NAME}"
        EXTRACTED="${TEMP_DIR}/${BINARY_NAME}"
    fi
    [ -f "$EXTRACTED" ] || error "${ARCHIVE_NAME} does not contain ${BINARY_NAME}"

    # Start the new binary before it replaces anything: a build that cannot
    # run here (libc too old, wrong architecture) must not take the place of
    # a working install.
    cp "$EXTRACTED" "$STAGED"
    chmod +x "$STAGED"
    if ! "$STAGED" --version >/dev/null 2>&1; then
        rm -f "$STAGED"
        error "The downloaded binary (${TARGET}) does not start on this system — nothing was installed or replaced.
On Linux the prebuilt binaries need glibc >= 2.35, or x86_64 for the static musl build.
Build from source instead: cargo install --git https://github.com/${REPO} icm-cli"
    fi
    mv -f "$STAGED" "$DEST"

    info "Installed to ${DEST}"
}

# Say what this build can do for semantic search, in the binary's own words:
# some builds link ONNX Runtime, others fetch it on demand, the static musl
# build is keyword-only. Silent on releases that predate the subcommand.
#
# A build that leaves semantic search off until the user acts gets a warning
# on top: icm itself only offers the download in a terminal, and says nothing
# as an MCP server or from hooks. On an upgrade the warning matters most:
# releases up to 0.10.63 carried the runtime on Linux and on Intel Macs, so
# the upgrade is what turns semantic search off.
# The patterns below are the wording of `icm embeddings status`; the release
# build fails if that wording changes (scripts/release-check.sh, smoke).
print_embeddings_status() {
    status=$("$DEST" embeddings status 2>/dev/null) || return 0
    [ -n "$status" ] || return 0
    echo "  Semantic search:"
    printf '%s\n' "$status" | sed 's/^/    /'
    echo ""
    case "$status" in
        *"not installed"*"icm embeddings download"*)
            warn "Semantic search is OFF until you run: ${BINARY_NAME} embeddings download"
            ;;
        *"no prebuilt runtime"*)
            warn "Semantic search is OFF: no ONNX Runtime can be downloaded for this platform."
            warn "To enable it, set ORT_DYLIB_PATH to your own ONNX Runtime library (see above)."
            ;;
        *)
            return 0
            ;;
    esac
    warn "Until then icm uses keyword search only. As an MCP server and from hooks it does not ask."
    if [ -n "$PREVIOUS_VERSION" ]; then
        warn "If semantic search worked before this upgrade, it does not any more."
    fi
    echo ""
}

print_path_warning() {
    case ":${PATH:-}:" in
        *":${INSTALL_DIR}:"*) ;;
        *)
            warn "${INSTALL_DIR} is not in your PATH. Add it with:"
            printf '  export PATH="%s:$PATH"\n' "$INSTALL_DIR"
            ;;
    esac
}

main() {
    parse_args "$@"
    require curl
    require uname

    detect_os
    detect_arch
    info "Platform: ${OS} ${ARCH}"

    PREVIOUS_VERSION=$(current_version || true)
    get_latest_version
    info "Target version: ${VERSION}"

    if [ -n "$PREVIOUS_VERSION" ]; then
        info "Upgrading icm (current: ${PREVIOUS_VERSION})"
    else
        info "Installing icm"
    fi

    install_binary

    echo ""
    if [ -n "$PREVIOUS_VERSION" ]; then
        info "Upgrade complete: ${PREVIOUS_VERSION} → ${VERSION}"
    else
        info "Installation complete: ${VERSION}"
    fi
    echo ""
    echo "  Next steps:"
    echo "    1. icm init              # configure your AI tools (MCP)"
    echo "    2. icm init --mode hook  # install Claude Code hooks"
    echo "    3. Restart your AI tool to activate"
    echo ""
    print_embeddings_status
    print_path_warning
}

main "$@"
