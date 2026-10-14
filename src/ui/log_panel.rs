//! HEX and text log side by side, each with clear/export (mirrors the original LogWidget).

use super::state::{AppState, Dir, HexLine, LogStore, hex_string};
use super::widgets::{notify_error, notify_info};
use gpui_kit::component::button::Button;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::*;

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Hex,
    Text,
}

pub struct LogPanel {
    app: Entity<AppState>,
    hex_scroll: UniformListScrollHandle,
    text_scroll: UniformListScrollHandle,
    _subs: Vec<Subscription>,
}

impl LogPanel {
    pub fn new(app: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let log = app.read(cx).log.clone();
        let subs = vec![
            cx.observe(&log, |this: &mut Self, log, cx| {
                let l = log.read(cx);
                this.hex_scroll.scroll_to_item(l.hex.len().saturating_sub(1), ScrollStrategy::Bottom);
                this.text_scroll.scroll_to_item(l.text.len().saturating_sub(1), ScrollStrategy::Bottom);
                cx.notify();
            }),
            cx.observe(&app, |_, _, cx| cx.notify()),
        ];
        Self {
            app,
            hex_scroll: UniformListScrollHandle::new(),
            text_scroll: UniformListScrollHandle::new(),
            _subs: subs,
        }
    }

    fn export(&self, kind: Kind, window: &mut Window, cx: &mut Context<Self>) {
        let log = self.app.read(cx).log.read(cx);
        let body: String = match kind {
            Kind::Hex => log.hex.iter().map(|l| format!("{}\n", hex_line(l))).collect(),
            Kind::Text => log.text.iter().map(|l| format!("{l}\n")).collect(),
        };
        let lang = self.app.read(cx).lang;
        let name = if kind == Kind::Hex { "hex_log.txt" } else { "text_log.txt" };
        let dir = dirs::document_dir().unwrap_or_else(std::env::temp_dir);
        let rx = cx.prompt_for_new_path(&dir, Some(name));
        cx.spawn_in(window, async move |_, cx| {
            let Ok(Ok(Some(path))) = rx.await else { return };
            let res = std::fs::write(&path, body);
            let _ = cx.update(|window, cx| match res {
                Ok(()) => notify_info(
                    format!("{}: {}", crate::i18n::tr(lang, "msg_save_success"), path.display()),
                    window,
                    cx,
                ),
                Err(e) => notify_error(format!("{}: {e}", crate::i18n::tr(lang, "msg_error")), window, cx),
            });
        })
        .detach();
    }

    fn column(&self, kind: Kind, cx: &mut Context<Self>) -> impl IntoElement {
        let app = self.app.read(cx);
        let (title, count, handle) = match kind {
            Kind::Hex => (app.t("grp_hex_log"), app.log.read(cx).hex.len(), self.hex_scroll.clone()),
            Kind::Text => (app.t("grp_txt_log"), app.log.read(cx).text.len(), self.text_scroll.clone()),
        };
        let log = app.log.clone();
        let id = if kind == Kind::Hex { "hex" } else { "text" };
        let fg = cx.theme().foreground;
        let (tx_c, rx_c, bad_c) = (rgb(0x64B5F6), rgb(0x81C784), rgb(0xE57373));
        let list = uniform_list(SharedString::from(format!("log-{id}")), count, move |range, _, cx| {
            let l = log.read(cx);
            range
                .map(|i| {
                    let (txt, color) = match kind {
                        Kind::Hex => {
                            let h = &l.hex[i];
                            let c = match h.dir {
                                Dir::Tx => tx_c.into(),
                                Dir::Rx => rx_c.into(),
                                Dir::RxBad => bad_c.into(),
                            };
                            (hex_line(h), c)
                        }
                        Kind::Text => (l.text[i].clone(), fg),
                    };
                    div().text_color(color).whitespace_nowrap().child(txt)
                })
                .collect()
        })
        .track_scroll(&handle)
        .flex_1()
        .font_family("Consolas")
        .text_xs();

        v_flex()
            .flex_1()
            .h_full()
            .min_w_0()
            .gap_1()
            .child(
                h_flex()
                    .gap_2()
                    .child(div().text_color(rgb(0xFA8F01)).font_weight(FontWeight::BOLD).child(title))
                    .child(div().flex_1())
                    .child(Button::new(SharedString::from(format!("clear-{id}"))).small().label(app.t("btn_clear_log")).on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.app.read(cx).log.clone().update(cx, |l: &mut LogStore, cx| match kind {
                                Kind::Hex => l.clear_hex(cx),
                                Kind::Text => l.clear_text(cx),
                            })
                        }),
                    ))
                    .child(
                        Button::new(SharedString::from(format!("export-{id}")))
                            .small()
                            .label(app.t("btn_export_log"))
                            .on_click(cx.listener(move |this, _, window, cx| this.export(kind, window, cx))),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .p_1()
                    .rounded_md()
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().secondary)
                    .child(list),
            )
    }
}

fn hex_line(l: &HexLine) -> String {
    let tag = match l.dir {
        Dir::Tx => "TX",
        Dir::Rx => "RX",
        Dir::RxBad => "RX bad checksum",
    };
    format!("{} [{tag}] {}", l.time, hex_string(&l.bytes))
}

impl Render for LogPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .size_full()
            .gap_4()
            .p_1()
            .child(self.column(Kind::Hex, cx))
            .child(self.column(Kind::Text, cx))
    }
}
