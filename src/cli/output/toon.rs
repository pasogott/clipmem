use std::fmt::Write;

use serde_json::Value;

use super::model::{
    ListEnvelope, ListRow, RecallEnvelope, RecallOutputRow, SnapshotListRow, TimelineListRow,
};

pub(in crate::cli) struct ToonSearchRowProjection {
    pub(in crate::cli) snapshot_id: i64,
    pub(in crate::cli) event_id: i64,
    pub(in crate::cli) observed_at: String,
    pub(in crate::cli) first_seen_at: String,
    pub(in crate::cli) last_seen_at: String,
    pub(in crate::cli) kind: String,
    pub(in crate::cli) app_name: Option<String>,
    pub(in crate::cli) app_bundle_id: Option<String>,
    pub(in crate::cli) display_text: String,
    pub(in crate::cli) capture_count: usize,
    pub(in crate::cli) item_count: usize,
    pub(in crate::cli) total_bytes: usize,
    pub(in crate::cli) score: Option<f64>,
    pub(in crate::cli) why_matched: Option<String>,
}

impl ToonSearchRowProjection {
    pub(in crate::cli) fn from_snapshot_row(row: &SnapshotListRow) -> Self {
        Self::from_parts(ToonSearchRowParts {
            snapshot_id: row.snapshot_id,
            event_id: row.event_id,
            observed_at: row.observed_at.clone(),
            first_seen_at: row.first_seen_at.clone(),
            last_seen_at: row.last_seen_at.clone(),
            kind: row.kind.clone(),
            app_name: row.app_name.clone(),
            app_bundle_id: row.app_bundle_id.clone(),
            display_text: first_non_empty_text(&[
                row.best_text.as_str(),
                row.preview_text.as_str(),
                row.projection.text_summary.as_str(),
            ]),
            capture_count: row.capture_count,
            item_count: row.item_count,
            total_bytes: row.total_bytes,
            score: row.score,
            why_matched: row.why_matched.clone(),
        })
    }

    pub(in crate::cli) fn from_recall_row(row: &RecallOutputRow) -> Self {
        Self::from_parts(ToonSearchRowParts {
            snapshot_id: row.snapshot_id,
            event_id: row.event_id,
            observed_at: row.observed_at.clone(),
            first_seen_at: row.first_seen_at.clone(),
            last_seen_at: row.last_seen_at.clone(),
            kind: row.kind.clone(),
            app_name: row.app_name.clone(),
            app_bundle_id: row.app_bundle_id.clone(),
            display_text: first_non_empty_text(&[
                row.best_text.as_str(),
                row.snippet.as_str(),
                row.preview_text.as_str(),
                row.projection.text_summary.as_str(),
            ]),
            capture_count: row.capture_count,
            item_count: row.item_count,
            total_bytes: row.total_bytes,
            score: row.score,
            why_matched: row.why_matched.clone(),
        })
    }

    fn from_parts(parts: ToonSearchRowParts) -> Self {
        Self {
            snapshot_id: parts.snapshot_id,
            event_id: parts.event_id,
            observed_at: parts.observed_at,
            first_seen_at: parts.first_seen_at,
            last_seen_at: parts.last_seen_at,
            kind: parts.kind,
            app_name: parts.app_name,
            app_bundle_id: parts.app_bundle_id,
            display_text: parts.display_text,
            capture_count: parts.capture_count,
            item_count: parts.item_count,
            total_bytes: parts.total_bytes,
            score: parts.score,
            why_matched: parts.why_matched,
        }
    }

    fn schema_row() -> Self {
        Self {
            snapshot_id: 0,
            event_id: 0,
            observed_at: String::new(),
            first_seen_at: String::new(),
            last_seen_at: String::new(),
            kind: String::new(),
            app_name: None,
            app_bundle_id: None,
            display_text: String::new(),
            capture_count: 0,
            item_count: 0,
            total_bytes: 0,
            score: None,
            why_matched: None,
        }
    }

