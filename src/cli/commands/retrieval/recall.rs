use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::Path;

use anyhow::{anyhow, Result};
use serde_json::json;

use crate::db::{Database, RetrievalFilters, SearchMode, SearchResults};
use crate::model::SearchHit;

use crate::cli::output::{
    RecallEnvelope, RecallMatchConfidence, RecallOutputRow, OUTPUT_SCHEMA_VERSION,
};
use crate::cli::presentation::{emit_recall_output, generated_at_now};
use crate::cli::schema::RecallArgs;

use super::super::retrieval_support::{
    load_snapshot_projections, merge_applied_filters, normalize_retrieval_filters,
};
use super::super::runtime::open_read_only_db;

mod scoring;
pub(in crate::cli) use scoring::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::cli) enum RecallCandidateSource {
    Search,
    Recent,
}

#[derive(Debug, Clone)]
pub(in crate::cli) struct RecallCandidate {
    pub(in crate::cli) hit: SearchHit,
    pub(in crate::cli) source: RecallCandidateSource,
    pub(in crate::cli) match_quality: Option<f64>,
    pub(in crate::cli) sort_score: f64,
    pub(in crate::cli) app_preferred: bool,
}

#[derive(Debug)]
pub(in crate::cli) struct RecallComputation {
    pub(in crate::cli) best: RecallCandidate,
    pub(in crate::cli) alternatives: Vec<RecallCandidate>,
    pub(in crate::cli) why_selected: String,
    pub(in crate::cli) search_mode_used: Option<SearchMode>,
}

pub(in crate::cli) fn recall(db_path: &Path, args: &RecallArgs) -> Result<()> {
    let format = args.output.resolved()?;
    let filters = normalize_retrieval_filters(&args.filters)?;
    let db = open_read_only_db(db_path)?;
    let recall = anyhow::Context::context(
        compute_recall(&db, args, &filters),
        "recall failed; if this is unexpected, run `clipmem service status` and `clipmem doctor`",
    )?;
    let envelope = build_recall_envelope(&db, args, &filters, &recall)?;
    emit_recall_output(format, &envelope)
}

fn build_recall_envelope(
    db: &Database,
    args: &RecallArgs,
    filters: &RetrievalFilters,
    recall: &RecallComputation,
) -> Result<RecallEnvelope> {
    let generated_at = generated_at_now()?;
    let projections = load_snapshot_projections(
        db,
        std::iter::once(recall.best.hit.snapshot_id()).chain(
            recall
                .alternatives
                .iter()
                .map(|candidate| candidate.hit.snapshot_id()),
        ),
    )?;
    let best_projection = projections
        .get(&recall.best.hit.snapshot_id())
        .cloned()
        .unwrap_or_default();
    let best_candidate = RecallOutputRow::from_hit(&recall.best.hit, args.full, &best_projection);
    const QUERY_MATCH_EVIDENCE_SCORE: f64 = 0.5;
    // Complex explicit FTS queries have no per-hit quality; the legacy
    // enum/score contract stays in its old domain and the additive
    // match_kind field carries the honest semantics.
    let recent_fallback = args.query.is_some()
        && matches!(recall.best.source, RecallCandidateSource::Recent)
        && recall.best.match_quality.is_none();
    let match_kind = if recent_fallback {
        "recent_fallback"
    } else if recall.best.match_quality.is_some() {
        "scored"
    } else {
        "query_match"
    };
    let effective_score = recall
        .best
        .match_quality
        .unwrap_or(QUERY_MATCH_EVIDENCE_SCORE);
    let best_match_score = (!recent_fallback).then_some(effective_score);
    let confidence = RecallMatchConfidence::from_normalized_score(effective_score);
    let quoted_text = args
        .quote
        .then(|| best_candidate.best_text.clone())
        .filter(|text| !text.is_empty());
    Ok(RecallEnvelope {
        schema_version: OUTPUT_SCHEMA_VERSION,
        command: "recall",
        generated_at,
        applied_filters: merge_applied_filters(
            filters,
            json!({
                "limit": args.limit,
                "query_present": args.query.is_some(),
                "requested_mode": args.query.as_ref().map(|_| args.search_mode().as_str()),
                "mode_used": recall.search_mode_used.map(SearchMode::as_str),
                "full": args.full,
                "quote": args.quote,
                "min_score": args.min_score,
                "prefer_recent": args.prefer_recent,
                "prefer_app": args.prefer_app,
            }),
        ),
        query: args.query.clone(),
        match_kind,
        best_candidate,
        alternatives: recall
            .alternatives
            .iter()
            .map(|candidate| {
                let projection = projections
                    .get(&candidate.hit.snapshot_id())
                    .cloned()
                    .unwrap_or_default();
                RecallOutputRow::from_hit(&candidate.hit, false, &projection)
            })
            .collect(),
        best_match_confidence: confidence,
        best_match_score,
        why_selected: recall.why_selected.clone(),
        quoted_text,
        score_semantics: "evidence_v1",
    })
}

