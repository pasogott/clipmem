use anyhow::Result;

use crate::db::types::RetrievalFilters;

const EVENT_FILTER_PARAMETERS: &[&str] = &[":since", ":until", ":app_like", ":bundle_id"];

pub(in crate::db) fn has_temporal_event_filters(filters: &RetrievalFilters) -> bool {
    filters.since().is_some() || filters.until().is_some() || filters.hours().is_some()
}

pub(in crate::db) fn requires_matching_events(filters: &RetrievalFilters) -> bool {
    has_temporal_event_filters(filters)
}

pub(in crate::db) fn can_use_snapshot_stats_since_filter(filters: &RetrievalFilters) -> bool {
    (filters.since().is_some() || filters.hours().is_some())
        && filters.until().is_none()
        && filters.app().is_none()
        && filters.bundle_id().is_none()
}

pub(in crate::db) fn can_use_snapshot_stats_for_stats(filters: &RetrievalFilters) -> bool {
    !has_temporal_event_filters(filters) && filters.app().is_none() && filters.bundle_id().is_none()
}

pub(in crate::db) fn can_use_snapshot_event_cache(filters: &RetrievalFilters) -> bool {
    !has_temporal_event_filters(filters)
        && (filters.app().is_some() || filters.bundle_id().is_some())
}

pub(in crate::db) fn event_filter_clause(alias: &str) -> String {
    format!(
        "(:since IS NULL OR datetime({alias}.observed_at) >= datetime(:since))
         AND (:until IS NULL OR datetime({alias}.observed_at) <= datetime(:until))
         AND (:app_like IS NULL OR ({alias}.frontmost_app_name IS NOT NULL AND lower({alias}.frontmost_app_name) LIKE :app_like ESCAPE '\\'))
         AND (:bundle_id IS NULL OR ({alias}.frontmost_app_bundle_id IS NOT NULL AND lower({alias}.frontmost_app_bundle_id) = :bundle_id))"
    )
}

// Some query variants bind a shared named-parameter set even when a parameter
// has no semantic predicate. Keep those binding-only placeholders explicit.
pub(in crate::db) fn parameter_bindings_clause(parameters: &[&str]) -> String {
    parameters
        .iter()
        .map(|parameter| format!("({parameter} IS NULL OR {parameter} IS NOT NULL)"))
        .collect::<Vec<_>>()
        .join("\n         AND ")
}

pub(in crate::db) fn event_filter_parameter_bindings_clause() -> String {
    parameter_bindings_clause(EVENT_FILTER_PARAMETERS)
}

pub(in crate::db) fn snapshot_event_filter_clause(cache_alias: &str) -> String {
    let temporal_parameter_bindings = parameter_bindings_clause(&[":since", ":until"]);
    format!(
        "{temporal_parameter_bindings}
         AND (:app_like IS NULL OR ({cache_alias}.app_names_lower != '' AND {cache_alias}.app_names_lower LIKE :app_like ESCAPE '\\'))
         AND (:bundle_id IS NULL OR instr(char(31) || {cache_alias}.bundle_ids_lower || char(31), char(31) || :bundle_id || char(31)) > 0)
         AND (:app_like IS NULL OR :bundle_id IS NULL OR EXISTS (
             SELECT 1 FROM capture_events ce WHERE ce.snapshot_id = {cache_alias}.snapshot_id
             AND lower(ce.frontmost_app_name) LIKE :app_like ESCAPE '\\'
             AND lower(ce.frontmost_app_bundle_id) = :bundle_id
         ))"
    )
}

pub(in crate::db) fn base_event_filter_clause(
    cache_alias: &str,
    include_matching_events: bool,
    use_snapshot_event_cache: bool,
) -> String {
    if include_matching_events {
        event_filter_parameter_bindings_clause()
    } else if use_snapshot_event_cache {
        snapshot_event_filter_clause(cache_alias)
    } else {
        event_filter_parameter_bindings_clause()
    }
}

pub(in crate::db) fn snapshot_stats_since_filter_clause(
    use_snapshot_stats_since_filter: bool,
) -> String {
    if use_snapshot_stats_since_filter {
        "ss.last_observed_at >= datetime(:since)
         AND datetime(ss.last_observed_at) >= datetime(:since)"
            .to_string()
    } else {
        parameter_bindings_clause(&[":since"])
    }
}

