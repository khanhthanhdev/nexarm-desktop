//! System settings: firmware/battery, BT mode, factory reset, motion acceleration,
//! buzzer, coordinate limits, chassis and kinematics parameters.

use crate::i18n::{Lang, tr_or};
use crate::protocol::{self, ChassisConfig, Request, Response, cmd};
use crate::ui::state::{AppState, DeviceEvent};
use crate::ui::widgets::{self, NumState, StrSelect, field, section};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, NumberInput};
use gpui_kit::component::select::Select;
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState, SliderValue};
use gpui_kit::component::{Sizable as _, h_flex, v_flex};
use gpui_kit::*;
use std::time::Duration;

/// Buzzer fields: (label key, default) — on ms, off ms, count, frequency Hz.
const BUZZER: [(&str, f64); 4] = [("lbl_buz_on", 100.), ("lbl_buz_off", 100.), ("lbl_buz_count", 2.), ("lbl_buz_freq", 2000.)];
const LIMITS: [(&str, f64); 12] = [
    ("X min", 50.),
    ("X max", 450.),
    ("Y min", -350.),
    ("Y max", 350.),
    ("Z min", 0.),
    ("Z max", 500.),
    ("Pitch min", -90.),
    ("Pitch max", 90.),
    ("Roll min", -90.),
    ("Roll max", 90.),
    ("Claw min", -60.),
    ("Claw max", 30.),
];
/// Pitch limits travel ×10 on the wire.
const PITCH_LIMITS: [usize; 2] = [6, 7];
/// Kinematics parameters in wire order: (label key, default).
const KINEMATICS: [(&str, f64); 9] = [
    ("kin_linkage1", 110.45),
    ("kin_linkage2", 225.0),
    ("kin_linkage2_perp", 36.97),
    ("kin_linkage3", 145.0),
    ("kin_linkage3_perp", 0.0),
    ("kin_linkage4", 130.23),
    ("kin_fixed_offset", 0.0),
    ("kin_base_radius", 50.0),
    ("kin_base_high", 70.5),
];
/// How long "set OK" replaces a button label.
const FLASH: Duration = Duration::from_millis(1500);

#[derive(Clone, Copy, PartialEq)]
enum Flash {
    Acc,
    Kin,
}

pub struct SystemPage {
    app: Entity<AppState>,
    firmware: Option<(String, Option<String>)>,
    battery_mv: Option<u16>,
    acc_slider: Entity<SliderState>,
    acc: NumState,
    buzzer: Vec<NumState>,
    limits: Vec<NumState>,
    chassis_type: Entity<StrSelect>,
    wheel_dia: NumState,
    wheel_base: NumState,
    track_width: NumState,
    max_speed: NumState,
    kin: Vec<NumState>,
    flash: Option<Flash>,
    _flash_task: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

fn chassis_items(lang: Lang) -> Vec<SharedString> {
    [("chassis_none", "None"), ("chassis_mecanum", "Mecanum"), ("chassis_tank", "Tank/Diff")]
        .iter()
        .map(|(k, d)| tr_or(lang, k, d).into())
        .collect()
}

/// Round to 2 decimals so f32 replies display like the original spin boxes.
fn round2(v: f32) -> f64 {
    (v as f64 * 100.).round() / 100.
}

impl SystemPage {
    pub fn new(app: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let lang = app.read(cx).lang;
        let saved_acc = app.read(cx).settings.move_acc.clamp(1, 254) as f64;
        let acc_slider =
            cx.new(|_| SliderState::new().min(1.).max(254.).step(1.).default_value(saved_acc as f32));
        let acc = widgets::num_input(saved_acc, 1., 254., 1., window, cx);
        let buzzer = BUZZER.iter().map(|&(_, v)| widgets::num_input(v, 0., 10000., 1., window, cx)).collect();
        let limits = LIMITS.iter().map(|&(_, v)| widgets::num_input(v, -1000., 1000., 1., window, cx)).collect();
        let kin = KINEMATICS.iter().map(|&(_, v)| widgets::num_input(v, -500., 500., 0.01, window, cx)).collect();

        let subs = vec![
            cx.observe(&app, |_, _, cx| cx.notify()),
            cx.subscribe_in(&app, window, |this, _, ev: &DeviceEvent, window, cx| this.on_device(ev, window, cx)),
            cx.subscribe_in(&acc_slider, window, |this, _, ev: &SliderEvent, window, cx| {
                let (SliderEvent::Change(SliderValue::Single(v)) | SliderEvent::Release(SliderValue::Single(v))) = ev
                else {
                    return;
                };
                let v = (*v as f64).round();
                if widgets::read_num(&this.acc, cx) != v {
                    widgets::set_num(&this.acc, v, window, cx);
                }
            }),
            cx.subscribe_in(&acc, window, |this, state, ev: &InputEvent, window, cx| {
                if !matches!(ev, InputEvent::Change) {
                    return;
                }
                let v = widgets::read_num(state, cx).clamp(1., 254.) as f32;
                if this.acc_slider.read(cx).value() != SliderValue::Single(v) {
                    this.acc_slider.update(cx, |s, cx| s.set_value(v, window, cx));
                }
            }),
        ];

        Self {
            app,
            firmware: None,
            battery_mv: None,
            acc_slider,
            acc,
            buzzer,
            limits,
            chassis_type: widgets::select(chassis_items(lang), 0, window, cx),
            wheel_dia: widgets::num_input(25., 10., 500., 0.1, window, cx),
            wheel_base: widgets::num_input(190., 10., 1000., 0.1, window, cx),
            track_width: widgets::num_input(95., 10., 1000., 0.1, window, cx),
            max_speed: widgets::num_input(100., 1., 100., 1., window, cx),
            kin,
            flash: None,
            _flash_task: None,
            _subs: subs,
        }
    }