pub(in crate::cli) fn compute_recall(
    db: &Database,
    args: &RecallArgs,
    filters: &RetrievalFilters,
) -> Result<RecallComputation> {
    let query = args
        .query
        .as_deref()
        .map(str::trim)
        .filter(|query| !query.is_empty());
    let mut merged = HashMap::<i64, RecallCandidate>::new();
    let mut search_mode_used = None;
    let mut search_was_weak = false;

    if let Some(query) = query {
        let results = run_search_query(db, query, args.search_mode(), args.limit, filters)?;
        search_mode_used = Some(results.mode_used());
        let mut recent_ids = results.hits().iter().collect::<Vec<_>>();
        recent_ids.sort_by(|a, b| {
            b.last_observed_at()
                .cmp(a.last_observed_at())
                .then_with(|| b.snapshot_id().cmp(&a.snapshot_id()))
        });
        let recent_ranks = recent_ids
            .iter()
            .enumerate()
            .map(|(rank, hit)| (hit.snapshot_id(), rank))
            .collect::<HashMap<_, _>>();
        let search_candidates = results
            .hits()
            .iter()
            .enumerate()
            .map(|(index, hit)| {
                let mut candidate = build_search_candidate(
                    hit,
                    query,
                    results.mode_used(),
                    index,
                    args.prefer_app.as_deref(),
                    false,
                );
                if args.prefer_recent {
                    candidate.sort_score +=
                        recent_index_boost(recent_ranks[&hit.snapshot_id()]) * 0.6;
                }
                candidate
            })
            .collect::<Vec<_>>();

        let threshold = args
            .min_score
            .unwrap_or(default_recall_threshold(results.mode_used()));
        search_was_weak = search_candidates.iter().all(|candidate| {
            candidate
                .match_quality
                .is_some_and(|quality| quality < threshold)
        });

        for mut candidate in search_candidates {
            if search_was_weak {
                candidate.sort_score *= 0.45;
            }
            upsert_recall_candidate(&mut merged, candidate);
        }

        if search_was_weak {
            for (index, hit) in db
                .recent(args.limit, filters)?
                .into_hits()
                .into_iter()
                .enumerate()
            {
                let mut candidate = build_recent_candidate(
                    hit,
                    index,
                    args.prefer_app.as_deref(),
                    args.prefer_recent,
                );
                // Recency orders fallback suggestions; it is not query-match evidence.
                candidate.match_quality = None;
                upsert_recall_candidate(&mut merged, candidate);
            }
        }
    } else {
        for (index, hit) in db
            .recent(args.limit, filters)?
            .into_hits()
            .into_iter()
            .enumerate()
        {
            upsert_recall_candidate(
                &mut merged,
                build_recent_candidate(hit, index, args.prefer_app.as_deref(), args.prefer_recent),
            );
        }
    }

    let mut ranked = merged.into_values().collect::<Vec<_>>();
    ranked.sort_by(compare_recall_candidates);

    let best = ranked
        .first()
        .cloned()
        .ok_or_else(|| {
            anyhow!(
                "no clipboard candidates matched the recall request; if this is unexpected, run `clipmem service status` to confirm the watcher is running"
            )
        })?;
    let alternatives = ranked
        .into_iter()
        .skip(1)
        .take(args.limit.saturating_sub(1))
        .collect::<Vec<_>>();
    let why_selected = build_recall_why_selected(
        &best,
        query,
        search_was_weak,
        args.prefer_recent,
        args.prefer_app.as_deref(),
    );

    Ok(RecallComputation {
        best,
        alternatives,
        why_selected,
        search_mode_used,
    })
}