    pub(in crate::cli) fn field_names() -> Vec<&'static str> {
        toon_field_names(&Self::schema_row().fields_and_values())
    }

    pub(in crate::cli) fn fields_and_values(&self) -> Vec<(&'static str, Value)> {
        vec![
            ("snapshot_id", Value::from(self.snapshot_id)),
            ("event_id", Value::from(self.event_id)),
            ("observed_at", Value::String(self.observed_at.clone())),
            ("first_seen_at", Value::String(self.first_seen_at.clone())),
            ("last_seen_at", Value::String(self.last_seen_at.clone())),
            ("kind", Value::String(self.kind.clone())),
            (
                "app_name",
                self.app_name.clone().map_or(Value::Null, Value::String),
            ),
            (
                "app_bundle_id",
                self.app_bundle_id
                    .clone()
                    .map_or(Value::Null, Value::String),
            ),
            ("display_text", Value::String(self.display_text.clone())),
            ("capture_count", Value::from(self.capture_count as u64)),
            ("item_count", Value::from(self.item_count as u64)),
            ("total_bytes", Value::from(self.total_bytes as u64)),
            ("score", self.score.map_or(Value::Null, Value::from)),
            (
                "why_matched",
                self.why_matched.clone().map_or(Value::Null, Value::String),
            ),
        ]
    }
}

struct ToonSearchRowParts {
    snapshot_id: i64,
    event_id: i64,
    observed_at: String,
    first_seen_at: String,
    last_seen_at: String,
    kind: String,
    app_name: Option<String>,
    app_bundle_id: Option<String>,
    display_text: String,
    capture_count: usize,
    item_count: usize,
    total_bytes: usize,
    score: Option<f64>,
    why_matched: Option<String>,
}

pub(in crate::cli) struct ToonTimelineRowProjection {
    pub(in crate::cli) event_id: i64,
    pub(in crate::cli) snapshot_id: i64,
    pub(in crate::cli) observed_at: String,
    pub(in crate::cli) change_count: i64,
    pub(in crate::cli) kind: String,
    pub(in crate::cli) app_name: Option<String>,
    pub(in crate::cli) app_bundle_id: Option<String>,
    pub(in crate::cli) display_text: String,
    pub(in crate::cli) item_count: usize,
    pub(in crate::cli) total_bytes: usize,
}

impl ToonTimelineRowProjection {
    pub(in crate::cli) fn from_row(row: &TimelineListRow) -> Self {
        Self {
            event_id: row.event_id,
            snapshot_id: row.snapshot_id,
            observed_at: row.observed_at.clone(),
            change_count: row.change_count,
            kind: row.kind.clone(),
            app_name: row.app_name.clone(),
            app_bundle_id: row.app_bundle_id.clone(),
            display_text: first_non_empty_text(&[
                row.best_text.as_str(),
                row.preview_text.as_str(),
                row.projection.text_summary.as_str(),
            ]),
            item_count: row.item_count,
            total_bytes: row.total_bytes,
        }
    }

    fn schema_row() -> Self {
        Self {
            event_id: 0,
            snapshot_id: 0,
            observed_at: String::new(),
            change_count: 0,
            kind: String::new(),
            app_name: None,
            app_bundle_id: None,
            display_text: String::new(),
            item_count: 0,
            total_bytes: 0,
        }
    }

    pub(in crate::cli) fn field_names() -> Vec<&'static str> {
        toon_field_names(&Self::schema_row().fields_and_values())
    }

    pub(in crate::cli) fn fields_and_values(&self) -> Vec<(&'static str, Value)> {
        vec![
            ("event_id", Value::from(self.event_id)),
            ("snapshot_id", Value::from(self.snapshot_id)),
            ("observed_at", Value::String(self.observed_at.clone())),
            ("change_count", Value::from(self.change_count)),
            ("kind", Value::String(self.kind.clone())),
            (
                "app_name",
                self.app_name.clone().map_or(Value::Null, Value::String),
            ),
            (
                "app_bundle_id",
                self.app_bundle_id
                    .clone()
                    .map_or(Value::Null, Value::String),
            ),
            ("display_text", Value::String(self.display_text.clone())),
            ("item_count", Value::from(self.item_count as u64)),
            ("total_bytes", Value::from(self.total_bytes as u64)),
        ]
    }
}

