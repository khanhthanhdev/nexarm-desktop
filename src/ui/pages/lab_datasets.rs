//! Lab / Datasets: browse datasets, view episodes (video frame plus joint curves), curate by excluding
//! episodes (nothing is deleted), edit into new datasets, merge, and move to and from the Hub.

use crate::engine::api::{DatasetSummary, EpisodeSummary, JointsData, Operation};
use crate::engine::events::{EngineEvent, JOINT_NAMES};
use crate::i18n::tr;
use crate::ui::chart::{ChartSpec, SERIES_COLORS, Series, line_chart};
use crate::ui::engine_state::{EngineNote, EngineState};
use crate::ui::lab_util::{call, clock, decode_jpeg, release_image, short_count};
use crate::ui::state::AppState;
use crate::ui::widgets::{self, NumState, StrSelect, section};
use gpui_kit::component::button::Button;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputState, NumberInput};
use gpui_kit::component::select::Select;
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState, SliderValue};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

struct EpisodeView {
    episode: u32,
    joints: JointsData,
    frame: u32,
    /// Latest frame requested; a fetch in flight is repeated when this moved on.
    wanted: u32,
    fetching: bool,
}

pub struct LabDatasetsPage {
    app: Entity<AppState>,
    engine: Entity<EngineState>,
    datasets: Vec<DatasetSummary>,
    merge_selection: BTreeSet<String>,
    current: Option<String>,
    features: Vec<String>,
    episodes: Vec<EpisodeSummary>,
    edit_selection: BTreeSet<u32>,
    view: Option<EpisodeView>,
    image: Option<Arc<RenderImage>>,
    camera: Entity<StrSelect>,
    frame_slider: Entity<SliderState>,
    playing: Option<Task<()>>,
    output: Entity<InputState>,
    split_fraction: NumState,
    hub_repo: Entity<InputState>,
    private: bool,
    remove_feature: Entity<StrSelect>,
    operations: Vec<Operation>,
    _subs: Vec<Subscription>,
}

