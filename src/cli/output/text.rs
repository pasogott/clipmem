use std::fmt::Write;

use super::model::{
    CaptureOnceOutput, GetEnvelope, ListEnvelope, ListRow, SettingsIgnoreListOutput, SettingsView,
    SnapshotListRow, StatsEnvelope, TimelineListRow,
};
use super::support::{
    format_utc_timestamp, push_blank_line, push_snapshot_leaderboard,
    push_snapshot_leaderboard_entry,
};
use crate::cli::display::{format_duration_seconds, peak_bucket};
use crate::db::SnapshotMetadata;
use crate::db::{ImageOptimizationReport, StorageCompactReport};
use crate::model::DoctorReport;

pub(in crate::cli) fn render_capture_once_text(output: &CaptureOnceOutput) -> String {
    let mut out = String::new();
    match output {
        CaptureOnceOutput::Stored(output) => {
            let store_state = if output.store.inserted_new_snapshot() {
                "new content"
            } else {
                "already known content"
            };
            let _ = writeln!(
                out,
                "stored snapshot {} via event {} ({store_state})",
                output.store.snapshot_id(),
                output.store.event_id()
            );
            let _ = writeln!(out, "kind: {}", output.snapshot.snapshot_kind());
            let _ = writeln!(out, "bytes: {}", output.snapshot.total_bytes());
            if let Some(app) = output.snapshot.frontmost_app_name() {
                let _ = writeln!(out, "frontmost app: {app}");
            }
            if !output.snapshot.preview_text().is_empty() {
                let _ = writeln!(out, "preview: {}", output.snapshot.preview_text());
            }
        }
        CaptureOnceOutput::Skipped(output) => {
            let _ = writeln!(out, "skipped capture reason={}", output.reason.as_str());
            let _ = writeln!(out, "kind: {}", output.kind);
            let _ = writeln!(out, "bytes: {}", output.total_bytes);
            if let Some(app) = &output.frontmost_app_name {
                let _ = writeln!(out, "frontmost app: {app}");
            }
        }
    }

    out
}

pub(in crate::cli) fn render_list_text(envelope: &ListEnvelope) -> String {
    let mut out = String::new();
    if envelope.command == "search" {
        if let Some(mode_used) = envelope
            .applied_filters
            .get("mode_used")
            .and_then(serde_json::Value::as_str)
        {
            let _ = writeln!(out, "search mode: {mode_used}");
            if !envelope.results.is_empty() {
                out.push('\n');
            }
        }
    }

    for row in &envelope.results {
        match row {
            ListRow::Snapshot(row) => push_snapshot_row_text(&mut out, row),
            ListRow::Timeline(row) => push_timeline_row_text(&mut out, row),
        }
    }

    if let Some(cursor) = &envelope.next_cursor {
        let _ = writeln!(
            out,
            "More results available. Repeat this command with --cursor {cursor}"
        );
    }
    out
}

fn push_snapshot_row_text(out: &mut String, row: &SnapshotListRow) {
    let app = row
        .app_name
        .as_deref()
        .or(row.app_bundle_id.as_deref())
        .unwrap_or("unknown app");

    let _ = writeln!(
        out,
        "[{}] {} · last seen {} · captures {} · {} · {} bytes · {} items",
        row.snapshot_id,
        row.kind,
        format_utc_timestamp(&row.last_seen_at),
        row.capture_count,
        app,
        row.total_bytes,
        row.item_count
    );

    if !row.preview_text.is_empty() {
        let _ = writeln!(out, "  preview: {}", row.preview_text);
    }
    if let Some(why_matched) = row
        .why_matched
        .as_deref()
        .filter(|value| *value != row.preview_text)
    {
        let _ = writeln!(out, "  match:   {why_matched}");
    }
    if !row.projection.urls.is_empty() {
        let _ = writeln!(out, "  urls:    {}", row.projection.urls.join(", "));
    }
    if !row.projection.file_paths.is_empty() {
        let _ = writeln!(out, "  files:   {}", row.projection.file_paths.join(", "));
    }
    if let Some(score) = row.score {
        let _ = writeln!(out, "  score:   {score:.3}");
    }
    push_blank_line(out);
}

