use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension, TransactionBehavior};

use crate::db::core::clamp_result_limit;
use crate::db::sqlite_helpers::{collect_rows, row_usize, usize_to_i64};
use crate::db::types::{
    ArchiveChangeKind, Database, OcrCandidate, OcrCandidateSummary, OcrResultRecord,
    OcrStatusReport,
};

use super::jobs::{
    active_lease_for_key, complete_job_tx, enqueue_job_tx, fail_job_tx, reset_failed_job_tx,
    JobLease, OCR_ALGORITHM_VERSION, OCR_JOB_KIND,
};
use super::revision::bump_revision_tx;

const MAX_OCR_CLAIM_REFILL_ROUNDS: usize = 1_024;

impl OcrCandidate {
    fn new(
        raw_sha256: String,
        blob_value: Vec<u8>,
        snapshot_count: usize,
        lease: JobLease,
    ) -> Self {
        Self {
            raw_sha256,
            blob_value,
            snapshot_count,
            lease,
        }
    }

    pub(crate) fn raw_sha256(&self) -> &str {
        &self.raw_sha256
    }

    pub(crate) fn blob_value(&self) -> &[u8] {
        &self.blob_value
    }
}

impl Database {
    pub(crate) fn with_ocr_lease_renewal<T>(
        &self,
        candidate: &OcrCandidate,
        operation: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        if self.path == std::path::Path::new(":memory:") {
            return operation();
        }
        let path = self.path.clone();
        let lease = candidate.lease.clone();
        let (stop, receiver) = std::sync::mpsc::sync_channel(1);
        let heartbeat = std::thread::Builder::new()
            .name("clipmem-ocr-lease".into())
            .spawn(move || -> Result<()> {
                loop {
                    match receiver.recv_timeout(std::time::Duration::from_secs(30)) {
                        Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                            return Ok(())
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                            let mut db = Database::open_read_write_current(&path)?;
                            if !db.renew_job(&lease)? {
                                anyhow::bail!("OCR lease ownership was lost during recognition");
                            }
                        }
                    }
                }
            })
            .context("start OCR lease renewal")?;
        let result = operation();
        let _ = stop.send(());
        heartbeat
            .join()
            .map_err(|_| anyhow::anyhow!("OCR lease renewal thread panicked"))??;
        result
    }

    pub(crate) fn store_ocr_candidate_text(
        &mut self,
        candidate: &OcrCandidate,
        engine: &str,
        recognition_level: &str,
        text: &str,
    ) -> Result<()> {
        self.store_claimed_ocr_text(
            &candidate.lease,
            candidate.raw_sha256(),
            engine,
            recognition_level,
            text,
        )
    }

    pub(crate) fn store_ocr_candidate_failure(
        &mut self,
        candidate: &OcrCandidate,
        engine: &str,
        recognition_level: &str,
        error: &str,
    ) -> Result<()> {
        self.store_claimed_ocr_failure(
            &candidate.lease,
            candidate.raw_sha256(),
            engine,
            recognition_level,
            error,
        )
    }

    #[allow(dead_code)]
    pub(crate) fn enqueue_ocr_for_snapshot(&mut self, snapshot_id: i64) -> Result<usize> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("begin ocr enqueue transaction")?;
        let inserted = enqueue_ocr_for_snapshot_tx(&tx, snapshot_id)?;
        rebuild_snapshot_ocr_cache(&tx, snapshot_id)?;
        if inserted != 0 {
            bump_revision_tx(&tx, &[ArchiveChangeKind::Ocr])?;
        }
        tx.commit().context("commit ocr enqueue transaction")?;
        Ok(inserted)
    }

    pub(crate) fn next_ocr_candidates(
        &mut self,
        limit: usize,
        snapshot_id: Option<i64>,
        retry_failed: bool,
    ) -> Result<Vec<OcrCandidate>> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("begin ocr candidate transaction")?;

        let pruned = prune_orphan_ocr_results_tx(&tx)?;
        let enqueued = enqueue_ocr_candidates_tx(&tx, snapshot_id)?;
        let requeued = if retry_failed {
            tx.execute(
                r"
                    UPDATE ocr_results
                    SET status = 'pending',
                        error = NULL,
                        updated_at = CURRENT_TIMESTAMP
                    WHERE status = 'failed'
                      AND (
                          ?1 IS NULL
                          OR EXISTS (
                              SELECT 1
                              FROM item_representations ir
                              WHERE ir.raw_sha256 = ocr_results.raw_sha256
                                AND ir.snapshot_id = ?1
                          )
                      )
                ",
                [snapshot_id],
            )
            .context("requeue failed ocr results")?
        } else {
            0
        };

        enqueue_ocr_jobs_tx(&tx, snapshot_id)?;
        if retry_failed {
            let mut stmt = tx.prepare(
                r"
                    SELECT j.dedupe_key FROM jobs j
                    WHERE j.kind = 'ocr'
                      AND j.algorithm_version = 'vision-v1'
                      AND j.state IN ('queued', 'failed')
                      AND (?1 IS NULL OR EXISTS (
                          SELECT 1 FROM item_representations ir
                          WHERE ir.raw_sha256 = j.dedupe_key AND ir.snapshot_id = ?1
                      ))
                ",
            )?;
            let hashes =
                collect_rows(stmt.query_map([snapshot_id], |row| row.get::<_, String>(0))?)?;
            drop(stmt);
            for hash in hashes {
                reset_failed_job_tx(&tx, OCR_JOB_KIND, &hash, OCR_ALGORITHM_VERSION)?;
            }
        }
        if pruned != 0 || enqueued != 0 || requeued != 0 {
            bump_revision_tx(&tx, &[ArchiveChangeKind::Ocr])?;
        }
        tx.commit().context("commit ocr candidate transaction")?;
        self.claim_pending_ocr_candidates(limit, snapshot_id)
    }

    pub(crate) fn claim_pending_ocr_candidates(
        &mut self,
        limit: usize,
        snapshot_id: Option<i64>,
    ) -> Result<Vec<OcrCandidate>> {
        let requested = clamp_result_limit(limit);
        let owner = format!("ocr-{}", std::process::id());
        let mut candidates = Vec::with_capacity(requested);
        for _ in 0..MAX_OCR_CLAIM_REFILL_ROUNDS {
            let remaining = requested.saturating_sub(candidates.len());
            if remaining == 0 {
                break;
            }
            let leases = self.claim_ocr_jobs_for_snapshot(&owner, remaining, snapshot_id)?;
            if leases.is_empty() {
                break;
            }
            for lease in leases {
                if let Some(candidate) = load_ocr_candidate(&self.conn, lease.clone(), snapshot_id)?
                {
                    candidates.push(candidate);
                } else {
                    self.skip_claimed_job(&lease, "source_missing")?;
                }
            }
        }
        Ok(candidates)
    }

    #[allow(dead_code)]
    pub(crate) fn store_ocr_text(
        &mut self,
        raw_sha256: &str,
        engine: &str,
        recognition_level: &str,
        text: &str,
    ) -> Result<()> {
        let lease =
            active_lease_for_key(&self.conn, OCR_JOB_KIND, raw_sha256, OCR_ALGORITHM_VERSION)?
                .context("OCR result has no active durable job lease")?;
        self.store_claimed_ocr_text(&lease, raw_sha256, engine, recognition_level, text)
    }

    fn store_claimed_ocr_text(
        &mut self,
        lease: &JobLease,
        raw_sha256: &str,
        engine: &str,
        recognition_level: &str,
        text: &str,
    ) -> Result<()> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("begin ocr result transaction")?;
        let status = if text.trim().is_empty() {
            "skipped"
        } else {
            "ready"
        };
        let changed = tx
            .execute(
                r"
                UPDATE ocr_results
                SET status = ?2,
                    engine = ?3,
                    recognition_level = ?4,
                    text_value = ?5,
                    error = NULL,
                    attempt_count = attempt_count + 1,
                    updated_at = CURRENT_TIMESTAMP
                WHERE raw_sha256 = ?1
            ",
                params![raw_sha256, status, engine, recognition_level, text.trim()],
            )
            .context("store ocr text result")?;
        rebuild_snapshot_ocr_cache_for_hash(&tx, raw_sha256)?;
        complete_job_tx(
            &tx,
            lease,
            if status == "ready" {
                "succeeded"
            } else {
                "skipped"
            },
            Some(raw_sha256),
        )?;
        if changed != 0 {
            bump_revision_tx(&tx, &[ArchiveChangeKind::Ocr])?;
        }
        tx.commit().context("commit ocr result transaction")?;
        Ok(())
    }

    #[allow(dead_code)]
    pub(crate) fn store_ocr_failure(
        &mut self,
        raw_sha256: &str,
        engine: &str,
        recognition_level: &str,
        error: &str,
    ) -> Result<()> {
        let lease =
            active_lease_for_key(&self.conn, OCR_JOB_KIND, raw_sha256, OCR_ALGORITHM_VERSION)?
                .context("OCR failure has no active durable job lease")?;
        self.store_claimed_ocr_failure(&lease, raw_sha256, engine, recognition_level, error)
    }

    fn store_claimed_ocr_failure(
        &mut self,
        lease: &JobLease,
        raw_sha256: &str,
        engine: &str,
        recognition_level: &str,
        error: &str,
    ) -> Result<()> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("begin ocr failure transaction")?;
        fail_job_tx(&tx, lease, error)?;
        let changed = tx
            .execute(
                r"
                UPDATE ocr_results
                SET status = 'failed',
                    engine = ?2,
                    recognition_level = ?3,
                    text_value = NULL,
                    error = ?4,
                    attempt_count = attempt_count + 1,
                    updated_at = CURRENT_TIMESTAMP
                WHERE raw_sha256 = ?1
            ",
                params![raw_sha256, engine, recognition_level, error],
            )
            .context("store ocr failure result")?;
        rebuild_snapshot_ocr_cache_for_hash(&tx, raw_sha256)?;
        if changed != 0 {
            bump_revision_tx(&tx, &[ArchiveChangeKind::Ocr])?;
        }
        tx.commit().context("commit ocr failure transaction")?;
        Ok(())
    }

    pub(crate) fn ocr_status_report(&self) -> Result<OcrStatusReport> {
        self.conn
            .query_row(
                r"
                    SELECT
                        (SELECT COUNT(*) FROM ocr_results WHERE status = 'pending'),
                        (SELECT COUNT(*) FROM ocr_results WHERE status = 'ready'),
                        (SELECT COUNT(*) FROM ocr_results WHERE status = 'failed'),
                        (SELECT COUNT(*) FROM ocr_results WHERE status = 'skipped'),
                        (
                            SELECT COUNT(*)
                            FROM snapshot_ocr_cache
                            WHERE ocr_text != ''
                        )
                ",
                [],
                |row| {
                    Ok(OcrStatusReport::new(
                        row_usize(row, 0)?,
                        row_usize(row, 1)?,
                        row_usize(row, 2)?,
                        row_usize(row, 3)?,
                        row_usize(row, 4)?,
                    ))
                },
            )
            .context("load ocr status report")
    }

    pub(crate) fn snapshot_has_outstanding_ocr(&self, snapshot_id: i64) -> Result<bool> {
        self.conn
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM jobs j
             WHERE j.kind = ?1 AND j.algorithm_version = ?2
               AND j.state IN ('queued', 'leased')
               AND EXISTS (SELECT 1 FROM item_representations ir
                   WHERE ir.snapshot_id = ?3 AND ir.raw_sha256 = j.dedupe_key))",
                params![OCR_JOB_KIND, OCR_ALGORITHM_VERSION, snapshot_id],
                |row| row.get(0),
            )
            .context("check captured snapshot OCR completion")
    }

    pub(crate) fn ocr_candidate_summaries(
        &self,
        limit: usize,
        snapshot_id: Option<i64>,
    ) -> Result<Vec<OcrCandidateSummary>> {
        let limit = usize_to_i64(clamp_result_limit(limit))?;
        let mut stmt = self
            .conn
            .prepare(
                r"
                    WITH candidate_hashes AS (
                        SELECT o.raw_sha256, o.updated_at
                        FROM ocr_results o
                        WHERE o.status = 'pending'
                          AND EXISTS (
                              SELECT 1
                              FROM item_representations ir
                              WHERE ir.raw_sha256 = o.raw_sha256
                                AND ir.kind = 'image'
                                AND length(ir.blob_value) > 0
                          )
                          AND (
                              ?1 IS NULL
                              OR EXISTS (
                                  SELECT 1
                                  FROM item_representations ir
                                  WHERE ir.raw_sha256 = o.raw_sha256
                                    AND ir.snapshot_id = ?1
                              )
                          )
                        ORDER BY o.updated_at ASC, o.raw_sha256 ASC
                        LIMIT ?2
                    )
                    SELECT
                        c.raw_sha256,
                        COALESCE((
                            SELECT MAX(ir.byte_len)
                            FROM item_representations ir
                            WHERE ir.raw_sha256 = c.raw_sha256
                        ), 0) AS byte_len,
                        (
                            SELECT COUNT(DISTINCT ir.snapshot_id)
                            FROM item_representations ir
                            WHERE ir.raw_sha256 = c.raw_sha256
                        ) AS snapshot_count,
                        c.updated_at
                    FROM candidate_hashes c
                ",
            )
            .context("prepare ocr candidate summary query")?;
        let rows = stmt
            .query_map(params![snapshot_id, limit], |row| {
                Ok(OcrCandidateSummary::new(
                    row.get(0)?,
                    row_usize(row, 1)?,
                    row_usize(row, 2)?,
                    row.get(3)?,
                ))
            })
            .context("execute ocr candidate summary query")?;
        collect_rows(rows).context("collect ocr candidate summaries")
    }

    pub(crate) fn ocr_result(&self, raw_sha256: &str) -> Result<Option<OcrResultRecord>> {
        self.conn
            .query_row(
                r"
                    SELECT
                        o.raw_sha256,
                        o.status,
                        o.engine,
                        o.recognition_level,
                        o.text_value,
                        o.error,
                        o.attempt_count,
                        o.updated_at,
                        (
                            SELECT COUNT(DISTINCT ir.snapshot_id)
                            FROM item_representations ir
                            WHERE ir.raw_sha256 = o.raw_sha256
                        ) AS snapshot_count
                    FROM ocr_results o
                    WHERE o.raw_sha256 = ?1
                ",
                [raw_sha256],
                |row| {
                    Ok(OcrResultRecord::new(
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row_usize(row, 6)?,
                        row.get(7)?,
                        row_usize(row, 8)?,
                    ))
                },
            )
            .optional()
            .context("load ocr result")
    }

    pub(crate) fn clear_ocr_result(&mut self, raw_sha256: &str) -> Result<bool> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("begin ocr clear transaction")?;
        let changed = tx
            .execute(
                "DELETE FROM ocr_results WHERE raw_sha256 = ?1",
                [raw_sha256],
            )
            .context("delete ocr result")?;
        tx.execute(
            "DELETE FROM jobs WHERE kind = ?1 AND dedupe_key = ?2 AND algorithm_version = ?3",
            params![OCR_JOB_KIND, raw_sha256, OCR_ALGORITHM_VERSION],
        )
        .context("delete matching durable OCR job")?;
        rebuild_snapshot_ocr_cache_for_hash(&tx, raw_sha256)?;
        if changed != 0 {
            bump_revision_tx(&tx, &[ArchiveChangeKind::Ocr])?;
        }
        tx.commit().context("commit ocr clear transaction")?;
        Ok(changed != 0)
    }
}