impl LabDatasetsPage {
    pub fn new(app: Entity<AppState>, engine: Entity<EngineState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let camera = widgets::select(vec![], 0, window, cx);
        let remove_feature = widgets::select(vec![], 0, window, cx);
        let frame_slider = cx.new(|_| SliderState::new().min(0.).max(1.).step(1.).default_value(0.));
        let subs = vec![
            cx.observe(&app, |_, _, cx| cx.notify()),
            cx.observe(&engine, |_, _, cx| cx.notify()),
            cx.subscribe_in(&engine, window, |this, _, note: &EngineNote, window, cx| match note {
                EngineNote::Status | EngineNote::Profiles => this.refresh(window, cx),
                EngineNote::Error(msg) => widgets::notify_error(msg.clone(), window, cx),
                EngineNote::Event(EngineEvent::JobsChanged { kind, .. }) if kind == "operation" => this.refresh(window, cx),
                _ => {}
            }),
            cx.subscribe_in(&frame_slider, window, |this, _, ev: &SliderEvent, window, cx| {
                if let SliderEvent::Change(SliderValue::Single(v)) = ev {
                    this.seek(v.round() as u32, window, cx);
                }
            }),
            cx.subscribe_in(&camera, window, |this, _, _: &gpui_kit::component::select::SelectEvent<_>, window, cx| {
                this.fetch_frame(window, cx)
            }),
        ];
        Self {
            output: widgets::text_input("", "owner/new_name", window, cx),
            split_fraction: widgets::num_input(0.8, 0.05, 0.95, 0.05, window, cx),
            hub_repo: widgets::text_input("", "owner/name", window, cx),
            app,
            engine,
            datasets: Vec::new(),
            merge_selection: BTreeSet::new(),
            current: None,
            features: Vec::new(),
            episodes: Vec::new(),
            edit_selection: BTreeSet::new(),
            view: None,
            image: None,
            camera,
            frame_slider,
            playing: None,
            private: true,
            remove_feature,
            operations: Vec::new(),
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

    // ---------------------------------------------------------------- loading
    pub fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.ready(cx) {
            return;
        }
        call(
            self.client(cx),
            window,
            cx,
            |c| (c.datasets(), c.operations()),
            |this, (datasets, operations), window, cx| {
                match datasets {
                    Ok(d) => this.datasets = d,
                    Err(e) => this.fail(e, window, cx),
                }
                if let Ok(o) = operations {
                    this.operations = o.into_iter().take(8).collect();
                }
                this.merge_selection.retain(|r| this.datasets.iter().any(|d| &d.repo_id == r));
                if let Some(cur) = this.current.clone() {
                    if this.datasets.iter().any(|d| d.repo_id == cur) {
                        this.reload_episodes(window, cx);
                    } else {
                        this.current = None;
                        this.episodes.clear();
                        this.view = None;
                    }
                }
                cx.notify();
            },
        );
    }

    fn open_dataset(&mut self, repo_id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.playing = None;
        self.current = Some(repo_id.clone());
        self.view = None;
        self.edit_selection.clear();
        release_image(self.image.take(), window, cx);
        call(
            self.client(cx),
            window,
            cx,
            move |c| c.dataset_info(&repo_id),
            |this, res, window, cx| {
                match res {
                    Ok(v) => {
                        let features: Vec<String> =
                            v["info"]["features"].as_object().map(|m| m.keys().cloned().collect()).unwrap_or_default();
                        let cams: Vec<SharedString> = v["summary"]["cameras"]
                            .as_array()
                            .map(|a| a.iter().filter_map(|s| s.as_str().map(|s| s.to_string().into())).collect())
                            .unwrap_or_default();
                        widgets::set_items(&this.camera, cams, 0, window, cx);
                        widgets::set_items(&this.remove_feature, features.iter().cloned().map(Into::into).collect(), 0, window, cx);
                        this.features = features;
                    }
                    Err(e) => this.fail(e, window, cx),
                }
                this.reload_episodes(window, cx);
            },
        );
    }

    fn reload_episodes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.current.clone() else { return };
        let first_load = self.episodes.is_empty();
        call(
            self.client(cx),
            window,
            cx,
            move |c| c.episodes(&repo),
            move |this, res, window, cx| {
                match res {
                    Ok(eps) => {
                        this.episodes = eps;
                        if first_load
                            && this.view.is_none()
                            && let Some(first) = this.episodes.first().map(|e| e.episode_index)
                        {
                            this.open_episode(first, window, cx);
                        }
                    }
                    Err(e) => this.fail(e, window, cx),
                }
                cx.notify();
            },
        );
    }

    fn open_episode(&mut self, episode: u32, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.current.clone() else { return };
        self.playing = None;
        call(
            self.client(cx),
            window,
            cx,
            move |c| c.joints(&repo, episode),
            move |this, res, window, cx| {
                match res {
                    Ok(joints) => {
                        let n = joints.timestamps.len().max(1);
                        this.frame_slider.update(cx, |s, _| *s = SliderState::new().min(0.).max((n - 1) as f32).step(1.).default_value(0.));
                        this.view = Some(EpisodeView { episode, joints, frame: 0, wanted: 0, fetching: false });
                        this.fetch_frame(window, cx);
                    }
                    Err(e) => this.fail(e, window, cx),
                }
                cx.notify();
            },
        );
    }

    fn seek(&mut self, frame: u32, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = &mut self.view else { return };
        let max = view.joints.timestamps.len().saturating_sub(1) as u32;
        view.wanted = frame.min(max);
        self.fetch_frame(window, cx);
        cx.notify();
    }

