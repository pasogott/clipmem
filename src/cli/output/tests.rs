use serde_json::{json, Value};
use std::fmt::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::db::{Database, ImageOptimizationReport};
use crate::model::{
    build_item, build_representation, build_snapshot, CaptureContext, CaptureEvent,
    CaptureStoreResult, DoctorReport, SnapshotDetails, SnapshotKind,
};

use super::json::write_json_pretty;
use super::markdown::{render_get_markdown, render_list_markdown, render_recall_markdown};
use super::model::{
    CaptureOnceOutput, CaptureOnceSkippedOutput, CaptureOnceStoredOutput, GetEnvelope,
    ListEnvelope, ListRow, RecallEnvelope, RecallMatchConfidence, RecallOutputRow,
    SettingsIgnoreListOutput, SettingsView, SnapshotListRow, TextProjectionOutput, TimelineListRow,
    OUTPUT_SCHEMA_VERSION,
};
use super::text::{
    render_capture_once_text, render_doctor_text, render_get_text, render_image_optimization_text,
    render_list_text, render_settings_ignore_list_text, render_settings_view_text,
    render_storage_compact_text,
};
use super::toon::{
    render_list_toon, render_recall_toon, ToonSearchRowProjection, ToonTimelineRowProjection,
};

fn text_projection_output(
    best_text_uti: Option<&str>,
    urls: Vec<String>,
    file_paths: Vec<String>,
    text_summary: String,
) -> TextProjectionOutput {
    TextProjectionOutput {
        best_text_uti: best_text_uti.map(ToOwned::to_owned),
        text_fragments: Vec::new(),
        urls,
        file_paths,
        html_text: None,
        rtf_text: None,
        text_summary,
        ocr_text: None,
        ocr_status: None,
        text_projection_version: 3,
        text_projection_diagnostics: Vec::new(),
    }
}

#[test]
pub(in crate::cli) fn render_capture_once_text_reports_summary_lines() {
    let text = render_capture_once_text(&CaptureOnceOutput::Stored(CaptureOnceStoredOutput {
        store: CaptureStoreResult::new(9, 12, true),
        snapshot: build_snapshot(
            CaptureContext::new(3)
                .with_frontmost_app_name("Terminal")
                .with_frontmost_app_bundle_id("com.apple.Terminal"),
            vec![build_item(
                0,
                vec![build_representation(
                    "public.utf8-plain-text".to_string(),
                    None,
                    b"git status".to_vec(),
                )],
            )],
        ),
    }));

    assert!(text.contains("stored snapshot 9 via event 12"));
    assert!(text.contains("kind: plain_text"));
    assert!(text.contains("preview: git status"));
}

#[test]
pub(in crate::cli) fn render_capture_once_text_skips_preview_for_filtered_content() {
    let text = render_capture_once_text(&CaptureOnceOutput::Skipped(CaptureOnceSkippedOutput {
        status: "skipped",
        reason: crate::db::CaptureSkipReason::ApiKeyFilter,
        kind: "plain_text".to_string(),
        total_bytes: 48,
        frontmost_app_name: Some("Terminal".to_string()),
        frontmost_app_bundle_id: Some("com.apple.Terminal".to_string()),
    }));

    assert!(text.contains("skipped capture reason=api_key_filter"));
    assert!(text.contains("kind: plain_text"));
    assert!(!text.contains("preview:"));
}