    pub fn on_shown(&mut self, _: &mut Window, _: &mut Context<Self>) {}

    fn on_device(&mut self, ev: &DeviceEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            DeviceEvent::LangChanged => {
                let lang = self.app.read(cx).lang;
                let ix = widgets::selected_index(&self.chassis_type, cx);
                widgets::set_items(&self.chassis_type, chassis_items(lang), ix, window, cx);
            }
            DeviceEvent::Response(Response::FirmwareVersion { esp, at32 }) => {
                // A reply without the AT32 part keeps the previously shown AT32 version.
                let prev_at32 = self.firmware.take().and_then(|(_, a)| a);
                self.firmware = Some((esp.clone(), at32.clone().or(prev_at32)));
            }
            DeviceEvent::Response(Response::Battery(mv)) => self.battery_mv = Some(*mv),
            DeviceEvent::Response(Response::CoordLimits(v)) => {
                for (i, raw) in v.iter().enumerate() {
                    let shown = if PITCH_LIMITS.contains(&i) { raw.div_euclid(10) } else { *raw };
                    widgets::set_num(&self.limits[i], shown as f64, window, cx);
                }
            }
            DeviceEvent::Response(Response::Chassis(c)) => {
                let ix = (c.kind as usize).min(2);
                let lang = self.app.read(cx).lang;
                widgets::set_items(&self.chassis_type, chassis_items(lang), ix, window, cx);
                widgets::set_num(&self.wheel_dia, round2(c.wheel_diameter).clamp(10., 500.), window, cx);
                widgets::set_num(&self.wheel_base, round2(c.wheel_base).clamp(10., 1000.), window, cx);
                widgets::set_num(&self.track_width, round2(c.track_width).clamp(10., 1000.), window, cx);
                widgets::set_num(&self.max_speed, (c.max_speed as f64).clamp(1., 100.), window, cx);
            }
            DeviceEvent::Response(Response::Kinematics(v)) => {
                for (s, x) in self.kin.iter().zip(v) {
                    widgets::set_num(s, round2(*x).clamp(-500., 500.), window, cx);
                }
            }
            _ => return,
        }
        cx.notify();
    }

    fn send(&self, req: Request, cx: &mut Context<Self>) {
        self.app.update(cx, |a, cx| a.send(req, cx));
    }

    fn send_sys(&self, command: u8, args: impl Into<Vec<u8>>, cx: &mut Context<Self>) {
        self.send(Request::sys(command, args), cx);
    }

