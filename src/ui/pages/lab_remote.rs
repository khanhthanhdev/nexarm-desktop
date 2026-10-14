//! Lab / Remote: drive a follower on another machine with the leader on this one.
//!
//! * **Station** (where the follower is): optionally lets this app's engine listen beyond loopback with a
//!   token, then hosts the follower. The arm stays parked at rest until an operator engages.
//! * **Operator** (where the leader is): connects to a station and streams the leader to it.

use crate::engine::supervisor::is_loopback_host;
use crate::i18n::tr;
use crate::ui::engine_state::{EngineNote, EngineState};
use crate::ui::state::AppState;
use crate::ui::widgets::{self, NumState, section};
use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Input, InputState, NumberInput};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::*;
use serde_json::json;

pub struct LabRemotePage {
    app: Entity<AppState>,
    engine: Entity<EngineState>,
    bind_host: Entity<InputState>,
    token: Entity<InputState>,
    rate_cap: NumState,
    station: Entity<InputState>,
    station_token: Entity<InputState>,
    _subs: Vec<Subscription>,
}

fn station_label(addr: &str) -> String {
    format!("station:{}", addr.trim())
}

/// `host:port` an operator should enter for a station bound to `bind_host`.
pub fn station_address(bind_host: &str, url: &str) -> String {
    let port = crate::engine::supervisor::url_port(url).unwrap_or(8765);
    match bind_host.trim() {
        "" | "0.0.0.0" | "::" => format!("<this machine's address>:{port}"),
        h => format!("{h}:{port}"),
    }
}

impl LabRemotePage {
    pub fn new(app: Entity<AppState>, engine: Entity<EngineState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (bind_host, token, remote_station) = {
            let s = &app.read(cx).settings;
            (s.engine.bind_host.clone(), s.engine.token.clone(), s.remote_station.clone())
        };
        let station_token = crate::settings::load_secret(&station_label(&remote_station)).unwrap_or_default();
        let subs = vec![
            cx.observe(&app, |_, _, cx| cx.notify()),
            cx.observe(&engine, |_, _, cx| cx.notify()),
            cx.subscribe_in(&engine, window, |_, _, note: &EngineNote, window, cx| {
                if let EngineNote::Error(msg) = note {
                    widgets::notify_error(msg.clone(), window, cx);
                }
            }),
        ];
        Self {
            bind_host: widgets::text_input(&bind_host, "127.0.0.1", window, cx),
            token: widgets::text_input(&token, "", window, cx),
            rate_cap: widgets::num_input(1500., 100., 6000., 100., window, cx),
            station: widgets::text_input(&remote_station, "100.64.0.7:8765", window, cx),
            station_token: cx.new(|cx| {
                InputState::new(window, cx).masked(true).placeholder("station token").default_value(station_token)
            }),
            app,
            engine,
            _subs: subs,
        }
    }

    fn t(&self, key: &str, cx: &App) -> String {
        tr(self.app.read(cx).lang, key)
    }

    // ------------------------------------------------------------ station
    /// Save the bind address and token, then restart the engine with them.
    fn apply_access(&mut self, local_only: bool, window: &mut Window, cx: &mut Context<Self>) {
        let mut host = if local_only { "127.0.0.1".to_string() } else { widgets::read_text(&self.bind_host, cx).trim().to_string() };
        if host.is_empty() {
            host = "127.0.0.1".into();
        }
        let mut token = widgets::read_text(&self.token, cx).trim().to_string();
        if !is_loopback_host(&host) && token.is_empty() {
            token = crate::engine::supervisor::generate_token();
            widgets::set_text(&self.token, token.clone(), window, cx);
        }
        widgets::set_text(&self.bind_host, host.clone(), window, cx);
        self.app.update(cx, |a, _| {
            a.settings.engine.bind_host = host;
            a.settings.engine.token = token;
            a.persist();
        });
        if !self.engine.read(cx).owns_engine() {
            widgets::notify_info(self.t("lab_remote_not_owned", cx), window, cx);
        }
        self.engine.update(cx, |e, cx| e.restart_engine(cx));
    }

