use gpui_kit::component::{ActiveTheme as _, hsl};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

const MAX_HIGHLIGHT_LINES: usize = 2500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonTokenKind {
    Key,
    String,
    Number,
    BoolNull,
    Punctuation,
    Plain,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenSpan<'a> {
    pub text: &'a str,
    pub kind: JsonTokenKind,
}

/// Tokenize a single line of JSON into syntax spans.
pub fn tokenize_json_line(line: &str) -> Vec<TokenSpan<'_>> {
    let mut spans = Vec::new();
    let bytes = line.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        let b = bytes[i];
        if b.is_ascii_whitespace() {
            let start = i;
            while i < len && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            spans.push(TokenSpan {
                text: &line[start..i],
                kind: JsonTokenKind::Plain,
            });
            continue;
        }

        if b == b'"' {
            let start = i;
            i += 1;
            let mut escaped = false;
            while i < len {
                let ch = bytes[i];
                if escaped {
                    escaped = false;
                    i += 1;
                } else if ch == b'\\' {
                    escaped = true;
                    i += 1;
                } else if ch == b'"' {
                    i += 1;
                    break;
                } else {
                    i += 1;
                }
            }
            // Look ahead past spaces for ':' to distinguish object keys from string values.
            let mut look = i;
            while look < len && bytes[look].is_ascii_whitespace() {
                look += 1;
            }
            let kind = if look < len && bytes[look] == b':' {
                JsonTokenKind::Key
            } else {
                JsonTokenKind::String
            };
            spans.push(TokenSpan {
                text: &line[start..i],
                kind,
            });
            continue;
        }

        if matches!(b, b'{' | b'}' | b'[' | b']' | b':' | b',') {
            spans.push(TokenSpan {
                text: &line[i..i + 1],
                kind: JsonTokenKind::Punctuation,
            });
            i += 1;
            continue;
        }

        if b == b'-' || b.is_ascii_digit() {
            let start = i;
            if b == b'-' {
                i += 1;
            }
            while i < len
                && (bytes[i].is_ascii_digit()
                    || matches!(bytes[i], b'.' | b'e' | b'E' | b'+' | b'-'))
            {
                i += 1;
            }
            spans.push(TokenSpan {
                text: &line[start..i],
                kind: JsonTokenKind::Number,
            });
            continue;
        }

        let rest = &line[i..];
        let keyword = if rest.starts_with("true") {
            Some(4)
        } else if rest.starts_with("false") {
            Some(5)
        } else if rest.starts_with("null") {
            Some(4)
        } else {
            None
        };
        if let Some(kw_len) = keyword {
            let after = i + kw_len;
            if after == len || !bytes[after].is_ascii_alphanumeric() {
                spans.push(TokenSpan {
                    text: &line[i..after],
                    kind: JsonTokenKind::BoolNull,
                });
                i = after;
                continue;
            }
        }

        let start = i;
        while i < len {
            let next = bytes[i];
            if next.is_ascii_whitespace()
                || matches!(next, b'"' | b'{' | b'}' | b'[' | b']' | b':' | b',')
            {
                break;
            }
            i += 1;
        }
        spans.push(TokenSpan {
            text: &line[start..i],
            kind: JsonTokenKind::Plain,
        });
    }

    spans
}

/// Count case-insensitive occurrences of `needle` in `haystack`.
pub fn count_search_matches(haystack: &str, needle: &str) -> usize {
    let needle_trimmed = needle.trim();
    if needle_trimmed.is_empty() {
        return 0;
    }
    let hay_lower = haystack.to_ascii_lowercase();
    let ned_lower = needle_trimmed.to_ascii_lowercase();
    hay_lower.match_indices(&ned_lower).count()
}

fn token_color(kind: JsonTokenKind, is_dark: bool, cx: &App) -> Hsla {
    match kind {
        JsonTokenKind::Key => {
            if is_dark {
                hsl(205., 88., 72.)
            } else {
                hsl(214., 80., 38.)
            }
        }
        JsonTokenKind::String => {
            if is_dark {
                hsl(142., 62., 66.)
            } else {
                hsl(146., 72., 28.)
            }
        }
        JsonTokenKind::Number => {
            if is_dark {
                hsl(34., 90., 68.)
            } else {
                hsl(24., 86., 40.)
            }
        }
        JsonTokenKind::BoolNull => {
            if is_dark {
                hsl(276., 78., 74.)
            } else {
                hsl(272., 68., 44.)
            }
        }
        JsonTokenKind::Punctuation => cx.theme().muted_foreground,
        JsonTokenKind::Plain => cx.theme().foreground,
    }
}

