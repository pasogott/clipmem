use std::io::Write;

/// Escape terminal controls while retaining only the SGR styles emitted by our renderer.
pub(super) fn sanitize(text: &str, preserve_styles: bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    while cursor < text.len() {
        let rest = &text[cursor..];
        if preserve_styles && rest.starts_with("\x1b[") {
            if let Some(end) = rest
                .as_bytes()
                .iter()
                .take(16)
                .position(|byte| *byte == b'm')
            {
                let codes = &rest[2..end];
                if codes
                    .split(';')
                    .all(|code| matches!(code, "0" | "1" | "31" | "32" | "33" | "36" | "96"))
                {
                    out.push_str(&rest[..=end]);
                    cursor += end + 1;
                    continue;
                }
            }
        }
        let ch = rest
            .chars()
            .next()
            .expect("cursor should remain at a UTF-8 boundary within text");
        if (ch.is_control() && !matches!(ch, '\n' | '\t'))
            || matches!(ch, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        {
            out.extend(ch.escape_default());
        } else {
            out.push(ch);
        }
        cursor += ch.len_utf8();
    }
    out
}

pub(super) fn write_display(text: &str, preserve_styles: bool) -> anyhow::Result<()> {
    std::io::stdout()
        .lock()
        .write_all(sanitize(text, preserve_styles).as_bytes())?;
    Ok(())
}

pub(super) fn write_raw(text: &str) -> anyhow::Result<()> {
    std::io::stdout().lock().write_all(text.as_bytes())?;
    Ok(())
}

pub(super) fn is_broken_pipe(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|e| e.kind() == std::io::ErrorKind::BrokenPipe)
            || cause
                .downcast_ref::<serde_json::Error>()
                .is_some_and(|e| e.io_error_kind() == Some(std::io::ErrorKind::BrokenPipe))
    })
}

pub(super) fn write_error(text: &str) {
    let _ = std::io::stderr()
        .lock()
        .write_all(sanitize(text, false).as_bytes());
}

#[cfg(test)]
mod tests {
    use super::sanitize;
    #[test]
    fn clipboard_controls_cannot_clear_terminal_or_set_clipboard() {
        let payload = "safe\x1b[2J\x1b]52;c;c2VjcmV0\x07\r\u{009b}2J\u{202e}hidden";
        for styles in [false, true] {
            let output = sanitize(payload, styles);
            assert!(!output.contains('\x1b'));
            assert!(!output.contains('\r'));
            assert!(!output.contains('\u{009b}'));
            assert!(!output.contains('\u{202e}'));
            assert!(output.contains("safe"));
        }
        assert_eq!(
            sanitize("\x1b[1;32mTitle\x1b[0m", true),
            "\x1b[1;32mTitle\x1b[0m"
        );
        assert!(!sanitize("\x1b[8mconcealed", true).contains('\x1b'));
    }
}
