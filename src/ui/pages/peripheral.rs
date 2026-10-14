//! Peripherals & chassis: conveyor, stepper rail, ESP-NOW, PS3 pad, mecanum/tank pads, DC motors.

use crate::i18n::tr;
use crate::protocol::{self, Request, Response, cmd};
use crate::ui::state::{AppState, DeviceEvent};
use crate::ui::widgets::{self, NumState, StrSelect, field, section};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::Dialog;
use gpui_kit::component::input::{Input, InputState, NumberInput};
use gpui_kit::component::select::Select;
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState, SliderValue};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::time::Duration;

const STEPPER_DIVS: [&str; 6] = ["1", "2", "4", "8", "16", "32"];
const FLASH: Duration = Duration::from_millis(1000);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Dir {
    Up,
    Down,
    Left,
    Right,
    LeftStrafe,
    RightStrafe,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PadKind {
    Mecanum,
    Tank,
}

/// Pressed directions of one pad; several may be held at once (mouse + keys).
struct Pad {
    active: Vec<Dir>,
    focus: FocusHandle,
}

impl Pad {
    fn new(cx: &mut App) -> Self {
        Self { active: Vec::new(), focus: cx.focus_handle() }
    }
}

pub struct PeripheralPage {
    app: Entity<AppState>,
    belt: NumState,
    steps: NumState,
    div: Entity<StrSelect>,
    mac: Entity<InputState>,
    channel: NumState,
    sync_acc: NumState,
    ps3_mac: Entity<InputState>,
    multi: Vec<NumState>,
    motors: Vec<Entity<SliderState>>,
    motor_vals: [i32; 4],
    mec: Pad,
    tank: Pad,
    /// Button currently showing the "OK" confirmation, and the task that clears it.
    flashed: Option<&'static str>,
    _flash_task: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl PeripheralPage {
    pub fn new(app: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let motors: Vec<_> = (0..4)
            .map(|_| cx.new(|_| SliderState::new().min(-100.).max(100.).step(1.).default_value(0.)))
            .collect();
        let mut subs = vec![
            cx.observe(&app, |_, _, cx| cx.notify()),
            cx.subscribe_in(&app, window, |this, _, ev: &DeviceEvent, window, cx| {
                if let DeviceEvent::Response(Response::ChannelScan(v)) = ev {
                    this.show_channel_scan(*v, window, cx);
                }
            }),
        ];
        for (i, m) in motors.iter().enumerate() {
            subs.push(cx.subscribe(m, move |this, _, ev: &SliderEvent, cx| {
                if let SliderEvent::Change(SliderValue::Single(v)) = ev {
                    this.on_motor_slide(i, v.round() as i32, cx);
                }
            }));
        }
        Self {
            belt: widgets::num_input(50., -100., 100., 1., window, cx),
            steps: widgets::num_input(2000., -100_000., 100_000., 1., window, cx),
            div: widgets::select(STEPPER_DIVS.iter().map(|s| (*s).into()).collect(), 2, window, cx),
            mac: widgets::text_input("A4:C1:38:12:34:56", "", window, cx),
            channel: widgets::num_input(1., 0., 14., 1., window, cx),
            sync_acc: widgets::num_input(0., 0., 254., 1., window, cx),
            ps3_mac: widgets::text_input("10:00:00:00:85:95", "", window, cx),
            multi: (0..4).map(|_| widgets::num_input(0., -100., 100., 1., window, cx)).collect(),
            motors,
            motor_vals: [0; 4],
            mec: Pad::new(cx),
            tank: Pad::new(cx),
            flashed: None,
            _flash_task: None,
            app,
            _subs: subs,
        }
    }

    pub fn on_shown(&mut self, _: &mut Window, _: &mut Context<Self>) {}

    fn send(&self, req: Request, cx: &mut Context<Self>) {
        self.app.update(cx, |a, cx| a.send(req, cx));
    }

    fn num(state: &NumState, lo: f64, hi: f64, cx: &App) -> i32 {
        widgets::read_num(state, cx).clamp(lo, hi) as i32
    }

    /// Briefly show "OK" on the clicked button, like the original's feedback.
    fn flash(&mut self, id: &'static str, cx: &mut Context<Self>) {
        self.flashed = Some(id);
        self._flash_task = Some(cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            cx.background_executor().timer(FLASH).await;
            let _ = this.update(cx, |p, cx| {
                p.flashed = None;
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Button that runs `action` then flashes its confirmation.
    fn button(
        &self,
        id: &'static str,
        key: &str,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> Button {
        let lang = self.app.read(cx).lang;
        let label = if self.flashed == Some(id) { tr(lang, "msg_set_success") } else { tr(lang, key) };
        Button::new(id).small().label(label).on_click(cx.listener(move |this, _, _, cx| {
            action(this, cx);
            this.flash(id, cx);
        }))
    }

    fn send_mac(&mut self, command: u8, input: &Entity<InputState>, what: &str, cx: &mut Context<Self>) {
        let text = widgets::read_text(input, cx);
        match parse_mac(&text) {
            Some(bytes) => self.send(Request::sys(command, bytes), cx),
            None => self.app.update(cx, |a, cx| a.log_text(format!("{what} Error: invalid MAC '{}'", text.trim()), cx)),
        }
    }

    fn on_motor_slide(&mut self, i: usize, v: i32, cx: &mut Context<Self>) {
        if self.motor_vals[i] == v {
            return;
        }
        self.motor_vals[i] = v;
        self.send(protocol::i8s(cmd::SET_SINGLE_MOTOR, &[i as i32 + 1, v]), cx);
        cx.notify();
    }

    // ---------------------------------------------------------------- pads

    fn pad(&mut self, kind: PadKind) -> &mut Pad {
        match kind {
            PadKind::Mecanum => &mut self.mec,
            PadKind::Tank => &mut self.tank,
        }
    }

    fn press(&mut self, kind: PadKind, dir: Dir, cx: &mut Context<Self>) {
        if self.pad(kind).active.contains(&dir) {
            return;
        }
        self.pad(kind).active.push(dir);
        match kind {
            PadKind::Mecanum => self.send_mecanum(cx),
            PadKind::Tank => {
                let (s, t) = match dir {
                    Dir::Up => (50, 0),
                    Dir::Down => (-50, 0),
                    Dir::Left => (0, 50),
                    Dir::Right => (0, -50),
                    _ => return cx.notify(),
                };
                self.send(protocol::i8s(cmd::TANK_CONTROL, &[s, t]), cx);
            }
        }
        cx.notify();
    }

    fn release(&mut self, kind: PadKind, dir: Dir, cx: &mut Context<Self>) {
        let pad = self.pad(kind);
        let Some(ix) = pad.active.iter().position(|d| *d == dir) else { return };
        pad.active.remove(ix);
        let any_left = !pad.active.is_empty();
        // Mecanum keeps driving with the remaining directions; tank always stops.
        if kind == PadKind::Mecanum && any_left {
            self.send_mecanum(cx);
        } else {
            self.stop_motors(cx);
        }
        cx.notify();
    }

    fn release_all(&mut self, kind: PadKind, cx: &mut Context<Self>) {
        for d in self.pad(kind).active.clone() {
            self.release(kind, d, cx);
        }
    }

    fn stop_motors(&self, cx: &mut Context<Self>) {
        self.send(Request::sys(cmd::STOP_ALL_MOTOR, []), cx);
    }

    /// Sum the vectors of every held direction, clamp each axis to ±100.
    fn send_mecanum(&self, cx: &mut Context<Self>) {
        let (mut vx, mut vy, mut vz) = (0, 0, 0);
        for d in &self.mec.active {
            let (dx, dy, dz) = match d {
                Dir::Up => (0, -50, 0),
                Dir::Down => (0, 50, 0),
                Dir::Left => (0, 0, 50),
                Dir::Right => (0, 0, -50),
                Dir::LeftStrafe => (-50, 0, 0),
                Dir::RightStrafe => (50, 0, 0),
            };
            vx += dx;
            vy += dy;
            vz += dz;
        }
        let c = |v: i32| v.clamp(-100, 100);
        self.send(protocol::i8s(cmd::MECANUM_CONTROL, &[c(vx), c(vy), c(vz)]), cx);
    }

    fn key_dir(kind: PadKind, key: &str) -> Option<Dir> {
        match (kind, key) {
            (PadKind::Mecanum, "w") | (PadKind::Tank, "up") => Some(Dir::Up),
            (PadKind::Mecanum, "s") | (PadKind::Tank, "down") => Some(Dir::Down),
            (PadKind::Mecanum, "a") | (PadKind::Tank, "left") => Some(Dir::Left),
            (PadKind::Mecanum, "d") | (PadKind::Tank, "right") => Some(Dir::Right),
            (PadKind::Mecanum, "q") => Some(Dir::LeftStrafe),
            (PadKind::Mecanum, "e") => Some(Dir::RightStrafe),
            _ => None,
        }
    }

    fn render_pad(&self, kind: PadKind, cx: &mut Context<Self>) -> impl IntoElement {
        let lang = self.app.read(cx).lang;
        let t = |k: &str| tr(lang, k);
        let pad = match kind {
            PadKind::Mecanum => &self.mec,
            PadKind::Tank => &self.tank,
        };
        let theme = cx.theme();
        let (bg, border, fg) = (theme.secondary, theme.border, theme.foreground);
        let size = px(76.);
        let mec = kind == PadKind::Mecanum;
        let dir_btn = |dir: Dir, key: &str, name: String| {
            let active = pad.active.contains(&dir);
            div()
                .id(SharedString::from(format!("pad-{}-{dir:?}", mec as u8)))
                .size(size)
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .rounded_md()
                .border_1()
                .border_color(border)
                .bg(if active { rgb(0xFA8F01).into() } else { bg })
                .when(active, |d| d.text_color(white()))
                .cursor_pointer()
                .font_weight(FontWeight::BOLD)
                .child(key.to_string())
                .child(div().text_xs().child(name))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        window.focus(&this.pad(kind).focus, cx);
                        this.press(kind, dir, cx);
                    }),
                )
        };
        let (k_up, k_down, k_left, k_right) = if mec { ("W", "S", "A", "D") } else { ("↑", "↓", "←", "→") };
        let cell = || div().size(size);
        let stop = div()
            .id(SharedString::from(format!("pad-{}-stop", mec as u8)))
            .size(size)
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .border_2()
            .border_color(fg)
            .cursor_pointer()
            .font_weight(FontWeight::BOLD)
            .child(t("lbl_stop"))
            .on_click(cx.listener(|this, _, _, cx| this.stop_motors(cx)));

        let cross = v_flex()
            .gap_1()
            .child(h_flex().gap_1().child(cell()).child(dir_btn(Dir::Up, k_up, t("lbl_fwd"))).child(cell()))
            .child(
                h_flex()
                    .gap_1()
                    .child(dir_btn(Dir::Left, k_left, t("lbl_left")))
                    .child(stop)
                    .child(dir_btn(Dir::Right, k_right, t("lbl_right"))),
            )
            .child(h_flex().gap_1().child(cell()).child(dir_btn(Dir::Down, k_down, t("lbl_back"))).child(cell()));

        h_flex()
            .id(SharedString::from(format!("pad-{}", mec as u8)))
            .track_focus(&pad.focus)
            .justify_center()
            .items_center()
            .gap_3()
            .p_2()
            .when(mec, |d| d.child(dir_btn(Dir::LeftStrafe, "Q", t("lbl_left"))))
            .child(cross)
            .when(mec, |d| d.child(dir_btn(Dir::RightStrafe, "E", t("lbl_right"))))
            // Releasing the mouse anywhere releases every held direction, as in the original.
            .on_mouse_up(MouseButton::Left, cx.listener(move |this, _, _, cx| this.release_all(kind, cx)))
            .on_mouse_up_out(MouseButton::Left, cx.listener(move |this, _, _, cx| this.release_all(kind, cx)))
            .on_key_down(cx.listener(move |this, ev: &KeyDownEvent, _, cx| {
                if ev.is_held {
                    return;
                }
                let key = ev.keystroke.key.as_str();
                if key == "space" {
                    this.stop_motors(cx);
                } else if let Some(d) = Self::key_dir(kind, key) {
                    this.press(kind, d, cx);
                }
            }))
            .on_key_up(cx.listener(move |this, ev: &KeyUpEvent, _, cx| {
                if let Some(d) = Self::key_dir(kind, ev.keystroke.key.as_str()) {
                    this.release(kind, d, cx);
                }
            }))
    }

    fn show_channel_scan(&mut self, results: [(u8, i8); 13], window: &mut Window, cx: &mut Context<Self>) {
        let lang = self.app.read(cx).lang;
        let s = move |k: &str, d: &str| crate::i18n::tr_or(lang, k, d);
        let title = s("title_channel_scan", "ESP-NOW Channel Quality");
        let hint = s("lbl_channel_hint", "");
        let heads = [s("lbl_channel", "Channel"), s("lbl_ap_count", "APs"), s("lbl_rssi", "Signal(dBm)"), s("lbl_quality", "Quality")];
        window.open_dialog(cx, move |d: Dialog, _, cx| {
            let row = |cells: [String; 4], color: Option<Rgba>| {
                h_flex().py_1().children(cells.into_iter().enumerate().map(move |(i, c)| {
                    div().flex_1().text_center().when(i == 3 && color.is_some(), |d| d.text_color(color.unwrap())).child(c)
                }))
            };
            let rows = results.iter().enumerate().map(|(i, &(aps, rssi))| {
                let (q, c) = match aps {
                    0..=2 => ("★★★", 0x4CAF50),
                    3..=5 => ("★★", 0xFF9800),
                    _ => ("★", 0xF44336),
                };
                row([(i + 1).to_string(), aps.to_string(), rssi.to_string(), q.to_string()], Some(rgb(c)))
            });
            d.title(title.clone()).w(px(520.)).child(
                v_flex()
                    .gap_1()
                    .when(!hint.is_empty(), |v| v.child(div().text_sm().child(hint.clone())))
                    .child(
                        row(heads.clone(), None)
                            .font_weight(FontWeight::BOLD)
                            .border_b_1()
                            .border_color(cx.theme().border),
                    )
                    .children(rows),
            )
        });
    }
}

/// "AA:BB:CC:DD:EE:FF" → bytes; `None` when any part is not a hex byte.
fn parse_mac(s: &str) -> Option<Vec<u8>> {
    s.trim().split(':').map(|p| u8::from_str_radix(p.trim(), 16).ok()).collect()
}

impl Render for PeripheralPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let lang = self.app.read(cx).lang;
        let t = |k: &str| tr(lang, k);

        let conveyor = section(t("lbl_conveyor")).flex_1().child(field(t("lbl_speed"), NumberInput::new(&self.belt).small(), cx)).child(
            h_flex()
                .gap_2()
                .child(self.button("conv-run", "btn_run", cx, |this, cx| {
                    let v = Self::num(&this.belt, -100., 100., cx);
                    this.send(protocol::i8s(cmd::CONVEYOR_SET, &[v]), cx);
                }))
                .child(self.button("conv-stop", "btn_stop", cx, |this, cx| {
                    this.send(protocol::i8s(cmd::CONVEYOR_SET, &[0]), cx)
                })),
        );

        let stepper = section(t("lbl_stepper"))
            .flex_1()
            .child(field(t("lbl_steps"), NumberInput::new(&self.steps).small(), cx))
            .child(
                h_flex()
                    .gap_2()
                    .child(self.button("step-run", "btn_run", cx, |this, cx| {
                        let n = Self::num(&this.steps, -100_000., 100_000., cx);
                        this.send(protocol::stepper_run(n), cx);
                    }))
                    .child(self.button("step-reset", "btn_reset", cx, |this, cx| {
                        this.send(Request::sys(cmd::STEPPER_RESET, []), cx)
                    })),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().child(field(t("lbl_div"), Select::new(&self.div).small(), cx)))
                    .child(self.button("step-div", "btn_set_div", cx, |this, cx| {
                        let div = widgets::selected_text(&this.div, cx).and_then(|s| s.parse().ok()).unwrap_or(4);
                        this.send(protocol::stepper_div(div), cx);
                    })),
            );

        let esp = section("ESP-NOW")
            .flex_1()
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(self.button("esp-on", "btn_esp_mode_on", cx, |this, cx| {
                        this.send(Request::sys(cmd::ESPNOW_SYNC_CTRL, [1]), cx)
                    }))
                    .child(self.button("esp-off", "btn_esp_mode_off", cx, |this, cx| {
                        this.send(Request::sys(cmd::ESPNOW_SYNC_CTRL, [0]), cx)
                    })),
            )
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(self.button("sync-on", "btn_sync_on", cx, |this, cx| {
                        this.send(Request::sys(cmd::ESPNOW_SYNC_CTRL, [1]), cx)
                    }))
                    .child(self.button("sync-off", "btn_sync_off", cx, |this, cx| {
                        this.send(Request::sys(cmd::ESPNOW_SYNC_CTRL, [0]), cx)
                    })),
            )
            .child(field("MAC:", Input::new(&self.mac).small(), cx))
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(self.button("mac-pair", "btn_pair", cx, |this, cx| {
                        let mac = this.mac.clone();
                        this.send_mac(cmd::SET_PEER_MAC, &mac, "MAC Pair", cx);
                    }))
                    .child(self.button("mac-scan", "btn_scan", cx, |this, cx| {
                        this.send(Request::sys(cmd::SCAN_WIFI_CHANNELS, []), cx)
                    })),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().child(field(t("lbl_channel"), NumberInput::new(&self.channel).small(), cx)))
                    .child(self.button("set-ch", "btn_set_channel", cx, |this, cx| {
                        let ch = Self::num(&this.channel, 0., 14., cx) as u8;
                        this.send(Request::sys(cmd::SET_ESPNOW_CHANNEL, [ch]), cx);
                    })),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().child(field(t("lbl_sync_acc"), NumberInput::new(&self.sync_acc).small(), cx)))
                    .child(self.button("set-sync-acc", "btn_set_acc", cx, |this, cx| {
                        let acc = Self::num(&this.sync_acc, 0., 254., cx) as u8;
                        this.send(Request::sys(cmd::SET_GLOBAL_ACC, [acc]), cx);
                    })),
            )
            .child(self.button("scan-ch", "btn_scan_channels", cx, |this, cx| {
                this.send(Request::sys(cmd::SCAN_WIFI_CHANNELS, []), cx)
            }));

        let ps3 = section("PS3 MAC")
            .flex_1()
            .child(div().text_sm().child("PS3 MAC:"))
            .child(Input::new(&self.ps3_mac).small())
            .child(self.button("set-ps3", "btn_set_mac", cx, |this, cx| {
                let mac = this.ps3_mac.clone();
                this.send_mac(cmd::SET_PS3_MAC, &mac, "PS3 MAC", cx);
            }));

        let wheels = h_flex()
            .gap_3()
            .items_start()
            .child(section(t("grp_mecanum")).flex_1().child(self.render_pad(PadKind::Mecanum, cx)))
            .child(section(t("grp_tank")).flex_1().child(self.render_pad(PadKind::Tank, cx)));

        let sliders = (0..4).map(|i| {
            h_flex()
                .gap_3()
                .child(div().w(px(64.)).child(format!("M{}: {}", i + 1, self.motor_vals[i])))
                .child(div().flex_1().child(Slider::new(&self.motors[i])))
        });
        let multi = h_flex()
            .gap_2()
            .flex_wrap()
            .items_center()
            .child(t("lbl_sync_label"))
            .children((0..4).map(|i| {
                h_flex().gap_1().items_center().child(format!("M{}", i + 1)).child(div().w(px(130.)).child(NumberInput::new(&self.multi[i]).small()))
            }))
            .child(self.button("multi-send", "btn_sync_send", cx, |this, cx| {
                let v: Vec<i32> = this.multi.iter().map(|s| Self::num(s, -100., 100., cx)).collect();
                this.send(protocol::i8s(cmd::SET_MOTOR_SPEED, &v), cx);
            }))
            .child(self.button("stop-all", "btn_stop_all", cx, |this, cx| this.stop_motors(cx)).danger());
        let motors = section(t("grp_single_motor")).child(v_flex().gap_2().children(sliders)).child(multi);

        v_flex()
            .id("peripheral-page")
            .size_full()
            .p_3()
            .gap_3()
            .overflow_y_scroll()
            .child(h_flex().gap_3().items_start().child(conveyor).child(stepper).child(esp).child(ps3))
            .child(wheels)
            .child(motors)
    }
}

#[cfg(test)]
mod tests {
    use super::parse_mac;

    #[test]
    fn mac_parsing() {
        assert_eq!(parse_mac(" 24:6F:28:aa:0B:ff "), Some(vec![0x24, 0x6F, 0x28, 0xAA, 0x0B, 0xFF]));
        assert_eq!(parse_mac("24:6F:zz"), None);
        assert_eq!(parse_mac("1FF:00"), None);
    }
}