#[test]
pub(in crate::cli) fn render_list_text_includes_search_mode_match_score_urls_and_files() {
    let text = render_list_text(&ListEnvelope {
        schema_version: OUTPUT_SCHEMA_VERSION,
        command: "search",
        generated_at: "2026-04-17T10:00:00Z".to_string(),
        applied_filters: json!({
            "mode_used": "literal",
        }),
        truncated: false,
        next_cursor: None,
        score_semantics: None,
        results: vec![ListRow::Snapshot(SnapshotListRow {
            snapshot_id: 42,
            event_id: 77,
            sha256: "abc123".to_string(),
            kind: "plain_text".to_string(),
            observed_at: "2026-04-16 11:00:00".to_string(),
            first_seen_at: "2026-04-16 10:00:00".to_string(),
            last_seen_at: "2026-04-16 11:00:00".to_string(),
            app_name: Some("Terminal".to_string()),
            app_bundle_id: Some("com.apple.Terminal".to_string()),
            best_text: "git clone".to_string(),
            projection: text_projection_output(
                None,
                vec!["https://example.com".to_string()],
                vec!["/Users/test/repo".to_string()],
                "git clone".to_string(),
            ),
            preview_text: "git clone".to_string(),
            item_count: 1,
            total_bytes: 9,
            capture_count: 3,
            score: Some(0.125),
            why_matched: Some("⟦git⟧ clone".to_string()),
            matched_fields: vec!["best_text".to_string(), "search_text".to_string()],
            match_evidence: None,
        })],
    });

    assert!(text.contains("search mode: literal"));
    assert!(text.contains("[42] plain_text"));
    assert!(text.contains("match:   ⟦git⟧ clone"));
    assert!(text.contains("urls:    https://example.com"));
    assert!(text.contains("files:   /Users/test/repo"));
    assert!(text.contains("score:   0.125"));
}

#[test]
pub(in crate::cli) fn render_get_text_lists_items_and_events() {
    let text = render_get_text(&GetEnvelope {
        schema_version: OUTPUT_SCHEMA_VERSION,
        command: "get",
        generated_at: "2026-04-17T10:00:00Z".to_string(),
        applied_filters: json!({}),
        snapshot: SnapshotDetails::new(
            7,
            "abc123".to_string(),
            SnapshotKind::PlainText,
            "git push".to_string(),
            "git push".to_string(),
            1,
            8,
            "2026-04-16 10:00:00".to_string(),
            2,
            "2026-04-16 10:00:00".to_string(),
            "2026-04-16 11:00:00".to_string(),
            Some("Terminal".to_string()),
            Some("com.apple.Terminal".to_string()),
            None,
            None,
            vec![CaptureEvent::new(
                21,
                "2026-04-16 11:00:00".to_string(),
                3,
                Some("Terminal".to_string()),
                Some("com.apple.Terminal".to_string()),
            )],
            vec![build_item(
                0,
                vec![build_representation(
                    "public.utf8-plain-text".to_string(),
                    None,
                    b"git push".to_vec(),
                )],
            )],
        )
        .into(),
    });

    assert!(text.contains("snapshot 7"));
    assert!(text.contains("item 0 · kind=plain_text"));
    assert!(text.contains("event 21"));
}

#[test]
pub(in crate::cli) fn render_doctor_text_lists_compile_options() {
    let text = render_doctor_text(&DoctorReport::new(
        "/tmp/clipmem.sqlite3".to_string(),
        "3.46.0".to_string(),
        "wal".to_string(),
        true,
        true,
        vec!["ENABLE_FTS5".to_string()],
        Some(crate::model::DoctorIntegrityReport::new(0, 0, 0)),
    ));

    assert!(text.contains("database: /tmp/clipmem.sqlite3"));
    assert!(text.contains("compile options:"));
    assert!(text.contains("ENABLE_FTS5"));
}

#[test]
pub(in crate::cli) fn render_storage_compact_text_distinguishes_dry_run_from_completed_compaction()
{
    let path = temp_storage_path("compact.sqlite3");
    let mut db = Database::open_or_init(&path).unwrap();
    let dry_run = db.compact_storage(true).unwrap();
    let completed = db.compact_storage(false).unwrap();

    let dry_run_text = render_storage_compact_text(&dry_run);
    let completed_text = render_storage_compact_text(&completed);

    assert!(dry_run_text.starts_with(&format!("storage compact dry-run db={}", path.display())));
    assert!(completed_text.starts_with(&format!("storage compacted db={}", path.display())));
    assert!(completed_text.contains("before="));
    assert!(completed_text.contains("after="));
    assert!(completed_text.contains("checkpoint_busy="));

    cleanup_storage_path(&path);
}

