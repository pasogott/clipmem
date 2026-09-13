use std::io::Cursor;
use std::path::Path;

use anyhow::{Context, Result};
use image::{DynamicImage, ExtendedColorType, ImageDecoder, ImageEncoder, ImageReader, Limits};
use rusqlite::{params, TransactionBehavior};

use super::config::{
    ImageOptimizationCandidate, IMAGE_OPTIMIZATION_FORMAT, IMAGE_OPTIMIZATION_MAX_DIMENSION,
    IMAGE_PREVIEW_ENCODER_VERSION, IMAGE_PREVIEW_LONG_EDGE, IMAGE_PREVIEW_MAX_SOURCE_BYTES,
    IMAGE_PREVIEW_OPTIONS_HASH, WEBP_UTI,
};
use super::jobs::{
    complete_job_tx, enqueue_image_optimization_jobs, load_claimed_image_candidate, JobLease,
    IMAGE_OPTIMIZATION_JOB_KIND,
};
use super::revision::bump_revision_tx;
use crate::db::core::{clamp_result_limit, pragma_usize, storage_file_sizes};
use crate::db::sqlite_helpers::{collect_rows, row_usize, usize_to_i64};
use crate::db::types::{
    ArchiveChangeKind, Database, ImageOptimizationCandidateSummary, ImageOptimizationProgressEvent,
    ImageOptimizationReport, ImagePreviewStatus,
};

#[derive(Debug, Clone)]
pub(in crate::db) struct OptimizedImage {
    pub(in crate::db) bytes: Vec<u8>,
    pub(in crate::db) width: u32,
    pub(in crate::db) height: u32,
}

const IMAGE_OPTIMIZATION_BATCH_LIMIT: usize = 1;

impl Database {
    pub(crate) fn image_preview_status(&self) -> Result<ImagePreviewStatus> {
        self.conn
            .query_row(
                "SELECT COALESCE(SUM(status = 'ready'), 0), COALESCE(SUM(status != 'ready'), 0), COALESCE(SUM(byte_len), 0) FROM representation_derivatives WHERE derivative_kind = 'preview'",
                [],
                |row| Ok(ImagePreviewStatus {
                    ready: row_usize(row, 0)?,
                    skipped: row_usize(row, 1)?,
                    bytes: row_usize(row, 2)?,
                }),
            )
            .context("load image preview status")
    }

    pub(crate) fn optimize_images(
        &mut self,
        dry_run: bool,
        limit: Option<usize>,
        auto_compact: bool,
    ) -> Result<ImageOptimizationReport> {
        self.optimize_images_with_progress(dry_run, limit, auto_compact, |_| Ok(()))
    }

    pub(crate) fn optimize_images_with_progress(
        &mut self,
        dry_run: bool,
        limit: Option<usize>,
        auto_compact: bool,
        mut progress: impl FnMut(ImageOptimizationProgressEvent) -> Result<()>,
    ) -> Result<ImageOptimizationReport> {
        let initial_file_bytes = if database_path_is_file_backed(&self.path) {
            Some(storage_file_sizes(&self.path)?.total_bytes())
        } else {
            None
        };
        let limit = limit.map(clamp_result_limit);
        let total_rows = count_image_optimization_candidates(&self.conn, limit)?;
        let mut report = ImageOptimizationReport {
            dry_run,
            format: IMAGE_OPTIMIZATION_FORMAT,
            scanned_rows: 0,
            compressed_rows: 0,
            skipped_rows: 0,
            conflict_count: 0,
            original_bytes: 0,
            optimized_bytes: 0,
            logical_saved_bytes: 0,
            compact_run: false,
            compact: None,
            compact_error: None,
            filesystem_saved_bytes: 0,
            filesystem_growth_bytes: 0,
            compact_recommended: false,
        };
        progress(ImageOptimizationProgressEvent::Started { total_rows })?;

        self.process_image_optimization_candidates(
            dry_run,
            total_rows,
            &mut report,
            &mut progress,
        )?;
        self.maybe_compact_after_image_optimization(
            dry_run,
            auto_compact,
            initial_file_bytes,
            total_rows,
            &mut report,
            &mut progress,
        )?;

        progress(ImageOptimizationProgressEvent::Complete {
            report: Box::new(report.clone()),
        })?;
        Ok(report)
    }

