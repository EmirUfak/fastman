//! Response body tokenization, search highlights, and virtual line rendering.

use gpui_kit::component::{ActiveTheme as _, hsl};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::{ops::Range, sync::Arc};

const MAX_WRAP_LINES: usize = 400;
const MAX_DISPLAY_LINE_BYTES: usize = 4096;
const MAX_DISPLAY_LINES: usize = 50_000;
pub const MAX_PRETTY_JSON_SOURCE_BYTES: usize = 512 * 1024;
pub const MAX_HIGHLIGHTED_JSON_LINES: usize = 20_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JsonTokenKind {
    Key,
    String,
    Number,
    BoolNull,
    Punctuation,
    Plain,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TokenSpan<'a> {
    text: &'a str,
    kind: JsonTokenKind,
}

#[derive(Debug, Clone)]
pub struct HighlightedLine {
    text: SharedString,
    tokens: Vec<(Range<usize>, JsonTokenKind)>,
}

#[derive(Clone)]
pub enum CodeLines {
    Json(Arc<Vec<HighlightedLine>>),
    Plain(Arc<Vec<SharedString>>),
}

impl CodeLines {
    pub fn len(&self) -> usize {
        match self {
            Self::Json(lines) => lines.len(),
            Self::Plain(lines) => lines.len(),
        }
    }

    fn line_and_tokens(&self, idx: usize) -> (&SharedString, &[(Range<usize>, JsonTokenKind)]) {
        match self {
            Self::Json(items) => (&items[idx].text, &items[idx].tokens),
            Self::Plain(items) => (&items[idx], &[]),
        }
    }
}

/// Tokenize a single line of JSON into syntax spans.
fn tokenize_json_line(line: &str) -> Vec<TokenSpan<'_>> {
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

/// Precompute syntax token byte ranges for each line of JSON once per response.
pub fn precompute_json_lines(text: &str) -> Vec<HighlightedLine> {
    if text.is_empty() {
        return vec![HighlightedLine {
            text: "".into(),
            tokens: Vec::new(),
        }];
    }
    text.lines()
        .take(MAX_HIGHLIGHTED_JSON_LINES)
        .map(|line| {
            let mut tokens = Vec::new();
            let mut offset = 0usize;
            for span in tokenize_json_line(line) {
                let end = offset + span.text.len();
                if span.kind != JsonTokenKind::Plain && offset < end {
                    tokens.push((offset..end, span.kind));
                }
                offset = end;
            }
            HighlightedLine {
                text: line.to_owned().into(),
                tokens,
            }
        })
        .collect()
}

/// Split plain text into `SharedString` lines once per response, chunking
/// abnormally long lines so UI text layout stays bounded.
pub fn precompute_plain_lines(text: &str) -> Vec<SharedString> {
    if text.is_empty() {
        return vec!["".into()];
    }
    let mut out = Vec::new();
    for line in text.lines() {
        if line.len() <= MAX_DISPLAY_LINE_BYTES {
            out.push(line.to_owned().into());
            if out.len() >= MAX_DISPLAY_LINES {
                break;
            }
            continue;
        }
        let mut start = 0;
        while start < line.len() {
            if out.len() >= MAX_DISPLAY_LINES {
                return out;
            }
            let mut end = (start + MAX_DISPLAY_LINE_BYTES).min(line.len());
            while end > start && !line.is_char_boundary(end) {
                end -= 1;
            }
            if end == start {
                end = line.len();
            }
            out.push(line[start..end].to_owned().into());
            start = end;
        }
    }
    if out.is_empty() {
        out.push("".into());
    }
    out
}

