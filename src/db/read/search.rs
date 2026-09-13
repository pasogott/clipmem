use anyhow::{Context, Result};
use rusqlite::{named_params, params, Error as SqlError, Result as SqlResult, Row};

use super::filter_sql::{
    app_like_pattern, can_use_snapshot_event_cache, effective_since_param, escape_like_pattern,
    has_temporal_event_filters,
};
use super::queries::{unified_fts_query, unified_literal_query};
use super::query_analysis::{analyze_query, is_invalid_fts_query, is_simple_fts_query};
use super::row_mapping::map_search_hit_row;
use super::search_results::paginate_rows;
use crate::db::core::clamp_result_limit;
use crate::db::sqlite_helpers::{collect_rows, usize_to_i64};
use crate::db::types::{Database, RetrievalFilters, SearchCursorState, SearchMode, SearchResults};
use crate::model::{MatchEvidence, SearchHit, SearchOrderKey};

struct SearchExecutionParams<'a> {
    fetch_limit: i64,
    app_like: Option<String>,
    bundle_id: Option<String>,
    kind: Option<&'a str>,
    since: Option<String>,
    until: Option<&'a str>,
    has_text: bool,
    has_url: bool,
    has_file_url: bool,
    has_image: bool,
    has_pdf: bool,
    min_bytes: Option<i64>,
    max_bytes: Option<i64>,
    cursor_last_seen_at: Option<&'a str>,
    cursor_snapshot_id: Option<i64>,
    cursor_primary: Option<f64>,
    cursor_secondary: Option<f64>,
    cursor_exact: bool,
}

impl<'a> SearchExecutionParams<'a> {
    fn new(
        limit: usize,
        filters: &'a RetrievalFilters,
        cursor: Option<&'a SearchCursorState>,
    ) -> Result<Self> {
        Ok(Self {
            fetch_limit: usize_to_i64(limit.saturating_add(1))?,
            app_like: app_like_pattern(filters),
            bundle_id: filters.bundle_id().map(|value| value.to_ascii_lowercase()),
            kind: filters.kind().map(|value| value.as_str()),
            since: effective_since_param(filters)?,
            until: filters.until(),
            has_text: filters.has_text(),
            has_url: filters.has_url(),
            has_file_url: filters.has_file_url(),
            has_image: filters.has_image(),
            has_pdf: filters.has_pdf(),
            min_bytes: filters.min_bytes().map(usize_to_i64).transpose()?,
            max_bytes: filters.max_bytes().map(usize_to_i64).transpose()?,
            cursor_last_seen_at: cursor.map(SearchCursorState::last_seen_at),
            cursor_snapshot_id: cursor.map(SearchCursorState::snapshot_id),
            cursor_primary: cursor.and_then(SearchCursorState::order_primary),
            cursor_secondary: cursor.and_then(SearchCursorState::order_secondary),
            cursor_exact: cursor.is_some_and(SearchCursorState::exact_phrase),
        })
    }
}

fn collect_search_results<I>(
    rows: I,
    limit: usize,
    mode: SearchMode,
    context: &'static str,
) -> Result<SearchResults>
where
    I: Iterator<Item = SqlResult<SearchHit>>,
{
    collect_rows(rows)
        .map(|hits| paginate_rows(hits, limit))
        .map(|page| {
            let has_more = page.has_more();
            SearchResults::new(mode, page.into_items(), has_more)
        })
        .with_context(|| format!("collect {context} search rows"))
}

fn map_unified_search_hit_row(
    row: &Row<'_>,
    query: &str,
    simple_query: bool,
) -> SqlResult<SearchHit> {
    let hit = map_search_hit_row(row, true)?;
    let exact_phrase = row.get::<_, i64>("exact_phrase_match")? != 0;
    let primary = row.get("order_primary")?;
    let secondary = row.get("order_secondary")?;
    let documents = [
        row.get::<_, String>("evidence_native_text")?,
        row.get::<_, String>("evidence_preview_text")?,
        row.get::<_, String>("evidence_ocr_text")?,
        row.get::<_, String>("evidence_url_text")?,
        row.get::<_, String>("evidence_file_path_text")?,
        row.get::<_, String>("evidence_app_names")?,
        row.get::<_, String>("evidence_app_bundle_ids")?,
    ];
    let terms = normalized_query_terms(query);
    let (term_coverage, quality) = if simple_query && !terms.is_empty() {
        let (coverage, quality) = evidence_match_quality(&terms, &documents, exact_phrase);
        (Some(coverage), Some(quality))
    } else {
        (None, None)
    };
    let snippet_source = match hit.why_matched() {
        Some("Exact phrase match in OCR text") => Some("ocr_text".to_string()),
        Some(
            "Exact phrase match in best text"
            | "Exact text match in best text"
            | "Prefix match in best text"
            | "Phrase match in best text",
        ) => Some("native_text".to_string()),
        Some("Exact URL match") => Some("urls".to_string()),
        Some("Path fragment match in file paths") => Some("file_paths".to_string()),
        Some("Bundle ID match") => Some("historical_app_bundle_id".to_string()),
        Some("App name match") => Some("historical_app".to_string()),
        _ => hit.matched_fields().first().cloned(),
    };
    let snippet_text = hit.why_matched().map(ToOwned::to_owned);
    let evidence = MatchEvidence::new(
        hit.matched_fields().to_vec(),
        exact_phrase,
        term_coverage,
        snippet_source,
        snippet_text,
        if simple_query {
            "simple_lexical"
        } else {
            "complex_fts"
        },
        quality,
    );
    Ok(hit.with_search_evidence(
        evidence,
        SearchOrderKey {
            primary,
            secondary,
            exact_phrase,
        },
    ))
}

