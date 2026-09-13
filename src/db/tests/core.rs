use super::*;
use crate::capture_service::CaptureApplicationService;
use crate::db::{CaptureMode, CaptureOutcome};
use crate::model::{build_item, build_representation, build_snapshot, CaptureContext};

#[test]
fn doctor_invariants_count_missing_canonical_document() -> Result<()> {
    let mut db = Database::open_in_memory()?;
    let stored = db.store_capture(&fake_snapshot(1, "missing document"))?;
    db.conn.execute(
        "DELETE FROM snapshot_search_documents WHERE snapshot_id=?1",
        [stored.snapshot_id()],
    )?;

    assert!(db.doctor()?.integrity().is_none());
    let verified = db.doctor_verifying_invariants()?;
    assert_eq!(
        verified
            .integrity()
            .context("integrity should be verified")?
            .projection_mismatch_count(),
        1
    );
    Ok(())
}

#[test]
fn doctor_shadow_compare_accepts_builder_v3_rich_text_differences() -> Result<()> {
    let mut db = Database::open_in_memory()?;
    let html = r#"<p>Visible <strong>text</strong></p><a href="https://example.com/rich">link</a>"#;
    let snapshot = build_snapshot(
        CaptureContext::new(1),
        vec![build_item(
            0,
            vec![build_representation(
                "public.html".to_string(),
                Some(html.to_string()),
                html.as_bytes().to_vec(),
            )],
        )],
    );
    db.store_capture(&snapshot)?;

    let verified = db.doctor_verifying_invariants()?;
    assert_eq!(
        verified
            .integrity()
            .context("integrity should be verified")?
            .projection_mismatch_count(),
        0
    );
    Ok(())
}

#[test]
fn duplicate_snapshots_share_content_row() -> Result<()> {
    let mut db = Database::open_in_memory()?;

    let first = fake_snapshot(1, "git clone https://example.com/repo");
    let second = fake_snapshot(2, "git clone https://example.com/repo");

    let first_store = db.store_capture(&first)?;
    let second_store = db.store_capture(&second)?;

    assert!(first_store.inserted_new_snapshot());
    assert!(!second_store.inserted_new_snapshot());
    assert_eq!(first_store.snapshot_id(), second_store.snapshot_id());

    let results = db.search_auto("git", 10, &unfiltered())?;
    assert_eq!(results.mode_used(), SearchMode::Fts);
    assert_eq!(results.hits().len(), 1);
    assert_eq!(results.hits()[0].capture_count(), 2);

    let details = db
        .find_snapshot(first_store.snapshot_id(), 10)?
        .context("expected stored snapshot details")?;
    assert_eq!(details.capture_count(), 2);
    assert_eq!(details.items().len(), 1);

    Ok(())
}

#[test]
fn open_existing_does_not_create_missing_database_paths() {
    let path = temp_db_path("open-existing-missing");

    let result = Database::open_existing(&path);

    assert!(result.is_err());
    assert!(!path.exists());
}

#[test]
fn current_open_modes_return_typed_schema_and_identity_errors() -> Result<()> {
    let missing = temp_db_path("typed-open-missing");
    assert!(matches!(
        Database::open_read_only_current(&missing),
        Err(crate::db::DatabaseOpenError::Missing(_))
    ));

    let unrelated = temp_db_path("typed-open-not-archive");
    std::fs::create_dir_all(unrelated.parent().expect("path has parent"))?;
    rusqlite::Connection::open(&unrelated)?.execute("CREATE TABLE unrelated (id INTEGER)", [])?;
    assert!(matches!(
        Database::open_read_only_current(&unrelated),
        Err(crate::db::DatabaseOpenError::NotArchive(_))
    ));
    cleanup_db(&unrelated);

    let migration = temp_db_path("typed-open-migration");
    drop(Database::open_or_init_and_migrate(&migration)?);
    rusqlite::Connection::open(&migration)?.pragma_update(None, "user_version", 18)?;
    assert!(matches!(
        Database::open_read_only_current(&migration),
        Err(crate::db::DatabaseOpenError::MigrationRequired {
            found: 18,
            supported: CURRENT_SCHEMA_VERSION
        })
    ));
    cleanup_db(&migration);

    let newer = temp_db_path("typed-open-newer");
    drop(Database::open_or_init_and_migrate(&newer)?);
    rusqlite::Connection::open(&newer)?.pragma_update(
        None,
        "user_version",
        CURRENT_SCHEMA_VERSION + 1,
    )?;
    assert!(matches!(
        Database::open_read_write_current(&newer),
        Err(crate::db::DatabaseOpenError::NewerSchema { .. })
    ));
    cleanup_db(&newer);
    Ok(())
}