    pub(crate) fn image_optimization_candidate_summaries(
        &self,
        limit: usize,
    ) -> Result<Vec<ImageOptimizationCandidateSummary>> {
        load_image_optimization_candidate_summaries(&self.conn, limit)
    }

    fn process_image_optimization_candidates(
        &mut self,
        dry_run: bool,
        total_rows: usize,
        report: &mut ImageOptimizationReport,
        progress: &mut impl FnMut(ImageOptimizationProgressEvent) -> Result<()>,
    ) -> Result<()> {
        if !dry_run {
            enqueue_image_optimization_jobs(&mut self.conn, total_rows)?;
        }
        let mut dry_run_offset = 0;
        while report.scanned_rows < total_rows {
            let batch_limit =
                (total_rows - report.scanned_rows).min(IMAGE_OPTIMIZATION_BATCH_LIMIT);
            let batch_offset = if dry_run { dry_run_offset } else { 0 };
            let claimed;
            let candidates = if dry_run {
                load_image_optimization_candidate_batch(&self.conn, batch_limit, batch_offset)?
                    .into_iter()
                    .map(|candidate| (candidate, None))
                    .collect()
            } else {
                claimed = self.claim_jobs(
                    IMAGE_OPTIMIZATION_JOB_KIND,
                    &format!("image-optimize-{}", std::process::id()),
                    batch_limit,
                )?;
                let mut loaded = Vec::with_capacity(claimed.len());
                for lease in claimed {
                    if let Some(candidate) = load_claimed_image_candidate(&self.conn, &lease)? {
                        loaded.push((candidate, Some(lease)));
                    } else {
                        self.skip_claimed_job(&lease, "source_missing_or_derivative_exists")?;
                    }
                }
                loaded
            };
            if candidates.is_empty() {
                break;
            }

            if dry_run {
                dry_run_offset += candidates.len();
            }

            for (candidate, lease) in candidates {
                process_image_optimization_candidate(
                    &mut self.conn,
                    dry_run,
                    &candidate,
                    lease.as_ref(),
                    report,
                )?;
                Self::emit_image_optimization_scan_progress(progress, report, total_rows)?;
            }
        }
        Ok(())
    }

    fn maybe_compact_after_image_optimization(
        &mut self,
        dry_run: bool,
        auto_compact: bool,
        initial_file_bytes: Option<u64>,
        total_rows: usize,
        report: &mut ImageOptimizationReport,
        progress: &mut impl FnMut(ImageOptimizationProgressEvent) -> Result<()>,
    ) -> Result<()> {
        if dry_run {
            return Ok(());
        }

        if auto_compact && database_path_is_file_backed(&self.path) {
            let compact_wanted = report.compact_recommended || storage_compaction_would_help(self)?;
            if compact_wanted {
                self.compact_after_image_optimization(
                    initial_file_bytes,
                    total_rows,
                    report,
                    progress,
                )?;
            } else {
                self.assign_current_image_optimization_filesystem_delta(
                    initial_file_bytes,
                    report,
                )?;
            }
        } else if report.compact_recommended && !auto_compact {
            self.assign_current_image_optimization_filesystem_delta(initial_file_bytes, report)?;
        }
        Ok(())
    }

    fn compact_after_image_optimization(
        &mut self,
        initial_file_bytes: Option<u64>,
        total_rows: usize,
        report: &mut ImageOptimizationReport,
        progress: &mut impl FnMut(ImageOptimizationProgressEvent) -> Result<()>,
    ) -> Result<()> {
        report.compact_run = true;
        progress(ImageOptimizationProgressEvent::Compacting {
            scanned_rows: report.scanned_rows,
            total_rows,
            compressed_rows: report.compressed_rows,
            skipped_rows: report.skipped_rows,
            conflict_count: report.conflict_count,
        })?;

        match self.compact_storage(false) {
            Ok(compact) => {
                assign_image_optimization_filesystem_delta(
                    report,
                    initial_file_bytes,
                    compact.total_after_bytes,
                );
                report.compact_recommended = false;
                report.compact = Some(compact);
            }
            Err(error) => {
                report.compact_error = Some(format_compaction_error(&error));
                report.compact_recommended = true;
            }
        }
        Ok(())
    }

    fn assign_current_image_optimization_filesystem_delta(
        &self,
        initial_file_bytes: Option<u64>,
        report: &mut ImageOptimizationReport,
    ) -> Result<()> {
        if let Some(initial) = initial_file_bytes {
            let final_bytes = storage_file_sizes(&self.path)?.total_bytes();
            assign_image_optimization_filesystem_delta(report, Some(initial), final_bytes);
        }
        Ok(())
    }

