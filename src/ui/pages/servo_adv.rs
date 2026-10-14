//! Servo advanced settings: ID/mode, position offset, PID, overload protection, baud, max torque.

use crate::i18n::{Lang, tr, tr_list};
use crate::protocol::{self, BROADCAST_ID, Request, Response, cmd};
use crate::ui::state::{AppState, DeviceEvent};
use crate::ui::widgets::{self, NumState, StrSelect, section};
use gpui_kit::component::button::Button;
use gpui_kit::component::input::{InputEvent, NumberInput};
use gpui_kit::component::select::Select;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::*;
use std::time::Duration;

/// Servo baud choices; the number before ':' is the firmware code.
const BAUDS: [&str; 7] = ["0: 1M", "1: 500K", "2: 250K", "4: 115200", "5: 76800", "6: 57600", "7: 38400"];
const MODES_FALLBACK: [&str; 4] = ["0: Position", "1: Wheel", "2: PWM", "3: Stepper"];
const FLASH: Duration = Duration::from_millis(1500);

pub struct ServoAdvPage {
    app: Entity<AppState>,
    old_id: NumState,
    new_id: NumState,
    mode_id: NumState,
    mode: Entity<StrSelect>,
    adv_id: NumState,
    offset: NumState,
    p: NumState,
    i: NumState,
    d: NumState,
    min_force: NumState,
    ol_id: NumState,
    ol_torque: NumState,
    ol_time: NumState,
    ol_thresh: NumState,
    baud_id: NumState,
    baud: Entity<StrSelect>,
    mt_id: NumState,
    mt_val: NumState,
    /// Button key currently showing "set OK"; cleared by `_flash_task`.
    flash: Option<&'static str>,
    _flash_task: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

fn modes(lang: Lang) -> Vec<SharedString> {
    let list = tr_list(lang, "servo_modes");
    if list.is_empty() {
        MODES_FALLBACK.iter().map(|s| (*s).into()).collect()
    } else {
        list.into_iter().map(Into::into).collect()
    }
}

/// Firmware baud code → index in [`BAUDS`].
fn baud_index(code: u8) -> usize {
    BAUDS.iter().position(|b| b.split(':').next() == Some(&code.to_string())).unwrap_or(0)
}

impl ServoAdvPage {
    pub fn new(app: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let lang = app.read(cx).lang;
        let id = |window: &mut Window, cx: &mut Context<Self>| widgets::num_input(1., 1., 255., 1., window, cx);
        let byte = |v: f64, window: &mut Window, cx: &mut Context<Self>| widgets::num_input(v, 0., 254., 1., window, cx);
        let offset = widgets::num_input(0., -2047., 2047., 1., window, cx);
        let mode = widgets::select(modes(lang), 0, window, cx);
        let subs = vec![
            cx.observe(&app, |_, _, cx| cx.notify()),
            cx.subscribe_in(&app, window, |this, _, ev: &DeviceEvent, window, cx| match ev {
                DeviceEvent::Response(r) => this.on_response(r, window, cx),
                DeviceEvent::LangChanged => {
                    let lang = this.app.read(cx).lang;
                    let ix = widgets::selected_index(&this.mode, cx);
                    widgets::set_items(&this.mode, modes(lang), ix, window, cx);
                }
                _ => {}
            }),
            // Enter / focus-out previews the offset live, without the read-back.
            cx.subscribe_in(&offset, window, |this, _, ev: &InputEvent, window, cx| {
                if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    this.send_offset(window, cx);
                }
            }),
        ];
        Self {
            old_id: id(window, cx),
            new_id: id(window, cx),
            mode_id: id(window, cx),
            mode,
            adv_id: id(window, cx),
            offset,
            p: widgets::num_input(0., 0., 255., 1., window, cx),
            i: widgets::num_input(0., 0., 255., 1., window, cx),
            d: widgets::num_input(0., 0., 255., 1., window, cx),
            min_force: widgets::num_input(0., 0., 65535., 1., window, cx),
            ol_id: id(window, cx),
            ol_torque: byte(20., window, cx),
            ol_time: byte(200., window, cx),
            ol_thresh: byte(30., window, cx),
            baud_id: id(window, cx),
            baud: widgets::select(BAUDS.iter().map(|b| (*b).into()).collect(), 0, window, cx),
            mt_id: id(window, cx),
            mt_val: widgets::num_input(1000., 0., 1000., 1., window, cx),
            flash: None,
            _flash_task: None,
            _subs: subs,
            app,
        }
    }

    pub fn on_shown(&mut self, _: &mut Window, _: &mut Context<Self>) {}

    /// Spin-box value clamped to its range, as Qt would hold it.
    fn val(state: &NumState, lo: i64, hi: i64, cx: &App) -> i64 {
        widgets::read_i(state, cx).clamp(lo, hi)
    }