pub(in crate::db) fn evidence_match_quality(
    terms: &[String],
    documents: &[String],
    exact_phrase: bool,
) -> (f64, f64) {
    let terms_in_one_field = documents.iter().any(|document| {
        let lower = document.to_lowercase();
        terms.iter().all(|term| lower.contains(term))
    });
    let combined = documents.join("\n").to_lowercase();
    let matched = terms
        .iter()
        .filter(|term| combined.contains(term.as_str()))
        .count();
    let coverage = matched as f64 / terms.len() as f64;
    let quality = if exact_phrase {
        0.96
    } else if terms_in_one_field {
        0.86
    } else if coverage == 1.0 {
        0.80
    } else if coverage >= 0.60 {
        0.68
    } else {
        0.50
    };
    (coverage, quality)
}

fn normalized_query_terms(query: &str) -> Vec<String> {
    query
        .split(|character: char| !character.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|term| !term.is_empty())
        .collect()
}

impl Database {
    pub(crate) fn latest_capture_observed_at(&self) -> Result<Option<String>> {
        let observed_at = self
            .conn
            .query_row("SELECT MAX(observed_at) FROM capture_events", [], |row| {
                row.get::<_, Option<String>>(0)
            })
            .context("read latest capture timestamp")?;
        Ok(observed_at)
    }