#[cfg(unix)]
#[test]
fn read_only_current_does_not_chmod_or_create_sqlite_sidecars() -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;

    // File permissions do not prove immutability. A WAL archive without usable
    // sidecars must fail safely instead of bypassing locking.
    let shared = temp_db_path("read-only-no-filesystem-writes");
    let parent = shared.with_extension("dir");
    std::fs::create_dir_all(&parent)?;
    let path = parent.join("archive.sqlite3");
    drop(Database::open_or_init_and_migrate(&path)?);
    for suffix in ["-wal", "-shm"] {
        let _ = std::fs::remove_file(sidecar_path(&path, suffix));
    }
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o440))?;
    let before = std::fs::metadata(&path)?.permissions().mode() & 0o777;
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o555))?;

    let result = (|| -> Result<()> {
        assert!(Database::open_read_only_current(&path).is_err());

        assert_eq!(
            std::fs::metadata(&path)?.permissions().mode() & 0o777,
            before
        );
        assert!(!sidecar_path(&path, "-wal").exists());
        assert!(!sidecar_path(&path, "-shm").exists());
        Ok(())
    })();

    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o755))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640))?;
    cleanup_db(&path);
    let _ = std::fs::remove_dir(&parent);
    result
}

#[test]
fn read_only_current_reads_committed_wal_state_while_writer_remains_open() -> Result<()> {
    let path = temp_db_path("read-only-live-wal");
    drop(Database::open_or_init_and_migrate(&path)?);
    let mut writer = Database::open_read_write_current(&path)?;
    writer.conn.pragma_update(None, "wal_autocheckpoint", 0)?;
    let stored = writer.store_capture(&fake_snapshot(1, "visible from wal"))?;
    assert!(sidecar_path(&path, "-wal").exists());

    let reader = Database::open_read_only_current(&path)?;
    let metadata = reader
        .find_snapshot_metadata(stored.snapshot_id(), 1)?
        .context("reader should observe committed WAL content")?;
    assert_eq!(metadata.best_text(), "visible from wal");

    drop(reader);
    drop(writer);
    cleanup_db(&path);
    Ok(())
}

#[cfg(unix)]
#[test]
fn hardening_sqlite_sidecars_applies_private_file_permissions() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let path = temp_db_path("sidecar-permissions");
    std::fs::create_dir_all(path.parent().unwrap())?;

    for suffix in ["-wal", "-shm"] {
        let sidecar = super::super::sidecar_path(&path, suffix);
        std::fs::write(&sidecar, b"sidecar")?;
        std::fs::set_permissions(&sidecar, std::fs::Permissions::from_mode(0o666))?;
    }

    super::super::harden_existing_sqlite_sidecar_permissions(&path)?;

    for suffix in ["-wal", "-shm"] {
        let sidecar = super::super::sidecar_path(&path, suffix);
        let mode = std::fs::metadata(&sidecar)?.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    cleanup_db(&path);
    Ok(())
}

#[test]
fn search_like_treats_percent_as_literal() -> Result<()> {
    let mut db = Database::open_in_memory()?;

    db.store_capture(&fake_snapshot(1, "Discount: 50 percent off"))?;
    db.store_capture(&fake_snapshot(2, "Discount: 50%"))?;

    let hits = db.search_literal("50%", 10, &unfiltered())?;
    let previews: Vec<_> = hits
        .hits()
        .iter()
        .map(crate::model::SearchHit::preview_text)
        .collect();

    assert_eq!(previews, vec!["Discount: 50%"]);
    Ok(())
}

