//! Guided calibration of the follower arm: release the arm, set the centre in the servos,
//! record each joint's range of motion by hand, and keep the result as a named profile.
//!
//! The zero lives in the servos (centre calibration and position offsets). Offsets are
//! read back from the arm, never computed here.

use crate::calib::{self, JOINTS, JointCheck, Profile, RangeTracker};
use crate::i18n::{tr, tr_list};
use crate::protocol::{self, Request, Response, cmd};
use crate::ui::state::{AppState, DeviceEvent};
use crate::ui::widgets::{self, section};
use gpui_kit::component::button::Button;
use gpui_kit::component::input::Input;
use gpui_kit::component::select::Select;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::*;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Position poll interval while recording.
const POLL: Duration = Duration::from_millis(40);
/// Gap between consecutive servo requests.
const GAP: Duration = Duration::from_millis(60);

pub struct CalibPage {
    app: Entity<AppState>,
    name: Entity<component::input::InputState>,
    saved: Entity<widgets::StrSelect>,
    now: [Option<u16>; JOINTS],
    offsets: [Option<i16>; JOINTS],
    tracker: RangeTracker,
    recording: bool,
    /// Profile last loaded, for restoring its offsets.
    loaded: Option<Profile>,
    _poll: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl CalibPage {
    pub fn new(app: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subs = vec![
            cx.observe(&app, |_, _, cx| cx.notify()),
            cx.subscribe(&app, |this, _, ev: &DeviceEvent, cx| match ev {
                DeviceEvent::Response(r) => this.on_response(r, cx),
                DeviceEvent::Disconnected => this.stop_recording(cx),
                _ => {}
            }),
        ];
        let names = calib::list(&calib::profiles_dir()).into_iter().map(Into::into).collect();
        Self {
            name: widgets::text_input("", "", window, cx),
            saved: widgets::select(names, 0, window, cx),
            now: [None; JOINTS],
            offsets: [None; JOINTS],
            tracker: RangeTracker::default(),
            recording: false,
            loaded: None,
            _poll: None,
            _subs: subs,
            app,
        }
    }

    fn t(&self, key: &str, cx: &App) -> String {
        tr(self.app.read(cx).lang, key)
    }

    /// Notify and return `false` when there is no link to the arm.
    fn require_link(&self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let connected = self.app.read(cx).is_connected();
        if !connected {
            widgets::notify_error(self.t("msg_calib_not_connected", cx), window, cx);
        }
        connected
    }

    fn on_response(&mut self, r: &Response, cx: &mut Context<Self>) {
        match r {
            Response::JointPositions(p) => {
                for (slot, v) in self.now.iter_mut().zip(p) {
                    if v.is_some() {
                        *slot = *v;
                    }
                }
                if self.recording && p.iter().all(Option::is_some) {
                    self.tracker.record(&std::array::from_fn(|i| p[i].unwrap_or_default()));
                }
            }
            Response::ServoOffset { id, offset } if (1..=JOINTS as u8).contains(id) => {
                self.offsets[*id as usize - 1] = Some(*offset);
            }
            _ => return,
        }
        cx.notify();
    }

    // ------------------------------------------------------------ actions

    fn torque_off(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.require_link(window, cx) {
            return;
        }
        let app = self.app.clone();
        widgets::confirm(self.t("btn_calib_torque_off", cx), self.t("msg_calib_torque_off", cx), window, cx, move |_, cx| {
            let reqs = (1..=JOINTS as u8).map(|id| protocol::servo_torque(id, false)).collect();
            app.update(cx, |a, cx| a.send_paced(reqs, GAP, cx));
        });
    }

    fn torque_on(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.require_link(window, cx) {
            return;
        }
        let reqs = (1..=JOINTS as u8).map(|id| protocol::servo_torque(id, true)).collect();
        self.app.update(cx, |a, cx| a.send_paced(reqs, GAP, cx));
    }

    /// Take the current pose as the centre in every servo, then read the offsets back.
    fn set_centre(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.require_link(window, cx) {
            return;
        }
        let app = self.app.clone();
        widgets::confirm(self.t("btn_calib_set_center", cx), self.t("msg_cali_confirm", cx), window, cx, move |_, cx| {
            let mut reqs: Vec<Request> = (1..=JOINTS as u8).map(|id| Request::sys(cmd::SERVO_CALI_POS, [id])).collect();
            reqs.extend((1..=JOINTS as u8).map(|id| Request::sys(cmd::GET_POS_OFFSET, [id])));
            app.update(cx, |a, cx| a.send_paced(reqs, GAP, cx));
        });
    }

    fn read_offsets(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.require_link(window, cx) {
            return;
        }
        let reqs = (1..=JOINTS as u8).map(|id| Request::sys(cmd::GET_POS_OFFSET, [id])).collect();
        self.app.update(cx, |a, cx| a.send_paced(reqs, GAP, cx));
    }

    fn start_recording(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.require_link(window, cx) {
            return;
        }
        self.recording = true;
        let app = self.app.clone();
        self._poll = Some(cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                let more = this.update(cx, |p, cx| {
                    let live = p.recording && p.app.read(cx).is_connected();
                    if live {
                        app.update(cx, |a, cx| a.send_sys(cmd::GET_REAL_JOINT_ANGLES, [], cx));
                    } else {
                        p.recording = false;
                        cx.notify();
                    }
                    live
                });
                if !more.unwrap_or(false) {
                    break;
                }
                cx.background_executor().timer(POLL).await;
            }
        }));
        cx.notify();
    }

    /// Stop polling; also called when the link drops or the page is left.
    pub fn stop_recording(&mut self, cx: &mut Context<Self>) {
        self.recording = false;
        self._poll = None;
        cx.notify();
    }

    fn clear_ranges(&mut self, cx: &mut Context<Self>) {
        self.tracker = RangeTracker::default();
        cx.notify();
    }

    fn refresh_saved(&mut self, select: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        let names: Vec<String> = calib::list(&calib::profiles_dir());
        let ix = select.and_then(|n| names.iter().position(|x| x == n)).unwrap_or(0);
        widgets::set_items(&self.saved, names.into_iter().map(Into::into).collect(), ix, window, cx);
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = widgets::read_text(&self.name, cx);
        let Some(slug) = calib::slug(&name) else {
            return widgets::notify_error(self.t("msg_calib_name_empty", cx), window, cx);
        };
        let Some(ranges) = self.tracker.ranges() else {
            return widgets::notify_error(self.t("msg_calib_nothing_to_save", cx), window, cx);
        };
        let saved_at = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let profile = Profile { name: slug.clone(), saved_at, offsets: self.offsets, ranges };
        match calib::save(&calib::profiles_dir(), &profile) {
            Ok(()) => {
                self.refresh_saved(Some(&slug), window, cx);
                widgets::notify_info(self.t("msg_calib_saved", cx), window, cx);
            }
            Err(e) => widgets::notify_error(format!("{} {e}", self.t("msg_calib_error", cx)), window, cx),
        }
    }

    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = widgets::selected_text(&self.saved, cx) else { return };
        match calib::load(&calib::profiles_dir(), &name) {
            Ok(p) => {
                self.offsets = p.offsets;
                self.tracker = RangeTracker::default();
                self.tracker.record(&p.ranges.map(|r| r.0));
                self.tracker.record(&p.ranges.map(|r| r.1));
                self.loaded = Some(p);
                cx.notify();
            }
            Err(e) => widgets::notify_error(format!("{} {e}", self.t("msg_calib_error", cx)), window, cx),
        }
    }

    fn delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = widgets::selected_text(&self.saved, cx) else { return };
        if let Err(e) = calib::delete(&calib::profiles_dir(), &name) {
            widgets::notify_error(format!("{} {e}", self.t("msg_calib_error", cx)), window, cx);
        }
        self.refresh_saved(None, window, cx);
    }

    /// Write the loaded profile's offsets back to the servos.
    fn restore_offsets(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(profile) = self.loaded.clone() else { return };
        if !self.require_link(window, cx) {
            return;
        }
        let app = self.app.clone();
        widgets::confirm(self.t("btn_calib_restore", cx), self.t("msg_calib_restore_confirm", cx), window, cx, move |_, cx| {
            let mut reqs = Vec::new();
            for (id, offset) in (1u8..).zip(profile.offsets) {
                if let Some(o) = offset {
                    reqs.push(protocol::set_pos_offset(id, o.clamp(-2047, 2047)));
                    reqs.push(Request::sys(cmd::GET_POS_OFFSET, [id]));
                }
            }
            app.update(cx, |a, cx| a.send_paced(reqs, GAP, cx));
        });
    }

    // ------------------------------------------------------------- render

    fn button(&self, key: &'static str, enabled: bool, cx: &mut Context<Self>, on: fn(&mut Self, &mut Window, &mut Context<Self>)) -> Button {
        Button::new(key)
            .small()
            .label(self.t(key, cx))
            .disabled(!enabled)
            .on_click(cx.listener(move |this, _, window, cx| on(this, window, cx)))
    }

    fn table(&self, cx: &App) -> Div {
        let lang = self.app.read(cx).lang;
        let muted = cx.theme().muted_foreground;
        let ranges = self.tracker.ranges();
        let cols = tr_list(lang, "calib_cols");
        let width = [px(70.), px(70.), px(70.), px(70.), px(80.), px(200.)];
        let cell = |i: usize, text: String| div().w(width[i]).text_sm().child(text);
        let num = |v: Option<u16>| v.map_or("-".to_string(), |v| v.to_string());
        let header = h_flex().gap_2().text_color(muted).children(cols.into_iter().enumerate().map(|(i, c)| cell(i, c)));
        let rows = (0..JOINTS).map(|j| {
            let range = ranges.map(|r| r[j]);
            let check = match range.map(calib::check_joint) {
                None => String::new(),
                Some(JointCheck::Ok) => tr(lang, "calib_check_ok"),
                Some(JointCheck::NotSwept) => tr(lang, "calib_check_not_swept"),
                Some(JointCheck::CentreOutside) => tr(lang, "calib_check_centre_outside"),
            };
            let offset = self.offsets[j].map_or("-".to_string(), |o| o.to_string());
            h_flex()
                .gap_2()
                .child(cell(0, format!("{}", j + 1)))
                .child(cell(1, num(self.now[j])))
                .child(cell(2, num(range.map(|r| r.0))))
                .child(cell(3, num(range.map(|r| r.1))))
                .child(cell(4, offset))
                .child(cell(5, check))
        });
        v_flex().gap_1().child(header).children(rows)
    }
}

