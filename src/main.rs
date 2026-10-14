#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use gpui_kit::*;
use nexarm_desktop::i18n::{Lang, tr};
use nexarm_desktop::settings::Settings;
use nexarm_desktop::ui::shell::Shell;
use nexarm_desktop::ui::state::apply_theme;

fn main() {
    gpui_kit::application().with_assets(gpui_kit::assets::Assets).run(|cx| {
        gpui_kit::init(cx);
        let settings = Settings::load();
        apply_theme(&settings.theme, cx);
        let title = tr(Lang::from_code(&settings.lang), "window_title");
        // Preferred size, shrunk to fit small or highly scaled displays.
        let screen = cx.primary_display().map(|d| d.bounds().size);
        let fit = |want: Pixels, have: Option<Pixels>| have.map_or(want, |h| want.min(h * 0.92));
        let window_size = size(fit(px(1360.), screen.map(|s| s.width)), fit(px(900.), screen.map(|s| s.height)));
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, window_size, cx))),
            titlebar: Some(TitlebarOptions { title: Some(title.into()), ..Default::default() }),
            window_min_size: Some(size(px(800.), px(600.))),
            ..Default::default()
        };
        gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| Shell::new(window, cx)))
            .expect("failed to open window");
        cx.activate(true);
    });
}