#[test]
fn stats_empty_result_returns_zeroes_and_empty_leaderboards() -> Result<()> {
    let mut db = Database::open_in_memory()?;
    db.store_capture(&fake_snapshot(1, "hello"))?;

    let stats = db.stats(&filters_with_app("No Such App"))?;

    assert_eq!(stats.snapshot_count(), 0);
    assert_eq!(stats.capture_event_count(), 0);
    assert_eq!(stats.unique_app_count(), 0);
    assert_eq!(stats.total_bytes(), 0);
    assert_eq!(stats.average_bytes_per_snapshot(), 0.0);
    assert_eq!(stats.average_captures_per_snapshot(), 0.0);
    assert_eq!(stats.dedupe_ratio(), 0.0);
    assert_eq!(stats.first_observed_at(), None);
    assert_eq!(stats.last_observed_at(), None);
    assert_eq!(stats.archive_span_seconds(), None);
    assert!(stats.most_recopied_snapshot().is_none());
    assert!(stats.kind_breakdown().is_empty());
    assert!(stats.top_apps().is_empty());
    assert_eq!(stats.busiest_hours().len(), 24);
    assert_eq!(stats.busiest_weekdays().len(), 7);
    assert!(stats.largest_snapshots().is_empty());
    assert!(stats.most_captured_snapshots().is_empty());
    Ok(())
}

#[test]
fn stats_uses_event_and_snapshot_filtering_semantics() -> Result<()> {
    let mut db = Database::open_in_memory()?;
    let first = db.store_capture(&fake_snapshot(1, "repeat text"))?;
    let second = db.store_capture(&fake_snapshot(2, "repeat text"))?;
    let third = db.store_capture(&fake_snapshot(3, "other text"))?;
    set_event_app(
        &db,
        first.event_id(),
        Some("Terminal"),
        Some("com.apple.Terminal"),
    )?;
    set_event_app(
        &db,
        second.event_id(),
        Some("Safari"),
        Some("com.apple.Safari"),
    )?;
    set_event_app(
        &db,
        third.event_id(),
        Some("Safari"),
        Some("com.apple.Safari"),
    )?;

    let stats = db.stats(&filters_with_app("safari"))?;

    assert_eq!(stats.capture_event_count(), 2);
    assert_eq!(stats.snapshot_count(), 2);
    assert_eq!(stats.most_recopied_snapshot().unwrap().capture_count(), 1);
    assert_eq!(stats.top_apps()[0].app(), "Safari");
    assert_eq!(stats.top_apps()[0].capture_event_count(), 2);

    let text_stats = db.stats(&filters_with_kind(RetrievalKind::Text))?;
    assert_eq!(text_stats.capture_event_count(), 3);
    assert_eq!(text_stats.snapshot_count(), 2);
    Ok(())
}

#[test]
fn stats_app_grouping_falls_back_to_bundle_and_unknown() -> Result<()> {
    let mut db = Database::open_in_memory()?;
    let first = db.store_capture(&fake_snapshot(1, "first"))?;
    let second = db.store_capture(&fake_snapshot(2, "second"))?;
    let third = db.store_capture(&fake_snapshot(3, "third"))?;
    set_event_app(
        &db,
        first.event_id(),
        Some("Named App"),
        Some("com.example.named"),
    )?;
    set_event_app(&db, second.event_id(), None, Some("com.example.bundle"))?;
    set_event_app(&db, third.event_id(), None, None)?;

    let stats = db.stats(&unfiltered())?;
    let apps: Vec<_> = stats.top_apps().iter().map(|entry| entry.app()).collect();

    assert!(apps.contains(&"Named App"));
    assert!(apps.contains(&"com.example.bundle"));
    assert!(apps.contains(&"Unknown"));
    assert_eq!(stats.unique_app_count(), 3);
    Ok(())
}

#[test]
fn stats_ordering_ties_are_deterministic() -> Result<()> {
    let mut db = Database::open_in_memory()?;
    let first = db.store_capture(&fake_snapshot(1, "alpha"))?;
    let second = db.store_capture(&fake_snapshot(2, "bravo"))?;
    set_event_observed_at(&db, first.event_id(), "2026-04-17T10:00:00Z")?;
    set_event_observed_at(&db, second.event_id(), "2026-04-17T10:00:00Z")?;

    let stats = db.stats(&unfiltered())?;

    assert_eq!(
        stats.largest_snapshots()[0].snapshot_id(),
        first.snapshot_id()
    );
    assert_eq!(
        stats.most_captured_snapshots()[0].snapshot_id(),
        first.snapshot_id()
    );
    assert_eq!(stats.busiest_hours()[10].capture_event_count(), 2);
    assert_eq!(
        stats
            .busiest_weekdays()
            .iter()
            .find(|entry| entry.bucket() == "Friday")
            .unwrap()
            .capture_event_count(),
        2
    );
    Ok(())
}

