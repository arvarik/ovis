//! Interactive confirmations.
//!
//! Reads from `/dev/tty` rather than stdin, because `ovis page delete -` takes
//! its ids *on* stdin: prompting there would eat the input. When there is no
//! terminal to ask — or `--no-input` was passed — the command fails with exit 10
//! instead of assuming yes.

use std::io::{BufRead, BufReader, IsTerminal, Write};

use crate::error::{CliError, CliResult};

/// How the process was told to behave about prompting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Interaction {
    /// `-y`: pre-answered yes.
    pub assume_yes: bool,
    /// `--no-input`: never prompt; an unconfirmed destructive op is exit 10.
    pub no_input: bool,
}

impl Interaction {
    pub fn new(assume_yes: bool, no_input: bool) -> Self {
        Self {
            assume_yes,
            no_input,
        }
    }
}

impl Default for Interaction {
    fn default() -> Self {
        Self {
            assume_yes: false,
            no_input: true,
        }
    }
}

/// A terminal we can both prompt on and read from, independent of stdin/stdout.
struct Tty {
    reader: BufReader<std::fs::File>,
    writer: std::fs::File,
}

fn open_tty() -> Option<Tty> {
    let reader = std::fs::File::open("/dev/tty").ok()?;
    let writer = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/tty")
        .ok()?;
    Some(Tty {
        reader: BufReader::new(reader),
        writer,
    })
}

/// True when a prompt could actually be answered.
pub fn can_prompt() -> bool {
    open_tty().is_some() || std::io::stdin().is_terminal()
}

fn ask(question: &str) -> CliResult<String> {
    if let Some(mut tty) = open_tty() {
        write!(tty.writer, "{question}")?;
        tty.writer.flush()?;
        let mut line = String::new();
        tty.reader.read_line(&mut line)?;
        return Ok(line.trim().to_string());
    }

    // No /dev/tty (a container without one, say). Falling back to stdin is only
    // safe when stdin is itself a terminal — otherwise we would consume piped
    // data meant for the command.
    if std::io::stdin().is_terminal() {
        eprint!("{question}");
        std::io::stderr().flush()?;
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        return Ok(line.trim().to_string());
    }

    Err(CliError::NeedsConfirmation(
        "confirmation is required but there is no terminal to ask on".into(),
    ))
}

/// Yes/no, defaulting to **no**. Destructive operations never default to yes.
pub fn confirm(question: &str, interaction: Interaction) -> CliResult<bool> {
    if interaction.assume_yes {
        return Ok(true);
    }
    if interaction.no_input {
        return Err(CliError::NeedsConfirmation(format!(
            "{question} — refused because --no-input is set"
        )));
    }
    if !can_prompt() {
        return Err(CliError::NeedsConfirmation(format!(
            "{question} — refused because there is no terminal to prompt on"
        )));
    }
    let answer = ask(&format!("{question} [y/N]: "))?;
    Ok(matches!(answer.to_ascii_lowercase().as_str(), "y" | "yes"))
}

/// Ask a question and return the answer, with a default for an empty reply.
pub fn ask_line(question: &str, default: Option<&str>) -> CliResult<String> {
    let rendered = match default {
        Some(d) if !d.is_empty() => format!("{question} [{d}]: "),
        _ => format!("{question}: "),
    };
    let answer = ask(&rendered)?;
    if answer.is_empty() {
        return Ok(default.unwrap_or_default().to_string());
    }
    Ok(answer)
}

/// Read a secret without echoing it.
///
/// Raw mode rather than a `read_line`, so the password never appears on screen,
/// in scrollback, or in a shell history. It is also never passed as an argument
/// to anything — `ps` would show that.
pub fn read_password(question: &str) -> CliResult<String> {
    let Some(mut tty) = open_tty() else {
        return Err(CliError::NeedsConfirmation(
            "a password is needed but there is no terminal to read it from".into(),
        ));
    };

    write!(tty.writer, "{question}")?;
    tty.writer.flush()?;

    crossterm::terminal::enable_raw_mode()
        .map_err(|e| CliError::Other(anyhow::anyhow!("cannot disable echo: {e}")))?;
    let result = read_password_raw(&mut tty);
    let _ = crossterm::terminal::disable_raw_mode();
    let _ = writeln!(tty.writer);

    result
}

fn read_password_raw(tty: &mut Tty) -> CliResult<String> {
    use std::io::Read;
    let mut password = String::new();
    let mut byte = [0u8; 1];
    loop {
        if tty.reader.read(&mut byte)? == 0 {
            break;
        }
        match byte[0] {
            b'\r' | b'\n' => break,
            // Ctrl+C in raw mode arrives as a byte rather than a signal.
            0x03 => {
                return Err(CliError::NeedsConfirmation(
                    "cancelled at the password prompt".into(),
                ))
            }
            0x7f | 0x08 => {
                password.pop();
            }
            b => password.push(b as char),
        }
    }
    Ok(password)
}

/// Ask the user to type an exact string back. Used for `connector delete`, where
/// `-y` deliberately does *not* skip the echo: the operation can destroy a
/// hundred thousand documents.
pub fn confirm_exact(question: &str, expected: &str, interaction: Interaction) -> CliResult<()> {
    if interaction.no_input {
        return Err(CliError::NeedsConfirmation(format!(
            "{question} — refused because --no-input is set. Pass --confirm-name '{expected}' \
             to supply it non-interactively"
        )));
    }
    if !can_prompt() {
        return Err(CliError::NeedsConfirmation(format!(
            "{question} — there is no terminal to prompt on. Pass --confirm-name '{expected}'"
        )));
    }
    let answer = ask(&format!("{question}\nType the name to confirm: "))?;
    if answer == expected {
        Ok(())
    } else {
        Err(CliError::NeedsConfirmation(format!(
            "'{answer}' does not match '{expected}'; nothing was changed"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assume_yes_short_circuits_before_any_terminal_is_needed() {
        let answer = confirm("delete?", Interaction::new(true, false)).unwrap();
        assert!(answer);
        // Even together with --no-input: -y is an answer, not a request to ask.
        assert!(confirm("delete?", Interaction::new(true, true)).unwrap());
    }

    #[test]
    fn no_input_without_yes_is_exit_10_rather_than_a_silent_yes() {
        let err = confirm("delete?", Interaction::new(false, true)).unwrap_err();
        assert_eq!(err.exit_code(), crate::error::exit::NEEDS_CONFIRMATION);
        assert!(err.message().contains("--no-input"));
    }

    #[test]
    fn the_name_echo_is_never_skipped_by_yes() {
        // -y set, and it still refuses: this guard exists because the operation
        // can destroy a 100k-document connector.
        let err = confirm_exact("really?", "tildes", Interaction::new(true, true)).unwrap_err();
        assert_eq!(err.exit_code(), crate::error::exit::NEEDS_CONFIRMATION);
        assert!(err.message().contains("--confirm-name"));
    }

    #[test]
    fn the_default_interaction_never_prompts() {
        // Anything constructing an Interaction by mistake gets the safe end of
        // the trade: refuse rather than proceed unconfirmed.
        let default = Interaction::default();
        assert!(!default.assume_yes);
        assert!(default.no_input);
    }
}
