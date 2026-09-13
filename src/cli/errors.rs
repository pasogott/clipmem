use clap::error::ErrorKind;

use super::exit::{CliError, CliExitCode};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CliValueError {
    kind: ErrorKind,
    message: String,
}

impl CliValueError {
    pub(super) fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for CliValueError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CliValueError {}

#[derive(Debug)]
pub(in crate::cli) struct UnsupportedFormatError {
    message: String,
}

impl UnsupportedFormatError {
    pub(in crate::cli) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for UnsupportedFormatError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for UnsupportedFormatError {}

#[derive(Debug)]
pub(in crate::cli) struct CliCommandError {
    exit_code: CliExitCode,
    message: String,
}

impl CliCommandError {
    fn new(exit_code: CliExitCode, message: impl Into<String>) -> Self {
        Self {
            exit_code,
            message: message.into(),
        }
    }

    const fn exit_code(&self) -> CliExitCode {
        self.exit_code
    }

    fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for CliCommandError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CliCommandError {}

pub(in crate::cli) fn invalid_args_error(message: impl Into<String>) -> anyhow::Error {
    CliCommandError::new(CliExitCode::InvalidArgs, message).into()
}

pub(in crate::cli) fn not_found_error(message: impl Into<String>) -> anyhow::Error {
    CliCommandError::new(CliExitCode::NotFound, message).into()
}

pub(in crate::cli) fn db_error(message: impl Into<String>) -> anyhow::Error {
    CliCommandError::new(CliExitCode::DbError, message).into()
}

pub(in crate::cli) fn platform_error(message: impl Into<String>) -> anyhow::Error {
    CliCommandError::new(CliExitCode::PlatformError, message).into()
}

pub(super) fn value_error(kind: ErrorKind, message: impl Into<String>) -> CliValueError {
    CliValueError::new(kind, message)
}

pub(super) fn clap_error_from_value(error: CliValueError) -> clap::Error {
    clap_error(error.kind, error.message)
}

pub(super) fn clap_error(kind: ErrorKind, message: impl Into<String>) -> clap::Error {
    clap::Command::new("clipmem").error(kind, message.into())
}

pub(super) fn classify_clap_error(error: clap::Error) -> CliError {
    CliError::new(classify_clap_exit_code(error.kind()), error.to_string())
}

pub(super) fn classify_clap_exit_code(kind: ErrorKind) -> CliExitCode {
    match kind {
        ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => CliExitCode::Ok,
        _ => CliExitCode::InvalidArgs,
    }
}

pub(super) fn classify_command_error(error: anyhow::Error) -> CliError {
    if let Some(error) = error.downcast_ref::<CliCommandError>() {
        return CliError::new(error.exit_code(), error.message().to_string());
    }

    let message = sanitize_error_message(&error);
    let exit_code = if is_unsupported_format_error(&error, &message) {
        CliExitCode::UnsupportedFormat
    } else if is_not_found_error(&error, &message) {
        CliExitCode::NotFound
    } else if is_platform_error(&error, &message) {
        CliExitCode::PlatformError
    } else if is_invalid_argument_error(&error, &message) {
        CliExitCode::InvalidArgs
    } else if is_db_error(&error, &message) {
        CliExitCode::DbError
    } else {
        CliExitCode::Internal
    };

    CliError::new(exit_code, message)
}

pub(super) fn sanitize_error_message(error: &anyhow::Error) -> String {
    let chain_messages = error
        .chain()
        .map(std::string::ToString::to_string)
        .collect::<Vec<_>>();
    let message = chain_messages
        .first()
        .cloned()
        .unwrap_or_else(|| "command failed".to_string());
    let lower_chain = chain_messages.join(" | ").to_ascii_lowercase();

    if lower_chain.contains("prerelease schema") {
        "database operation failed; this may be an incompatible prerelease schema. Move the database aside and run `clipmem setup`.".to_string()
    } else if lower_chain.contains("sqlite") {
        "database operation failed; run `clipmem doctor`, and if this is an older prerelease archive, move it aside and run `clipmem setup`.".to_string()
    } else {
        message
    }
}

pub(super) fn is_invalid_argument_error(error: &anyhow::Error, message: &str) -> bool {
    error.downcast_ref::<clap::Error>().is_some()
        || error.downcast_ref::<CliValueError>().is_some()
        || message.contains("cursor does not match")
        || message.contains("cursor is for command")
        || message.contains("cursor mode")
        || message.contains("invalid cursor")
        || message.contains("already exists (pass --force to replace it)")
        || message.contains("symbolic link")
        || message.contains("not a regular file")
}

pub(super) fn is_not_found_error(error: &anyhow::Error, message: &str) -> bool {
    if error
        .chain()
        .any(|cause| cause.downcast_ref::<rusqlite::Error>().is_some())
    {
        return false;
    }

    message.contains(" was not found")
        || message.contains("representation not found")
        || message.starts_with("get failed for snapshot ")
        || message.contains("Missing /")
        || message.starts_with("Missing ")
}

pub(super) fn is_unsupported_format_error(_error: &anyhow::Error, message: &str) -> bool {
    _error.downcast_ref::<UnsupportedFormatError>().is_some()
        || message.contains("unsupported format")
}

pub(super) fn is_platform_error(error: &anyhow::Error, message: &str) -> bool {
    message.contains("clipboard capture is only supported on macOS")
        || message.contains("clipboard restore is only supported on macOS")
        || message.contains("setup and service commands are only supported on macOS")
        || message.contains("capture failed")
        || message.contains("capture-once clipboard read failed")
        || message.contains("restore failed")
        || message.contains("read clipboard change count failed")
        || error.chain().any(|cause| {
            let cause = cause.to_string();
            cause.contains("clipboard capture is only supported on macOS")
                || cause.contains("clipboard restore is only supported on macOS")
        })
}

pub(super) fn is_db_error(error: &anyhow::Error, message: &str) -> bool {
    error.downcast_ref::<rusqlite::Error>().is_some()
        || error
            .chain()
            .any(|cause| cause.downcast_ref::<rusqlite::Error>().is_some())
        || message.contains("database")
        || message.contains("SQLite")
        || message.contains("failed to open database")
        || message.contains("failed to write")
        || message.contains("query failed")
}

#[cfg(test)]
mod tests {
    use anyhow::anyhow;
    use clap::error::ErrorKind;

