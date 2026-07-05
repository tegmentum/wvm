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

# The version of an installed wvm binary (empty if absent). `--version` returns
# before any runtime bootstrap, so this is safe and fast to call.
wvm_version() {
    [ -x "$1" ] || return 0
    "$1" --version 2>/dev/null | awk '{print $2}'
}

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
    # Download to a temp path and swap into place only after the checksum
    # passes, so an interrupted or corrupt re-download (e.g. a user re-running
    # the installer to upgrade) never clobbers a working binary.
    tmp="$BIN_DIR/.wvm.download"
    if curl -fL --progress-bar "$base/$asset" -o "$tmp"; then
        verify_checksum "$tmp" "$base/$asset.sha256"
        chmod +x "$tmp"
        mv -f "$tmp" "$BIN_DIR/wvm"
        return 0
    fi
    rm -f "$tmp"
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

# Write a shell-sourceable env file that prepends wvm's bin dir to PATH.
# Kept POSIX-compatible so bash, zsh, and plain sh can all source it.
write_env_file() {
    cat > "$WVM_HOME/env" <<EOF
#!/bin/sh
# wvm shell setup. Prepends the wvm bin directory to PATH.
# This file is sourced from your shell rc; edit WVM_HOME to relocate.
case ":\${PATH}:" in
    *:"$BIN_DIR":*) ;;
    *) export PATH="$BIN_DIR:\$PATH" ;;
esac
EOF
    # Fold in the shim-on-PATH setup and the `wvm use` hook so `use`/`deactivate`
    # work straight away — no separate `wvm shell-init >> ~/.rc` step. Generated
    # by the binary (native, offline) so the hook has a single source of truth.
    if [ -x "$BIN_DIR/wvm" ]; then
        "$BIN_DIR/wvm" shell-init >> "$WVM_HOME/env" 2>/dev/null || true
    fi
    # fish uses a different syntax, so give it its own env file. The POSIX `use`
    # hook isn't fish-compatible, so fish gets the PATH wiring only.
    cat > "$WVM_HOME/env.fish" <<EOF
# wvm shell setup. Prepends the wvm bin and shim directories to PATH.
if not contains "$BIN_DIR" \$PATH
    set -gx PATH "$BIN_DIR" \$PATH
end
if not contains "$WVM_HOME/shims" \$PATH
    set -gx PATH "$WVM_HOME/shims" \$PATH
end
EOF
}

# Base marker tagging every line this installer manages, so a re-install can
# find and replace its own prior lines (even a stale path from an older install)
# without disturbing anything the user added by hand. Each purpose (env,
# completions) gets its own tagged marker so several managed lines coexist.
WVM_MARKER="# wvm-managed"

# Install a managed line into an rc file. `tag` distinguishes purposes (e.g.
# `env`, `completions`); only lines carrying that same tag are replaced, so
# unrelated managed lines survive. Drops any prior line for this tag (stale
# path/format), then appends the current one. Flags CONFIG_CHANGED and reports
# only when the file actually changed.
wire_rc() {
    body="$1"
    file="$2"
    tag="$3"
    verb="${4:-updated}"
    marker="$WVM_MARKER:$tag"
    new_line="$body $marker"
    [ -e "$file" ] || { mkdir -p "$(dirname "$file")" && : > "$file"; }
    # Already exactly right? Leave it untouched. Must still return 0: these calls
    # often sit at the tail of a `&&`/bare statement, and a non-zero return here
    # would trip `set -e` and abort the whole installer on any re-run.
    if grep -qxF -- "$new_line" "$file" 2>/dev/null; then
        return 0
    fi
    # Strip any prior line for this tag, then append the fresh one.
    tmp="$file.wvm.$$"
    grep -vF -- "$marker" "$file" > "$tmp" 2>/dev/null || : > "$tmp"
    printf '%s\n' "$new_line" >> "$tmp"
    mv "$tmp" "$file"
    say "  $verb $file"
    CONFIG_CHANGED=1
    return 0
}