pub(in crate::cli) fn run_search_query(
    db: &Database,
    query: &str,
    mode: SearchMode,
    limit: usize,
    filters: &RetrievalFilters,
) -> Result<SearchResults> {
    let result = match mode {
        SearchMode::Auto => db.search_auto(query, limit, filters),
        SearchMode::Fts => db.search_fts(query, limit, filters),
        SearchMode::Literal => db.search_literal(query, limit, filters),
    };
    result.map_err(super::classify_search_error)
}

pub(in crate::cli) fn build_search_candidate(
    hit: &SearchHit,
    _query: &str,
    _mode_used: SearchMode,
    index: usize,
    prefer_app: Option<&str>,
    prefer_recent: bool,
) -> RecallCandidate {
    let match_quality = hit.match_quality();
    let normalized_score = match_quality.unwrap_or(0.50);
    let app_preferred = matches_preferred_app(hit, prefer_app);
    let mut sort_score = normalized_score;
    sort_score += app_preference_boost(app_preferred);
    if prefer_recent {
        sort_score += recent_index_boost(index) * 0.6;
    }
    sort_score += search_rank_bonus(index);

    RecallCandidate {
        hit: hit.clone(),
        source: RecallCandidateSource::Search,
        match_quality,
        sort_score,
        app_preferred,
    }
}

pub(in crate::cli) fn build_recent_candidate(
    hit: SearchHit,
    index: usize,
    prefer_app: Option<&str>,
    prefer_recent: bool,
) -> RecallCandidate {
    let app_preferred = matches_preferred_app(&hit, prefer_app);
    let text_bonus = if !hit.preview_text().trim().is_empty() {
        0.08
    } else {
        0.0
    };
    let mut normalized_score = 0.55 + recent_index_boost(index) + text_bonus;
    if prefer_recent {
        normalized_score += 0.08;
    }
    normalized_score += app_preference_boost(app_preferred);
    normalized_score = normalized_score.clamp(0.0, 0.99);

    RecallCandidate {
        hit,
        source: RecallCandidateSource::Recent,
        match_quality: Some(normalized_score),
        sort_score: normalized_score,
        app_preferred,
    }
}

pub(in crate::cli) fn upsert_recall_candidate(
    store: &mut HashMap<i64, RecallCandidate>,
    candidate: RecallCandidate,
) {
    match store.get_mut(&candidate.hit.snapshot_id()) {
        Some(existing) => {
            let replace = compare_recall_candidates(&candidate, existing) == Ordering::Less;
            if replace {
                *existing = candidate;
            }
        }
        None => {
            store.insert(candidate.hit.snapshot_id(), candidate);
        }
    }
}

pub(in crate::cli) fn compare_recall_candidates(
    left: &RecallCandidate,
    right: &RecallCandidate,
) -> Ordering {
    right
        .sort_score
        .partial_cmp(&left.sort_score)
        .unwrap_or(Ordering::Equal)
        .then_with(|| {
            right
                .hit
                .last_observed_at()
                .cmp(left.hit.last_observed_at())
        })
        .then_with(|| right.hit.snapshot_id().cmp(&left.hit.snapshot_id()))
        .then_with(|| match (left.source, right.source) {
            (RecallCandidateSource::Search, RecallCandidateSource::Recent) => Ordering::Less,
            (RecallCandidateSource::Recent, RecallCandidateSource::Search) => Ordering::Greater,
            _ => Ordering::Equal,
        })
}