fn push_timeline_row_text(out: &mut String, row: &TimelineListRow) {
    let app = row
        .app_name
        .as_deref()
        .or(row.app_bundle_id.as_deref())
        .unwrap_or("unknown app");

    let _ = writeln!(
        out,
        "event {} · snapshot {} · {} · change_count={} · {} · {} bytes",
        row.event_id,
        row.snapshot_id,
        format_utc_timestamp(&row.observed_at),
        row.change_count,
        app,
        row.total_bytes
    );

    if !row.best_text.is_empty() {
        let _ = writeln!(out, "  best:    {}", row.best_text);
    }
    if !row.preview_text.is_empty() && row.preview_text != row.best_text {
        let _ = writeln!(out, "  preview: {}", row.preview_text);
    }
    if !row.projection.urls.is_empty() {
        let _ = writeln!(out, "  urls:    {}", row.projection.urls.join(", "));
    }
    if !row.projection.file_paths.is_empty() {
        let _ = writeln!(out, "  files:   {}", row.projection.file_paths.join(", "));
    }
    push_blank_line(out);
}

pub(in crate::cli) fn render_stats_text(envelope: &StatsEnvelope) -> String {
    let stats = &envelope.stats;
    let mut out = String::new();
    let _ = writeln!(out, "Overview");
    let _ = writeln!(out, "  snapshots: {}", stats.snapshot_count());
    let _ = writeln!(out, "  capture events: {}", stats.capture_event_count());
    let _ = writeln!(out, "  unique apps: {}", stats.unique_app_count());
    let _ = writeln!(out, "  total bytes: {}", stats.total_bytes());
    let _ = writeln!(
        out,
        "  average bytes per snapshot: {:.1}",
        stats.average_bytes_per_snapshot()
    );
    let _ = writeln!(
        out,
        "  average captures per snapshot: {:.2}",
        stats.average_captures_per_snapshot()
    );
    let _ = writeln!(out, "  dedupe ratio: {:.1}%", stats.dedupe_ratio() * 100.0);
    let _ = writeln!(
        out,
        "  observed: {} to {}",
        stats
            .first_observed_at()
            .map_or("none".to_string(), format_utc_timestamp),
        stats
            .last_observed_at()
            .map_or("none".to_string(), format_utc_timestamp)
    );
    let _ = writeln!(
        out,
        "  archive span: {}",
        stats.archive_span_seconds().map_or_else(
            || "0s".to_string(),
            |seconds| { format_duration_seconds(seconds.max(0) as u64) }
        )
    );
    push_blank_line(&mut out);

    let _ = writeln!(out, "Content mix");
    if stats.kind_breakdown().is_empty() {
        let _ = writeln!(out, "  none");
    } else {
        for entry in stats.kind_breakdown() {
            let _ = writeln!(
                out,
                "  {}: {} snapshots, {} bytes",
                entry.kind(),
                entry.snapshot_count(),
                entry.total_bytes()
            );
        }
    }
    push_blank_line(&mut out);

    let _ = writeln!(out, "Top apps");
    if stats.top_apps().is_empty() {
        let _ = writeln!(out, "  none");
    } else {
        for app in stats.top_apps() {
            let _ = writeln!(out, "  {}: {} events", app.app(), app.capture_event_count());
        }
    }
    push_blank_line(&mut out);

    let _ = writeln!(out, "Activity patterns");
    let busiest_hour = peak_bucket(stats.busiest_hours()).map_or_else(
        || "none".to_string(),
        |entry| {
            format!(
                "{}:00 UTC ({} events)",
                entry.bucket(),
                entry.capture_event_count()
            )
        },
    );
    let busiest_weekday = peak_bucket(stats.busiest_weekdays()).map_or_else(
        || "none".to_string(),
        |entry| {
            format!(
                "{} ({} events)",
                entry.bucket(),
                entry.capture_event_count()
            )
        },
    );
    let _ = writeln!(out, "  Busiest hour: {busiest_hour}");
    let _ = writeln!(out, "  Busiest weekday: {busiest_weekday}");
    push_blank_line(&mut out);

    let _ = writeln!(out, "Leaderboards");
    if let Some(snapshot) = stats.most_recopied_snapshot() {
        let _ = writeln!(out, "  Most re-copied snapshot:");
        push_snapshot_leaderboard_entry(&mut out, snapshot, 4);
    } else {
        let _ = writeln!(out, "  Most re-copied snapshot: none");
    }
    let _ = writeln!(out, "  Largest snapshots:");
    push_snapshot_leaderboard(&mut out, stats.largest_snapshots(), 4);
    let _ = writeln!(out, "  Most captured snapshots:");
    push_snapshot_leaderboard(&mut out, stats.most_captured_snapshots(), 4);

    out
}

