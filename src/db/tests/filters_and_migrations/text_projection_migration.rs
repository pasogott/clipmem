use super::*;

#[test]
fn v23_rebuild_indexes_blob_backed_producer_text_uti() -> Result<()> {
    let path = temp_db_path("v23-blob-backed-producer-text");
    let csv = b"account,region\nacme,eu-central";
    {
        let mut db = Database::open_or_init(&path)?;
        let stored = db.store_capture(&fake_snapshot(1, "legacy placeholder"))?;
        db.conn.execute(
            "UPDATE item_representations
             SET uti = 'public.comma-separated-values-text', kind = 'binary',
                 byte_len = ?2, raw_sha256 = 'legacy-csv-sha',
                 text_value = NULL, blob_value = ?3
             WHERE snapshot_id = ?1",
            params![stored.snapshot_id(), csv.len() as i64, csv.as_slice()],
        )?;
        db.conn.execute(
            "UPDATE snapshot_items
             SET primary_kind = 'binary', primary_uti = 'public.comma-separated-values-text',
                 preview_text = '', search_text = ''
             WHERE snapshot_id = ?1",
            [stored.snapshot_id()],
        )?;
        db.conn.execute(
            "UPDATE snapshots SET snapshot_kind = 'binary', preview_text = '', search_text = ''
             WHERE id = ?1",
            [stored.snapshot_id()],
        )?;
        db.conn.execute(
            "DELETE FROM snapshot_search_documents WHERE snapshot_id = ?1",
            [stored.snapshot_id()],
        )?;
        db.conn.pragma_update(None, "user_version", 22)?;
    }

    let db = Database::open_or_init_and_migrate(&path)?;
    let builder_version: i64 = db.conn.query_row(
        "SELECT builder_version FROM snapshot_search_documents",
        [],
        |row| row.get(0),
    )?;

    assert_eq!(builder_version, 4);
    assert_eq!(
        db.search_literal("acme,eu-central", 10, &unfiltered())?
            .hits()
            .len(),
        1
    );
    let metadata = db
        .find_snapshot_metadata(1, 1)?
        .context("migrated snapshot metadata should exist")?;
    assert_eq!(metadata.best_text(), String::from_utf8_lossy(csv));
    assert_eq!(metadata.text_fragments().len(), 1);

    let documents = db.find_snapshot_documents(&[1])?;
    let list_projection = documents.get(&1).context("list projection should exist")?;
    assert_eq!(list_projection.best_text(), String::from_utf8_lossy(csv));
    assert_eq!(list_projection.text_fragments().len(), 1);

    let with_text = RetrievalFilters::default().requiring_text();
    assert_eq!(db.recent(10, &with_text)?.hits().len(), 1);

    drop(db);
    cleanup_db(&path);
    Ok(())
}