impl Render for CalibPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let lang = self.app.read(cx).lang;
        let t = |k: &str| tr(lang, k);
        let connected = self.app.read(cx).is_connected();
        let muted = cx.theme().muted_foreground;
        let hint = |k: &str| div().text_sm().text_color(muted).child(t(k));
        let row = || h_flex().gap_3().items_center().flex_wrap();
        let recording = self.recording;
        let have_profile = self.loaded.is_some();
        let have_saved = widgets::selected_text(&self.saved, cx).is_some();

        let release = section(t("grp_calib_step1")).child(hint("calib_hint_release")).child(
            row()
                .child(self.button("btn_calib_torque_off", connected, cx, Self::torque_off))
                .child(self.button("btn_calib_torque_on", connected, cx, Self::torque_on)),
        );
        let centre = section(t("grp_calib_step2")).child(hint("calib_hint_center")).child(
            row()
                .child(self.button("btn_calib_set_center", connected, cx, Self::set_centre))
                .child(self.button("btn_calib_read_offsets", connected, cx, Self::read_offsets)),
        );
        let record_btn = if recording {
            self.button("btn_calib_rec_stop", true, cx, |this, _, cx| this.stop_recording(cx))
        } else {
            self.button("btn_calib_rec_start", connected, cx, Self::start_recording)
        };
        let range = section(t("grp_calib_step3"))
            .child(hint("calib_hint_range"))
            .child(
                row()
                    .child(record_btn)
                    .child(self.button("btn_calib_rec_clear", true, cx, |this, _, cx| this.clear_ranges(cx)))
                    .child(div().text_sm().text_color(muted).child(format!("{} {}", t("calib_samples"), self.tracker.samples()))),
            )
            .child(self.table(cx));
        let profiles = section(t("grp_calib_profiles"))
            .child(
                row()
                    .child(widgets::field(t("lbl_calib_name"), Input::new(&self.name).small(), cx).flex_1().min_w(px(260.)))
                    .child(self.button("btn_calib_save", true, cx, Self::save)),
            )
            .child(
                row()
                    .child(widgets::field(t("lbl_calib_saved"), Select::new(&self.saved).small(), cx).flex_1().min_w(px(260.)))
                    .child(self.button("btn_calib_load", have_saved, cx, Self::load))
                    .child(self.button("btn_calib_restore", have_profile && connected, cx, Self::restore_offsets))
                    .child(self.button("btn_calib_delete", have_saved, cx, Self::delete)),
            );

        v_flex()
            .id("calib-page")
            .size_full()
            .p_3()
            .gap_3()
            .overflow_y_scroll()
            .child(release)
            .child(centre)
            .child(range)
            .child(profiles)
    }
}
