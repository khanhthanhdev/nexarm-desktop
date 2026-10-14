//! Lab / Train: queue training jobs, follow them live (loss and learning-rate curves, logs) and pick
//! the checkpoints they produce. The job runs wherever the runner says; today that is this machine.

use crate::engine::api::{Checkpoint, Job, MetricPoint, TrainRequest};
use crate::engine::events::EngineEvent;
use crate::i18n::tr;
use crate::ui::chart::{ChartSpec, Series, line_chart};
use crate::ui::engine_state::{EngineNote, EngineState};
use crate::ui::lab_util::{call, short_count};
use crate::ui::state::AppState;
use crate::ui::widgets::{self, NumState, StrSelect, section};
use gpui_kit::component::button::Button;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputState, NumberInput};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::select::Select;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::BTreeMap;

pub struct LabTrainPage {
    app: Entity<AppState>,
    engine: Entity<EngineState>,
    dataset: Entity<StrSelect>,
    dataset_names: Vec<String>,
    policy: Entity<InputState>,
    steps: NumState,
    batch: NumState,
    log_freq: NumState,
    save_freq: NumState,
    job_name: Entity<InputState>,
    extra: Entity<InputState>,
    use_excluded: bool,
    jobs: Vec<Job>,
    selected: Option<String>,
    metrics: Vec<MetricPoint>,
    logs: Vec<String>,
    checkpoints: Vec<Checkpoint>,
    _subs: Vec<Subscription>,
}

/// Space- or newline-separated `key=value` items into extra `lerobot-train` flags; items without `=` are ignored.
pub fn parse_extra_args(text: &str) -> BTreeMap<String, String> {
    text.split_whitespace()
        .filter_map(|l| {
            let l = l.trim().trim_start_matches("--");
            let (k, v) = l.split_once('=')?;
            let (k, v) = (k.trim(), v.trim());
            (!k.is_empty()).then(|| (k.to_string(), v.to_string()))
        })
        .collect()
}

impl LabTrainPage {
    pub fn new(app: Entity<AppState>, engine: Entity<EngineState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subs = vec![
            cx.observe(&app, |_, _, cx| cx.notify()),
            cx.observe(&engine, |_, _, cx| cx.notify()),
            cx.subscribe_in(&engine, window, |this, _, note: &EngineNote, window, cx| match note {
                EngineNote::Status => this.refresh(window, cx),
                EngineNote::Error(msg) => widgets::notify_error(msg.clone(), window, cx),
                EngineNote::Event(EngineEvent::JobsChanged { kind, .. }) if kind == "train" => this.refresh(window, cx),
                EngineNote::Event(EngineEvent::JobProgress { id, .. }) if this.selected.as_deref() == Some(id.as_str()) => {
                    this.load_selected(window, cx)
                }
                _ => {}
            }),
        ];
        Self {
            dataset: widgets::select(vec![], 0, window, cx),
            dataset_names: Vec::new(),
            policy: widgets::text_input("act", "act", window, cx),
            steps: widgets::num_input(20000., 1., 10_000_000., 100., window, cx),
            batch: widgets::num_input(8., 1., 4096., 1., window, cx),
            log_freq: widgets::num_input(200., 1., 100_000., 10., window, cx),
            save_freq: widgets::num_input(5000., 0., 10_000_000., 100., window, cx),
            job_name: widgets::text_input("", "", window, cx),
            extra: widgets::text_input("", "policy.chunk_size=50 optimizer.lr=1e-4", window, cx),
            use_excluded: true,
            app,
            engine,
            jobs: Vec::new(),
            selected: None,
            metrics: Vec::new(),
            logs: Vec::new(),
            checkpoints: Vec::new(),
            _subs: subs,
        }
    }

    fn t(&self, key: &str, cx: &App) -> String {
        tr(self.app.read(cx).lang, key)
    }

    fn client(&self, cx: &App) -> crate::engine::client::EngineClient {
        self.engine.read(cx).client.clone()
    }

    fn ready(&self, cx: &App) -> bool {
        self.engine.read(cx).is_ready()
    }

    fn fail(&self, err: impl std::fmt::Display, window: &mut Window, cx: &mut Context<Self>) {
        widgets::notify_error(err.to_string(), window, cx);
    }

