use super::filter_sql::{
    base_event_filter_clause, event_filter_clause, event_filter_where_clause,
    parameter_bindings_clause, snapshot_filter_clause, snapshot_stats_since_filter_clause,
};
use crate::db::types::TimelineSort;

mod matching_events;

use matching_events::{matching_events_cte, matching_events_join};

pub(in crate::db) fn unified_fts_query(use_event_cache: bool, temporal: bool) -> String {
    format!(
        r"
WITH candidates AS (
 SELECT ssd.snapshot_id,
  bm25(snapshot_search_documents_fts,4.0,2.0,3.0,1.8,2.2,1.2,1.4) raw_rank,
  CASE WHEN :phrase_lower != '' AND (lower(ssd.native_text) LIKE :phrase_like ESCAPE '\' OR lower(ssd.preview_text) LIKE :phrase_like ESCAPE '\' OR lower(ssd.ocr_text) LIKE :phrase_like ESCAPE '\' OR lower(ssd.url_text) LIKE :phrase_like ESCAPE '\' OR lower(ssd.file_path_text) LIKE :phrase_like ESCAPE '\' OR lower(ssd.historical_app_names) LIKE :phrase_like ESCAPE '\' OR lower(ssd.historical_app_bundle_ids) LIKE :phrase_like ESCAPE '\') THEN 1 ELSE 0 END exact_phrase_match,
  trim(CASE WHEN instr(highlight(snapshot_search_documents_fts,0,'⟦','⟧'),'⟦')>0 THEN 'native_text'||char(30)||'best_text'||char(30)||'search_text'||char(30) ELSE '' END || CASE WHEN instr(highlight(snapshot_search_documents_fts,1,'⟦','⟧'),'⟦')>0 THEN 'preview_text'||char(30)||'best_text'||char(30) ELSE '' END || CASE WHEN instr(highlight(snapshot_search_documents_fts,2,'⟦','⟧'),'⟦')>0 THEN 'ocr_text'||char(30) ELSE '' END || CASE WHEN instr(highlight(snapshot_search_documents_fts,3,'⟦','⟧'),'⟦')>0 THEN 'urls'||char(30) ELSE '' END || CASE WHEN instr(highlight(snapshot_search_documents_fts,4,'⟦','⟧'),'⟦')>0 THEN 'file_paths'||char(30) ELSE '' END || CASE WHEN instr(highlight(snapshot_search_documents_fts,5,'⟦','⟧'),'⟦')>0 THEN 'historical_app'||char(30)||'app_name'||char(30) ELSE '' END || CASE WHEN instr(highlight(snapshot_search_documents_fts,6,'⟦','⟧'),'⟦')>0 THEN 'historical_app_bundle_id'||char(30)||'app_bundle_id'||char(30) ELSE '' END,char(30)) matched_fields,
  COALESCE(CASE WHEN :phrase_lower != '' AND lower(ssd.native_text) LIKE :phrase_like ESCAPE '\' THEN 'Exact phrase match in best text' WHEN :phrase_lower != '' AND lower(ssd.ocr_text) LIKE :phrase_like ESCAPE '\' THEN 'Exact phrase match in OCR text' END,CASE WHEN instr(highlight(snapshot_search_documents_fts,0,'⟦','⟧'),'⟦')>0 THEN snippet(snapshot_search_documents_fts,0,'⟦','⟧',' … ',24) END,CASE WHEN instr(highlight(snapshot_search_documents_fts,1,'⟦','⟧'),'⟦')>0 THEN snippet(snapshot_search_documents_fts,1,'⟦','⟧',' … ',24) END,CASE WHEN instr(highlight(snapshot_search_documents_fts,2,'⟦','⟧'),'⟦')>0 THEN snippet(snapshot_search_documents_fts,2,'⟦','⟧',' … ',24) END,CASE WHEN instr(highlight(snapshot_search_documents_fts,3,'⟦','⟧'),'⟦')>0 THEN snippet(snapshot_search_documents_fts,3,'⟦','⟧',' … ',24) END,CASE WHEN instr(highlight(snapshot_search_documents_fts,4,'⟦','⟧'),'⟦')>0 THEN snippet(snapshot_search_documents_fts,4,'⟦','⟧',' … ',24) END,CASE WHEN instr(highlight(snapshot_search_documents_fts,5,'⟦','⟧'),'⟦')>0 THEN snippet(snapshot_search_documents_fts,5,'⟦','⟧',' … ',24) END,CASE WHEN instr(highlight(snapshot_search_documents_fts,6,'⟦','⟧'),'⟦')>0 THEN snippet(snapshot_search_documents_fts,6,'⟦','⟧',' … ',24) END,ssd.preview_text) why_matched
 FROM snapshot_search_documents_fts JOIN snapshot_search_documents ssd ON ssd.snapshot_id=snapshot_search_documents_fts.rowid JOIN snapshots s ON s.id=ssd.snapshot_id LEFT JOIN snapshot_event_filter_cache se ON se.snapshot_id=s.id
 WHERE snapshot_search_documents_fts MATCH :query AND {snapshot_filter} AND {event_filter}
)
SELECT s.id snapshot_id,ss.last_event_id event_id,s.sha256,s.snapshot_kind,COALESCE(NULLIF(s.preview_text,''),NULLIF(ssd.ocr_text,''),'') preview_text,s.search_text,c.why_matched,c.matched_fields,ss.capture_count,ss.first_observed_at,ss.last_observed_at,ss.last_frontmost_app_name,ss.last_frontmost_app_bundle_id,ssd.url_text urls,ssd.file_path_text file_urls,s.total_bytes,s.item_count,c.raw_rank score,c.raw_rank order_primary,0.0 order_secondary,c.exact_phrase_match,ssd.native_text evidence_native_text,ssd.preview_text evidence_preview_text,ssd.ocr_text evidence_ocr_text,ssd.url_text evidence_url_text,ssd.file_path_text evidence_file_path_text,ssd.historical_app_names evidence_app_names,ssd.historical_app_bundle_ids evidence_app_bundle_ids
FROM candidates c JOIN snapshot_search_documents ssd ON ssd.snapshot_id=c.snapshot_id JOIN snapshots s ON s.id=c.snapshot_id JOIN snapshot_stats ss ON ss.snapshot_id=c.snapshot_id
WHERE (:cursor_primary IS NULL OR c.raw_rank>:cursor_primary OR (c.raw_rank=:cursor_primary AND c.exact_phrase_match<:cursor_exact) OR (c.raw_rank=:cursor_primary AND c.exact_phrase_match=:cursor_exact AND (ss.last_observed_at<:cursor_last_seen_at OR (ss.last_observed_at=:cursor_last_seen_at AND s.id<:cursor_snapshot_id))))
ORDER BY c.raw_rank ASC,c.exact_phrase_match DESC,ss.last_observed_at DESC,s.id DESC LIMIT :limit",
        snapshot_filter = snapshot_filter_clause("s", "s.id"),
        event_filter = event_filter_where_clause("s.id", "se", use_event_cache, temporal)
    )
}

pub(in crate::db) fn unified_literal_query(use_event_cache: bool, temporal: bool) -> String {
    format!(
        r"
WITH candidates AS (
 SELECT ssd.snapshot_id,
  CASE WHEN lower(ssd.native_text)=:query_lower OR lower(ssd.preview_text)=:query_lower OR lower(ssd.ocr_text)=:query_lower OR lower(ssd.url_text)=:query_lower OR lower(ssd.file_path_text)=:query_lower THEN 5 WHEN lower(ssd.native_text) LIKE :prefix_like ESCAPE '\' OR lower(ssd.preview_text) LIKE :prefix_like ESCAPE '\' OR lower(ssd.ocr_text) LIKE :prefix_like ESCAPE '\' THEN 4 WHEN lower(ssd.native_text) LIKE :like ESCAPE '\' OR lower(ssd.preview_text) LIKE :like ESCAPE '\' OR lower(ssd.ocr_text) LIKE :like ESCAPE '\' OR lower(ssd.url_text) LIKE :like ESCAPE '\' OR lower(ssd.file_path_text) LIKE :like ESCAPE '\' OR lower(ssd.file_path_text) LIKE :loose_like ESCAPE '\' OR lower(ssd.historical_app_names) LIKE :like ESCAPE '\' OR lower(ssd.historical_app_bundle_ids) LIKE :like ESCAPE '\' THEN 3 ELSE 1 END match_tier,
  (CASE WHEN lower(ssd.native_text) LIKE :like ESCAPE '\' THEN 1 ELSE 0 END+CASE WHEN lower(ssd.preview_text) LIKE :like ESCAPE '\' THEN 1 ELSE 0 END+CASE WHEN lower(ssd.ocr_text) LIKE :like ESCAPE '\' THEN 1 ELSE 0 END+CASE WHEN lower(ssd.url_text) LIKE :like ESCAPE '\' THEN 1 ELSE 0 END+CASE WHEN lower(ssd.file_path_text) LIKE :like ESCAPE '\' OR lower(ssd.file_path_text) LIKE :loose_like ESCAPE '\' THEN 1 ELSE 0 END+CASE WHEN lower(ssd.historical_app_names) LIKE :like ESCAPE '\' THEN 1 ELSE 0 END+CASE WHEN lower(ssd.historical_app_bundle_ids) LIKE :like ESCAPE '\' THEN 1 ELSE 0 END) detail_score,
  trim(CASE WHEN lower(ssd.native_text) LIKE :like ESCAPE '\' THEN 'native_text'||char(30)||'best_text'||char(30)||'search_text'||char(30) ELSE '' END||CASE WHEN lower(ssd.preview_text) LIKE :like ESCAPE '\' THEN 'preview_text'||char(30)||'best_text'||char(30) ELSE '' END||CASE WHEN lower(ssd.ocr_text) LIKE :like ESCAPE '\' THEN 'ocr_text'||char(30) ELSE '' END||CASE WHEN lower(ssd.url_text) LIKE :like ESCAPE '\' THEN 'urls'||char(30) ELSE '' END||CASE WHEN lower(ssd.file_path_text) LIKE :like ESCAPE '\' OR lower(ssd.file_path_text) LIKE :loose_like ESCAPE '\' THEN 'file_paths'||char(30) ELSE '' END||CASE WHEN lower(ssd.historical_app_names) LIKE :like ESCAPE '\' THEN 'historical_app'||char(30)||'app_name'||char(30) ELSE '' END||CASE WHEN lower(ssd.historical_app_bundle_ids) LIKE :like ESCAPE '\' THEN 'historical_app_bundle_id'||char(30)||'app_bundle_id'||char(30) ELSE '' END,char(30)) matched_fields
 FROM snapshot_search_documents ssd JOIN snapshots s ON s.id=ssd.snapshot_id LEFT JOIN snapshot_event_filter_cache se ON se.snapshot_id=s.id
 WHERE (lower(ssd.native_text) LIKE :loose_like ESCAPE '\' OR lower(ssd.preview_text) LIKE :loose_like ESCAPE '\' OR lower(ssd.ocr_text) LIKE :loose_like ESCAPE '\' OR lower(ssd.url_text) LIKE :loose_like ESCAPE '\' OR lower(ssd.file_path_text) LIKE :loose_like ESCAPE '\' OR lower(ssd.historical_app_names) LIKE :loose_like ESCAPE '\' OR lower(ssd.historical_app_bundle_ids) LIKE :loose_like ESCAPE '\' OR lower(ssd.native_text) LIKE :like ESCAPE '\' OR lower(ssd.preview_text) LIKE :like ESCAPE '\' OR lower(ssd.ocr_text) LIKE :like ESCAPE '\' OR lower(ssd.url_text) LIKE :like ESCAPE '\' OR lower(ssd.file_path_text) LIKE :like ESCAPE '\' OR lower(ssd.historical_app_names) LIKE :like ESCAPE '\' OR lower(ssd.historical_app_bundle_ids) LIKE :like ESCAPE '\') AND {snapshot_filter} AND {event_filter}
)
SELECT s.id snapshot_id,ss.last_event_id event_id,s.sha256,s.snapshot_kind,COALESCE(NULLIF(s.preview_text,''),NULLIF(ssd.ocr_text,''),'') preview_text,s.search_text,CASE WHEN lower(ssd.url_text)=:query_lower THEN 'Exact URL match' WHEN lower(ssd.file_path_text) LIKE :loose_like ESCAPE '\' THEN 'Path fragment match in file paths' WHEN lower(ssd.historical_app_bundle_ids) LIKE :like ESCAPE '\' THEN 'Bundle ID match' WHEN lower(ssd.historical_app_names) LIKE :like ESCAPE '\' THEN 'App name match' WHEN c.match_tier=5 THEN 'Exact text match in best text' WHEN c.match_tier=4 THEN 'Prefix match in best text' WHEN c.match_tier=3 THEN 'Phrase match in best text' ELSE 'Query term match' END why_matched,c.matched_fields,ss.capture_count,ss.first_observed_at,ss.last_observed_at,ss.last_frontmost_app_name,ss.last_frontmost_app_bundle_id,ssd.url_text urls,ssd.file_path_text file_urls,s.total_bytes,s.item_count,(c.match_tier+c.detail_score/100.0) score,CAST(c.match_tier AS REAL) order_primary,CAST(c.detail_score AS REAL) order_secondary,CASE WHEN c.match_tier>=3 THEN 1 ELSE 0 END exact_phrase_match,ssd.native_text evidence_native_text,ssd.preview_text evidence_preview_text,ssd.ocr_text evidence_ocr_text,ssd.url_text evidence_url_text,ssd.file_path_text evidence_file_path_text,ssd.historical_app_names evidence_app_names,ssd.historical_app_bundle_ids evidence_app_bundle_ids
FROM candidates c JOIN snapshot_search_documents ssd ON ssd.snapshot_id=c.snapshot_id JOIN snapshots s ON s.id=c.snapshot_id JOIN snapshot_stats ss ON ss.snapshot_id=c.snapshot_id
WHERE (:cursor_primary IS NULL OR c.match_tier<:cursor_primary OR (c.match_tier=:cursor_primary AND c.detail_score<:cursor_secondary) OR (c.match_tier=:cursor_primary AND c.detail_score=:cursor_secondary AND (ss.last_observed_at<:cursor_last_seen_at OR (ss.last_observed_at=:cursor_last_seen_at AND s.id<:cursor_snapshot_id))))
ORDER BY c.match_tier DESC,c.detail_score DESC,ss.last_observed_at DESC,s.id DESC LIMIT :limit",
        snapshot_filter = snapshot_filter_clause("s", "s.id"),
        event_filter = event_filter_where_clause("s.id", "se", use_event_cache, temporal)
    )
}

pub(in crate::db) fn recent_query(
    include_matching_events: bool,
    use_snapshot_event_cache: bool,
    use_snapshot_stats_since_filter: bool,
) -> String {
    format!(
        "{matching_events_cte}
         SELECT
             base.snapshot_id,
             base.event_id,
             base.sha256,
             base.snapshot_kind,
             base.preview_text,
             base.search_text,
             base.why_matched,
             base.matched_fields,
             base.capture_count,
             base.first_observed_at,
             base.last_observed_at,
             base.last_frontmost_app_name,
             base.last_frontmost_app_bundle_id,
             base.urls,
             base.file_urls,
             base.total_bytes,
             base.item_count,
             base.score
         FROM (
             SELECT
                 s.id AS snapshot_id,
                 ss.last_event_id AS event_id,
                 s.sha256 AS sha256,
                 s.snapshot_kind AS snapshot_kind,
                 COALESCE(NULLIF(s.preview_text, ''), soc.ocr_text, '') AS preview_text,
                 s.search_text AS search_text,
                 NULL AS why_matched,
                 '' AS matched_fields,
                 ss.capture_count AS capture_count,
                 ss.first_observed_at AS first_observed_at,
                 ss.last_observed_at AS last_observed_at,
                 ss.last_frontmost_app_name AS last_frontmost_app_name,
                 ss.last_frontmost_app_bundle_id AS last_frontmost_app_bundle_id,
                 COALESCE(sp.urls, '') AS urls,
                 COALESCE(sp.file_urls, '') AS file_urls,
                 s.total_bytes AS total_bytes,
                 s.item_count AS item_count,
                 NULL AS score
             FROM snapshots s
             JOIN snapshot_stats ss ON ss.snapshot_id = s.id
             {matching_events_join}
             LEFT JOIN snapshot_projection_cache sp ON sp.snapshot_id = s.id
             LEFT JOIN snapshot_ocr_cache soc ON soc.snapshot_id = s.id
             LEFT JOIN snapshot_event_filter_cache se ON se.snapshot_id = s.id
             WHERE {snapshot_filter_clause}
               AND {base_event_filter_clause}
               AND {snapshot_stats_since_filter_clause}
         ) base
         WHERE (
             :cursor_last_seen_at IS NULL
             OR base.last_observed_at < :cursor_last_seen_at
             OR (base.last_observed_at = :cursor_last_seen_at AND base.snapshot_id < :cursor_snapshot_id)
         )
         ORDER BY base.last_observed_at DESC, base.snapshot_id DESC
         LIMIT :limit",
        matching_events_cte = matching_events_cte(include_matching_events),
        matching_events_join = matching_events_join(include_matching_events),
        base_event_filter_clause = base_event_filter_clause(
            "se",
            include_matching_events,
            use_snapshot_event_cache,
        ),
        snapshot_stats_since_filter_clause =
            snapshot_stats_since_filter_clause(use_snapshot_stats_since_filter),
        snapshot_filter_clause = snapshot_filter_clause("s", "s.id"),
    )
}

pub(in crate::db) fn timeline_query(sort: TimelineSort) -> String {
    let cursor_predicate = match sort {
        TimelineSort::Desc => {
            r"(
                :cursor_observed_at IS NULL
                OR ce.observed_at < :cursor_observed_at
                OR (ce.observed_at = :cursor_observed_at AND ce.id < :cursor_event_id)
            )"
        }
        TimelineSort::Asc => {
            r"(
                :cursor_observed_at IS NULL
                OR ce.observed_at > :cursor_observed_at
                OR (ce.observed_at = :cursor_observed_at AND ce.id > :cursor_event_id)
            )"
        }
    };
    let ordering = match sort {
        TimelineSort::Desc => "ce.observed_at DESC, ce.id DESC",
        TimelineSort::Asc => "ce.observed_at ASC, ce.id ASC",
    };

    format!(
        r"
         SELECT
             ce.id AS event_id,
             ce.snapshot_id AS snapshot_id,
             ce.observed_at AS observed_at,
             ce.change_count AS change_count,
             s.sha256 AS sha256,
             s.snapshot_kind AS snapshot_kind,
             COALESCE(NULLIF(s.preview_text, ''), NULLIF(soc.ocr_text, ''), s.search_text, '') AS best_text,
             COALESCE(NULLIF(s.preview_text, ''), soc.ocr_text, '') AS preview_text,
             ce.frontmost_app_name AS frontmost_app_name,
             ce.frontmost_app_bundle_id AS frontmost_app_bundle_id,
             COALESCE(sp.urls, '') AS urls,
             COALESCE(sp.file_urls, '') AS file_urls,
             s.total_bytes AS total_bytes,
             s.item_count AS item_count
         FROM capture_events ce
         JOIN snapshots s ON s.id = ce.snapshot_id
         LEFT JOIN snapshot_projection_cache sp ON sp.snapshot_id = ce.snapshot_id
         LEFT JOIN snapshot_ocr_cache soc ON soc.snapshot_id = ce.snapshot_id
         WHERE {event_filter_clause}
           AND {snapshot_filter_clause}
           AND {cursor_predicate}
         ORDER BY {ordering}
         LIMIT :limit",
        event_filter_clause = event_filter_clause("ce"),
        snapshot_filter_clause = snapshot_filter_clause("s", "ce.snapshot_id"),
    )
}

#[allow(dead_code)]
pub(in crate::db) fn literal_query(
    include_matching_events: bool,
    use_snapshot_event_cache: bool,
    use_literal_fts: bool,
) -> String {
    let mut ctes = Vec::new();
    let literal_candidates_join = if use_literal_fts {
        ctes.push(
            "literal_candidates AS (
                 SELECT rowid AS snapshot_id
                 FROM snapshots_literal_fts
                 WHERE snapshots_literal_fts MATCH :literal_match
             )"
            .to_string(),
        );
        "JOIN literal_candidates lc ON lc.snapshot_id = s.id"
    } else {
        ""
    };
    if include_matching_events {
        ctes.push(format!(
            "matching_events AS (
                 SELECT DISTINCT ce.snapshot_id
                 FROM capture_events ce
                 WHERE {}
             )",
            event_filter_clause("ce")
        ));
    }
    let with_clause = if ctes.is_empty() {
        String::new()
    } else {
        format!("WITH {}", ctes.join(", "))
    };
    format!(
        "{with_clause}
         SELECT
             base.snapshot_id,
             base.event_id,
             base.sha256,
             base.snapshot_kind,
             base.preview_text,
             base.search_text,
             base.why_matched,
             base.matched_fields,
             base.capture_count,
             base.first_observed_at,
             base.last_observed_at,
             base.last_frontmost_app_name,
             base.last_frontmost_app_bundle_id,
             base.urls,
             base.file_urls,
             base.total_bytes,
             base.item_count,
             base.score
         FROM (
             SELECT
                 s.id AS snapshot_id,
                 ss.last_event_id AS event_id,
                 s.sha256 AS sha256,
                 s.snapshot_kind AS snapshot_kind,
                 s.preview_text AS preview_text,
                 s.search_text AS search_text,
                 CASE
                     WHEN lower(COALESCE(sp.urls, '')) = :query_lower THEN 'Exact URL match'
                     WHEN :path_fragment_like IS NOT NULL AND lower(COALESCE(sp.file_urls, '')) LIKE :path_fragment_like ESCAPE '\\' THEN 'Path fragment match in file paths'
                     WHEN lower(COALESCE(sp.file_urls, '')) LIKE :file_url_like ESCAPE '\\' THEN 'File name match in file paths'
                     WHEN lower(COALESCE(ss.last_frontmost_app_bundle_id, '')) = :query_lower THEN 'Bundle ID match'
                     WHEN :exact_phrase_lower IS NOT NULL AND lower(COALESCE(NULLIF(s.preview_text, ''), s.search_text, '')) LIKE ('%' || :exact_phrase_lower || '%') ESCAPE '\\' THEN 'Exact phrase match in best text'
                     WHEN lower(COALESCE(NULLIF(s.preview_text, ''), s.search_text, '')) = :query_lower THEN 'Exact text match in best text'
                     WHEN lower(COALESCE(NULLIF(s.preview_text, ''), s.search_text, '')) LIKE :prefix_like ESCAPE '\\' THEN 'Prefix match in best text'
                     WHEN lower(COALESCE(NULLIF(s.preview_text, ''), s.search_text, '')) LIKE :like ESCAPE '\\' THEN 'Literal text match in best text'
                     WHEN lower(COALESCE(s.search_text, '')) LIKE :like ESCAPE '\\' THEN 'Literal search-text match'
                     WHEN lower(COALESCE(NULLIF(s.preview_text, ''), s.search_text, '')) LIKE :loose_like ESCAPE '\\' THEN 'Ordered term match in best text'
                     WHEN lower(COALESCE(s.search_text, '')) LIKE :loose_like ESCAPE '\\' THEN 'Ordered term match in search text'
                     WHEN lower(COALESCE(ss.last_frontmost_app_name, '')) LIKE :like ESCAPE '\\' THEN 'App name match'
                     ELSE 'Literal field match'
                 END AS why_matched,
                 TRIM(
                     CASE WHEN lower(COALESCE(NULLIF(s.preview_text, ''), '')) LIKE :like ESCAPE '\\' THEN 'best_text' || char(30) ELSE '' END ||
                     CASE WHEN lower(COALESCE(s.preview_text, '')) LIKE :like ESCAPE '\\' THEN 'preview_text' || char(30) ELSE '' END ||
                     CASE WHEN lower(COALESCE(s.search_text, '')) LIKE :like ESCAPE '\\' THEN 'search_text' || char(30) ELSE '' END ||
                     CASE WHEN lower(COALESCE(NULLIF(s.preview_text, ''), s.search_text, '')) LIKE :loose_like ESCAPE '\\' THEN 'best_text' || char(30) ELSE '' END ||
                     CASE WHEN lower(COALESCE(s.search_text, '')) LIKE :loose_like ESCAPE '\\' THEN 'search_text' || char(30) ELSE '' END ||
                     CASE WHEN lower(COALESCE(sp.urls, '')) LIKE :like ESCAPE '\\' OR lower(COALESCE(sp.urls, '')) = :query_lower THEN 'urls' || char(30) ELSE '' END ||
                     CASE WHEN (:path_fragment_like IS NOT NULL AND lower(COALESCE(sp.file_urls, '')) LIKE :path_fragment_like ESCAPE '\\') OR lower(COALESCE(sp.file_urls, '')) LIKE :file_url_like ESCAPE '\\' THEN 'file_paths' || char(30) ELSE '' END ||
                     CASE WHEN lower(COALESCE(ss.last_frontmost_app_name, '')) LIKE :like ESCAPE '\\' THEN 'app_name' || char(30) ELSE '' END ||
                     CASE WHEN lower(COALESCE(ss.last_frontmost_app_bundle_id, '')) LIKE :like ESCAPE '\\' OR lower(COALESCE(ss.last_frontmost_app_bundle_id, '')) = :query_lower THEN 'app_bundle_id' || char(30) ELSE '' END,
                     char(30)
                 ) AS matched_fields,
                 ss.capture_count AS capture_count,
                 ss.first_observed_at AS first_observed_at,
                 ss.last_observed_at AS last_observed_at,
                 ss.last_frontmost_app_name AS last_frontmost_app_name,
                 ss.last_frontmost_app_bundle_id AS last_frontmost_app_bundle_id,
                 COALESCE(sp.urls, '') AS urls,
                 COALESCE(sp.file_urls, '') AS file_urls,
                 s.total_bytes AS total_bytes,
                 s.item_count AS item_count,
                 (
                     CASE WHEN lower(COALESCE(sp.urls, '')) = :query_lower THEN 1.16 ELSE 0 END +
                     CASE WHEN :path_fragment_like IS NOT NULL AND lower(COALESCE(sp.file_urls, '')) LIKE :path_fragment_like ESCAPE '\\' THEN 1.12 ELSE 0 END +
                     CASE WHEN lower(COALESCE(sp.file_urls, '')) LIKE :file_url_like ESCAPE '\\' THEN 0.98 ELSE 0 END +
                     CASE WHEN lower(COALESCE(ss.last_frontmost_app_bundle_id, '')) = :query_lower THEN 1.08 ELSE 0 END +
                     CASE WHEN :exact_phrase_lower IS NOT NULL AND lower(COALESCE(NULLIF(s.preview_text, ''), s.search_text, '')) LIKE ('%' || :exact_phrase_lower || '%') ESCAPE '\\' THEN 0.98 ELSE 0 END +
                     CASE WHEN lower(COALESCE(NULLIF(s.preview_text, ''), s.search_text, '')) = :query_lower THEN 0.96 ELSE 0 END +
                     CASE WHEN lower(COALESCE(NULLIF(s.preview_text, ''), s.search_text, '')) LIKE :prefix_like ESCAPE '\\' THEN 0.88 ELSE 0 END +
                     CASE WHEN lower(COALESCE(NULLIF(s.preview_text, ''), s.search_text, '')) LIKE :like ESCAPE '\\' THEN 0.78 ELSE 0 END +
                     CASE WHEN lower(COALESCE(s.search_text, '')) LIKE :like ESCAPE '\\' THEN 0.72 ELSE 0 END +
                     CASE WHEN lower(COALESCE(sp.urls, '')) LIKE :like ESCAPE '\\' THEN 0.9 ELSE 0 END +
                     CASE WHEN lower(COALESCE(sp.urls, '')) LIKE :like ESCAPE '\\' THEN -MIN(length(COALESCE(sp.urls, '')), 120) * 0.001 ELSE 0 END +
                     CASE WHEN lower(COALESCE(NULLIF(s.preview_text, ''), s.search_text, '')) LIKE :loose_like ESCAPE '\\' THEN 0.7 ELSE 0 END +
                     CASE WHEN lower(COALESCE(s.search_text, '')) LIKE :loose_like ESCAPE '\\' THEN 0.66 ELSE 0 END +
                     CASE WHEN lower(COALESCE(sp.urls, '')) LIKE :prefix_like ESCAPE '\\' THEN 0.08 ELSE 0 END +
                     CASE WHEN lower(COALESCE(sp.urls, '')) LIKE :prefix_like ESCAPE '\\' THEN -MIN(length(COALESCE(sp.urls, '')), 120) * 0.001 ELSE 0 END +
                     CASE WHEN lower(COALESCE(ss.last_frontmost_app_bundle_id, '')) LIKE :like ESCAPE '\\' THEN 0.84 ELSE 0 END +
                     CASE WHEN lower(COALESCE(ss.last_frontmost_app_name, '')) LIKE :like ESCAPE '\\' THEN 0.7 ELSE 0 END +
                     CASE
                         WHEN datetime(ss.last_observed_at) >= datetime('now', '-24 hours') THEN 0.05
                         WHEN datetime(ss.last_observed_at) >= datetime('now', '-7 days') THEN 0.02
                         ELSE 0
                     END
                 ) AS score
             FROM snapshots s
             {literal_candidates_join}
             JOIN snapshot_stats ss ON ss.snapshot_id = s.id
             {matching_events_join}
             LEFT JOIN snapshot_projection_cache sp ON sp.snapshot_id = s.id
             LEFT JOIN snapshot_event_filter_cache se ON se.snapshot_id = s.id
             WHERE (
                   lower(COALESCE(s.search_text, '')) LIKE :like ESCAPE '\\'
                 OR lower(COALESCE(s.preview_text, '')) LIKE :like ESCAPE '\\'
                 OR lower(COALESCE(s.search_text, '')) LIKE :loose_like ESCAPE '\\'
                 OR lower(COALESCE(s.preview_text, '')) LIKE :loose_like ESCAPE '\\'
                 OR lower(COALESCE(sp.urls, '')) LIKE :like ESCAPE '\\'
                 OR lower(COALESCE(sp.file_urls, '')) LIKE :file_url_like ESCAPE '\\'
                 OR lower(COALESCE(ss.last_frontmost_app_name, '')) LIKE :like ESCAPE '\\'
                 OR lower(COALESCE(ss.last_frontmost_app_bundle_id, '')) LIKE :like ESCAPE '\\'
                 OR (:path_fragment_like IS NOT NULL AND lower(COALESCE(sp.file_urls, '')) LIKE :path_fragment_like ESCAPE '\\')
               )
               AND {snapshot_filter_clause}
               AND {base_event_filter_clause}
               AND {literal_match_parameter_bindings_clause}
         ) base
         WHERE (
             :cursor_score IS NULL
             OR base.score < :cursor_score
             OR (
                 base.score = :cursor_score
                 AND (
                     base.last_observed_at < :cursor_last_seen_at
                     OR (base.last_observed_at = :cursor_last_seen_at AND base.snapshot_id < :cursor_snapshot_id)
                 )
             )
         )
         ORDER BY base.score DESC, base.last_observed_at DESC, base.snapshot_id DESC
         LIMIT :limit",
        with_clause = with_clause,
        literal_candidates_join = literal_candidates_join,
        matching_events_join = matching_events_join(include_matching_events),
        base_event_filter_clause = base_event_filter_clause(
            "se",
            include_matching_events,
            use_snapshot_event_cache,
        ),
        literal_match_parameter_bindings_clause =
            parameter_bindings_clause(&[":literal_match"]),
        snapshot_filter_clause = snapshot_filter_clause("s", "s.id"),
    )
}

#[allow(dead_code)]
pub(in crate::db) fn file_path_literal_query(
    include_matching_events: bool,
    use_snapshot_event_cache: bool,
    use_literal_fts: bool,
) -> String {
    let mut ctes = Vec::new();
    let path_candidates_join = if use_literal_fts {
        ctes.push(
            "path_candidates AS (
                 SELECT rowid AS snapshot_id
                 FROM snapshot_file_url_fts
                 WHERE snapshot_file_url_fts MATCH :literal_match
             )"
            .to_string(),
        );
        "JOIN path_candidates pc ON pc.snapshot_id = s.id"
    } else {
        ""
    };
    if include_matching_events {
        ctes.push(format!(
            "matching_events AS (
                 SELECT DISTINCT ce.snapshot_id
                 FROM capture_events ce
                 WHERE {}
             )",
            event_filter_clause("ce")
        ));
    }
    let with_clause = if ctes.is_empty() {
        String::new()
    } else {
        format!("WITH {}", ctes.join(", "))
    };

    format!(
        "{with_clause}
         SELECT
             base.snapshot_id,
             base.event_id,
             base.sha256,
             base.snapshot_kind,
             base.preview_text,
             base.search_text,
             base.why_matched,
             base.matched_fields,
             base.capture_count,
             base.first_observed_at,
             base.last_observed_at,
             base.last_frontmost_app_name,
             base.last_frontmost_app_bundle_id,
             base.urls,
             base.file_urls,
             base.total_bytes,
             base.item_count,
             base.score
         FROM (
             SELECT
                 s.id AS snapshot_id,
                 ss.last_event_id AS event_id,
                 s.sha256 AS sha256,
                 s.snapshot_kind AS snapshot_kind,
                 s.preview_text AS preview_text,
                 s.search_text AS search_text,
                 'Path fragment match in file paths' AS why_matched,
                 'file_paths' AS matched_fields,
                 ss.capture_count AS capture_count,
                 ss.first_observed_at AS first_observed_at,
                 ss.last_observed_at AS last_observed_at,
                 ss.last_frontmost_app_name AS last_frontmost_app_name,
                 ss.last_frontmost_app_bundle_id AS last_frontmost_app_bundle_id,
                 COALESCE(sp.urls, '') AS urls,
                 COALESCE(sp.file_urls, '') AS file_urls,
                 s.total_bytes AS total_bytes,
                 s.item_count AS item_count,
                 (
                     CASE
                         WHEN instr(lower(COALESCE(sp.file_urls, '')), :path_fragment) > 0
                             THEN 1.12
                         ELSE 0
                     END +
                     CASE
                         WHEN datetime(ss.last_observed_at) >= datetime('now', '-24 hours') THEN 0.05
                         WHEN datetime(ss.last_observed_at) >= datetime('now', '-7 days') THEN 0.02
                         ELSE 0
                     END
                 ) AS score
             FROM snapshots s
             {path_candidates_join}
             JOIN snapshot_stats ss ON ss.snapshot_id = s.id
             {matching_events_join}
             JOIN snapshot_projection_cache sp ON sp.snapshot_id = s.id
             LEFT JOIN snapshot_event_filter_cache se ON se.snapshot_id = s.id
             WHERE lower(COALESCE(sp.file_urls, '')) LIKE :path_like ESCAPE '\\'
               AND {snapshot_filter_clause}
               AND {base_event_filter_clause}
               AND {literal_match_parameter_bindings_clause}
         ) base
         WHERE (
             :cursor_score IS NULL
             OR base.score < :cursor_score
             OR (
                 base.score = :cursor_score
                 AND (
                     base.last_observed_at < :cursor_last_seen_at
                     OR (base.last_observed_at = :cursor_last_seen_at AND base.snapshot_id < :cursor_snapshot_id)
                 )
             )
         )
         ORDER BY base.score DESC, base.last_observed_at DESC, base.snapshot_id DESC
         LIMIT :limit",
        with_clause = with_clause,
        path_candidates_join = path_candidates_join,
        matching_events_join = matching_events_join(include_matching_events),
        snapshot_filter_clause = snapshot_filter_clause("s", "s.id"),
        base_event_filter_clause = base_event_filter_clause(
            "se",
            include_matching_events,
            use_snapshot_event_cache,
        ),
        literal_match_parameter_bindings_clause =
            parameter_bindings_clause(&[":literal_match"]),
    )
}