#[test]
pub(in crate::cli) fn render_image_optimization_text_reports_compact_error_and_recommendation() {
    let report = ImageOptimizationReport {
        dry_run: false,
        format: "webp",
        scanned_rows: 5,
        compressed_rows: 3,
        skipped_rows: 2,
        conflict_count: 1,
        original_bytes: 10_000,
        optimized_bytes: 4_000,
        logical_saved_bytes: 6_000,
        compact_run: false,
        compact: None,
        compact_error: Some("database is locked".to_string()),
        filesystem_saved_bytes: 0,
        filesystem_growth_bytes: 128,
        compact_recommended: true,
    };

    let rendered = render_image_optimization_text(&report);

    assert!(rendered.starts_with("image optimization complete; database compaction failed"));
    assert!(rendered.contains("format=webp scanned=5 compressed=3 skipped=2 conflicts=1"));
    assert!(rendered.contains("compact_error=database is locked"));
    assert!(rendered.contains("Run `clipmem storage compact`"));
}

#[test]
pub(in crate::cli) fn render_settings_view_text_lists_policy_and_ignored_bundle_ids() {
    let view = SettingsView {
        paused: true,
        api_key_filter_enabled: false,
        ocr_enabled: true,
        retention_seconds: Some(3_600),
        retention: "1h".to_string(),
        ignored_bundle_ids: vec![
            "com.apple.Terminal".to_string(),
            "com.example.SecretApp".to_string(),
        ],
    };

    let rendered = render_settings_view_text(&view);

    assert_eq!(
        rendered,
        concat!(
            "paused: true\n",
            "api key filter: false\n",
            "ocr: true\n",
            "retention: 1h\n",
            "ignored bundle ids: 2\n",
            "  - com.apple.Terminal\n",
            "  - com.example.SecretApp\n",
        )
    );
}

#[test]
pub(in crate::cli) fn render_settings_ignore_list_text_handles_empty_and_populated_lists() {
    let empty = SettingsIgnoreListOutput {
        ignored_bundle_ids: Vec::new(),
    };
    assert_eq!(
        render_settings_ignore_list_text(&empty),
        "ignored bundle ids: 0\n"
    );

    let populated = SettingsIgnoreListOutput {
        ignored_bundle_ids: vec!["com.apple.Terminal".to_string()],
    };
    assert_eq!(
        render_settings_ignore_list_text(&populated),
        "ignored bundle ids: 1\n  - com.apple.Terminal\n"
    );
}

#[test]
pub(in crate::cli) fn render_timeline_list_text_reports_event_history() {
    let text = render_list_text(&ListEnvelope {
        schema_version: OUTPUT_SCHEMA_VERSION,
        command: "timeline",
        generated_at: "2026-04-17T10:00:00Z".to_string(),
        applied_filters: json!({}),
        truncated: false,
        next_cursor: None,
        score_semantics: None,
        results: vec![ListRow::Timeline(TimelineListRow {
            event_id: 21,
            snapshot_id: 7,
            observed_at: "2026-04-16 11:00:00".to_string(),
            change_count: 3,
            kind: "plain_text".to_string(),
            app_name: Some("Terminal".to_string()),
            app_bundle_id: Some("com.apple.Terminal".to_string()),
            best_text: "git push".to_string(),
            projection: text_projection_output(
                None,
                vec!["https://example.com".to_string()],
                vec!["/Users/test/repo".to_string()],
                "git push".to_string(),
            ),
            preview_text: "git push".to_string(),
            total_bytes: 8,
            sha256: "abc123".to_string(),
            item_count: 1,
        })],
    });

    assert!(text.contains("event 21"));
    assert!(text.contains("change_count=3"));
    assert!(text.contains("best:    git push"));
}

