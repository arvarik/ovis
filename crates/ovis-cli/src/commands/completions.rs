//! `ovis completions <shell>`.
//!
//! Static completions from clap, then a targeted rewrite that makes every
//! connector argument complete *live* connector names instead of filenames.
//!
//! `clap_complete`'s dynamic engine is still unstable, so the connector hook is
//! applied by rewriting the generated script's own value specs. That is a real
//! coupling to clap's output, which is why [`tests`] asserts the rewrite
//! actually matched something — a clap upgrade that changes the shape fails the
//! build rather than silently producing filename completion again.

use clap::CommandFactory;
use clap_complete::Shell;

use crate::cli::Cli;
use crate::ctx::Ctx;
use crate::error::{CliError, CliResult};

/// How long a completion may wait on the server. A completion that hangs is
/// worse than one that returns nothing.
const BUDGET_SECS: &str = "0.3";
/// 332 connectors today; an unbounded candidate list would be unusable anyway.
const MAX_CANDIDATES: usize = 500;

pub fn run(ctx: &Ctx, shell: Shell) -> CliResult<()> {
    let script = generate(shell)?;
    ctx.out.print(script)?;

    ctx.out.note(match shell {
        Shell::Bash => "install: ovis completions bash > /usr/local/etc/bash_completion.d/ovis",
        Shell::Zsh => "install: ovis completions zsh > \"${fpath[1]}/_ovis\"  (then: compinit)",
        Shell::Fish => "install: ovis completions fish > ~/.config/fish/completions/ovis.fish",
        _ => "redirect this into your shell's completion directory",
    });
    if !matches!(shell, Shell::Bash | Shell::Zsh | Shell::Fish) {
        ctx.out
            .warn("live connector-name completion is only wired up for bash, zsh and fish");
    }
    Ok(())
}

pub fn generate(shell: Shell) -> CliResult<String> {
    let mut buffer: Vec<u8> = Vec::new();
    clap_complete::generate(shell, &mut Cli::command(), "ovis", &mut buffer);
    let script = String::from_utf8(buffer)
        .map_err(|e| CliError::Other(anyhow::anyhow!("clap produced invalid UTF-8: {e}")))?;

    let script = hide_helper_command(&script, shell);

    Ok(match shell {
        Shell::Zsh => with_zsh_connectors(&script),
        Shell::Bash => with_bash_connectors(&script),
        Shell::Fish => with_fish_connectors(&script),
        _ => script,
    })
}

/// Drop `__connector-names` from the *candidate* lists.
///
/// `#[command(hide = true)]` keeps it out of `--help`, but `clap_complete` 4.6
/// offers hidden subcommands anyway, so `ovis <TAB>` would suggest an internal
/// helper. Only the lines that offer it are removed; the ones that complete its
/// own flags are harmless and stay.
fn hide_helper_command(script: &str, shell: Shell) -> String {
    match shell {
        // It is the last entry of the top-level `opts` string.
        Shell::Bash => script.replace(" __connector-names\"", "\""),
        Shell::Zsh => strip_lines(script, |line| line.contains("'__connector-names:")),
        Shell::Fish => strip_lines(script, |line| {
            line.contains("__fish_ovis_needs_command") && line.contains("-a \"__connector-names\"")
        }),
        _ => script.to_string(),
    }
}

fn strip_lines(script: &str, drop: impl Fn(&str) -> bool) -> String {
    let mut out: String = script
        .lines()
        .filter(|line| !drop(line))
        .collect::<Vec<_>>()
        .join("\n");
    out.push('\n');
    out
}

/// The value specs clap emits for every argument that names a connector. Each is
/// rewritten to call the live helper instead of `_default` (filenames).
const ZSH_CONNECTOR_SPECS: [&str; 4] = [
    ":ID|NAME:_default",
    ":connector:_default",
    "::connector:_default",
    "*::connectors:_default",
];

