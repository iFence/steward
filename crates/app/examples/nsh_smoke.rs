//! NSH integration smoke test.
//!
//! This example is intentionally separate from the shipped Steward binary
//! while the shell is being migrated. It proves that the pinned GPUI Fast,
//! nsh-platform-gpui, and NSH engine versions compose on Windows.

#[cfg(target_os = "windows")]
fn main() {
    use gpui::{size, AppContext, Bounds, WindowBounds, WindowOptions};
    use gpui_platform::application;
    use nsh_core::ui::prelude::*;
    use nsh_platform_gpui::{App, AppOptions, AppView, Root};

    #[component]
    fn Smoke() -> impl IntoElement {
        view().size_full().p_6().bg(rgb(0x1b1c1f)).child(
            text("Steward · NSH smoke")
                .text_size(px(24.0))
                .text_color(rgb(0xffffff)),
        )
    }

    application().run(|cx| {
        nsh_platform_gpui::set_ui_sets(vec![nsh_base::registry()]);
        let bounds = Bounds::centered(None, size(gpui::px(520.0), gpui::px(280.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |window, cx| {
                cx.new(|cx| {
                    let view = AppView::new(AppOptions::default(), window, cx);
                    view.load(App::Rust(Root::of::<Smoke>()));
                    view
                })
            },
        )
        .expect("open NSH smoke window");
    });
}

#[cfg(not(target_os = "windows"))]
fn main() {}