#[test]
pub(in crate::cli) fn markdown_and_toon_render_envelopes() {
    let row = ListRow::Snapshot(SnapshotListRow {
        snapshot_id: 1,
        event_id: 2,
        sha256: "abc".to_string(),
        kind: "plain_text".to_string(),
        observed_at: "2026-04-17T10:00:00Z".to_string(),
        first_seen_at: "2026-04-17T09:00:00Z".to_string(),
        last_seen_at: "2026-04-17T10:00:00Z".to_string(),
        app_name: Some("Terminal".to_string()),
        app_bundle_id: Some("com.apple.Terminal".to_string()),
        best_text: "git status".to_string(),
        projection: text_projection_output(
            Some("public.utf8-plain-text"),
            vec!["https://example.com".to_string()],
            Vec::new(),
            "git status".to_string(),
        ),
        preview_text: "git status".to_string(),
        item_count: 1,
        total_bytes: 10,
        capture_count: 1,
        score: Some(0.3),
        why_matched: Some("git status".to_string()),
        matched_fields: vec!["best_text".to_string(), "search_text".to_string()],
        match_evidence: None,
    });
    let row_json = serde_json::to_value(&row).expect("serialize snapshot row");
    assert_eq!(row_json["best_text_uti"], "public.utf8-plain-text");
    assert_eq!(row_json["urls"], json!(["https://example.com"]));
    assert_eq!(row_json["snapshot_id"], 1);
    assert!(row_json.get("projection").is_none());

    let envelope = ListEnvelope {
        schema_version: OUTPUT_SCHEMA_VERSION,
        command: "search",
        generated_at: "2026-04-17T10:00:00Z".to_string(),
        applied_filters: json!({
            "query": "git",
            "requested_mode": "auto",
            "mode_used": "literal",
            "apps": ["Terminal", "Safari"],
            "window": {
                "hours": 24,
            },
            "flags": [
                {
                    "name": "prefer_recent",
                    "enabled": true,
                }
            ],
            "limit": 10,
            "cursor": null,
        }),
        truncated: false,
        next_cursor: None,
        score_semantics: None,
        results: vec![row],
    };
    let markdown = render_list_markdown(&envelope);
    let toon = render_list_toon(&envelope);

    assert!(markdown.contains("| snapshot_id | kind | observed_at |"));
    assert!(toon.contains(
            "results[1\t]{snapshot_id\tevent_id\tobserved_at\tfirst_seen_at\tlast_seen_at\tkind\tapp_name\tapp_bundle_id\tdisplay_text\tcapture_count\titem_count\ttotal_bytes\tscore\twhy_matched}:"
        ));
    assert!(toon.contains("git status"));
    assert!(!toon.contains("sha256"));
    assert!(!toon.contains("text_fragments"));
    assert!(toon.contains("apps[2\t]: Terminal\tSafari"));
    assert!(toon.contains("window:"));
    assert!(toon.contains("hours: 24"));
    assert!(toon.contains("flags[1]:"));
    assert!(toon.contains("name: prefer_recent"));
}

#[test]
pub(in crate::cli) fn get_markdown_renders_summary_items_and_events() {
    let markdown = render_get_markdown(&GetEnvelope {
        schema_version: OUTPUT_SCHEMA_VERSION,
        command: "get",
        generated_at: "2026-04-17T10:00:00Z".to_string(),
        applied_filters: json!({}),
        snapshot: SnapshotDetails::new(
            7,
            "abc123".to_string(),
            SnapshotKind::PlainText,
            "git push".to_string(),
            "git push".to_string(),
            1,
            8,
            "2026-04-16 10:00:00".to_string(),
            2,
            "2026-04-16 10:00:00".to_string(),
            "2026-04-16 11:00:00".to_string(),
            Some("Terminal".to_string()),
            Some("com.apple.Terminal".to_string()),
            Some("OCR invoice total | 42".to_string()),
            Some("ready".to_string()),
            vec![CaptureEvent::new(
                21,
                "2026-04-16 11:00:00".to_string(),
                3,
                Some("Terminal".to_string()),
                Some("com.apple.Terminal".to_string()),
            )],
            vec![build_item(
                0,
                vec![build_representation(
                    "public.utf8-plain-text".to_string(),
                    None,
                    b"git push".to_vec(),
                )],
            )],
        )
        .into(),
    });

    assert!(markdown.contains("## Items"));
    assert!(markdown.contains("## Recent Events"));
    assert!(markdown.contains("Snapshot: 7"));
    assert!(markdown.contains("- OCR: OCR invoice total \\| 42"));
    assert!(markdown.contains("- OCR status: ready"));
}

