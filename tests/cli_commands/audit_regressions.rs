use super::*;

#[test]
fn search_cursor_rejects_mutated_and_different_archives() -> Result<()> {
    let path = temp_db_path("cursor-archive-revision");
    seed_database(
        &path,
        &[text_snapshot(1, "git status"), text_snapshot(2, "git log")],
    )?;
    let first = run_cli(&[
        "--db",
        path.to_str().unwrap(),
        "search",
        "git",
        "--limit",
        "1",
        "--json",
    ]);
    assert!(first.status.success(), "{}", stderr_text(&first));
    let payload: Value = serde_json::from_slice(&first.stdout)?;
    let cursor = payload["next_cursor"].as_str().unwrap();
    let next = run_cli(&[
        "--db",
        path.to_str().unwrap(),
        "search",
        "git",
        "--limit",
        "1",
        "--cursor",
        cursor,
        "--json",
    ]);
    assert!(next.status.success(), "{}", stderr_text(&next));
    seed_database(&path, &[text_snapshot(3, "unrelated newer clipboard")])?;
    let stale = run_cli(&[
        "--db",
        path.to_str().unwrap(),
        "search",
        "git",
        "--cursor",
        cursor,
        "--json",
    ]);
    assert!(!stale.status.success());
    assert!(stderr_text(&stale).contains("cursor is stale"));
    let other = temp_db_path("cursor-other-archive");
    seed_database(
        &other,
        &[text_snapshot(1, "git status"), text_snapshot(2, "git log")],
    )?;
    let foreign = run_cli(&[
        "--db",
        other.to_str().unwrap(),
        "search",
        "git",
        "--cursor",
        cursor,
        "--json",
    ]);
    assert!(!foreign.status.success());
    assert!(stderr_text(&foreign).contains("another archive"));
    cleanup_db(&path);
    cleanup_db(&other);
    Ok(())
}

#[test]
fn combined_app_and_bundle_filters_require_one_matching_copy_event() -> Result<()> {
    let path = temp_db_path("correlated-app-filter");
    seed_database(
        &path,
        &[
            app_text_snapshot(1, "FirstApp", "com.first", "same content"),
            app_text_snapshot(2, "SecondApp", "com.second", "same content"),
        ],
    )?;
    for command in ["recent", "search"] {
        let mut args = vec!["--db", path.to_str().unwrap(), command];
        if command == "search" {
            args.push("same");
        }
        args.extend(["--app", "FirstApp", "--bundle-id", "com.second", "--json"]);
        let output = run_cli(&args);
        assert!(output.status.success(), "{}", stderr_text(&output));
        let payload: Value = serde_json::from_slice(&output.stdout)?;
        assert_eq!(payload["results"].as_array().unwrap().len(), 0);
    }
    cleanup_db(&path);
    Ok(())
}

#[test]
fn recall_stats_and_get_support_human_output() -> Result<()> {
    let path = temp_db_path("recall-stats-get-human");
    let ids = seed_database(
        &path,
        &[
            text_snapshot(1, "cargo test --package clipmem"),
            app_text_snapshot(2, "Safari", "com.apple.Safari", "release notes draft"),
        ],
    )?;

    let recall = run_cli(&[
        "--db",
        path.to_str().expect("db path should be UTF-8"),
        "recall",
        "cargo test",
        "--human",
    ]);
    let recall_stdout = stdout_text(&recall);
    assert!(recall.status.success());
    assert_human_output(&recall_stdout, "clipmem Recall");
    assert!(recall_stdout.contains("Best Match"));
    assert!(recall_stdout.contains("cargo test"));
    assert!(recall_stdout.contains("Provenance"));

    let stats = run_cli(&[
        "--db",
        path.to_str().expect("db path should be UTF-8"),
        "stats",
        "--human",
    ]);
    let stats_stdout = stdout_text(&stats);
    assert!(stats.status.success());
    assert_human_output(&stats_stdout, "clipmem Archive Stats");
    assert!(stats_stdout.contains("Dedupe meter"));
    assert!(stats_stdout.contains("Content Mix"));
    assert!(stats_stdout.contains("Top Apps"));

    let get = run_cli(&[
        "--db",
        path.to_str().expect("db path should be UTF-8"),
        "get",
        &ids[0].to_string(),
        "--human",
    ]);
    let get_stdout = stdout_text(&get);
    assert!(get.status.success());
    assert_human_output(&get_stdout, "clipmem Snapshot");
    assert!(get_stdout.contains("Items"));
    assert!(get_stdout.contains("cargo test"));

    cleanup_db(&path);
    Ok(())
}