#[test]
fn capture_policy_persists_across_reopen() -> Result<()> {
    let path = temp_db_path("capture-policy-persistence");

    {
        let mut db = Database::open_or_init(&path)?;
        db.set_paused(true)?;
        db.set_api_key_filter_enabled(true)?;
        db.set_retention_seconds(Some(30 * 24 * 60 * 60))?;
        assert!(db.add_ignored_bundle_id("Com.Apple.Terminal")?);
    }

    let reopened = Database::open_existing(&path)?;
    let policy = reopened.capture_policy()?;

    assert!(policy.settings().paused());
    assert!(policy.settings().api_key_filter_enabled());
    assert_eq!(
        policy.settings().retention_seconds(),
        Some(30 * 24 * 60 * 60)
    );
    assert_eq!(
        policy.ignored_bundle_ids(),
        &["com.apple.terminal".to_string()]
    );

    cleanup_db(&path);
    Ok(())
}

#[test]
fn ocr_setting_defaults_to_off_and_toggles() -> Result<()> {
    let mut db = Database::open_in_memory()?;

    assert!(!db.capture_settings()?.ocr_enabled());
    db.set_ocr_enabled(true)?;
    assert!(db.capture_settings()?.ocr_enabled());
    db.set_ocr_enabled(false)?;
    assert!(!db.capture_settings()?.ocr_enabled());

    Ok(())
}

#[test]
fn ready_ocr_text_is_cached_deduped_and_searchable() -> Result<()> {
    let mut db = Database::open_in_memory()?;
    let stored = db.store_capture(&image_snapshot(
        1,
        vec![
            ("public.png", b"same-image-bytes".to_vec()),
            ("public.jpeg", b"same-image-bytes".to_vec()),
        ],
    ))?;

    assert_eq!(db.enqueue_ocr_for_snapshot(stored.snapshot_id())?, 1);
    assert_eq!(db.archive_revision()?.ocr_revision(), 1);
    let candidates = db.next_ocr_candidates(25, None, false)?;
    assert_eq!(candidates.len(), 1);
    db.store_ocr_text(
        candidates[0].raw_sha256(),
        "fake",
        "fast",
        "Invoice Total 42",
    )?;
    assert_eq!(db.archive_revision()?.ocr_revision(), 2);

    let details = db
        .find_snapshot(stored.snapshot_id(), 10)?
        .expect("stored image snapshot should exist");
    assert_eq!(details.ocr_text(), Some("Invoice Total 42"));
    assert_eq!(details.ocr_status(), Some("ready"));
    assert_eq!(details.best_text(), "Invoice Total 42");
    assert_eq!(details.best_text_uti(), Some("com.clipmem.ocr.text"));

    let results = db.search_auto("Invoice", 10, &unfiltered())?;
    assert_eq!(results.hits().len(), 1);
    assert_eq!(results.hits()[0].snapshot_id(), stored.snapshot_id());
    assert_eq!(
        results.hits()[0].matched_fields(),
        &["ocr_text".to_string()]
    );
    assert_eq!(db.ocr_status_report()?.snapshots_with_ocr_text(), 1);

    Ok(())
}

#[test]
fn failed_ocr_results_do_not_enter_search() -> Result<()> {
    let mut db = Database::open_in_memory()?;
    let stored = db.store_capture(&image_snapshot(
        1,
        vec![("public.png", b"not-a-supported-image".to_vec())],
    ))?;
    db.enqueue_ocr_for_snapshot(stored.snapshot_id())?;
    let candidates = db.next_ocr_candidates(25, None, false)?;
    assert_eq!(candidates.len(), 1);

    db.store_ocr_failure(candidates[0].raw_sha256(), "fake", "fast", "decode failed")?;

    let details = db
        .find_snapshot(stored.snapshot_id(), 10)?
        .expect("stored image snapshot should exist");
    assert_eq!(details.ocr_text(), None);
    assert_eq!(details.ocr_status(), Some("failed"));
    assert!(db
        .search_auto("decode", 10, &unfiltered())?
        .hits()
        .is_empty());
    assert_eq!(db.ocr_status_report()?.failed(), 1);

    Ok(())
}

