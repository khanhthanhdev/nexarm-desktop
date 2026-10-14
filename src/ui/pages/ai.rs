//! AI play: visual offset calibration, tracking and grabbing modes.

use crate::protocol::{self, cmd};
use crate::ui::state::{AppState, DeviceEvent, LogStore};
use crate::ui::widgets::{self, NumState, section};
use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonVariants as _};
use gpui_kit::component::input::NumberInput;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde::{Deserialize, Serialize};

/// Start/stop mode rows: (label key, command). Start sends `[1]`, stop `[0]`.
const TRACK_MODES: [(&str, u8); 3] =
    [("lbl_face_track", cmd::FACE_TRACK), ("lbl_tag_track", cmd::APRILTAG_TRACK), ("lbl_gesture_track", cmd::GESTURE_TRACK)];
const GRAB_MODES: [(&str, u8); 3] =
    [("lbl_tag_grab", cmd::APRILTAG_GRAB), ("lbl_waste_grab", cmd::GARBAGE_GRAB), ("lbl_llm", cmd::LLM_CONTROL)];
/// Color buttons: (label key, firmware color id).
const COLORS: [(&str, u8); 3] = [("color_red", 1), ("color_green", 2), ("color_blue", 3)];
const ACCENT: u32 = 0xFA8F01;

/// Offsets last saved with "Save & Apply" (the original kept them in `offset_config.json`).
#[derive(Serialize, Deserialize)]
#[serde(default)]
struct OffsetConfig {
    x: f64,
    y: f64,
    z: f64,
}

impl Default for OffsetConfig {
    fn default() -> Self {
        Self { x: 15., y: 50., z: 60. }
    }
}

fn offset_config_path() -> std::path::PathBuf {
    crate::settings::config_dir().join("offset_config.json")
}

pub struct AiPage {
    app: Entity<AppState>,
    offsets: [NumState; 3],
    /// Commands whose "start" is currently lit.
    active_modes: Vec<u8>,
    color_track: Option<u8>,
    color_grab: Option<u8>,
    /// Calibration sub-command currently lit (1 align, 2 test grab).
    calib: Option<u8>,
    ex: Option<String>,
    ey: Option<String>,
    status_key: &'static str,
    /// Last firmware text entry already scanned for calibration feedback.
    last_text: Option<String>,
    _subs: Vec<Subscription>,
}