# Wire the env file into the user's shell startup based on their login shell.
# Sets SOURCE_CMD to the command the user can run to update their current shell,
# and CONFIG_CHANGED=1 if any rc file was actually modified this run.
configure_shell() {
    write_env_file
    shell_name="$(basename "${SHELL:-}")"
    posix_line=". \"$WVM_HOME/env\""
    SOURCE_CMD=""
    CONFIG_CHANGED=0
    case "$shell_name" in
        bash)
            # .bashrc for interactive shells; .bash_profile/.profile for login.
            for rc in "$HOME/.bashrc" "$HOME/.bash_profile" "$HOME/.profile"; do
                [ -e "$rc" ] && wire_rc "$posix_line" "$rc" env
            done
            # Guarantee at least one file carries it.
            [ -e "$HOME/.bashrc" ] || wire_rc "$posix_line" "$HOME/.bashrc" env created
            SOURCE_CMD="source \"$WVM_HOME/env\""
            ;;
        zsh)
            zdir="${ZDOTDIR:-$HOME}"
            wire_rc "$posix_line" "$zdir/.zshrc" env
            SOURCE_CMD="source \"$WVM_HOME/env\""
            ;;
        fish)
            # fish auto-loads files in conf.d on startup.
            confd="${XDG_CONFIG_HOME:-$HOME/.config}/fish/conf.d"
            mkdir -p "$confd"
            wire_rc "source \"$WVM_HOME/env.fish\"" "$confd/wvm.fish" env
            SOURCE_CMD="source \"$WVM_HOME/env.fish\""
            ;;
        *)
            wire_rc "$posix_line" "$HOME/.profile" env
            SOURCE_CMD=". \"$WVM_HOME/env\""
            ;;
    esac
    [ "$CONFIG_CHANGED" -eq 0 ] && say "  shell already configured for wvm"
    return 0
}

# Generate and install a shell completion script for the detected shell, using
# the freshly installed binary (native `wvm completions` needs no runtime).
# fish auto-loads its completions directory; bash/zsh get a managed source line.
# Relies on `shell_name` set by configure_shell. Best-effort: a binary without
# the `completions` subcommand (e.g. an older source build) is silently skipped.
install_completions() {
    wvm_bin="$BIN_DIR/wvm"
    [ -x "$wvm_bin" ] || return 0
    comp_dir="$WVM_HOME/completions"
    mkdir -p "$comp_dir"
    case "$shell_name" in
        bash)
            "$wvm_bin" completions bash > "$comp_dir/wvm.bash" 2>/dev/null || return 0
            wire_rc "source \"$comp_dir/wvm.bash\"" "$HOME/.bashrc" completions
            ;;
        zsh)
            "$wvm_bin" completions zsh > "$comp_dir/_wvm" 2>/dev/null || return 0
            wire_rc "source \"$comp_dir/_wvm\"" "${ZDOTDIR:-$HOME}/.zshrc" completions
            ;;
        fish)
            fdir="${XDG_CONFIG_HOME:-$HOME/.config}/fish/completions"
            mkdir -p "$fdir"
            "$wvm_bin" completions fish > "$fdir/wvm.fish" 2>/dev/null || return 0
            say "  installed fish completions to $fdir/wvm.fish"
            ;;
    esac
}

main() {
    command -v curl >/dev/null 2>&1 || err "curl is required"
    target="$(detect_target)"

    # Re-running the installer is a supported upgrade/repair path: note any
    # existing version so we can report the transition rather than silently
    # overwriting.
    prev_version="$(wvm_version "$BIN_DIR/wvm")"
    if [ -n "$prev_version" ]; then
        say "Found wvm $prev_version in $BIN_DIR; fetching the latest release ..."
    else
        say "Installing wvm for $target into $BIN_DIR"
    fi

    if ! install_from_release "$target"; then
        install_from_source || err "could not install a prebuilt binary or build from source"
    fi

    new_version="$(wvm_version "$BIN_DIR/wvm")"
    say ""
    if [ -z "$prev_version" ]; then
        say "wvm ${new_version:+$new_version }installed to $BIN_DIR/wvm"
    elif [ "$prev_version" = "$new_version" ]; then
        say "wvm $new_version reinstalled (already up to date)"
    else
        say "wvm upgraded $prev_version -> ${new_version:-unknown}"
    fi

    say "Configuring your shell ..."
    configure_shell
    install_completions
    case ":$PATH:" in
        *":$BIN_DIR:"*)
            say "  wvm is already on your PATH"
            ;;
        *)
            say ""
            say "To start using wvm, restart your shell or run:"
            say "    $SOURCE_CMD"
            ;;
    esac
    say ""
    say "Next:"
    say "    wvm install latest    # installs a runtime for your projects"
    say "    wvm default latest    # (or: wvm install lts)"
    say "    wvm exec -- --version"
}

main "$@"
