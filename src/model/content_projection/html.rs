use std::collections::HashSet;

use super::{ProjectionDiagnostic, TextProjectionResult};

const MAX_INPUT_BYTES: usize = 1_000_000;
const MAX_OUTPUT_CHARS: usize = 200_000;
const MAX_TAGS: usize = 100_000;

const HIDDEN: &[&str] = &["script", "style", "noscript", "template", "head"];
const BLOCKS: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "br",
    "dd",
    "div",
    "dl",
    "dt",
    "figcaption",
    "figure",
    "footer",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hr",
    "li",
    "main",
    "nav",
    "ol",
    "p",
    "pre",
    "section",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "tr",
    "ul",
];

pub fn project_html(input: &str) -> TextProjectionResult {
    let (input, mut diagnostics) = bounded_input(input);
    let mut out = String::new();
    let mut urls = Vec::new();
    let mut seen_urls = HashSet::new();
    let mut tags = 0usize;
    let mut cursor = 0usize;

    while cursor < input.len() && tags < MAX_TAGS {
        let rest = &input[cursor..];
        let Some(relative) = rest.find('<') else {
            push_decoded_text(&mut out, rest);
            cursor = input.len();
            break;
        };
        push_decoded_text(&mut out, &rest[..relative]);
        cursor += relative;

        if input[cursor..].starts_with("<!--") {
            if let Some(end) = input[cursor + 4..].find("-->") {
                cursor += 4 + end + 3;
            } else {
                diagnostics.push(ProjectionDiagnostic::MalformedMarkup);
                break;
            }
            continue;
        }

        let Some(end) = find_tag_end(&input[cursor + 1..]) else {
            diagnostics.push(ProjectionDiagnostic::MalformedMarkup);
            // No closing delimiter remains. Consume the malformed tail once.
            push_decoded_text(&mut out, &input[cursor..]);
            break;
        };
        tags += 1;
        let raw = &input[cursor + 1..cursor + 1 + end];
        cursor += end + 2;
        let tag = parse_tag(raw);
        if tag.name.is_empty() {
            continue;
        }
        let hidden = HIDDEN.contains(&tag.name.as_str());
        if hidden && !tag.closing {
            let Some(after_close) = find_hidden_close(input, cursor, &tag.name) else {
                diagnostics.push(ProjectionDiagnostic::MalformedMarkup);
                cursor = input.len();
                break;
            };
            cursor = after_close;
            tags += 1;
            continue;
        }
        if BLOCKS.contains(&tag.name.as_str()) {
            push_boundary(&mut out);
        }
        for value in tag.urls {
            let decoded = decode_entities(&value).trim().to_string();
            if !decoded.is_empty() && seen_urls.insert(decoded.to_ascii_lowercase()) {
                urls.push(decoded);
            }
        }
    }
    if tags == MAX_TAGS && cursor < input.len() {
        diagnostics.push(ProjectionDiagnostic::InputTruncated);
    }
    finish(out, urls, diagnostics)
}

fn find_raw_text_close(input: &str, start: usize, name: &str) -> Option<usize> {
    let rest = &input[start..];
    for (relative, _) in rest.match_indices("</") {
        let name_start = start + relative + 2;
        let Some(candidate) = input.get(name_start..name_start + name.len()) else {
            continue;
        };
        if !candidate.eq_ignore_ascii_case(name) {
            continue;
        }
        let after_name = name_start + name.len();
        if !input[after_name..]
            .chars()
            .next()
            .is_some_and(|ch| ch == '>' || ch.is_ascii_whitespace())
        {
            continue;
        }
        let close_end = input[after_name..].find('>')?;
        return Some(after_name + close_end + 1);
    }
    None
}

struct Tag {
    name: String,
    closing: bool,
    urls: Vec<String>,
}

