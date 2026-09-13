use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::cli::errors::invalid_args_error;
use crate::db::{
    RecentCursorState, RetrievalFilters, SearchCursorState, SearchMode, TimelineCursorState,
    TimelineSort,
};
use crate::model::{SearchHit, TimelineEvent};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(in crate::cli) struct SearchCursorToken {
    pub(in crate::cli) version: u32,
    pub(in crate::cli) command: String,
    pub(in crate::cli) query: String,
    pub(in crate::cli) requested_mode: SearchMode,
    pub(in crate::cli) mode_used: SearchMode,
    pub(in crate::cli) filters: RetrievalFilters,
    pub(in crate::cli) last_seen_at: String,
    pub(in crate::cli) snapshot_id: i64,
    pub(in crate::cli) score: Option<f64>,
    pub(in crate::cli) query_hash: String,
    pub(in crate::cli) filter_hash: String,
    pub(in crate::cli) order_primary: Option<f64>,
    pub(in crate::cli) order_secondary: Option<f64>,
    pub(in crate::cli) exact_phrase: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(in crate::cli) struct RecentCursorToken {
    pub(in crate::cli) command: String,
    pub(in crate::cli) filters: RetrievalFilters,
    pub(in crate::cli) last_seen_at: String,
    pub(in crate::cli) snapshot_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(in crate::cli) struct TimelineCursorToken {
    pub(in crate::cli) command: String,
    pub(in crate::cli) filters: RetrievalFilters,
    pub(in crate::cli) sort: TimelineSort,
    pub(in crate::cli) observed_at: String,
    pub(in crate::cli) event_id: i64,
}

pub(in crate::cli) fn bind_search_cursor(encoded: &str, archive_state: &str) -> Result<String> {
    let mut token: serde_json::Value = decode_cursor(encoded)?;
    token["archive_state"] = archive_state.into();
    encode_cursor(&token)
}

pub(in crate::cli) fn validate_search_archive_state(
    encoded: &str,
    archive_state: &str,
) -> Result<()> {
    let token: serde_json::Value = decode_cursor(encoded)?;
    if token
        .get("archive_state")
        .and_then(serde_json::Value::as_str)
        != Some(archive_state)
    {
        return Err(invalid_args_error(
            "search cursor is stale or belongs to another archive; rerun without --cursor",
        ));
    }
    Ok(())
}

pub(in crate::cli) fn parse_search_cursor(
    encoded: &str,
    query: &str,
    requested_mode: SearchMode,
    filters: &RetrievalFilters,
) -> Result<SearchCursorState> {
    let value: serde_json::Value = decode_cursor(encoded)?;
    if value.get("version").and_then(serde_json::Value::as_u64) != Some(2) {
        return Err(invalid_args_error(
            "cursor version no longer supported; rerun without cursor",
        ));
    }
    let token: SearchCursorToken = serde_json::from_value(value)
        .map_err(|error| anyhow!("invalid cursor payload: {error}"))?;
    if token.command != "search" {
        return Err(invalid_args_error(format!(
            "cursor is for command `{}` but was used with `search`",
            token.command
        )));
    }
    if token.query_hash != stable_hash(&query) || token.requested_mode != requested_mode {
        return Err(invalid_args_error(
            "cursor does not match the active search query or mode",
        ));
    }
    if token.filter_hash != stable_hash(filters) {
        return Err(invalid_args_error(
            "cursor does not match the active search filters",
        ));
    }
    if requested_mode != SearchMode::Auto && token.mode_used != requested_mode {
        return Err(invalid_args_error(format!(
            "cursor mode `{}` does not match the requested search mode `{}`",
            token.mode_used.as_str(),
            requested_mode.as_str()
        )));
    }

    Ok(SearchCursorState::new(
        token.mode_used,
        token.last_seen_at,
        token.snapshot_id,
        token.order_primary,
        token.order_secondary,
        token.exact_phrase,
    ))
}

pub(in crate::cli) fn parse_recent_cursor(
    encoded: &str,
    filters: &RetrievalFilters,
) -> Result<RecentCursorState> {
    let token: RecentCursorToken = decode_cursor(encoded)?;
    if token.command != "recent" {
        return Err(invalid_args_error(format!(
            "cursor is for command `{}` but was used with `recent`",
            token.command
        )));
    }
    if token.filters != *filters {
        return Err(invalid_args_error(
            "cursor does not match the active recent filters",
        ));
    }

    Ok(RecentCursorState::new(
        token.last_seen_at,
        token.snapshot_id,
    ))
}

pub(in crate::cli) fn parse_timeline_cursor(
    encoded: &str,
    filters: &RetrievalFilters,
    sort: TimelineSort,
) -> Result<TimelineCursorState> {
    let token: TimelineCursorToken = decode_cursor(encoded)?;
    if token.command != "timeline" {
        return Err(invalid_args_error(format!(
            "cursor is for command `{}` but was used with `timeline`",
            token.command
        )));
    }
    if token.filters != *filters || token.sort != sort {
        return Err(invalid_args_error(
            "cursor does not match the active timeline filters or sort",
        ));
    }

    Ok(TimelineCursorState::new(token.observed_at, token.event_id))
}

pub(in crate::cli) fn encode_search_cursor(
    query: &str,
    requested_mode: SearchMode,
    filters: &RetrievalFilters,
    mode_used: SearchMode,
    hit: &SearchHit,
) -> Result<String> {
    encode_cursor(&SearchCursorToken {
        version: 2,
        command: "search".to_string(),
        query: query.to_string(),
        requested_mode,
        mode_used,
        filters: filters.clone(),
        last_seen_at: hit.last_observed_at().to_string(),
        snapshot_id: hit.snapshot_id(),
        score: hit.score(),
        query_hash: stable_hash(&query),
        filter_hash: stable_hash(filters),
        order_primary: hit
            .search_order_key()
            .map(|key| key.primary)
            .or(hit.score()),
        order_secondary: hit
            .search_order_key()
            .map(|key| key.secondary)
            .or(Some(0.0)),
        exact_phrase: hit.search_order_key().is_some_and(|key| key.exact_phrase),
    })
}

fn stable_hash(value: &impl Serialize) -> String {
    let bytes = serde_json::to_vec(value).expect("cursor hash input should serialize");
    hex::encode(Sha256::digest(bytes))
}

pub(in crate::cli) fn encode_recent_cursor(
    filters: &RetrievalFilters,
    hit: &SearchHit,
) -> Result<String> {
    encode_cursor(&RecentCursorToken {
        command: "recent".to_string(),
        filters: filters.clone(),
        last_seen_at: hit.last_observed_at().to_string(),
        snapshot_id: hit.snapshot_id(),
    })
}

pub(in crate::cli) fn encode_timeline_cursor(
    filters: &RetrievalFilters,
    sort: TimelineSort,
    event: &TimelineEvent,
) -> Result<String> {
    encode_cursor(&TimelineCursorToken {
        command: "timeline".to_string(),
        filters: filters.clone(),
        sort,
        observed_at: event.observed_at().to_string(),
        event_id: event.event_id(),
    })
}

pub(in crate::cli) fn encode_cursor<T: Serialize>(token: &T) -> Result<String> {
    let json = serde_json::to_vec(token)?;
    Ok(hex::encode(json))
}

pub(in crate::cli) fn decode_cursor<T: for<'de> Deserialize<'de>>(encoded: &str) -> Result<T> {
    let bytes =
        hex::decode(encoded).map_err(|error| anyhow!("invalid cursor encoding: {error}"))?;
    serde_json::from_slice(&bytes).map_err(|error| anyhow!("invalid cursor payload: {error}"))
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use crate::db::{RetrievalFilters, SearchMode, TimelineSort};
    use crate::model::{SearchHit, SearchHitParts, SnapshotKind, TimelineEvent};

    use super::{
        decode_cursor, encode_cursor, encode_recent_cursor, encode_search_cursor,
        encode_timeline_cursor, parse_recent_cursor, parse_search_cursor, parse_timeline_cursor,
        SearchCursorToken, TimelineCursorToken,
    };

    #[test]
    fn search_cursor_round_trips_with_auto_mode_and_score() {
        let filters = RetrievalFilters::default().with_hours(Some(24));
        let hit = search_hit(42, "2026-04-16T10:00:00Z", Some(0.75));
        let encoded =
            encode_search_cursor("git", SearchMode::Auto, &filters, SearchMode::Fts, &hit).unwrap();

        let cursor = parse_search_cursor(&encoded, "git", SearchMode::Auto, &filters).unwrap();

        assert_eq!(cursor.mode_used(), SearchMode::Fts);
        assert_eq!(cursor.order_primary(), Some(0.75));
        assert_eq!(cursor.last_seen_at(), "2026-04-16T10:00:00Z");
        assert_eq!(cursor.snapshot_id(), 42);
    }

    #[test]
    fn search_cursor_rejects_wrong_command() {
        let filters = RetrievalFilters::default();
        let encoded = encode_cursor(&SearchCursorToken {
            version: 2,
            command: "recent".to_string(),
            query: "git".to_string(),
            requested_mode: SearchMode::Auto,
            mode_used: SearchMode::Fts,
            filters: filters.clone(),
            last_seen_at: "2026-04-16T10:00:00Z".to_string(),
            snapshot_id: 42,
            score: Some(0.75),
            query_hash: String::new(),
            filter_hash: String::new(),
            order_primary: Some(0.75),
            order_secondary: Some(0.0),
            exact_phrase: false,
        })
        .unwrap();

        let error = parse_search_cursor(&encoded, "git", SearchMode::Auto, &filters).unwrap_err();

        assert!(error
            .to_string()
            .contains("cursor is for command `recent` but was used with `search`"));
    }

    #[test]
    fn search_cursor_rejects_legacy_versions_with_actionable_guidance() {
        let filters = RetrievalFilters::default();
        let encoded = encode_cursor(&serde_json::json!({
            "command": "search",
            "query": "git",
            "requested_mode": "Auto",
            "mode_used": "Fts",
            "filters": filters,
            "last_seen_at": "2026-04-16T10:00:00Z",
            "snapshot_id": 42,
            "score": -0.5
        }))
        .unwrap();
        let error = parse_search_cursor(
            &encoded,
            "git",
            SearchMode::Auto,
            &RetrievalFilters::default(),
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("cursor version no longer supported; rerun without cursor"));
    }

    #[test]
    fn search_cursor_rejects_query_mode_and_filter_mismatches() {
        let filters = RetrievalFilters::default().with_hours(Some(24));
        let hit = search_hit(42, "2026-04-16T10:00:00Z", Some(0.75));
        let encoded =
            encode_search_cursor("git", SearchMode::Auto, &filters, SearchMode::Fts, &hit).unwrap();

        assert_error_contains(
            parse_search_cursor(&encoded, "cargo", SearchMode::Auto, &filters).unwrap_err(),
            "cursor does not match the active search query or mode",
        );
        assert_error_contains(
            parse_search_cursor(&encoded, "git", SearchMode::Literal, &filters).unwrap_err(),
            "cursor does not match the active search query or mode",
        );
        assert_error_contains(
            parse_search_cursor(
                &encoded,
                "git",
                SearchMode::Auto,
                &RetrievalFilters::default().with_hours(Some(12)),
            )
            .unwrap_err(),
            "cursor does not match the active search filters",
        );

        let mode_mismatch =
            encode_search_cursor("git", SearchMode::Literal, &filters, SearchMode::Fts, &hit)
                .unwrap();
        assert_error_contains(
            parse_search_cursor(&mode_mismatch, "git", SearchMode::Literal, &filters).unwrap_err(),
            "cursor mode `fts` does not match the requested search mode `literal`",
        );
    }

    #[test]
    fn recent_cursor_round_trips_and_rejects_filter_mismatch() {
        let filters = RetrievalFilters::default().with_hours(Some(24));
        let hit = search_hit(42, "2026-04-16T10:00:00Z", None);
        let encoded = encode_recent_cursor(&filters, &hit).unwrap();

        let cursor = parse_recent_cursor(&encoded, &filters).unwrap();

        assert_eq!(cursor.last_seen_at(), "2026-04-16T10:00:00Z");
        assert_eq!(cursor.snapshot_id(), 42);
        assert_error_contains(
            parse_recent_cursor(&encoded, &RetrievalFilters::default().with_hours(Some(12)))
                .unwrap_err(),
            "cursor does not match the active recent filters",
        );
    }

    #[test]
    fn recent_cursor_rejects_wrong_command() {
        let filters = RetrievalFilters::default();
        let encoded = encode_cursor(&SearchCursorToken {
            version: 2,
            command: "search".to_string(),
            query: "git".to_string(),
            requested_mode: SearchMode::Auto,
            mode_used: SearchMode::Fts,
            filters: filters.clone(),
            last_seen_at: "2026-04-16T10:00:00Z".to_string(),
            snapshot_id: 42,
            score: Some(0.75),
            query_hash: String::new(),
            filter_hash: String::new(),
            order_primary: Some(0.75),
            order_secondary: Some(0.0),
            exact_phrase: false,
        })
        .unwrap();

        let error = parse_recent_cursor(&encoded, &filters).unwrap_err();

        assert!(error
            .to_string()
            .contains("cursor is for command `search` but was used with `recent`"));
    }

    #[test]
    fn timeline_cursor_round_trips_and_rejects_sort_mismatch() {
        let filters = RetrievalFilters::default().with_hours(Some(24));
        let event = timeline_event(77, "2026-04-16T11:00:00Z");
        let encoded = encode_timeline_cursor(&filters, TimelineSort::Desc, &event).unwrap();

        let cursor = parse_timeline_cursor(&encoded, &filters, TimelineSort::Desc).unwrap();

        assert_eq!(cursor.observed_at(), "2026-04-16T11:00:00Z");
        assert_eq!(cursor.event_id(), 77);
        assert_error_contains(
            parse_timeline_cursor(&encoded, &filters, TimelineSort::Asc).unwrap_err(),
            "cursor does not match the active timeline filters or sort",
        );
    }

    #[test]
    fn timeline_cursor_rejects_wrong_command() {
        let filters = RetrievalFilters::default();
        let encoded = encode_cursor(&TimelineCursorToken {
            command: "recent".to_string(),
            filters: filters.clone(),
            sort: TimelineSort::Desc,
            observed_at: "2026-04-16T11:00:00Z".to_string(),
            event_id: 77,
        })
        .unwrap();

        let error = parse_timeline_cursor(&encoded, &filters, TimelineSort::Desc).unwrap_err();

        assert!(error
            .to_string()
            .contains("cursor is for command `recent` but was used with `timeline`"));
    }

    #[test]
    fn decode_cursor_rejects_invalid_hex() {
        let error = decode_cursor::<Value>("not hex").unwrap_err();

        assert!(error.to_string().contains("invalid cursor encoding"));
    }

    #[test]
    fn decode_cursor_rejects_invalid_json_payload() {
        let encoded = hex::encode(b"not json");

        let error = decode_cursor::<Value>(&encoded).unwrap_err();

        assert!(error.to_string().contains("invalid cursor payload"));
    }

    fn search_hit(snapshot_id: i64, last_observed_at: &str, score: Option<f64>) -> SearchHit {
        SearchHit::from_parts(
            SearchHitParts::plain_text(snapshot_id, snapshot_id + 100, "git status".to_string())
                .with_sha256("abc".to_string())
                .with_match(
                    Some("git status".to_string()),
                    vec!["best_text".to_string(), "search_text".to_string()],
                )
                .with_capture_summary(
                    1,
                    "2026-04-16T09:00:00Z".to_string(),
                    last_observed_at.to_string(),
                )
                .with_last_frontmost_app(
                    Some("Terminal".to_string()),
                    Some("com.apple.Terminal".to_string()),
                )
                .with_size(10, 1)
                .with_score(score),
        )
    }

    fn timeline_event(event_id: i64, observed_at: &str) -> TimelineEvent {
        TimelineEvent::new(
            event_id,
            42,
            observed_at.to_string(),
            9,
            "abc".to_string(),
            SnapshotKind::PlainText,
            "git status".to_string(),
            "git status".to_string(),
            Some("Terminal".to_string()),
            Some("com.apple.Terminal".to_string()),
            Vec::new(),
            Vec::new(),
            10,
            1,
        )
    }

    fn assert_error_contains(error: anyhow::Error, expected: &str) {
        let message = error.to_string();
        assert!(
            message.contains(expected),
            "expected `{message}` to contain `{expected}`"
        );
    }
}