    fn start_hosting(&mut self, cx: &mut Context<Self>) {
        let cap = widgets::read_num(&self.rate_cap, cx).max(100.0);
        self.engine.update(cx, |e, cx| e.start_session("hosting", json!({ "fps": 30, "rate_cap_ticks_s": cap }), cx));
    }

    fn access_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let (host, url) = {
            let s = &self.app.read(cx).settings.engine;
            (s.bind_host.clone(), s.url.clone())
        };
        let remote = !is_loopback_host(if host.is_empty() { "127.0.0.1" } else { &host });
        section(self.t("lab_remote_access", cx))
            .child(div().text_sm().text_color(muted).child(self.t("lab_remote_access_hint", cx)))
            .child(widgets::field(self.t("lab_remote_bind", cx), Input::new(&self.bind_host).small(), cx))
            .child(widgets::field(self.t("lab_remote_token", cx), Input::new(&self.token).small(), cx))
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(Button::new("remote-apply").small().label(self.t("lab_remote_apply", cx)).on_click(cx.listener(
                        |this, _, window, cx| this.apply_access(false, window, cx),
                    )))
                    .child(Button::new("remote-local").small().outline().label(self.t("lab_remote_local_only", cx)).on_click(
                        cx.listener(|this, _, window, cx| this.apply_access(true, window, cx)),
                    )),
            )
            .child(div().text_sm().child(if remote {
                format!("{}: {}", self.t("lab_remote_share", cx), station_address(&host, &url))
            } else {
                self.t("lab_remote_local_now", cx)
            }))
    }

    fn station_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (ready, has_profile, session, busy) = {
            let e = self.engine.read(cx);
            (e.is_ready(), e.selected_profile(cx).is_some(), e.session.clone().filter(|s| s.kind == "hosting"), e.busy)
        };
        let muted = cx.theme().muted_foreground;
        let control = |id: &'static str, key: &str, action: &'static str, cx: &mut Context<Self>| {
            Button::new(id).small().outline().label(self.t(key, cx)).disabled(busy).on_click(cx.listener(move |this, _, _, cx| {
                this.engine.update(cx, |e, cx| e.control(action, json!({}), cx))
            }))
        };
        let mut s = section(self.t("lab_remote_station", cx))
            .child(div().text_sm().text_color(muted).child(self.t("lab_remote_station_hint", cx)))
            .child(widgets::field(self.t("lab_remote_rate_cap", cx), div().w(px(140.)).child(NumberInput::new(&self.rate_cap).small()), cx));
        match session {
            None => {
                s = s.child(
                    Button::new("host-start")
                        .label(self.t("lab_remote_host_start", cx))
                        .loading(busy)
                        .disabled(!(ready && has_profile) || busy || self.engine.read(cx).session.is_some())
                        .on_click(cx.listener(|this, _, _, cx| this.start_hosting(cx))),
                );
            }
            Some(info) => {
                let phase = info.status_str("phase").unwrap_or("parked").to_string();
                let operator = info.status.get("operator").and_then(|o| o.as_str()).map(str::to_string);
                let age = info.status_f64("frame_age_ms");
                let color = match phase.as_str() {
                    "engaged" => rgb(0x4CAF50),
                    "parked" => rgb(0x9E9E9E),
                    _ => rgb(0xFA8F01),
                };
                s = s
                    .child(div().text_lg().text_color(color).child(self.t(&format!("lab_remote_phase_{phase}"), cx)))
                    .child(div().text_sm().text_color(muted).child(format!(
                        "{}: {}{}",
                        self.t("lab_remote_operator", cx),
                        operator.unwrap_or_else(|| "—".into()),
                        age.map(|a| format!(" · {a:.0} ms")).unwrap_or_default()
                    )))
                    .child(
                        h_flex()
                            .gap_2()
                            .flex_wrap()
                            .child(control("host-park", "lab_remote_park", "park", cx))
                            .child(control("host-block", "lab_remote_block", "block", cx))
                            .child(control("host-allow", "lab_remote_allow", "allow", cx))
                            .child(Button::new("host-stop").small().label(self.t("lab_remote_host_stop", cx)).disabled(busy).on_click(
                                cx.listener(|this, _, _, cx| this.engine.update(cx, |e, cx| e.stop_session(cx))),
                            )),
                    );
            }
        }
        s
    }

    // ----------------------------------------------------------- operator
    fn start_operator(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let station = widgets::read_text(&self.station, cx).trim().to_string();
        let token = widgets::read_text(&self.station_token, cx).trim().to_string();
        if !station.contains(':') {
            return widgets::notify_error(self.t("lab_remote_need_station", cx), window, cx);
        }
        self.app.update(cx, |a, _| {
            a.settings.remote_station = station.clone();
            a.persist();
        });
        if let Err(e) = crate::settings::save_secret(&station_label(&station), &token) {
            self.app.update(cx, |a, cx| a.log_text(format!("Keychain: {e}"), cx));
        }
        self.engine.update(cx, |e, cx| {
            e.start_session("remote_teleoperation", json!({ "station": station, "token": token, "fps": 30 }), cx)
        });
    }

    fn operator_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (ready, has_leader, session, busy, any_session) = {
            let e = self.engine.read(cx);
            (
                e.is_ready(),
                e.selected_profile(cx).is_some_and(|p| p.leader_port.is_some()),
                e.session.clone().filter(|s| s.kind == "remote_teleoperation"),
                e.busy,
                e.session.is_some(),
            )
        };
        let muted = cx.theme().muted_foreground;
        let mut s = section(self.t("lab_remote_operator_section", cx))
            .child(div().text_sm().text_color(muted).child(self.t("lab_remote_operator_hint", cx)))
            .child(widgets::field(self.t("lab_remote_station_addr", cx), Input::new(&self.station).small(), cx))
            .child(widgets::field(self.t("lab_remote_token", cx), Input::new(&self.station_token).small(), cx));
        match session {
            None => {
                s = s.child(
                    Button::new("operator-start")
                        .label(self.t("lab_remote_operator_start", cx))
                        .loading(busy)
                        .disabled(!(ready && has_leader) || busy || any_session)
                        .on_click(cx.listener(|this, _, window, cx| this.start_operator(window, cx))),
                );
                if ready && !has_leader {
                    s = s.child(div().text_sm().text_color(rgb(0xF44336)).child(self.t("lab_coach_need_leader", cx)));
                }
            }
            Some(info) => {
                let phase = info.status_str("station_phase").unwrap_or("connecting").to_string();
                let rtt = info.status_f64("rtt_ms");
                s = s
                    .child(div().text_lg().child(self.t(&format!("lab_remote_phase_{phase}"), cx)))
                    .child(div().text_sm().text_color(muted).child(format!(
                        "RTT {} · {} {}",
                        rtt.map(|r| format!("{r:.0} ms")).unwrap_or_else(|| "—".into()),
                        info.status_f64("frames_sent").unwrap_or(0.0) as u64,
                        self.t("lab_remote_frames", cx)
                    )))
                    .child(Button::new("operator-stop").small().label(self.t("lab_remote_operator_stop", cx)).disabled(busy).on_click(
                        cx.listener(|this, _, _, cx| this.engine.update(cx, |e, cx| e.stop_session(cx))),
                    ));
            }
        }
        s
    }
}

impl Render for LabRemotePage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut page = v_flex().id("lab-remote-page").size_full().p_3().gap_3().overflow_y_scroll();
        if !self.engine.read(cx).is_ready() {
            page = page.child(div().text_color(cx.theme().muted_foreground).child(self.t("lab_need_engine", cx)));
        }
        page.child(self.operator_section(cx)).child(self.station_section(cx)).child(self.access_section(cx))
    }
}

#[cfg(test)]
mod tests {
    use super::station_address;

    #[test]
    fn the_shared_address_names_the_bind_host_or_asks_for_one() {
        assert_eq!(station_address("100.64.0.7", "http://127.0.0.1:8765"), "100.64.0.7:8765");
        assert_eq!(station_address("192.168.1.5", "http://127.0.0.1:9000"), "192.168.1.5:9000");
        assert!(station_address("0.0.0.0", "http://127.0.0.1:8765").contains("this machine"));
    }
}
