use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{ensure, Context, Result};
use rusqlite::{params, OptionalExtension, Transaction, TransactionBehavior};

use crate::db::sqlite_helpers::{row_usize, usize_to_i64};
use crate::db::types::Database;

use super::config::ImageOptimizationCandidate;

static TOKEN_SEQUENCE: AtomicU64 = AtomicU64::new(1);

pub(in crate::db) const OCR_JOB_KIND: &str = "ocr";
pub(in crate::db) const IMAGE_OPTIMIZATION_JOB_KIND: &str = "image_preview_derivative";
pub(in crate::db) const OCR_ALGORITHM_VERSION: &str = "vision-v1";
pub(in crate::db) const IMAGE_OPTIMIZATION_ALGORITHM_VERSION: &str =
    "webp-lossless-long-edge-2048-v1";
const DEFAULT_LEASE_SECONDS: i64 = 120;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(in crate::db) struct JobLease {
    id: i64,
    dedupe_key: String,
    owner: String,
    token: String,
}

impl JobLease {
    #[cfg(test)]
    pub(in crate::db) fn id(&self) -> i64 {
        self.id
    }

    pub(in crate::db) fn dedupe_key(&self) -> &str {
        &self.dedupe_key
    }
}

#[allow(dead_code)]
pub(in crate::db) fn active_lease_for_key(
    conn: &rusqlite::Connection,
    kind: &str,
    dedupe_key: &str,
    algorithm_version: &str,
) -> Result<Option<JobLease>> {
    conn.query_row(
        r"
            SELECT id, dedupe_key, lease_owner, lease_token
            FROM jobs
            WHERE kind = ?1 AND dedupe_key = ?2 AND algorithm_version = ?3
              AND state = 'leased' AND lease_until > CURRENT_TIMESTAMP
        ",
        params![kind, dedupe_key, algorithm_version],
        |row| {
            Ok(JobLease {
                id: row.get(0)?,
                dedupe_key: row.get(1)?,
                owner: row.get(2)?,
                token: row.get(3)?,
            })
        },
    )
    .optional()
    .context("load active durable job lease")
}

impl Database {
    pub(in crate::db) fn claim_jobs(
        &mut self,
        kind: &str,
        owner: &str,
        limit: usize,
    ) -> Result<Vec<JobLease>> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("begin durable job claim transaction")?;
        let leases = claim_jobs_tx(&tx, kind, owner, limit, DEFAULT_LEASE_SECONDS, None)?;
        tx.commit()
            .context("commit durable job claim transaction")?;
        Ok(leases)
    }

    pub(in crate::db) fn claim_ocr_jobs_for_snapshot(
        &mut self,
        owner: &str,
        limit: usize,
        snapshot_id: Option<i64>,
    ) -> Result<Vec<JobLease>> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("begin scoped OCR job claim transaction")?;
        let leases = claim_jobs_tx(
            &tx,
            OCR_JOB_KIND,
            owner,
            limit,
            DEFAULT_LEASE_SECONDS,
            snapshot_id,
        )?;
        tx.commit()
            .context("commit scoped OCR job claim transaction")?;
        Ok(leases)
    }

    #[allow(dead_code)]
    pub(in crate::db) fn renew_job(&mut self, lease: &JobLease) -> Result<bool> {
        let changed = self
            .conn
            .execute(
                r"
                    UPDATE jobs
                    SET lease_until = datetime('now', '+120 seconds'),
                        updated_at = CURRENT_TIMESTAMP
                    WHERE id = ?1
                      AND state = 'leased'
                      AND lease_owner = ?2
                      AND lease_token = ?3
                      AND lease_until > CURRENT_TIMESTAMP
                ",
                params![lease.id, lease.owner, lease.token],
            )
            .context("renew durable job lease")?;
        Ok(changed == 1)
    }

    pub(in crate::db) fn skip_claimed_job(&mut self, lease: &JobLease, reason: &str) -> Result<()> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("begin durable job skip transaction")?;
        complete_job_tx(&tx, lease, "skipped", Some(reason))?;
        tx.commit().context("commit durable job skip transaction")
    }
}

