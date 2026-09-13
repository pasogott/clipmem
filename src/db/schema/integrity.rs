use anyhow::{bail, Context, Result};
use rusqlite::Connection;

pub(super) fn enforce_representation_item_integrity(conn: &Connection) -> Result<()> {
    let orphan_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM item_representations ir LEFT JOIN snapshot_items si ON si.snapshot_id = ir.snapshot_id AND si.item_index = ir.item_index WHERE si.snapshot_id IS NULL",
            [],
            |row| row.get(0),
        )
        .context("count orphan item representations")?;
    if orphan_count != 0 {
        bail!(
            "schema v20 integrity preflight found {orphan_count} item representation row(s) without a matching snapshot item; migration made no changes. Run `clipmem doctor --verify-invariants` and remove or quarantine confirmed orphan rows before retrying"
        );
    }

    conn.execute_batch(
        r"
        CREATE TABLE item_representations_v20 (
            snapshot_id INTEGER NOT NULL,
            item_index INTEGER NOT NULL CHECK (item_index >= 0),
            uti TEXT NOT NULL,
            kind TEXT NOT NULL,
            byte_len INTEGER NOT NULL CHECK (byte_len >= 0),
            raw_sha256 TEXT NOT NULL,
            text_value TEXT,
            blob_value BLOB NOT NULL,
            image_compression_status TEXT NOT NULL DEFAULT 'uncompressed' CHECK (image_compression_status IN ('uncompressed', 'compressed', 'skipped')),
            image_compression_format TEXT,
            image_compressed_at TEXT,
            image_original_byte_len INTEGER,
            image_original_raw_sha256 TEXT,
            image_compression_reason TEXT,
            PRIMARY KEY (snapshot_id, item_index, uti),
            FOREIGN KEY (snapshot_id, item_index) REFERENCES snapshot_items(snapshot_id, item_index) ON DELETE CASCADE
        );
        INSERT INTO item_representations_v20 (
            snapshot_id, item_index, uti, kind, byte_len, raw_sha256, text_value, blob_value,
            image_compression_status, image_compression_format, image_compressed_at,
            image_original_byte_len, image_original_raw_sha256, image_compression_reason
        ) SELECT snapshot_id, item_index, uti, kind, byte_len, raw_sha256, text_value, blob_value,
            image_compression_status, image_compression_format, image_compressed_at,
            image_original_byte_len, image_original_raw_sha256, image_compression_reason
          FROM item_representations;
        DROP TABLE item_representations;
        ALTER TABLE item_representations_v20 RENAME TO item_representations;
        CREATE INDEX idx_item_representations_image_candidates ON item_representations(kind, raw_sha256) WHERE length(blob_value) > 0;
        CREATE INDEX idx_item_representations_raw_sha256_snapshot ON item_representations(raw_sha256, snapshot_id);
        "
    )
    .context("rebuild item_representations with composite foreign key")?;
    Ok(())
}

pub(super) fn secure_fts_indexes(conn: &Connection) -> Result<()> {
    for table in [
        "snapshots_fts",
        "snapshots_literal_fts",
        "snapshot_file_url_fts",
        "snapshot_ocr_fts",
        "snapshot_ocr_literal_fts",
        "snapshot_search_documents_fts",
        "snapshot_search_documents_literal_fts",
    ] {
        // Names are a fixed application-owned list, never user input.
        conn.execute(
            &format!("INSERT INTO {table}({table}, rank) VALUES ('secure-delete', 1)"),
            [],
        )
        .with_context(|| format!("enable secure deletion for {table}"))?;
        conn.execute(
            &format!("INSERT INTO {table}({table}) VALUES ('rebuild')"),
            [],
        )
        .with_context(|| format!("remove legacy deletion tombstones from {table}"))?;
    }
    Ok(())
}

pub(in crate::db) fn legacy_prerelease_schema_detected(conn: &Connection) -> Result<bool> {
    let mut stmt = conn
        .prepare("PRAGMA table_info(item_representations)")
        .context("prepare PRAGMA table_info(item_representations)")?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .context("read item_representations columns")?;
    let columns = super::collect_rows(rows).context("collect item_representations columns")?;
    if columns.is_empty() {
        return Ok(false);
    }

    let has_kind = columns.iter().any(|column| column == "kind");
    let has_legacy_marker = super::LEGACY_PRERELEASE_COLUMNS
        .iter()
        .any(|legacy| columns.iter().any(|column| column == legacy));
    Ok(!has_kind || has_legacy_marker)
}
