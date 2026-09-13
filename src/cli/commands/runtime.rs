use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::app::{
    format_watch_capture_line, mark_change_handled, should_skip_initial_change, WatchState,
};
use crate::capture_service::CaptureApplicationService;
use crate::db::{CaptureMode, CaptureOutcome, CaptureSkipReason, Database};
use crate::model::ClipboardSnapshot;
use crate::platform::{capture_snapshot, current_change_count};

use crate::cli::commands::notify::notify_app_refresh;
use crate::cli::errors::{db_error, platform_error};
use crate::cli::human::render_capture_once_human;
use crate::cli::output::{
    render_capture_once_text, CaptureOnceOutput, CaptureOnceSkippedOutput, CaptureOnceStoredOutput,
};
use crate::cli::presentation::emit_json_or_text;
use crate::cli::schema::{CaptureOnceArgs, WatchArgs};

static OCR_WORKERS: LazyLock<Mutex<HashSet<PathBuf>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

fn capture_skip_reason_label(reason: CaptureSkipReason) -> &'static str {
    reason.as_str()
}

pub(in crate::cli) fn watch(db_path: &Path, args: &WatchArgs) -> Result<()> {
    let mut db = open_or_init_db(db_path)?;
    let interval_ms = args.interval_ms.max(50);
    let mut state = WatchState::new();
    let _ocr_worker = start_ocr_worker(db.path().to_path_buf())?;
    db.apply_retention_policy()
        .context("apply retention at watcher startup")?;
    let mut retention_checked_at = Instant::now();

    loop {
        if retention_checked_at.elapsed() >= Duration::from_secs(30) {
            retention_checked_at = Instant::now();
            match db.apply_retention_policy() {
                Ok(Some(report)) if report.snapshot_count() > 0 => notify_app_refresh(),
                Ok(_) => {}
                Err(error) => eprintln!("retention maintenance failed: {error:#}"),
            }
        }
        if let Err(err) = run_watch_iteration(&mut db, args, &mut state) {
            if crate::cli::terminal::is_broken_pipe(&err) {
                return Ok(());
            }
            crate::cli::terminal::write_error(&format!("{err:#}\n"));
            thread::sleep(Duration::from_secs(1));
        }

        thread::sleep(Duration::from_millis(interval_ms));
    }
}

pub(in crate::cli) fn run_watch_iteration(
    db: &mut Database,
    args: &WatchArgs,
    state: &mut WatchState,
) -> Result<()> {
    run_watch_iteration_with_capture(db, args, state, current_change_count, capture_snapshot)
}

pub(in crate::cli) fn run_watch_iteration_with_capture<CountFn, CaptureFn>(
    db: &mut Database,
    args: &WatchArgs,
    state: &mut WatchState,
    current_change_count_fn: CountFn,
    capture_snapshot_fn: CaptureFn,
) -> Result<()>
where
    CountFn: FnOnce() -> Result<i64>,
    CaptureFn: FnOnce() -> Result<ClipboardSnapshot>,
{
    // Pausing must stop content reads as well as database writes.
    if db.capture_settings()?.paused() {
        return Ok(());
    }
    let change_count = current_change_count_fn()
        .map_err(|error| platform_error(format!("read clipboard change count failed: {error}")))?;

    if should_skip_initial_change(args.skip_initial, state) {
        mark_change_handled(change_count, state);
        return Ok(());
    }

    if !crate::app::should_capture_change(change_count, state) {
        return Ok(());
    }

    let snapshot = capture_snapshot_fn()
        .map_err(|error| platform_error(format!("capture failed: {error}")))?;
    if !crate::app::should_capture_change(snapshot.change_count(), state) {
        return Ok(());
    }

    if snapshot.items().is_empty() {
        // Lazy providers can populate this same generation on a later poll.
        return Ok(());
    }
    let outcome = CaptureApplicationService::new(db).capture(&snapshot, CaptureMode::Watch)?;
    match outcome {
        CaptureOutcome::Stored {
            store: result,
            enqueue_ocr,
        }
        | CaptureOutcome::ObservedExisting {
            store: result,
            enqueue_ocr,
        } => {
            let _ = enqueue_ocr; // The watcher-owned worker drains durable capture jobs.
            mark_change_handled(snapshot.change_count(), state);
            db.apply_retention_policy()
                .context("apply retention policy failed")?;
            notify_app_refresh();
            if !args.quiet {
                displayln!("{}", format_watch_capture_line(&snapshot, &result));
            }
        }
        other => {
            mark_change_handled(snapshot.change_count(), state);
            let reason = capture_outcome_skip_reason(&other);
            if !args.quiet {
                displayln!(
                    "skipped capture reason={} kind={} bytes={} source={}",
                    capture_skip_reason_label(reason),
                    snapshot.snapshot_kind(),
                    snapshot.total_bytes(),
                    snapshot
                        .frontmost_app_name()
                        .or(snapshot.frontmost_app_bundle_id())
                        .unwrap_or("unknown app")
                );
            }
        }
    }

    Ok(())
}