    /// Show "set OK" on a button for a moment.
    fn flash(&mut self, which: Flash, cx: &mut Context<Self>) {
        self.flash = Some(which);
        cx.notify();
        self._flash_task = Some(cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            cx.background_executor().timer(FLASH).await;
            let _ = this.update(cx, |p, cx| {
                p.flash = None;
                cx.notify();
            });
        }));
    }

    fn set_move_acc(&mut self, cx: &mut Context<Self>) {
        let v = widgets::read_num(&self.acc, cx).clamp(1., 254.) as u8;
        self.send_sys(cmd::SET_MOVE_ACC, [v], cx);
        self.app.update(cx, |a, _| {
            a.settings.move_acc = v;
            a.persist();
        });
        self.flash(Flash::Acc, cx);
    }

    fn set_buzzer(&mut self, cx: &mut Context<Self>) {
        let v: Vec<f64> = self.buzzer.iter().map(|s| widgets::read_num(s, cx).clamp(0., 10000.)).collect();
        self.send(protocol::buzzer(v[0] as u32, v[1] as u32, v[2] as u16, v[3] as u16), cx);
    }

    fn set_coord_limits(&mut self, cx: &mut Context<Self>) {
        let mut v = [0i16; 12];
        for (i, s) in self.limits.iter().enumerate() {
            let x = widgets::read_i(s, cx).clamp(-1000, 1000) as i16;
            v[i] = if PITCH_LIMITS.contains(&i) { x * 10 } else { x };
        }
        self.send(protocol::set_coord_limits(v), cx);
    }

    fn set_chassis(&mut self, cx: &mut Context<Self>) {
        let c = ChassisConfig {
            kind: widgets::selected_index(&self.chassis_type, cx) as u8,
            wheel_diameter: widgets::read_num(&self.wheel_dia, cx).clamp(10., 500.) as f32,
            wheel_base: widgets::read_num(&self.wheel_base, cx).clamp(10., 1000.) as f32,
            track_width: widgets::read_num(&self.track_width, cx).clamp(10., 1000.) as f32,
            max_speed: widgets::read_num(&self.max_speed, cx).clamp(1., 100.) as u8,
        };
        self.send(protocol::set_chassis(&c), cx);
    }

    fn set_kinematics(&mut self, cx: &mut Context<Self>) {
        let mut p = [0f32; 9];
        for (x, s) in p.iter_mut().zip(&self.kin) {
            *x = widgets::read_num(s, cx).clamp(-500., 500.) as f32;
        }
        self.send(protocol::set_kinematics(p), cx);
        self.flash(Flash::Kin, cx);
    }

    fn factory_reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (title, msg) = {
            let lang = self.app.read(cx).lang;
            (tr_or(lang, "btn_factory_reset", "Factory Reset"), tr_or(lang, "btn_factory_reset", "Factory Reset") + "?")
        };
        let app = self.app.clone();
        widgets::confirm(title, msg, window, cx, move |_, cx| {
            app.update(cx, |a, cx| a.send_sys(cmd::FACTORY_RESET, [], cx));
        });
    }
}

/// Fixed-width wrapper so wrapped rows of fields line up like a grid.
fn cell(w: f32, child: impl IntoElement) -> Div {
    div().w(px(w)).child(child)
}