pub(in crate::db) fn event_filter_where_clause(
    snapshot_id_expr: &str,
    cache_alias: &str,
    use_snapshot_event_cache: bool,
    has_temporal_event_filters: bool,
) -> String {
    if use_snapshot_event_cache {
        snapshot_event_filter_clause(cache_alias)
    } else if has_temporal_event_filters {
        format!(
            "EXISTS (
                 SELECT 1
                 FROM capture_events ce
                 WHERE ce.snapshot_id = {snapshot_id_expr}
                   AND {}
             )",
            event_filter_clause("ce")
        )
    } else {
        event_filter_parameter_bindings_clause()
    }
}

struct RetrievalKindPredicate {
    parameter_value: &'static str,
    predicate: SnapshotPredicate,
}

enum SnapshotPredicate {
    RepresentationKinds(&'static [&'static str]),
    SnapshotKinds(&'static [&'static str]),
}

struct PresencePredicate {
    parameter: &'static str,
    predicate: PresencePredicateSql,
}

enum PresencePredicateSql {
    SearchableText,
    RepresentationKinds(&'static [&'static str]),
}

const TEXT_RETRIEVAL_KINDS: &[&str] = &["plain_text", "html", "json", "xml", "rtf"];
const OTHER_SNAPSHOT_KINDS: &[&str] = &["mixed", "empty"];

const RETRIEVAL_KIND_PREDICATES: &[RetrievalKindPredicate] = &[
    RetrievalKindPredicate {
        parameter_value: "text",
        predicate: SnapshotPredicate::RepresentationKinds(TEXT_RETRIEVAL_KINDS),
    },
    RetrievalKindPredicate {
        parameter_value: "html",
        predicate: SnapshotPredicate::RepresentationKinds(&["html"]),
    },
    RetrievalKindPredicate {
        parameter_value: "rtf",
        predicate: SnapshotPredicate::RepresentationKinds(&["rtf"]),
    },
    RetrievalKindPredicate {
        parameter_value: "url",
        predicate: SnapshotPredicate::RepresentationKinds(&["url"]),
    },
    RetrievalKindPredicate {
        parameter_value: "file",
        predicate: SnapshotPredicate::RepresentationKinds(&["file_url"]),
    },
    RetrievalKindPredicate {
        parameter_value: "image",
        predicate: SnapshotPredicate::RepresentationKinds(&["image"]),
    },
    RetrievalKindPredicate {
        parameter_value: "pdf",
        predicate: SnapshotPredicate::RepresentationKinds(&["pdf"]),
    },
    RetrievalKindPredicate {
        parameter_value: "binary",
        predicate: SnapshotPredicate::RepresentationKinds(&["binary"]),
    },
    RetrievalKindPredicate {
        parameter_value: "other",
        predicate: SnapshotPredicate::SnapshotKinds(OTHER_SNAPSHOT_KINDS),
    },
];

const PRESENCE_PREDICATES: &[PresencePredicate] = &[
    PresencePredicate {
        parameter: ":has_text",
        predicate: PresencePredicateSql::SearchableText,
    },
    PresencePredicate {
        parameter: ":has_url",
        predicate: PresencePredicateSql::RepresentationKinds(&["url"]),
    },
    PresencePredicate {
        parameter: ":has_file_url",
        predicate: PresencePredicateSql::RepresentationKinds(&["file_url"]),
    },
    PresencePredicate {
        parameter: ":has_image",
        predicate: PresencePredicateSql::RepresentationKinds(&["image"]),
    },
    PresencePredicate {
        parameter: ":has_pdf",
        predicate: PresencePredicateSql::RepresentationKinds(&["pdf"]),
    },
];