    fn emit_image_optimization_scan_progress(
        progress: &mut impl FnMut(ImageOptimizationProgressEvent) -> Result<()>,
        report: &ImageOptimizationReport,
        total_rows: usize,
    ) -> Result<()> {
        progress(ImageOptimizationProgressEvent::Scanning {
            scanned_rows: report.scanned_rows,
            total_rows,
            compressed_rows: report.compressed_rows,
            skipped_rows: report.skipped_rows,
            conflict_count: report.conflict_count,
        })
    }
}

fn process_image_optimization_candidate(
    conn: &mut rusqlite::Connection,
    dry_run: bool,
    candidate: &ImageOptimizationCandidate,
    lease: Option<&JobLease>,
    report: &mut ImageOptimizationReport,
) -> Result<()> {
    report.scanned_rows += 1;
    let optimized = match encode_candidate_as_lossless_webp(candidate) {
        Ok(optimized) => optimized,
        Err(reason) => {
            record_image_optimization_skip(conn, dry_run, candidate, lease, report, reason, false)?;
            return Ok(());
        }
    };

    let saved_bytes = 0;
    report.compressed_rows += 1;
    report.original_bytes += candidate.byte_len;
    report.optimized_bytes += optimized.bytes.len();
    report.logical_saved_bytes += saved_bytes;

    if !dry_run {
        store_image_preview_derivative(conn, candidate, optimized, lease)?;
    }
    Ok(())
}

fn record_image_optimization_skip(
    conn: &mut rusqlite::Connection,
    dry_run: bool,
    candidate: &ImageOptimizationCandidate,
    lease: Option<&JobLease>,
    report: &mut ImageOptimizationReport,
    reason: &str,
    is_conflict: bool,
) -> Result<()> {
    report.skipped_rows += 1;
    if is_conflict {
        report.conflict_count += 1;
    }
    if !dry_run {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        record_image_preview_failure(&tx, candidate, reason)?;
        if let Some(lease) = lease {
            complete_job_tx(&tx, lease, "skipped", Some(reason))?;
        }
        bump_revision_tx(&tx, &[ArchiveChangeKind::Storage])?;
        tx.commit()?;
    }
    Ok(())
}

fn assign_image_optimization_filesystem_delta(
    report: &mut ImageOptimizationReport,
    initial_bytes: Option<u64>,
    final_bytes: u64,
) {
    if let Some(initial) = initial_bytes {
        report.filesystem_saved_bytes = initial.saturating_sub(final_bytes);
        report.filesystem_growth_bytes = final_bytes.saturating_sub(initial);
    }
}

pub(in crate::db) fn count_image_optimization_candidates(
    conn: &rusqlite::Connection,
    limit: Option<usize>,
) -> Result<usize> {
    let count = conn
        .query_row(
            r"
                SELECT COUNT(DISTINCT raw_sha256)
                FROM item_representations ir
                WHERE kind = 'image' AND length(blob_value) > 0
                  AND NOT EXISTS (
                    SELECT 1 FROM representation_derivatives rd
                    WHERE rd.source_raw_sha256 = ir.raw_sha256
                      AND rd.derivative_kind = 'preview'
                      AND rd.encoder_version = ?1
                      AND rd.encoder_options_hash = ?2
                  )
            ",
            params![IMAGE_PREVIEW_ENCODER_VERSION, IMAGE_PREVIEW_OPTIONS_HASH],
            |row| row_usize(row, 0),
        )
        .context("count image optimization candidates")?;
    Ok(limit.map_or(count, |limit| count.min(limit)))
}

