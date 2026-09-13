use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{anyhow, Context, Result};

use crate::db::{PurgeReport, SnapshotDeletionReport};
use crate::platform::restore_items_registered;

use crate::cli::display::format_duration_compact;
use crate::cli::errors::{invalid_args_error, not_found_error};
use crate::cli::formats::OutputFormat;
use crate::cli::human::{
    render_export_human, render_forget_human, render_purge_human, render_restore_human,
};
use crate::cli::output::{ExportOutput, PreviewOutput, RestoreOutput};
use crate::cli::presentation::emit_json_or_text;
use crate::cli::schema::{ExportArgs, ForgetArgs, PreviewArgs, PurgeArgs, RestoreArgs};

use super::mutation_support::require_text_or_json;
use super::notify::notify_app_refresh;
use super::retrieval_support::normalize_retrieval_filters;
use super::runtime::{open_read_only_db, open_read_write_db};

pub(in crate::cli) fn export_snapshot_bytes(db_path: &Path, args: &ExportArgs) -> Result<()> {
    let format = require_text_or_json(args.output.resolved()?, "export")?;
    let filters = normalize_retrieval_filters(&args.filters)?;
    let db = open_read_only_db(db_path)?;
    if !db.snapshot_matches_filters(args.snapshot_id, &filters)? {
        return Err(anyhow!(
            "snapshot {} does not satisfy the active filters",
            args.snapshot_id
        ));
    }
    let representation = db
        .read_representation_payload(args.snapshot_id, args.item, &args.uti)?
        .ok_or_else(|| {
            not_found_error(format!(
                "representation not found for snapshot {} item {} uti {}",
                args.snapshot_id, args.item, args.uti
            ))
        })?;

    if let Some(parent) = args
        .out
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        anyhow::Context::with_context(std::fs::create_dir_all(parent), || {
            format!("failed to create {}", parent.display())
        })?;
    }

    atomic_write_export(&args.out, args.force, representation.bytes())?;

    let output = ExportOutput {
        snapshot_id: args.snapshot_id,
        item_index: args.item,
        uti: args.uti.clone(),
        byte_count: representation.manifest().byte_len(),
        raw_sha256: representation.manifest().raw_sha256().to_string(),
        out: args.out.display().to_string(),
    };
    match format {
        OutputFormat::Json => emit_json_or_text(true, &output, render_export_text)?,
        OutputFormat::Human => display!("{}", render_export_human(&output)),
        OutputFormat::Text => display!("{}", render_export_text(&output)),
        _ => unreachable!("unsupported export format should be rejected earlier"),
    }

    Ok(())
}

pub(in crate::cli) fn preview_snapshot_bytes(db_path: &Path, args: &PreviewArgs) -> Result<()> {
    let format = require_text_or_json(args.output.resolved()?, "preview")?;
    let db = open_read_only_db(db_path)?;
    let preview = db.read_representation_preview(args.snapshot_id, args.item, &args.uti)?;

    let output = if let Some(preview) = preview {
        if let Some(parent) = args
            .out
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        atomic_write_export(&args.out, args.force, preview.bytes())?;
        PreviewOutput {
            snapshot_id: args.snapshot_id,
            item_index: args.item,
            source_uti: args.uti.clone(),
            status: "ready",
            available: true,
            source_raw_sha256: Some(preview.source_raw_sha256().to_string()),
            output_uti: Some(preview.output_uti().to_string()),
            encoder_version: Some(preview.encoder_version()),
            encoder_options_hash: Some(preview.encoder_options_hash().to_string()),
            width: Some(preview.width()),
            height: Some(preview.height()),
            byte_count: Some(preview.bytes().len()),
            out: Some(args.out.display().to_string()),
        }
    } else {
        PreviewOutput {
            snapshot_id: args.snapshot_id,
            item_index: args.item,
            source_uti: args.uti.clone(),
            status: "unavailable",
            available: false,
            source_raw_sha256: None,
            output_uti: None,
            encoder_version: None,
            encoder_options_hash: None,
            width: None,
            height: None,
            byte_count: None,
            out: None,
        }
    };

    emit_json_or_text(
        matches!(format, OutputFormat::Json),
        &output,
        render_preview_text,
    )
}