pub(in crate::db) fn enqueue_job_tx(
    tx: &Transaction<'_>,
    kind: &str,
    dedupe_key: &str,
    algorithm_version: &str,
    source_ref_json: &str,
    max_attempts: usize,
) -> Result<usize> {
    tx.execute(
        r"
            INSERT INTO jobs (
                kind, dedupe_key, algorithm_version, source_ref_json, max_attempts
            ) VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(kind, dedupe_key, algorithm_version) DO NOTHING
        ",
        params![
            kind,
            dedupe_key,
            algorithm_version,
            source_ref_json,
            i64::try_from(max_attempts).context("job max attempts exceeds SQLite range")?
        ],
    )
    .context("enqueue durable job")
}

pub(in crate::db) fn reset_failed_job_tx(
    tx: &Transaction<'_>,
    kind: &str,
    dedupe_key: &str,
    algorithm_version: &str,
) -> Result<usize> {
    tx.execute(
        r"
            UPDATE jobs
            SET state = 'queued',
                not_before = CURRENT_TIMESTAMP,
                lease_owner = NULL,
                lease_token = NULL,
                lease_until = NULL,
                attempts = 0,
                last_error = NULL,
                finished_at = NULL,
                updated_at = CURRENT_TIMESTAMP
            WHERE kind = ?1
              AND dedupe_key = ?2
              AND algorithm_version = ?3
              AND state IN ('failed', 'queued')
        ",
        params![kind, dedupe_key, algorithm_version],
    )
    .context("reset failed durable job")
}

pub(in crate::db) fn complete_job_tx(
    tx: &Transaction<'_>,
    lease: &JobLease,
    state: &str,
    result_ref_json: Option<&str>,
) -> Result<()> {
    ensure!(
        matches!(state, "succeeded" | "skipped"),
        "invalid terminal job state"
    );
    let changed = tx
        .execute(
            r"
                UPDATE jobs
                SET state = ?2,
                    result_ref_json = ?3,
                    lease_owner = NULL,
                    lease_token = NULL,
                    lease_until = NULL,
                    finished_at = CURRENT_TIMESTAMP,
                    updated_at = CURRENT_TIMESTAMP
                WHERE id = ?1
                  AND state = 'leased'
                  AND lease_owner = ?4
                  AND lease_token = ?5
                  AND lease_until > CURRENT_TIMESTAMP
            ",
            params![lease.id, state, result_ref_json, lease.owner, lease.token],
        )
        .context("complete durable job")?;
    ensure!(changed == 1, "job lease was lost before completion");
    refresh_attached_operations_tx(tx, lease.id)?;
    Ok(())
}

pub(in crate::db) fn fail_job_tx(
    tx: &Transaction<'_>,
    lease: &JobLease,
    error: &str,
) -> Result<bool> {
    let terminal = tx
        .query_row(
            "SELECT attempts >= max_attempts FROM jobs WHERE id = ?1",
            [lease.id],
            |row| row.get::<_, bool>(0),
        )
        .context("classify durable job failure")?;
    let changed = tx
        .execute(
            r"
                UPDATE jobs
                SET state = CASE WHEN ?4 THEN 'failed' ELSE 'queued' END,
                    not_before = CASE
                        WHEN ?4 THEN not_before
                        ELSE datetime('now', printf('+%d seconds', MIN(300, 1 << MIN(attempts, 8))))
                    END,
                    last_error = ?2,
                    lease_owner = NULL,
                    lease_token = NULL,
                    lease_until = NULL,
                    finished_at = CASE WHEN ?4 THEN CURRENT_TIMESTAMP ELSE NULL END,
                    updated_at = CURRENT_TIMESTAMP
                WHERE id = ?1
                  AND state = 'leased'
                  AND lease_owner = ?3
                  AND lease_token = ?5
                  AND lease_until > CURRENT_TIMESTAMP
            ",
            params![lease.id, error, lease.owner, terminal, lease.token],
        )
        .context("record durable job failure")?;
    ensure!(changed == 1, "job lease was lost before failure recording");
    refresh_attached_operations_tx(tx, lease.id)?;
    Ok(terminal)
}

