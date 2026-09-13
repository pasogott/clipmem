use std::ffi::OsString;
use std::process::ExitCode;

use clap::Parser;

use super::errors::{classify_clap_error, classify_clap_exit_code, classify_command_error};
use super::exit::{CliError, CliExitCode};
use super::schema::Cli;
use super::validate::validate_cli;
use super::{commands::entry::run_command, db_path};

/// Parse CLI arguments and execute the requested command.
///
/// # Errors
///
/// Returns an error if command execution fails.
pub fn run() -> std::result::Result<(), CliError> {
    run_from(std::env::args_os())
}

/// Run the CLI entrypoint with an explicit argument vector.
///
/// # Errors
///
/// Returns an error if argument parsing or command execution fails.
pub fn run_from<I, T>(args: I) -> std::result::Result<(), CliError>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let cli = Cli::try_parse_from(args).map_err(classify_clap_error)?;
    validate_cli(&cli).map_err(classify_clap_error)?;
    let db_path = cli.db.unwrap_or_else(db_path::default_db_path);
    match run_command(cli.command, &db_path) {
        Err(error) if super::terminal::is_broken_pipe(&error) => Ok(()),
        result => result.map_err(classify_command_error),
    }
}

pub fn run_cli() -> ExitCode {
    match try_run_cli(std::env::args_os()) {
        Ok(code) => code.as_exit_code(),
        Err(error) => {
            if error.use_stderr() {
                super::terminal::write_error(&error.to_string());
            } else if let Err(write_error) = super::terminal::write_raw(&error.to_string()) {
                return if super::terminal::is_broken_pipe(&write_error) {
                    CliExitCode::Ok
                } else {
                    CliExitCode::Internal
                }
                .as_exit_code();
            }
            classify_clap_exit_code(error.kind()).as_exit_code()
        }
    }
}

fn try_run_cli<I, T>(args: I) -> std::result::Result<CliExitCode, clap::Error>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let cli = Cli::try_parse_from(args)?;
    validate_cli(&cli)?;
    let db_path = cli.db.unwrap_or_else(db_path::default_db_path);
    match run_command(cli.command, &db_path) {
        Ok(()) => Ok(CliExitCode::Ok),
        Err(error) if super::terminal::is_broken_pipe(&error) => Ok(CliExitCode::Ok),
        Err(error) => {
            let classified = classify_command_error(error);
            super::terminal::write_error(&format!("{}\n", classified.message()));
            Ok(classified.exit_code())
        }
    }
}
