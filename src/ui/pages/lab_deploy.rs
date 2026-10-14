//! Lab / Deploy: run a trained policy on the arm (rollout), evaluate it into an `eval_*` dataset, or
//! coach it (DAgger). A checkpoint trained on another robot type is refused by the engine.

use crate::engine::api::{ModelInfo, SessionInfo};
use crate::i18n::tr;
use crate::ui::engine_state::{EngineNote, EngineState};
use crate::ui::lab_util::call;
use crate::ui::state::AppState;
use crate::ui::widgets::{self, NumState, StrSelect, section};
use gpui_kit::component::button::Button;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputState, NumberInput};
use gpui_kit::component::select::{Select, SelectEvent};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::{Value, json};

/// The deploy modes, in picker order. Each is also the engine session kind.
pub const MODES: [&str; 3] = ["rollout", "eval", "coaching"];

/// Session kinds this page owns.
fn is_deploy_kind(kind: &str) -> bool {
    MODES.contains(&kind)
}

/// Coaching controls enabled in each policy phase: `(control, label key, phases where it applies)`.
pub const COACH_CONTROLS: [(&str, &str, &[&str]); 6] = [
    ("takeover", "lab_coach_takeover", &["autonomous", "paused"]),
    ("handback", "lab_coach_handback", &["correcting", "paused"]),
    ("hold", "lab_coach_hold", &["autonomous"]),
    ("resume", "lab_coach_resume", &["paused"]),
    ("reset", "lab_coach_reset", &["autonomous", "paused"]),
    ("drop_last", "lab_coach_drop", &["correcting"]),
];

pub struct LabDeployPage {
    app: Entity<AppState>,
    engine: Entity<EngineState>,
    models: Vec<ModelInfo>,
    model: Entity<StrSelect>,
    hub_model: Entity<InputState>,
    mode: Entity<StrSelect>,
    task: Entity<InputState>,
    duration: NumState,
    fps: NumState,
    eval_repo: Entity<InputState>,
    eval_episodes: NumState,
    eval_episode_time: NumState,
    eval_reset_time: NumState,
    coach_repo: Entity<InputState>,
    coach_episodes: NumState,
    record_autonomous: bool,
    allow_unverified: bool,
    _subs: Vec<Subscription>,
}

impl LabDeployPage {
    pub fn new(app: Entity<AppState>, engine: Entity<EngineState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let lang = app.read(cx).lang;
        let mode_items = MODES.iter().map(|m| SharedString::from(tr(lang, &format!("lab_dep_mode_{m}")))).collect();
        let mode = widgets::select(mode_items, 0, window, cx);
        let subs = vec![
            cx.observe(&app, |_, _, cx| cx.notify()),
            cx.observe(&engine, |_, _, cx| cx.notify()),
            cx.observe(&mode, |_, _, cx| cx.notify()),
            cx.subscribe_in(&mode, window, |_, _, _: &SelectEvent<_>, _, cx| cx.notify()),
            cx.subscribe_in(&engine, window, |this, _, note: &EngineNote, window, cx| match note {
                EngineNote::Status => this.refresh(window, cx),
                EngineNote::Error(msg) => widgets::notify_error(msg.clone(), window, cx),
                _ => {}
            }),
        ];
        Self {
            model: widgets::select(vec![], 0, window, cx),
            hub_model: widgets::text_input("", "owner/policy_name (Hub)", window, cx),
            task: widgets::text_input("", "Stack the bowls", window, cx),
            duration: widgets::num_input(30., 1., 3600., 1., window, cx),
            fps: widgets::num_input(30., 1., 120., 1., window, cx),
            eval_repo: widgets::text_input("", "owner/eval_run_1", window, cx),
            eval_episodes: widgets::num_input(5., 1., 200., 1., window, cx),
            eval_episode_time: widgets::num_input(30., 1., 600., 1., window, cx),
            eval_reset_time: widgets::num_input(10., 0., 600., 1., window, cx),
            coach_repo: widgets::text_input("", "owner/coaching_data", window, cx),
            coach_episodes: widgets::num_input(10., 1., 500., 1., window, cx),
            record_autonomous: false,
            allow_unverified: false,
            models: Vec::new(),
            mode,
            app,
            engine,
            _subs: subs,
        }
    }

