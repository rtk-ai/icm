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
    TARGET="${ARCH}-${TARGET_SUFFIX}"

    if [ "$OS" = "windows" ]; then
        EXT="zip"
        : "${INSTALL_DIR:=${LOCALAPPDATA:-$HOME}/icm/bin}"
    else
        EXT="tar.gz"
    fi

    mkdir -p "$INSTALL_DIR" || error "Cannot create install directory: $INSTALL_DIR"

    ARCHIVE_NAME="${BINARY_NAME}-${TARGET}.${EXT}"
    BASE_URL="https://github.com/${REPO}/releases/download/${VERSION}"
    TEMP_DIR=$(mktemp -d)
    trap 'rm -rf "$TEMP_DIR"' EXIT

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
        mv -f "${TEMP_DIR}/${BINARY_NAME}.exe" "$DEST"
    else
        tar -xzf "$ARCHIVE" -C "$TEMP_DIR"
        DEST="${INSTALL_DIR}/${BINARY_NAME}"
        mv -f "${TEMP_DIR}/${BINARY_NAME}" "$DEST"
        chmod +x "$DEST"
    fi

    info "Installed to ${DEST}"
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
    print_path_warning
}

main "$@"