    pub fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.ready(cx) {
            return;
        }
        call(
            self.client(cx),
            window,
            cx,
            |c| (c.jobs(), c.datasets()),
            |this, (jobs, datasets), window, cx| {
                match jobs {
                    Ok(j) => this.jobs = j,
                    Err(e) => this.fail(e, window, cx),
                }
                if let Ok(d) = datasets {
                    let names: Vec<String> = d.into_iter().map(|d| d.repo_id).collect();
                    if names != this.dataset_names {
                        let current = widgets::selected_text(&this.dataset, cx);
                        let ix = current.and_then(|c| names.iter().position(|n| *n == c)).unwrap_or(0);
                        widgets::set_items(&this.dataset, names.iter().cloned().map(Into::into).collect(), ix, window, cx);
                        this.dataset_names = names;
                    }
                }
                if this.selected.is_none() {
                    this.selected = this.jobs.first().map(|j| j.id.clone());
                }
                this.load_selected(window, cx);
                cx.notify();
            },
        );
    }

    fn load_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected.clone() else { return };
        let for_id = id.clone();
        call(
            self.client(cx),
            window,
            cx,
            move |c| (c.job_metrics(&id), c.job_logs(&id, 120), c.job_checkpoints(&id), c.job(&id)),
            move |this, (metrics, logs, checkpoints, job), _, cx| {
                if this.selected.as_deref() != Some(for_id.as_str()) {
                    return;
                }
                this.metrics = metrics.unwrap_or_default();
                this.logs = logs.unwrap_or_default();
                this.checkpoints = checkpoints.unwrap_or_default();
                if let Ok(job) = job
                    && let Some(slot) = this.jobs.iter_mut().find(|j| j.id == job.id)
                {
                    *slot = job;
                }
                cx.notify();
            },
        );
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dataset) = widgets::selected_text(&self.dataset, cx) else {
            return widgets::notify_error(self.t("lab_train_need_dataset", cx), window, cx);
        };
        let policy = widgets::read_text(&self.policy, cx).trim().to_string();
        if policy.is_empty() {
            return widgets::notify_error(self.t("lab_train_need_policy", cx), window, cx);
        }
        let name = widgets::read_text(&self.job_name, cx).trim().to_string();
        let save = widgets::read_i(&self.save_freq, cx);
        let request = TrainRequest {
            dataset_repo_id: dataset,
            policy_type: policy,
            steps: widgets::read_i(&self.steps, cx).max(1) as u32,
            batch_size: widgets::read_i(&self.batch, cx).max(1) as u32,
            log_freq: widgets::read_i(&self.log_freq, cx).max(1) as u32,
            save_freq: (save > 0).then_some(save as u32),
            job_name: (!name.is_empty()).then_some(name),
            use_excluded: self.use_excluded,
            extra_args: parse_extra_args(&widgets::read_text(&self.extra, cx)),
            runner: "local".into(),
            server: None,
        };
        call(
            self.client(cx),
            window,
            cx,
            move |c| c.submit_job(&request),
            |this, res, window, cx| {
                match res {
                    Ok(job) => {
                        this.selected = Some(job.id.clone());
                        this.jobs.insert(0, job);
                        this.metrics.clear();
                        this.logs.clear();
                        this.checkpoints.clear();
                    }
                    Err(e) => this.fail(e, window, cx),
                }
                cx.notify();
            },
        );
    }

    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected.clone() else { return };
        call(self.client(cx), window, cx, move |c| c.cancel_job(&id), |this, res, window, cx| {
            if let Err(e) = res {
                this.fail(e, window, cx);
            }
            this.refresh(window, cx);
        });
    }

    // ---------------------------------------------------------------- render
    fn form_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ready = self.ready(cx);
        let num = |s: &NumState| div().w(px(150.)).child(NumberInput::new(s).small());
        section(self.t("lab_train_new", cx))
            .child(div().text_sm().text_color(cx.theme().muted_foreground).child(self.t("lab_train_hint", cx)))
            .child(widgets::field(self.t("lab_train_dataset", cx), Select::new(&self.dataset).small().placeholder("…"), cx))
            .child(widgets::field(self.t("lab_train_policy", cx), Input::new(&self.policy).small(), cx))
            .child(
                h_flex()
                    .gap_3()
                    .flex_wrap()
                    .child(widgets::field(self.t("lab_train_steps", cx), num(&self.steps), cx))
                    .child(widgets::field(self.t("lab_train_batch", cx), num(&self.batch), cx))
                    .child(widgets::field(self.t("lab_train_log_freq", cx), num(&self.log_freq), cx))
                    .child(widgets::field(self.t("lab_train_save_freq", cx), num(&self.save_freq), cx)),
            )
            .child(widgets::field(self.t("lab_train_job_name", cx), Input::new(&self.job_name).small(), cx))
            .child(widgets::field(self.t("lab_train_extra", cx), Input::new(&self.extra).small(), cx))
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(Checkbox::new("train-excluded").label(self.t("lab_train_use_excluded", cx)).checked(self.use_excluded).on_click(
                        cx.listener(|this, checked: &bool, _, cx| {
                            this.use_excluded = *checked;
                            cx.notify();
                        }),
                    ))
                    .child(Button::new("train-submit").label(self.t("lab_train_submit", cx)).disabled(!ready).on_click(
                        cx.listener(|this, _, window, cx| this.submit(window, cx)),
                    )),
            )
    }

    fn queue_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let rows = self.jobs.iter().enumerate().map(|(i, j)| {
            let id = j.id.clone();
            let color = match j.state.as_str() {
                "done" => rgb(0x4CAF50),
                "failed" => rgb(0xF44336),
                "running" => rgb(0xFA8F01),
                _ => rgb(0x9E9E9E),
            };
            let shown = self.selected.as_deref() == Some(j.id.as_str());
            div()
                .id(("job", i))
                .px_2()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .when(shown, |d| d.bg(cx.theme().secondary))
                .child(
                    h_flex()
                        .gap_3()
                        .items_center()
                        .child(div().w(px(90.)).text_color(color).child(j.state.clone()))
                        .child(div().flex_1().child(format!(
                            "{} · {} · {}/{} steps{}",
                            j.request.policy_type,
                            j.request.dataset_repo_id,
                            short_count(j.step as u64),
                            short_count(j.total_steps as u64),
                            j.loss.map(|l| format!(" · loss {l:.3}")).unwrap_or_default()
                        ))),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.selected = Some(id.clone());
                    this.metrics.clear();
                    this.logs.clear();
                    this.checkpoints.clear();
                    this.load_selected(window, cx);
                    cx.notify();
                }))
        });
        let active = self.jobs.iter().find(|j| Some(&j.id) == self.selected.as_ref()).is_some_and(Job::is_active);
        section(self.t("lab_train_queue", cx))
            .child(div().text_sm().text_color(muted).child(self.t("lab_train_queue_hint", cx)))
            .children(rows)
            .child(
                Button::new("train-cancel")
                    .small()
                    .outline()
                    .label(self.t("lab_train_cancel", cx))
                    .disabled(!active)
                    .on_click(cx.listener(|this, _, window, cx| this.cancel(window, cx))),
            )
    }

    fn detail_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let Some(job) = self.jobs.iter().find(|j| Some(&j.id) == self.selected.as_ref()) else {
            return section(self.t("lab_train_detail", cx)).child(div().text_sm().text_color(muted).child(self.t("lab_train_pick", cx)));
        };
        let curve = |key: &str| -> Vec<(f32, f32)> {
            self.metrics.iter().filter_map(|m| m.values.get(key).map(|v| (m.step as f32, *v as f32))).collect()
        };
        let loss = ChartSpec {
            series: vec![Series { label: "loss".into(), color: 0xFA8F01, points: curve("loss") }],
            ..Default::default()
        };
        let lr = ChartSpec {
            series: vec![Series { label: "lr".into(), color: 0x2196F3, points: curve("lr") }],
            log_y: true,
            ..Default::default()
        };
        let mut body = section(format!("{} · {}", self.t("lab_train_detail", cx), job.id))
            .child(Progress::new("train-progress").value(job.progress() * 100.0))
            .child(div().text_sm().text_color(muted).child(format!(
                "{} · {}/{} · loss {} · lr {}",
                job.state,
                job.step,
                job.total_steps,
                job.loss.map(|l| format!("{l:.4}")).unwrap_or_else(|| "—".into()),
                job.lr.map(|l| format!("{l:.2e}")).unwrap_or_else(|| "—".into()),
            )));
        if let Some(err) = &job.error {
            body = body.child(div().text_sm().text_color(rgb(0xF44336)).child(err.clone()));
        }
        body.child(div().text_sm().child(self.t("lab_train_loss", cx)))
            .child(div().h(px(160.)).w_full().child(line_chart(loss, 0x3A3A3A, 0xFFFFFF)))
            .child(div().text_sm().child(self.t("lab_train_lr", cx)))
            .child(div().h(px(120.)).w_full().child(line_chart(lr, 0x3A3A3A, 0xFFFFFF)))
            .child(div().text_sm().child(self.t("lab_train_checkpoints", cx)))
            .children(self.checkpoints.iter().map(|c| {
                div().text_sm().text_color(muted).child(format!("{}  {}", c.step.map(|s| format!("step {s}")).unwrap_or_default(), c.path))
            }))
            .child(div().text_sm().child(self.t("lab_train_log", cx)))
            .child(
                v_flex()
                    .id("train-log")
                    .max_h(px(220.))
                    .overflow_y_scroll()
                    .p_2()
                    .rounded_md()
                    .bg(cx.theme().secondary)
                    .children(self.logs.iter().map(|l| div().text_xs().child(l.clone()))),
            )
    }
}

impl Render for LabTrainPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut page = v_flex().id("lab-train-page").size_full().p_3().gap_3().overflow_y_scroll();
        if !self.ready(cx) {
            page = page.child(div().text_color(cx.theme().muted_foreground).child(self.t("lab_need_engine", cx)));
        }
        page.child(self.form_section(cx)).child(self.queue_section(cx)).child(self.detail_section(cx))
    }
}

#[cfg(test)]
mod tests {
    use super::parse_extra_args;

    #[test]
    fn extra_args_parse_key_value_lines() {
        let m = parse_extra_args("policy.chunk_size=50 --optimizer.lr=1e-4\nnonsense =x");
        assert_eq!(m.len(), 2);
        assert_eq!(m["policy.chunk_size"], "50");
        assert_eq!(m["optimizer.lr"], "1e-4");
    }
}
