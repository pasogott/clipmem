use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension, TransactionBehavior};

use super::ocr::{enqueue_ocr_for_snapshot_tx, rebuild_snapshot_ocr_cache};
use super::rebuild::{
    insert_item, rebuild_snapshot_projection_cache_for_snapshot, set_representation_cache_deferred,
};
use super::revision::bump_revision_tx;
use super::search_document::rebuild_snapshot_search_document;
use crate::db::sqlite_helpers::usize_to_i64;
use crate::db::types::{ArchiveChangeKind, CaptureMode, CaptureOutcome, Database};
use crate::model::{CaptureStoreResult, ClipboardSnapshot};
use crate::sensitive;

impl Database {
    /// Store a captured clipboard snapshot and append a capture event for the observation.
    ///
    /// # Errors
    ///
    /// Returns an error if any transaction step fails, including snapshot, item,
    /// representation, or capture-event inserts, or the final commit.
    pub fn store_capture(&mut self, snapshot: &ClipboardSnapshot) -> Result<CaptureStoreResult> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("begin capture transaction")?;

        let result = store_capture_tx(&tx, snapshot)?;
        tx.commit().context("commit capture transaction")?;
        Ok(result)
    }

    pub(crate) fn capture_with_policy(
        &mut self,
        snapshot: &ClipboardSnapshot,
        _mode: CaptureMode,
    ) -> Result<CaptureOutcome> {
        let db_path = self.path.clone();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("begin capture application transaction")?;
        expire_restore_operations(&tx)?;

        let (paused, api_key_filter_enabled, ocr_enabled): (bool, bool, bool) = tx
            .query_row(
                "SELECT paused, api_key_filter_enabled, ocr_enabled FROM clipmem_settings WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .context("load capture policy")?;
        if paused {
            tx.commit().context("commit paused capture decision")?;
            return Ok(CaptureOutcome::SkippedPaused);
        }
        // Both the frontmost application and the inferred content origin
        // (e.g. a Chromium app writing the clipboard in the background) are
        // policy identities; ignoring either one skips the capture.
        for bundle_id in [
            snapshot.frontmost_app_bundle_id(),
            snapshot.content_origin_bundle_id(),
        ]
        .into_iter()
        .flatten()
        {
            let ignored: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM ignored_bundle_ids WHERE bundle_id = ?1 COLLATE NOCASE)",
                    [bundle_id],
                    |row| row.get(0),
                )
                .context("check ignored application")?;
            if ignored {
                tx.commit().context("commit ignored-app capture decision")?;
                return Ok(CaptureOutcome::SkippedIgnoredApp {
                    bundle_id: bundle_id.to_string(),
                });
            }
        }
        if sensitive::has_private_clipboard_marker(snapshot) {
            tx.commit()
                .context("commit private clipboard marker decision")?;
            return Ok(CaptureOutcome::SkippedPrivateMarker);
        }
        if api_key_filter_enabled && sensitive::should_skip_snapshot_for_api_key_filter(snapshot) {
            tx.commit().context("commit sensitive capture decision")?;
            return Ok(CaptureOutcome::SkippedSensitive);
        }

        let suppressed_operation: Option<String> = tx
            .query_row(
                "SELECT ro.operation_id
                 FROM restore_operations ro
                 JOIN restore_operation_generations rog
                   ON rog.operation_id = ro.operation_id
                 WHERE datetime(ro.expires_at) >= datetime('now')
                   AND ro.state IN ('preparing', 'written', 'failed')
                   AND rog.change_count = ?1
                 ORDER BY ro.created_at DESC, ro.operation_id DESC
                 LIMIT 1",
                [snapshot.change_count()],
                |row| row.get(0),
            )
            .optional()
            .context("check restore suppression operations")?
            .or(super::restore_journal::operation_for_generation(
                &db_path,
                snapshot.change_count(),
            ));
        if let Some(operation_id) = suppressed_operation {
            tx.commit().context("commit restore suppression decision")?;
            return Ok(CaptureOutcome::SuppressedRestore { operation_id });
        }

        let store = store_capture_tx(&tx, snapshot)?;
        let enqueue_ocr = if ocr_enabled {
            let inserted = enqueue_ocr_for_snapshot_tx(&tx, store.snapshot_id())?;
            rebuild_snapshot_ocr_cache(&tx, store.snapshot_id())?;
            if inserted != 0 {
                bump_revision_tx(&tx, &[ArchiveChangeKind::Ocr])?;
            }
            true
        } else {
            false
        };
        let new_snapshot = store.inserted_new_snapshot();
        tx.commit()
            .context("commit capture application transaction")?;

        if new_snapshot {
            Ok(CaptureOutcome::Stored { store, enqueue_ocr })
        } else {
            Ok(CaptureOutcome::ObservedExisting { store, enqueue_ocr })
        }
    }

    #[cfg(test)]
    pub(crate) fn begin_restore_operation(
        &self,
        snapshot_id: i64,
        snapshot_sha256: &str,
        expected_change_count: i64,
    ) -> Result<String> {
        self.conn.execute(
            "UPDATE restore_operations SET state = 'expired' WHERE state IN ('preparing', 'written', 'failed') AND datetime(expires_at) < datetime('now')",
            [],
        ).context("expire restore operations")?;
        let operation_id: String = self
            .conn
            .query_row(
                "INSERT INTO restore_operations (operation_id, snapshot_id, snapshot_sha256, state, expected_change_count, expires_at)
             VALUES (lower(hex(randomblob(16))), ?1, ?2, 'preparing', ?3, datetime('now', '+5 seconds'))
             RETURNING operation_id",
                params![snapshot_id, snapshot_sha256, expected_change_count],
                |row| row.get(0),
            )
            .context("begin restore operation")?;
        self.conn
            .execute(
                "INSERT OR IGNORE INTO restore_operation_generations (operation_id, change_count)
                 VALUES (?1, ?2)",
                params![operation_id, expected_change_count],
            )
            .context("register restore operation generation")?;
        Ok(operation_id)
    }

    #[cfg(test)]
    pub(crate) fn reassign_restore_operation_generation(
        &self,
        operation_id: &str,
        expected_change_count: i64,
    ) -> Result<()> {
        self.conn
            .execute(
                "INSERT OR IGNORE INTO restore_operation_generations (operation_id, change_count)
             VALUES (?1, ?2)",
                params![operation_id, expected_change_count],
            )
            .context("append restore operation generation")?;
        self.conn
            .execute(
                "UPDATE restore_operations SET expected_change_count = ?2,
             expires_at = datetime('now', '+5 seconds') WHERE operation_id = ?1",
                params![operation_id, expected_change_count],
            )
            .context("reassign restore operation generation")?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn mark_restore_written(&self, operation_id: &str, change_count: i64) -> Result<()> {
        self.conn
            .execute(
                "INSERT OR IGNORE INTO restore_operation_generations (operation_id, change_count)
             VALUES (?1, ?2)",
                params![operation_id, change_count],
            )
            .context("register written restore generation")?;
        self.conn
            .execute(
                "UPDATE restore_operations SET state = 'written', result_change_count = ?2,
                 expires_at = datetime('now', '+30 seconds') WHERE operation_id = ?1",
                params![operation_id, change_count],
            )
            .context("mark restore operation written")?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn mark_restore_failed(&self, operation_id: &str, failure: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE restore_operations SET state = 'failed', failure = ?2 WHERE operation_id = ?1",
            params![operation_id, failure],
        ).context("mark restore operation failed")?;
        Ok(())
    }
}

