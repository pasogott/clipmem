use std::fmt::Write;

use crate::cli::human::{
    first_non_empty, format_bytes, format_count, format_timestamp_short, human_score,
    render_confidence, render_filter_summary, render_score_cell, separator, truncate_cell,
    truncate_multiline, HumanTheme, WIDTH,
};
use crate::cli::output::{ListEnvelope, ListRow, RecallEnvelope};

pub(in crate::cli) fn render_list_human(envelope: &ListEnvelope) -> String {
    let theme = HumanTheme::detect();
    let title = match envelope.command {
        "search" => "clipmem Search",
        "recent" => "clipmem Recent",
        "timeline" => "clipmem Timeline",
        other => return render_unknown_list_human(other, envelope),
    };
    let mut out = header(&theme, title);
    push_meta(&mut out, envelope);

    if envelope.results.is_empty() {
        out.push_str("No results.\n");
        return out;
    }

    if envelope.command == "timeline" {
        push_timeline_table(&mut out, &theme, &envelope.results);
    } else {
        push_snapshot_table(&mut out, &theme, &envelope.results);
    }

    out
}

pub(in crate::cli) fn render_recall_human(envelope: &RecallEnvelope) -> String {
    let theme = HumanTheme::detect();
    let best = &envelope.best_candidate;
    let full = envelope
        .applied_filters
        .get("full")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let limit = if full { usize::MAX } else { 480 };
    let mut out = header(&theme, "clipmem Recall");
    let _ = writeln!(
        out,
        "Query: {}  Filters: {}",
        envelope.query.as_deref().unwrap_or("(recent clipboard)"),
        render_filter_summary(&envelope.applied_filters)
    );
    out.push('\n');

    let _ = writeln!(out, "{}", theme.section("Best Match"));
    out.push_str(&separator(60, false));
    out.push('\n');
    if let Some(quoted_text) = &envelope.quoted_text {
        out.push_str(&truncate_multiline(quoted_text, limit));
        out.push('\n');
    } else if !best.best_text.trim().is_empty() {
        out.push_str(&truncate_multiline(&best.best_text, limit));
        out.push('\n');
    } else {
        out.push_str("No direct text was recovered for this clipboard item.\n");
    }
    out.push('\n');
    let score = envelope.best_match_score.unwrap_or(0.0);
    let _ = writeln!(
        out,
        "Provenance: snapshot {}  observed {}  app {}  confidence {}  score {}",
        theme.id(best.snapshot_id),
        format_timestamp_short(&best.observed_at),
        best.app_name
            .as_deref()
            .or(best.app_bundle_id.as_deref())
            .unwrap_or("unknown app"),
        render_confidence(&envelope.best_match_confidence),
        theme.score(score, &format!("{:.1}%", score * 100.0))
    );
    let _ = writeln!(out, "Why: {}", envelope.why_selected);
    if !best.projection.urls.is_empty() {
        let _ = writeln!(
            out,
            "URLs: {}",
            truncate_cell(&best.projection.urls.join(", "), 88)
        );
    }
    if !best.projection.file_paths.is_empty() {
        let _ = writeln!(
            out,
            "Files: {}",
            truncate_cell(&best.projection.file_paths.join(", "), 88)
        );
    }

    if !envelope.alternatives.is_empty() {
        out.push('\n');
        let _ = writeln!(out, "{}", theme.section("Alternatives"));
        out.push_str(&separator(WIDTH, false));
        out.push('\n');
        let _ = writeln!(
            out,
            "{:>3}  {:>7}  {:<14}  {:<18}  {:>7}  Preview",
            "#", "ID", "Seen", "App", "Score"
        );
        out.push_str(&separator(WIDTH, false));
        out.push('\n');
        for (idx, row) in envelope.alternatives.iter().take(10).enumerate() {
            let score_text = render_score_cell(&theme, row.score);
            let app = row
                .app_name
                .as_deref()
                .or(row.app_bundle_id.as_deref())
                .unwrap_or("unknown");
            let _ = writeln!(
                out,
                "{:>2}.  {:>7}  {:<14}  {:<18}  {}  {}",
                idx + 1,
                theme.id(row.snapshot_id),
                format_timestamp_short(&row.observed_at),
                truncate_cell(app, 18),
                score_text,
                truncate_cell(&row.snippet, 36)
            );
        }
    }

    out
}

pub(in crate::cli) fn render_unknown_list_human(command: &str, envelope: &ListEnvelope) -> String {
    let theme = HumanTheme::detect();
    let mut out = header(&theme, &format!("clipmem {command}"));
    push_meta(&mut out, envelope);
    out
}