fn render_preview_text(output: &PreviewOutput) -> String {
    if output.available {
        format!(
            "preview snapshot={} item={} source_uti={} status=ready output_uti={} bytes={} dimensions={}x{} out={}\n",
            output.snapshot_id,
            output.item_index,
            output.source_uti,
            output.output_uti.as_deref().unwrap_or("unknown"),
            output.byte_count.unwrap_or_default(),
            output.width.unwrap_or_default(),
            output.height.unwrap_or_default(),
            output.out.as_deref().unwrap_or("unknown")
        )
    } else {
        format!(
            "preview snapshot={} item={} source_uti={} status=unavailable available=false\n",
            output.snapshot_id, output.item_index, output.source_uti
        )
    }
}

pub(in crate::cli) fn restore_snapshot(db_path: &Path, args: &RestoreArgs) -> Result<()> {
    let format = require_text_or_json(args.output.resolved()?, "restore")?;
    let mut db = open_read_write_db(db_path)?;
    let payload = anyhow::Context::with_context(db.load_restore_payload(args.snapshot_id), || {
        format!("restore failed for snapshot {}", args.snapshot_id)
    })?
    .ok_or_else(|| not_found_error(format!("snapshot {} was not found", args.snapshot_id)))?;
    // Hold SQLite's write ownership across every pasteboard mutation. Watchers
    // may capture concurrently, but their policy transaction cannot observe a
    // restore-created generation until its exact suppression rows are committed.
    let mut suppression =
        db.begin_restore_suppression(args.snapshot_id, payload.snapshot_sha256())?;
    let operation_id = suppression.operation_id().to_string();
    let report = match restore_items_registered(payload.items(), |change_count| {
        suppression.register_generation(change_count)
    }) {
        Ok(report) => report,
        Err(err) => {
            if let Err(mark_err) = suppression.finish_failed(&format!("{err:#}")) {
                eprintln!("restore operation failure recording failed: {mark_err:#}");
            }
            return Err(err).context("restore failed");
        }
    };
    suppression
        .finish_written(report.change_count())
        .context("clipboard restored but suppression registration is incomplete")?;
    let output = RestoreOutput {
        snapshot_id: args.snapshot_id,
        item_count: report.item_count(),
        representation_count: report.representation_count(),
        total_bytes: report.total_bytes(),
        operation_id,
        change_count: report.change_count(),
        rollback_available: report.rollback_available(),
    };
    match format {
        OutputFormat::Json => emit_json_or_text(true, &output, render_restore_text)?,
        OutputFormat::Human => display!("{}", render_restore_human(&output)),
        OutputFormat::Text => display!("{}", render_restore_text(&output)),
        _ => unreachable!("unsupported restore format should be rejected earlier"),
    }
    notify_app_refresh();

    Ok(())
}

pub(in crate::cli) fn forget_snapshot(db_path: &Path, args: &ForgetArgs) -> Result<()> {
    let format = require_text_or_json(args.output.resolved()?, "forget")?;
    let mut db = open_read_write_db(db_path)?;
    let report = anyhow::Context::with_context(db.forget_snapshot(args.snapshot_id), || {
        format!("forget failed for snapshot {}", args.snapshot_id)
    })?
    .ok_or_else(|| not_found_error(format!("snapshot {} was not found", args.snapshot_id)))?;
    match format {
        OutputFormat::Json => emit_json_or_text(true, &report, render_forget_text)?,
        OutputFormat::Human => display!("{}", render_forget_human(&report)),
        OutputFormat::Text => display!("{}", render_forget_text(&report)),
        _ => unreachable!("unsupported forget format should be rejected earlier"),
    }
    notify_app_refresh();

    Ok(())
}