fn parse_tag(raw: &str) -> Tag {
    let trimmed = raw.trim();
    let closing = trimmed.starts_with('/');
    let body = trimmed.trim_start_matches('/').trim_start();
    let name = body
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect::<String>()
        .to_ascii_lowercase();
    Tag {
        name,
        closing,
        urls: extract_url_attributes(body),
    }
}

fn extract_url_attributes(body: &str) -> Vec<String> {
    let mut values = Vec::new();
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && !bytes[i].is_ascii_alphabetic() {
            i += 1;
        }
        let start = i;
        while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'-') {
            i += 1;
        }
        let name = body[start..i].to_ascii_lowercase();
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b'=' {
            continue;
        }
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let quote = bytes.get(i).copied().filter(|b| matches!(b, b'\'' | b'"'));
        if quote.is_some() {
            i += 1;
        }
        let value_start = i;
        while i < bytes.len()
            && quote.map_or(!bytes[i].is_ascii_whitespace() && bytes[i] != b'>', |q| {
                bytes[i] != q
            })
        {
            i += 1;
        }
        if matches!(name.as_str(), "href" | "src") && value_start < i {
            values.push(body[value_start..i].to_string());
        }
        if quote.is_some() && i < bytes.len() {
            i += 1;
        }
    }
    values
}

fn find_tag_end(input: &str) -> Option<usize> {
    let mut quote = None;
    for (i, ch) in input.char_indices() {
        match (quote, ch) {
            (None, '\'' | '"') => quote = Some(ch),
            (Some(q), c) if q == c => quote = None,
            (None, '>') => return Some(i),
            _ => {}
        }
    }
    None
}

fn bounded_input(input: &str) -> (&str, Vec<ProjectionDiagnostic>) {
    if input.len() <= MAX_INPUT_BYTES {
        return (input, Vec::new());
    }
    let mut end = MAX_INPUT_BYTES;
    while !input.is_char_boundary(end) {
        end -= 1;
    }
    (&input[..end], vec![ProjectionDiagnostic::InputTruncated])
}

fn push_decoded_text(out: &mut String, text: &str) {
    out.push_str(&decode_entities(text));
}
fn push_boundary(out: &mut String) {
    if !out.ends_with('\n') {
        out.push('\n');
    }
}

fn finish(
    out: String,
    urls: Vec<String>,
    mut diagnostics: Vec<ProjectionDiagnostic>,
) -> TextProjectionResult {
    let normalized = normalize_lines(&out);
    let text = if normalized.chars().count() > MAX_OUTPUT_CHARS {
        diagnostics.push(ProjectionDiagnostic::OutputTruncated);
        normalized.chars().take(MAX_OUTPUT_CHARS).collect()
    } else {
        normalized
    };
    TextProjectionResult {
        text,
        urls,
        diagnostics,
    }
}

fn normalize_lines(text: &str) -> String {
    let mut lines = Vec::new();
    for line in text.lines() {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() {
            if lines.last().is_some_and(|v: &String| !v.is_empty()) {
                lines.push(String::new());
            }
        } else {
            lines.push(line);
        }
    }
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines.join("\n")
}

fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    while let Some(relative) = text[cursor..].find('&') {
        let at = cursor + relative;
        out.push_str(&text[cursor..at]);
        let tail = &text[at + 1..];
        let Some(end) = tail
            .as_bytes()
            .iter()
            .take(33)
            .position(|byte| *byte == b';')
        else {
            out.push('&');
            cursor = at + 1;
            continue;
        };
        let entity = &tail[..end];
        if let Some(decoded) = decode_entity(entity) {
            out.push_str(&decoded);
            cursor = at + end + 2;
        } else {
            out.push('&');
            cursor = at + 1;
        }
    }
    out.push_str(&text[cursor..]);
    out
}