#[test]
pub(in crate::cli) fn recall_markdown_and_toon_render_best_match_and_alternatives() {
    let envelope = RecallEnvelope {
        schema_version: OUTPUT_SCHEMA_VERSION,
        command: "recall",
        generated_at: "2026-04-17T10:00:00Z".to_string(),
        applied_filters: json!({
            "limit": 5,
            "prefer_recent": true,
        }),
        query: Some("git".to_string()),
        best_candidate: RecallOutputRow {
            snapshot_id: 1,
            event_id: 2,
            sha256: "abc".to_string(),
            kind: "plain_text".to_string(),
            observed_at: "2026-04-17T10:00:00Z".to_string(),
            first_seen_at: "2026-04-17T09:00:00Z".to_string(),
            last_seen_at: "2026-04-17T10:00:00Z".to_string(),
            app_name: Some("Terminal".to_string()),
            app_bundle_id: Some("com.apple.Terminal".to_string()),
            best_text: "".to_string(),
            projection: text_projection_output(
                Some("public.utf8-plain-text"),
                vec!["https://example.com".to_string()],
                vec!["/tmp/file.txt".to_string()],
                "git status".to_string(),
            ),
            preview_text: "git status".to_string(),
            item_count: 1,
            total_bytes: 10,
            capture_count: 1,
            score: Some(0.1),
            why_matched: Some("git status".to_string()),
            matched_fields: vec!["best_text".to_string(), "search_text".to_string()],
            match_evidence: None,
            snippet: "quoted snippet".to_string(),
        },
        alternatives: vec![RecallOutputRow {
            snapshot_id: 3,
            event_id: 4,
            sha256: "def".to_string(),
            kind: "plain_text".to_string(),
            observed_at: "2026-04-17T09:00:00Z".to_string(),
            first_seen_at: "2026-04-17T08:00:00Z".to_string(),
            last_seen_at: "2026-04-17T09:00:00Z".to_string(),
            app_name: Some("Editor".to_string()),
            app_bundle_id: Some("com.example.Editor".to_string()),
            best_text: "git commit".to_string(),
            projection: text_projection_output(
                Some("public.utf8-plain-text"),
                Vec::new(),
                Vec::new(),
                "git commit".to_string(),
            ),
            preview_text: "git commit".to_string(),
            item_count: 1,
            total_bytes: 10,
            capture_count: 1,
            score: None,
            why_matched: None,
            matched_fields: Vec::new(),
            match_evidence: None,
            snippet: String::new(),
        }],
        best_match_confidence: RecallMatchConfidence::High,
        match_kind: "scored",
        best_match_score: Some(0.91),
        why_selected: "Selected the strongest search match".to_string(),
        quoted_text: Some("git status".to_string()),
        score_semantics: "evidence_v1",
    };

    let markdown = render_recall_markdown(&envelope);
    let toon = render_recall_toon(&envelope);

    assert!(markdown.contains("# Best Match"));
    assert!(markdown.contains("## Alternatives"));
    assert!(markdown.contains("> git status"));
    assert!(toon.contains(
            "best_candidate[1\t]{snapshot_id\tevent_id\tobserved_at\tfirst_seen_at\tlast_seen_at\tkind\tapp_name\tapp_bundle_id\tdisplay_text\tcapture_count\titem_count\ttotal_bytes\tscore\twhy_matched}:"
        ));
    assert!(toon.contains("alternatives[1\t]{snapshot_id\tevent_id\tobserved_at\tfirst_seen_at\tlast_seen_at\tkind\tapp_name\tapp_bundle_id\tdisplay_text\tcapture_count\titem_count\ttotal_bytes\tscore\twhy_matched}:"));
    assert!(toon.contains("quoted snippet"));
    assert!(toon.contains("git commit"));
    assert!(!toon.contains("\tsnippet}:"));
    assert!(!toon.contains("matched_fields"));
    assert!(!toon.contains("sha256"));
}

