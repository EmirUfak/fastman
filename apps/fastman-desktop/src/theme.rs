use gpui_kit::component::{ActiveTheme as _, Icon, hsl};
use gpui_kit::*;

/// Load a bundled Lucide SVG icon from `gpui-kit-assets`.
pub fn svg_icon(path: &'static str) -> Icon {
    Icon::default().path(path)
}

/// Signature Fastman coral-orange accent used sparingly for brand identity and active document emphasis.
pub fn brand_accent(is_dark: bool) -> Hsla {
    if is_dark {
        hsl(16., 92., 62.)
    } else {
        hsl(16., 86., 48.)
    }
}

/// Semantic `(background, foreground, border)` colors for an HTTP method.
pub fn method_colors(method: &str, is_dark: bool) -> (Hsla, Hsla, Hsla) {
    let hue = match method.to_ascii_uppercase().as_str() {
        "GET" => 152.,
        "POST" => 38.,
        "PUT" => 212.,
        "PATCH" => 268.,
        "DELETE" => 2.,
        _ => 215.,
    };
    let is_neutral = !matches!(
        method.to_ascii_uppercase().as_str(),
        "GET" | "POST" | "PUT" | "PATCH" | "DELETE"
    );
    if is_neutral {
        if is_dark {
            (hsl(hue, 12., 18.), hsl(hue, 14., 72.), hsl(hue, 12., 28.))
        } else {
            (hsl(hue, 16., 94.), hsl(hue, 16., 36.), hsl(hue, 16., 84.))
        }
    } else if is_dark {
        (
            hsla(hue / 360., 0.65, 0.45, 0.16),
            hsl(hue, 82., 68.),
            hsla(hue / 360., 0.65, 0.55, 0.32),
        )
    } else {
        (
            hsla(hue / 360., 0.80, 0.48, 0.11),
            hsl(hue, 82., 34.),
            hsla(hue / 360., 0.75, 0.45, 0.28),
        )
    }
}

/// Pill badge for HTTP methods in the explorer, URL method selector, and dropdown menus.
pub fn method_badge(method: &str, fixed_width: Option<Pixels>, cx: &App) -> AnyElement {
    let is_dark = cx.theme().is_dark();
    let (bg, fg, border) = method_colors(method, is_dark);
    let label = method.to_ascii_uppercase();
    let mut el = div()
        .flex()
        .items_center()
        .justify_center()
        .flex_shrink_0()
        .h(px(19.))
        .px(px(6.))
        .rounded(px(4.))
        .border_1()
        .border_color(border)
        .bg(bg)
        .text_color(fg)
        .font_family(cx.theme().mono_font_family.clone())
        .text_size(px(10.))
        .font_weight(FontWeight::SEMIBOLD)
        .line_height(px(12.));
    if let Some(width) = fixed_width {
        el = el.w(width);
    }
    el.child(label).into_any_element()
}

/// Compact colored method token used inside open document tabs.
pub fn method_mini_label(method: &str, cx: &App) -> AnyElement {
    let (_, fg, _) = method_colors(method, cx.theme().is_dark());
    let upper = method.to_ascii_uppercase();
    let short = match upper.as_str() {
        "DELETE" => "DEL",
        "OPTIONS" => "OPT",
        "PATCH" => "PTCH",
        other => other,
    };
    div()
        .flex_shrink_0()
        .font_family(cx.theme().mono_font_family.clone())
        .text_size(px(10.))
        .font_weight(FontWeight::BOLD)
        .text_color(fg)
        .child(short.to_owned())
        .into_any_element()
}

/// Semantic `(background, foreground, border)` colors for an HTTP status code.
pub fn status_colors(status: u16, is_dark: bool) -> (Hsla, Hsla, Hsla) {
    let hue = match status {
        200..=299 => 152.,
        300..=399 => 212.,
        400..=499 => 38.,
        _ => 2.,
    };
    if is_dark {
        (
            hsla(hue / 360., 0.65, 0.45, 0.18),
            hsl(hue, 84., 68.),
            hsla(hue / 360., 0.65, 0.55, 0.35),
        )
    } else {
        (
            hsla(hue / 360., 0.80, 0.48, 0.12),
            hsl(hue, 84., 32.),
            hsla(hue / 360., 0.75, 0.45, 0.30),
        )
    }
}

/// Colored status code badge (`200 OK`, `404 Not Found`, etc.).
pub fn status_badge(status: u16, reason: &str, cx: &App) -> AnyElement {
    let (bg, fg, border) = status_colors(status, cx.theme().is_dark());
    let label = if reason.is_empty() {
        status.to_string()
    } else {
        format!("{status} {reason}")
    };
    div()
        .flex()
        .items_center()
        .flex_shrink_0()
        .h(px(22.))
        .px(px(8.))
        .rounded(px(5.))
        .border_1()
        .border_color(border)
        .bg(bg)
        .text_color(fg)
        .font_family(cx.theme().mono_font_family.clone())
        .text_size(px(11.5))
        .font_weight(FontWeight::SEMIBOLD)
        .child(label)
        .into_any_element()
}

/// Compact pill counter for section tabs and header buttons.
pub fn count_pill(count: usize, cx: &App) -> AnyElement {
    div()
        .ml_1()
        .px(px(5.))
        .py(px(1.))
        .rounded_full()
        .border_1()
        .border_color(cx.theme().border.opacity(0.8))
        .bg(cx.theme().secondary)
        .text_color(cx.theme().muted_foreground)
        .font_family(cx.theme().mono_font_family.clone())
        .text_size(px(10.))
        .font_weight(FontWeight::MEDIUM)
        .line_height(px(12.))
        .child(count.to_string())
        .into_any_element()
}

/// Small indicator dot showing active configuration or unsaved changes.
pub fn dot_indicator(color: Hsla) -> AnyElement {
    div()
        .w(px(6.))
        .h(px(6.))
        .flex_shrink_0()
        .rounded_full()
        .bg(color)
        .into_any_element()
}

/// Apply crisp, responsive animation timing tokens across interactive controls.
pub fn apply_crisp_motion(cx: &mut App) {
    gpui_kit::component::Theme::update(cx, |theme| {
        theme.motion.duration_fast = std::time::Duration::from_millis(90);
        theme.motion.duration_normal = std::time::Duration::from_millis(140);
        theme.motion.duration_slow = std::time::Duration::from_millis(200);
        theme.motion.spring_control =
            gpui_kit::base::Spring::new(std::time::Duration::from_millis(140));
        theme.motion.spring_move =
            gpui_kit::base::Spring::new(std::time::Duration::from_millis(180))
                .with_damping(0.90)
                .with_epsilon(0.1);
    });
}