fn decode_entity(entity: &str) -> Option<String> {
    if let Some(number) = entity.strip_prefix('#') {
        let value = if let Some(hex) = number
            .strip_prefix('x')
            .or_else(|| number.strip_prefix('X'))
        {
            u32::from_str_radix(hex, 16).ok()?
        } else {
            number.parse::<u32>().ok()?
        };
        let ch = if (0x80..=0x9f).contains(&value) {
            web_atoms::C1_REPLACEMENTS[(value - 0x80) as usize].unwrap_or(char::from_u32(value)?)
        } else {
            char::from_u32(value)
                .filter(|ch| *ch != '\0')
                .unwrap_or('�')
        };
        return Some(ch.to_string());
    }
    let &(first, second) = web_atoms::NAMED_ENTITIES.get(&format!("{entity};"))?;
    let mut decoded = char::from_u32(first)?.to_string();
    if second != 0 {
        decoded.push(char::from_u32(second)?);
    }
    Some(decoded)
}

fn find_hidden_close(input: &str, start: usize, name: &str) -> Option<usize> {
    if matches!(name, "script" | "style") {
        return find_raw_text_close(input, start, name);
    }
    let mut depth = 1;
    let mut cursor = start;
    while let Some(relative) = input[cursor..].find('<') {
        cursor += relative;
        if input[cursor..].starts_with("<!--") {
            cursor += 4 + input[cursor + 4..].find("-->")? + 3;
            continue;
        }
        let end = find_tag_end(&input[cursor + 1..])?;
        let tag = parse_tag(&input[cursor + 1..cursor + 1 + end]);
        cursor += end + 2;
        if tag.name == name {
            if tag.closing {
                depth -= 1;
            } else {
                depth += 1;
            }
            if depth == 0 {
                return Some(cursor);
            }
        } else if !tag.closing && matches!(tag.name.as_str(), "script" | "style") {
            cursor = find_raw_text_close(input, cursor, &tag.name)?;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::project_html;
    use crate::model::ProjectionDiagnostic;

    #[test]
    fn nested_hidden_content_and_named_entities() {
        let p = project_html("<template>outer<template>inner</template>still hidden</template><p>&copy; &eacute; &NotEqualTilde;</p>");
        assert_eq!(p.text, "© é ≂̸");
    }

    #[test]
    fn long_unterminated_markup_and_entities_are_consumed_once() {
        let tags = "<".repeat(100_000);
        let projected = project_html(&tags);
        assert_eq!(projected.text, tags);
        assert!(projected
            .diagnostics
            .contains(&ProjectionDiagnostic::MalformedMarkup));
        let entities = "&".repeat(100_000);
        assert_eq!(project_html(&entities).text, entities);
    }

    #[test]
    fn visible_text_boundaries_entities_and_urls() {
        let p = project_html(
            r#"<head><title>no</title></head><p>Hello &amp; <b>world</b></p><script>bad()</script><a href="https://example.com?a=1&amp;b=2">link</a>"#,
        );
        assert_eq!(p.text, "Hello & world\nlink");
        assert_eq!(p.urls, ["https://example.com?a=1&b=2"]);
    }

    #[test]
    fn malformed_markup_is_best_effort() {
        assert_eq!(project_html("hello <broken").text, "hello <broken");
    }

    #[test]
    fn script_markup_like_strings_do_not_hide_following_content() {
        let projected = project_html(r#"<script>const x = "<style>";</script><p>Visible</p>"#);

        assert_eq!(projected.text, "Visible");
        assert!(projected.diagnostics.is_empty());
    }

    #[test]
    fn style_raw_text_ends_at_first_matching_close_tag() {
        let projected = project_html(
            r#"<style>.x::after { content: "</style>"; color: red }</style><p>Visible</p>"#,
        );

        assert_eq!(projected.text, "\"; color: red }\nVisible");
        assert!(projected.diagnostics.is_empty());
    }

    #[test]
    fn unterminated_script_hides_remaining_text_with_diagnostic() {
        let projected = project_html("Before<script>const x = '<p>hidden</p>';");

        assert_eq!(projected.text, "Before");
        assert!(projected
            .diagnostics
            .contains(&ProjectionDiagnostic::MalformedMarkup));
    }
}
