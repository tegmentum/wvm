//! Static shell-completion script generation for `wvm`.
//!
//! Handled natively (before any runtime bootstrap) so `install.sh` can generate
//! and install completions immediately after dropping in the binary. The
//! command list mirrors the app's dispatch in `wvm-app/src/lib.rs` — keep them
//! in sync (the internal `ensure` command is intentionally omitted).

use anyhow::{bail, Result};

/// Subcommands offered for completion, with a one-line description each.
const COMMANDS: &[(&str, &str)] = &[
    ("install", "Install a runtime"),
    ("list", "List available versions"),
    ("current", "Show the effective runtime version"),
    ("path", "Print a runtime's filesystem path"),
    ("default", "Set the persistent default"),
    ("use", "Switch the runtime for this shell"),
    ("upgrade", "Pull the newest match for a floating line"),
    ("deactivate", "Clear the per-shell override"),
    ("shell-init", "Print the shell hook for `use`"),
    ("register", "Record an app's runtime dependency"),
    ("unregister", "Drop an application registration"),
    ("apps", "List registered applications"),
    ("usage", "Show runtime invocations"),
    ("uninstall", "Remove an installed runtime"),
    ("verify", "Validate installation integrity"),
    ("gc", "Reclaim unreferenced store objects"),
    ("objects", "List stored objects"),
    ("completions", "Print a shell completion script"),
    ("help", "Show help"),
];

/// Commands that select an already-known runtime: offer the floating aliases
/// (`latest`/`lts`) *and* the concrete installed versions.
const SELECT_CMDS: &[&str] = &["use", "default", "path", "upgrade"];
/// Commands that install a (possibly not-yet-present) version: floating aliases
/// only — you install versions you don't have.
const INSTALL_CMDS: &[&str] = &["install"];
/// Commands that remove an installed runtime: only installed versions.
const REMOVE_CMDS: &[&str] = &["uninstall"];

/// Shell snippet that lists installed versions at completion time. Runs the
/// native, offline `--installed` helper below; `2>/dev/null` swallows the error
/// from an older binary that predates it (yielding an empty list).
const INSTALLED_CMD: &str = "wvm completions --installed 2>/dev/null";

/// `wvm completions <shell>` — print a completion script to stdout.
pub fn print(shell: Option<&str>) -> Result<()> {
    match shell {
        Some("bash") => print!("{}", bash()),
        Some("zsh") => print!("{}", zsh()),
        Some("fish") => print!("{}", fish()),
        Some(other) => bail!("unsupported shell `{other}` (expected: bash, zsh, or fish)"),
        None => bail!("usage: wvm completions <bash|zsh|fish>"),
    }
    Ok(())
}

/// `wvm completions --installed` — print installed runtime versions, one per
/// line. Native and offline (reads the versions directory, no runtime
/// bootstrap), so it is cheap enough to call on every `<Tab>`.
pub fn installed() -> Result<()> {
    let layout = wvm_core::layout::Layout::discover()?;
    for v in wvm_core::discovery::installed_versions(&layout).unwrap_or_default() {
        println!("{v}");
    }
    Ok(())
}

fn command_names() -> String {
    COMMANDS
        .iter()
        .map(|(c, _)| *c)
        .collect::<Vec<_>>()
        .join(" ")
}

/// `cmd1|cmd2|…` for a shell `case` alternation.
fn alt(cmds: &[&str]) -> String {
    cmds.join("|")
}

fn bash() -> String {
    format!(
        r#"# bash completion for wvm
_wvm() {{
    local cur prev
    cur="${{COMP_WORDS[COMP_CWORD]}}"
    prev="${{COMP_WORDS[COMP_CWORD-1]}}"
    if [ "$COMP_CWORD" -eq 1 ]; then
        COMPREPLY=( $(compgen -W "{cmds} --version --upgrade --help" -- "$cur") )
        return
    fi
    case "$prev" in
        {select}) COMPREPLY=( $(compgen -W "latest lts $({installed})" -- "$cur") ); return ;;
        {install}) COMPREPLY=( $(compgen -W "latest lts" -- "$cur") ); return ;;
        {remove}) COMPREPLY=( $(compgen -W "$({installed})" -- "$cur") ); return ;;
        completions) COMPREPLY=( $(compgen -W "bash zsh fish" -- "$cur") ); return ;;
    esac
}}
complete -F _wvm wvm
"#,
        cmds = command_names(),
        select = alt(SELECT_CMDS),
        install = alt(INSTALL_CMDS),
        remove = alt(REMOVE_CMDS),
        installed = INSTALLED_CMD,
    )
}

fn zsh() -> String {
    format!(
        r#"#compdef wvm
# zsh completion for wvm
(( $+functions[compdef] )) || {{ autoload -Uz compinit && compinit -C }}
_wvm() {{
    local -a cmds
    cmds=({cmds})
    if (( CURRENT == 2 )); then
        _describe -t commands 'wvm command' cmds
        return
    fi
    case "${{words[2]}}" in
        {select}) compadd latest lts ${{(f)"$({installed})"}} ;;
        {install}) compadd latest lts ;;
        {remove}) compadd ${{(f)"$({installed})"}} ;;
        completions) compadd bash zsh fish ;;
    esac
}}
compdef _wvm wvm
"#,
        cmds = command_names(),
        select = alt(SELECT_CMDS),
        install = alt(INSTALL_CMDS),
        remove = alt(REMOVE_CMDS),
        installed = INSTALLED_CMD,
    )
}

fn fish() -> String {
    let mut out = String::from("# fish completion for wvm\ncomplete -c wvm -f\n");
    for (cmd, desc) in COMMANDS {
        out.push_str(&format!(
            "complete -c wvm -n __fish_use_subcommand -a {cmd} -d '{desc}'\n"
        ));
    }
    // Floating aliases for the select + install commands.
    out.push_str(&format!(
        "complete -c wvm -n '__fish_seen_subcommand_from {}' -a 'latest lts'\n",
        [SELECT_CMDS, INSTALL_CMDS].concat().join(" ")
    ));
    // Installed versions for the select + remove commands.
    out.push_str(&format!(
        "complete -c wvm -n '__fish_seen_subcommand_from {}' -a '({INSTALLED_CMD})'\n",
        [SELECT_CMDS, REMOVE_CMDS].concat().join(" ")
    ));
    out.push_str(
        "complete -c wvm -n '__fish_seen_subcommand_from completions' -a 'bash zsh fish'\n",
    );
    out
}
