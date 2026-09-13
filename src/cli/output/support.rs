use std::fmt::Write;

use serde_json::Value;

use crate::db::StatsSnapshotLeaderboardEntry;

use super::row_text::truncate_for_display;

pub(in crate::cli) fn render_filter_pairs(filters: &Value) -> String {
    let Some(object) = filters.as_object() else {
        return "none".to_string();
    };

    object
        .iter()
        .map(|(key, value)| {
            let rendered = match value {
                Value::Null => "null".to_string(),
                Value::String(text) => text.clone(),
                other => other.to_string(),
            };
            format!("{key}={rendered}")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

pub(in crate::cli) fn escape_markdown_cell(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('|', "\\|")
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\n', "<br>")
}

pub(in crate::cli) fn truncate_for_markdown(value: &str, limit: usize) -> String {
    truncate_for_display(value, limit)
}

pub(in crate::cli) fn push_blank_line(out: &mut String) {
    out.push('\n');
}

pub(in crate::cli) fn push_snapshot_leaderboard(
    out: &mut String,
    snapshots: &[StatsSnapshotLeaderboardEntry],
    indent: usize,
) {
    if snapshots.is_empty() {
        let _ = writeln!(out, "{}none", " ".repeat(indent));
        return;
    }

    for snapshot in snapshots {
        push_snapshot_leaderboard_entry(out, snapshot, indent);
    }
}

pub(in crate::cli) fn push_snapshot_leaderboard_entry(
    out: &mut String,
    snapshot: &StatsSnapshotLeaderboardEntry,
    indent: usize,
) {
    let prefix = " ".repeat(indent);
    let app = snapshot.app_name().unwrap_or("Unknown");
    let preview = if snapshot.preview_text().trim().is_empty() {
        "(no preview)".to_string()
    } else {
        truncate_for_markdown(&snapshot.preview_text().replace('\n', " "), 120)
    };
    let _ = writeln!(
        out,
        "{prefix}[{}] {} captures, {}, {} bytes, last seen {}, {}",
        snapshot.snapshot_id(),
        snapshot.capture_count(),
        snapshot.kind(),
        snapshot.total_bytes(),
        format_utc_timestamp(snapshot.last_observed_at()),
        app
    );
    let _ = writeln!(out, "{prefix}  preview: {preview}");
}

pub(in crate::cli) fn format_utc_timestamp(timestamp: &str) -> String {
    if timestamp.ends_with('Z')
        || timestamp.ends_with(" UTC")
        || timestamp.contains('+')
        || timestamp.rfind('-').is_some_and(|idx| idx > 9)
    {
        timestamp.to_string()
    } else {
        format!("{timestamp} UTC")
    }
}