/// Count case-insensitive occurrences of `needle` in `haystack`.
pub fn count_search_matches(haystack: &str, needle: &str) -> usize {
    let needle_trimmed = needle.trim();
    if needle_trimmed.is_empty() {
        return 0;
    }
    if needle_trimmed.is_ascii() {
        let hay = haystack.as_bytes();
        let ned = needle_trimmed.as_bytes();
        if hay.len() < ned.len() {
            return 0;
        }
        let mut count = 0;
        let mut i = 0;
        while i + ned.len() <= hay.len() {
            if hay[i..i + ned.len()].eq_ignore_ascii_case(ned) {
                count += 1;
                i += ned.len();
            } else {
                i += 1;
            }
        }
        return count;
    }
    let ned_lower = needle_trimmed.to_ascii_lowercase();
    haystack
        .lines()
        .map(|line| line.to_ascii_lowercase().match_indices(&ned_lower).count())
        .sum()
}

fn token_color(kind: JsonTokenKind, is_dark: bool, muted_fg: Hsla, foreground: Hsla) -> Hsla {
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
        JsonTokenKind::Punctuation => muted_fg,
        JsonTokenKind::Plain => foreground,
    }
}

/// Pure helper producing strictly ascending, non-overlapping UTF-8 char-boundary segments
/// combining syntax tokens and case-insensitive search matches.
fn build_line_segments(
    line: &str,
    tokens: &[(Range<usize>, JsonTokenKind)],
    search_lower: &str,
) -> Vec<(Range<usize>, Option<JsonTokenKind>, bool)> {
    if line.is_empty() {
        return Vec::new();
    }
    let matches: Vec<Range<usize>> = if search_lower.is_empty() {
        Vec::new()
    } else {
        let lower = line.to_ascii_lowercase();
        if lower.len() == line.len() {
            lower
                .match_indices(search_lower)
                .filter_map(|(start, matched)| {
                    let end = start + matched.len();
                    (start < end
                        && end <= line.len()
                        && line.is_char_boundary(start)
                        && line.is_char_boundary(end))
                    .then_some(start..end)
                })
                .collect()
        } else {
            Vec::new()
        }
    };

    if matches.is_empty() {
        return tokens
            .iter()
            .filter(|(r, _)| {
                r.start < r.end
                    && r.end <= line.len()
                    && line.is_char_boundary(r.start)
                    && line.is_char_boundary(r.end)
            })
            .map(|(r, kind)| (r.clone(), Some(*kind), false))
            .collect();
    }

    let mut boundaries = Vec::with_capacity(tokens.len() * 2 + matches.len() * 2 + 2);
    boundaries.push(0);
    boundaries.push(line.len());
    for (r, _) in tokens {
        if r.start < r.end && r.end <= line.len() {
            boundaries.push(r.start);
            boundaries.push(r.end);
        }
    }
    for r in &matches {
        boundaries.push(r.start);
        boundaries.push(r.end);
    }
    boundaries.sort_unstable();
    boundaries.dedup();

    let mut out: Vec<(Range<usize>, Option<JsonTokenKind>, bool)> = Vec::new();
    let mut tok_idx = 0usize;
    let mut match_idx = 0usize;

    for pair in boundaries.windows(2) {
        let a = pair[0];
        let b = pair[1];
        if a >= b || !line.is_char_boundary(a) || !line.is_char_boundary(b) {
            continue;
        }
        while tok_idx < tokens.len() && tokens[tok_idx].0.end <= a {
            tok_idx += 1;
        }
        while match_idx < matches.len() && matches[match_idx].end <= a {
            match_idx += 1;
        }
        let kind = tokens
            .get(tok_idx)
            .and_then(|(r, k)| (r.start <= a && b <= r.end).then_some(*k));
        let is_match = matches
            .get(match_idx)
            .is_some_and(|r| r.start <= a && b <= r.end);

        if kind.is_none() && !is_match {
            continue;
        }
        if let Some(last) = out.last_mut()
            && last.0.end == a
            && last.1 == kind
            && last.2 == is_match
        {
            last.0.end = b;
        } else {
            out.push((a..b, kind, is_match));
        }
    }

    out
}