fn capture_outcome_skip_reason(outcome: &CaptureOutcome) -> CaptureSkipReason {
    match outcome {
        CaptureOutcome::SuppressedRestore { operation_id } => {
            let _ = operation_id;
            CaptureSkipReason::RestoredSnapshot
        }
        CaptureOutcome::SkippedPaused => CaptureSkipReason::Paused,
        CaptureOutcome::SkippedIgnoredApp { bundle_id } => {
            let _ = bundle_id;
            CaptureSkipReason::IgnoredApp
        }
        CaptureOutcome::SkippedSensitive => CaptureSkipReason::ApiKeyFilter,
        CaptureOutcome::SkippedPrivateMarker => CaptureSkipReason::PrivateClipboardMarker,
        CaptureOutcome::TransientPlatformChange => CaptureSkipReason::TransientPlatformChange,
        CaptureOutcome::Stored { .. } | CaptureOutcome::ObservedExisting { .. } => {
            unreachable!("stored outcomes do not have skip reasons")
        }
    }
}

struct OcrWorker {
    stop: std::sync::mpsc::Sender<()>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Drop for OcrWorker {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            if thread.join().is_err() {
                eprintln!("OCR worker panicked during shutdown");
            }
        }
    }
}

fn start_ocr_worker(db_path: PathBuf) -> Result<Option<OcrWorker>> {
    let Some(registration) = claim_ocr_worker(&db_path) else {
        return Ok(None);
    };
    let (stop, receiver) = std::sync::mpsc::channel();
    let thread = thread::Builder::new()
        .name("clipmem-ocr".into())
        .spawn(move || {
            let _registration = registration;
            let engine = crate::ocr::default_engine();
            loop {
                let run = || -> Result<usize> {
                    let mut db = Database::open_read_write_current(&db_path)?;
                    if !db.capture_settings()?.ocr_enabled() {
                        return Ok(0);
                    }
                    let report = crate::ocr::run_queued_ocr_jobs(&mut db, &engine, 1)?;
                    if report.processed() > 0 {
                        notify_app_refresh();
                    }
                    Ok(report.processed())
                };
                let delay = match run() {
                    Ok(0) => Duration::from_secs(1),
                    Ok(_) => Duration::ZERO,
                    Err(error) => {
                        eprintln!("ocr failed: {error:#}");
                        Duration::from_secs(5)
                    }
                };
                match receiver.recv_timeout(delay) {
                    Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                }
            }
        })
        .context("start OCR worker")?;
    Ok(Some(OcrWorker {
        stop,
        thread: Some(thread),
    }))
}

fn claim_ocr_worker(db_path: &Path) -> Option<OcrWorkerRegistration> {
    let key = canonical_ocr_worker_key(db_path);
    let mut workers = ocr_workers();
    if !workers.insert(key.clone()) {
        return None;
    }
    Some(OcrWorkerRegistration { key })
}

