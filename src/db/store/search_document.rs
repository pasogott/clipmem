use anyhow::{Context, Result};
use rusqlite::Connection;
use std::collections::{BTreeMap, HashSet};

use crate::model::{build_item, build_representation, FlattenedTextProjection};

pub(in crate::db) const SNAPSHOT_DOCUMENT_BUILDER_VERSION: i64 = 4;

pub(in crate::db) fn needs_current_builder_migration(version: i64) -> bool {
    version <= 24
}

pub(in crate::db) fn rebuild_snapshot_search_document(
    conn: &Connection,
    snapshot_id: i64,
) -> Result<()> {
    let projection = load_text_projection(conn, snapshot_id)?;
    let mut native_text = projection
        .text_fragments()
        .iter()
        .map(|fragment| fragment.text())
        .collect::<Vec<_>>()
        .join("\n\n");
    if native_text.is_empty() {
        native_text = conn
            .query_row(
                "SELECT CASE WHEN snapshot_kind IN ('plain_text','html','json','xml','rtf') \
                 THEN search_text ELSE '' END FROM snapshots WHERE id = ?1",
                [snapshot_id],
                |row| row.get(0),
            )
            .context("load legacy canonical text fallback")?;
    }
    let has_native_text = !native_text.trim().is_empty();
    let legacy_urls: String = conn
        .query_row(
            "SELECT COALESCE((SELECT urls FROM snapshot_projection_cache WHERE snapshot_id = ?1), '')",
            [snapshot_id],
            |row| row.get(0),
        )
        .context("load legacy URL projection")?;
    let merged_urls = merge_url_text(&legacy_urls, projection.urls());
    conn.execute(
        r"
        INSERT INTO snapshot_search_documents (
            snapshot_id, builder_version, native_text, preview_text, ocr_text,
            url_text, file_path_text, historical_app_names,
            historical_app_bundle_ids, has_native_text, has_ocr_text, has_url,
            has_file_url, has_image, has_pdf, last_observed_at, last_event_id
        )
        SELECT
            s.id,
            ?2,
            ?3,
            COALESCE(s.preview_text, ''),
            COALESCE(soc.ocr_text, ''),
            ?5,
            COALESCE(sp.file_urls, ''),
            COALESCE(se.app_names_lower, ''),
            COALESCE(se.bundle_ids_lower, ''),
            ?4,
            COALESCE(soc.ocr_text, '') != '',
            ?5 != '',
            COALESCE(sp.file_urls, '') != '',
            EXISTS (SELECT 1 FROM item_representations ir WHERE ir.snapshot_id = s.id AND ir.kind = 'image'),
            EXISTS (SELECT 1 FROM item_representations ir WHERE ir.snapshot_id = s.id AND ir.kind = 'pdf'),
            ss.last_observed_at,
            ss.last_event_id
        FROM snapshots s
        JOIN snapshot_stats ss ON ss.snapshot_id = s.id
        LEFT JOIN snapshot_projection_cache sp ON sp.snapshot_id = s.id
        LEFT JOIN snapshot_event_filter_cache se ON se.snapshot_id = s.id
        LEFT JOIN snapshot_ocr_cache soc ON soc.snapshot_id = s.id
        WHERE s.id = ?1
        ON CONFLICT(snapshot_id) DO UPDATE SET
            builder_version = excluded.builder_version,
            native_text = excluded.native_text,
            preview_text = excluded.preview_text,
            ocr_text = excluded.ocr_text,
            url_text = excluded.url_text,
            file_path_text = excluded.file_path_text,
            historical_app_names = excluded.historical_app_names,
            historical_app_bundle_ids = excluded.historical_app_bundle_ids,
            has_native_text = excluded.has_native_text,
            has_ocr_text = excluded.has_ocr_text,
            has_url = excluded.has_url,
            has_file_url = excluded.has_file_url,
            has_image = excluded.has_image,
            has_pdf = excluded.has_pdf,
            last_observed_at = excluded.last_observed_at,
            last_event_id = excluded.last_event_id
        ",
        (
            snapshot_id,
            SNAPSHOT_DOCUMENT_BUILDER_VERSION,
            native_text,
            has_native_text,
            merged_urls,
        ),
    )
    .context("build snapshot search document")?;

    conn.execute(
        "DELETE FROM snapshot_search_documents_fts WHERE rowid = ?1",
        [snapshot_id],
    )
    .context("delete old snapshot search FTS row")?;
    conn.execute(
        r"
        INSERT INTO snapshot_search_documents_fts (
            rowid, native_text, preview_text, ocr_text, url_text, file_path_text,
            historical_app_names, historical_app_bundle_ids
        )
        SELECT snapshot_id, native_text, preview_text, ocr_text, url_text,
               file_path_text, historical_app_names, historical_app_bundle_ids
        FROM snapshot_search_documents WHERE snapshot_id = ?1
        ",
        [snapshot_id],
    )
    .context("insert snapshot search FTS row")?;

    conn.execute(
        "DELETE FROM snapshot_search_documents_literal_fts WHERE rowid = ?1",
        [snapshot_id],
    )
    .context("delete old snapshot literal FTS row")?;
    conn.execute(
        r"
        INSERT INTO snapshot_search_documents_literal_fts (rowid, haystack)
        SELECT snapshot_id, lower(
            native_text || char(31) || preview_text || char(31) || ocr_text ||
            char(31) || url_text || char(31) || file_path_text || char(31) ||
            historical_app_names || char(31) || historical_app_bundle_ids
        )
        FROM snapshot_search_documents WHERE snapshot_id = ?1
        ",
        [snapshot_id],
    )
    .context("insert snapshot literal FTS row")?;
    Ok(())
}