pub(in crate::cli) fn build_recall_why_selected(
    best: &RecallCandidate,
    query: Option<&str>,
    search_was_weak: bool,
    prefer_recent: bool,
    prefer_app: Option<&str>,
) -> String {
    let mut parts = Vec::new();

    match (query, best.source, search_was_weak) {
        (Some(query), RecallCandidateSource::Search, false) => {
            parts.push(format!(
                "Selected the strongest search match for \"{query}\""
            ));
        }
        (Some(query), RecallCandidateSource::Search, true) => {
            parts.push(format!(
                "Selected the best available query match for \"{query}\" after weak search results were merged with recent candidates"
            ));
        }
        (Some(_query), RecallCandidateSource::Recent, true) => {
            parts.push(
                "Fell back to recent clipboard items because query matches were weak".to_string(),
            );
        }
        (None, RecallCandidateSource::Recent, _) => {
            parts.push("Selected the most likely useful recent clipboard item".to_string());
        }
        _ => {
            parts.push("Selected the top-ranked clipboard candidate".to_string());
        }
    }

    if best.app_preferred {
        if let Some(prefer_app) = prefer_app {
            parts.push(format!("it matched the preferred app \"{prefer_app}\""));
        }
    }
    if prefer_recent && matches!(best.source, RecallCandidateSource::Recent) {
        parts.push("recency preference boosted this candidate".to_string());
    }

    parts.join("; ")
}

#[cfg(test)]
mod ranking_tests {
    use super::{command_specificity_bonus, expanded_recall_queries, term_stuffing_penalty};

    #[test]
    fn recall_does_not_apply_unvalidated_product_expansions() {
        assert!(expanded_recall_queries("half off launch deal").is_empty());
        assert!(expanded_recall_queries("remote tomojax pytest").is_empty());
        assert!(expanded_recall_queries("clipboard watcher needs disk permission").is_empty());
    }

    #[test]
    fn command_bonus_prefers_command_shaped_results_without_naming_commands() {
        let command = command_specificity_bonus(
            "cargo test --test cli_commands search -- --nocapture",
            "cargo test search",
        );
        let prose = command_specificity_bonus(
            "notes about a previous cargo test search run",
            "cargo test search",
        );

        assert!(command > prose);
        assert!(command <= 0.26);
    }

    #[test]
    fn term_stuffing_penalty_only_applies_to_repeated_query_terms() {
        assert_eq!(
            term_stuffing_penalty("git status --short && git diff --stat", "git status"),
            0.0
        );
        assert!(
            term_stuffing_penalty(
                "git status git status git status git status dashboard",
                "git status"
            ) > 0.0
        );
    }
}

#[cfg(test)]
mod profile_tests {
    use super::{literal_match_score, matches_preferred_app};
    use crate::model::{SearchHit, SearchHitParts};
    use std::time::{Duration, Instant};

    #[test]
    #[ignore = "profiling harness for recall literal scoring"]
    fn profile_recall_literal_match_scoring() {
        let hits = large_recall_hits(20_000);
        let query = "needle term";

        let before = median_duration(11, || {
            let total = hits
                .iter()
                .map(|hit| literal_match_score_before_for_profile(hit, query))
                .sum::<f64>();
            assert!(total > 10_000.0);
        });
        let after = median_duration(11, || {
            let total = hits
                .iter()
                .map(|hit| literal_match_score(hit, query))
                .sum::<f64>();
            assert!(total > 10_000.0);
        });

        eprintln!(
            "recall_literal_lowercase_before={before:?} recall_literal_ascii_after={after:?}"
        );
    }

