//! `ovis` entry point.
//!
//! The only job here is: parse, dispatch, and turn a failure into the right
//! exit code with a message on **stderr**. Every command path used to end in
//! `Ok(())` and exit 0 regardless of what happened.

use clap::Parser;
use ovis_cli::cli::Cli;
use ovis_cli::error::exit;
use ovis_cli::output::style::Tone;

fn main() {
    // clap's own errors (`--help`, `--version`, a bad flag) are already
    // formatted and carry their own exit code; usage failures must be 2.
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => {
            let _ = err.print();
            std::process::exit(match err.kind() {
                clap::error::ErrorKind::DisplayHelp
                | clap::error::ErrorKind::DisplayVersion
                | clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => exit::OK,
                _ => exit::USAGE,
            });
        }
    };

    init_logging(cli.globals.verbose);

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("error: cannot start the async runtime: {err}");
            std::process::exit(exit::GENERIC);
        }
    };

    let code = match runtime.block_on(ovis_cli::commands::dispatch(&cli)) {
        Ok(()) => exit::OK,
        Err(err) => {
            report(&err, &cli);
            err.exit_code()
        }
    };
    std::process::exit(code);
}

fn report(err: &ovis_cli::CliError, cli: &Cli) {
    // Colour on the error path follows the same policy as everything else, and
    // an explicit --color never is honoured even for failures.
    let colored = match cli.globals.color {
        Some(ovis_cli::output::ColorChoice::Never) => false,
        Some(ovis_cli::output::ColorChoice::Always) => true,
        _ => {
            std::io::IsTerminal::is_terminal(&std::io::stderr())
                && !std::env::var("NO_COLOR").is_ok_and(|v| !v.is_empty())
        }
    };

    eprintln!("{} {}", Tone::Error.paint("error:", colored), err.message());
    if let Some(hint) = err.hint() {
        eprintln!("{} {hint}", Tone::Info.paint("hint:", colored));
    }
}

/// `-v` turns on our own tracing; `-vv` turns on everything's. Logs go to
/// stderr so they never contaminate piped data.
fn init_logging(verbosity: u8) {
    if verbosity == 0 {
        return;
    }
    let filter = match verbosity {
        1 => "ovis_cli=debug,ovis_core=debug",
        2 => "debug",
        _ => "trace",
    };
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(filter)),
        )
        .with_writer(std::io::stderr)
        .try_init();
}
