//! Lab / Session: connect to the engine, edit the robot profile, check the arm calibration and run a
//! teleoperation session. Every other Lab page needs a ready engine and a selected profile from here.

use crate::engine::api::{CameraProfile, RobotProfile};
use crate::engine::events::JOINT_NAMES;
use crate::i18n::tr;
use crate::ui::engine_state::{EngineNote, EngineState, EngineStatus, teleop_params};
use crate::ui::state::AppState;
use crate::ui::widgets::{self, NumState, StrSelect, section};
use gpui_kit::component::button::Button;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, NumberInput};
use gpui_kit::component::select::{Select, SelectEvent};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::*;

struct CameraRow {
    name: Entity<gpui_kit::component::input::InputState>,
    source: Entity<gpui_kit::component::input::InputState>,
    width: NumState,
    height: NumState,
    fps: NumState,
}

pub struct LabSessionPage {
    app: Entity<AppState>,
    engine: Entity<EngineState>,
    url: Entity<gpui_kit::component::input::InputState>,
    command: Entity<gpui_kit::component::input::InputState>,
    workdir: Entity<gpui_kit::component::input::InputState>,
    auto_start: bool,
    profile_pick: Entity<StrSelect>,
    name: Entity<gpui_kit::component::input::InputState>,
    follower_port: Entity<gpui_kit::component::input::InputState>,
    leader_port: Entity<gpui_kit::component::input::InputState>,
    baud: NumState,
    cameras: Vec<CameraRow>,
    _subs: Vec<Subscription>,
}

impl LabSessionPage {
    pub fn new(app: Entity<AppState>, engine: Entity<EngineState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = app.read(cx).settings.engine.clone();
        let url = widgets::text_input(&settings.url, "http://127.0.0.1:8765", window, cx);
        let command = widgets::text_input(&settings.command, "lerobot-nexarm-service", window, cx);
        let workdir = widgets::text_input(&settings.working_dir, "", window, cx);
        let profile_pick = widgets::select(vec![], 0, window, cx);
        let name = widgets::text_input("", "bench", window, cx);
        let follower_port = widgets::text_input("", "COM19", window, cx);
        let leader_port = widgets::text_input("", "COM18", window, cx);
        let baud = widgets::num_input(1_000_000., 9600., 2_000_000., 1., window, cx);
        let subs = vec![
            cx.observe(&engine, |_, _, cx| cx.notify()),
            cx.subscribe_in(&engine, window, |this, _, note: &EngineNote, window, cx| match note {
                EngineNote::Profiles => this.reload_profiles(window, cx),
                EngineNote::Error(msg) => widgets::notify_error(msg.clone(), window, cx),
                _ => {}
            }),
            cx.subscribe_in(&profile_pick, window, |this, _, ev: &SelectEvent<_>, window, cx| {
                let SelectEvent::Confirm(Some(v)) = ev else { return };
                let name = v.to_string();
                this.engine.update(cx, |e, cx| e.select_profile(&name, cx));
                this.load_profile(window, cx);
            }),
            cx.observe(&app, |_, _, cx| cx.notify()),
        ];
        let mut page = Self {
            app,
            engine,
            url,
            command,
            workdir,
            auto_start: settings.auto_start,
            profile_pick,
            name,
            follower_port,
            leader_port,
            baud,
            cameras: Vec::new(),
            _subs: subs,
        };
        page.reload_profiles(window, cx);
        page
    }

    fn t(&self, key: &str, cx: &App) -> String {
        tr(self.app.read(cx).lang, key)
    }

    /// Fill the profile picker from the engine, then the form from the selected profile.
    fn reload_profiles(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (names, selected) = {
            let e = self.engine.read(cx);
            let names: Vec<SharedString> = e.profiles.iter().map(|p| SharedString::from(p.name.clone())).collect();
            let selected = e.selected_profile(cx).and_then(|p| e.profiles.iter().position(|x| x.name == p.name)).unwrap_or(0);
            (names, selected)
        };
        widgets::set_items(&self.profile_pick, names, selected, window, cx);
        self.load_profile(window, cx);
    }

