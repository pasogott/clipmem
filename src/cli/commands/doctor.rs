use std::path::Path;

use anyhow::{Context, Result};

use crate::cli::human::render_doctor_human;
use crate::cli::output::render_doctor_text;
use crate::cli::presentation::emit_json_or_text;
use crate::cli::schema::DoctorArgs;

use super::runtime::open_read_only_db;

pub(in crate::cli) fn doctor(db_path: &Path, args: &DoctorArgs) -> Result<()> {
    let db = open_read_only_db(db_path)?;
    let report = if args.verify_invariants {
        db.doctor_verifying_invariants()
    } else {
        db.doctor()
    }
    .context("doctor diagnostics failed")?;
    if args.human {
        display!("{}", render_doctor_human(&report));
    } else {
        emit_json_or_text(args.json, &report, render_doctor_text)?;
    }

    Ok(())
}
