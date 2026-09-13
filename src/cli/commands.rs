// Fallible output propagates a closed consumer to the CLI entrypoint.
macro_rules! display {
    ($($arg:tt)*) => { crate::cli::terminal::write_display(&format!($($arg)*), true)? };
}
macro_rules! displayln {
    ($($arg:tt)*) => { crate::cli::terminal::write_display(&format!("{}\n", format_args!($($arg)*)), true)? };
}

mod agents;
mod app;
mod archive_mutate;
mod doctor;
pub(in crate::cli) mod entry;
mod mutation_support;
pub(in crate::cli) mod notify;
mod ocr;
mod retrieval;
mod retrieval_support;
mod runtime;
mod settings;
mod storage;