#[allow(dead_code)]
pub(in crate::db) fn fts_query(
    use_snapshot_event_cache: bool,
    has_temporal_event_filters: bool,
    use_simple_scoring: bool,
) -> String {
    let why_matched_expression = if use_simple_scoring {
        "NULL"
    } else {
        r"CASE
                     WHEN :exact_phrase_lower IS NOT NULL AND lower(COALESCE(NULLIF(s.preview_text, ''), s.search_text, '')) LIKE ('%' || :exact_phrase_lower || '%') ESCAPE '\'
                         THEN 'Exact phrase match in best text'
                     WHEN lower(COALESCE(NULLIF(s.preview_text, ''), s.search_text, '')) = :query_lower
                         THEN 'Exact text match in best text'
                     WHEN lower(COALESCE(NULLIF(s.preview_text, ''), s.search_text, '')) LIKE ('%' || :query_lower || '%') ESCAPE '\'
                         THEN 'Phrase match in best text'
                     ELSE NULL
                 END"
    };
    let matched_fields_expression = if use_simple_scoring {
        "'search_text'"
    } else {
        r"CASE
                     WHEN :exact_phrase_lower IS NOT NULL AND lower(COALESCE(NULLIF(s.preview_text, ''), s.search_text, '')) LIKE ('%' || :exact_phrase_lower || '%') ESCAPE '\'
                         THEN 'best_text' || char(30) || 'search_text'
                     WHEN lower(COALESCE(NULLIF(s.preview_text, ''), s.search_text, '')) LIKE ('%' || :query_lower || '%') ESCAPE '\'
                         THEN 'best_text' || char(30) || 'search_text'
                     ELSE 'search_text'
                 END"
    };
    let score_expression = if use_simple_scoring {
        "bm25(snapshots_fts)"
    } else {
        r"(
                     bm25(snapshots_fts)
                     - CASE
                         WHEN :exact_phrase_lower IS NOT NULL AND lower(COALESCE(NULLIF(s.preview_text, ''), s.search_text, '')) LIKE ('%' || :exact_phrase_lower || '%') ESCAPE '\' THEN 0.2
                         WHEN lower(COALESCE(NULLIF(s.preview_text, ''), s.search_text, '')) LIKE ('%' || :query_lower || '%') ESCAPE '\' THEN 0.08
                         ELSE 0
                     END
                 )"
    };

    format!(
        "
         WITH search_rows AS (
             SELECT
                 s.id AS snapshot_id,
                 ss.last_event_id AS event_id,
                 s.sha256 AS sha256,
                 s.snapshot_kind AS snapshot_kind,
                 s.preview_text AS preview_text,
                 s.search_text AS search_text,
                 {why_matched_expression} AS why_matched,
                 {matched_fields_expression} AS matched_fields,
                 ss.capture_count AS capture_count,
                 ss.first_observed_at AS first_observed_at,
                 ss.last_observed_at AS last_observed_at,
                 ss.last_frontmost_app_name AS last_frontmost_app_name,
                 ss.last_frontmost_app_bundle_id AS last_frontmost_app_bundle_id,
                 s.total_bytes AS total_bytes,
                 s.item_count AS item_count,
                 {score_expression} AS score
             FROM snapshots_fts
             JOIN snapshots s ON s.id = snapshots_fts.rowid
             JOIN snapshot_stats ss ON ss.snapshot_id = s.id
             LEFT JOIN snapshot_event_filter_cache se ON se.snapshot_id = s.id
             WHERE snapshots_fts MATCH :query
               AND {search_parameter_bindings_clause}
               AND {snapshot_filter_clause}
               AND {event_filter_where_clause}
         )
         , ranked_rows AS (
             SELECT
                 search_rows.snapshot_id,
                 search_rows.event_id,
                 search_rows.sha256,
                 search_rows.snapshot_kind,
                 search_rows.preview_text,
                 search_rows.search_text,
                 search_rows.why_matched,
                 search_rows.matched_fields,
                 search_rows.capture_count,
                 search_rows.first_observed_at,
                 search_rows.last_observed_at,
                 search_rows.last_frontmost_app_name,
                 search_rows.last_frontmost_app_bundle_id,
                 search_rows.total_bytes,
                 search_rows.item_count,
                 search_rows.score
             FROM search_rows
             WHERE (
                 :cursor_score IS NULL
                 OR search_rows.score > :cursor_score
                 OR (
                     search_rows.score = :cursor_score
                     AND (
                         search_rows.last_observed_at < :cursor_last_seen_at
                         OR (
                             search_rows.last_observed_at = :cursor_last_seen_at
                             AND search_rows.snapshot_id < :cursor_snapshot_id
                         )
                     )
                 )
             )
             ORDER BY search_rows.score ASC, search_rows.last_observed_at DESC, search_rows.snapshot_id DESC
             LIMIT :limit
         )
         SELECT
             ranked_rows.snapshot_id,
             ranked_rows.event_id,
             ranked_rows.sha256,
             ranked_rows.snapshot_kind,
             ranked_rows.preview_text,
             ranked_rows.search_text,
             COALESCE(
                 ranked_rows.why_matched,
                 snippet(snapshots_fts, 0, '⟦', '⟧', ' … ', 24),
                 ranked_rows.preview_text
             ) AS why_matched,
             ranked_rows.matched_fields,
             ranked_rows.capture_count,
             ranked_rows.first_observed_at,
             ranked_rows.last_observed_at,
             ranked_rows.last_frontmost_app_name,
             ranked_rows.last_frontmost_app_bundle_id,
             COALESCE(sp.urls, '') AS urls,
             COALESCE(sp.file_urls, '') AS file_urls,
             ranked_rows.total_bytes,
             ranked_rows.item_count,
             ranked_rows.score
         FROM ranked_rows
         JOIN snapshots_fts ON snapshots_fts.rowid = ranked_rows.snapshot_id
         LEFT JOIN snapshot_projection_cache sp ON sp.snapshot_id = ranked_rows.snapshot_id
         ORDER BY ranked_rows.score ASC, ranked_rows.last_observed_at DESC, ranked_rows.snapshot_id DESC",
        event_filter_where_clause = event_filter_where_clause(
            "s.id",
            "se",
            use_snapshot_event_cache,
            has_temporal_event_filters,
        ),
        snapshot_filter_clause = snapshot_filter_clause("s", "s.id"),
        search_parameter_bindings_clause =
            parameter_bindings_clause(&[":query_lower", ":exact_phrase_lower"]),
        why_matched_expression = why_matched_expression,
        matched_fields_expression = matched_fields_expression,
        score_expression = score_expression,
    )
}

