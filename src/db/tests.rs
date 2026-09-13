#![allow(deprecated)]

mod support;

pub(in crate::db::tests) use self::support::*;
pub(in crate::db::tests) use super::{sidecar_path, storage_file_sizes, TimelineCursorState};

mod core;
mod filters_and_migrations;
mod fts_lifecycle;
mod image_and_perf;
mod indexes_and_profiles;

mod archive_safety;