pub(in crate::cli) fn header(theme: &HumanTheme, title: &str) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{}", theme.title(title));
    out.push_str(&separator(WIDTH.min(72), true));
    out.push('\n');
    out
}

pub(in crate::cli) fn push_meta(out: &mut String, envelope: &ListEnvelope) {
    let _ = writeln!(
        out,
        "Generated {}  Filters: {}  Results: {}",
        format_timestamp_short(&envelope.generated_at),
        render_filter_summary(&envelope.applied_filters),
        envelope.results.len()
    );
    if envelope.truncated {
        let cursor = envelope
            .next_cursor
            .as_deref()
            .map(|cursor| truncate_cell(cursor, 36))
            .unwrap_or_else(|| "unavailable".to_string());
        let _ = writeln!(
            out,
            "More results available. Cursor: {cursor} (use `--format json` for the full token)"
        );
    }
    out.push('\n');
}

pub(in crate::cli) fn push_snapshot_table(out: &mut String, theme: &HumanTheme, rows: &[ListRow]) {
    let has_score = rows.iter().any(|row| match row {
        ListRow::Snapshot(row) => human_score(row.score).is_some(),
        ListRow::Timeline(_) => false,
    });
    out.push_str(&separator(WIDTH, false));
    out.push('\n');
    if has_score {
        let _ = writeln!(
            out,
            "{:>3}  {:>7}  {:<14}  {:<16}  {:<7}  {:>8}  {:>8}  {:>7}  Preview",
            "#", "ID", "Seen", "App", "Kind", "Size", "Captures", "Score"
        );
    } else {
        let _ = writeln!(
            out,
            "{:>3}  {:>7}  {:<14}  {:<16}  {:<7}  {:>8}  {:>8}  Preview",
            "#", "ID", "Seen", "App", "Kind", "Size", "Captures"
        );
    }
    out.push_str(&separator(WIDTH, false));
    out.push('\n');

    for (idx, row) in rows.iter().enumerate() {
        let ListRow::Snapshot(row) = row else {
            continue;
        };
        let app = row
            .app_name
            .as_deref()
            .or(row.app_bundle_id.as_deref())
            .unwrap_or("unknown");
        let text = first_non_empty(&[
            &row.best_text,
            &row.preview_text,
            &row.projection.text_summary,
        ]);
        if has_score {
            let score_text = render_score_cell(theme, row.score);
            let _ = writeln!(
                out,
                "{:>2}.  {:>7}  {:<14}  {:<16}  {:<7}  {:>8}  {:>8}  {}  {}",
                idx + 1,
                theme.id(row.snapshot_id),
                format_timestamp_short(&row.observed_at),
                truncate_cell(app, 16),
                row.kind,
                format_bytes(row.total_bytes as u64),
                format_count(row.capture_count),
                score_text,
                truncate_cell(text, 24)
            );
        } else {
            let _ = writeln!(
                out,
                "{:>2}.  {:>7}  {:<14}  {:<16}  {:<7}  {:>8}  {:>8}  {}",
                idx + 1,
                theme.id(row.snapshot_id),
                format_timestamp_short(&row.observed_at),
                truncate_cell(app, 16),
                row.kind,
                format_bytes(row.total_bytes as u64),
                format_count(row.capture_count),
                truncate_cell(text, 34)
            );
        }
    }
}

pub(in crate::cli) fn push_timeline_table(out: &mut String, theme: &HumanTheme, rows: &[ListRow]) {
    out.push_str(&separator(WIDTH, false));
    out.push('\n');
    let _ = writeln!(
        out,
        "{:>3}  {:>7}  {:>8}  {:<14}  {:<16}  {:<7}  {:>8}  Preview",
        "#", "Event", "Snapshot", "Seen", "App", "Kind", "Size"
    );
    out.push_str(&separator(WIDTH, false));
    out.push('\n');
    for (idx, row) in rows.iter().enumerate() {
        let ListRow::Timeline(row) = row else {
            continue;
        };
        let app = row
            .app_name
            .as_deref()
            .or(row.app_bundle_id.as_deref())
            .unwrap_or("unknown");
        let text = first_non_empty(&[
            &row.best_text,
            &row.preview_text,
            &row.projection.text_summary,
        ]);
        let _ = writeln!(
            out,
            "{:>2}.  {:>7}  {:>8}  {:<14}  {:<16}  {:<7}  {:>8}  {}",
            idx + 1,
            theme.id(row.event_id),
            row.snapshot_id,
            format_timestamp_short(&row.observed_at),
            truncate_cell(app, 16),
            row.kind,
            format_bytes(row.total_bytes as u64),
            truncate_cell(text, 28)
        );
    }
}