pub(in crate::cli) fn purge_snapshots(db_path: &Path, args: &PurgeArgs) -> Result<()> {
    let format = require_text_or_json(args.output.resolved()?, "purge")?;
    let mut db = open_read_write_db(db_path)?;
    let report = anyhow::Context::with_context(
        db.purge_snapshots_older_than(args.older_than.seconds(), args.dry_run),
        || format!("purge failed for duration {}", args.older_than.raw()),
    )?;
    match format {
        OutputFormat::Json => emit_json_or_text(true, &report, render_purge_text)?,
        OutputFormat::Human => display!("{}", render_purge_human(&report)),
        OutputFormat::Text => display!("{}", render_purge_text(&report)),
        _ => unreachable!("unsupported purge format should be rejected earlier"),
    }
    if !args.dry_run && report.snapshot_count() > 0 {
        notify_app_refresh();
    }

    Ok(())
}

static EXPORT_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn validate_export_destination(path: &Path, force: bool) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            let file_type = metadata.file_type();
            if file_type.is_symlink() {
                return Err(invalid_args_error(format!(
                    "export destination {} is a symbolic link; choose a regular file path instead",
                    path.display()
                )));
            }
            if !metadata.is_file() {
                return Err(invalid_args_error(format!(
                    "export destination {} is not a regular file",
                    path.display()
                )));
            }
            if !force {
                return Err(invalid_args_error(format!(
                    "export destination {} already exists (pass --force to replace it)",
                    path.display()
                )));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| format!("failed to inspect {}", path.display()));
        }
    }

    Ok(())
}

fn atomic_write_export(path: &Path, force: bool, bytes: &[u8]) -> Result<()> {
    validate_export_destination(path, force)?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("export");
    let (temp_path, mut temp_file) = (0..100)
        .find_map(|_| {
            let sequence = EXPORT_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let candidate = parent.join(format!(
                ".{file_name}.clipmem-{}-{sequence}.tmp",
                std::process::id()
            ));
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&candidate) {
                Ok(file) => Some(Ok((candidate, file))),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => None,
                Err(error) => Some(Err(error)),
            }
        })
        .transpose()
        .with_context(|| {
            format!(
                "failed to create temporary export beside {}",
                path.display()
            )
        })?
        .ok_or_else(|| {
            anyhow!(
                "failed to allocate a unique temporary export file beside {}",
                path.display()
            )
        })?;
    let mut cleanup = TempExportGuard {
        path: temp_path,
        armed: true,
    };

    temp_file
        .write_all(bytes)
        .with_context(|| format!("failed to write temporary export for {}", path.display()))?;
    temp_file
        .flush()
        .with_context(|| format!("failed to flush temporary export for {}", path.display()))?;
    temp_file
        .sync_all()
        .with_context(|| format!("failed to sync temporary export for {}", path.display()))?;
    drop(temp_file);
    set_export_permissions(&cleanup.path)?;

    if force {
        std::fs::rename(&cleanup.path, path)
            .with_context(|| format!("failed to atomically replace {}", path.display()))?;
    } else {
        match std::fs::hard_link(&cleanup.path, path) {
            Ok(()) => std::fs::remove_file(&cleanup.path).with_context(|| {
                format!("failed to remove temporary export for {}", path.display())
            })?,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(invalid_args_error(format!(
                    "export destination {} already exists (pass --force to replace it)",
                    path.display()
                )));
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed to publish export {}", path.display()));
            }
        }
    }
    cleanup.armed = false;
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .with_context(|| format!("failed to sync export directory {}", parent.display()))?;
    Ok(())
}

struct TempExportGuard {
    path: std::path::PathBuf,
    armed: bool,
}

impl Drop for TempExportGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[cfg(unix)]
fn set_export_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("failed to set export permissions on {}", path.display()))
}

#[cfg(not(unix))]
fn set_export_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

fn render_restore_text(output: &RestoreOutput) -> String {
    format!(
        "restored snapshot={} items={} representations={} bytes={}\n",
        output.snapshot_id, output.item_count, output.representation_count, output.total_bytes
    )
}

fn render_export_text(output: &ExportOutput) -> String {
    format!(
        "exported snapshot={} item={} uti={} bytes={} sha256={} out={}\n",
        output.snapshot_id,
        output.item_index,
        output.uti,
        output.byte_count,
        output.raw_sha256,
        output.out
    )
}

