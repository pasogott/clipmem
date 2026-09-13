use super::*;

#[test]
fn get_json_wraps_snapshot_in_versioned_envelope() -> Result<()> {
    let path = temp_db_path("get-json");
    let ids = seed_database(
        &path,
        &[text_snapshot(1, "git clone https://example.com/repo")],
    )?;

    let output = run_cli(&[
        "--db",
        path.to_str().expect("db path should be UTF-8"),
        "get",
        &ids[0].to_string(),
        "--format",
        "json",
    ]);
    let payload: Value = serde_json::from_slice(&output.stdout).expect("get JSON should parse");

    assert!(output.status.success());
    assert_eq!(payload["schema_version"].as_u64(), Some(2));
    assert_eq!(payload["command"].as_str(), Some("get"));
    assert_eq!(payload["snapshot"]["snapshot_id"].as_i64(), Some(ids[0]));
    assert_eq!(
        payload["snapshot"]["best_text"].as_str(),
        Some("git clone https://example.com/repo")
    );
    assert_eq!(
        payload["snapshot"]["best_text_uti"].as_str(),
        Some("public.utf8-plain-text")
    );
    assert!(payload["snapshot"]["ocr_text"].is_null());
    assert!(payload["snapshot"]["ocr_status"].is_null());
    assert_eq!(
        payload["snapshot"]["text_fragments"][0]["text"].as_str(),
        Some("git clone https://example.com/repo")
    );
    assert!(payload["snapshot"]["urls"]
        .as_array()
        .expect("urls should be an array")
        .is_empty());
    assert!(payload["snapshot"]["file_paths"]
        .as_array()
        .expect("file_paths should be an array")
        .is_empty());
    assert!(payload["snapshot"]["text_summary"]
        .as_str()
        .unwrap_or_default()
        .contains("git clone https://example.com/repo"));
    assert_eq!(
        payload["snapshot"]["preview_text"].as_str(),
        Some("git clone https://example.com/repo")
    );

    cleanup_db(&path);
    Ok(())
}

#[test]
fn jsonl_output_emits_meta_and_result_records() -> Result<()> {
    let path = temp_db_path("search-jsonl");
    seed_database(&path, &[text_snapshot(1, "git status")])?;

    let output = run_cli(&[
        "--db",
        path.to_str().expect("db path should be UTF-8"),
        "search",
        "--mode",
        "literal",
        "--format",
        "jsonl",
        "git",
    ]);
    let stdout = stdout_text(&output);
    let lines = stdout.lines().collect::<Vec<_>>();
    let meta: Value = serde_json::from_str(lines[0]).expect("meta JSONL line should parse");
    let row: Value = serde_json::from_str(lines[1]).expect("result JSONL line should parse");

    assert!(output.status.success());
    assert_eq!(lines.len(), 2);
    assert_eq!(meta["type"].as_str(), Some("meta"));
    assert_eq!(row["type"].as_str(), Some("result"));
    assert_eq!(row["best_text"].as_str(), Some("git status"));

    cleanup_db(&path);
    Ok(())
}

#[test]
fn markdown_output_is_compact_and_scannable() -> Result<()> {
    let path = temp_db_path("search-md");
    seed_database(&path, &[text_snapshot(1, "git status")])?;

    let output = run_cli(&[
        "--db",
        path.to_str().expect("db path should be UTF-8"),
        "search",
        "--format",
        "md",
        "git",
    ]);
    let stdout = stdout_text(&output);

    assert!(output.status.success());
    assert!(stdout.contains("# clipmem search"));
    assert!(stdout.contains("| snapshot_id | kind | observed_at |"));

    cleanup_db(&path);
    Ok(())
}

#[test]
fn toon_output_is_available_for_flattened_list_commands() -> Result<()> {
    let path = temp_db_path("recent-toon");
    seed_database(&path, &[text_snapshot(1, "git status")])?;

    let recent_output = run_cli(&[
        "--db",
        path.to_str().expect("db path should be UTF-8"),
        "recent",
        "--format",
        "toon",
    ]);
    let recent_stdout = stdout_text(&recent_output);

    assert!(recent_output.status.success());
    assert!(recent_stdout.contains(
        "results[1\t]{snapshot_id\tevent_id\tobserved_at\tfirst_seen_at\tlast_seen_at\tkind\tapp_name\tapp_bundle_id\tdisplay_text\tcapture_count\titem_count\ttotal_bytes\tscore\twhy_matched}:"
    ));
    assert!(recent_stdout.contains("git status"));
    assert!(!recent_stdout.contains("sha256"));
    assert!(!recent_stdout.contains("text_fragments"));
    assert!(!recent_stdout.contains("matched_fields"));

    let search_output = run_cli(&[
        "--db",
        path.to_str().expect("db path should be UTF-8"),
        "search",
        "git",
        "--format",
        "toon",
    ]);
    let search_stdout = stdout_text(&search_output);

    assert!(search_output.status.success());
    assert!(search_stdout.contains(
        "results[1\t]{snapshot_id\tevent_id\tobserved_at\tfirst_seen_at\tlast_seen_at\tkind\tapp_name\tapp_bundle_id\tdisplay_text\tcapture_count\titem_count\ttotal_bytes\tscore\twhy_matched}:"
    ));
    assert!(!search_stdout.contains("sha256"));
    assert!(!search_stdout.contains("urls"));

    let timeline_output = run_cli(&[
        "--db",
        path.to_str().expect("db path should be UTF-8"),
        "timeline",
        "--format",
        "toon",
    ]);
    let timeline_stdout = stdout_text(&timeline_output);

    assert!(timeline_output.status.success());
    assert!(timeline_stdout.contains(
        "results[1\t]{event_id\tsnapshot_id\tobserved_at\tchange_count\tkind\tapp_name\tapp_bundle_id\tdisplay_text\titem_count\ttotal_bytes}:"
    ));
    assert!(timeline_stdout.contains("git status"));
    assert!(!timeline_stdout.contains("sha256"));
    assert!(!timeline_stdout.contains("file_paths"));

    cleanup_db(&path);
    Ok(())
}

#[test]
fn get_rejects_toon_output() -> Result<()> {
    let path = temp_db_path("get-toon");
    let ids = seed_database(&path, &[text_snapshot(1, "git status")])?;

    let output = run_cli(&[
        "--db",
        path.to_str().expect("db path should be UTF-8"),
        "get",
        &ids[0].to_string(),
        "--format",
        "toon",
    ]);
    let stderr = stderr_text(&output);

    assert!(!output.status.success());
    assert!(stderr.contains("format toon is only supported for flattened list outputs"));

    cleanup_db(&path);
    Ok(())
}