    pub(crate) fn has_capture_within_hours(&self, hours: u32) -> Result<bool> {
        let sql = "SELECT EXISTS(
                SELECT 1
                FROM capture_events
                WHERE datetime(observed_at) >= datetime('now', printf('-%d hours', ?1))
            )";
        self.conn
            .query_row(sql, params![i64::from(hours)], |row| row.get::<_, i64>(0))
            .map(|value| value != 0)
            .context("read capture freshness")
    }

    /// Search stored snapshots in automatic mode.
    ///
    /// Automatic mode prefers FTS5, but falls back to literal `LIKE` matching for wildcard-like
    /// input, invalid FTS syntax, and empty FTS result sets.
    ///
    /// # Errors
    ///
    /// Returns an error if the query cannot be prepared or executed, or when `SQLite` returns a
    /// non-syntax FTS error.
    pub fn search_auto(
        &self,
        query: &str,
        limit: usize,
        filters: &RetrievalFilters,
    ) -> Result<SearchResults> {
        self.search_auto_page(query, limit, filters, None)
    }

    pub(crate) fn search_auto_page(
        &self,
        query: &str,
        limit: usize,
        filters: &RetrievalFilters,
        cursor: Option<&SearchCursorState>,
    ) -> Result<SearchResults> {
        let limit = clamp_result_limit(limit);
        let analysis = analyze_query(query);

        if analysis.literal_preferred {
            return self.search_literal_page(query, limit, filters, cursor);
        }

        if let Some(cursor) = cursor {
            return match cursor.mode_used() {
                SearchMode::Fts => self.search_fts_page(query, limit, filters, Some(cursor)),
                SearchMode::Literal => {
                    self.search_literal_page(query, limit, filters, Some(cursor))
                }
                SearchMode::Auto => self.search_literal_page(query, limit, filters, Some(cursor)),
            };
        }

        let results = match self.search_fts_page(query, limit, filters, None) {
            Ok(results) => results,
            Err(error) => {
                if let Some(sqlite_error) = error.downcast_ref::<SqlError>() {
                    if is_invalid_fts_query(sqlite_error) {
                        self.search_literal_page(query, limit, filters, None)?
                    } else {
                        return Err(error);
                    }
                } else {
                    return Err(error);
                }
            }
        };

        if results.hits().is_empty() {
            self.search_literal_page(query, limit, filters, None)
        } else {
            Ok(results)
        }
    }

    /// Search stored snapshots with explicit FTS5 semantics.
    ///
    /// # Errors
    ///
    /// Returns an error if the FTS query cannot be prepared or executed.
    pub fn search_fts(
        &self,
        query: &str,
        limit: usize,
        filters: &RetrievalFilters,
    ) -> Result<SearchResults> {
        self.search_fts_page(query, limit, filters, None)
    }

    pub(crate) fn search_fts_page(
        &self,
        query: &str,
        limit: usize,
        filters: &RetrievalFilters,
        cursor: Option<&SearchCursorState>,
    ) -> Result<SearchResults> {
        let limit = clamp_result_limit(limit);
        let params = SearchExecutionParams::new(limit, filters, cursor)?;
        let use_snapshot_event_cache = can_use_snapshot_event_cache(filters);
        let has_temporal_event_filters = has_temporal_event_filters(filters);
        let analysis = analyze_query(query);
        let sql = unified_fts_query(use_snapshot_event_cache, has_temporal_event_filters);
        let phrase = analysis.literal_search_lower();
        let phrase_like = format!("%{}%", escape_like_pattern(&phrase));

        let mut stmt = self
            .conn
            .prepare(&sql)
            .context("prepare FTS search query")?;
        let rows = stmt
            .query_map(
                named_params! {
                    ":query" : query,
                    ":phrase_lower" : phrase.as_ref(),
                    ":phrase_like" : phrase_like.as_str(),
                    ":since" : params.since.as_deref(),
                    ":until" : params.until,
                    ":app_like" : params.app_like.as_deref(),
                    ":bundle_id" : params.bundle_id.as_deref(),
                    ":kind" : params.kind,
                    ":has_text" : params.has_text,
                    ":has_url" : params.has_url,
                    ":has_file_url" : params.has_file_url,
                    ":has_image" : params.has_image,
                    ":has_pdf" : params.has_pdf,
                    ":min_bytes" : params.min_bytes,
                    ":max_bytes" : params.max_bytes,
                    ":cursor_primary" : params.cursor_primary,
                    ":cursor_exact" : params.cursor_exact,
                    ":cursor_last_seen_at" : params.cursor_last_seen_at,
                    ":cursor_snapshot_id" : params.cursor_snapshot_id,
                    ":limit" : params.fetch_limit,
                },
                |row| map_unified_search_hit_row(row, query, is_simple_fts_query(&analysis)),
            )
            .context("execute FTS search query")?;

        collect_search_results(rows, limit, SearchMode::Fts, "unified FTS")
    }

    /// Search stored snapshots with literal `LIKE` matching semantics.
    ///
    /// # Errors
    ///
    /// Returns an error if the literal query cannot be prepared or executed.
    pub fn search_literal(
        &self,
        query: &str,
        limit: usize,
        filters: &RetrievalFilters,
    ) -> Result<SearchResults> {
        self.search_literal_page(query, limit, filters, None)
    }

    pub(crate) fn search_literal_page(
        &self,
        query: &str,
        limit: usize,
        filters: &RetrievalFilters,
        cursor: Option<&SearchCursorState>,
    ) -> Result<SearchResults> {
        let limit = clamp_result_limit(limit);
        let params = SearchExecutionParams::new(limit, filters, cursor)?;
        let analysis = analyze_query(query);
        let literal_search_lower = analysis.literal_search_lower();
        let literal_search_like = escape_like_pattern(&literal_search_lower);
        let loose_literal_like = if analysis.path_fragment.is_some() {
            escape_like_pattern(analysis.path_fragment.as_deref().unwrap_or_default())
        } else {
            literal_search_lower
                .split_whitespace()
                .filter(|term| !term.is_empty())
                .map(escape_like_pattern)
                .collect::<Vec<_>>()
                .join("%")
        };
        let like = format!("%{literal_search_like}%");
        let loose_like = format!("%{loose_literal_like}%");
        let use_snapshot_event_cache = can_use_snapshot_event_cache(filters);
        let sql = unified_literal_query(
            use_snapshot_event_cache,
            has_temporal_event_filters(filters),
        );

        let mut stmt = self
            .conn
            .prepare(&sql)
            .context("prepare literal search query")?;
        let prefix_like = format!("{literal_search_like}%");
        let rows = stmt
            .query_map(
                named_params! {
                    ":query_lower" : literal_search_lower.as_ref(),
                    ":like" : like,
                    ":loose_like" : loose_like.as_str(),
                    ":prefix_like" : prefix_like.as_str(),
                    ":since" : params.since.as_deref(),
                    ":until" : params.until,
                    ":app_like" : params.app_like.as_deref(),
                    ":bundle_id" : params.bundle_id.as_deref(),
                    ":kind" : params.kind,
                    ":has_text" : params.has_text,
                    ":has_url" : params.has_url,
                    ":has_file_url" : params.has_file_url,
                    ":has_image" : params.has_image,
                    ":has_pdf" : params.has_pdf,
                    ":min_bytes" : params.min_bytes,
                    ":max_bytes" : params.max_bytes,
                    ":cursor_primary" : params.cursor_primary,
                    ":cursor_secondary" : params.cursor_secondary,
                    ":cursor_last_seen_at" : params.cursor_last_seen_at,
                    ":cursor_snapshot_id" : params.cursor_snapshot_id,
                    ":limit" : params.fetch_limit,
                },
                |row| map_unified_search_hit_row(row, query, true),
            )
            .context("execute literal search query")?;

        collect_search_results(rows, limit, SearchMode::Literal, "unified literal")
    }
}
