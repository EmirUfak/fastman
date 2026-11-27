use fastman_core::{AssertionRule, CaptureRule, CaptureSource};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::*;

#[derive(Clone, Copy)]
pub(super) enum Kind {
    Status,
    Header,
    Json,
    Latency,
    CaptureStatus,
    CaptureHeader,
    CaptureJson,
}

impl Kind {
    pub(super) const OUTPUTS: [Self; 3] =
        [Self::CaptureStatus, Self::CaptureHeader, Self::CaptureJson];

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Status | Self::CaptureStatus => "HTTP status",
            Self::Header | Self::CaptureHeader => "Response header",
            Self::Json | Self::CaptureJson => "JSON field",
            Self::Latency => "Response time",
        }
    }

    pub(super) fn short_tag(self) -> &'static str {
        match self {
            Self::Status | Self::CaptureStatus => "Status",
            Self::Header | Self::CaptureHeader => "Header",
            Self::Json | Self::CaptureJson => "JSON",
            Self::Latency => "Speed",
        }
    }

    fn fields(self) -> (&'static str, Option<&'static str>) {
        match self {
            Self::Status => ("200", None),
            Self::Header => ("Header name", Some("Expected value")),
            Self::Json => ("/path/to/field", Some("Expected JSON")),
            Self::Latency => ("1000", None),
            Self::CaptureStatus => ("output_name", None),
            Self::CaptureHeader => ("output_name", Some("Header-Name")),
            Self::CaptureJson => ("output_name", Some("/path/to/field")),
        }
    }
}

pub(super) struct Rule {
    pub(super) kind: Kind,
    primary: Entity<InputState>,
    secondary: Option<Entity<InputState>>,
    pub(super) required: bool,
    pub(super) sensitive: bool,
}

pub(super) fn input(
    label: &str,
    value: String,
    window: &mut Window,
    cx: &mut App,
) -> Entity<InputState> {
    cx.new(|cx| {
        let mut input = InputState::new(window, cx).placeholder(label.to_owned());
        input.set_value(value, window, cx);
        input
    })
}

impl Rule {
    fn new(
        kind: Kind,
        primary: String,
        secondary: String,
        window: &mut Window,
        cx: &mut App,
    ) -> Self {
        let (label, other) = kind.fields();
        Self {
            kind,
            primary: input(label, primary, window, cx),
            secondary: other.map(|label| input(label, secondary, window, cx)),
            required: true,
            sensitive: true,
        }
    }

    pub(super) fn empty(kind: Kind, window: &mut Window, cx: &mut App) -> Self {
        let primary = match kind {
            Kind::Status => "200",
            Kind::Latency => "1000",
            _ => "",
        };
        Self::new(kind, primary.into(), String::new(), window, cx)
    }

    pub(super) fn from_assertion(rule: AssertionRule, window: &mut Window, cx: &mut App) -> Self {
        let (kind, primary, secondary) = match rule {
            AssertionRule::Status { expected } => {
                (Kind::Status, expected.to_string(), String::new())
            }
            AssertionRule::Header { name, expected } => (Kind::Header, name, expected),
            AssertionRule::JsonField { pointer, expected } => {
                (Kind::Json, pointer, expected.to_string())
            }
            AssertionRule::Latency { max_ms } => (Kind::Latency, max_ms.to_string(), String::new()),
        };
        Self::new(kind, primary, secondary, window, cx)
    }

    pub(super) fn from_capture(rule: CaptureRule, window: &mut Window, cx: &mut App) -> Self {
        let (kind, secondary) = match rule.source {
            CaptureSource::Status => (Kind::CaptureStatus, String::new()),
            CaptureSource::Header { name } => (Kind::CaptureHeader, name),
            CaptureSource::JsonField { pointer } | CaptureSource::LinkedJsonField { pointer } => {
                (Kind::CaptureJson, pointer)
            }
        };
        let mut form = Self::new(kind, rule.name, secondary, window, cx);
        form.required = rule.required;
        form.sensitive = rule.sensitive;
        form
    }