pub(in crate::db) fn load_image_optimization_candidate_batch(
    conn: &rusqlite::Connection,
    limit: usize,
    offset: usize,
) -> Result<Vec<ImageOptimizationCandidate>> {
    let limit = usize_to_i64(clamp_result_limit(limit))?;
    let offset = usize_to_i64(offset)?;
    let mut stmt = conn
        .prepare(
            r"
                WITH candidates AS MATERIALIZED (
                SELECT snapshot_id, item_index, uti
                FROM item_representations ir
                WHERE kind = 'image' AND length(blob_value) > 0
                  AND NOT EXISTS (
                    SELECT 1 FROM representation_derivatives rd
                    WHERE rd.source_raw_sha256 = ir.raw_sha256
                      AND rd.derivative_kind = 'preview'
                      AND rd.encoder_version = ?3
                      AND rd.encoder_options_hash = ?4
                  )
                GROUP BY raw_sha256
                ORDER BY byte_len DESC, snapshot_id ASC, item_index ASC, uti ASC
                LIMIT ?1
                OFFSET ?2
                )
                SELECT ir.snapshot_id, ir.item_index, ir.uti, ir.byte_len, ir.raw_sha256,
                       CASE WHEN length(ir.blob_value) <= ?5 THEN ir.blob_value ELSE X'' END
                FROM candidates c JOIN item_representations ir
                  ON ir.snapshot_id = c.snapshot_id AND ir.item_index = c.item_index AND ir.uti = c.uti
            ",
        )
        .context("prepare image optimization candidate query")?;
    let rows = stmt
        .query_map(
            params![
                limit,
                offset,
                IMAGE_PREVIEW_ENCODER_VERSION,
                IMAGE_PREVIEW_OPTIONS_HASH,
                IMAGE_PREVIEW_MAX_SOURCE_BYTES as i64
            ],
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
        .context("execute image optimization candidate query")?;
    collect_rows(rows).context("collect image optimization candidates")
}

fn load_image_optimization_candidate_summaries(
    conn: &rusqlite::Connection,
    limit: usize,
) -> Result<Vec<ImageOptimizationCandidateSummary>> {
    let limit = usize_to_i64(clamp_result_limit(limit))?;
    let mut stmt = conn
        .prepare(
            r"
                SELECT snapshot_id, item_index, uti, byte_len, raw_sha256
                FROM item_representations ir
                WHERE kind = 'image' AND length(blob_value) > 0
                  AND NOT EXISTS (
                    SELECT 1 FROM representation_derivatives rd
                    WHERE rd.source_raw_sha256 = ir.raw_sha256
                      AND rd.derivative_kind = 'preview'
                      AND rd.encoder_version = ?2
                      AND rd.encoder_options_hash = ?3
                  )
                GROUP BY raw_sha256
                ORDER BY byte_len DESC, snapshot_id ASC, item_index ASC, uti ASC
                LIMIT ?1
            ",
        )
        .context("prepare image optimization candidate summary query")?;
    let rows = stmt
        .query_map(
            params![
                limit,
                IMAGE_PREVIEW_ENCODER_VERSION,
                IMAGE_PREVIEW_OPTIONS_HASH
            ],
            |row| {
                Ok(ImageOptimizationCandidateSummary::new(
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row_usize(row, 3)?,
                    row.get(4)?,
                ))
            },
        )
        .context("execute image optimization candidate summary query")?;
    collect_rows(rows).context("collect image optimization candidate summaries")
}

pub(in crate::db) fn database_path_is_file_backed(path: &Path) -> bool {
    path != Path::new(":memory:")
}

pub(in crate::db) fn storage_compaction_would_help(db: &Database) -> Result<bool> {
    let freelist_count = pragma_usize(&db.conn, "freelist_count")?;
    Ok(freelist_count > 0)
}

pub(in crate::db) fn format_compaction_error(error: &anyhow::Error) -> String {
    let message = error.to_string();
    let lower = message.to_ascii_lowercase();
    if lower.contains("busy") || lower.contains("locked") {
        format!("{message}; retry after stopping capture or after the database is no longer busy")
    } else {
        message
    }
}

pub(in crate::db) fn encode_candidate_as_lossless_webp(
    candidate: &ImageOptimizationCandidate,
) -> std::result::Result<OptimizedImage, &'static str> {
    if candidate.byte_len > IMAGE_PREVIEW_MAX_SOURCE_BYTES {
        return Err("source_exceeds_memory_limit");
    }
    if candidate_looks_animated_or_unsupported(candidate) {
        return Err("animated_or_unsupported");
    }

    let mut reader = ImageReader::new(Cursor::new(&candidate.blob_value))
        .with_guessed_format()
        .map_err(|_| "unsupported")?;
    let mut limits = Limits::default();
    limits.max_alloc = Some(128 * 1024 * 1024);
    limits.max_image_width = Some(IMAGE_OPTIMIZATION_MAX_DIMENSION);
    limits.max_image_height = Some(IMAGE_OPTIMIZATION_MAX_DIMENSION);
    reader.limits(limits);
    let mut decoder = reader
        .into_decoder()
        .map_err(|_| "corrupt_or_unsupported")?;
    let orientation = decoder
        .orientation()
        .map_err(|_| "corrupt_or_unsupported")?;
    if decoder.total_bytes() > 128 * 1024 * 1024 {
        return Err("decoded_image_exceeds_memory_limit");
    }
    let mut image = DynamicImage::from_decoder(decoder).map_err(|_| "corrupt_or_unsupported")?;
    image.apply_orientation(orientation);
    let image =
        if image.width() > IMAGE_PREVIEW_LONG_EDGE || image.height() > IMAGE_PREVIEW_LONG_EDGE {
            image.thumbnail(IMAGE_PREVIEW_LONG_EDGE, IMAGE_PREVIEW_LONG_EDGE)
        } else {
            image
        };
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    let mut bytes = Vec::new();
    image::codecs::webp::WebPEncoder::new_lossless(&mut bytes)
        .write_image(rgba.as_raw(), width, height, ExtendedColorType::Rgba8)
        .map_err(|_| "encode_failed")?;
    Ok(OptimizedImage {
        bytes,
        width,
        height,
    })
}