    #[test]
    #[ignore = "profiling harness for recall preferred-app matching"]
    fn profile_recall_preferred_app_matching() {
        let hits = large_recall_hits(20_000);

        let before = median_duration(11, || {
            let matches = hits
                .iter()
                .filter(|hit| matches_preferred_app_before_for_profile(hit, Some("terminal")))
                .count();
            assert_eq!(matches, hits.len());
        });
        let after = median_duration(11, || {
            let matches = hits
                .iter()
                .filter(|hit| matches_preferred_app(hit, Some("terminal")))
                .count();
            assert_eq!(matches, hits.len());
        });

        eprintln!("recall_preferred_app_lowercase_before={before:?} recall_preferred_app_ascii_after={after:?}");
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

    fn large_recall_hits(count: usize) -> Vec<SearchHit> {
        (0..count)
            .map(|index| {
                let preview = format!(
                    "Clipboard row {index} includes Needle text with surrounding words and term"
                );
                let why_matched = Some(format!(
                    "Search snippet {index} with another Needle Term occurrence"
                ));
                SearchHit::from_parts(
                    SearchHitParts::plain_text(index as i64 + 1, index as i64 + 100_000, preview)
                        .with_sha256(format!("{:064x}", index + 1))
                        .with_match(why_matched, vec!["best_text".to_string()])
                        .with_capture_summary(
                            1,
                            "2026-04-17 10:00:00".to_string(),
                            "2026-04-17 11:00:00".to_string(),
                        )
                        .with_last_frontmost_app(
                            Some("Terminal".to_string()),
                            Some("com.apple.Terminal".to_string()),
                        )
                        .with_size(128, 1)
                        .with_score(Some(0.25)),
                )
            })
            .collect()
    }

    fn literal_match_score_before_for_profile(hit: &SearchHit, query: &str) -> f64 {
        let query = query.trim().to_ascii_lowercase();
        if query.is_empty() {
            return 0.0;
        }

        let candidates = [
            hit.why_matched().unwrap_or(hit.preview_text()),
            hit.preview_text(),
        ];

        if candidates
            .iter()
            .any(|value| value.trim().eq_ignore_ascii_case(&query))
        {
            return 0.95;
        }
        if candidates
            .iter()
            .any(|value| value.to_ascii_lowercase().starts_with(&query))
        {
            return 0.88;
        }
        if candidates
            .iter()
            .any(|value| value.to_ascii_lowercase().contains(&query))
        {
            return 0.78;
        }

        let query_terms = token_candidates_for_profile(&query);
        if query_terms.is_empty() {
            return 0.0;
        }

        let best_overlap = candidates
            .iter()
            .map(|value| {
                let lower = value.to_ascii_lowercase();
                let matched = query_terms
                    .iter()
                    .filter(|term| lower.contains(**term))
                    .count();
                matched as f64 / query_terms.len() as f64
            })
            .fold(0.0, f64::max);

        (0.55 + best_overlap * 0.25).clamp(0.0, 0.82)
    }

    fn matches_preferred_app_before_for_profile(hit: &SearchHit, prefer_app: Option<&str>) -> bool {
        let Some(prefer_app) = prefer_app.map(str::trim).filter(|value| !value.is_empty()) else {
            return false;
        };
        let prefer_app = prefer_app.to_ascii_lowercase();
        hit.last_frontmost_app_name()
            .map(|value| value.to_ascii_lowercase().contains(&prefer_app))
            .unwrap_or(false)
            || hit
                .last_frontmost_app_bundle_id()
                .map(|value| value.to_ascii_lowercase().contains(&prefer_app))
                .unwrap_or(false)
    }

    fn token_candidates_for_profile(query: &str) -> Vec<&str> {
        query
            .split_whitespace()
            .filter(|term| !term.is_empty())
            .collect()
    }
}