pub(in crate::db) fn snapshot_filter_clause(
    snapshot_alias: &str,
    snapshot_id_expr: &str,
) -> String {
    let mut clauses = vec![
        format!("(:min_bytes IS NULL OR {snapshot_alias}.total_bytes >= :min_bytes)"),
        format!("(:max_bytes IS NULL OR {snapshot_alias}.total_bytes <= :max_bytes)"),
        retrieval_kind_filter_clause(snapshot_alias, snapshot_id_expr),
    ];
    clauses.extend(
        PRESENCE_PREDICATES
            .iter()
            .map(|predicate| presence_filter_clause(predicate, snapshot_alias, snapshot_id_expr)),
    );
    clauses.join("\n         AND ")
}

fn retrieval_kind_filter_clause(snapshot_alias: &str, snapshot_id_expr: &str) -> String {
    let predicates = RETRIEVAL_KIND_PREDICATES
        .iter()
        .map(|predicate| {
            format!(
                "(:kind = '{}' AND {})",
                predicate.parameter_value,
                snapshot_predicate_sql(&predicate.predicate, snapshot_alias, snapshot_id_expr)
            )
        })
        .collect::<Vec<_>>()
        .join("\n             OR ");

    format!("(:kind IS NULL\n             OR {predicates})")
}

fn presence_filter_clause(
    predicate: &PresencePredicate,
    snapshot_alias: &str,
    snapshot_id_expr: &str,
) -> String {
    format!(
        "({} = 0 OR {})",
        predicate.parameter,
        presence_predicate_sql(&predicate.predicate, snapshot_alias, snapshot_id_expr)
    )
}

fn snapshot_predicate_sql(
    predicate: &SnapshotPredicate,
    snapshot_alias: &str,
    snapshot_id_expr: &str,
) -> String {
    match predicate {
        SnapshotPredicate::RepresentationKinds(kinds) => {
            representation_kind_exists_clause(snapshot_id_expr, kinds, false)
        }
        SnapshotPredicate::SnapshotKinds(kinds) => {
            format!(
                "{snapshot_alias}.snapshot_kind IN ({})",
                sql_string_list(kinds)
            )
        }
    }
}

fn presence_predicate_sql(
    predicate: &PresencePredicateSql,
    _snapshot_alias: &str,
    snapshot_id_expr: &str,
) -> String {
    match predicate {
        PresencePredicateSql::SearchableText => format!(
            "EXISTS (
                 SELECT 1 FROM snapshot_search_documents ssd
                 WHERE ssd.snapshot_id = {snapshot_id_expr}
                   AND (ssd.has_native_text = 1 OR ssd.has_ocr_text = 1)
             )"
        ),
        PresencePredicateSql::RepresentationKinds(kinds) => {
            representation_kind_exists_clause(snapshot_id_expr, kinds, false)
        }
    }
}

fn representation_kind_exists_clause(
    snapshot_id_expr: &str,
    kinds: &[&str],
    require_text_value: bool,
) -> String {
    let text_value_clause = if require_text_value {
        "\n                   AND ir.text_value IS NOT NULL AND ir.text_value != ''"
    } else {
        ""
    };
    format!(
        "EXISTS (
                 SELECT 1 FROM item_representations ir
                 WHERE ir.snapshot_id = {snapshot_id_expr}
                   AND ir.kind IN ({}){text_value_clause}
             )",
        sql_string_list(kinds)
    )
}

fn sql_string_list(values: &[&str]) -> String {
    values
        .iter()
        .map(|value| format!("'{value}'"))
        .collect::<Vec<_>>()
        .join(", ")
}

pub(in crate::db) fn app_like_pattern(filters: &RetrievalFilters) -> Option<String> {
    filters
        .app()
        .map(|value| format!("%{}%", escape_like_pattern(&value.to_ascii_lowercase())))
}

pub(in crate::db) fn effective_since_param(filters: &RetrievalFilters) -> Result<Option<String>> {
    if let Some(since) = filters.since() {
        return Ok(Some(since.to_string()));
    }

    let Some(hours) = filters.hours() else {
        return Ok(None);
    };
    let since = time::OffsetDateTime::now_utc()
        .checked_sub(time::Duration::hours(i64::from(hours)))
        .ok_or_else(|| anyhow::anyhow!("hours exceeds the supported calendar range"))?
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|error| anyhow::anyhow!("format filter time: {error}"))?;
    Ok(Some(since))
}

pub(in crate::db) fn escape_like_pattern(query: &str) -> String {
    query
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}