fn build_line_highlights(
    line: &str,
    tokens: &[(Range<usize>, JsonTokenKind)],
    search_lower: &str,
    is_dark: bool,
    muted_fg: Hsla,
    foreground: Hsla,
    highlight_bg: Hsla,
) -> Vec<(Range<usize>, HighlightStyle)> {
    build_line_segments(line, tokens, search_lower)
        .into_iter()
        .map(|(range, kind, is_match)| {
            let mut style = HighlightStyle::default();
            if let Some(kind) = kind {
                style.color = Some(token_color(kind, is_dark, muted_fg, foreground));
            }
            if is_match {
                style.background_color = Some(highlight_bg);
                style.font_weight = Some(FontWeight::SEMIBOLD);
            }
            (range, style)
        })
        .collect()
}

/// Render pre-tokenized response lines via virtualized `uniform_list` (or wrapped `StyledText` for small payloads).
pub fn render_virtual_code_lines(
    lines: CodeLines,
    wrap: bool,
    search_lower: &str,
    scroll_handle: &UniformListScrollHandle,
    cx: &App,
) -> AnyElement {
    let is_dark = cx.theme().is_dark();
    let mono = cx.theme().mono_font_family.clone();
    let foreground = cx.theme().foreground;
    let muted_fg = cx.theme().muted_foreground;
    let gutter_fg = muted_fg.opacity(0.65);
    let border = cx.theme().border.opacity(0.6);
    let row_hover = cx.theme().muted.opacity(0.35);
    let highlight_bg = if is_dark {
        hsla(42. / 360., 0.90, 0.50, 0.34)
    } else {
        hsla(45. / 360., 0.96, 0.52, 0.42)
    };
    let line_count = lines.len();

    if wrap && line_count <= MAX_WRAP_LINES {
        let mut rows = Vec::with_capacity(line_count);
        for idx in 0..line_count {
            let (text, tokens) = lines.line_and_tokens(idx);
            let runs = build_line_highlights(
                text,
                tokens,
                search_lower,
                is_dark,
                muted_fg,
                foreground,
                highlight_bg,
            );
            rows.push(render_code_row(
                idx,
                text.clone(),
                runs,
                true,
                gutter_fg,
                border,
                row_hover,
            ));
        }
        return div()
            .id("response-body-wrap-scroll")
            .size_full()
            .overflow_y_scroll()
            .font_family(mono)
            .text_size(px(12.))
            .line_height(px(20.))
            .text_color(foreground)
            .child(div().w_full().py_1p5().flex().flex_col().children(rows))
            .into_any_element();
    }

    let search_owned = search_lower.to_owned();
    let mono_list = mono.clone();
    uniform_list(
        "response-body-list",
        line_count,
        move |range, _, _| -> Vec<AnyElement> {
            let mut items = Vec::with_capacity(range.len());
            for idx in range {
                let (text, tokens) = lines.line_and_tokens(idx);
                let runs = build_line_highlights(
                    text,
                    tokens,
                    &search_owned,
                    is_dark,
                    muted_fg,
                    foreground,
                    highlight_bg,
                );
                items.push(render_code_row(
                    idx,
                    text.clone(),
                    runs,
                    false,
                    gutter_fg,
                    border,
                    row_hover,
                ));
            }
            items
        },
    )
    .size_full()
    .py_1p5()
    .font_family(mono_list)
    .text_size(px(12.))
    .line_height(px(20.))
    .text_color(foreground)
    .track_scroll(scroll_handle)
    .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
    .into_any_element()
}