impl Render for SystemPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let lang = self.app.read(cx).lang;
        let t = |k: &str| crate::i18n::tr(lang, k);
        let t_or = |k: &str, d: &str| tr_or(lang, k, d);
        let set_ok = t_or("msg_set_success", "Set OK");

        let fw_esp = match &self.firmware {
            Some((esp, _)) => format!("{}:  V {esp}", t_or("lbl_firmware_esp", "ESP32 FW")),
            None => format!("{}:  --", t_or("lbl_firmware_esp", "ESP32 FW")),
        };
        let fw_at32 = match self.firmware.as_ref().and_then(|(_, a)| a.as_ref()) {
            Some(at32) => format!("{}:  V {at32}", t_or("lbl_firmware_at32", "AT32 FW")),
            None => format!("{}:  --", t_or("lbl_firmware_at32", "AT32 FW")),
        };
        let battery = match self.battery_mv {
            Some(mv) => format!("{}:  {:.2} V", t_or("lbl_battery", "Voltage"), mv as f64 / 1000.0),
            None => format!("{}:  --", t_or("lbl_battery", "Voltage")),
        };

        let sys = section(t("grp_sys_params")).child(
            h_flex()
                .gap_2()
                .flex_wrap()
                .items_center()
                .child(Button::new("read-ver").label(t("btn_read_ver")).on_click(
                    cx.listener(|this, _, _, cx| this.send_sys(cmd::FIRMWARE_VERSION_CHECK, [], cx)),
                ))
                .child(Button::new("read-bat").label(t("btn_read_bat")).on_click(
                    cx.listener(|this, _, _, cx| this.send_sys(cmd::CHECK_BAT_LEVEL, [], cx)),
                ))
                .child(Button::new("bt-ps3").label(t_or("btn_switch_ps3", "Switch to PS3")).on_click(
                    cx.listener(|this, _, _, cx| this.send_sys(cmd::SET_BT_MODE, [1], cx)),
                ))
                .child(Button::new("bt-ble").label(t_or("btn_switch_ble", "Switch to BLE")).on_click(
                    cx.listener(|this, _, _, cx| this.send_sys(cmd::SET_BT_MODE, [0], cx)),
                ))
                .child(
                    Button::new("factory-reset")
                        .danger()
                        .label(t_or("btn_factory_reset", "Factory Reset"))
                        .on_click(cx.listener(|this, _, window, cx| this.factory_reset(window, cx))),
                )
                .child(div().ml_4().child(fw_esp))
                .child(div().ml_2().child(fw_at32))
                .child(div().ml_4().child(battery)),
        );

        let acc = section(t_or("grp_move_acc", "Global Motion Acceleration")).child(
            h_flex()
                .gap_3()
                .items_center()
                .child(t_or("lbl_acc_val", "Acceleration Value (0-254):"))
                .child(div().flex_1().min_w(px(160.)).child(Slider::new(&self.acc_slider)))
                .child(div().w(px(140.)).child(NumberInput::new(&self.acc).small()))
                .child(
                    Button::new("set-acc")
                        .primary()
                        .label(if self.flash == Some(Flash::Acc) { set_ok.clone() } else { t_or("btn_set_acc", "Set/Apply") })
                        .on_click(cx.listener(|this, _, _, cx| this.set_move_acc(cx))),
                ),
        );

        let buzzer = section(t("grp_buzzer")).child(
            h_flex()
                .gap_3()
                .flex_wrap()
                .items_center()
                .children(
                    BUZZER
                        .iter()
                        .zip(&self.buzzer)
                        .map(|((k, _), s)| cell(220., field(t(k), NumberInput::new(s).small(), cx))),
                )
                .child(
                    Button::new("buzzer")
                        .label(t("btn_buz_send"))
                        .on_click(cx.listener(|this, _, _, cx| this.set_buzzer(cx))),
                ),
        );

        let limits = section(t("grp_coord_limits"))
            .child(
                h_flex().gap_3().flex_wrap().children(
                    LIMITS
                        .iter()
                        .zip(&self.limits)
                        .map(|((k, _), s)| cell(200., field(format!("{k}:"), NumberInput::new(s).small(), cx))),
                ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .child(Button::new("read-cl").label(t("btn_read_coord_limits")).on_click(
                        cx.listener(|this, _, _, cx| this.send_sys(cmd::GET_COORD_LIMITS, [], cx)),
                    ))
                    .child(
                        Button::new("set-cl")
                            .primary()
                            .label(t("btn_set_coord_limits"))
                            .on_click(cx.listener(|this, _, _, cx| this.set_coord_limits(cx))),
                    ),
            );

        let chassis = section(t_or("grp_chassis_config", "Chassis Parameters"))
            .child(
                h_flex()
                    .gap_3()
                    .flex_wrap()
                    .child(cell(260., field(t_or("lbl_chassis_type", "Type:"), Select::new(&self.chassis_type).small(), cx)))
                    .child(cell(260., field(t_or("lbl_wheel_diameter", "Wheel Dia:"), NumberInput::new(&self.wheel_dia).small(), cx)))
                    .child(cell(260., field(t_or("lbl_wheel_base", "Wheelbase:"), NumberInput::new(&self.wheel_base).small(), cx)))
                    .child(cell(260., field(t_or("lbl_track_width", "Track Width:"), NumberInput::new(&self.track_width).small(), cx)))
                    .child(cell(260., field(t_or("lbl_max_speed", "Max Speed:"), NumberInput::new(&self.max_speed).small(), cx))),
            )
            .child(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .child(Button::new("read-chassis").label(t_or("btn_read_chassis", "Read Chassis")).on_click(
                        cx.listener(|this, _, _, cx| this.send_sys(cmd::GET_CHASSIS_CONFIG, [], cx)),
                    ))
                    .child(
                        Button::new("set-chassis")
                            .primary()
                            .label(t_or("btn_set_chassis", "Write Chassis"))
                            .on_click(cx.listener(|this, _, _, cx| this.set_chassis(cx))),
                    ),
            );

        let kinematics = section(t_or("grp_kinematics", "Kinematics Parameters"))
            .child(
                h_flex().gap_3().flex_wrap().children(
                    KINEMATICS
                        .iter()
                        .zip(&self.kin)
                        .map(|((k, _), s)| cell(260., field(t(k), NumberInput::new(s).small(), cx))),
                ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("set-kin")
                            .primary()
                            .label(if self.flash == Some(Flash::Kin) { set_ok } else { t_or("btn_set_kin", "Write Params") })
                            .on_click(cx.listener(|this, _, _, cx| this.set_kinematics(cx))),
                    )
                    .child(Button::new("read-kin").label(t_or("btn_read_kin", "Read Params")).on_click(
                        cx.listener(|this, _, _, cx| this.send_sys(cmd::GET_KINEMATICS_PARAM, [], cx)),
                    )),
            );

        div().id("system-page").size_full().overflow_y_scroll().child(
            v_flex()
                .p_3()
                .gap_3()
                .child(sys)
                .child(acc)
                .child(buzzer)
                .child(limits)
                .child(chassis)
                .child(kinematics),
        )
    }
}
