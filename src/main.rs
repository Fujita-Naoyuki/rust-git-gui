#![windows_subsystem = "windows"]

mod app;
mod config;
mod git;
mod graph;
mod model;
mod views;

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::*;

use crate::app::GitApp;

fn main() {
    gpui_kit::application().run(|cx| {
        gpui_kit::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);

        let bounds = Bounds::centered(None, size(px(1600.), px(1000.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(1100.), px(600.))),
            titlebar: Some(TitlebarOptions {
                title: Some("RustGitGUI".into()),
                ..Default::default()
            }),
            app_id: Some("rust-git-gui".into()),
            ..Default::default()
        };
        gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| GitApp::new(window, cx))
        })
        .expect("failed to open window");
        cx.activate(true);
    });
}