    use super::{
        classify_clap_exit_code, classify_command_error, db_error, invalid_args_error, is_db_error,
        is_not_found_error, is_platform_error, sanitize_error_message, CliCommandError,
        CliExitCode, UnsupportedFormatError,
    };

    #[test]
    fn classify_command_error_preserves_explicit_cli_command_error_codes() {
        let error = CliCommandError::new(CliExitCode::NotFound, "snapshot 42 was not found");

        let classified = classify_command_error(error.into());

        assert_eq!(classified.exit_code(), CliExitCode::NotFound);
        assert_eq!(classified.message(), "snapshot 42 was not found");
    }

    #[test]
    fn classify_clap_exit_code_treats_help_and_version_as_ok() {
        assert_eq!(
            classify_clap_exit_code(ErrorKind::DisplayHelp),
            CliExitCode::Ok
        );
        assert_eq!(
            classify_clap_exit_code(ErrorKind::DisplayVersion),
            CliExitCode::Ok
        );
        assert_eq!(
            classify_clap_exit_code(ErrorKind::UnknownArgument),
            CliExitCode::InvalidArgs
        );
    }

    #[test]
    fn sanitize_error_message_rewrites_sqlite_schema_failures() {
        let error = anyhow!("no such column: capture_events.legacy_column")
            .context("query failed while reading prerelease schema");

        let message = sanitize_error_message(&error);

        assert!(message.contains("incompatible prerelease schema"));
        assert!(message.contains("clipmem setup"));
    }

    #[test]
    fn sanitize_error_message_keeps_non_database_messages() {
        let error = invalid_args_error("cursor does not match the active search filters");

        assert_eq!(
            sanitize_error_message(&error),
            "cursor does not match the active search filters"
        );
    }

    #[test]
    fn rusqlite_errors_are_db_errors_not_not_found_errors() {
        let error: anyhow::Error = rusqlite::Error::QueryReturnedNoRows.into();
        let message = "snapshot was not found";

        assert!(is_db_error(&error, message));
        assert!(!is_not_found_error(&error, message));
        assert_eq!(
            classify_command_error(error).exit_code(),
            CliExitCode::DbError
        );
    }

    #[test]
    fn platform_error_detection_checks_error_chain() {
        let error = anyhow!("clipboard restore is only supported on macOS")
            .context("restore failed for snapshot 42");
        let message = sanitize_error_message(&error);

        assert!(is_platform_error(&error, &message));
        assert_eq!(
            classify_command_error(error).exit_code(),
            CliExitCode::PlatformError
        );
    }

    #[test]
    fn unsupported_format_error_is_classified_before_invalid_args() {
        let error = UnsupportedFormatError::new(
            "export only supports `text`, `json`, and `human` output, got `jsonl`",
        );

        let classified = classify_command_error(error.into());

        assert_eq!(classified.exit_code(), CliExitCode::UnsupportedFormat);
    }

    #[test]
    fn explicit_error_helpers_set_expected_exit_codes() {
        assert_eq!(
            classify_command_error(invalid_args_error("bad flag")).exit_code(),
            CliExitCode::InvalidArgs
        );
        assert_eq!(
            classify_command_error(db_error("database write failed")).exit_code(),
            CliExitCode::DbError
        );
    }
}
