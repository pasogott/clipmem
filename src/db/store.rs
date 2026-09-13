mod capture;
mod config;
pub(in crate::db) mod jobs;
pub(in crate::db) mod ocr;
mod optimize;
mod purge;
mod rebuild;
mod restore;
mod restore_journal;
pub(in crate::db) mod revision;
pub(in crate::db) mod search_document;
mod settings;

#[cfg(test)]
mod tests;