#[test]
fn malformed_queries_and_unicode_durations_are_validation_errors() -> Result<()> {
    let path = temp_db_path("invalid-query-regression");
    seed_database(&path, &[text_snapshot(1, "ordinary text")])?;
    let path_str = path.to_str().unwrap();
    for args in [
        vec![
            "--db",
            path_str,
            "search",
            "\"unterminated",
            "--mode",
            "fts",
        ],
        vec![
            "--db",
            path_str,
            "recent",
            "--hours",
            "18446744073709551615",
        ],
        vec!["--db", path_str, "settings", "retention", "é"],
    ] {
        let output = run_cli(&args);
        assert_eq!(status_code(&output), 2, "{}", stderr_text(&output));
        assert!(!stderr_text(&output).contains("panicked"));
        assert!(!stderr_text(&output).contains("move aside"));
    }
    cleanup_db(&path);
    Ok(())
}

#[cfg(unix)]
#[test]
fn early_closing_output_consumers_exit_successfully_in_every_format() -> Result<()> {
    let path = temp_db_path("broken-pipe-regression");
    seed_database(
        &path,
        &[text_snapshot(1, &"clipboard text\n".repeat(30_000))],
    )?;
    for format in ["text", "human", "json", "jsonl", "md", "toon"] {
        let mut child = process::Command::new(env!("CARGO_BIN_EXE_clipmem"))
            .args(["--db", path.to_str().unwrap(), "recent", "--format", format])
            .stdout(process::Stdio::piped())
            .stderr(process::Stdio::piped())
            .spawn()?;
        drop(child.stdout.take());
        let output = child.wait_with_output()?;
        assert!(
            output.status.success(),
            "{format}: {}",
            stderr_text(&output)
        );
        assert!(!stderr_text(&output).contains("panicked"));
    }
    cleanup_db(&path);
    Ok(())
}

#[cfg(unix)]
#[test]
fn setup_check_reports_paused_capture_and_preserves_pause() -> Result<()> {
    let root = temp_test_dir("paused-setup-check");
    fs::create_dir_all(&root)?;
    write_executable(
        &root.join("clipmem"),
        r#"#!/bin/sh
case "$1" in
 --version) echo 'clipmem test' ;;
 doctor) echo '{"fts5_create_virtual_table_ok":true}' ;;
 service) echo '{"paused":true,"homebrew":{"running":false,"loaded":false},"launchagent":{"running":true,"loaded":true},"stale":false,"recent_capture_within_last_hour":true,"conflict":false}' ;;
 agents) exit 0 ;;
 *) echo 'unexpected mutation' >&2; exit 99 ;;
esac
"#,
    )?;
    let path_value = format!("{}:/usr/bin:/bin", root.display());
    let output = run_command_with_env(
        Path::new("skills/clipboard-memory/scripts/check-setup.sh"),
        &["--json"],
        &[("PATH", &path_value)],
    );
    assert_eq!(status_code(&output), 4, "{}", stderr_text(&output));
    assert!(stderr_text(&output).is_empty(), "{}", stderr_text(&output));
    let result: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(result["paused"], true);
    assert_eq!(result["ok"], false);
    assert!(result["summary"].as_str().unwrap().contains("pause off"));
    fs::remove_dir_all(root)?;
    Ok(())
}