#[allow(dead_code)]
pub(in crate::db) fn ocr_fts_query(
    use_snapshot_event_cache: bool,
    has_temporal_event_filters: bool,
) -> String {
    format!(
        "
         SELECT
             s.id AS snapshot_id,
             ss.last_event_id AS event_id,
             s.sha256 AS sha256,
             s.snapshot_kind AS snapshot_kind,
             COALESCE(NULLIF(s.preview_text, ''), soc.ocr_text, '') AS preview_text,
             s.search_text AS search_text,
             COALESCE(snippet(snapshot_ocr_fts, 0, '⟦', '⟧', ' … ', 24), 'OCR text match') AS why_matched,
             'ocr_text' AS matched_fields,
             ss.capture_count AS capture_count,
             ss.first_observed_at AS first_observed_at,
             ss.last_observed_at AS last_observed_at,
             ss.last_frontmost_app_name AS last_frontmost_app_name,
             ss.last_frontmost_app_bundle_id AS last_frontmost_app_bundle_id,
             COALESCE(sp.urls, '') AS urls,
             COALESCE(sp.file_urls, '') AS file_urls,
             s.total_bytes AS total_bytes,
             s.item_count AS item_count,
             bm25(snapshot_ocr_fts) AS score
         FROM snapshot_ocr_fts
         JOIN snapshot_ocr_cache soc ON soc.snapshot_id = snapshot_ocr_fts.rowid
         JOIN snapshots s ON s.id = snapshot_ocr_fts.rowid
         JOIN snapshot_stats ss ON ss.snapshot_id = s.id
         LEFT JOIN snapshot_projection_cache sp ON sp.snapshot_id = s.id
         LEFT JOIN snapshot_event_filter_cache se ON se.snapshot_id = s.id
         WHERE snapshot_ocr_fts MATCH :query
           AND soc.ocr_text != ''
           AND {snapshot_filter_clause}
           AND {event_filter_where_clause}
           AND (
               :cursor_score IS NULL
               OR bm25(snapshot_ocr_fts) > :cursor_score
               OR (
                   bm25(snapshot_ocr_fts) = :cursor_score
                   AND (
                       ss.last_observed_at < :cursor_last_seen_at
                       OR (ss.last_observed_at = :cursor_last_seen_at AND s.id < :cursor_snapshot_id)
                   )
               )
           )
         ORDER BY score ASC, ss.last_observed_at DESC, s.id DESC
         LIMIT :limit",
        event_filter_where_clause = event_filter_where_clause(
            "s.id",
            "se",
            use_snapshot_event_cache,
            has_temporal_event_filters,
        ),
        snapshot_filter_clause = snapshot_filter_clause("s", "s.id"),
    )
}