fn with_zsh_connectors(script: &str) -> String {
    let mut out = script.to_string();
    // Longest first: "::connector:_default" contains ":connector:_default".
    let mut specs = ZSH_CONNECTOR_SPECS;
    specs.sort_by_key(|s| std::cmp::Reverse(s.len()));
    for spec in specs {
        let replacement = spec.replace("_default", "_ovis_connector_names");
        out = out.replace(spec, &replacement);
    }
    format!(
        r#"{out}
# --- live connector names -------------------------------------------------
# Falls silent when the server is unreachable, so completion never blocks or
# prints an error into the command line.
_ovis_connector_names() {{
  local -a names
  names=(${{(f)"$(ovis __connector-names 2>/dev/null | head -{MAX_CANDIDATES})"}})
  if (( ${{#names}} )); then
    _describe -t connectors 'connector' names
  else
    _default
  fi
}}
zstyle ':completion:*:*:ovis:*' timeout {BUDGET_SECS}
"#
    )
}

/// The case body clap emits for a `--connector`/`-c` value: filename
/// completion, which is never what a connector argument wants.
const BASH_FILE_BODY: &str = "COMPREPLY=($(compgen -f \"${cur}\"))";

fn with_bash_connectors(script: &str) -> String {
    let mut out = script.to_string();
    for flag in ["--connector", "-c"] {
        let from = format!("{flag})\n                    {BASH_FILE_BODY}");
        let to = format!(
            "{flag})\n                    COMPREPLY=($(compgen -W \"$(_ovis_connector_names)\" \
             -- \"${{cur}}\"))"
        );
        out = out.replace(&from, &to);
    }
    format!(
        r#"{out}
# --- live connector names -------------------------------------------------
# Falls silent when the server is unreachable.
_ovis_connector_names() {{
  ovis __connector-names 2>/dev/null | head -{MAX_CANDIDATES}
}}
"#
    )
}

/// Fish completions are additive, so the hook is a plain extra rule rather than
/// a rewrite.
fn with_fish_connectors(script: &str) -> String {
    format!(
        r#"{script}
# --- live connector names -------------------------------------------------
# Falls silent when the server is unreachable.
function __ovis_connector_names
    ovis __connector-names 2>/dev/null | head -{MAX_CANDIDATES}
end
complete -c ovis -s c -l connector -x -a '(__ovis_connector_names)'
complete -c ovis -n '__fish_seen_subcommand_from view docs attempts errors pause resume run prune delete' \
  -a '(__ovis_connector_names)'
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_supported_shell_generates_a_script() {
        for shell in [Shell::Bash, Shell::Zsh, Shell::Fish] {
            let script = generate(shell).unwrap();
            assert!(!script.is_empty(), "{shell:?} produced nothing");
            assert!(script.contains("connector"), "{shell:?}");
            assert!(script.contains("page"), "{shell:?}");
        }
    }

    #[test]
    fn the_hidden_helper_command_is_not_offered_as_a_completion() {
        // It exists for the completion scripts, not for people, and
        // clap_complete does not honour `hide = true` on its own.
        let zsh = generate(Shell::Zsh).unwrap();
        assert!(
            !zsh.contains("'__connector-names:"),
            "zsh offers the helper as a subcommand"
        );

        let bash = generate(Shell::Bash).unwrap();
        assert!(
            !bash.contains(" __connector-names\""),
            "bash lists the helper in its top-level opts"
        );

        let fish = generate(Shell::Fish).unwrap();
        assert!(
            !fish.lines().any(|l| l.contains("__fish_ovis_needs_command")
                && l.contains("-a \"__connector-names\"")),
            "fish offers the helper as a command"
        );

        // …but every script still calls it, which is the whole point.
        for (shell, script) in [
            (Shell::Zsh, &zsh),
            (Shell::Bash, &bash),
            (Shell::Fish, &fish),
        ] {
            assert!(
                script.contains("ovis __connector-names"),
                "{shell:?} lost the live-name hook"
            );
        }
    }

    #[test]
    fn the_real_subcommands_survive_the_hidden_command_strip() {
        let bash = generate(Shell::Bash).unwrap();
        for noun in ["page", "connector", "search", "stats", "status", "tui"] {
            assert!(bash.contains(noun), "bash lost {noun}");
        }
        let zsh = generate(Shell::Zsh).unwrap();
        assert!(zsh.contains("'connector:Inspect and control Onyx connectors'"));
        assert!(zsh.contains("'page:"));
    }

    #[test]
    fn zsh_connector_arguments_complete_names_rather_than_filenames() {
        let script = generate(Shell::Zsh).unwrap();
        // The rewrite must actually have matched: a clap upgrade that changes
        // the value-spec shape has to fail here, not silently regress to
        // filename completion.
        assert!(
            script.contains(":ID|NAME:_ovis_connector_names"),
            "the --connector value spec was not rewritten"
        );
        assert!(
            script.contains(":connector:_ovis_connector_names"),
            "the connector positional was not rewritten"
        );
        assert!(
            !script.contains(":ID|NAME:_default"),
            "some connector argument still completes filenames"
        );
        assert!(script.contains("_ovis_connector_names() {"));
        assert!(script.contains("ovis __connector-names"));
    }

    #[test]
    fn bash_connector_flags_complete_names_rather_than_filenames() {
        let script = generate(Shell::Bash).unwrap();
        assert!(
            script.contains("compgen -W \"$(_ovis_connector_names)\""),
            "the --connector case body was not rewritten"
        );
        assert!(script.contains("_ovis_connector_names() {"));
    }

    #[test]
    fn bash_file_taking_flags_keep_filename_completion() {
        let script = generate(Shell::Bash).unwrap();
        // --from-file and --config genuinely want paths; the rewrite must be
        // scoped to connector arguments only.
        let from_file = script
            .split("--from-file)")
            .nth(1)
            .expect("--from-file is in the tree");
        assert!(
            from_file.contains(BASH_FILE_BODY),
            "--from-file lost filename completion"
        );
    }

    #[test]
    fn every_shell_hook_degrades_silently_and_bounds_its_candidates() {
        for shell in [Shell::Bash, Shell::Zsh, Shell::Fish] {
            let script = generate(shell).unwrap();
            assert!(
                script.contains("2>/dev/null"),
                "{shell:?} would print errors"
            );
            assert!(
                script.contains(&format!("head -{MAX_CANDIDATES}")),
                "{shell:?} does not bound the candidate list"
            );
        }
    }

    #[test]
    fn an_unsupported_shell_still_gets_a_usable_static_script() {
        let script = generate(Shell::PowerShell).unwrap();
        assert!(!script.is_empty());
        assert!(!script.contains("_ovis_connector_names"));
    }
}