fn claim_jobs_tx(
    tx: &Transaction<'_>,
    kind: &str,
    owner: &str,
    limit: usize,
    lease_seconds: i64,
    snapshot_id: Option<i64>,
) -> Result<Vec<JobLease>> {
    terminalize_exhausted_expired_leases_tx(tx, kind)?;
    let mut stmt = tx
        .prepare(
            r"
                SELECT id, dedupe_key
                FROM jobs
                WHERE kind = ?1
                  AND attempts < max_attempts
                  AND (?3 IS NULL OR EXISTS (
                      SELECT 1 FROM item_representations ir
                      WHERE ir.raw_sha256 = jobs.dedupe_key
                        AND ir.snapshot_id = ?3
                  ))
                  AND (
                      (state = 'queued' AND not_before <= CURRENT_TIMESTAMP)
                      OR (state = 'leased' AND lease_until <= CURRENT_TIMESTAMP)
                  )
                ORDER BY priority DESC, not_before ASC, created_at ASC, id ASC
                LIMIT ?2
            ",
        )
        .context("prepare durable job claim query")?;
    let candidates = stmt
        .query_map(
            params![
                kind,
                i64::try_from(limit).context("job claim limit exceeds SQLite range")?,
                snapshot_id
            ],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .context("execute durable job claim query")?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("collect durable job claim candidates")?;
    drop(stmt);

    let mut leases = Vec::with_capacity(candidates.len());
    for (id, dedupe_key) in candidates {
        let token = next_token(owner);
        let modifier = format!("+{lease_seconds} seconds");
        let changed = tx
            .execute(
                r"
                    UPDATE jobs
                    SET state = 'leased',
                        lease_owner = ?2,
                        lease_token = ?3,
                        lease_until = datetime('now', ?4),
                        attempts = attempts + 1,
                        updated_at = CURRENT_TIMESTAMP
                    WHERE id = ?1
                      AND attempts < max_attempts
                      AND (
                          (state = 'queued' AND not_before <= CURRENT_TIMESTAMP)
                          OR (state = 'leased' AND lease_until <= CURRENT_TIMESTAMP)
                      )
                ",
                params![id, owner, token, modifier],
            )
            .context("claim durable job")?;
        if changed == 1 {
            leases.push(JobLease {
                id,
                dedupe_key,
                owner: owner.to_string(),
                token,
            });
        }
    }
    Ok(leases)
}

fn terminalize_exhausted_expired_leases_tx(tx: &Transaction<'_>, kind: &str) -> Result<()> {
    let exhausted_ids = {
        let mut stmt = tx
            .prepare(
                r"
                    SELECT id
                    FROM jobs
                    WHERE kind = ?1
                      AND state = 'leased'
                      AND lease_until <= CURRENT_TIMESTAMP
                      AND attempts >= max_attempts
                    ORDER BY id
                ",
            )
            .context("prepare exhausted lease query")?;
        let collected = stmt
            .query_map([kind], |row| row.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        collected
    };
    for job_id in exhausted_ids {
        tx.execute(
            r"
                UPDATE jobs
                SET state = 'failed',
                    last_error = 'lease expired after final attempt',
                    lease_owner = NULL,
                    lease_token = NULL,
                    lease_until = NULL,
                    finished_at = CURRENT_TIMESTAMP,
                    updated_at = CURRENT_TIMESTAMP
                WHERE id = ?1
                  AND state = 'leased'
                  AND lease_until <= CURRENT_TIMESTAMP
                  AND attempts >= max_attempts
            ",
            [job_id],
        )
        .context("terminalize exhausted expired lease")?;
        if kind == OCR_JOB_KIND {
            let hash: String = tx.query_row(
                "SELECT dedupe_key FROM jobs WHERE id = ?1",
                [job_id],
                |row| row.get(0),
            )?;
            let changed = tx.execute("UPDATE ocr_results SET status = 'failed', error = 'lease expired after final attempt', updated_at = CURRENT_TIMESTAMP WHERE raw_sha256 = ?1 AND status = 'pending'", [&hash])?;
            if changed > 0 {
                super::ocr::rebuild_snapshot_ocr_cache_for_hash(tx, &hash)?;
                super::revision::bump_revision_tx(tx, &[crate::db::types::ArchiveChangeKind::Ocr])?;
            }
        }
        refresh_attached_operations_tx(tx, job_id)?;
    }
    Ok(())
}

fn refresh_attached_operations_tx(tx: &Transaction<'_>, job_id: i64) -> Result<()> {
    tx.execute(
        r"
            UPDATE job_operations
            SET state = CASE
                    WHEN EXISTS (
                        SELECT 1 FROM operation_jobs oj
                        JOIN jobs j ON j.id = oj.job_id
                        WHERE oj.operation_id = job_operations.id
                          AND j.state NOT IN ('succeeded', 'failed', 'skipped', 'cancelled')
                    ) THEN state
                    WHEN EXISTS (
                        SELECT 1 FROM operation_jobs oj
                        JOIN jobs j ON j.id = oj.job_id
                        WHERE oj.operation_id = job_operations.id AND j.state = 'failed'
                    ) THEN 'completed_with_errors'
                    ELSE 'completed'
                END,
                finished_at = CASE
                    WHEN NOT EXISTS (
                        SELECT 1 FROM operation_jobs oj
                        JOIN jobs j ON j.id = oj.job_id
                        WHERE oj.operation_id = job_operations.id
                          AND j.state NOT IN ('succeeded', 'failed', 'skipped', 'cancelled')
                    ) THEN CURRENT_TIMESTAMP
                    ELSE finished_at
                END
            WHERE id IN (SELECT operation_id FROM operation_jobs WHERE job_id = ?1)
        ",
        [job_id],
    )
    .context("refresh durable job operation state")?;
    Ok(())
}

fn next_token(owner: &str) -> String {
    let sequence = TOKEN_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("{owner}:{}:{sequence}", std::process::id())
}

pub(in crate::db) fn enqueue_image_optimization_jobs(
    conn: &mut rusqlite::Connection,
    limit: usize,
) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let rows = {
        let mut stmt = tx.prepare(
            r"
                SELECT MIN(snapshot_id), MIN(item_index), MIN(uti), raw_sha256
                FROM item_representations ir
                WHERE kind = 'image' AND length(blob_value) > 0
                  AND NOT EXISTS (
                    SELECT 1 FROM representation_derivatives rd
                    WHERE rd.source_raw_sha256 = ir.raw_sha256
                      AND rd.derivative_kind = 'preview'
                      AND rd.encoder_version = 1
                      AND rd.encoder_options_hash = 'webp-lossless-long-edge-2048-v1'
                  )
                GROUP BY raw_sha256
                ORDER BY MAX(byte_len) DESC, raw_sha256 LIMIT ?1
            ",
        )?;
        let collected = stmt
            .query_map([usize_to_i64(limit)?], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        collected
    };
    for (snapshot_id, item_index, uti, hash) in rows {
        let key = hash.clone();
        enqueue_job_tx(
            &tx,
            IMAGE_OPTIMIZATION_JOB_KIND,
            &key,
            IMAGE_OPTIMIZATION_ALGORITHM_VERSION,
            &image_source_ref_json(&hash, snapshot_id, item_index, &uti)?,
            3,
        )?;
    }
    tx.commit()?;
    Ok(())
}

fn image_source_ref_json(
    hash: &str,
    snapshot_id: i64,
    item_index: i64,
    uti: &str,
) -> Result<String> {
    serde_json::to_string(&serde_json::json!({
        "source_raw_sha256": hash,
        "snapshot_id": snapshot_id,
        "item_index": item_index,
        "uti": uti,
    }))
    .context("serialize image job source reference")
}

pub(in crate::db) fn load_claimed_image_candidate(
    conn: &rusqlite::Connection,
    lease: &JobLease,
) -> Result<Option<ImageOptimizationCandidate>> {
    let raw_sha256 = lease.dedupe_key();
    conn.query_row(
        r"SELECT snapshot_id, item_index, uti, byte_len, raw_sha256, CASE WHEN length(blob_value) <= ?2 THEN blob_value ELSE X'' END
           FROM item_representations ir
           WHERE raw_sha256 = ?1 AND kind = 'image' AND length(blob_value) > 0
             AND NOT EXISTS (
               SELECT 1 FROM representation_derivatives rd
               WHERE rd.source_raw_sha256 = ir.raw_sha256
                 AND rd.derivative_kind = 'preview'
                 AND rd.encoder_version = 1
                 AND rd.encoder_options_hash = 'webp-lossless-long-edge-2048-v1'
             )
           ORDER BY byte_len DESC, snapshot_id, item_index, uti LIMIT 1",
        rusqlite::params![raw_sha256, super::config::IMAGE_PREVIEW_MAX_SOURCE_BYTES as i64],
        |row| {
            Ok(ImageOptimizationCandidate {
                snapshot_id: row.get(0)?,
                item_index: row.get(1)?,
                uti: row.get(2)?,
                byte_len: row_usize(row, 3)?,
                raw_sha256: row.get(4)?,
                blob_value: row.get(5)?,
            })
        },
    )
    .optional()
    .context("load claimed image optimization payload")
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use anyhow::Result;

    use super::{
        complete_job_tx, enqueue_job_tx, image_source_ref_json, reset_failed_job_tx,
        OCR_ALGORITHM_VERSION, OCR_JOB_KIND,
    };
    use crate::db::Database;

    #[test]
    fn claims_are_exclusive_and_expired_lease_rejects_stale_completion() -> Result<()> {
        let mut db = Database::open_in_memory()?;
        let tx = db.conn.transaction()?;
        enqueue_job_tx(&tx, OCR_JOB_KIND, "hash", OCR_ALGORITHM_VERSION, "{}", 3)?;
        tx.commit()?;

        let first = db.claim_jobs(OCR_JOB_KIND, "worker-a", 1)?.remove(0);
        assert!(db.claim_jobs(OCR_JOB_KIND, "worker-b", 1)?.is_empty());
        db.conn.execute(
            "UPDATE jobs SET lease_until = datetime('now', '-1 second') WHERE id = ?1",
            [first.id()],
        )?;
        let second = db.claim_jobs(OCR_JOB_KIND, "worker-b", 1)?.remove(0);
        let tx = db.conn.transaction()?;
        assert!(complete_job_tx(&tx, &first, "succeeded", None).is_err());
        drop(tx);
        let tx = db.conn.transaction()?;
        complete_job_tx(&tx, &second, "succeeded", None)?;
        tx.commit()?;
        std::thread::sleep(Duration::from_millis(1));
        Ok(())
    }

    #[test]
    fn two_file_backed_connections_claim_disjoint_jobs_deterministically() -> Result<()> {
        let directory = std::env::temp_dir().join(format!(
            "clipmem-two-worker-{}-{}",
            std::process::id(),
            super::TOKEN_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&directory)?;
        let path = directory.join("archive.sqlite3");
        let mut first = Database::open_or_init_and_migrate(&path)?;
        let tx = first.conn.transaction()?;
        enqueue_job_tx(&tx, OCR_JOB_KIND, "a", OCR_ALGORITHM_VERSION, "{}", 3)?;
        enqueue_job_tx(&tx, OCR_JOB_KIND, "b", OCR_ALGORITHM_VERSION, "{}", 3)?;
        tx.commit()?;
        let mut second = Database::open_read_write_current(&path)?;

        let first_claim = first.claim_jobs(OCR_JOB_KIND, "worker-a", 1)?.remove(0);
        let second_claim = second.claim_jobs(OCR_JOB_KIND, "worker-b", 1)?.remove(0);
        assert_eq!(first_claim.dedupe_key(), "a");
        assert_eq!(second_claim.dedupe_key(), "b");
        assert!(first.claim_jobs(OCR_JOB_KIND, "worker-a", 1)?.is_empty());

        drop(second);
        drop(first);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("sqlite3-wal"));
        let _ = std::fs::remove_file(path.with_extension("sqlite3-shm"));
        let _ = std::fs::remove_dir(directory);
        Ok(())
    }

    #[test]
    fn image_source_reference_escapes_arbitrary_uti_text() -> Result<()> {
        let encoded = image_source_ref_json("hash", 1, 2, "public.\"odd\\uti\n")?;
        let value: serde_json::Value = serde_json::from_str(&encoded)?;
        assert_eq!(value["uti"], "public.\"odd\\uti\n");
        Ok(())
    }

    #[test]
    fn expired_final_attempt_becomes_failed_and_can_be_explicitly_requeued() -> Result<()> {
        let mut db = Database::open_in_memory()?;
        let tx = db.conn.transaction()?;
        enqueue_job_tx(&tx, OCR_JOB_KIND, "final", OCR_ALGORITHM_VERSION, "{}", 1)?;
        tx.commit()?;
        let lease = db.claim_jobs(OCR_JOB_KIND, "worker-a", 1)?.remove(0);
        db.conn.execute(
            "UPDATE jobs SET lease_until = datetime('now', '-1 second') WHERE id = ?1",
            [lease.id()],
        )?;

        assert!(db.claim_jobs(OCR_JOB_KIND, "worker-b", 1)?.is_empty());
        let (state, error): (String, Option<String>) = db.conn.query_row(
            "SELECT state, last_error FROM jobs WHERE id = ?1",
            [lease.id()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        assert_eq!(state, "failed");
        assert_eq!(error.as_deref(), Some("lease expired after final attempt"));

        let tx = db.conn.transaction()?;
        reset_failed_job_tx(&tx, OCR_JOB_KIND, "final", OCR_ALGORITHM_VERSION)?;
        tx.commit()?;
        assert_eq!(
            db.claim_jobs(OCR_JOB_KIND, "worker-c", 1)?[0].dedupe_key(),
            "final"
        );
        Ok(())
    }
}
