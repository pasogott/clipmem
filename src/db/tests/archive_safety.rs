use super::*;

#[test]
fn forget_and_compact_remove_marker_from_database_and_fts_pages() -> Result<()> {
    let path = temp_db_path("secure-forget");
    let marker = "forgottenuniquesecretmarkerxyz";
    let mut db = Database::open_or_init_and_migrate(&path)?;
    let stored = db.store_capture(&fake_snapshot(1, marker))?;
    db.store_capture(&fake_snapshot(2, "retained ordinary clipboard"))?;
    db.forget_snapshot(stored.snapshot_id())?;
    db.compact_storage(false)?;
    for file in [
        path.clone(),
        std::path::PathBuf::from(format!("{}-wal", path.display())),
    ] {
        if file.exists() {
            let bytes = std::fs::read(file)?;
            assert!(!bytes
                .windows(marker.len())
                .any(|window| window == marker.as_bytes()));
        }
    }
    assert_eq!(db.recent(10, &unfiltered())?.hits().len(), 1);
    drop(db);
    cleanup_db(&path);
    Ok(())
}

#[test]
fn exclusive_writer_cannot_be_bypassed_by_immutable_read_fallback() -> Result<()> {
    let path = temp_db_path("exclusive-read-lock");
    let db = Database::open_or_init_and_migrate(&path)?;
    db.conn
        .execute_batch("PRAGMA locking_mode=EXCLUSIVE; BEGIN EXCLUSIVE;")?;
    let error = Database::open_read_only_current(&path)
        .err()
        .expect("locked live database should remain locked");
    assert!(!error.to_string().contains("missing"));
    db.conn.execute_batch("ROLLBACK;")?;
    drop(db);
    assert!(Database::open_read_only_current(&path).is_ok());
    cleanup_db(&path);
    Ok(())
}

#[test]
fn deleting_repeated_snapshot_skips_per_event_rebuilds_but_event_deletion_updates_stats(
) -> Result<()> {
    let mut db = Database::open_in_memory()?;
    let first = db.store_capture(&fake_snapshot(1, "repeat"))?;
    let second = db.store_capture(&fake_snapshot(2, "repeat"))?;
    db.conn.execute(
        "DELETE FROM capture_events WHERE id = ?1",
        [second.event_id()],
    )?;
    let count: i64 = db.conn.query_row(
        "SELECT capture_count FROM snapshot_stats WHERE snapshot_id = ?1",
        [first.snapshot_id()],
        |row| row.get(0),
    )?;
    assert_eq!(count, 1);
    // Observe actual derived-cache rebuilds, independent of wall-clock speed.
    db.conn.execute_batch("CREATE TABLE rebuild_count (count INTEGER); INSERT INTO rebuild_count VALUES (0); CREATE TRIGGER observe_stats_rebuild AFTER INSERT ON snapshot_stats BEGIN UPDATE rebuild_count SET count = count + 1; END;")?;
    for change in 3..103 {
        db.store_capture(&fake_snapshot(change, "repeat"))?;
    }
    db.conn.execute("UPDATE rebuild_count SET count = 0", [])?;
    db.forget_snapshot(first.snapshot_id())?;
    let rebuilds: i64 = db
        .conn
        .query_row("SELECT count FROM rebuild_count", [], |row| row.get(0))?;
    assert_eq!(rebuilds, 0);
    assert!(db.find_snapshot(first.snapshot_id(), 1)?.is_none());
    Ok(())
}

#[test]
fn initialization_does_not_modify_foreign_database_or_parent_permissions() -> Result<()> {
    for application_id in [0, 123] {
        let path = temp_db_path(&format!("foreign-init-{application_id}"));
        std::fs::create_dir_all(path.parent().unwrap())?;
        let conn = rusqlite::Connection::open(&path)?;
        conn.execute_batch(
            "CREATE TABLE valuable (value TEXT); INSERT INTO valuable VALUES ('keep');",
        )?;
        conn.pragma_update(None, "application_id", application_id)?;
        drop(conn);
        let before = std::fs::read(&path)?;
        let error = Database::open_or_init_and_migrate(&path)
            .err()
            .expect("foreign archive should be rejected");
        assert!(error.to_string().contains("refusing to initialize"));
        assert_eq!(std::fs::read(&path)?, before);
        cleanup_db(&path);
    }
    Ok(())
}

#[test]
fn deleted_capture_ids_are_not_reused_after_reopen() -> Result<()> {
    let path = temp_db_path("durable-id-allocation");
    let mut db = Database::open_or_init_and_migrate(&path)?;
    let first = db.store_capture(&fake_snapshot(1, "first"))?;
    let second = db.store_capture(&fake_snapshot(2, "second"))?;
    db.conn.execute(
        "DELETE FROM snapshots WHERE id = ?1",
        [second.snapshot_id()],
    )?;
    drop(db);
    let mut db = Database::open_or_init_and_migrate(&path)?;
    let third = db.store_capture(&fake_snapshot(3, "third"))?;
    assert!(third.snapshot_id() > second.snapshot_id());
    assert!(third.event_id() > second.event_id());
    db.conn.execute("DELETE FROM snapshots", [])?;
    let fourth = db.store_capture(&fake_snapshot(4, "fourth"))?;
    assert!(fourth.snapshot_id() > third.snapshot_id());
    assert!(fourth.event_id() > third.event_id());
    assert!(db.find_snapshot(first.snapshot_id(), 1)?.is_none());
    drop(db);
    cleanup_db(&path);
    Ok(())
}

#[test]
fn v24_migration_seeds_durable_id_counters() -> Result<()> {
    let path = temp_db_path("v24-id-allocation");
    let mut db = Database::open_or_init_and_migrate(&path)?;
    let before = db.store_capture(&fake_snapshot(1, "before migration"))?;
    db.conn.execute_batch("DROP TRIGGER snapshots_id_sequence; DROP TRIGGER capture_events_id_sequence; DROP TABLE archive_id_sequences; PRAGMA user_version = 24;")?;
    drop(db);
    let mut db = Database::open_or_init_and_migrate(&path)?;
    db.conn.execute("DELETE FROM snapshots", [])?;
    let after = db.store_capture(&fake_snapshot(2, "after migration"))?;
    assert!(after.snapshot_id() > before.snapshot_id());
    assert!(after.event_id() > before.event_id());
    drop(db);
    cleanup_db(&path);
    Ok(())
}