impl AiPage {
    pub fn new(app: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let cfg: OffsetConfig = std::fs::read_to_string(offset_config_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let offsets = [cfg.x, cfg.y, cfg.z].map(|v| widgets::num_input(v, -200., 200., 1., window, cx));
        let log = app.read(cx).log.clone();
        let last_text = log.read(cx).text.back().cloned();
        let subs = vec![
            cx.observe(&app, |_, _, cx| cx.notify()),
            cx.subscribe(&app, |this, _, ev: &DeviceEvent, cx| {
                // The original resets the status caption when the language changes.
                if matches!(ev, DeviceEvent::LangChanged) {
                    this.status_key = "lbl_waiting";
                    cx.notify();
                }
            }),
            cx.observe(&log, |this, log, cx| this.scan_log(log, cx)),
        ];
        Self {
            app,
            offsets,
            active_modes: Vec::new(),
            color_track: None,
            color_grab: None,
            calib: None,
            ex: None,
            ey: None,
            status_key: "lbl_waiting",
            last_text,
            _subs: subs,
        }
    }

    pub fn on_shown(&mut self, _: &mut Window, _: &mut Context<Self>) {}

    fn send(&self, command: u8, args: &[u8], cx: &mut Context<Self>) {
        let args = args.to_vec();
        self.app.update(cx, |a, cx| a.send_sys(command, args, cx));
    }

    fn set_mode(&mut self, command: u8, on: bool, cx: &mut Context<Self>) {
        self.send(command, &[on as u8], cx);
        self.active_modes.retain(|c| *c != command);
        if on {
            self.active_modes.push(command);
        }
        cx.notify();
    }

    fn calibrate(&mut self, sub: u8, cx: &mut Context<Self>) {
        self.send(cmd::CALIBRATION, &[sub], cx);
        self.calib = matches!(sub, 1 | 2).then_some(sub);
        cx.notify();
    }

    fn save_offsets(&mut self, cx: &mut Context<Self>) {
        self.calib = None;
        let [x, y, z] = [0, 1, 2].map(|i| widgets::read_num(&self.offsets[i], cx).clamp(-200., 200.));
        let req = protocol::apriltag_offset(x as f32, y as f32, z as f32);
        self.app.update(cx, |a, cx| a.send(req, cx));
        let saved = serde_json::to_string(&OffsetConfig { x, y, z })
            .map_err(anyhow::Error::from)
            .and_then(|s| Ok(std::fs::write(offset_config_path(), s)?));
        if let Err(e) = saved {
            self.app.update(cx, |a, cx| a.log_text(format!("Failed to save local config: {e}"), cx));
        }
        cx.notify();
    }

    /// Feed newly logged firmware text to the calibration parser.
    fn scan_log(&mut self, log: Entity<LogStore>, cx: &mut Context<Self>) {
        let text = &log.read(cx).text;
        let start = match &self.last_text {
            Some(last) => text.iter().rposition(|l| l == last).map_or(0, |i| i + 1),
            None => 0,
        };
        let fresh: Vec<String> = text.iter().skip(start).cloned().collect();
        self.last_text = text.back().cloned();
        let mut changed = false;
        for entry in &fresh {
            for line in entry.lines() {
                if let Some((ex, ey, aligned)) = parse_calibration(line.trim()) {
                    self.ex = Some(ex);
                    self.ey = Some(ey);
                    self.status_key = if aligned { "status_aligned" } else { "status_adjusting" };
                    changed = true;
                }
            }
        }
        if changed {
            cx.notify();
        }
    }

    fn lit(&self, b: Button, on: bool, cx: &App) -> Button {
        b.when(on, |b| {
            b.custom(ButtonCustomVariant::new(cx).color(rgb(ACCENT).into()).foreground(white()).hover(rgb(ACCENT).into()))
        })
    }

    fn mode_row(&self, key: &str, command: u8, cx: &mut Context<Self>) -> Div {
        let lang = self.app.read(cx).lang;
        let t = move |k: &str| crate::i18n::tr(lang, k);
        let on = self.active_modes.contains(&command);
        let start = Button::new(("mode-start", command as usize)).label(t("btn_start_play")).w_full();
        row(
            t(key),
            vec![
                self.lit(start, on, cx).on_click(cx.listener(move |this, _, _, cx| this.set_mode(command, true, cx))).into_any_element(),
                Button::new(("mode-stop", command as usize))
                    .label(t("btn_stop_play"))
                    .w_full()
                    .on_click(cx.listener(move |this, _, _, cx| this.set_mode(command, false, cx)))
                    .into_any_element(),
            ],
        )
    }

    /// Red/green/blue/stop row; `grab` selects color grab (46) instead of color tracking (40).
    fn color_row(&self, grab: bool, cx: &mut Context<Self>) -> Div {
        let lang = self.app.read(cx).lang;
        let t = move |k: &str| crate::i18n::tr(lang, k);
        let current = if grab { self.color_grab } else { self.color_track };
        let base = if grab { 100 } else { 0 };
        let mut buttons: Vec<AnyElement> = COLORS
            .iter()
            .map(|&(key, id)| {
                let b = Button::new(("color", base + id as usize)).label(t(key)).w_full();
                self.lit(b, current == Some(id), cx)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if grab {
                            this.send(cmd::COLOR_GRAB, &[1, id], cx);
                            this.color_grab = Some(id);
                        } else {
                            this.send(cmd::COLOR_TRACK, &[1, id], cx);
                            this.color_track = Some(id);
                        }
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect();
        buttons.push(
            Button::new(("color-stop", base))
                .label(t("btn_stop_play"))
                .w_full()
                .on_click(cx.listener(move |this, _, _, cx| {
                    // The original stops color grab with one byte but color tracking with two.
                    if grab {
                        this.send(cmd::COLOR_GRAB, &[0], cx);
                        this.color_grab = None;
                    } else {
                        this.send(cmd::COLOR_TRACK, &[0, 0], cx);
                        this.color_track = None;
                    }
                    cx.notify();
                }))
                .into_any_element(),
        );
        row(t(if grab { "lbl_color_grab" } else { "lbl_color_track" }), buttons)
    }

    fn render_calibration(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let lang = self.app.read(cx).lang;
        let t = move |k: &str| crate::i18n::tr(lang, k);
        let accent = |s: String| div().text_color(rgb(ACCENT)).font_weight(FontWeight::BOLD).child(s);
        let ex = format!("EX: {}", self.ex.as_deref().map_or("--".into(), |v| format!("{v} px")));
        let ey = format!("EY: {}", self.ey.as_deref().map_or("--".into(), |v| format!("{v} px")));
        let calib_btn = |id: u8, key: &str, cx: &mut Context<Self>| {
            let b = Button::new(("calib", id as usize)).label(t(key)).w_full();
            self.lit(b, self.calib == Some(id), cx)
                .on_click(cx.listener(move |this, _, _, cx| this.calibrate(id, cx)))
                .into_any_element()
        };
        let offset = |i: usize, name: &'static str, cx: &App| {
            h_flex()
                .gap_1()
                .items_center()
                .child(div().text_sm().text_color(cx.theme().muted_foreground).child(name))
                .child(div().flex_1().min_w_0().child(NumberInput::new(&self.offsets[i]).small()))
                .into_any_element()
        };
        section(t("grp_ai_calib")).child(
            v_flex()
                .gap_3()
                .child(row(
                    String::new(),
                    vec![
                        accent(ex).into_any_element(),
                        accent(ey).into_any_element(),
                        div().child(accent(t(self.status_key))).into_any_element(),
                    ],
                ))
                .child(row(
                    t("lbl_tag_calib"),
                    vec![
                        offset(0, "X:", cx),
                        offset(1, "Y:", cx),
                        offset(2, "Z:", cx),
                        Button::new("save-offsets")
                            .label(t("btn_save_apply"))
                            .w_full()
                            .on_click(cx.listener(|this, _, _, cx| this.save_offsets(cx)))
                            .into_any_element(),
                    ],
                ))
                .child(row(
                    String::new(),
                    vec![
                        calib_btn(1, "btn_calib_start", cx),
                        calib_btn(2, "btn_calib_grab", cx),
                        calib_btn(3, "btn_calib_reset", cx),
                        calib_btn(0, "btn_calib_stop", cx),
                    ],
                )),
        )
    }
}

/// Grid row like the original: label column (weight 3) then up to four equal cells.
fn row(label: String, cells: Vec<AnyElement>) -> Div {
    let n = cells.len();
    h_flex()
        .gap_2()
        .items_center()
        .child(div().flex_grow(3.).flex_basis(px(0.)).child(label))
        .children(cells.into_iter().map(|c| div().flex_grow(1.).flex_basis(px(0.)).min_w_0().child(c)))
        .children((n..4).map(|_| div().flex_grow(1.).flex_basis(px(0.))))
}

/// Firmware calibration feedback: `(ex, ey, aligned)`.
///
/// Accepts `[校准] ... X偏:<n> Y偏:<n>` (aligned) and
/// `[校准] <dir> <n> 像素 | <dir> <n> 像素` (still adjusting).
fn parse_calibration(line: &str) -> Option<(String, String, bool)> {
    if !line.contains("[校准]") {
        return None;
    }
    if let Some(i) = line.find("X偏:") {
        let rest = &line[i + "X偏:".len()..];
        let (ex, rest) = take_int(rest, true)?;
        let (ey, _) = take_int(rest.strip_prefix(" Y偏:")?, true)?;
        return Some((ex, ey, true));
    }
    let rest = &line[line.find("[校准] ")? + "[校准] ".len()..];
    let (left, right) = rest.split_once(" 像素 | ")?;
    let x = last_number(left)?;
    let y = last_number(&right[..right.find(" 像素")?])?;
    Some((x, y, false))
}

/// Leading (optionally signed) integer and the remaining text.
fn take_int(s: &str, signed: bool) -> Option<(String, &str)> {
    let sign = usize::from(signed && s.starts_with('-'));
    let digits = s[sign..].bytes().take_while(u8::is_ascii_digit).count();
    (digits > 0).then(|| (s[..sign + digits].to_string(), &s[sign + digits..]))
}

/// Unsigned number that ends `s` after a space (`<dir> <n>`).
fn last_number(s: &str) -> Option<String> {
    let (_, n) = s.rsplit_once(' ')?;
    (!n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())).then(|| n.to_string())
}

impl Render for AiPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let lang = self.app.read(cx).lang;
        let t = move |k: &str| crate::i18n::tr(lang, k);
        let (grp_tracking, grp_grabbing) = (t("grp_tracking"), t("grp_grabbing"));

