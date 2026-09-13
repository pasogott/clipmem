use super::*;

#[test]
fn url_only_html_enters_builder_v3_search_document() -> Result<()> {
    let mut db = Database::open_in_memory()?;
    let html = r#"<img src="https://example.com/image.png">"#;
    let snapshot = build_snapshot(
        CaptureContext::new(1),
        vec![build_item(
            0,
            vec![build_representation(
                "public.html".to_string(),
                Some(html.to_string()),
                html.as_bytes().to_vec(),
            )],
        )],
    );
    let stored = db.store_capture(&snapshot)?;

    let (builder_version, url_text): (i64, String) = db.conn.query_row(
        "SELECT builder_version, url_text FROM snapshot_search_documents WHERE snapshot_id = ?1",
        [stored.snapshot_id()],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;

    assert_eq!(builder_version, 4);
    assert_eq!(url_text, "https://example.com/image.png");
    Ok(())
}

#[test]
fn mixed_native_and_html_urls_merge_into_search_document() -> Result<()> {
    let mut db = Database::open_in_memory()?;
    let native = "https://native.example/path";
    let html_url = "https://html.example/article";
    let html = format!(r#"<a href="{native}"></a><a href="{html_url}"></a>"#);
    let snapshot = build_snapshot(
        CaptureContext::new(1),
        vec![build_item(
            0,
            vec![
                build_representation(
                    "public.url".to_string(),
                    Some(native.to_string()),
                    native.as_bytes().to_vec(),
                ),
                build_representation(
                    "public.html".to_string(),
                    Some(html.clone()),
                    html.into_bytes(),
                ),
            ],
        )],
    );
    let stored = db.store_capture(&snapshot)?;

    let url_text: String = db.conn.query_row(
        "SELECT url_text FROM snapshot_search_documents WHERE snapshot_id = ?1",
        [stored.snapshot_id()],
        |row| row.get(0),
    )?;

    assert_eq!(url_text, format!("{native}\n{html_url}"));
    assert_eq!(
        db.search_literal(native, 10, &unfiltered())?.hits().len(),
        1
    );
    assert_eq!(
        db.search_literal(html_url, 10, &unfiltered())?.hits().len(),
        1
    );
    assert_eq!(
        db.doctor_verifying_invariants()?
            .integrity()
            .context("integrity should be verified")?
            .projection_mismatch_count(),
        0
    );
    Ok(())
}
