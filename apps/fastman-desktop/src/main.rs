mod explorer;
mod kv_editor;
mod request_editor;
mod response_body_view;
mod response_view;
mod theme;
mod workspace;

use gpui_kit::*;
use workspace::WorkspaceView;

gpui_kit::actions!(
    fastman,
    [
        SendRequest,
        SaveRequest,
        NewRequest,
        CloseActiveTab,
        FocusUrl,
        FocusSearch,
        ToggleTheme,
        OpenVariables,
        OpenImport,
    ]
);

fn bind_shortcuts(cx: &mut App) {
    #[cfg(target_os = "macos")]
    let bindings = [
        KeyBinding::new("cmd-enter", SendRequest, None),
        KeyBinding::new("cmd-s", SaveRequest, None),
        KeyBinding::new("cmd-n", NewRequest, None),
        KeyBinding::new("cmd-w", CloseActiveTab, None),
        KeyBinding::new("cmd-l", FocusUrl, None),
        KeyBinding::new("cmd-f", FocusSearch, None),
        KeyBinding::new("cmd-shift-t", ToggleTheme, None),
    ];
    #[cfg(not(target_os = "macos"))]
    let bindings = [
        KeyBinding::new("ctrl-enter", SendRequest, None),
        KeyBinding::new("ctrl-s", SaveRequest, None),
        KeyBinding::new("ctrl-n", NewRequest, None),
        KeyBinding::new("ctrl-w", CloseActiveTab, None),
        KeyBinding::new("ctrl-l", FocusUrl, None),
        KeyBinding::new("ctrl-f", FocusSearch, None),
        KeyBinding::new("ctrl-shift-t", ToggleTheme, None),
    ];
    cx.bind_keys(bindings);
}

fn main() {
    application().with_assets(assets::AllAssets).run(|cx| {
        init(cx);
        bind_shortcuts(cx);

        gpui_kit::component::Theme::change(gpui_kit::component::ThemeMode::Dark, None, cx);
        theme::apply_crisp_motion(cx);

        let window_bounds = cx.primary_display().map(|display| {
            let visible = display.visible_bounds();
            let width = (visible.size.width * 0.84)
                .max(px(1080.))
                .min(visible.size.width);
            let height = (visible.size.height * 0.84)
                .max(px(720.))
                .min(visible.size.height);
            let center = visible.center();
            WindowBounds::Windowed(Bounds::new(
                point(center.x - width / 2., center.y - height / 2.),
                size(width, height),
            ))
        });

        open_window(
            WindowOptions {
                window_bounds,
                titlebar: Some(TitlebarOptions {
                    title: Some("Fastman".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            cx,
            |window, cx| {
                cx.new(|cx| WorkspaceView::new(window, cx).expect("Initialize Fastman workspace"))
            },
        )
        .expect("Open Fastman window");
    });
}