#[test]
fn new_snapshot_with_cached_ready_ocr_is_searchable_in_same_capture() -> Result<()> {
    let mut db = Database::open_in_memory()?;
    db.set_ocr_enabled(true)?;

    let image_bytes = b"cached-ocr-image".to_vec();
    let first = CaptureApplicationService::new(&mut db).capture(
        &image_snapshot(1, vec![("public.png", image_bytes.clone())]),
        CaptureMode::Watch,
    )?;
    assert!(matches!(first, CaptureOutcome::Stored { .. }));

    // Mark the shared image hash as OCR-ready between captures.
    db.conn.execute(
        "UPDATE ocr_results SET status = 'ready', text_value = 'zanzibar receipt total'",
        [],
    )?;

    // A different snapshot reusing the same representation hash must pick up
    // the cached OCR text in its search document within the capture itself.
    let second = CaptureApplicationService::new(&mut db).capture(
        &image_snapshot(
            2,
            vec![
                ("public.png", image_bytes),
                ("public.utf8-plain-text", b"with a caption".to_vec()),
            ],
        ),
        CaptureMode::Watch,
    )?;
    let snapshot_id = match second {
        CaptureOutcome::Stored { ref store, .. } => store.snapshot_id(),
        other => panic!("expected stored capture, got {other:?}"),
    };

    let (ocr_text, has_ocr): (String, bool) = db.conn.query_row(
        "SELECT ocr_text, has_ocr_text FROM snapshot_search_documents WHERE snapshot_id = ?1",
        [snapshot_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    assert!(ocr_text.contains("zanzibar receipt total"));
    assert!(has_ocr);
    // Snapshot 1 only picks the text up via the worker's by-hash rebuild,
    // which this test bypasses; the new snapshot must match immediately.
    let hits = db.search_auto("zanzibar", 10, &unfiltered())?;
    assert!(hits
        .hits()
        .iter()
        .any(|hit| hit.snapshot_id() == snapshot_id));

    Ok(())
}

#[test]
fn next_ocr_candidates_prunes_orphan_pending_rows() -> Result<()> {
    let mut db = Database::open_in_memory()?;
    db.conn.execute(
        "INSERT INTO ocr_results (raw_sha256, status, updated_at)
         VALUES (?1, 'pending', '2026-04-29 11:21:58')",
        ["4240b1324fdb30ff0f0fd538b0bb40a8a93e4f75072ff0cd25f5886d11fe45fe"],
    )?;
    let stored = db.store_capture(&image_snapshot(
        1,
        vec![("public.png", b"valid-image-candidate".to_vec())],
    ))?;
    db.enqueue_ocr_for_snapshot(stored.snapshot_id())?;

    let summaries = db.ocr_candidate_summaries(25, None)?;
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].byte_len(), b"valid-image-candidate".len());

    let candidates = db.next_ocr_candidates(25, None, false)?;

    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].blob_value(), b"valid-image-candidate");
    assert!(db
        .ocr_result("4240b1324fdb30ff0f0fd538b0bb40a8a93e4f75072ff0cd25f5886d11fe45fe")?
        .is_none());
    assert_eq!(db.ocr_status_report()?.pending(), 1);

    Ok(())
}

#[test]
fn ocr_candidate_summaries_keep_global_snapshot_count_when_filtered() -> Result<()> {
    let mut db = Database::open_in_memory()?;
    let shared_image = b"shared-image-bytes".to_vec();
    let mut snapshot_ids = Vec::new();

    for change_count in 1..=3 {
        let image_item = build_item(
            0,
            vec![build_representation(
                "public.png".to_string(),
                None,
                shared_image.clone(),
            )],
        );
        let text_item = build_item(
            1,
            vec![build_representation(
                "public.utf8-plain-text".to_string(),
                None,
                format!("unique snapshot {change_count}").into_bytes(),
            )],
        );
        let stored = db.store_capture(&build_snapshot(
            CaptureContext::new(change_count),
            vec![image_item, text_item],
        ))?;
        snapshot_ids.push(stored.snapshot_id());
        db.enqueue_ocr_for_snapshot(stored.snapshot_id())?;
    }

    let summaries = db.ocr_candidate_summaries(10, Some(snapshot_ids[0]))?;

    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].snapshot_count(), 3);
    Ok(())
}

