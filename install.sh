#!/bin/sh
# WVM bootstrap installer.
#
#   curl -fsSL https://raw.githubusercontent.com/tegmentum/wvm/main/install.sh | sh
#
# Establishes the native `wvm` binary. Once installed:
#
#   wvm install latest
#   wvm default latest
#   wvm exec -- --version
#
# `wvm` is a thin native bootstrapper: it downloads and locks a protected seed
# Wasmtime runtime on first use, then runs the WVM application as a WebAssembly
# component on that runtime. All later operations are performed in WASM.

set -eu

REPO="${WVM_REPO:-tegmentum/wvm}"
WVM_HOME="${WVM_HOME:-$HOME/.tegmentum/wvm}"
BIN_DIR="$WVM_HOME/bin"

say() { printf '%s\n' "$*"; }
err() { printf 'error: %s\n' "$*" >&2; exit 1; }

detect_target() {
    os="$(uname -s)"
    arch="$(uname -m)"
    case "$os" in
        Linux) os="linux" ;;
        Darwin) os="macos" ;;
        *) err "unsupported OS: $os" ;;
    esac
    case "$arch" in
        x86_64 | amd64) arch="x86_64" ;;
        arm64 | aarch64) arch="aarch64" ;;
        *) err "unsupported architecture: $arch" ;;
    esac
    printf '%s-%s' "$arch" "$os"
}

verify_checksum() {
    file="$1"
    sumurl="$2"
    expected="$(curl -fsSL "$sumurl" 2>/dev/null | awk '{print $1}')" || return 0
    [ -n "$expected" ] || return 0
    if command -v sha256sum >/dev/null 2>&1; then
        actual="$(sha256sum "$file" | awk '{print $1}')"
    elif command -v shasum >/dev/null 2>&1; then
        actual="$(shasum -a 256 "$file" | awk '{print $1}')"
    else
        say "  (no sha256 tool found; skipping checksum verification)"
        return 0
    fi
    [ "$expected" = "$actual" ] || err "checksum mismatch for $file"
    say "  verified checksum"
}

install_from_release() {
    target="$1"
    asset="wvm-$target"
    base="https://github.com/$REPO/releases/latest/download"
    say "Fetching $asset ..."
    mkdir -p "$BIN_DIR"
    if curl -fsSL "$base/$asset" -o "$BIN_DIR/wvm" 2>/dev/null; then
        verify_checksum "$BIN_DIR/wvm" "$base/$asset.sha256"
        chmod +x "$BIN_DIR/wvm"
        return 0
    fi
    return 1
}

install_from_source() {
    command -v cargo >/dev/null 2>&1 || return 1
    command -v wac >/dev/null 2>&1 || {
        say "  (building from source needs 'wac'; install with: cargo install wac-cli)"
        return 1
    }
    say "No prebuilt binary available; building from source ..."
    tmp="$(mktemp -d)"
    git clone --depth 1 "https://github.com/$REPO" "$tmp/wvm" >/dev/null 2>&1 || return 1
    # `make` builds the wasm app, composes it with the SQLite component, and
    # builds the native bootstrapper with the app embedded.
    ( cd "$tmp/wvm" && rustup target add wasm32-wasip2 >/dev/null 2>&1; make ) || return 1
    mkdir -p "$BIN_DIR"
    cp "$tmp/wvm/target/release/wvm" "$BIN_DIR/wvm"
    chmod +x "$BIN_DIR/wvm"
    rm -rf "$tmp"
    return 0
}

main() {
    command -v curl >/dev/null 2>&1 || err "curl is required"
    target="$(detect_target)"
    say "Installing wvm for $target into $BIN_DIR"

    if ! install_from_release "$target"; then
        install_from_source || err "could not install a prebuilt binary or build from source"
    fi

    say ""
    say "wvm installed to $BIN_DIR/wvm"
    case ":$PATH:" in
        *":$BIN_DIR:"*) ;;
        *)
            say "Add wvm to your PATH:"
            say "    export PATH=\"$BIN_DIR:\$PATH\""
            ;;
    esac
    say ""
    say "On first run, wvm downloads and locks a protected seed Wasmtime runtime,"
    say "then runs as a WebAssembly component on it."
    say ""
    say "Next:"
    say "    wvm install latest    # installs a runtime for your projects"
    say "    wvm default latest    # (or: wvm install lts)"
    say "    wvm exec -- --version"
}

main "$@"