fn ocr_workers() -> MutexGuard<'static, HashSet<PathBuf>> {
    match OCR_WORKERS.lock() {
        Ok(workers) => workers,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn canonical_ocr_worker_key(db_path: &Path) -> PathBuf {
    if let Ok(path) = db_path.canonicalize() {
        return path;
    }
    if db_path.is_absolute() {
        return db_path.to_path_buf();
    }
    std::env::current_dir()
        .map(|cwd| cwd.join(db_path))
        .unwrap_or_else(|_| db_path.to_path_buf())
}

struct OcrWorkerRegistration {
    key: PathBuf,
}

impl Drop for OcrWorkerRegistration {
    fn drop(&mut self) {
        ocr_workers().remove(&self.key);
    }
}

pub(in crate::cli) fn capture_once(db_path: &Path, args: &CaptureOnceArgs) -> Result<()> {
    let mut db = open_or_init_db(db_path)?;
    let payload = capture_once_payload(&mut db, || {
        capture_snapshot()
            .map_err(|error| platform_error(format!("capture-once clipboard read failed: {error}")))
    })?;
    if args.human {
        display!("{}", render_capture_once_human(&payload));
    } else {
        emit_json_or_text(args.json, &payload, render_capture_once_text)?;
    }

    Ok(())
}

fn capture_once_payload<CaptureFn>(
    db: &mut Database,
    capture_snapshot_fn: CaptureFn,
) -> Result<CaptureOnceOutput>
where
    CaptureFn: FnOnce() -> Result<ClipboardSnapshot>,
{
    if db.capture_settings()?.paused() {
        return Ok(CaptureOnceOutput::Skipped(CaptureOnceSkippedOutput {
            status: "skipped",
            reason: CaptureSkipReason::Paused,
            kind: "unknown".into(),
            total_bytes: 0,
            frontmost_app_name: None,
            frontmost_app_bundle_id: None,
        }));
    }
    let snapshot = capture_snapshot_fn()?;
    let payload =
        match CaptureApplicationService::new(db).capture(&snapshot, CaptureMode::Manual)? {
            CaptureOutcome::Stored { store, enqueue_ocr }
            | CaptureOutcome::ObservedExisting { store, enqueue_ocr } => {
                let ocr_completion = if enqueue_ocr {
                    crate::ocr::finish_snapshot_ocr(
                        db,
                        &crate::ocr::default_engine(),
                        store.snapshot_id(),
                        std::time::Duration::from_secs(30),
                    )
                } else {
                    Ok(())
                };
                db.apply_retention_policy()
                    .context("apply retention policy failed")?;
                notify_app_refresh();
                ocr_completion?;
                CaptureOnceOutput::Stored(CaptureOnceStoredOutput { store, snapshot })
            }
            outcome => {
                let reason = capture_outcome_skip_reason(&outcome);
                CaptureOnceOutput::Skipped(CaptureOnceSkippedOutput {
                    status: "skipped",
                    reason,
                    kind: snapshot.snapshot_kind().as_str().to_string(),
                    total_bytes: snapshot.total_bytes(),
                    frontmost_app_name: snapshot.frontmost_app_name().map(str::to_string),
                    frontmost_app_bundle_id: snapshot.frontmost_app_bundle_id().map(str::to_string),
                })
            }
        };

    Ok(payload)
}

pub(in crate::cli) fn open_read_only_db(path: &Path) -> Result<Database> {
    map_current_open(path, Database::open_read_only_current(path))
}

pub(in crate::cli) fn open_read_write_db(path: &Path) -> Result<Database> {
    map_current_open(path, Database::open_read_write_current(path))
}

fn map_current_open(
    path: &Path,
    result: std::result::Result<Database, crate::db::DatabaseOpenError>,
) -> Result<Database> {
    match result {
        Ok(db) => {
            if let Err(error) = db.ensure_supported_schema_shape() {
                if error
                    .chain()
                    .any(|cause| cause.to_string().contains("prerelease schema"))
                {
                    return Err(db_error(
                        "database operation failed; this may be an incompatible prerelease schema. Move the database aside and run `clipmem setup`."
                    ));
                }
                return Err(error);
            }
            Ok(db)
        }
        Err(error) => {
            match error {
                crate::db::DatabaseOpenError::Missing(_) => Err(db_error(format!(
                    "database does not exist at {}. Run `clipmem setup` to initialize capture.",
                    path.display()
                ))),
                crate::db::DatabaseOpenError::MigrationRequired { found, supported } => {
                    Err(db_error(format!(
                        "database schema version {found} requires migration to version {supported}. Run `clipmem setup` to migrate the archive."
                    )))
                }
                crate::db::DatabaseOpenError::NewerSchema { found, supported } => {
                    Err(db_error(format!(
                        "database schema version {found} is newer than this clipmem build supports ({supported}); upgrade clipmem before opening it"
                    )))
                }
                crate::db::DatabaseOpenError::NotArchive(_) => Err(db_error(format!(
                    "database at {} is not a clipmem archive",
                    path.display()
                ))),
                other => Err(anyhow::Error::from(other))
                    .with_context(|| format!("failed to open database at {}", path.display())),
            }
        }
    }
}

pub(in crate::cli) fn open_or_init_db(path: &Path) -> Result<Database> {
    anyhow::Context::with_context(Database::open_or_init_and_migrate(path), || {
        format!("failed to open database at {}", path.display())
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process;
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::db::Database;
    use crate::model::{build_item, build_representation, build_snapshot, CaptureContext};

    use super::{capture_once_payload, claim_ocr_worker};

    #[test]
    fn ocr_worker_guard_allows_distinct_database_paths_in_same_process() {
        let first = temp_db_path("ocr-worker-first");
        let second = temp_db_path("ocr-worker-second");
        create_empty_file(&first);
        create_empty_file(&second);

        let first_registration =
            claim_ocr_worker(&first).expect("first database should claim worker");
        assert!(
            claim_ocr_worker(&first).is_none(),
            "same database should not claim a second worker"
        );

        let second_registration =
            claim_ocr_worker(&second).expect("different database should claim its own worker");

        drop(first_registration);
        assert!(
            claim_ocr_worker(&first).is_some(),
            "dropped registrations should release their database key"
        );

        drop(second_registration);
        cleanup_db(&first);
        cleanup_db(&second);
    }

    #[test]
    fn capture_once_reports_restored_snapshot_as_skip() -> anyhow::Result<()> {
        let mut db = Database::open_in_memory()?;
        let stored = db.store_capture(&fake_snapshot(1, "restored text"))?;
        let snapshot = db
            .find_snapshot(stored.snapshot_id(), 1)?
            .expect("stored snapshot should exist");
        let operation = db.begin_restore_operation(stored.snapshot_id(), snapshot.sha256(), 2)?;
        db.mark_restore_written(&operation, 2)?;

        let payload = capture_once_payload(&mut db, || Ok(fake_snapshot(2, "restored text")))?;

        match payload {
            crate::cli::output::CaptureOnceOutput::Skipped(skipped) => {
                assert_eq!(skipped.reason.as_str(), "restored_snapshot");
            }
            other => panic!("expected restored snapshot skip, got {other:?}"),
        }
        let event_count: i64 =
            db.conn
                .query_row("SELECT COUNT(*) FROM capture_events", [], |row| row.get(0))?;
        assert_eq!(event_count, 1);
        Ok(())
    }

    fn temp_db_path(test_name: &str) -> PathBuf {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after unix epoch")
            .as_nanos();
        std::env::temp_dir()
            .join("clipmem-ocr-worker-tests")
            .join(format!("{test_name}-{}-{timestamp}.sqlite3", process::id()))
    }

    fn create_empty_file(path: &Path) {
        fs::create_dir_all(path.parent().expect("test path should have parent"))
            .expect("test database parent should be created");
        fs::File::create(path).expect("test database file should be created");
    }

    fn cleanup_db(path: &Path) {
        for suffix in ["", "-shm", "-wal"] {
            let candidate = if suffix.is_empty() {
                path.to_path_buf()
            } else {
                PathBuf::from(format!("{}{suffix}", path.display()))
            };
            let _ = fs::remove_file(candidate);
        }
        if let Some(parent) = path.parent() {
            let _ = fs::remove_dir(parent);
        }
    }

    fn fake_snapshot(change_count: i64, text: &str) -> crate::model::ClipboardSnapshot {
        build_snapshot(
            CaptureContext::new(change_count)
                .with_frontmost_app_name("Terminal")
                .with_frontmost_app_bundle_id("com.apple.Terminal"),
            vec![build_item(
                0,
                vec![build_representation(
                    "public.utf8-plain-text".to_string(),
                    None,
                    text.as_bytes().to_vec(),
                )],
            )],
        )
    }
}
