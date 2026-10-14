#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::*;
use nexarm_desktop::i18n::{Lang, tr};
use nexarm_desktop::settings::Settings;
use nexarm_desktop::ui::shell::Shell;

fn main() {
    gpui_kit::application().with_assets(gpui_kit::assets::Assets).run(|cx| {
        gpui_kit::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);
        let title = tr(Lang::from_code(&Settings::load().lang), "window_title");
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(1360.), px(900.)), cx))),
            titlebar: Some(TitlebarOptions { title: Some(title.into()), ..Default::default() }),
            window_min_size: Some(size(px(800.), px(600.))),
            ..Default::default()
        };
        gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| Shell::new(window, cx)))
            .expect("failed to open window");
        cx.activate(true);
    });
}