    fn t(&self, key: &str, cx: &App) -> String {
        tr(self.app.read(cx).lang, key)
    }

    fn mode_key(&self, cx: &App) -> &'static str {
        MODES[widgets::selected_index(&self.mode, cx).min(MODES.len() - 1)]
    }

    pub fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.engine.read(cx).is_ready() {
            return;
        }
        call(self.engine.read(cx).client.clone(), window, cx, |c| c.models(), |this, res, window, cx| {
            match res {
                Ok(models) => {
                    let ix = widgets::selected_index(&this.model, cx);
                    let labels = models.iter().map(|m| SharedString::from(m.label())).collect();
                    widgets::set_items(&this.model, labels, ix, window, cx);
                    this.models = models;
                }
                Err(e) => widgets::notify_error(e.to_string(), window, cx),
            }
            cx.notify();
        });
    }

    /// The checkpoint to deploy: a Hub id typed in, else the picked local model.
    fn checkpoint(&self, cx: &App) -> Option<String> {
        let hub = widgets::read_text(&self.hub_model, cx).trim().to_string();
        if !hub.is_empty() {
            return Some(hub);
        }
        self.models.get(widgets::selected_index(&self.model, cx)).map(|m| m.id.clone())
    }

    fn params(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<Value> {
        let Some(checkpoint) = self.checkpoint(cx) else {
            widgets::notify_error(self.t("lab_dep_need_model", cx), window, cx);
            return None;
        };
        let task = widgets::read_text(&self.task, cx).trim().to_string();
        let mut params = json!({
            "checkpoint": checkpoint,
            "task": task,
            "fps": widgets::read_i(&self.fps, cx).max(1),
            "allow_unverified": self.allow_unverified,
        });
        match self.mode_key(cx) {
            "rollout" => params["duration_s"] = json!(widgets::read_num(&self.duration, cx).max(1.0)),
            "eval" => {
                let repo = widgets::read_text(&self.eval_repo, cx).trim().to_string();
                if !repo.split('/').nth(1).is_some_and(|n| n.starts_with("eval_")) {
                    widgets::notify_error(self.t("lab_dep_need_eval_name", cx), window, cx);
                    return None;
                }
                params["repo_id"] = json!(repo);
                params["num_episodes"] = json!(widgets::read_i(&self.eval_episodes, cx).max(1));
                params["episode_time_s"] = json!(widgets::read_num(&self.eval_episode_time, cx).max(1.0));
                params["reset_time_s"] = json!(widgets::read_num(&self.eval_reset_time, cx).max(0.0));
            }
            _ => {
                let repo = widgets::read_text(&self.coach_repo, cx).trim().to_string();
                if !repo.contains('/') || repo.split('/').nth(1).is_some_and(|n| n.starts_with("eval_")) {
                    widgets::notify_error(self.t("lab_coach_need_name", cx), window, cx);
                    return None;
                }
                params["repo_id"] = json!(repo);
                params["num_episodes"] = json!(widgets::read_i(&self.coach_episodes, cx).max(1));
                params["record_autonomous"] = json!(self.record_autonomous);
            }
        }
        Some(params)
    }

    fn setup_section(&self, active: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let (ready, has_profile, has_cameras, has_leader, busy) = {
            let e = self.engine.read(cx);
            let p = e.selected_profile(cx);
            (
                e.is_ready(),
                p.is_some(),
                p.as_ref().is_some_and(|p| !p.cameras.is_empty()),
                p.as_ref().is_some_and(|p| p.leader_port.is_some()),
                e.busy,
            )
        };
        let muted = cx.theme().muted_foreground;
        let mode = self.mode_key(cx);
        let num = |s: &NumState| div().w(px(140.)).child(NumberInput::new(s).small());
        let mut s = section(self.t("lab_dep_setup", cx))
            .child(div().text_sm().text_color(muted).child(self.t("lab_dep_hint", cx)))
            .child(widgets::field(self.t("lab_dep_mode", cx), Select::new(&self.mode).small(), cx))
            .child(widgets::field(self.t("lab_dep_model", cx), Select::new(&self.model).small().placeholder("…"), cx))
            .child(widgets::field(self.t("lab_dep_hub_model", cx), Input::new(&self.hub_model).small(), cx))
            .child(widgets::field(self.t("lab_rec_task", cx), Input::new(&self.task).small(), cx))
            .child(widgets::field("fps", num(&self.fps), cx));
        s = match mode {
            "eval" => s.child(widgets::field(self.t("lab_dep_eval_repo", cx), Input::new(&self.eval_repo).small(), cx)).child(
                h_flex()
                    .gap_3()
                    .flex_wrap()
                    .child(widgets::field(self.t("lab_rec_episodes", cx), num(&self.eval_episodes), cx))
                    .child(widgets::field(self.t("lab_rec_episode_time", cx), num(&self.eval_episode_time), cx))
                    .child(widgets::field(self.t("lab_rec_reset_time", cx), num(&self.eval_reset_time), cx)),
            ),
            "coaching" => s
                .child(div().text_sm().text_color(muted).child(self.t("lab_coach_hint", cx)))
                .child(widgets::field(self.t("lab_coach_repo", cx), Input::new(&self.coach_repo).small(), cx))
                .child(widgets::field(self.t("lab_coach_episodes", cx), num(&self.coach_episodes), cx))
                .child(Checkbox::new("coach-autonomous").label(self.t("lab_coach_record_autonomous", cx)).checked(self.record_autonomous).on_click(
                    cx.listener(|this, checked: &bool, _, cx| {
                        this.record_autonomous = *checked;
                        cx.notify();
                    }),
                )),
            _ => s.child(widgets::field(self.t("lab_dep_duration", cx), num(&self.duration), cx)),
        };
        let needs_leader = mode == "coaching";
        s.child(
            h_flex()
                .gap_3()
                .items_center()
                .flex_wrap()
                .child(Checkbox::new("dep-unverified").label(self.t("lab_dep_unverified", cx)).checked(self.allow_unverified).on_click(
                    cx.listener(|this, checked: &bool, _, cx| {
                        this.allow_unverified = *checked;
                        cx.notify();
                    }),
                ))
                .child(Button::new("dep-refresh").small().outline().label(self.t("refresh", cx)).disabled(!ready).on_click(
                    cx.listener(|this, _, window, cx| this.refresh(window, cx)),
                ))
                .child(
                    Button::new("dep-start")
                        .label(self.t(&format!("lab_dep_start_{mode}"), cx))
                        .loading(busy && !active)
                        .disabled(!(ready && has_profile && has_cameras && (has_leader || !needs_leader)) || active || busy)
                        .on_click(cx.listener(|this, _, window, cx| {
                            if let Some(params) = this.params(window, cx) {
                                let kind = this.mode_key(cx);
                                this.engine.update(cx, |e, cx| e.start_session(kind, params, cx));
                            }
                        })),
                ),
        )
        .child(div().text_sm().text_color(muted).child(self.t("lab_dep_start_hint", cx)))
        .child(div().text_sm().text_color(rgb(0xF44336)).when_some((needs_leader && has_profile && !has_leader).then_some(()), |d, ()| {
            d.child(self.t("lab_coach_need_leader", cx))
        }))
    }

    fn coach_controls(&self, session: &SessionInfo, busy: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let phase = session.status_str("phase").unwrap_or("autonomous").to_string();
        let corrections = session.status_f64("corrections").unwrap_or(0.0) as u32;
        let discarded = session.status_f64("discarded").unwrap_or(0.0) as u32;
        let held = session.status.get("hold").and_then(Value::as_bool).unwrap_or(false);
        let phase_color = match phase.as_str() {
            "correcting" => rgb(0xF44336),
            "paused" => rgb(0xFA8F01),
            _ => rgb(0x4CAF50),
        };
        let buttons = COACH_CONTROLS.iter().map(|(control, key, phases)| {
            let enabled = phases.contains(&phase.as_str()) && !busy;
            let control: &'static str = control;
            Button::new(format!("coach-{control}"))
                .small()
                .outline()
                .label(self.t(key, cx))
                .disabled(!enabled)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.engine.update(cx, |e, cx| e.control(control, json!({}), cx))
                }))
        });
        v_flex()
            .gap_2()
            .child(div().text_lg().text_color(phase_color).child(format!(
                "{}{}",
                self.t(&format!("lab_coach_phase_{phase}"), cx),
                if held { format!(" · {}", self.t("lab_coach_held", cx)) } else { String::new() }
            )))
            .child(div().text_sm().text_color(cx.theme().muted_foreground).child(format!(
                "{}: {corrections} · {}: {discarded}",
                self.t("lab_coach_corrections", cx),
                self.t("lab_coach_discarded", cx)
            )))
            .child(h_flex().gap_2().flex_wrap().children(buttons).child(
                Button::new("coach-finish").small().label(self.t("lab_coach_finish", cx)).disabled(busy).on_click(
                    cx.listener(|this, _, _, cx| this.engine.update(cx, |e, cx| e.control("finish", json!({}), cx))),
                ),
            ))
    }

    fn live_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let session = self.engine.read(cx).session.clone().filter(|s| is_deploy_kind(&s.kind) || s.kind == "coaching");
        let muted = cx.theme().muted_foreground;
        let Some(session) = session else {
            return section(self.t("lab_dep_live", cx)).child(div().text_sm().text_color(muted).child(self.t("lab_dep_idle", cx)));
        };
        let busy = self.engine.read(cx).busy;
        let eval = session.kind == "eval";
        let control = |id: &'static str, key: &str, action: &'static str, cx: &mut Context<Self>| {
            Button::new(id).small().outline().label(self.t(key, cx)).disabled(busy).on_click(cx.listener(move |this, _, _, cx| {
                this.engine.update(cx, |e, cx| e.control(action, json!({}), cx))
            }))
        };
        let mut row = h_flex().gap_2().flex_wrap();
        if eval {
            row = row
                .child(control("dep-exit", "lab_rec_exit_early", "exit_early", cx))
                .child(control("dep-redo", "lab_rec_rerecord", "rerecord", cx))
                .child(control("dep-finish", "lab_rec_finish", "finish", cx));
        }
        let mut body = section(self.t("lab_dep_live", cx))
            .child(div().text_lg().child(format!("{} · {}", session.kind, session.state)))
            .child(div().text_sm().text_color(muted).child(format!(
                "{} · {:.0}s",
                session.status_str("checkpoint").unwrap_or(""),
                session.status_f64("elapsed_s").unwrap_or(0.0)
            )));
        if session.kind == "coaching" {
            body = body.child(self.coach_controls(&session, busy, cx));
        }
        body.child(row.child(
            Button::new("dep-stop").small().label(self.t("lab_dep_stop", cx)).disabled(busy).on_click(
                cx.listener(|this, _, _, cx| this.engine.update(cx, |e, cx| e.stop_session(cx))),
            ),
        ))
    }
}