#[test]
pub(in crate::cli) fn timeline_toon_uses_scalar_projection_display_text_fallback() {
    let envelope = ListEnvelope {
        schema_version: OUTPUT_SCHEMA_VERSION,
        command: "timeline",
        generated_at: "2026-04-17T10:00:00Z".to_string(),
        applied_filters: json!({ "hours": 24 }),
        truncated: false,
        next_cursor: None,
        score_semantics: None,
        results: vec![ListRow::Timeline(TimelineListRow {
            event_id: 9,
            snapshot_id: 3,
            observed_at: "2026-04-17T10:00:00Z".to_string(),
            change_count: 2,
            kind: "plain_text".to_string(),
            app_name: Some("Terminal".to_string()),
            app_bundle_id: Some("com.apple.Terminal".to_string()),
            best_text: String::new(),
            projection: text_projection_output(
                None,
                Vec::new(),
                Vec::new(),
                "git summary".to_string(),
            ),
            preview_text: "git preview".to_string(),
            total_bytes: 42,
            sha256: "abc123".to_string(),
            item_count: 1,
        })],
    };

    let toon = render_list_toon(&envelope);

    assert!(toon.contains(
            "results[1\t]{event_id\tsnapshot_id\tobserved_at\tchange_count\tkind\tapp_name\tapp_bundle_id\tdisplay_text\titem_count\ttotal_bytes}:"
        ));
    assert!(toon.contains("git preview"));
    assert!(!toon.contains("best_text_uti"));
    assert!(!toon.contains("sha256"));
}

#[test]
#[ignore = "profiling harness for CLI JSON serialization"]
pub(in crate::cli) fn profile_json_output_serialization() {
    let envelope = large_list_envelope(10_000);

    let before = median_duration(7, || {
        let json = serde_json::to_string_pretty(&envelope).expect("serialize envelope to string");
        assert!(json.len() > 1_000_000);
    });
    let after = median_duration(7, || {
        let mut out = Vec::new();
        write_json_pretty(&mut out, &envelope).expect("stream envelope to writer");
        assert!(out.len() > 1_000_000);
    });

    eprintln!("json_output_string_before={before:?} json_output_writer_after={after:?}");
}

#[test]
#[ignore = "profiling harness for CLI TOON rendering"]
pub(in crate::cli) fn profile_toon_output_rendering() {
    let envelope = large_list_envelope(10_000);

    let before = median_duration(7, || {
        let toon = render_list_toon_join_encoded_for_profile(&envelope);
        assert!(toon.len() > 1_000_000);
    });
    let after = median_duration(7, || {
        let toon = render_list_toon(&envelope);
        assert!(toon.len() > 1_000_000);
    });

    eprintln!("toon_output_join_before={before:?} toon_output_stream_after={after:?}");
}

fn median_duration(runs: usize, mut f: impl FnMut()) -> Duration {
    let mut samples = Vec::with_capacity(runs);
    for _ in 0..runs {
        let started = Instant::now();
        f();
        samples.push(started.elapsed());
    }
    samples.sort();
    samples[samples.len() / 2]
}