#[test]
fn store_capture_if_allowed_skips_api_key_like_snapshots_when_enabled() -> Result<()> {
    let mut db = Database::open_in_memory()?;
    db.set_api_key_filter_enabled(true)?;

    let outcome = CaptureApplicationService::new(&mut db).capture(
        &fake_snapshot(1, "Authorization: Bearer 8JfA-2mQpV_4tLz9XnR6cH0wKdS7yBu3"),
        CaptureMode::Manual,
    )?;

    assert!(matches!(outcome, CaptureOutcome::SkippedSensitive));
    assert!(db.recent(10, &unfiltered())?.hits().is_empty());
    Ok(())
}

#[test]
fn store_capture_if_allowed_stores_sensitive_text_when_filter_is_disabled() -> Result<()> {
    let mut db = Database::open_in_memory()?;

    let outcome = CaptureApplicationService::new(&mut db).capture(
        &fake_snapshot(1, "Authorization: Bearer 8JfA-2mQpV_4tLz9XnR6cH0wKdS7yBu3"),
        CaptureMode::Manual,
    )?;

    assert!(matches!(outcome, CaptureOutcome::Stored { .. }));
    assert_eq!(db.recent(10, &unfiltered())?.hits().len(), 1);
    Ok(())
}

#[test]
fn recent_reports_whether_more_results_are_available() -> Result<()> {
    let mut db = Database::open_in_memory()?;
    db.store_capture(&fake_snapshot(1, "first recent item"))?;
    db.store_capture(&fake_snapshot(2, "second recent item"))?;

    let first_page = db.recent(1, &unfiltered())?;
    let full_page = db.recent(2, &unfiltered())?;

    assert_eq!(first_page.hits().len(), 1);
    assert!(first_page.has_more());
    assert_eq!(full_page.hits().len(), 2);
    assert!(!full_page.has_more());
    Ok(())
}

#[test]
fn timeline_exposes_public_events_and_pagination_state() -> Result<()> {
    let mut db = Database::open_in_memory()?;
    db.store_capture(&fake_snapshot(1, "first timeline item"))?;
    db.store_capture(&fake_snapshot(2, "second timeline item"))?;

    let first_page = db.timeline(1, &unfiltered(), TimelineSort::Desc)?;
    let full_page = db.timeline(2, &unfiltered(), TimelineSort::Desc)?;

    assert_eq!(first_page.events().len(), 1);
    assert!(first_page.has_more());
    assert_eq!(full_page.events().len(), 2);
    assert!(!full_page.has_more());
    Ok(())
}

#[test]
fn written_restore_suppresses_only_its_exact_generation() -> Result<()> {
    let mut db = Database::open_in_memory()?;
    let stored = db.store_capture(&fake_snapshot(1, "git status"))?;

    let sha = db
        .find_snapshot(stored.snapshot_id(), 1)?
        .unwrap()
        .sha256()
        .to_string();
    let operation = db.begin_restore_operation(stored.snapshot_id(), &sha, 2)?;
    db.mark_restore_written(&operation, 2)?;
    let outcome = CaptureApplicationService::new(&mut db)
        .capture(&fake_snapshot(2, "git status"), CaptureMode::Watch)?;

    assert!(matches!(outcome, CaptureOutcome::SuppressedRestore { .. }));
    let event_count: i64 = db
        .conn
        .query_row("SELECT COUNT(*) FROM capture_events", [], |row| row.get(0))?;
    assert_eq!(event_count, 1);

    let outcome = CaptureApplicationService::new(&mut db)
        .capture(&fake_snapshot(3, "git status"), CaptureMode::Watch)?;

    assert!(matches!(outcome, CaptureOutcome::ObservedExisting { .. }));
    let event_count: i64 = db
        .conn
        .query_row("SELECT COUNT(*) FROM capture_events", [], |row| row.get(0))?;
    assert_eq!(event_count, 2);
    Ok(())
}