pub(in crate::cli) fn render_get_text(envelope: &GetEnvelope) -> String {
    render_snapshot_text(&envelope.snapshot)
}

pub(in crate::cli) fn render_snapshot_text(snapshot: &SnapshotMetadata) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "snapshot {} · sha256={} · kind={} · captures={}",
        snapshot.snapshot_id(),
        snapshot.sha256(),
        snapshot.snapshot_kind(),
        snapshot.capture_count()
    );
    let _ = writeln!(
        out,
        "first seen: {} · last seen: {}",
        format_utc_timestamp(snapshot.first_observed_at()),
        format_utc_timestamp(snapshot.last_observed_at())
    );
    let _ = writeln!(
        out,
        "bytes: {} · items: {}",
        snapshot.total_bytes(),
        snapshot.item_count()
    );
    if let Some(app) = snapshot
        .last_frontmost_app_name()
        .or(snapshot.last_frontmost_app_bundle_id())
    {
        let _ = writeln!(out, "last frontmost app: {app}");
    }
    if !snapshot.best_text().is_empty() {
        let _ = writeln!(out, "best: {}", snapshot.best_text());
    }
    if !snapshot.preview_text().is_empty() {
        let _ = writeln!(out, "preview: {}", snapshot.preview_text());
    }
    if !snapshot.urls().is_empty() {
        let _ = writeln!(out, "urls: {}", snapshot.urls().join(", "));
    }
    if !snapshot.file_paths().is_empty() {
        let _ = writeln!(out, "files: {}", snapshot.file_paths().join(", "));
    }
    if let Some(ocr_text) = snapshot.ocr_text() {
        let _ = writeln!(out, "ocr: {ocr_text}");
    }
    if let Some(ocr_status) = snapshot.ocr_status() {
        let _ = writeln!(out, "ocr status: {ocr_status}");
    }
    push_blank_line(&mut out);

    for item in snapshot.items() {
        let _ = writeln!(
            out,
            "item {} · kind={} · bytes={}{}",
            item.item_index(),
            item.primary_kind(),
            item.total_bytes(),
            item.primary_uti()
                .map(|uti| format!(" · primary type={uti}"))
                .unwrap_or_default()
        );
        if !item.preview_text().is_empty() {
            let _ = writeln!(out, "  preview: {}", item.preview_text());
        }
        for rep in item.representations() {
            let _ = writeln!(
                out,
                "  - {} · kind={} · {} bytes · sha256={}",
                rep.uti(),
                rep.kind(),
                rep.byte_len(),
                rep.raw_sha256()
            );
            if let Some(text) = rep.text_value() {
                let _ = writeln!(out, "    text: {text}");
            }
        }
        push_blank_line(&mut out);
    }

    if !snapshot.recent_events().is_empty() {
        let _ = writeln!(out, "recent events:");
        for event in snapshot.recent_events() {
            let source = event
                .frontmost_app_name()
                .or(event.frontmost_app_bundle_id())
                .unwrap_or("unknown app");
            let _ = writeln!(
                out,
                "  event {} · {} · change_count={} · {}",
                event.event_id(),
                format_utc_timestamp(event.observed_at()),
                event.change_count(),
                source
            );
        }
    }

    out
}