fn store_capture_tx(
    tx: &rusqlite::Transaction<'_>,
    snapshot: &ClipboardSnapshot,
) -> Result<CaptureStoreResult> {
    let inserted_snapshot_id: Option<i64> = tx
        .query_row(
            "INSERT INTO snapshots (
                    id, sha256,
                    snapshot_kind,
                    preview_text,
                    search_text,
                    item_count,
                    total_bytes
                ) VALUES ((SELECT high_water + 1 FROM archive_id_sequences WHERE name = 'snapshots'), ?1, ?2, ?3, ?4, ?5, ?6)
                ON CONFLICT(sha256) DO NOTHING
                RETURNING id",
            params![
                snapshot.fingerprint(),
                snapshot.snapshot_kind().as_str(),
                snapshot.preview_text(),
                snapshot.search_text(),
                usize_to_i64(snapshot.item_count())?,
                usize_to_i64(snapshot.total_bytes())?,
            ],
            |row| row.get(0),
        )
        .optional()
        .context("insert snapshot row")?;

    let snapshot_id = if let Some(id) = inserted_snapshot_id {
        set_representation_cache_deferred(tx, true)?;
        for item in snapshot.items() {
            insert_item(tx, id, item)
                .with_context(|| format!("insert snapshot item {}", item.item_index()))?;
        }
        rebuild_snapshot_projection_cache_for_snapshot(tx, id)?;
        set_representation_cache_deferred(tx, false)?;

        id
    } else {
        tx.query_row(
            "SELECT id FROM snapshots WHERE sha256 = ?1",
            [snapshot.fingerprint()],
            |row| row.get(0),
        )
        .context("lookup existing snapshot by fingerprint")?
    };

    tx.execute(
        "INSERT INTO capture_events (
                id, snapshot_id,
                change_count,
                frontmost_app_bundle_id,
                frontmost_app_name,
                content_origin_bundle_id,
                content_origin_name,
                content_origin_kind
            ) VALUES ((SELECT high_water + 1 FROM archive_id_sequences WHERE name = 'capture_events'), ?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            snapshot_id,
            snapshot.change_count(),
            snapshot.frontmost_app_bundle_id(),
            snapshot.frontmost_app_name(),
            snapshot.content_origin_bundle_id(),
            snapshot.content_origin_name(),
            snapshot.content_origin_kind(),
        ],
    )
    .context("insert capture event row")?;
    let event_id = tx.last_insert_rowid();

    rebuild_snapshot_search_document(tx, snapshot_id)?;

    bump_revision_tx(tx, &[ArchiveChangeKind::ArchiveContent])?;
    Ok(CaptureStoreResult::new(
        snapshot_id,
        event_id,
        inserted_snapshot_id.is_some(),
    ))
}

pub(super) fn expire_restore_operations(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute(
        "UPDATE restore_operations SET state = 'expired'
         WHERE state IN ('preparing', 'written', 'failed') AND datetime(expires_at) < datetime('now')",
        [],
    )
    .context("expire restore operations")?;
    Ok(())
}