fn merge_url_text(native_urls: &str, projected_urls: &[String]) -> String {
    let mut seen = native_urls
        .split('\u{1f}')
        .filter(|url| !url.is_empty())
        .collect::<HashSet<_>>();
    let additions = projected_urls
        .iter()
        .map(String::as_str)
        .filter(|url| seen.insert(url))
        .collect::<Vec<_>>();

    match (native_urls.is_empty(), additions.is_empty()) {
        (_, true) => native_urls.to_string(),
        (true, false) => additions.join("\n"),
        (false, false) => format!("{native_urls}\n{}", additions.join("\n")),
    }
}

fn load_text_projection(conn: &Connection, snapshot_id: i64) -> Result<FlattenedTextProjection> {
    let mut stmt = conn
        .prepare(
            "SELECT item_index, uti, text_value, blob_value FROM item_representations \
             WHERE snapshot_id = ?1 ORDER BY item_index, uti",
        )
        .context("prepare canonical text projection representations")?;
    let rows = stmt
        .query_map([snapshot_id], |row| {
            Ok((
                row.get::<_, i64>(0)? as usize,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Vec<u8>>(3)?,
            ))
        })
        .context("query canonical text projection representations")?;
    let mut grouped = BTreeMap::<usize, Vec<_>>::new();
    for row in rows {
        let (item_index, uti, text, bytes) =
            row.context("read canonical projection representation")?;
        grouped
            .entry(item_index)
            .or_default()
            .push(build_representation(uti, text, bytes));
    }
    let items = grouped
        .into_iter()
        .map(|(index, representations)| build_item(index, representations))
        .collect::<Vec<_>>();
    Ok(FlattenedTextProjection::from_items(&items))
}

pub(in crate::db) fn rebuild_all_snapshot_search_documents(conn: &Connection) -> Result<()> {
    conn.execute("DELETE FROM snapshot_search_documents", [])
        .context("clear snapshot search documents")?;
    conn.execute("DELETE FROM snapshot_search_documents_fts", [])
        .context("clear snapshot search FTS")?;
    conn.execute("DELETE FROM snapshot_search_documents_literal_fts", [])
        .context("clear snapshot literal search FTS")?;

    let mut stmt = conn
        .prepare("SELECT id FROM snapshots ORDER BY id")
        .context("prepare snapshot search document rebuild")?;
    let ids = stmt
        .query_map([], |row| row.get::<_, i64>(0))
        .context("query snapshot ids for search document rebuild")?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("collect snapshot ids for search document rebuild")?;
    drop(stmt);
    for snapshot_id in ids {
        rebuild_snapshot_search_document(conn, snapshot_id)?;
    }
    Ok(())
}