    fn id_of(state: &NumState, cx: &App) -> u8 {
        Self::val(state, 1, 255, cx) as u8
    }

    fn send(&self, req: Request, cx: &mut Context<Self>) {
        self.app.update(cx, |a, cx| a.send(req, cx));
    }

    fn sys(&self, command: u8, args: Vec<u8>, cx: &mut Context<Self>) {
        self.send(Request::sys(command, args), cx);
    }

    /// Show "set OK" on the button for 1.5 s.
    fn flash(&mut self, key: &'static str, cx: &mut Context<Self>) {
        self.flash = Some(key);
        self._flash_task = Some(cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            cx.background_executor().timer(FLASH).await;
            let _ = this.update(cx, |p, cx| {
                p.flash = None;
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Clamp the offset field back into range and send it; returns the servo id.
    fn send_offset(&mut self, window: &mut Window, cx: &mut Context<Self>) -> u8 {
        let sid = Self::id_of(&self.adv_id, cx);
        let offset = Self::val(&self.offset, -2047, 2047, cx);
        widgets::set_num(&self.offset, offset as f64, window, cx);
        self.send(protocol::set_pos_offset(sid, offset as i16), cx);
        sid
    }

    fn on_response(&mut self, r: &Response, window: &mut Window, cx: &mut Context<Self>) {
        let is = |state: &NumState, id: u8, cx: &App| Self::id_of(state, cx) == id;
        match *r {
            Response::ServoOffset { id, offset } if is(&self.adv_id, id, cx) => {
                widgets::set_num(&self.offset, offset.clamp(-2047, 2047) as f64, window, cx);
            }
            Response::ServoPid { id, p, i, d, min_force } if is(&self.adv_id, id, cx) => {
                widgets::set_num(&self.p, p as f64, window, cx);
                widgets::set_num(&self.i, i as f64, window, cx);
                widgets::set_num(&self.d, d as f64, window, cx);
                widgets::set_num(&self.min_force, min_force as f64, window, cx);
            }
            Response::ServoOverload { id, torque, time, threshold } if is(&self.ol_id, id, cx) => {
                widgets::set_num(&self.ol_torque, torque as f64, window, cx);
                widgets::set_num(&self.ol_time, time as f64, window, cx);
                widgets::set_num(&self.ol_thresh, threshold as f64, window, cx);
            }
            Response::ServoBaud { id, code } if is(&self.baud_id, id, cx) => {
                let items = BAUDS.iter().map(|b| (*b).into()).collect();
                widgets::set_items(&self.baud, items, baud_index(code), window, cx);
            }
            Response::ServoMaxTorque { id, torque } if is(&self.mt_id, id, cx) => {
                widgets::set_num(&self.mt_val, torque as f64, window, cx);
            }
            _ => {}
        }
    }

    fn baud_code(&self, cx: &App) -> u8 {
        BAUDS[widgets::selected_index(&self.baud, cx).min(BAUDS.len() - 1)]
            .split(':')
            .next()
            .and_then(|c| c.parse().ok())
            .unwrap_or(0)
    }

    fn action(&mut self, key: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let id = |s: &NumState| Self::id_of(s, cx);
        match key {
            "btn_set_id" => self.sys(cmd::SET_SERVO_ID, vec![id(&self.old_id), id(&self.new_id)], cx),
            "btn_set_mode" => {
                let mode = widgets::selected_index(&self.mode, cx) as u8;
                self.sys(cmd::SET_SERVO_MODE, vec![id(&self.mode_id), mode], cx)
            }
            "btn_set_offset" => {
                let sid = self.send_offset(window, cx);
                self.sys(cmd::GET_POS_OFFSET, vec![sid], cx);
            }
            "btn_get_offset" => return self.sys(cmd::GET_POS_OFFSET, vec![id(&self.adv_id)], cx),
            "btn_set_pid" => {
                let b = |s: &NumState| Self::val(s, 0, 255, cx) as u8;
                let req = protocol::set_pid(
                    id(&self.adv_id),
                    b(&self.p),
                    b(&self.i),
                    b(&self.d),
                    Self::val(&self.min_force, 0, 65535, cx) as u16,
                );
                self.send(req, cx);
            }
            "btn_get_pid" => return self.sys(cmd::GET_PID_PARAM, vec![id(&self.adv_id)], cx),
            "btn_read_overload" => return self.sys(cmd::READ_OVERLOAD, vec![id(&self.ol_id)], cx),
            "btn_set_overload" => {
                let b = |s: &NumState| Self::val(s, 0, 254, cx) as u8;
                let args = vec![id(&self.ol_id), b(&self.ol_torque), b(&self.ol_time), b(&self.ol_thresh)];
                self.sys(cmd::SET_OVERLOAD, args, cx);
            }
            "btn_read_baud" => return self.sys(cmd::READ_BAUD, vec![id(&self.baud_id)], cx),
            "btn_set_baud" => self.sys(cmd::SET_BAUD, vec![id(&self.baud_id), self.baud_code(cx)], cx),
            "btn_set_baud_all" => self.sys(cmd::SET_BAUD, vec![BROADCAST_ID, self.baud_code(cx)], cx),
            "btn_read_max_torque" => return self.sys(cmd::READ_MAX_TORQUE, vec![id(&self.mt_id)], cx),
            "btn_set_max_torque" => {
                let req = protocol::set_max_torque(id(&self.mt_id), Self::val(&self.mt_val, 0, 1000, cx) as u16);
                self.send(req, cx);
            }
            _ => return,
        }
        // Every "set" action flashes its button; reads returned above.
        self.flash(key, cx);
    }

    fn button(&self, key: &'static str, lang: Lang, cx: &mut Context<Self>) -> Button {
        let label = if self.flash == Some(key) { tr(lang, "msg_set_success") } else { tr(lang, key) };
        Button::new(key)
            .small()
            .label(label)
            .on_click(cx.listener(move |this, _, window, cx| this.action(key, window, cx)))
    }
}

/// `label [control]` cell sized to share a row with others.
fn cell(label: impl Into<SharedString>, control: impl IntoElement, cx: &App) -> Div {
    widgets::field(label, control, cx).flex_1().min_w(px(220.))
}

fn num(state: &NumState) -> NumberInput {
    NumberInput::new(state).small()
}

impl Render for ServoAdvPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let lang = self.app.read(cx).lang;
        let t = |k: &str| tr(lang, k);
        let row = || h_flex().gap_3().items_center().flex_wrap();
        let muted = cx.theme().muted_foreground;
        let pid_part = |name: &'static str, state: &NumState| {
            h_flex().gap_1().items_center().child(div().text_sm().text_color(muted).child(name)).child(num(state).w(px(110.)))
        };

        let cfg = section(t("grp_servo_cfg"))
            .child(
                row()
                    .child(cell(t("lbl_old_id"), num(&self.old_id), cx))
                    .child(cell(t("lbl_new_id"), num(&self.new_id), cx))
                    .child(self.button("btn_set_id", lang, cx)),
            )
            .child(
                row()
                    .child(cell(t("lbl_servo_id"), num(&self.mode_id), cx))
                    .child(cell(t("lbl_mode"), Select::new(&self.mode).small(), cx))
                    .child(self.button("btn_set_mode", lang, cx)),
            );

        let adv = section(t("grp_servo_adv"))
            .child(
                row()
                    .child(cell(t("lbl_servo_id"), num(&self.adv_id), cx))
                    .child(cell(t("lbl_offset"), num(&self.offset), cx))
                    .child(self.button("btn_set_offset", lang, cx))
                    .child(self.button("btn_get_offset", lang, cx)),
            )
            .child(
                row()
                    .child(div().min_w(px(84.)).text_sm().text_color(muted).child(t("lbl_pid")))
                    .child(pid_part("P:", &self.p))
                    .child(pid_part("I:", &self.i))
                    .child(pid_part("D:", &self.d))
                    .child(pid_part("MinF:", &self.min_force))
                    .child(self.button("btn_set_pid", lang, cx))
                    .child(self.button("btn_get_pid", lang, cx)),
            );

        let overload = section(t("grp_overload"))
            .child(
                row()
                    .child(cell(t("lbl_servo_id"), num(&self.ol_id), cx))
                    .child(cell(t("lbl_overload_torque"), num(&self.ol_torque), cx))
                    .child(self.button("btn_read_overload", lang, cx))
                    .child(self.button("btn_set_overload", lang, cx)),
            )
            .child(
                row()
                    .child(cell(t("lbl_overload_time"), num(&self.ol_time), cx))
                    .child(cell(crate::i18n::tr_or(lang, "lbl_overload_thresh", "Torque Threshold"), num(&self.ol_thresh), cx)),
            );

        let baud = section(t("grp_baud")).child(
            row()
                .child(cell(t("lbl_servo_id"), num(&self.baud_id), cx))
                .child(cell(t("lbl_baud"), Select::new(&self.baud).small(), cx))
                .child(self.button("btn_read_baud", lang, cx))
                .child(self.button("btn_set_baud", lang, cx))
                .child(self.button("btn_set_baud_all", lang, cx)),
        );

        let torque = section(t("grp_max_torque")).child(
            row()
                .child(cell(t("lbl_servo_id"), num(&self.mt_id), cx))
                .child(cell(t("lbl_max_torque"), num(&self.mt_val), cx))
                .child(self.button("btn_read_max_torque", lang, cx))
                .child(self.button("btn_set_max_torque", lang, cx)),
        );

        v_flex()
            .id("servo-adv-page")
            .size_full()
            .p_3()
            .gap_3()
            .overflow_y_scroll()
            .child(cfg)
            .child(adv)
            .child(overload)
            .child(baud)
            .child(torque)
    }
}