fn render_forget_text(report: &SnapshotDeletionReport) -> String {
    format!(
        "forgot snapshot={} items={} representations={} capture_events={} bytes={}\n",
        report.snapshot_id(),
        report.item_count(),
        report.representation_count(),
        report.capture_event_count(),
        report.total_bytes()
    )
}

fn render_purge_text(report: &PurgeReport) -> String {
    let action = if report.dry_run() {
        "purge dry-run"
    } else {
        "purged"
    };
    format!(
        "{} older_than={} snapshots={} items={} representations={} capture_events={} bytes={}\n",
        action,
        format_duration_compact(report.older_than_seconds()),
        report.snapshot_count(),
        report.item_count(),
        report.representation_count(),
        report.capture_event_count(),
        report.total_bytes()
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Barrier};
    use std::thread;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::atomic_write_export;

    static TEMP_PATH_SEQUENCE: AtomicU64 = AtomicU64::new(1);

    #[test]
    fn create_export_destination_rejects_existing_file_without_force() {
        let path = temp_path("existing-export.bin");
        std::fs::write(&path, b"old").unwrap();

        let error = atomic_write_export(&path, false, b"new").unwrap_err();

        assert!(error.to_string().contains("already exists"));
        assert_eq!(std::fs::read(&path).unwrap(), b"old");

        cleanup_path(&path);
    }

    #[test]
    fn create_export_destination_replaces_existing_file_with_force() {
        let path = temp_path("forced-export.bin");
        std::fs::write(&path, b"old").unwrap();

        atomic_write_export(&path, true, b"new").unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"new");

        cleanup_path(&path);
    }

    #[test]
    fn create_export_destination_rejects_directory() {
        let path = temp_path("export-directory");
        std::fs::create_dir_all(&path).unwrap();

        let error = atomic_write_export(&path, true, b"new").unwrap_err();

        assert!(error.to_string().contains("not a regular file"));

        cleanup_path(&path);
    }

    #[cfg(unix)]
    #[test]
    fn create_export_destination_rejects_symlink() {
        let target = temp_path("export-target.bin");
        let link = temp_path("export-link.bin");
        std::fs::write(&target, b"target").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let error = atomic_write_export(&link, true, b"new").unwrap_err();

        assert!(error.to_string().contains("symbolic link"));
        assert_eq!(std::fs::read(&target).unwrap(), b"target");

        cleanup_path(&link);
        cleanup_path(&target);
    }

    #[cfg(unix)]
    #[test]
    fn atomic_export_write_failure_preserves_existing_target() {
        use std::os::unix::fs::PermissionsExt as _;

        let path = temp_path("failed-force-export.bin");
        std::fs::write(&path, b"old").unwrap();
        let parent = path.parent().unwrap();
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o500)).unwrap();
        let result = atomic_write_export(&path, true, b"new");
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700)).unwrap();

        assert!(result.is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"old");
        cleanup_path(&path);
    }

    #[test]
    fn non_force_export_race_publishes_exactly_one_complete_file() {
        let path = temp_path("racing-export.bin");
        let barrier = Arc::new(Barrier::new(3));
        let handles = [b"first".as_slice(), b"second".as_slice()].map(|bytes| {
            let path = path.clone();
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                atomic_write_export(&path, false, bytes)
            })
        });
        barrier.wait();
        let results = handles.map(|handle| handle.join().unwrap());
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes == b"first" || bytes == b"second");
        cleanup_path(&path);
    }

    fn temp_path(name: &str) -> PathBuf {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after unix epoch")
            .as_nanos();

        let dir = std::env::temp_dir()
            .join("clipmem-archive-mutate-tests")
            .join(format!(
                "{}-{timestamp}-{}",
                std::process::id(),
                TEMP_PATH_SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    fn cleanup_path(path: &std::path::Path) {
        if let Ok(metadata) = std::fs::symlink_metadata(path) {
            if metadata.is_dir() && !metadata.file_type().is_symlink() {
                let _ = std::fs::remove_dir_all(path);
            } else {
                let _ = std::fs::remove_file(path);
            }
        }
        if let Some(parent) = path.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }
}