fn large_list_envelope(row_count: usize) -> ListEnvelope {
    let results = (0..row_count)
        .map(|index| {
            ListRow::Snapshot(SnapshotListRow {
                snapshot_id: index as i64 + 1,
                event_id: index as i64 + 10_000,
                sha256: format!("{:064x}", index + 1),
                kind: "plain_text".to_string(),
                observed_at: "2026-04-17T10:00:00Z".to_string(),
                first_seen_at: "2026-04-17T09:00:00Z".to_string(),
                last_seen_at: "2026-04-17T10:00:00Z".to_string(),
                app_name: Some("Terminal".to_string()),
                app_bundle_id: Some("com.apple.Terminal".to_string()),
                best_text: format!("git status output row {index} with enough text to resemble a real clipboard row"),
                projection: text_projection_output(
                    Some("public.utf8-plain-text"),
                    vec![format!("https://example.com/{index}")],
                    vec![format!("/Users/test/repo/{index}/Cargo.toml")],
                    format!("git status summary {index}"),
                ),
                preview_text: format!("git status preview {index}"),
                item_count: 1,
                total_bytes: 128,
                capture_count: 1,
                score: Some(0.42),
                why_matched: Some("Exact text match".to_string()),
                matched_fields: vec!["best_text".to_string(), "search_text".to_string()],
                match_evidence: None,
            })
        })
        .collect();

    ListEnvelope {
        schema_version: OUTPUT_SCHEMA_VERSION,
        command: "search",
        generated_at: "2026-04-17T10:00:00Z".to_string(),
        applied_filters: json!({
            "query": "git",
            "requested_mode": "auto",
            "mode_used": "literal",
            "limit": row_count,
        }),
        truncated: false,
        next_cursor: None,
        score_semantics: None,
        results,
    }
}

fn render_list_toon_join_encoded_for_profile(envelope: &ListEnvelope) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "schema_version: {}", envelope.schema_version);
    let _ = writeln!(out, "command: {}", envelope.command);
    let _ = writeln!(out, "generated_at: {}", envelope.generated_at);
    let _ = writeln!(out, "applied_filters: {}", envelope.applied_filters);
    let _ = writeln!(out, "truncated: {}", envelope.truncated);
    let _ = writeln!(
        out,
        "next_cursor: {}",
        envelope.next_cursor.as_deref().unwrap_or("null")
    );

    let field_names = if envelope.command == "timeline" {
        ToonTimelineRowProjection::field_names()
    } else {
        ToonSearchRowProjection::field_names()
    };
    let _ = writeln!(
        out,
        "results[{}\t]{{{}}}:",
        envelope.results.len(),
        field_names.join("\t")
    );

    for row in &envelope.results {
        let fields_and_values = match row {
            ListRow::Snapshot(row) => {
                ToonSearchRowProjection::from_snapshot_row(row).fields_and_values()
            }
            ListRow::Timeline(row) => ToonTimelineRowProjection::from_row(row).fields_and_values(),
        };
        let encoded = fields_and_values
            .iter()
            .map(|(_field, value)| encode_toon_scalar_for_profile(value))
            .collect::<Vec<_>>()
            .join("\t");
        let _ = writeln!(out, "  {encoded}");
    }

    out
}

fn encode_toon_scalar_for_profile(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => number.to_string(),
        Value::String(text) => encode_toon_string_for_profile(text),
        Value::Array(_) | Value::Object(_) => {
            unreachable!("profile scalar encoder only accepts primitive values")
        }
    }
}

fn encode_toon_string_for_profile(text: &str) -> String {
    if text.is_empty()
        || text.contains('\t')
        || text.contains('\n')
        || text.contains('\r')
        || text.contains(':')
        || text.contains('"')
        || text.contains('\\')
        || text.starts_with(' ')
        || text.ends_with(' ')
    {
        let escaped = text
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
            .replace('\t', "\\t");
        format!("\"{escaped}\"")
    } else {
        text.to_string()
    }
}

fn temp_storage_path(name: &str) -> PathBuf {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();

    let dir = std::env::temp_dir()
        .join("clipmem-storage-tests")
        .join(format!("{}-{timestamp}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn cleanup_storage_path(path: &std::path::Path) {
    if let Ok(metadata) = std::fs::symlink_metadata(path) {
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            let _ = std::fs::remove_dir_all(path);
        } else {
            let _ = std::fs::remove_file(path);
        }
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::remove_dir_all(parent);
    }
}