    fn values(&self, cx: &App) -> (String, String) {
        (
            self.primary.read(cx).value().to_string(),
            self.secondary
                .as_ref()
                .map_or_else(String::new, |field| field.read(cx).value().to_string()),
        )
    }

    pub(super) fn assertion(&self, cx: &App) -> Result<AssertionRule, &'static str> {
        let (primary, secondary) = self.values(cx);
        Ok(match self.kind {
            Kind::Status => AssertionRule::Status {
                expected: primary
                    .trim()
                    .parse::<u16>()
                    .ok()
                    .filter(|code| (100..=999).contains(code))
                    .ok_or("Enter an HTTP status from 100 to 999.")?,
            },
            Kind::Header => AssertionRule::Header {
                name: header_name(primary)?,
                expected: secondary,
            },
            Kind::Json => AssertionRule::JsonField {
                pointer: json_pointer(primary)?,
                expected: serde_json::from_str(&secondary)
                    .map_err(|_| "Enter a JSON value, with quotes for text.")?,
            },
            Kind::Latency => AssertionRule::Latency {
                max_ms: primary
                    .trim()
                    .parse()
                    .map_err(|_| "Enter a whole number of milliseconds.")?,
            },
            _ => unreachable!("output form in assertions"),
        })
    }

    pub(super) fn output_name(&self, cx: &App) -> String {
        self.primary.read(cx).value().trim().to_owned()
    }

    pub(super) fn capture(&self, cx: &App) -> Result<CaptureRule, &'static str> {
        let name = self.output_name(cx);
        if name.is_empty() || name.len() > 128 || name.contains(['{', '}']) {
            return Err("Enter an output name up to 128 bytes, without braces.");
        }
        let (_, secondary) = self.values(cx);
        Ok(CaptureRule {
            name,
            source: match self.kind {
                Kind::CaptureStatus => CaptureSource::Status,
                Kind::CaptureHeader => CaptureSource::Header {
                    name: header_name(secondary)?,
                },
                Kind::CaptureJson => CaptureSource::JsonField {
                    pointer: json_pointer(secondary)?,
                },
                _ => unreachable!("assertion form in outputs"),
            },
            required: self.required,
            sensitive: self.sensitive,
        })
    }

    /// Compact inline fields for a single-row assertion inside the step inspector.
    pub(super) fn render_assertion_fields(&self, cx: &App) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        match self.kind {
            Kind::Status => div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap_1p5()
                .child(div().text_xs().text_color(muted).child("=="))
                .child(div().w(px(76.)).child(Input::new(&self.primary).xsmall()))
                .into_any_element(),
            Kind::Latency => div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap_1p5()
                .child(div().text_xs().text_color(muted).child("<="))
                .child(div().w(px(84.)).child(Input::new(&self.primary).xsmall()))
                .child(
                    div()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_xs()
                        .text_color(muted)
                        .child("ms"),
                )
                .into_any_element(),
            Kind::Header | Kind::Json => {
                let mut row = div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&self.primary).xsmall()),
                    );
                if let Some(secondary) = &self.secondary {
                    row = row
                        .child(div().text_xs().text_color(muted).child("="))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(Input::new(secondary).xsmall()),
                        );
                }
                row.into_any_element()
            }
            _ => unreachable!("output form in assertions"),
        }
    }

    /// Compact fields for a custom output capture inside the outputs column.
    pub(super) fn render_capture_fields(&self, cx: &App) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let mut row = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(Input::new(&self.primary).xsmall());
        if let Some(secondary) = &self.secondary {
            row = row.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(div().text_xs().text_color(muted).child("from"))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(secondary).xsmall()),
                    ),
            );
        }
        row.into_any_element()
    }
}

fn header_name(name: String) -> Result<String, &'static str> {
    let name = name.trim();
    if !fastman_core::security::is_valid_header_name(name) {
        return Err("Enter a valid HTTP header name.");
    }
    Ok(name.into())
}

fn json_pointer(pointer: String) -> Result<String, &'static str> {
    if !pointer.is_empty() && !pointer.starts_with('/') {
        return Err("A JSON pointer starts with /, or is empty for the whole value.");
    }
    Ok(pointer)
}