#[allow(dead_code)]
pub(in crate::db) fn ocr_literal_query(
    include_matching_events: bool,
    use_snapshot_event_cache: bool,
    use_literal_fts: bool,
) -> String {
    let mut ctes = Vec::new();
    let literal_candidates_join = if use_literal_fts {
        ctes.push(
            "ocr_literal_candidates AS (
                 SELECT rowid AS snapshot_id
                 FROM snapshot_ocr_literal_fts
                 WHERE snapshot_ocr_literal_fts MATCH :literal_match
             )"
            .to_string(),
        );
        "JOIN ocr_literal_candidates olc ON olc.snapshot_id = s.id"
    } else {
        ""
    };
    if include_matching_events {
        ctes.push(format!(
            "matching_events AS (
                 SELECT DISTINCT ce.snapshot_id
                 FROM capture_events ce
                 WHERE {}
             )",
            event_filter_clause("ce")
        ));
    }
    let with_clause = if ctes.is_empty() {
        String::new()
    } else {
        format!("WITH {}", ctes.join(", "))
    };
    format!(
        "{with_clause}
         SELECT
             s.id AS snapshot_id,
             ss.last_event_id AS event_id,
             s.sha256 AS sha256,
             s.snapshot_kind AS snapshot_kind,
             COALESCE(NULLIF(s.preview_text, ''), soc.ocr_text, '') AS preview_text,
             s.search_text AS search_text,
             CASE
                 WHEN :exact_phrase_lower IS NOT NULL AND lower(soc.ocr_text) LIKE ('%' || :exact_phrase_lower || '%') ESCAPE '\\' THEN 'Exact phrase match in OCR text'
                 WHEN lower(soc.ocr_text) = :query_lower THEN 'Exact OCR text match'
                 WHEN lower(soc.ocr_text) LIKE :prefix_like ESCAPE '\\' THEN 'OCR text prefix match'
                 ELSE 'OCR text match'
             END AS why_matched,
             'ocr_text' AS matched_fields,
             ss.capture_count AS capture_count,
             ss.first_observed_at AS first_observed_at,
             ss.last_observed_at AS last_observed_at,
             ss.last_frontmost_app_name AS last_frontmost_app_name,
             ss.last_frontmost_app_bundle_id AS last_frontmost_app_bundle_id,
             COALESCE(sp.urls, '') AS urls,
             COALESCE(sp.file_urls, '') AS file_urls,
             s.total_bytes AS total_bytes,
             s.item_count AS item_count,
             (
                 CASE WHEN :exact_phrase_lower IS NOT NULL AND lower(soc.ocr_text) LIKE ('%' || :exact_phrase_lower || '%') ESCAPE '\\' THEN 0.82 ELSE 0 END +
                 CASE WHEN lower(soc.ocr_text) = :query_lower THEN 0.8 ELSE 0 END +
                 CASE WHEN lower(soc.ocr_text) LIKE :prefix_like ESCAPE '\\' THEN 0.74 ELSE 0 END +
                 CASE WHEN lower(soc.ocr_text) LIKE :like ESCAPE '\\' THEN 0.68 ELSE 0 END +
                 CASE
                     WHEN datetime(ss.last_observed_at) >= datetime('now', '-24 hours') THEN 0.05
                     WHEN datetime(ss.last_observed_at) >= datetime('now', '-7 days') THEN 0.02
                     ELSE 0
                 END
             ) AS score
         FROM snapshot_ocr_cache soc
         JOIN snapshots s ON s.id = soc.snapshot_id
         {literal_candidates_join}
         JOIN snapshot_stats ss ON ss.snapshot_id = s.id
         {matching_events_join}
         LEFT JOIN snapshot_projection_cache sp ON sp.snapshot_id = s.id
         LEFT JOIN snapshot_event_filter_cache se ON se.snapshot_id = s.id
         WHERE soc.ocr_text != ''
           AND lower(soc.ocr_text) LIKE :like ESCAPE '\\'
           AND {literal_match_parameter_bindings_clause}
           AND {snapshot_filter_clause}
           AND {base_event_filter_clause}
           AND (
               :cursor_score IS NULL
               OR score < :cursor_score
               OR (
                   score = :cursor_score
                   AND (
                       ss.last_observed_at < :cursor_last_seen_at
                       OR (ss.last_observed_at = :cursor_last_seen_at AND s.id < :cursor_snapshot_id)
                   )
               )
           )
         ORDER BY score DESC, ss.last_observed_at DESC, s.id DESC
         LIMIT :limit",
        with_clause = with_clause,
        literal_candidates_join = literal_candidates_join,
        matching_events_join = matching_events_join(include_matching_events),
        base_event_filter_clause = base_event_filter_clause(
            "se",
            include_matching_events,
            use_snapshot_event_cache,
        ),
        literal_match_parameter_bindings_clause =
            parameter_bindings_clause(&[":literal_match"]),
        snapshot_filter_clause = snapshot_filter_clause("s", "s.id"),
    )
}