    fn load_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.engine.read(cx).selected_profile(cx) else { return };
        widgets::set_text(&self.name, p.name.clone(), window, cx);
        widgets::set_text(&self.follower_port, p.follower_port.clone(), window, cx);
        widgets::set_text(&self.leader_port, p.leader_port.clone().unwrap_or_default(), window, cx);
        widgets::set_num(&self.baud, p.baudrate as f64, window, cx);
        self.cameras = p.cameras.iter().map(|(name, cam)| Self::camera_row(name, cam, window, cx)).collect();
        cx.notify();
    }

    fn camera_row(name: &str, cam: &CameraProfile, window: &mut Window, cx: &mut App) -> CameraRow {
        CameraRow {
            name: widgets::text_input(name, "front", window, cx),
            source: widgets::text_input(&cam.source, "0", window, cx),
            width: widgets::num_input(cam.width as f64, 16., 4096., 1., window, cx),
            height: widgets::num_input(cam.height as f64, 16., 4096., 1., window, cx),
            fps: widgets::num_input(cam.fps as f64, 1., 240., 1., window, cx),
        }
    }

    /// The profile described by the form.
    fn form_profile(&self, cx: &App) -> RobotProfile {
        let leader = widgets::read_text(&self.leader_port, cx).trim().to_string();
        RobotProfile {
            name: widgets::read_text(&self.name, cx).trim().to_string(),
            follower_port: widgets::read_text(&self.follower_port, cx).trim().to_string(),
            leader_port: (!leader.is_empty()).then_some(leader),
            baudrate: widgets::read_i(&self.baud, cx).max(1) as u32,
            cameras: self
                .cameras
                .iter()
                .map(|r| {
                    (
                        widgets::read_text(&r.name, cx).trim().to_string(),
                        CameraProfile {
                            kind: "opencv".into(),
                            source: widgets::read_text(&r.source, cx).trim().to_string(),
                            width: widgets::read_i(&r.width, cx).max(1) as u32,
                            height: widgets::read_i(&r.height, cx).max(1) as u32,
                            fps: widgets::read_i(&r.fps, cx).max(1) as u32,
                        },
                    )
                })
                .filter(|(name, _)| !name.is_empty())
                .collect(),
        }
    }

    fn save_engine_settings(&mut self, cx: &mut Context<Self>) {
        let (url, command, workdir) = (
            widgets::read_text(&self.url, cx).trim().to_string(),
            widgets::read_text(&self.command, cx).trim().to_string(),
            widgets::read_text(&self.workdir, cx).trim().to_string(),
        );
        let auto = self.auto_start;
        self.app.update(cx, |a, _| {
            a.settings.engine.url = url;
            a.settings.engine.command = command;
            a.settings.engine.working_dir = workdir;
            a.settings.engine.auto_start = auto;
            a.persist();
        });
    }

    fn engine_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (status_text, ready, owns, starting, mismatch) = {
            let lang = self.app.read(cx).lang;
            let e = self.engine.read(cx);
            (
                e.status_text(|k| tr(lang, k)),
                e.is_ready(),
                e.owns_engine(),
                matches!(e.status, EngineStatus::Starting),
                matches!(e.status, EngineStatus::Mismatch(_)),
            )
        };
        let dot = if ready {
            rgb(0x4CAF50)
        } else if starting {
            rgb(0xFA8F01)
        } else {
            rgb(0xF44336)
        };
        let _ = mismatch;
        section(self.t("lab_engine", cx))
            .child(h_flex().gap_2().items_center().child(div().text_color(dot).child("●")).child(status_text))
            .child(widgets::field(self.t("lab_engine_url", cx), Input::new(&self.url).small(), cx))
            .child(widgets::field(self.t("lab_engine_command", cx), Input::new(&self.command).small(), cx))
            .child(widgets::field(self.t("lab_engine_workdir", cx), Input::new(&self.workdir).small(), cx))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .flex_wrap()
                    .child(Checkbox::new("engine-auto").label(self.t("lab_engine_auto", cx)).checked(self.auto_start).on_click(
                        cx.listener(|this, checked: &bool, _, cx| {
                            this.auto_start = *checked;
                            cx.notify();
                        }),
                    ))
                    .child(
                        Button::new("engine-connect")
                            .small()
                            .label(self.t("lab_engine_connect", cx))
                            .disabled(starting)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.save_engine_settings(cx);
                                this.engine.update(cx, |e, cx| e.connect_engine(cx));
                            })),
                    )
                    .child(
                        Button::new("engine-stop")
                            .small()
                            .outline()
                            .label(self.t("lab_engine_stop", cx))
                            .disabled(!owns)
                            .on_click(cx.listener(|this, _, _, cx| this.engine.update(cx, |e, cx| e.stop_engine(cx)))),
                    )
                    .child(
                        Button::new("engine-3d")
                            .small()
                            .outline()
                            .label(self.t("btn_open_3d", cx))
                            .on_click(cx.listener(|this, _, _, cx| this.app.update(cx, |a, cx| a.open_3d_viewer(cx)))),
                    ),
            )
            .child(div().text_sm().text_color(cx.theme().muted_foreground).child(self.t("lab_engine_hint", cx)))
    }

    fn profile_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ready = self.engine.read(cx).is_ready();
        let ports = self
            .engine
            .read(cx)
            .ports
            .iter()
            .map(|p| if p.description.is_empty() { p.device.clone() } else { format!("{} ({})", p.device, p.description) })
            .collect::<Vec<_>>()
            .join(", ");
        let muted = cx.theme().muted_foreground;
        let rows = self.cameras.iter().enumerate().map(|(i, r)| {
            h_flex()
                .gap_2()
                .items_center()
                .child(div().w(px(110.)).child(Input::new(&r.name).small()))
                .child(div().w(px(90.)).child(Input::new(&r.source).small()))
                .child(div().w(px(100.)).child(NumberInput::new(&r.width).small()))
                .child(div().w(px(100.)).child(NumberInput::new(&r.height).small()))
                .child(div().w(px(90.)).child(NumberInput::new(&r.fps).small()))
                .child(Button::new(("cam-del", i)).small().outline().label("✕").on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.cameras.remove(i);
                        cx.notify();
                    },
                )))
        });
        let header = h_flex()
            .gap_2()
            .text_sm()
            .text_color(muted)
            .child(div().w(px(110.)).child(self.t("lab_cam_name", cx)))
            .child(div().w(px(90.)).child(self.t("lab_cam_source", cx)))
            .child(div().w(px(100.)).child(self.t("lab_cam_width", cx)))
            .child(div().w(px(100.)).child(self.t("lab_cam_height", cx)))
            .child(div().w(px(90.)).child("fps"));
        section(self.t("lab_profile", cx))
            .child(widgets::field(self.t("lab_profile_pick", cx), Select::new(&self.profile_pick).small().placeholder("…"), cx))
            .child(widgets::field(self.t("lab_profile_name", cx), Input::new(&self.name).small(), cx))
            .child(widgets::field(self.t("lab_follower_port", cx), Input::new(&self.follower_port).small(), cx))
            .child(widgets::field(self.t("lab_leader_port", cx), Input::new(&self.leader_port).small(), cx))
            .child(widgets::field(self.t("lbl_baud_rate", cx), NumberInput::new(&self.baud).small(), cx))
            .child(div().text_sm().text_color(muted).child(format!("{}: {ports}", self.t("lab_detected_ports", cx))))
            .child(header)
            .children(rows)
            .child(
                h_flex()
                    .gap_2()
                    .child(Button::new("cam-add").small().outline().label(self.t("lab_cam_add", cx)).on_click(cx.listener(
                        |this, _, window, cx| {
                            let n = this.cameras.len();
                            let name = if n == 0 { "front".to_string() } else { format!("cam{n}") };
                            this.cameras.push(Self::camera_row(&name, &CameraProfile::default(), window, cx));
                            cx.notify();
                        },
                    )))
                    .child(
                        Button::new("profile-save")
                            .small()
                            .label(self.t("lab_profile_save", cx))
                            .disabled(!ready)
                            .on_click(cx.listener(|this, _, _, cx| {
                                let p = this.form_profile(cx);
                                this.engine.update(cx, |e, cx| e.save_profile(p, cx));
                            })),
                    )
                    .child(
                        Button::new("profile-delete")
                            .small()
                            .outline()
                            .label(self.t("btn_delete", cx))
                            .disabled(!ready)
                            .on_click(cx.listener(|this, _, window, cx| {
                                let name = widgets::read_text(&this.name, cx).trim().to_string();
                                let engine = this.engine.clone();
                                widgets::confirm(
                                    this.t("lab_profile_delete", cx),
                                    this.t("lab_profile_delete_msg", cx).replace("{name}", &name),
                                    window,
                                    cx,
                                    move |_, cx| engine.update(cx, |e, cx| e.delete_profile(name.clone(), cx)),
                                );
                            })),
                    ),
            )
    }

    fn calibration_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (ready, idle, report) = {
            let e = self.engine.read(cx);
            (e.is_ready(), e.session.is_none() && !e.busy, e.calibration.clone())
        };
        let muted = cx.theme().muted_foreground;
        let mut rows = Vec::new();
        if let Some(r) = &report {
            rows.push(div().text_sm().text_color(muted).child(r.hint.clone()));
            for j in &r.joints {
                let color = if j.ok { rgb(0x4CAF50) } else { rgb(0xF44336) };
                rows.push(
                    div().text_sm().child(format!(
                        "{:<14} leader {:>5}  follower {:>5}  Δ {:+5}  {}",
                        j.joint,
                        j.leader_mapped,
                        j.follower,
                        j.delta,
                        if j.ok { "✓" } else { "✗" }
                    ))
                    .text_color(color),
                );
            }
        }
        section(self.t("lab_calibration", cx))
            .child(div().text_sm().text_color(muted).child(self.t("lab_calibration_hint", cx)))
            .child(
                Button::new("calibration-check")
                    .small()
                    .label(self.t("lab_calibration_check", cx))
                    .disabled(!(ready && idle))
                    .on_click(cx.listener(|this, _, _, cx| this.engine.update(cx, |e, cx| e.check_calibration(cx)))),
            )
            .children(rows)
    }

    fn session_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (ready, session, busy, ticks, has_profile) = {
            let e = self.engine.read(cx);
            (e.is_ready(), e.session.clone(), e.busy, e.last_ticks, e.selected_profile(cx).is_some())
        };
        let active = session.is_some();
        let mut body = v_flex().gap_1();
        if let Some(s) = &session {
            body = body.child(div().text_sm().child(format!("{} · {} · {}", s.kind, s.state, s.profile)));
            if let Some(e) = &s.error {
                body = body.child(div().text_sm().text_color(rgb(0xF44336)).child(e.clone()));
            }
        }
        if let (Some(t), true) = (ticks, active) {
            let line = JOINT_NAMES.iter().zip(t).map(|(n, v)| format!("{n} {v:.0}")).collect::<Vec<_>>().join("   ");
            body = body.child(div().text_sm().text_color(cx.theme().muted_foreground).child(line));
        }
        section(self.t("lab_session", cx))
            .child(div().text_sm().text_color(cx.theme().muted_foreground).child(self.t("lab_session_hint", cx)))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("teleop-start")
                            .small()
                            .label(self.t("lab_teleop_start", cx))
                            .loading(busy && !active)
                            .disabled(!(ready && has_profile) || active || busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.engine.update(cx, |e, cx| e.start_session("teleoperation", teleop_params(), cx))
                            })),
                    )
                    .child(
                        Button::new("teleop-stop")
                            .small()
                            .outline()
                            .label(self.t("lab_session_stop", cx))
                            .disabled(!active || busy)
                            .on_click(cx.listener(|this, _, _, cx| this.engine.update(cx, |e, cx| e.stop_session(cx)))),
                    ),
            )
            .child(body)
    }
}

impl Render for LabSessionPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("lab-session-page")
            .size_full()
            .p_3()
            .gap_3()
            .overflow_y_scroll()
            .child(self.engine_section(cx))
            .child(self.session_section(cx))
            .child(self.profile_section(cx))
            .child(self.calibration_section(cx))
    }
}