#[test]
fn expired_restore_operation_does_not_suppress_capture() -> Result<()> {
    let mut db = Database::open_in_memory()?;
    let stored = db.store_capture(&fake_snapshot(1, "git status"))?;
    let sha256 = db
        .find_snapshot(stored.snapshot_id(), 1)?
        .unwrap()
        .sha256()
        .to_string();
    let operation = db.begin_restore_operation(stored.snapshot_id(), &sha256, 2)?;
    db.mark_restore_written(&operation, 2)?;
    db.conn.execute(
        "UPDATE restore_operations SET expires_at = datetime('now', '-1 second') WHERE operation_id = ?1",
        [&operation],
    )?;

    let outcome = CaptureApplicationService::new(&mut db)
        .capture(&fake_snapshot(2, "git status"), CaptureMode::Watch)?;

    assert!(matches!(outcome, CaptureOutcome::ObservedExisting { .. }));
    let state: String = db.conn.query_row(
        "SELECT state FROM restore_operations WHERE operation_id = ?1",
        [&operation],
        |row| row.get(0),
    )?;
    assert_eq!(state, "expired");
    Ok(())
}

#[test]
fn forget_snapshot_cascades_and_removes_search_visibility() -> Result<()> {
    let mut db = Database::open_in_memory()?;
    let stored = db.store_capture(&fake_snapshot(1, "git status"))?;

    let report = db
        .forget_snapshot(stored.snapshot_id())?
        .expect("stored snapshot should be forgettable");

    assert_eq!(report.snapshot_id(), stored.snapshot_id());
    assert_eq!(report.item_count(), 1);
    assert_eq!(report.representation_count(), 1);
    assert_eq!(report.capture_event_count(), 1);
    assert_eq!(report.total_bytes(), "git status".len());
    assert!(db.find_snapshot(stored.snapshot_id(), 10)?.is_none());
    assert!(db.search_auto("git", 10, &unfiltered())?.hits().is_empty());

    let snapshot_count: i64 = db
        .conn
        .query_row("SELECT COUNT(*) FROM snapshots", [], |row| row.get(0))?;
    let item_count: i64 = db
        .conn
        .query_row("SELECT COUNT(*) FROM snapshot_items", [], |row| row.get(0))?;
    let representation_count: i64 =
        db.conn
            .query_row("SELECT COUNT(*) FROM item_representations", [], |row| {
                row.get(0)
            })?;
    let event_count: i64 = db
        .conn
        .query_row("SELECT COUNT(*) FROM capture_events", [], |row| row.get(0))?;

    assert_eq!(snapshot_count, 0);
    assert_eq!(item_count, 0);
    assert_eq!(representation_count, 0);
    assert_eq!(event_count, 0);
    Ok(())
}

#[test]
fn forget_snapshot_deletes_orphaned_ocr_results() -> Result<()> {
    let mut db = Database::open_in_memory()?;
    let image_bytes = b"same-image-bytes".to_vec();
    let first = db.store_capture(&image_snapshot(
        1,
        vec![("public.png", image_bytes.clone())],
    ))?;
    let second = db.store_capture(&image_snapshot(
        2,
        vec![
            ("public.png", image_bytes),
            ("public.utf8-plain-text", b"second distinct item".to_vec()),
        ],
    ))?;

    assert_eq!(db.enqueue_ocr_for_snapshot(first.snapshot_id())?, 1);
    let candidates = db.next_ocr_candidates(25, None, false)?;
    assert_eq!(candidates.len(), 1);
    db.store_ocr_text(
        candidates[0].raw_sha256(),
        "fake",
        "fast",
        "Shared image text",
    )?;

    db.forget_snapshot(first.snapshot_id())?;
    assert_eq!(ocr_result_count(&db)?, 1);

    db.forget_snapshot(second.snapshot_id())?;
    assert_eq!(ocr_result_count(&db)?, 0);
    Ok(())
}

fn ocr_result_count(db: &Database) -> Result<i64> {
    Ok(db
        .conn
        .query_row("SELECT COUNT(*) FROM ocr_results", [], |row| row.get(0))?)
}
