mod html;
mod rtf;

use serde::Serialize;

use super::ClipboardKind;

pub const TEXT_PROJECTION_VERSION: u32 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayRole {
    Primary,
    Supplemental,
    Metadata,
    Hidden,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchRole {
    Searchable,
    ExactOnly,
    Excluded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CopyTextRole {
    Preferred,
    Supplemental,
    Excluded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ContentRoles {
    pub display: DisplayRole,
    pub search: SearchRole,
    pub copy_text: CopyTextRole,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionDiagnostic {
    InputTruncated,
    OutputTruncated,
    MalformedMarkup,
    UnsupportedCharset,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextProjectionResult {
    pub text: String,
    pub urls: Vec<String>,
    pub diagnostics: Vec<ProjectionDiagnostic>,
}

pub fn content_roles(uti: &str, kind: ClipboardKind) -> ContentRoles {
    let lower = uti.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "org.chromium.source-url" | "org.chromium.source-title"
    ) {
        return ContentRoles {
            display: DisplayRole::Metadata,
            search: SearchRole::Excluded,
            copy_text: CopyTextRole::Excluded,
        };
    }

    match kind {
        ClipboardKind::PlainText | ClipboardKind::Json | ClipboardKind::Xml => ContentRoles {
            display: DisplayRole::Primary,
            search: SearchRole::Searchable,
            copy_text: CopyTextRole::Preferred,
        },
        ClipboardKind::Html | ClipboardKind::Rtf => ContentRoles {
            display: DisplayRole::Supplemental,
            search: SearchRole::Searchable,
            copy_text: CopyTextRole::Supplemental,
        },
        ClipboardKind::Url | ClipboardKind::FileUrl => ContentRoles {
            display: DisplayRole::Metadata,
            search: SearchRole::ExactOnly,
            copy_text: CopyTextRole::Excluded,
        },
        _ => ContentRoles {
            display: DisplayRole::Hidden,
            search: SearchRole::Excluded,
            copy_text: CopyTextRole::Excluded,
        },
    }
}

pub(crate) fn html_to_text_lossy(html: &str) -> String {
    html::project_html(html).text
}

pub(crate) fn rtf_to_text_lossy(rtf: &str) -> String {
    rtf::project_rtf(rtf).text
}

pub(crate) use html::project_html;
pub(crate) use rtf::project_rtf;