pub(in crate::cli) fn render_doctor_text(report: &DoctorReport) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "database: {}", report.db_path());
    let _ = writeln!(out, "sqlite version: {}", report.sqlite_version());
    let _ = writeln!(out, "journal mode: {}", report.journal_mode());
    let _ = writeln!(
        out,
        "fts5 compile option present: {}",
        report.fts5_compile_option_present()
    );
    let _ = writeln!(
        out,
        "fts5 temp table creation works: {}",
        report.fts5_create_virtual_table_ok()
    );
    if let Some(integrity) = report.integrity() {
        let _ = writeln!(
            out,
            "orphan representations: {}",
            integrity.orphan_representation_count()
        );
        let _ = writeln!(
            out,
            "foreign key violations: {}",
            integrity.foreign_key_violation_count()
        );
        let _ = writeln!(
            out,
            "projection mismatches: {}",
            integrity.projection_mismatch_count()
        );
    } else {
        let _ = writeln!(out, "integrity checks: skipped");
    }

    if !report.compile_options().is_empty() {
        out.push('\n');
        let _ = writeln!(out, "compile options:");
        for opt in report.compile_options() {
            let _ = writeln!(out, "  {opt}");
        }
    }

    out
}

pub(in crate::cli) fn render_storage_compact_text(report: &StorageCompactReport) -> String {
    let action = if report.dry_run {
        "storage compact dry-run"
    } else {
        "storage compacted"
    };
    format!(
        "{} db={} before={} after={} reclaimed={} estimated_reclaimable={} page_count={} freelist_count={} checkpoint_busy={} checkpoint_log={} checkpointed={}\n",
        action,
        report.db_path,
        report.total_before_bytes,
        report.total_after_bytes,
        report.reclaimed_bytes,
        report.estimated_reclaimable_bytes,
        report.page_count,
        report.freelist_count,
        report.checkpoint.busy,
        report.checkpoint.log,
        report.checkpoint.checkpointed
    )
}

pub(in crate::cli) fn render_image_optimization_text(report: &ImageOptimizationReport) -> String {
    let action = if report.dry_run {
        "image optimization dry-run"
    } else if report.compact_error.is_some() {
        "image optimization complete; database compaction failed"
    } else {
        "image optimization complete"
    };
    let recommendation = if report.compact_recommended {
        " Run `clipmem storage compact` to return freed pages to the filesystem."
    } else {
        ""
    };
    let compact_error = report
        .compact_error
        .as_ref()
        .map(|error| format!(" compact_error={error}"))
        .unwrap_or_default();
    format!(
        "{} format={} scanned={} compressed={} skipped={} conflicts={} original_bytes={} optimized_bytes={} logical_saved_bytes={} compact_run={} filesystem_saved_bytes={} filesystem_growth_bytes={}{}{}\n",
        action,
        report.format,
        report.scanned_rows,
        report.compressed_rows,
        report.skipped_rows,
        report.conflict_count,
        report.original_bytes,
        report.optimized_bytes,
        report.logical_saved_bytes,
        report.compact_run,
        report.filesystem_saved_bytes,
        report.filesystem_growth_bytes,
        compact_error,
        recommendation
    )
}

pub(in crate::cli) fn render_settings_view_text(view: &SettingsView) -> String {
    let mut out = String::new();
    out.push_str(&format!("paused: {}\n", view.paused));
    out.push_str(&format!(
        "api key filter: {}\n",
        view.api_key_filter_enabled
    ));
    out.push_str(&format!("ocr: {}\n", view.ocr_enabled));
    out.push_str(&format!("retention: {}\n", view.retention));
    out.push_str(&format!(
        "ignored bundle ids: {}\n",
        view.ignored_bundle_ids.len()
    ));
    for bundle_id in &view.ignored_bundle_ids {
        out.push_str(&format!("  - {bundle_id}\n"));
    }
    out
}

pub(in crate::cli) fn render_settings_ignore_list_text(
    output: &SettingsIgnoreListOutput,
) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "ignored bundle ids: {}\n",
        output.ignored_bundle_ids.len()
    ));
    for bundle_id in &output.ignored_bundle_ids {
        out.push_str(&format!("  - {bundle_id}\n"));
    }
    out
}
