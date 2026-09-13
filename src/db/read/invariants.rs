use crate::model::DoctorIntegrityReport;
use anyhow::{Context, Result};
use rusqlite::Connection;

pub(super) fn verify(conn: &Connection) -> Result<DoctorIntegrityReport> {
    let count = |sql: &str, context: &str| -> Result<usize> {
        Ok(conn
            .query_row(sql, [], |row| row.get::<_, i64>(0))
            .with_context(|| context.to_string())? as usize)
    };
    let orphans = count(
        "SELECT COUNT(*) FROM item_representations ir LEFT JOIN snapshot_items si ON si.snapshot_id=ir.snapshot_id AND si.item_index=ir.item_index WHERE si.snapshot_id IS NULL",
        "count orphan representations",
    )?;
    let foreign_keys = count(
        "SELECT COUNT(*) FROM pragma_foreign_key_check",
        "count foreign key violations",
    )?;
    let projection_mismatches = count(
        r"SELECT COUNT(*) FROM snapshots s
          LEFT JOIN snapshot_search_documents d ON d.snapshot_id=s.id
          LEFT JOIN snapshot_stats ss ON ss.snapshot_id=s.id
          LEFT JOIN snapshot_projection_cache p ON p.snapshot_id=d.snapshot_id
          LEFT JOIN snapshot_event_filter_cache e ON e.snapshot_id=d.snapshot_id
          LEFT JOIN snapshot_ocr_cache o ON o.snapshot_id=d.snapshot_id
          WHERE d.snapshot_id IS NULL
             OR ss.snapshot_id IS NULL
             OR d.builder_version != 4
             OR d.has_native_text != (trim(d.native_text) != '')
             OR d.has_ocr_text != (trim(d.ocr_text) != '')
             OR d.has_url != (trim(d.url_text) != '')
             OR d.has_file_url != (trim(d.file_path_text) != '')
             OR d.preview_text != COALESCE(s.preview_text,'')
             OR d.ocr_text != COALESCE(o.ocr_text,'')
             OR (COALESCE(p.urls,'') != ''
                 AND d.url_text != p.urls
                 AND substr(d.url_text, 1, length(p.urls) + 1) != (p.urls || char(10)))
             OR d.file_path_text != COALESCE(p.file_urls,'')
             OR d.historical_app_names != COALESCE(e.app_names_lower,'')
             OR d.historical_app_bundle_ids != COALESCE(e.bundle_ids_lower,'')
             OR d.last_observed_at != ss.last_observed_at
             OR d.last_event_id != ss.last_event_id",
        "shadow compare canonical and rollback projections",
    )?;
    Ok(DoctorIntegrityReport::new(
        orphans,
        foreign_keys,
        projection_mismatches,
    ))
}
