use std::collections::HashSet;

use serde::Serialize;

use crate::file_url::projected_file_url_path;

use super::builders::{normalize_whitespace, truncate_chars};
use super::clipboard::ClipboardItem;
use super::content_projection::{
    content_roles, project_html, project_rtf, CopyTextRole, DisplayRole, ProjectionDiagnostic,
    TEXT_PROJECTION_VERSION,
};
use super::kinds::ClipboardKind;

const TEXT_SUMMARY_LIMIT: usize = 320;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TextFragment {
    text: String,
    uti: String,
    kind: ClipboardKind,
    item_index: usize,
    representation_index: usize,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct FlattenedTextProjection {
    best_text: String,
    best_text_uti: Option<String>,
    text_fragments: Vec<TextFragment>,
    urls: Vec<String>,
    file_paths: Vec<String>,
    html_text: Option<String>,
    rtf_text: Option<String>,
    text_summary: String,
    ocr_text: Option<String>,
    ocr_status: Option<String>,
    projection_version: u32,
    diagnostics: Vec<ProjectionDiagnostic>,
}

#[derive(Debug, Clone)]
struct BestTextCandidate {
    priority: u8,
    text: String,
    uti: String,
}

impl TextFragment {
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    #[must_use]
    pub fn uti(&self) -> &str {
        &self.uti
    }

    #[must_use]
    pub fn kind(&self) -> ClipboardKind {
        self.kind
    }

    #[must_use]
    pub fn item_index(&self) -> usize {
        self.item_index
    }

    #[must_use]
    pub fn representation_index(&self) -> usize {
        self.representation_index
    }
}

impl FlattenedTextProjection {
    #[must_use]
    pub fn from_items(items: &[ClipboardItem]) -> Self {
        let mut fragments = Vec::new();
        let mut fragment_keys = HashSet::new();
        let mut urls = Vec::new();
        let mut url_keys = HashSet::new();
        let mut file_paths = Vec::new();
        let mut file_path_keys = HashSet::new();
        let mut html_fragments = Vec::new();
        let mut html_keys = HashSet::new();
        let mut rtf_fragments = Vec::new();
        let mut rtf_keys = HashSet::new();
        let mut best_candidate: Option<BestTextCandidate> = None;
        let mut diagnostics = Vec::new();

        for item in items {
            for (representation_index, representation) in item.representations().iter().enumerate()
            {
                let roles = content_roles(representation.uti(), representation.kind());

                let Some(text_value) = representation.text_value() else {
                    continue;
                };

                let Some(normalized_text) = normalize_nonempty_text(text_value) else {
                    continue;
                };
                let projected = projected_fragment(representation.kind(), text_value);
                diagnostics.extend(projected.diagnostics.iter().cloned());
                if representation.kind() == ClipboardKind::Html {
                    for url in &projected.urls {
                        push_distinct(&mut urls, &mut url_keys, url.clone());
                    }
                }
                let projected_text = projected.text;
                let fragment_text = if matches!(
                    representation.kind(),
                    ClipboardKind::PlainText | ClipboardKind::Json | ClipboardKind::Xml
                ) {
                    (!projected_text.trim().is_empty()).then_some(projected_text)
                } else {
                    normalize_nonempty_display_text(&projected_text)
                };
                let Some(fragment_text) = fragment_text else {
                    continue;
                };

                if roles.copy_text != CopyTextRole::Excluded {
                    if let Some(priority) =
                        best_text_priority(representation.kind(), &fragment_text)
                    {
                        consider_best_candidate(
                            &mut best_candidate,
                            BestTextCandidate {
                                priority,
                                text: fragment_text.clone(),
                                uti: representation.uti().to_string(),
                            },
                        );
                    }
                }

                let fragment_key = normalize_whitespace(&fragment_text).to_lowercase();
                if matches!(
                    roles.display,
                    DisplayRole::Primary | DisplayRole::Supplemental
                ) && fragment_keys.insert(fragment_key)
                {
                    fragments.push(TextFragment {
                        text: fragment_text.clone(),
                        uti: representation.uti().to_string(),
                        kind: representation.kind(),
                        item_index: item.item_index(),
                        representation_index,
                    });
                }

                match representation.kind() {
                    ClipboardKind::Url => {
                        push_distinct(&mut urls, &mut url_keys, normalized_text.clone())
                    }
                    ClipboardKind::FileUrl => {
                        if let Some(decoded_path) = projected_file_url_path(&normalized_text) {
                            push_distinct(&mut file_paths, &mut file_path_keys, decoded_path);
                        }
                    }
                    ClipboardKind::Html => {
                        let plain = fragment_text.clone();
                        if let Some(normalized_plain) = normalize_nonempty_display_text(&plain) {
                            push_distinct_display(
                                &mut html_fragments,
                                &mut html_keys,
                                normalized_plain,
                            );
                        }
                    }
                    ClipboardKind::Rtf => {
                        let plain = fragment_text;
                        if let Some(normalized_plain) = normalize_nonempty_display_text(&plain) {
                            push_distinct_display(
                                &mut rtf_fragments,
                                &mut rtf_keys,
                                normalized_plain,
                            );
                        }
                    }
                    _ => {}
                }
            }
        }

        let best_text = best_candidate
            .as_ref()
            .map(|candidate| candidate.text.clone())
            .unwrap_or_default();
        let best_text_uti = best_candidate.map(|candidate| candidate.uti);
        let html_text = join_optional_fragments(&html_fragments);
        let rtf_text = join_optional_fragments(&rtf_fragments);
        let text_summary = truncate_chars(&fragments_text_summary(&fragments), TEXT_SUMMARY_LIMIT);

        Self {
            best_text,
            best_text_uti,
            text_fragments: fragments,
            urls,
            file_paths,
            html_text,
            rtf_text,
            text_summary,
            ocr_text: None,
            ocr_status: None,
            projection_version: TEXT_PROJECTION_VERSION,
            diagnostics,
        }
    }

    #[must_use]
    pub fn with_ocr(mut self, ocr_text: Option<String>, ocr_status: Option<String>) -> Self {
        let normalized_ocr_text = ocr_text.and_then(|text| normalize_nonempty_text(&text));
        if self.best_text.trim().is_empty() {
            if let Some(text) = normalized_ocr_text.as_ref() {
                self.best_text = text.clone();
                self.best_text_uti = Some("com.clipmem.ocr.text".to_string());
                self.text_summary = truncate_chars(text, TEXT_SUMMARY_LIMIT);
            }
        }
        self.ocr_text = normalized_ocr_text;
        self.ocr_status = ocr_status;
        self
    }

    #[must_use]
    pub fn with_native_text_fallback(mut self, native_text: Option<String>) -> Self {
        if !self.text_fragments.is_empty() {
            return self;
        }
        let Some(text) = native_text.and_then(|text| normalize_nonempty_display_text(&text)) else {
            return self;
        };
        let uti = "com.clipmem.search-document.native-text".to_string();
        self.best_text = text.clone();
        self.best_text_uti = Some(uti.clone());
        self.text_summary = truncate_chars(&text, TEXT_SUMMARY_LIMIT);
        self.text_fragments.push(TextFragment {
            text,
            uti,
            kind: ClipboardKind::PlainText,
            item_index: 0,
            representation_index: 0,
        });
        self
    }

    #[must_use]
    pub fn best_text(&self) -> &str {
        &self.best_text
    }

    #[must_use]
    pub fn best_text_uti(&self) -> Option<&str> {
        self.best_text_uti.as_deref()
    }

    #[must_use]
    pub fn text_fragments(&self) -> &[TextFragment] {
        &self.text_fragments
    }

    #[must_use]
    pub fn urls(&self) -> &[String] {
        &self.urls
    }

    #[must_use]
    pub fn file_paths(&self) -> &[String] {
        &self.file_paths
    }

    #[must_use]
    pub fn html_text(&self) -> Option<&str> {
        self.html_text.as_deref()
    }

    #[must_use]
    pub fn rtf_text(&self) -> Option<&str> {
        self.rtf_text.as_deref()
    }

    #[must_use]
    pub fn text_summary(&self) -> &str {
        &self.text_summary
    }

    #[must_use]
    pub fn ocr_text(&self) -> Option<&str> {
        self.ocr_text.as_deref()
    }

    #[must_use]
    pub fn ocr_status(&self) -> Option<&str> {
        self.ocr_status.as_deref()
    }

    #[must_use]
    pub fn projection_version(&self) -> u32 {
        self.projection_version
    }

    #[must_use]
    pub fn diagnostics(&self) -> &[ProjectionDiagnostic] {
        &self.diagnostics
    }
}

fn projected_fragment(kind: ClipboardKind, normalized_text: &str) -> super::TextProjectionResult {
    match kind {
        ClipboardKind::Html => project_html(normalized_text),
        ClipboardKind::Rtf => project_rtf(normalized_text),
        _ => super::TextProjectionResult {
            text: normalized_text.to_string(),
            ..Default::default()
        },
    }
}

fn normalize_nonempty_text(text: &str) -> Option<String> {
    let normalized = normalize_whitespace(text);
    let trimmed = normalized.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn normalize_nonempty_display_text(text: &str) -> Option<String> {
    let normalized_breaks = text.replace("\r\n", "\n").replace('\r', "\n");
    let trimmed = normalized_breaks.trim();
    if trimmed.is_empty() {
        return None;
    }

    Some(
        trimmed
            .split('\n')
            .map(normalize_line_whitespace)
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

fn normalize_line_whitespace(line: &str) -> String {
    let mut parts = line.split_whitespace();
    let Some(first) = parts.next() else {
        return String::new();
    };

    let mut out = String::with_capacity(line.len());
    out.push_str(first);
    for part in parts {
        out.push(' ');
        out.push_str(part);
    }
    out
}

fn push_distinct(values: &mut Vec<String>, seen: &mut HashSet<String>, candidate: String) {
    let normalized = normalize_nonempty_text(&candidate).unwrap_or_default();
    if normalized.is_empty() {
        return;
    }

    let key = normalized.to_lowercase();
    if seen.insert(key) {
        values.push(normalized);
    }
}

fn push_distinct_display(values: &mut Vec<String>, seen: &mut HashSet<String>, candidate: String) {
    let key = normalize_whitespace(&candidate).to_lowercase();
    if !key.is_empty() && seen.insert(key) {
        values.push(candidate);
    }
}

fn join_optional_fragments(fragments: &[String]) -> Option<String> {
    (!fragments.is_empty()).then(|| fragments.join("\n\n"))
}

fn fragments_text_summary(fragments: &[TextFragment]) -> String {
    fragments
        .iter()
        .map(|fragment| fragment.text().to_string())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn consider_best_candidate(current: &mut Option<BestTextCandidate>, candidate: BestTextCandidate) {
    match current {
        Some(existing) if existing.priority <= candidate.priority => {}
        slot => *slot = Some(candidate),
    }
}

fn best_text_priority(kind: ClipboardKind, text: &str) -> Option<u8> {
    if text.is_empty() {
        return None;
    }

    Some(match kind {
        ClipboardKind::PlainText => 0,
        ClipboardKind::Json | ClipboardKind::Xml => 1,
        ClipboardKind::Html => 2,
        ClipboardKind::Rtf => 3,
        ClipboardKind::Url | ClipboardKind::FileUrl => 4,
        _ if kind.is_textual() => 1,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::super::builders::{build_item, build_representation};
    use super::super::kinds::ClipboardKind;
    use super::FlattenedTextProjection;

    #[test]
    fn projection_prefers_plain_text_and_collects_urls_and_paths() {
        let item = build_item(
            0,
            vec![
                build_representation(
                    "public.url".to_string(),
                    Some("https://example.com".to_string()),
                    b"https://example.com".to_vec(),
                ),
                build_representation(
                    "public.file-url".to_string(),
                    Some("file:///Users/test/Report%20Q2.txt".to_string()),
                    b"file:///Users/test/Report%20Q2.txt".to_vec(),
                ),
                build_representation(
                    "public.utf8-plain-text".to_string(),
                    Some("git status".to_string()),
                    b"git status".to_vec(),
                ),
            ],
        );

        let projection = FlattenedTextProjection::from_items(&[item]);

        assert_eq!(projection.best_text(), "git status");
        assert_eq!(projection.best_text_uti(), Some("public.utf8-plain-text"));
        assert_eq!(projection.urls(), &["https://example.com".to_string()]);
        assert_eq!(
            projection.file_paths(),
            &["/Users/test/Report Q2.txt".to_string()]
        );
        assert_eq!(projection.text_fragments().len(), 1);
    }

    #[test]
    fn projection_excludes_chromium_source_url_metadata_from_urls() {
        let item = build_item(
            0,
            vec![
                build_representation(
                    "org.chromium.source-url".to_string(),
                    Some("https://example.com/source-page".to_string()),
                    b"https://example.com/source-page".to_vec(),
                ),
                build_representation(
                    "public.utf8-plain-text".to_string(),
                    Some("selected article text".to_string()),
                    b"selected article text".to_vec(),
                ),
            ],
        );

        let projection = FlattenedTextProjection::from_items(&[item]);

        assert_eq!(projection.best_text(), "selected article text");
        assert!(projection.urls().is_empty());
        assert_eq!(projection.text_fragments().len(), 1);
    }

    #[test]
    fn projection_decodes_localhost_file_urls_to_absolute_paths() {
        let item = build_item(
            0,
            vec![build_representation(
                "public.file-url".to_string(),
                Some("file://localhost/Users/test/Report%20Q2.txt".to_string()),
                b"file://localhost/Users/test/Report%20Q2.txt".to_vec(),
            )],
        );

        let projection = FlattenedTextProjection::from_items(&[item]);

        assert_eq!(
            projection.file_paths(),
            &["/Users/test/Report Q2.txt".to_string()]
        );
    }

    #[test]
    fn projection_decodes_localhost_file_urls_case_insensitively() {
        let item = build_item(
            0,
            vec![build_representation(
                "public.file-url".to_string(),
                Some("file://LOCALHOST/Users/test/Report%20Q2.txt".to_string()),
                b"file://LOCALHOST/Users/test/Report%20Q2.txt".to_vec(),
            )],
        );

        let projection = FlattenedTextProjection::from_items(&[item]);

        assert_eq!(
            projection.file_paths(),
            &["/Users/test/Report Q2.txt".to_string()]
        );
    }

    #[test]
    fn projection_extracts_html_and_rtf_plain_text() {
        let item = build_item(
            0,
            vec![
                build_representation(
                    "public.html".to_string(),
                    Some("<p>Hello <strong>world</strong></p>".to_string()),
                    b"<p>Hello <strong>world</strong></p>".to_vec(),
                ),
                build_representation(
                    "public.rtf".to_string(),
                    Some(r"{\rtf1\ansi hello\par world}".to_string()),
                    br"{\rtf1\ansi hello\par world}".to_vec(),
                ),
            ],
        );

        let projection = FlattenedTextProjection::from_items(&[item]);

        assert_eq!(projection.best_text_uti(), Some("public.html"));
        assert_eq!(projection.html_text(), Some("Hello world"));
        assert_eq!(projection.rtf_text(), Some("hello\nworld"));
        assert!(!projection.text_summary().is_empty());
    }

    #[test]
    fn projection_dedupes_text_fragments_preserving_first_provenance() {
        let item = build_item(
            0,
            vec![
                build_representation(
                    "public.utf8-plain-text".to_string(),
                    Some(" Git   Status ".to_string()),
                    b" Git   Status ".to_vec(),
                ),
                build_representation(
                    "public.json".to_string(),
                    Some("git status".to_string()),
                    b"git status".to_vec(),
                ),
            ],
        );

        let projection = FlattenedTextProjection::from_items(&[item]);

        assert_eq!(projection.text_fragments().len(), 1);
        assert_eq!(
            projection.text_fragments()[0].kind(),
            ClipboardKind::PlainText
        );
        assert_eq!(projection.text_fragments()[0].representation_index(), 0);
    }

    #[test]
    fn projection_preserves_plain_text_line_breaks_for_display_text() {
        let markdown = "# Release notes\n**Markdown rendering** is now live.";
        let item = build_item(
            0,
            vec![build_representation(
                "public.utf8-plain-text".to_string(),
                Some(markdown.to_string()),
                markdown.as_bytes().to_vec(),
            )],
        );

        let projection = FlattenedTextProjection::from_items(&[item]);

        assert_eq!(projection.best_text(), markdown);
        assert_eq!(projection.text_summary(), markdown);
        assert_eq!(projection.text_fragments()[0].text(), markdown);
    }

    #[test]
    fn url_only_image_html_contributes_its_source_url() {
        let html = r#"<img src="https://example.com/image.png">"#;
        let item = build_item(
            0,
            vec![build_representation(
                "public.html".to_string(),
                Some(html.to_string()),
                html.as_bytes().to_vec(),
            )],
        );

        let projection = FlattenedTextProjection::from_items(&[item]);

        assert!(projection.text_fragments().is_empty());
        assert_eq!(projection.urls(), &["https://example.com/image.png"]);
    }

    #[test]
    fn empty_anchor_html_contributes_its_target_url() {
        let html = r#"<a href="https://example.com/target"></a>"#;
        let item = build_item(
            0,
            vec![build_representation(
                "public.html".to_string(),
                Some(html.to_string()),
                html.as_bytes().to_vec(),
            )],
        );

        let projection = FlattenedTextProjection::from_items(&[item]);

        assert!(projection.text_fragments().is_empty());
        assert_eq!(projection.urls(), &["https://example.com/target"]);
    }
}

#[cfg(test)]
mod source_fidelity_tests {
    use super::*;
    use crate::model::{build_item, build_representation};

    #[test]
    fn plain_json_and_xml_copy_text_preserves_exact_whitespace() {
        for (uti, text) in [
            (
                "public.utf8-plain-text",
                "  def run():\r\n\tprint(\"a  b\")\r\n",
            ),
            ("public.json", "{\n  \"value\": \"a  b\"\n}\n"),
            ("public.xml", "<text xml:space=\"preserve\"> a  b </text>\n"),
        ] {
            let items = [build_item(
                0,
                vec![build_representation(
                    uti.into(),
                    Some(text.into()),
                    text.as_bytes().to_vec(),
                )],
            )];
            let projection = FlattenedTextProjection::from_items(&items);
            assert_eq!(projection.best_text(), text, "{uti}");
            assert_eq!(projection.text_fragments()[0].text(), text, "{uti}");
        }
    }
}