pub(in crate::db) fn candidate_looks_animated_or_unsupported(
    candidate: &ImageOptimizationCandidate,
) -> bool {
    let uti = candidate.uti.to_ascii_lowercase();
    if uti.contains("gif") || uti.contains("webp") {
        return true;
    }

    let bytes = candidate.blob_value.as_slice();
    bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") || webp_has_animation_flag(bytes)
}

pub(in crate::db) fn webp_has_animation_flag(bytes: &[u8]) -> bool {
    bytes.len() >= 21
        && &bytes[0..4] == b"RIFF"
        && &bytes[8..12] == b"WEBP"
        && &bytes[12..16] == b"VP8X"
        && (bytes[20] & 0b0000_0010) != 0
}

pub(in crate::db) fn store_image_preview_derivative(
    conn: &mut rusqlite::Connection,
    candidate: &ImageOptimizationCandidate,
    optimized: OptimizedImage,
    lease: Option<&JobLease>,
) -> Result<()> {
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .context("begin image optimization transaction")?;

    tx.execute(
        r"
            INSERT OR IGNORE INTO representation_derivatives (
                source_snapshot_id, source_item_index, source_uti, source_raw_sha256,
                derivative_kind, encoder_version, encoder_options_hash, output_uti, codec,
                blob_value, byte_len, width, height, decoder_metadata, status, verified_at
            ) VALUES (?1, ?2, ?3, ?4, 'preview', ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                      'first_frame;orientation_applied_by_decoder', 'ready', CURRENT_TIMESTAMP)
        ",
        params![
            candidate.snapshot_id,
            candidate.item_index,
            &candidate.uti,
            &candidate.raw_sha256,
            IMAGE_PREVIEW_ENCODER_VERSION,
            IMAGE_PREVIEW_OPTIONS_HASH,
            WEBP_UTI,
            IMAGE_OPTIMIZATION_FORMAT,
            &optimized.bytes,
            usize_to_i64(optimized.bytes.len())?,
            optimized.width,
            optimized.height,
        ],
    )
    .context("store source-safe image preview derivative")?;
    if let Some(lease) = lease {
        complete_job_tx(&tx, lease, "succeeded", Some(&candidate.raw_sha256))?;
    }
    bump_revision_tx(&tx, &[ArchiveChangeKind::Storage])?;

    tx.commit()
        .context("commit image optimization transaction")?;
    Ok(())
}

// Runs inside the caller's IMMEDIATE transaction, which owns the Storage
// revision bump and any job completion for this skip.
fn record_image_preview_failure(
    tx: &rusqlite::Transaction<'_>,
    candidate: &ImageOptimizationCandidate,
    reason: &str,
) -> Result<()> {
    tx.execute(
        "INSERT OR IGNORE INTO representation_derivatives (source_snapshot_id, source_item_index, source_uti, source_raw_sha256, derivative_kind, encoder_version, encoder_options_hash, status, reason) VALUES (?1, ?2, ?3, ?4, 'preview', ?5, ?6, 'skipped', ?7)",
        params![candidate.snapshot_id, candidate.item_index, &candidate.uti, &candidate.raw_sha256, IMAGE_PREVIEW_ENCODER_VERSION, IMAGE_PREVIEW_OPTIONS_HASH, reason],
    ).context("record image preview skip")?;
    Ok(())
}