        let tracking = v_flex()
            .gap_3()
            .child(self.mode_row(TRACK_MODES[0].0, TRACK_MODES[0].1, cx))
            .child(self.color_row(false, cx))
            .child(self.mode_row(TRACK_MODES[1].0, TRACK_MODES[1].1, cx))
            .child(self.mode_row(TRACK_MODES[2].0, TRACK_MODES[2].1, cx));
        let grabbing = v_flex()
            .gap_3()
            .child(self.mode_row(GRAB_MODES[0].0, GRAB_MODES[0].1, cx))
            .child(self.mode_row(GRAB_MODES[1].0, GRAB_MODES[1].1, cx))
            .child(self.color_row(true, cx))
            .child(self.mode_row(GRAB_MODES[2].0, GRAB_MODES[2].1, cx));

        v_flex()
            .id("ai-page")
            .size_full()
            .p_3()
            .gap_3()
            .overflow_y_scroll()
            .text_color(cx.theme().foreground)
            .child(self.render_calibration(cx))
            .child(section(grp_tracking).child(tracking))
            .child(section(grp_grabbing).child(grabbing))
    }
}

#[cfg(test)]
mod tests {
    use super::parse_calibration;

    #[test]
    fn parses_calibration_feedback() {
        assert_eq!(
            parse_calibration("[校准] 已对准 X偏:-3 Y偏:12"),
            Some(("-3".into(), "12".into(), true))
        );
        assert_eq!(
            parse_calibration("[校准] 向左 25 像素 | 向上 7 像素"),
            Some(("25".into(), "7".into(), false))
        );
        assert_eq!(parse_calibration("X偏:1 Y偏:2"), None);
    }
}