fn render_code_row(
    idx: usize,
    text: SharedString,
    runs: Vec<(Range<usize>, HighlightStyle)>,
    wrap: bool,
    gutter_fg: Hsla,
    border: Hsla,
    row_hover: Hsla,
) -> AnyElement {
    let content: AnyElement = if text.is_empty() {
        " ".into_any_element()
    } else if runs.is_empty() {
        text.into_any_element()
    } else {
        StyledText::new(text)
            .with_highlights(runs)
            .into_any_element()
    };

    div()
        .w_full()
        .when(!wrap, |d| d.h(px(20.)))
        .when(wrap, |d| d.min_h(px(20.)).flex_shrink_0())
        .flex()
        .items_start()
        .hover(move |s| s.bg(row_hover))
        .when(cfg!(test), |d| {
            d.debug_selector(move || format!("code-row-{idx}"))
        })
        .child(
            div()
                .w(px(46.))
                .flex_shrink_0()
                .pr_2p5()
                .text_right()
                .text_color(gutter_fg)
                .text_size(px(11.))
                .line_height(px(20.))
                .child((idx + 1).to_string()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .border_l_1()
                .border_color(border)
                .px_3()
                .when(!wrap, |d| d.whitespace_nowrap())
                .child(content),
        )
        .into_any_element()
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

    #[::core::prelude::v1::test]
    fn precomputes_lines_and_keeps_highlights_ordered_and_char_safe() {
        let json = "{\n  \"greeting\": \"Merhaba dünya\",\n  \"count\": 42\n}";
        let lines = precompute_json_lines(json);
        assert_eq!(lines.len(), 4);

        let segs = build_line_segments(&lines[1].text, &lines[1].tokens, "dün");
        let mut prev_end = 0;
        let mut saw_match = false;
        for (range, _, matched) in &segs {
            assert!(range.start >= prev_end);
            assert!(range.start < range.end);
            assert!(lines[1].text.is_char_boundary(range.start));
            assert!(lines[1].text.is_char_boundary(range.end));
            prev_end = range.end;
            if *matched {
                saw_match = true;
            }
        }
        assert!(saw_match);
    }

    #[::core::prelude::v1::test]
    fn chunks_oversized_single_plain_line_without_splitting_utf8() {
        let single_line = "ğ".repeat(5000);
        let lines = precompute_plain_lines(&single_line);
        assert!(lines.len() >= 2);
        let rejoined: String = lines.iter().map(|s| s.as_ref()).collect();
        assert_eq!(rejoined, single_line);
    }

    #[::core::prelude::v1::test]
    fn wrapped_lines_do_not_overlap_when_overflowing_viewport() {
        let result = std::thread::Builder::new()
            .stack_size(16 * 1024 * 1024)
            .spawn(|| {
                let mut cx = gpui::TestAppContext::single();
                cx.update(gpui_kit::init);

                struct WrapProbe {
                    lines: CodeLines,
                    scroll: UniformListScrollHandle,
                }

                impl gpui::Render for WrapProbe {
                    fn render(
                        &mut self,
                        _window: &mut gpui::Window,
                        cx: &mut gpui::Context<Self>,
                    ) -> impl gpui::IntoElement {
                        div().size_full().flex().flex_col().child(
                            div().flex_1().min_h_0().overflow_hidden().child(
                                render_virtual_code_lines(
                                    self.lines.clone(),
                                    true,
                                    "",
                                    &self.scroll,
                                    cx,
                                ),
                            ),
                        )
                    }
                }

                let mut raw_lines: Vec<SharedString> = Vec::new();
                raw_lines.push("long-wrapped-line ".repeat(18).into());
                for i in 1..90 {
                    raw_lines.push(format!("line {i} short text").into());
                }
                let lines = CodeLines::Plain(Arc::new(raw_lines));

                let (_, vcx) = cx.add_window_view(move |_, _| WrapProbe {
                    lines,
                    scroll: UniformListScrollHandle::new(),
                });
                vcx.simulate_resize(gpui::size(px(360.), px(220.)));
                vcx.update(|window, cx| {
                    window.draw(cx).clear(cx);
                });

                let row0 = vcx.debug_bounds("code-row-0").expect("row 0 bounds");
                let row1 = vcx.debug_bounds("code-row-1").expect("row 1 bounds");
                assert!(
                    row0.size.height > px(20.),
                    "wrapped row 0 should grow taller than 20px, got {:?}",
                    row0.size.height
                );
                assert!(
                    row1.top() >= row0.bottom(),
                    "row 1 ({:?}) overlapped row 0 ({:?})",
                    row1,
                    row0
                );
            })
            .unwrap()
            .join();
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }
}
