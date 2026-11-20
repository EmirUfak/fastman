mod workspace;

use gpui_kit::*;
use workspace::WorkspaceView;

fn main() {
    application().with_assets(assets::Assets).run(|cx| {
        init(cx);

        gpui_kit::component::Theme::change(gpui_kit::component::ThemeMode::Dark, None, cx);

        let window_bounds = cx.primary_display().map(|display| {
            let visible = display.visible_bounds();
            let size = size(visible.size.width * 0.8, visible.size.height * 0.8);
            let center = visible.center();
            WindowBounds::Windowed(Bounds::new(
                point(center.x - size.width / 2., center.y - size.height / 2.),
                size,
            ))
        });

        open_window(
            WindowOptions {
                window_bounds,
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