impl Render for LabDeployPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.engine.read(cx).session.as_ref().is_some_and(|s| is_deploy_kind(&s.kind));
        let mut page = v_flex().id("lab-deploy-page").size_full().p_3().gap_3().overflow_y_scroll();
        if !self.engine.read(cx).is_ready() {
            page = page.child(div().text_color(cx.theme().muted_foreground).child(self.t("lab_need_engine", cx)));
        }
        page.child(self.setup_section(active, cx)).child(self.live_section(cx))
    }
}

#[cfg(test)]
mod tests {
    use super::COACH_CONTROLS;

    /// Every phase offers at least one way forward, and no control is enabled in a phase the engine refuses.
    #[test]
    fn coaching_controls_match_the_engine_plans() {
        let allowed = |control: &str, phase: &str| {
            COACH_CONTROLS.iter().any(|(c, _, phases)| *c == control && phases.contains(&phase))
        };
        // the same table as lerobot.nexarm_service.coaching._PLANS plus reset
        assert!(allowed("takeover", "autonomous") && allowed("takeover", "paused") && !allowed("takeover", "correcting"));
        assert!(allowed("handback", "correcting") && allowed("handback", "paused") && !allowed("handback", "autonomous"));
        assert!(allowed("hold", "autonomous") && !allowed("hold", "paused"));
        assert!(allowed("resume", "paused") && !allowed("resume", "autonomous"));
        assert!(allowed("reset", "paused") && allowed("reset", "autonomous") && !allowed("reset", "correcting"));
        assert!(allowed("drop_last", "correcting") && !allowed("drop_last", "paused"));
        for phase in ["autonomous", "paused", "correcting"] {
            assert!(COACH_CONTROLS.iter().any(|(_, _, p)| p.contains(&phase)), "{phase}");
        }
    }
}
