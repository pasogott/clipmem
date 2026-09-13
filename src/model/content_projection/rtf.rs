use super::{ProjectionDiagnostic, TextProjectionResult};

const MAX_INPUT_BYTES: usize = 1_000_000;
const MAX_OUTPUT_CHARS: usize = 200_000;
const MAX_GROUP_DEPTH: usize = 512;

#[derive(Clone, Copy)]
struct State {
    skip: bool,
    ignorable: bool,
    uc: usize,
    code_page: u32,
}

pub fn project_rtf(input: &str) -> TextProjectionResult {
    let mut diagnostics = Vec::new();
    let input = if input.len() > MAX_INPUT_BYTES {
        diagnostics.push(ProjectionDiagnostic::InputTruncated);
        let mut end = MAX_INPUT_BYTES;
        while !input.is_char_boundary(end) {
            end -= 1;
        }
        &input[..end]
    } else {
        input
    };
    let bytes = input.as_bytes();
    let mut stack = vec![State {
        skip: false,
        ignorable: false,
        uc: 1,
        code_page: 1252,
    }];
    let mut out = String::new();
    let mut i = 0usize;
    let mut fallback = 0usize;
    let mut pending_high_surrogate = None;
    // Emitted characters are counted incrementally; recounting the whole
    // output every iteration would make near-limit inputs quadratic.
    let mut emitted = 0usize;
    let mut overflow_depth = 0usize;
    while i < bytes.len() && emitted < MAX_OUTPUT_CHARS {
        if fallback > 0 {
            i = skip_fallback(bytes, i);
            fallback -= 1;
            continue;
        }
        if overflow_depth > 0 {
            match bytes[i] {
                b'{' => {
                    overflow_depth = overflow_depth.saturating_add(1);
                    i += 1;
                }
                b'}' => {
                    overflow_depth -= 1;
                    i += 1;
                }
                b'\\' => i = parse_control(bytes, i + 1).0,
                _ => {
                    i += input[i..]
                        .chars()
                        .next()
                        .expect("RTF input is valid UTF-8")
                        .len_utf8();
                }
            }
            continue;
        }
        let appended_from = out.len();
        match bytes[i] {
            b'{' => {
                flush_pending_surrogate(&mut out, &mut diagnostics, &mut pending_high_surrogate);
                if stack.len() < MAX_GROUP_DEPTH {
                    stack.push(*stack.last().unwrap());
                } else {
                    overflow_depth = 1;
                    diagnostics.push(ProjectionDiagnostic::InputTruncated);
                }
                i += 1;
            }
            b'}' => {
                flush_pending_surrogate(&mut out, &mut diagnostics, &mut pending_high_surrogate);
                if stack.len() > 1 {
                    stack.pop();
                } else {
                    diagnostics.push(ProjectionDiagnostic::MalformedMarkup);
                }
                i += 1;
            }
            b'\\' => {
                let (next, control) = parse_control(bytes, i + 1);
                i = next;
                let state = stack.last_mut().unwrap();
                match control {
                    Control::Symbol(b'\\' | b'{' | b'}') if !state.skip => {
                        flush_pending_surrogate(
                            &mut out,
                            &mut diagnostics,
                            &mut pending_high_surrogate,
                        );
                        out.push(control.symbol().unwrap() as char)
                    }
                    Control::Symbol(b'~') if !state.skip => out.push('\u{00a0}'),
                    Control::Symbol(b'_') if !state.skip => out.push('\u{2011}'),
                    Control::Symbol(b'-') if !state.skip => out.push('\u{00ad}'),
                    Control::Symbol(byte) if !byte.is_ascii() => {
                        diagnostics.push(ProjectionDiagnostic::MalformedMarkup);
                    }
                    Control::Symbol(b'*') => {
                        flush_pending_surrogate(
                            &mut out,
                            &mut diagnostics,
                            &mut pending_high_surrogate,
                        );
                        state.ignorable = true;
                    }
                    Control::Hex(value) if !state.skip => {
                        flush_pending_surrogate(
                            &mut out,
                            &mut diagnostics,
                            &mut pending_high_surrogate,
                        );
                        out.push(decode_hex_byte(value, state.code_page, &mut diagnostics));
                    }
                    Control::Word(word, parameter) => {
                        if is_destination(word) || (state.ignorable && !is_known_control(word)) {
                            state.skip = true;
                        }
                        if word == "uc" {
                            if let Some(n) = parameter {
                                state.uc = n.max(0) as usize;
                            }
                        }
                        if word == "ansi" {
                            state.code_page = 1252;
                        }
                        if word == "ansicpg" {
                            if let Some(n) = parameter {
                                state.code_page = u32::try_from(n).unwrap_or(0);
                            }
                        }
                        if word == "u" && !state.skip {
                            if let Some(n) = parameter {
                                push_unicode_unit(
                                    &mut out,
                                    &mut diagnostics,
                                    &mut pending_high_surrogate,
                                    (n as i16) as u16,
                                );
                                fallback = state.uc;
                            }
                        } else {
                            flush_pending_surrogate(
                                &mut out,
                                &mut diagnostics,
                                &mut pending_high_surrogate,
                            );
                        }
                        if !state.skip {
                            match word {
                                "par" | "line" => out.push('\n'),
                                "tab" => out.push('\t'),
                                "emdash" => out.push('—'),
                                "endash" => out.push('–'),
                                "bullet" => out.push('•'),
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
            byte => {
                let literal = input[i..].chars().next().expect("RTF input is valid UTF-8");
                if !stack.last().unwrap().skip && byte >= 0x20 {
                    flush_pending_surrogate(
                        &mut out,
                        &mut diagnostics,
                        &mut pending_high_surrogate,
                    );
                    out.push(literal);
                }
                i += literal.len_utf8();
            }
        }
        emitted += out[appended_from..].chars().count();
    }
    flush_pending_surrogate(&mut out, &mut diagnostics, &mut pending_high_surrogate);
    if stack.len() != 1 {
        diagnostics.push(ProjectionDiagnostic::MalformedMarkup);
    }
    if i < bytes.len() {
        diagnostics.push(ProjectionDiagnostic::OutputTruncated);
    }
    TextProjectionResult {
        text: normalize(&out),
        urls: Vec::new(),
        diagnostics,
    }
}

const WINDOWS_1252_C1: [char; 32] = [
    '\u{20ac}', '\u{fffd}', '\u{201a}', '\u{0192}', '\u{201e}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{02c6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{fffd}', '\u{017d}', '\u{fffd}',
    '\u{fffd}', '\u{2018}', '\u{2019}', '\u{201c}', '\u{201d}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{02dc}', '\u{2122}', '\u{0161}', '\u{203a}', '\u{0153}', '\u{fffd}', '\u{017e}', '\u{0178}',
];

fn decode_hex_byte(byte: u8, code_page: u32, diagnostics: &mut Vec<ProjectionDiagnostic>) -> char {
    if code_page != 1252 {
        diagnostics.push(ProjectionDiagnostic::UnsupportedCharset);
        return '\u{fffd}';
    }
    if !(0x80..=0x9f).contains(&byte) {
        return char::from(byte);
    }
    let decoded = WINDOWS_1252_C1[usize::from(byte - 0x80)];
    if decoded == '\u{fffd}' {
        diagnostics.push(ProjectionDiagnostic::UnsupportedCharset);
    }
    decoded
}

fn push_unicode_unit(
    out: &mut String,
    diagnostics: &mut Vec<ProjectionDiagnostic>,
    pending_high: &mut Option<u16>,
    unit: u16,
) {
    if (0xd800..=0xdbff).contains(&unit) {
        flush_pending_surrogate(out, diagnostics, pending_high);
        *pending_high = Some(unit);
        return;
    }

    if (0xdc00..=0xdfff).contains(&unit) {
        if let Some(high) = pending_high.take() {
            if let Some(decoded) = char::decode_utf16([high, unit]).next().and_then(Result::ok) {
                out.push(decoded);
                return;
            }
        }
        out.push('\u{fffd}');
        diagnostics.push(ProjectionDiagnostic::MalformedMarkup);
        return;
    }

    flush_pending_surrogate(out, diagnostics, pending_high);
    out.push(char::from_u32(u32::from(unit)).unwrap_or('\u{fffd}'));
}

fn flush_pending_surrogate(
    out: &mut String,
    diagnostics: &mut Vec<ProjectionDiagnostic>,
    pending_high: &mut Option<u16>,
) {
    if pending_high.take().is_some() {
        out.push('\u{fffd}');
        diagnostics.push(ProjectionDiagnostic::MalformedMarkup);
    }
}

enum Control<'a> {
    Symbol(u8),
    Hex(u8),
    Word(&'a str, Option<i32>),
}
impl Control<'_> {
    fn symbol(&self) -> Option<u8> {
        if let Self::Symbol(v) = self {
            Some(*v)
        } else {
            None
        }
    }
}

fn parse_control(bytes: &[u8], start: usize) -> (usize, Control<'_>) {
    if start >= bytes.len() {
        return (start, Control::Symbol(0));
    }
    if bytes[start] == b'\'' && start + 2 < bytes.len() {
        if let Ok(hex) = std::str::from_utf8(&bytes[start + 1..start + 3]) {
            if let Ok(v) = u8::from_str_radix(hex, 16) {
                return (start + 3, Control::Hex(v));
            }
        }
    }
    if !bytes[start].is_ascii_alphabetic() {
        let mut end = start + 1;
        while end < bytes.len() && (bytes[end] & 0xc0) == 0x80 {
            end += 1;
        }
        return (end, Control::Symbol(bytes[start]));
    }
    let mut i = start;
    while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
        i += 1;
    }
    let word = std::str::from_utf8(&bytes[start..i]).unwrap_or("");
    let number_start = i;
    if i < bytes.len() && bytes[i] == b'-' {
        i += 1;
    }
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    let parameter = (i > number_start)
        .then(|| {
            std::str::from_utf8(&bytes[number_start..i])
                .ok()?
                .parse()
                .ok()
        })
        .flatten();
    if i < bytes.len() && bytes[i] == b' ' {
        i += 1;
    }
    (i, Control::Word(word, parameter))
}

fn skip_fallback(bytes: &[u8], i: usize) -> usize {
    if bytes.get(i) == Some(&b'\\') {
        parse_control(bytes, i + 1).0
    } else {
        std::str::from_utf8(&bytes[i..])
            .ok()
            .and_then(|tail| tail.chars().next())
            .map_or(i + 1, |ch| i + ch.len_utf8())
    }
}

fn is_destination(word: &str) -> bool {
    matches!(
        word,
        "fonttbl"
            | "colortbl"
            | "stylesheet"
            | "info"
            | "pict"
            | "object"
            | "annotation"
            | "header"
            | "footer"
            | "fldinst"
            | "datastore"
            | "themedata"
    )
}
fn is_known_control(word: &str) -> bool {
    matches!(
        word,
        "rtf"
            | "ansi"
            | "ansicpg"
            | "mac"
            | "deff"
            | "par"
            | "line"
            | "tab"
            | "u"
            | "uc"
            | "emdash"
            | "endash"
            | "bullet"
    )
}
fn normalize(text: &str) -> String {
    text.lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::{project_rtf, MAX_GROUP_DEPTH};
    use crate::model::ProjectionDiagnostic;

    #[test]
    fn malformed_non_ascii_controls_keep_utf8_boundaries() {
        for symbol in ["é", "😀", "中"] {
            let result = project_rtf(&format!("{{\\rtf1 before \\{symbol} after}}"));
            assert_eq!(result.text, "before after");
            assert!(result
                .diagnostics
                .contains(&ProjectionDiagnostic::MalformedMarkup));
        }
    }

    #[test]
    fn unicode_fallback_destinations_and_escapes() {
        let p = project_rtf(
            r"{\rtf1\ansi{\fonttbl{\f0 ignored;}}Hello \u8212? world\par escaped \{brace\} \'e9{\pict deadbeef}}",
        );
        assert_eq!(p.text, "Hello — world\nescaped {brace} é");
    }

    #[test]
    fn ignorable_destination_is_skipped() {
        assert_eq!(
            project_rtf(r"{\rtf1 before {\*\unknown hidden} after}").text,
            "before after"
        );
    }

    #[test]
    fn unicode_surrogate_pair_round_trips_non_bmp_text() {
        let projected = project_rtf(r"{\rtf1\ansi Smile: \u-10179?\u-8704?}");

        assert_eq!(projected.text, "Smile: 😀");
        assert!(projected.diagnostics.is_empty());
    }

    #[test]
    fn malformed_unicode_surrogates_degrade_with_diagnostics() {
        let lone_high = project_rtf(r"{\rtf1\ansi \u-10179? text}");
        let lone_low = project_rtf(r"{\rtf1\ansi \u-8704?}");
        let mismatched = project_rtf(r"{\rtf1\ansi \u-10179?\u65?}");

        assert_eq!(lone_high.text, "� text");
        assert_eq!(lone_low.text, "�");
        assert_eq!(mismatched.text, "�A");
        for projected in [lone_high, lone_low, mismatched] {
            assert!(projected
                .diagnostics
                .contains(&ProjectionDiagnostic::MalformedMarkup));
        }
    }

    #[test]
    fn literal_utf8_text_round_trips() {
        let projected = project_rtf(r"{\rtf1\ansi café 中文 😀}");

        assert_eq!(projected.text, "café 中文 😀");
        assert!(projected.diagnostics.is_empty());
    }

    #[test]
    fn literal_unicode_controls_and_hex_escapes_coexist() {
        let projected = project_rtf(r"{\rtf1\ansi café 中文 \u8212? emoji \u-10179?\u-8704? \'e9}");

        assert_eq!(projected.text, "café 中文 — emoji 😀 é");
        assert!(projected.diagnostics.is_empty());
    }

    #[test]
    fn windows_1252_hex_escapes_decode_typographic_characters() {
        let projected = project_rtf(r"{\rtf1\ansi\ansicpg1252 \'93\'94\'80}");

        assert_eq!(projected.text, "“”€");
        assert!(projected.diagnostics.is_empty());
    }

    #[test]
    fn ansi_defaults_to_windows_1252() {
        let projected = project_rtf(r"{\rtf1\ansi \'93\'94\'80}");

        assert_eq!(projected.text, "“”€");
        assert!(projected.diagnostics.is_empty());
    }

    #[test]
    fn unsupported_code_page_replaces_hex_bytes_with_diagnostic() {
        let projected = project_rtf(r"{\rtf1\ansi\ansicpg932 \'93}");

        assert_eq!(projected.text, "�");
        assert!(projected
            .diagnostics
            .contains(&ProjectionDiagnostic::UnsupportedCharset));
    }

    #[test]
    fn depth_overflow_inside_skipped_destination_does_not_leak() {
        let nested = "{".repeat(MAX_GROUP_DEPTH + 8);
        let closes = "}".repeat(MAX_GROUP_DEPTH + 8);
        let rtf = format!(r"{{\rtf1{{\pict {nested}hidden{closes}LEAK}}visible}}");

        let projected = project_rtf(&rtf);

        assert_eq!(projected.text, "visible");
        assert!(projected
            .diagnostics
            .contains(&ProjectionDiagnostic::InputTruncated));
    }

    #[test]
    fn balanced_depth_overflow_does_not_corrupt_sibling_state() {
        let nested = "{".repeat(MAX_GROUP_DEPTH + 8);
        let closes = "}".repeat(MAX_GROUP_DEPTH + 8);
        let rtf = format!(r"{{\rtf1 {nested}\uc0 ignored{closes}\u65?}}");

        let projected = project_rtf(&rtf);

        assert_eq!(projected.text, "A");
        assert!(projected
            .diagnostics
            .contains(&ProjectionDiagnostic::InputTruncated));
    }
}
