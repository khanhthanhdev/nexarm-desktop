//! Guide: in-app documentation. A topic list on the left, and for the chosen topic what the page is,
//! when to use it, how to use it and what to watch for. The text lives in `assets/i18n/guide.json`.

use crate::i18n::{Lang, tr, tr_list};
use crate::ui::state::AppState;
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

const ACCENT: u32 = 0xFA8F01;

/// Guide topics in reading order: the topic id (the `guide_<id>_*` string keys) and the key of its title.
pub const TOPICS: [(&str, &str); 16] = [
    ("start", "guide_start_title"),
    ("servo", "tab_servo"),
    ("coord", "tab_coord"),
    ("peripheral", "tab_peripheral"),
    ("system", "tab_system"),
    ("ai", "tab_ai"),
    ("servo_adv", "tab_servo_adv"),
    ("teach", "tab_teach"),
    ("sync", "tab_sync"),
    ("calib", "tab_calib"),
    ("session", "tab_lab_session"),
    ("record", "tab_lab_record"),
    ("datasets", "tab_lab_datasets"),
    ("train", "tab_lab_train"),
    ("deploy", "tab_lab_deploy"),
    ("remote", "tab_lab_remote"),
];

pub struct GuidePage {
    app: Entity<AppState>,
    selected: usize,
    _subs: Vec<Subscription>,
}

impl GuidePage {
    pub fn new(app: Entity<AppState>, _: &mut Window, cx: &mut Context<Self>) -> Self {
        let subs = vec![cx.observe(&app, |_, _, cx| cx.notify())];
        Self { app, selected: 0, _subs: subs }
    }

    fn topic_list(&self, lang: Lang, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let items = TOPICS.iter().enumerate().map(|(i, (_, title))| {
            let active = i == self.selected;
            div()
                .id(("guide-topic", i))
                .px_3()
                .py_1()
                .rounded_md()
                .text_sm()
                .cursor_pointer()
                .when(active, |d| d.bg(rgb(ACCENT)).text_color(white()))
                .when(!active, |d| d.hover(|d| d.bg(theme.secondary_hover)))
                .child(tr(lang, title))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.selected = i;
                    cx.notify();
                }))
        });
        v_flex()
            .id("guide-topics")
            .w(px(230.))
            .h_full()
            .flex_none()
            .overflow_y_scroll()
            .border_r_1()
            .border_color(theme.border)
            .p_3()
            .gap_0p5()
            .child(
                div()
                    .px_2()
                    .pb_1()
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.muted_foreground)
                    .child(tr(lang, "guide_topics").to_uppercase()),
            )
            .children(items)
    }
}

fn heading(text: String, cx: &App) -> Div {
    div().text_sm().font_weight(FontWeight::BOLD).text_color(cx.theme().muted_foreground).child(text.to_uppercase())
}

/// A numbered list for `how`, bullets for `tips`.
fn list(items: Vec<String>, numbered: bool) -> Div {
    v_flex().gap_1p5().children(items.into_iter().enumerate().map(|(i, text)| {
        let marker = if numbered { format!("{}.", i + 1) } else { "•".to_string() };
        h_flex()
            .gap_2()
            .items_start()
            .child(div().w(px(22.)).flex_none().text_color(rgb(ACCENT)).font_weight(FontWeight::BOLD).child(marker))
            .child(div().flex_1().min_w_0().child(text))
    }))
}

impl Render for GuidePage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let lang = self.app.read(cx).lang;
        let (id, title_key) = TOPICS[self.selected];
        let key = |part: &str| format!("guide_{id}_{part}");

        let article = v_flex()
            .id("guide-article")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .p_5()
            .gap_4()
            .child(div().text_xl().font_weight(FontWeight::BOLD).child(tr(lang, title_key)))
            .child(v_flex().gap_1().child(heading(tr(lang, "guide_what"), cx)).child(tr(lang, &key("what"))))
            .child(v_flex().gap_1().child(heading(tr(lang, "guide_when"), cx)).child(tr(lang, &key("when"))))
            .child(v_flex().gap_1().child(heading(tr(lang, "guide_how"), cx)).child(list(tr_list(lang, &key("how")), true)))
            .child(v_flex().gap_1().child(heading(tr(lang, "guide_tips"), cx)).child(list(tr_list(lang, &key("tips")), false)));

        h_flex().size_full().child(self.topic_list(lang, cx)).child(article)
    }
}

#[cfg(test)]
mod tests {
    use super::TOPICS;
    use crate::i18n::{Lang, tr, tr_list};

    #[test]
    fn every_topic_is_written_in_both_languages() {
        for lang in [Lang::En, Lang::Vi] {
            for (id, title) in TOPICS {
                assert_ne!(tr(lang, title), title, "{id}: no title in {lang:?}");
                for part in ["what", "when"] {
                    let key = format!("guide_{id}_{part}");
                    assert_ne!(tr(lang, &key), key, "{key} missing in {lang:?}");
                }
                for part in ["how", "tips"] {
                    let key = format!("guide_{id}_{part}");
                    assert!(!tr_list(lang, &key).is_empty(), "{key} missing in {lang:?}");
                }
            }
        }
    }
}