pub(in crate::db) fn enqueue_ocr_candidates_tx(
    tx: &rusqlite::Transaction<'_>,
    snapshot_id: Option<i64>,
) -> Result<usize> {
    let inserted = tx
        .execute(
            r"
            INSERT INTO ocr_results (raw_sha256, status)
            SELECT DISTINCT ir.raw_sha256, 'pending'
            FROM item_representations ir
            WHERE ir.kind = 'image'
              AND length(ir.blob_value) > 0
              AND (?1 IS NULL OR ir.snapshot_id = ?1)
            ON CONFLICT(raw_sha256) DO NOTHING
        ",
            [snapshot_id],
        )
        .context("enqueue ocr candidates")?;
    enqueue_ocr_jobs_tx(tx, snapshot_id)?;
    Ok(inserted)
}

pub(in crate::db) fn enqueue_ocr_jobs_tx(
    tx: &rusqlite::Transaction<'_>,
    snapshot_id: Option<i64>,
) -> Result<usize> {
    let mut stmt = tx.prepare(
        r"
            SELECT DISTINCT o.raw_sha256
            FROM ocr_results o
            WHERE o.status = 'pending'
              AND EXISTS (
                  SELECT 1 FROM item_representations ir
                  WHERE ir.raw_sha256 = o.raw_sha256
                    AND ir.kind = 'image'
                    AND length(ir.blob_value) > 0
                    AND (?1 IS NULL OR ir.snapshot_id = ?1)
              )
            ORDER BY o.raw_sha256
        ",
    )?;
    let hashes = collect_rows(stmt.query_map([snapshot_id], |row| row.get::<_, String>(0))?)?;
    drop(stmt);
    let mut inserted = 0;
    for hash in hashes {
        inserted += enqueue_job_tx(
            tx,
            OCR_JOB_KIND,
            &hash,
            OCR_ALGORITHM_VERSION,
            &format!(r#"{{"raw_sha256":"{hash}"}}"#),
            3,
        )?;
    }
    Ok(inserted)
}

fn load_ocr_candidate(
    conn: &rusqlite::Connection,
    lease: JobLease,
    snapshot_id: Option<i64>,
) -> Result<Option<OcrCandidate>> {
    conn.query_row(
        r"
            SELECT
                ir.blob_value,
                (SELECT COUNT(DISTINCT all_ir.snapshot_id)
                 FROM item_representations all_ir
                 WHERE all_ir.raw_sha256 = ?1)
            FROM item_representations ir
            WHERE ir.raw_sha256 = ?1
              AND ir.kind = 'image'
              AND length(ir.blob_value) > 0
              AND (?2 IS NULL OR ir.snapshot_id = ?2)
            ORDER BY ir.byte_len DESC
            LIMIT 1
        ",
        params![lease.dedupe_key(), snapshot_id],
        |row| {
            Ok(OcrCandidate::new(
                lease.dedupe_key().to_string(),
                row.get(0)?,
                row_usize(row, 1)?,
                lease.clone(),
            ))
        },
    )
    .optional()
    .context("load claimed OCR payload")
}

pub(in crate::db) fn prune_orphan_ocr_results_tx(tx: &rusqlite::Transaction<'_>) -> Result<usize> {
    tx.execute(
        r"
            DELETE FROM ocr_results
            WHERE NOT EXISTS (
                SELECT 1
                FROM item_representations ir
                WHERE ir.raw_sha256 = ocr_results.raw_sha256
                  AND ir.kind = 'image'
                  AND length(ir.blob_value) > 0
            )
        ",
        [],
    )
    .context("prune orphan ocr results")
}

pub(in crate::db) fn enqueue_ocr_for_snapshot_tx(
    tx: &rusqlite::Transaction<'_>,
    snapshot_id: i64,
) -> Result<usize> {
    let inserted = enqueue_ocr_candidates_tx(tx, Some(snapshot_id))?;
    enqueue_ocr_jobs_tx(tx, Some(snapshot_id))?;
    Ok(inserted)
}

pub(in crate::db) fn rebuild_snapshot_ocr_cache_for_hash(
    tx: &rusqlite::Transaction<'_>,
    raw_sha256: &str,
) -> Result<()> {
    let mut stmt = tx
        .prepare(
            r"
                SELECT DISTINCT snapshot_id
                FROM item_representations
                WHERE raw_sha256 = ?1
            ",
        )
        .context("prepare affected ocr snapshot query")?;
    let rows = stmt
        .query_map([raw_sha256], |row| row.get::<_, i64>(0))
        .context("execute affected ocr snapshot query")?;
    let snapshot_ids = collect_rows(rows).context("collect affected ocr snapshots")?;
    drop(stmt);
    for snapshot_id in snapshot_ids {
        rebuild_snapshot_ocr_cache(tx, snapshot_id)?;
    }
    Ok(())
}

pub(in crate::db) fn rebuild_snapshot_ocr_cache(
    tx: &rusqlite::Transaction<'_>,
    snapshot_id: i64,
) -> Result<()> {
    tx.execute(
        r"
            INSERT INTO snapshot_ocr_cache (snapshot_id, ocr_text, status, updated_at)
            SELECT
                s.id,
                COALESCE((
                        SELECT GROUP_CONCAT(text_value, char(10) || char(10))
                    FROM (
                        SELECT DISTINCT o.text_value AS text_value
                        FROM item_representations ir
                        JOIN ocr_results o ON o.raw_sha256 = ir.raw_sha256
                        WHERE ir.snapshot_id = s.id
                          AND o.status = 'ready'
                          AND o.text_value IS NOT NULL
                          AND o.text_value != ''
                        ORDER BY o.text_value
                    )
                ), '') AS ocr_text,
                CASE
                    WHEN EXISTS (
                        SELECT 1
                        FROM item_representations ir
                        JOIN ocr_results o ON o.raw_sha256 = ir.raw_sha256
                        WHERE ir.snapshot_id = s.id
                          AND o.status = 'ready'
                          AND o.text_value IS NOT NULL
                          AND o.text_value != ''
                    ) THEN 'ready'
                    WHEN EXISTS (
                        SELECT 1
                        FROM item_representations ir
                        JOIN ocr_results o ON o.raw_sha256 = ir.raw_sha256
                        WHERE ir.snapshot_id = s.id
                          AND o.status = 'pending'
                    ) THEN 'pending'
                    WHEN EXISTS (
                        SELECT 1
                        FROM item_representations ir
                        JOIN ocr_results o ON o.raw_sha256 = ir.raw_sha256
                        WHERE ir.snapshot_id = s.id
                          AND o.status = 'failed'
                    ) THEN 'failed'
                    ELSE 'skipped'
                END AS status,
                CURRENT_TIMESTAMP
            FROM snapshots s
            WHERE s.id = ?1
            ON CONFLICT(snapshot_id) DO UPDATE SET
                ocr_text = excluded.ocr_text,
                status = excluded.status,
                updated_at = excluded.updated_at
        ",
        [snapshot_id],
    )
    .context("rebuild snapshot ocr cache")?;
    super::search_document::rebuild_snapshot_search_document(tx, snapshot_id)?;
    Ok(())
}