/// Render a monospace code viewer with line numbers, optional JSON syntax highlighting,
/// optional line wrapping, and case-insensitive search highlighting.
pub fn render_code_viewer(
    text: &str,
    is_json: bool,
    wrap: bool,
    search: &str,
    cx: &App,
) -> (AnyElement, usize) {
    let is_dark = cx.theme().is_dark();
    let mono = cx.theme().mono_font_family.clone();
    let gutter_fg = cx.theme().muted_foreground.opacity(0.65);
    let border = cx.theme().border;
    let search_trimmed = search.trim();
    let total_matches = count_search_matches(text, search_trimmed);
    let search_lower = search_trimmed.to_ascii_lowercase();
    let highlight_bg = if is_dark {
        hsla(42. / 360., 0.90, 0.50, 0.34)
    } else {
        hsla(45. / 360., 0.96, 0.52, 0.42)
    };

    let lines: Vec<&str> = if text.is_empty() {
        vec![""]
    } else {
        text.lines().collect()
    };

    if lines.len() > MAX_HIGHLIGHT_LINES {
        let preview_lines = lines.len();
        let el = div()
            .w_full()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .child(
                div()
                    .px_2p5()
                    .py_1p5()
                    .rounded(cx.theme().radius)
                    .bg(cx.theme().muted)
                    .text_size(px(11.5))
                    .text_color(cx.theme().muted_foreground)
                    .child(format!(
                        "Large payload ({preview_lines} lines) — showing fast plain monospace view."
                    )),
            )
            .child(
                div()
                    .w_full()
                    .font_family(mono)
                    .text_size(px(12.))
                    .line_height(px(19.))
                    .text_color(cx.theme().foreground)
                    .when(!wrap, |d| d.whitespace_nowrap())
                    .child(text.to_owned()),
            )
            .into_any_element();
        return (el, total_matches);
    }

    let mut rows = Vec::with_capacity(lines.len());
    for (idx, line) in lines.iter().enumerate() {
        let spans = if is_json {
            tokenize_json_line(line)
        } else {
            vec![TokenSpan {
                text: line,
                kind: JsonTokenKind::Plain,
            }]
        };

        let mut content = div()
            .flex_1()
            .min_w_0()
            .px_3()
            .flex()
            .flex_row()
            .when(wrap, |d| d.flex_wrap())
            .when(!wrap, |d| d.whitespace_nowrap());

        if spans.is_empty() || (spans.len() == 1 && spans[0].text.is_empty()) {
            content = content.child(" ");
        } else {
            for span in spans {
                let color = token_color(span.kind, is_dark, cx);
                if search_lower.is_empty() {
                    content = content.child(
                        div()
                            .text_color(color)
                            .when(!wrap, |d| d.whitespace_nowrap())
                            .child(span.text.to_owned()),
                    );
                } else {
                    content = render_highlighted_span(
                        content,
                        span.text,
                        &search_lower,
                        color,
                        highlight_bg,
                        wrap,
                    );
                }
            }
        }

        rows.push(
            div()
                .w_full()
                .min_h(px(20.))
                .flex()
                .items_start()
                .hover(|s| s.bg(cx.theme().muted.opacity(0.35)))
                .child(
                    div()
                        .w(px(46.))
                        .flex_shrink_0()
                        .pr_2p5()
                        .text_right()
                        .border_r_1()
                        .border_color(border.opacity(0.6))
                        .text_color(gutter_fg)
                        .text_size(px(11.))
                        .line_height(px(20.))
                        .child((idx + 1).to_string()),
                )
                .child(content)
                .into_any_element(),
        );
    }

    let el = div()
        .w_full()
        .py_1p5()
        .font_family(mono)
        .text_size(px(12.))
        .line_height(px(20.))
        .flex()
        .flex_col()
        .children(rows)
        .into_any_element();

    (el, total_matches)
}

fn render_highlighted_span(
    mut container: Div,
    text: &str,
    search_lower: &str,
    fg: Hsla,
    highlight_bg: Hsla,
    wrap: bool,
) -> Div {
    let lower = text.to_ascii_lowercase();
    if lower.len() != text.len() || !lower.contains(search_lower) {
        return container.child(
            div()
                .text_color(fg)
                .when(!wrap, |d| d.whitespace_nowrap())
                .child(text.to_owned()),
        );
    }

    let mut cursor = 0;
    while cursor < text.len() {
        if let Some(rel) = lower[cursor..].find(search_lower) {
            let start = cursor + rel;
            let end = start + search_lower.len();
            if start > cursor {
                container = container.child(
                    div()
                        .text_color(fg)
                        .when(!wrap, |d| d.whitespace_nowrap())
                        .child(text[cursor..start].to_owned()),
                );
            }
            container = container.child(
                div()
                    .bg(highlight_bg)
                    .rounded(px(2.))
                    .text_color(fg)
                    .font_weight(FontWeight::SEMIBOLD)
                    .when(!wrap, |d| d.whitespace_nowrap())
                    .child(text[start..end].to_owned()),
            );
            cursor = end;
        } else {
            container = container.child(
                div()
                    .text_color(fg)
                    .when(!wrap, |d| d.whitespace_nowrap())
                    .child(text[cursor..].to_owned()),
            );
            break;
        }
    }
    container
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn tokenizes_json_keys_values_numbers_and_booleans() {
        let line = r#"  "count": 42, "ok": true, "msg": "hi""#;
        let tokens = tokenize_json_line(line);
        assert!(
            tokens
                .iter()
                .any(|t| t.text == "\"count\"" && t.kind == JsonTokenKind::Key)
        );
        assert!(
            tokens
                .iter()
                .any(|t| t.text == "42" && t.kind == JsonTokenKind::Number)
        );
        assert!(
            tokens
                .iter()
                .any(|t| t.text == "true" && t.kind == JsonTokenKind::BoolNull)
        );
        assert!(
            tokens
                .iter()
                .any(|t| t.text == "\"hi\"" && t.kind == JsonTokenKind::String)
        );
    }

    #[::core::prelude::v1::test]
    fn counts_case_insensitive_search_matches() {
        let json = "{\n  \"status\": \"OK\",\n  \"message\": \"ok now\"\n}";
        assert_eq!(count_search_matches(json, "ok"), 2);
        assert_eq!(count_search_matches(json, "  "), 0);
    }
}
