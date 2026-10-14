//! Lab / Record: record LeRobot episodes by teleoperating, with live controls. Datasets recorded here
//! have the same schema as `lerobot-record` output, so the existing training scripts accept them.

use crate::i18n::tr;
use crate::ui::engine_state::{EngineNote, EngineState};
use crate::ui::state::AppState;
use crate::ui::widgets::{self, NumState, section};
use gpui_kit::component::button::Button;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputState, NumberInput};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::*;
use serde_json::json;

pub struct LabRecordPage {
    app: Entity<AppState>,
    engine: Entity<EngineState>,
    repo_id: Entity<InputState>,
    task: Entity<InputState>,
    new_task: Entity<InputState>,
    episodes: NumState,
    episode_time: NumState,
    reset_time: NumState,
    fps: NumState,
    resume: bool,
    _subs: Vec<Subscription>,
}

impl LabRecordPage {
    pub fn new(app: Entity<AppState>, engine: Entity<EngineState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
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
            repo_id: widgets::text_input("", "owner/dataset_name", window, cx),
            task: widgets::text_input("", "Stack the bowls", window, cx),
            new_task: widgets::text_input("", "", window, cx),
            episodes: widgets::num_input(50., 1., 1000., 1., window, cx),
            episode_time: widgets::num_input(20., 1., 600., 1., window, cx),
            reset_time: widgets::num_input(10., 0., 600., 1., window, cx),
            fps: widgets::num_input(30., 1., 120., 1., window, cx),
            resume: false,
            app,
            engine,
            _subs: subs,
        }
    }

    fn t(&self, key: &str, cx: &App) -> String {
        tr(self.app.read(cx).lang, key)
    }

    /// Session params from the form; `None` (after telling the user) when something is missing.
    fn params(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<serde_json::Value> {
        let repo = widgets::read_text(&self.repo_id, cx).trim().to_string();
        let task = widgets::read_text(&self.task, cx).trim().to_string();
        if !repo.contains('/') || task.is_empty() {
            widgets::notify_error(self.t("lab_rec_need_fields", cx), window, cx);
            return None;
        }
        Some(json!({
            "repo_id": repo,
            "task": task,
            "num_episodes": widgets::read_i(&self.episodes, cx).max(1),
            "episode_time_s": widgets::read_num(&self.episode_time, cx).max(1.0),
            "reset_time_s": widgets::read_num(&self.reset_time, cx).max(0.0),
            "fps": widgets::read_i(&self.fps, cx).max(1),
            "resume": self.resume,
        }))
    }

    fn control(&self, action: &'static str, cx: &mut Context<Self>) {
        self.engine.update(cx, |e, cx| e.control(action, json!({}), cx));
    }

    fn form_section(&self, recording: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let (ready, has_profile, has_cameras, busy) = {
            let e = self.engine.read(cx);
            let profile = e.selected_profile(cx);
            (e.is_ready(), profile.is_some(), profile.is_some_and(|p| !p.cameras.is_empty()), e.busy)
        };
        let muted = cx.theme().muted_foreground;
        let num = |s: &NumState| div().w(px(140.)).child(NumberInput::new(s).small());
        let mut s = section(self.t("lab_rec_setup", cx))
            .child(div().text_sm().text_color(muted).child(self.t("lab_rec_hint", cx)))
            .child(widgets::field(self.t("lab_rec_repo", cx), Input::new(&self.repo_id).small(), cx))
            .child(widgets::field(self.t("lab_rec_task", cx), Input::new(&self.task).small(), cx))
            .child(
                h_flex()
                    .gap_3()
                    .flex_wrap()
                    .child(widgets::field(self.t("lab_rec_episodes", cx), num(&self.episodes), cx))
                    .child(widgets::field(self.t("lab_rec_episode_time", cx), num(&self.episode_time), cx))
                    .child(widgets::field(self.t("lab_rec_reset_time", cx), num(&self.reset_time), cx))
                    .child(widgets::field("fps", num(&self.fps), cx)),
            )
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(Checkbox::new("rec-resume").label(self.t("lab_rec_resume", cx)).checked(self.resume).on_click(
                        cx.listener(|this, checked: &bool, _, cx| {
                            this.resume = *checked;
                            cx.notify();
                        }),
                    ))
                    .child(
                        Button::new("rec-start")
                            .label(self.t("lab_rec_start", cx))
                            .disabled(!(ready && has_profile && has_cameras) || recording || busy)
                            .on_click(cx.listener(|this, _, window, cx| {
                                if let Some(params) = this.params(window, cx) {
                                    this.engine.update(cx, |e, cx| e.start_session("recording", params, cx));
                                }
                            })),
                    ),
            );
        if ready && has_profile && !has_cameras {
            s = s.child(div().text_sm().text_color(rgb(0xF44336)).child(self.t("lab_rec_need_cameras", cx)));
        }
        if !ready {
            s = s.child(div().text_sm().text_color(rgb(0xF44336)).child(self.t("lab_need_engine", cx)));
        }
        s
    }

    fn live_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(session) = self.engine.read(cx).session.clone().filter(|s| s.kind == "recording") else {
            return section(self.t("lab_rec_live", cx)).child(
                div().text_sm().text_color(cx.theme().muted_foreground).child(self.t("lab_rec_idle", cx)),
            );
        };
        let busy = self.engine.read(cx).busy;
        let phase = session.status_str("phase").unwrap_or("preparing").to_string();
        let (elapsed, duration) = (session.status_f64("elapsed_s").unwrap_or(0.0), session.status_f64("duration_s").unwrap_or(1.0));
        let recorded = session.status_f64("episodes_recorded").unwrap_or(0.0) as u32;
        let target = session.status_f64("episodes_target").unwrap_or(0.0) as u32;
        let frames = session.status_f64("frames").unwrap_or(0.0) as u32;
        let paused = phase == "paused";
        let recording = matches!(phase.as_str(), "recording" | "paused");
        let progress = if recording { (elapsed / duration.max(0.1)).clamp(0.0, 1.0) as f32 * 100.0 } else { 0.0 };
        let btn = |id: &'static str, key: &str, action: &'static str, enabled: bool, cx: &mut Context<Self>| {
            Button::new(id).small().outline().label(self.t(key, cx)).disabled(!enabled || busy).on_click(
                cx.listener(move |this, _, _, cx| this.control(action, cx)),
            )
        };
        section(self.t("lab_rec_live", cx))
            .child(div().text_lg().child(format!(
                "{} · {} {}/{} · {} {}",
                self.t(&format!("lab_rec_phase_{phase}"), cx),
                self.t("lab_rec_episode", cx),
                recorded + u32::from(recording),
                target,
                frames,
                self.t("lab_rec_frames", cx)
            )))
            .child(Progress::new("rec-progress").value(progress))
            .child(div().text_sm().text_color(cx.theme().muted_foreground).child(format!(
                "{:.0}s / {:.0}s · {}: {}",
                elapsed,
                duration,
                self.t("lab_rec_task", cx),
                session.status_str("task").unwrap_or("")
            )))
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(if paused {
                        btn("rec-resume-btn", "lab_rec_resume_btn", "resume", recording, cx)
                    } else {
                        btn("rec-pause", "lab_rec_pause", "pause", recording, cx)
                    })
                    .child(btn("rec-exit", "lab_rec_exit_early", "exit_early", recording, cx))
                    .child(btn("rec-redo", "lab_rec_rerecord", "rerecord", recording, cx))
                    .child(btn("rec-finish", "lab_rec_finish", "finish", recording, cx))
                    .child(
                        Button::new("rec-stop")
                            .small()
                            .label(self.t("lab_rec_stop", cx))
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| this.engine.update(cx, |e, cx| e.stop_session(cx)))),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().flex_1().child(Input::new(&self.new_task).small()))
                    .child(Button::new("rec-set-task").small().outline().label(self.t("lab_rec_set_task", cx)).on_click(
                        cx.listener(|this, _, _, cx| {
                            let task = widgets::read_text(&this.new_task, cx).trim().to_string();
                            if !task.is_empty() {
                                this.engine.update(cx, |e, cx| e.control("set_task", json!({ "task": task }), cx));
                            }
                        }),
                    )),
            )
    }
}

impl Render for LabRecordPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let recording = self.engine.read(cx).session.as_ref().is_some_and(|s| s.kind == "recording");
        v_flex()
            .id("lab-record-page")
            .size_full()
            .p_3()
            .gap_3()
            .overflow_y_scroll()
            .child(self.form_section(recording, cx))
            .child(self.live_section(cx))
    }
}