    /// Fetch the wanted frame; if the target moves while a fetch is in flight it fetches again.
    fn fetch_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(repo), Some(camera)) = (self.current.clone(), widgets::selected_text(&self.camera, cx)) else { return };
        let Some(view) = &mut self.view else { return };
        if view.fetching {
            return;
        }
        view.fetching = true;
        let (episode, frame) = (view.episode, view.wanted);
        call(
            self.client(cx),
            window,
            cx,
            move |c| c.frame_jpeg(&repo, episode, &camera, frame, 640),
            move |this, res, window, cx| {
                let Some(view) = &mut this.view else { return };
                view.fetching = false;
                match res.map_err(|e| e.to_string()).and_then(|b| decode_jpeg(&b).map_err(|e| e.to_string())) {
                    Ok(img) => {
                        view.frame = frame;
                        release_image(this.image.replace(img), window, cx);
                    }
                    Err(e) => this.fail(e, window, cx),
                }
                let again = this.view.as_ref().is_some_and(|v| v.wanted != v.frame && v.episode == episode);
                if again {
                    this.fetch_frame(window, cx);
                }
                cx.notify();
            },
        );
    }

    fn toggle_play(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.playing.take().is_some() {
            return;
        }
        let Some(fps) = self.current.as_ref().and_then(|r| self.datasets.iter().find(|d| &d.repo_id == r)).map(|d| d.fps.max(1)) else {
            return;
        };
        let period = Duration::from_millis(1000 / fps as u64);
        self.playing = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(period).await;
                let keep = this
                    .update_in(cx, |p, window, cx| {
                        let Some(v) = &p.view else { return false };
                        let next = v.wanted + 1;
                        if next as usize >= v.joints.timestamps.len() {
                            return false;
                        }
                        p.frame_slider.update(cx, |s, cx| s.set_value(next as f32, window, cx));
                        p.seek(next, window, cx);
                        true
                    })
                    .unwrap_or(false);
                if !keep {
                    let _ = this.update(cx, |p, cx| {
                        p.playing = None;
                        cx.notify();
                    });
                    return;
                }
            }
        }));
        cx.notify();
    }

    // --------------------------------------------------------------- curation
    fn toggle_excluded(&mut self, episode: u32, excluded: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.current.clone() else { return };
        if let Some(e) = self.episodes.iter_mut().find(|e| e.episode_index == episode) {
            e.excluded = excluded;
        }
        let list: Vec<u32> = self.episodes.iter().filter(|e| e.excluded).map(|e| e.episode_index).collect();
        call(
            self.client(cx),
            window,
            cx,
            move |c| c.set_excluded(&repo, &list),
            |this, res, window, cx| {
                if let Err(e) = res {
                    this.fail(e, window, cx);
                    this.reload_episodes(window, cx);
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    // ------------------------------------------------------------------ edits
    fn output_name(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<String> {
        let name = widgets::read_text(&self.output, cx).trim().to_string();
        if name.is_empty() {
            widgets::notify_error(self.t("lab_ds_need_output", cx), window, cx);
            return None;
        }
        Some(name)
    }

    fn start_operation(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        work: impl FnOnce(crate::engine::client::EngineClient) -> Result<Operation, crate::engine::client::ClientError> + Send + 'static,
    ) {
        call(self.client(cx), window, cx, work, |this, res, window, cx| {
            match res {
                Ok(op) => {
                    this.operations.insert(0, op);
                    widgets::notify_info(this.t("lab_ds_op_started", cx), window, cx);
                }
                Err(e) => this.fail(e, window, cx),
            }
            cx.notify();
        });
    }

    fn delete_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(repo), Some(out)) = (self.current.clone(), self.output_name(window, cx)) else { return };
        let episodes: Vec<u32> = self.edit_selection.iter().copied().collect();
        if episodes.is_empty() {
            return widgets::notify_error(self.t("lab_ds_need_selection", cx), window, cx);
        }
        self.start_operation(window, cx, move |c| c.delete_episodes(&repo, &episodes, &out));
    }

    fn drop_excluded(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(repo), Some(out)) = (self.current.clone(), self.output_name(window, cx)) else { return };
        self.start_operation(window, cx, move |c| c.drop_excluded(&repo, &out));
    }

    fn split(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.current.clone() else { return };
        let train = widgets::read_num(&self.split_fraction, cx).clamp(0.05, 0.95);
        let splits = serde_json::json!({ "train": train, "val": 1.0 - train });
        self.start_operation(window, cx, move |c| c.split(&repo, splits));
    }

    fn remove_selected_feature(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(repo), Some(out)) = (self.current.clone(), self.output_name(window, cx)) else { return };
        let Some(feature) = widgets::selected_text(&self.remove_feature, cx) else { return };
        self.start_operation(window, cx, move |c| c.remove_features(&repo, &out, &[feature]));
    }

    fn rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(repo), Some(out)) = (self.current.clone(), self.output_name(window, cx)) else { return };
        call(
            self.client(cx),
            window,
            cx,
            move |c| c.rename_dataset(&repo, &out).map(|_| out),
            |this, res, window, cx| match res {
                Ok(new_name) => {
                    this.current = Some(new_name);
                    this.refresh(window, cx);
                }
                Err(e) => this.fail(e, window, cx),
            },
        );
    }

    fn upload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.current.clone() else { return };
        let private = self.private;
        self.start_operation(window, cx, move |c| c.upload_dataset(&repo, private));
    }

    fn merge(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(out) = self.output_name(window, cx) else { return };
        let repos: Vec<String> = self.merge_selection.iter().cloned().collect();
        if repos.len() < 2 {
            return widgets::notify_error(self.t("lab_ds_need_two", cx), window, cx);
        }
        self.start_operation(window, cx, move |c| c.merge_datasets(&repos, &out));
    }

    fn import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let repo = widgets::read_text(&self.hub_repo, cx).trim().to_string();
        if repo.is_empty() {
            return;
        }
        self.start_operation(window, cx, move |c| c.import_dataset(&repo));
    }

    // ---------------------------------------------------------------- render
    fn datasets_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let ready = self.ready(cx);
        let rows = self.datasets.iter().enumerate().map(|(i, d)| {
            let repo = d.repo_id.clone();
            let open = repo.clone();
            let pick = repo.clone();
            let selected = self.current.as_deref() == Some(d.repo_id.as_str());
            h_flex()
                .gap_2()
                .items_center()
                .child(Checkbox::new(("ds-merge", i)).checked(self.merge_selection.contains(&repo)).on_click(cx.listener(
                    move |this, checked: &bool, _, cx| {
                        if *checked {
                            this.merge_selection.insert(pick.clone());
                        } else {
                            this.merge_selection.remove(&pick);
                        }
                        cx.notify();
                    },
                )))
                .child(
                    div()
                        .id(("ds-open", i))
                        .flex_1()
                        .cursor_pointer()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .when(selected, |d| d.bg(cx.theme().secondary))
                        .child(format!(
                            "{}   ·   {} ep · {} fr · {} fps{}",
                            d.repo_id,
                            d.total_episodes,
                            short_count(d.total_frames),
                            d.fps,
                            if d.excluded_episodes > 0 { format!(" · {} excluded", d.excluded_episodes) } else { String::new() }
                        ))
                        .on_click(cx.listener(move |this, _, window, cx| this.open_dataset(open.clone(), window, cx))),
                )
        });
        section(self.t("lab_ds_datasets", cx))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .flex_wrap()
                    .child(Button::new("ds-refresh").small().outline().label(self.t("refresh", cx)).disabled(!ready).on_click(
                        cx.listener(|this, _, window, cx| this.refresh(window, cx)),
                    ))
                    .child(div().w(px(220.)).child(Input::new(&self.hub_repo).small()))
                    .child(Button::new("ds-import").small().outline().label(self.t("lab_ds_import", cx)).disabled(!ready).on_click(
                        cx.listener(|this, _, window, cx| this.import(window, cx)),
                    ))
                    .child(Button::new("ds-merge").small().label(self.t("lab_ds_merge", cx)).disabled(!ready).on_click(
                        cx.listener(|this, _, window, cx| this.merge(window, cx)),
                    )),
            )
            .child(div().text_sm().text_color(muted).child(self.t("lab_ds_merge_hint", cx)))
            .children(rows)
    }

    fn viewer_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let Some(view) = &self.view else {
            return section(self.t("lab_ds_viewer", cx)).child(div().text_sm().text_color(muted).child(self.t("lab_ds_pick", cx)));
        };
        let fps = self
            .current
            .as_ref()
            .and_then(|r| self.datasets.iter().find(|d| &d.repo_id == r))
            .map(|d| d.fps.max(1) as f64)
            .unwrap_or(30.0);
        let total = view.joints.timestamps.len();
        let now = view.joints.timestamps.get(view.frame as usize).copied().unwrap_or(0.0);
        let series: Vec<Series> = JOINT_NAMES
            .iter()
            .enumerate()
            .map(|(j, name)| Series {
                label: name.to_string(),
                color: SERIES_COLORS[j],
                points: view
                    .joints
                    .timestamps
                    .iter()
                    .zip(&view.joints.action)
                    .filter_map(|(t, row)| row.get(j).map(|v| (*t as f32, *v as f32)))
                    .collect(),
            })
            .collect();
        let legend = h_flex().gap_3().flex_wrap().children(JOINT_NAMES.iter().enumerate().map(|(j, name)| {
            let value = view.joints.action.get(view.frame as usize).and_then(|r| r.get(j)).copied().unwrap_or(0.0);
            div().text_sm().text_color(rgb(SERIES_COLORS[j])).child(format!("{name} {value:.0}"))
        }));
        let chart = ChartSpec { series, marker_x: Some(now as f32), ..Default::default() };
        let image: AnyElement = match &self.image {
            Some(img) => img_element(img.clone()),
            None => div().text_sm().text_color(muted).child("…").into_any_element(),
        };
        section(format!("{} · {} {}", self.t("lab_ds_viewer", cx), self.t("lab_ds_episode", cx), view.episode))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().w(px(260.)).child(Select::new(&self.camera).small()))
                    .child(Button::new("ds-play").small().label(if self.playing.is_some() { "⏸" } else { "▶" }).on_click(
                        cx.listener(|this, _, window, cx| this.toggle_play(window, cx)),
                    ))
                    .child(div().text_sm().text_color(muted).child(format!(
                        "{} / {}   {} / {}",
                        view.frame,
                        total.saturating_sub(1),
                        clock(now),
                        clock(total as f64 / fps)
                    ))),
            )
            .child(div().h(px(300.)).w_full().flex().justify_center().child(image))
            .child(Slider::new(&self.frame_slider))
            .child(legend)
            .child(div().h(px(180.)).w_full().child(line_chart(chart, 0x3A3A3A, 0xFFFFFF)))
    }

    fn episodes_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let rows = self.episodes.iter().enumerate().map(|(i, e)| {
            let ep = e.episode_index;
            let shown = self.view.as_ref().map(|v| v.episode) == Some(ep);
            h_flex()
                .gap_2()
                .items_center()
                .child(Checkbox::new(("ep-pick", i)).checked(self.edit_selection.contains(&ep)).on_click(cx.listener(
                    move |this, checked: &bool, _, cx| {
                        if *checked {
                            this.edit_selection.insert(ep);
                        } else {
                            this.edit_selection.remove(&ep);
                        }
                        cx.notify();
                    },
                )))
                .child(
                    div()
                        .id(("ep-open", i))
                        .flex_1()
                        .cursor_pointer()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .when(shown, |d| d.bg(cx.theme().secondary))
                        .when(e.excluded, |d| d.text_color(muted))
                        .child(format!(
                            "#{ep}   {} frames   {}",
                            e.length,
                            e.tasks.first().map(String::as_str).unwrap_or("")
                        ))
                        .on_click(cx.listener(move |this, _, window, cx| this.open_episode(ep, window, cx))),
                )
                .child(Checkbox::new(("ep-excl", i)).label(self.t("lab_ds_exclude", cx)).checked(e.excluded).on_click(cx.listener(
                    move |this, checked: &bool, window, cx| this.toggle_excluded(ep, *checked, window, cx),
                )))
        });
        section(self.t("lab_ds_episodes", cx))
            .child(div().text_sm().text_color(muted).child(self.t("lab_ds_curation_hint", cx)))
            .child(v_flex().id("ds-episodes").max_h(px(260.)).overflow_y_scroll().gap_1().children(rows))
    }

    fn edit_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let has = self.current.is_some() && self.ready(cx);
        let muted = cx.theme().muted_foreground;
        let btn = |id: &'static str, key: &str, cx: &mut Context<Self>, f: fn(&mut Self, &mut Window, &mut Context<Self>)| {
            Button::new(id).small().outline().label(self.t(key, cx)).disabled(!has).on_click(cx.listener(
                move |this, _, window, cx| f(this, window, cx),
            ))
        };
        section(self.t("lab_ds_edit", cx))
            .child(div().text_sm().text_color(muted).child(self.t("lab_ds_edit_hint", cx)))
            .child(widgets::field(self.t("lab_ds_output", cx), Input::new(&self.output).small(), cx))
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(btn("ds-delete", "lab_ds_delete_selected", cx, Self::delete_selected))
                    .child(btn("ds-drop", "lab_ds_drop_excluded", cx, Self::drop_excluded))
                    .child(btn("ds-rename", "lab_ds_rename", cx, Self::rename)),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .flex_wrap()
                    .child(div().w(px(120.)).child(NumberInput::new(&self.split_fraction).small()))
                    .child(btn("ds-split", "lab_ds_split", cx, Self::split))
                    .child(div().w(px(260.)).child(Select::new(&self.remove_feature).small()))
                    .child(btn("ds-rm-feature", "lab_ds_remove_feature", cx, Self::remove_selected_feature)),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(Checkbox::new("ds-private").label(self.t("lab_ds_private", cx)).checked(self.private).on_click(
                        cx.listener(|this, checked: &bool, _, cx| {
                            this.private = *checked;
                            cx.notify();
                        }),
                    ))
                    .child(btn("ds-upload", "lab_ds_upload", cx, Self::upload)),
            )
    }

    fn operations_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.operations.iter().map(|o| {
            let color = match o.state.as_str() {
                "failed" => rgb(0xF44336),
                "done" => rgb(0x4CAF50),
                _ => rgb(0xFA8F01),
            };
            div().text_sm().text_color(color).child(format!(
                "{} · {}{}",
                o.label,
                o.state,
                o.error.as_ref().map(|e| format!(" · {e}")).unwrap_or_default()
            ))
        });
        section(self.t("lab_ds_operations", cx)).children(rows)
    }
}

fn img_element(image: Arc<RenderImage>) -> AnyElement {
    img(image).h(px(300.)).object_fit(ObjectFit::Contain).into_any_element()
}

impl Render for LabDatasetsPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ready = self.ready(cx);
        let mut page = v_flex().id("lab-datasets-page").size_full().p_3().gap_3().overflow_y_scroll();
        if !ready {
            page = page.child(div().text_color(cx.theme().muted_foreground).child(self.t("lab_need_engine", cx)));
        }
        page.child(self.datasets_section(cx))
            .child(self.viewer_section(cx))
            .child(self.episodes_section(cx))
            .child(self.edit_section(cx))
            .child(self.operations_section(cx))
    }
}