fn toon_field_names(fields_and_values: &[(&'static str, Value)]) -> Vec<&'static str> {
    fields_and_values
        .iter()
        .map(|(field, _value)| *field)
        .collect()
}

fn toon_field_values(fields_and_values: Vec<(&'static str, Value)>) -> Vec<Value> {
    fields_and_values
        .into_iter()
        .map(|(_field, value)| value)
        .collect()
}

fn push_toon_field_values_tab_separated(
    out: &mut String,
    fields_and_values: Vec<(&'static str, Value)>,
) {
    let values = toon_field_values(fields_and_values);
    push_toon_scalars_tab_separated(out, &values);
}

pub(in crate::cli) fn first_non_empty_text(candidates: &[&str]) -> String {
    candidates
        .iter()
        .find(|candidate| !candidate.trim().is_empty())
        .map(|candidate| (*candidate).to_string())
        .unwrap_or_default()
}

pub(in crate::cli) fn render_list_toon(envelope: &ListEnvelope) -> String {
    let mut out = String::with_capacity(estimated_list_toon_capacity(envelope));
    render_toon_entry(
        &mut out,
        "schema_version",
        &Value::from(envelope.schema_version as u64),
        0,
    );
    render_toon_entry(
        &mut out,
        "command",
        &Value::String(envelope.command.to_string()),
        0,
    );
    render_toon_entry(
        &mut out,
        "generated_at",
        &Value::String(envelope.generated_at.clone()),
        0,
    );
    render_toon_entry(&mut out, "applied_filters", &envelope.applied_filters, 0);
    render_toon_entry(&mut out, "truncated", &Value::Bool(envelope.truncated), 0);
    render_toon_entry(
        &mut out,
        "next_cursor",
        &envelope
            .next_cursor
            .as_ref()
            .map_or(Value::Null, |value| Value::String(value.clone())),
        0,
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
        out.push_str("  ");
        push_toon_field_values_tab_separated(&mut out, fields_and_values);
        out.push('\n');
    }

    out
}

pub(in crate::cli) fn render_recall_toon(envelope: &RecallEnvelope) -> String {
    let mut out = String::new();
    render_toon_entry(
        &mut out,
        "schema_version",
        &Value::from(envelope.schema_version as u64),
        0,
    );
    render_toon_entry(&mut out, "command", &Value::String("recall".to_string()), 0);
    render_toon_entry(
        &mut out,
        "generated_at",
        &Value::String(envelope.generated_at.clone()),
        0,
    );
    render_toon_entry(
        &mut out,
        "query",
        &envelope.query.clone().map_or(Value::Null, Value::String),
        0,
    );
    render_toon_entry(
        &mut out,
        "best_match_confidence",
        &serde_json::to_value(&envelope.best_match_confidence).unwrap_or(Value::Null),
        0,
    );
    render_toon_entry(
        &mut out,
        "best_match_score",
        &envelope.best_match_score.map_or(Value::Null, Value::from),
        0,
    );
    render_toon_entry(
        &mut out,
        "why_selected",
        &Value::String(envelope.why_selected.clone()),
        0,
    );
    if let Some(quoted_text) = &envelope.quoted_text {
        render_toon_entry(
            &mut out,
            "quoted_text",
            &Value::String(quoted_text.clone()),
            0,
        );
    }
    render_toon_entry(&mut out, "applied_filters", &envelope.applied_filters, 0);

    render_recall_rows_toon(
        &mut out,
        "best_candidate",
        std::slice::from_ref(&envelope.best_candidate),
    );
    render_recall_rows_toon(&mut out, "alternatives", &envelope.alternatives);

    out
}

pub(in crate::cli) fn render_recall_rows_toon(
    out: &mut String,
    key: &str,
    rows: &[RecallOutputRow],
) {
    let _ = writeln!(
        out,
        "{key}[{}\t]{{{}}}:",
        rows.len(),
        ToonSearchRowProjection::field_names().join("\t")
    );
    for row in rows {
        let fields_and_values = ToonSearchRowProjection::from_recall_row(row).fields_and_values();
        out.push_str("  ");
        push_toon_field_values_tab_separated(out, fields_and_values);
        out.push('\n');
    }
}

pub(in crate::cli) fn render_toon_entry(out: &mut String, key: &str, value: &Value, indent: usize) {
    let padding = " ".repeat(indent);
    match value {
        Value::Object(object) => {
            let _ = writeln!(out, "{padding}{key}:");
            render_toon_object_entries(out, object, indent + 2);
        }
        Value::Array(array) => render_toon_array(out, Some(key), array, indent),
        _ => {
            let _ = write!(out, "{padding}{key}: ");
            push_toon_scalar(out, value);
            out.push('\n');
        }
    }
}

pub(in crate::cli) fn render_toon_object_entries(
    out: &mut String,
    object: &serde_json::Map<String, Value>,
    indent: usize,
) {
    for (key, value) in object {
        render_toon_entry(out, key, value, indent);
    }
}

pub(in crate::cli) fn render_toon_array(
    out: &mut String,
    key: Option<&str>,
    values: &[Value],
    indent: usize,
) {
    let padding = " ".repeat(indent);
    let key_prefix = key
        .map(|name| format!("{padding}{name}"))
        .unwrap_or(padding);

    if values.iter().all(Value::is_null)
        || values
            .iter()
            .all(|value| !matches!(value, Value::Array(_) | Value::Object(_)))
    {
        if values.is_empty() {
            let _ = writeln!(out, "{key_prefix}[0\t]:");
            return;
        }

        let _ = write!(out, "{key_prefix}[{}\t]: ", values.len());
        push_toon_scalars_tab_separated(out, values);
        out.push('\n');
        return;
    }

    let _ = writeln!(out, "{key_prefix}[{}]:", values.len());
    for value in values {
        render_toon_list_item(out, value, indent + 2);
    }
}

pub(in crate::cli) fn render_toon_list_item(out: &mut String, value: &Value, indent: usize) {
    let padding = " ".repeat(indent);
    match value {
        Value::Object(object) => render_toon_object_list_item(out, object, indent),
        Value::Array(array) => {
            let _ = writeln!(out, "{padding}-");
            render_toon_array(out, None, array, indent + 2);
        }
        _ => {
            let _ = write!(out, "{padding}- ");
            push_toon_scalar(out, value);
            out.push('\n');
        }
    }
}

pub(in crate::cli) fn render_toon_object_list_item(
    out: &mut String,
    object: &serde_json::Map<String, Value>,
    indent: usize,
) {
    let padding = " ".repeat(indent);
    let mut entries = object.iter();
    let Some((first_key, first_value)) = entries.next() else {
        let _ = writeln!(out, "{padding}-");
        return;
    };

    match first_value {
        Value::Object(nested) => {
            let _ = writeln!(out, "{padding}- {first_key}:");
            render_toon_object_entries(out, nested, indent + 4);
        }
        Value::Array(array) => {
            let _ = writeln!(out, "{padding}- {first_key}:");
            render_toon_array(out, None, array, indent + 4);
        }
        _ => {
            let _ = write!(out, "{padding}- {first_key}: ");
            push_toon_scalar(out, first_value);
            out.push('\n');
        }
    }

    for (key, value) in entries {
        render_toon_entry(out, key, value, indent + 2);
    }
}

pub(in crate::cli) fn push_toon_scalar(out: &mut String, value: &Value) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(flag) => {
            let _ = write!(out, "{flag}");
        }
        Value::Number(number) => {
            let _ = write!(out, "{number}");
        }
        Value::String(text) => push_toon_string(out, text),
        Value::Array(_) | Value::Object(_) => {
            unreachable!("push_toon_scalar only accepts primitive TOON values")
        }
    }
}

pub(in crate::cli) fn push_toon_string(out: &mut String, text: &str) {
    let needs_quotes = text.is_empty()
        || matches!(text, "true" | "false" | "null")
        || text.parse::<f64>().is_ok()
        || text.chars().any(|ch| {
            ch.is_control() || matches!(ch, ':' | '"' | '\\' | '[' | ']' | '{' | '}' | ',')
        })
        || text.starts_with('-')
        || text.starts_with('#')
        || text.trim() != text;
    if needs_quotes {
        out.push('"');
        for ch in text.chars() {
            match ch {
                '\\' => out.push_str("\\\\"),
                '"' => out.push_str("\\\""),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                '\u{0000}'..='\u{001f}' => {
                    let _ = write!(out, "\\u{:04x}", u32::from(ch));
                }
                _ => out.push(ch),
            }
        }
        out.push('"');
    } else {
        out.push_str(text);
    }
}

pub(in crate::cli) fn push_toon_scalars_tab_separated(out: &mut String, values: &[Value]) {
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            out.push('\t');
        }
        push_toon_scalar(out, value);
    }
}

fn estimated_list_toon_capacity(envelope: &ListEnvelope) -> usize {
    384 + envelope.results.len().saturating_mul(192)
}

#[cfg(test)]
mod scalar_regressions {
    #[test]
    fn toon_controls_use_supported_escapes_and_preserve_literal_backslashes() {
        let mut out = String::new();
        super::push_toon_string(&mut out, "\u{0008}\u{000c}\u{0007}\\b\\f\n\r\t");
        assert_eq!(out, r#""\u0008\u000c\u0007\\b\\f\n\r\t""#);
        for text in ["#", "# comment"] {
            out.clear();
            super::push_toon_string(&mut out, text);
            assert_eq!(out, format!("\"{text}\""));
        }
    }
}
