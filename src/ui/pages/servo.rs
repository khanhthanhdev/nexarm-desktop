//! Action group control: six servo controls, keyframe editor, online run and board download.

use crate::protocol::{self, ActionFrame, Request, ServoTarget, cmd};
use crate::ui::state::{AppState, DeviceEvent};
use crate::ui::widgets::{self, NumState, field, section};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState, NumberInput, NumberInputEvent, StepAction};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::time::{Duration, Instant};

/// Usable position range per servo (ID 1..6).
const RANGES: [(u16, u16); 6] = [(341, 3755), (1195, 2901), (1024, 3072), (683, 3413), (1024, 3072), (1024, 2731)];
/// Degrees covered by the full 0..4096 count range per servo.
const ANGLE_SPAN: [f64; 6] = [360., 360., 360., 360., 240., 240.];
const DEFAULT_ACC: u8 = 100;
const DEFAULT_SPD: u16 = 2000;
/// Minimum gap between position writes to one servo while dragging.
const SEND_INTERVAL: Duration = Duration::from_millis(50);
/// Gap between the six per-servo writes of one keyframe.
const SERVO_GAP: Duration = Duration::from_millis(2);
const DOWNLOAD_GAP: Duration = Duration::from_millis(150);

struct ServoCtl {
    slider: Entity<SliderState>,
    pos: Entity<InputState>,
    acc: Entity<InputState>,
    spd: Entity<InputState>,
    torque: bool,
}

/// Keyframe editor button handler.
type EditAction = fn(&mut ServoPage, &mut Context<ServoPage>);

#[derive(Clone, Copy)]
enum Param {
    Acc,
    Spd,
}

pub struct ServoPage {
    app: Entity<AppState>,
    ctl: Vec<ServoCtl>,
    vals: [u16; 6],
    acc: [u8; 6],
    spd: [u16; 6],
    dragging: [bool; 6],
    pending: [bool; 6],
    last_send: [Option<Instant>; 6],
    /// "Read positions" is waiting for these servo IDs (index 0 = ID 1).
    manual_read: [bool; 6],
    /// Last positions reported by the arm, used as start points for time-based speeds.
    latest: [u16; 6],
    /// Bumped on every position reply so waiters can tell a fresh reply arrived.
    pos_gen: u64,
    frames: Vec<ActionFrame>,
    selected: Option<usize>,
    run_row: Option<usize>,
    online_running: bool,
    stop_online: bool,
    delay: NumState,
    loops: NumState,
    gid: NumState,
    _flush: Task<()>,
    _subs: Vec<Subscription>,
}

fn angle_text(idx: usize, pos: u16) -> String {
    format!("{:+.1}°", (pos as f64 - 2048.) / 4096. * ANGLE_SPAN[idx])
}

/// Caller-stepped numeric input (the page applies steps itself so it can react immediately).
fn stepped_input(value: u16, window: &mut Window, cx: &mut App) -> Entity<InputState> {
    cx.new(|cx| InputState::new(window, cx).default_value(value.to_string()))
}

/// Six servo writes that move to `f` in its frame time, starting from `current` (updated in place).
fn frame_requests(f: &ActionFrame, current: &mut [u16; 6]) -> Vec<Request> {
    f.servos
        .iter()
        .take(6)
        .enumerate()
        .map(|(i, s)| {
            let sp = protocol::time_based_speed(current[i], s.pos, f.time);
            current[i] = s.pos;
            protocol::servo_move(i as u8 + 1, s.pos, s.acc, sp)
        })
        .collect()
}

/// Parse a `.d6a` file, accepting the legacy layouts (servo list of ints, bare row lists).
fn parse_d6a(text: &str) -> anyhow::Result<Vec<ActionFrame>> {
    use serde_json::Value;
    let rows: Vec<Value> = serde_json::from_str(text)?;
    let int = |v: Option<&Value>, d: i64| v.and_then(Value::as_i64).unwrap_or(d);
    rows.iter()
        .map(|r| {
            let (time, servos) = match r {
                Value::Array(list) => (1000, list.clone()),
                Value::Object(o) => (
                    int(o.get("time"), 1000),
                    o.get("servos").and_then(Value::as_array).cloned().unwrap_or_default(),
                ),
                _ => anyhow::bail!("invalid action frame: {r}"),
            };
            let servos = (0..6)
                .map(|i| match servos.get(i) {
                    Some(Value::Object(s)) => {
                        ServoTarget { pos: int(s.get("pos"), 2048) as u16, acc: int(s.get("acc"), 0) as u8 }
                    }
                    Some(v) => ServoTarget { pos: int(Some(v), 2048) as u16, acc: 0 },
                    None => ServoTarget { pos: 2048, acc: 0 },
                })
                .collect();
            Ok(ActionFrame { time: time.clamp(0, u16::MAX as i64) as u16, servos })
        })
        .collect()
}

impl ServoPage {
    pub fn new(app: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut subs = vec![
            cx.observe(&app, |_, _, cx| cx.notify()),
            cx.subscribe(&app, |this, _, ev: &DeviceEvent, cx| {
                if let DeviceEvent::Response(r) = ev {
                    this.on_response(r, cx);
                }
            }),
        ];
        let mut ctl = Vec::new();
        for (i, &(lo, hi)) in RANGES.iter().enumerate() {
            let mid = (lo + hi) / 2;
            let slider =
                cx.new(|_| SliderState::new().max(hi as f32).min(lo as f32).step(1.).default_value(mid as f32));
            let pos = cx.new(|cx| InputState::new(window, cx).default_value(mid.to_string()));
            let acc = stepped_input(DEFAULT_ACC as u16, window, cx);
            let spd = stepped_input(DEFAULT_SPD, window, cx);
            subs.push(cx.subscribe_in(&slider, window, move |this, _, ev: &SliderEvent, window, cx| match ev {
                SliderEvent::Change(v) => {
                    this.dragging[i] = true;
                    this.on_slide(i, v.start().round() as u16, window, cx);
                }
                SliderEvent::Release(_) => {
                    this.dragging[i] = false;
                    this.queue(i, true, cx);
                }
            }));
            subs.push(cx.subscribe_in(&pos, window, move |this, state, ev: &InputEvent, window, cx| {
                if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    let (lo, hi) = RANGES[i];
                    let v = widgets::read_num(state, cx).round().clamp(lo as f64, hi as f64) as u16;
                    widgets::set_num(state, v as f64, window, cx);
                    if v != this.vals[i] {
                        this.on_pos_edit(i, v, window, cx);
                    }
                }
            }));
            for (state, param) in [(&acc, Param::Acc), (&spd, Param::Spd)] {
                subs.push(cx.subscribe_in(state, window, move |this, state, ev: &InputEvent, window, cx| {
                    if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                        let v = widgets::read_num(state, cx);
                        this.set_param(i, param, v, window, cx);
                    }
                }));
                subs.push(cx.subscribe_in(state, window, move |this, _, ev: &NumberInputEvent, window, cx| {
                    let NumberInputEvent::Step(action) = ev;
                    let d = if matches!(action, StepAction::Increment) { 1. } else { -1. };
                    let v = this.param(i, param) as f64 + d;
                    this.set_param(i, param, v, window, cx);
                }));
            }
            ctl.push(ServoCtl { slider, pos, acc, spd, torque: true });
        }

        let flush = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(SEND_INTERVAL).await;
                if this.update(cx, |p, cx| p.flush(None, cx)).is_err() {
                    break;
                }
            }
        });

        Self {
            app,
            ctl,
            vals: RANGES.map(|(lo, hi)| (lo + hi) / 2),
            acc: [DEFAULT_ACC; 6],
            spd: [DEFAULT_SPD; 6],
            dragging: [false; 6],
            pending: [false; 6],
            last_send: [None; 6],
            manual_read: [false; 6],
            latest: [2048; 6],
            pos_gen: 0,
            frames: Vec::new(),
            selected: None,
            run_row: None,
            online_running: false,
            stop_online: false,
            delay: widgets::num_input(1000., 10., 10000., 1., window, cx),
            loops: widgets::num_input(1., 0., 9999., 1., window, cx),
            gid: widgets::num_input(0., 0., 255., 1., window, cx),
            _flush: flush,
            _subs: subs,
        }
    }

    /// Copy the last reported servo positions into the controls without moving the arm.
    pub fn on_shown(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(servos) = self.app.read(cx).pose.as_ref().map(|p| p.servos) else { return };
        for (i, v) in servos.iter().enumerate() {
            self.show_pos(i, (*v).max(0) as u16, window, cx);
        }
        cx.notify();
    }

    fn t(&self, key: &str, cx: &App) -> String {
        self.app.read(cx).t(key)
    }

    fn send(&self, req: Request, cx: &mut Context<Self>) {
        self.app.update(cx, |a, cx| a.send(req, cx));
    }

    fn send_paced(&self, reqs: Vec<Request>, gap: Duration, cx: &mut Context<Self>) {
        self.app.update(cx, |a, cx| a.send_paced(reqs, gap, cx));
    }

    // ------------------------------------------------------------ servo controls

    /// Show `pos` in slider and input and remember it, without sending anything.
    fn show_pos(&mut self, i: usize, pos: u16, window: &mut Window, cx: &mut Context<Self>) {
        self.vals[i] = pos;
        self.ctl[i].slider.update(cx, |s, cx| s.set_value(pos as f32, window, cx));
        widgets::set_num(&self.ctl[i].pos, pos as f64, window, cx);
    }

    /// Moving a servo by hand re-enables its torque, like the original.
    fn ensure_torque(&mut self, i: usize, cx: &mut Context<Self>) {
        if !self.ctl[i].torque {
            self.ctl[i].torque = true;
            self.send(protocol::servo_torque(i as u8 + 1, true), cx);
        }
    }

    fn on_slide(&mut self, i: usize, v: u16, window: &mut Window, cx: &mut Context<Self>) {
        self.ensure_torque(i, cx);
        self.vals[i] = v;
        widgets::set_num(&self.ctl[i].pos, v as f64, window, cx);
        self.queue(i, false, cx);
        cx.notify();
    }

    fn on_pos_edit(&mut self, i: usize, v: u16, window: &mut Window, cx: &mut Context<Self>) {
        self.ensure_torque(i, cx);
        self.vals[i] = v;
        self.ctl[i].slider.update(cx, |s, cx| s.set_value(v as f32, window, cx));
        self.queue(i, false, cx);
        cx.notify();
    }

    fn param(&self, i: usize, p: Param) -> u16 {
        match p {
            Param::Acc => self.acc[i] as u16,
            Param::Spd => self.spd[i],
        }
    }

    fn set_param(&mut self, i: usize, p: Param, v: f64, window: &mut Window, cx: &mut Context<Self>) {
        let (state, max) = match p {
            Param::Acc => (self.ctl[i].acc.clone(), 255.),
            Param::Spd => (self.ctl[i].spd.clone(), 3400.),
        };
        let v = v.round().clamp(0., max) as u16;
        widgets::set_num(&state, v as f64, window, cx);
        if v == self.param(i, p) {
            return;
        }
        match p {
            Param::Acc => self.acc[i] = v as u8,
            Param::Spd => self.spd[i] = v,
        }
        let immediate = !self.dragging[i];
        self.queue(i, immediate, cx);
    }

    fn queue(&mut self, i: usize, immediate: bool, cx: &mut Context<Self>) {
        self.pending[i] = true;
        if immediate {
            self.flush(Some(i), cx);
        }
    }

    /// Send queued position writes, at most one per servo every 50 ms unless forced.
    fn flush(&mut self, force: Option<usize>, cx: &mut Context<Self>) {
        let now = Instant::now();
        for i in 0..6 {
            if force.is_some_and(|f| f != i) || !self.pending[i] {
                continue;
            }
            if force.is_none() && self.last_send[i].is_some_and(|t| now - t < SEND_INTERVAL) {
                continue;
            }
            self.pending[i] = false;
            self.send(protocol::servo_move(i as u8 + 1, self.vals[i], self.acc[i], self.spd[i]), cx);
            self.last_send[i] = Some(now);
        }
    }

    fn toggle_torque(&mut self, i: usize, cx: &mut Context<Self>) {
        let on = !self.ctl[i].torque;
        self.ctl[i].torque = on;
        self.send(protocol::servo_torque(i as u8 + 1, on), cx);
        cx.notify();
    }

    fn torque_all(&mut self, on: bool, cx: &mut Context<Self>) {
        self.app.update(cx, |a, cx| a.send_sys(cmd::SET_TORQUE, [on as u8], cx));
        self.ctl.iter_mut().for_each(|c| c.torque = on);
        cx.notify();
    }

    fn home(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for i in 0..6 {
            self.show_pos(i, 2048, window, cx);
            self.ctl[i].torque = true;
        }
        let reqs = (0..6).map(|i| protocol::servo_move(i as u8 + 1, 2048, self.acc[i], self.spd[i])).collect();
        self.send_paced(reqs, SERVO_GAP, cx);
        cx.notify();
    }

    fn read_and_sync(&mut self, cx: &mut Context<Self>) {
        self.pending = [false; 6];
        self.manual_read = [true; 6];
        self.app.update(cx, |a, cx| a.send_sys(cmd::GET_REAL_JOINT_ANGLES, [], cx));
    }

    fn calibrate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let app = self.app.clone();
        widgets::confirm(self.t("btn_cali_pos", cx), self.t("msg_cali_confirm", cx), window, cx, move |_, cx| {
            let reqs = (1..=6).map(|id| Request::sys(cmd::SERVO_CALI_POS, [id])).collect();
            app.update(cx, |a, cx| a.send_paced(reqs, DOWNLOAD_GAP, cx));
        });
    }

    fn on_response(&mut self, r: &protocol::Response, cx: &mut Context<Self>) {
        let mut updates: Vec<(usize, u16)> = Vec::new();
        match r {
            protocol::Response::ServoPosition { id, pos } => {
                let i = *id as usize - 1;
                self.latest[i] = *pos;
                self.pos_gen += 1;
                if self.manual_read.iter().any(|b| *b) {
                    updates.push((i, *pos));
                    self.manual_read[i] = false;
                }
            }
            protocol::Response::JointPositions(p) => {
                for (i, v) in p.iter().enumerate() {
                    if let Some(v) = v {
                        self.latest[i] = *v;
                    }
                }
                self.pos_gen += 1;
                if self.manual_read.iter().any(|b| *b) {
                    updates.extend(self.latest.iter().enumerate().map(|(i, v)| (i, *v)));
                    self.manual_read = [false; 6];
                }
            }
            _ => return,
        }
        if updates.is_empty() {
            return;
        }
        // Showing positions needs a window; defer to the next frame of this page's window.
        let page = cx.entity();
        cx.defer(move |cx| {
            let Some(w) = cx.active_window().or_else(|| cx.windows().first().copied()) else { return };
            let _ = w.update(cx, |_, window, cx| {
                page.update(cx, |p, cx| {
                    for (i, v) in updates {
                        p.show_pos(i, v, window, cx);
                    }
                    cx.notify();
                })
            });
        });
    }

    /// Ask the arm for its joint positions and wait up to 350 ms for a reply.
    async fn actual_positions(this: &WeakEntity<Self>, cx: &mut AsyncApp) -> Option<[u16; 6]> {
        let generation = this
            .update(cx, |p, cx| {
                p.app.update(cx, |a, cx| a.send_sys(cmd::GET_REAL_JOINT_ANGLES, [], cx));
                p.pos_gen
            })
            .ok()?;
        for _ in 0..35 {
            cx.background_executor().timer(Duration::from_millis(10)).await;
            if this.read_with(cx, |p, _| p.pos_gen != generation).ok()? {
                break;
            }
        }
        this.read_with(cx, |p, _| p.latest).ok()
    }

    // ---------------------------------------------------------- keyframe editor

    fn frame_from_ui(&self, cx: &App) -> ActionFrame {
        ActionFrame {
            time: widgets::read_num(&self.delay, cx).clamp(0., 10000.) as u16,
            servos: (0..6).map(|i| ServoTarget { pos: self.vals[i], acc: self.acc[i] }).collect(),
        }
    }

    fn act_add(&mut self, cx: &mut Context<Self>) {
        self.frames.push(self.frame_from_ui(cx));
        self.selected = None;
        cx.notify();
    }

    fn act_update(&mut self, cx: &mut Context<Self>) {
        if let Some(i) = self.selected {
            self.frames[i] = self.frame_from_ui(cx);
            cx.notify();
        }
    }

    fn act_del(&mut self, cx: &mut Context<Self>) {
        if let Some(i) = self.selected {
            self.frames.remove(i);
            self.selected = (!self.frames.is_empty()).then(|| i.min(self.frames.len() - 1));
            cx.notify();
        }
    }

    fn act_clear(&mut self, cx: &mut Context<Self>) {
        self.frames.clear();
        self.selected = None;
        cx.notify();
    }

    fn act_move(&mut self, up: bool, cx: &mut Context<Self>) {
        let Some(i) = self.selected else { return };
        let j = if up { i.checked_sub(1) } else { Some(i + 1).filter(|j| *j < self.frames.len()) };
        if let Some(j) = j {
            self.frames.swap(i, j);
            self.selected = Some(j);
            cx.notify();
        }
    }

    /// Selecting a keyframe loads it into the controls and previews it on the arm.
    fn select_row(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected == Some(row) {
            return;
        }
        self.selected = Some(row);
        let f = self.frames[row].clone();
        self.pending = [false; 6];
        widgets::set_num(&self.delay, f.time as f64, window, cx);
        for (i, s) in f.servos.iter().enumerate().take(6) {
            self.show_pos(i, s.pos, window, cx);
            self.acc[i] = s.acc;
            widgets::set_num(&self.ctl[i].acc, s.acc as f64, window, cx);
            // Keyframes carry no speed: the original shows the default but keeps the stored speed.
            widgets::set_num(&self.ctl[i].spd, DEFAULT_SPD as f64, window, cx);
        }
        cx.notify();
        if self.online_running {
            return;
        }
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let Some(mut current) = Self::actual_positions(&this, cx).await else { return };
            let reqs = frame_requests(&f, &mut current);
            let _ = this.update(cx, |p, cx| {
                p.send_paced(reqs, SERVO_GAP, cx);
                p.latest = current;
            });
        })
        .detach();
    }

    fn total_time(&self) -> u64 {
        self.frames.iter().map(|f| f.time as u64).sum()
    }

    fn act_save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let body = match serde_json::to_string(&self.frames) {
            Ok(b) => b,
            Err(e) => return widgets::notify_error(e.to_string(), window, cx),
        };
        let err_title = self.t("msg_error", cx);
        let dir = dirs::document_dir().unwrap_or_else(std::env::temp_dir);
        let rx = cx.prompt_for_new_path(&dir, Some("action.d6a"));
        cx.spawn_in(window, async move |_, cx| {
            let Ok(Ok(Some(path))) = rx.await else { return };
            if let Err(e) = std::fs::write(&path, body) {
                let _ = cx.update(|window, cx| widgets::notify_error(format!("{err_title}: {e}"), window, cx));
            }
        })
        .detach();
    }

    fn act_load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let err_title = self.t("msg_error", cx);
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Open Action Group".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            let res = std::fs::read_to_string(&path).map_err(anyhow::Error::from).and_then(|t| parse_d6a(&t));
            let _ = this.update_in(cx, |p, window, cx| match res {
                Ok(frames) => {
                    p.frames = frames;
                    p.selected = None;
                    cx.notify();
                }
                Err(e) => widgets::notify_error(format!("{err_title}: {e:#}"), window, cx),
            });
        })
        .detach();
    }

    // ------------------------------------------------------------ run / download

    fn run_online(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.frames.is_empty() || self.online_running {
            return;
        }
        self.stop_online = false;
        self.online_running = true;
        let loops = widgets::read_num(&self.loops, cx).clamp(0., 9999.) as u32;
        let frames = self.frames.clone();
        cx.spawn_in(window, async move |this, cx| {
            let mut done = 0;
            'outer: loop {
                if this.read_with(cx, |p, _| p.stop_online).unwrap_or(true) {
                    break;
                }
                let Some(mut current) = Self::actual_positions(&this, cx).await else { return };
                for (step, f) in frames.iter().enumerate() {
                    if this.read_with(cx, |p, _| p.stop_online).unwrap_or(true) {
                        break;
                    }
                    let reqs = frame_requests(f, &mut current);
                    let _ = this.update(cx, |p, cx| {
                        p.run_row = Some(step);
                        p.selected = Some(step);
                        p.send_paced(reqs, SERVO_GAP, cx);
                        cx.notify();
                    });
                    let end = Instant::now() + Duration::from_millis(f.time as u64);
                    while Instant::now() < end {
                        if this.read_with(cx, |p, _| p.stop_online).unwrap_or(true) {
                            break;
                        }
                        cx.background_executor().timer(Duration::from_millis(50)).await;
                    }
                }
                let _ = this.update(cx, |p, _| p.latest = current);
                done += 1;
                if loops > 0 && done >= loops {
                    break 'outer;
                }
            }
            let _ = this.update_in(cx, |p, window, cx| {
                p.online_running = false;
                p.run_row = None;
                widgets::notify_info(p.t("msg_online_done", cx), window, cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn download(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.frames.is_empty() {
            return;
        }
        let gid = widgets::read_num(&self.gid, cx).clamp(0., 255.) as u8;
        let frames = self.frames.clone();
        let total = frames.len();
        cx.spawn_in(window, async move |this, cx| {
            let Some(mut current) = Self::actual_positions(&this, cx).await else { return };
            let reqs: Vec<Request> = frames
                .iter()
                .enumerate()
                .map(|(i, f)| protocol::action_download(gid, total as u8, i as u8 + 1, f, &mut current))
                .collect();
            let _ = this.update(cx, |p, cx| p.send_paced(reqs, DOWNLOAD_GAP, cx));
            cx.background_executor().timer(DOWNLOAD_GAP * total as u32).await;
            let _ = this.update_in(cx, |p, window, cx| {
                p.latest = current;
                let msg = p.t("msg_download_complete", cx).replace("{0}", &gid.to_string()).replace("{1}", &total.to_string());
                widgets::notify_info(msg, window, cx);
            });
        })
        .detach();
    }

    // ------------------------------------------------------------------- render

    fn render_servo(&self, i: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let c = &self.ctl[i];
        let theme = cx.theme();
        let small = |s: &str| div().text_sm().text_color(theme.muted_foreground).child(s.to_string());
        v_flex()
            .flex_1()
            .min_w(px(360.))
            .gap_2()
            .p_3()
            .rounded_md()
            .border_1()
            .border_color(theme.border)
            .bg(theme.secondary)
            .child(
                h_flex()
                    .justify_between()
                    .child(div().font_weight(FontWeight::BOLD).child(format!("ID: {}", i + 1)))
                    .child(
                        Button::new(("torque", i))
                            .small()
                            .when(c.torque, |b| b.primary())
                            .label(if c.torque { "ON" } else { "OFF" })
                            .on_click(cx.listener(move |this, _, _, cx| this.toggle_torque(i, cx))),
                    ),
            )
            .child(Slider::new(&c.slider))
            .child(
                h_flex()
                    .gap_2()
                    .child(small("Pos:"))
                    .child(div().w(px(80.)).child(Input::new(&c.pos).small()))
                    .child(small("Ang:"))
                    .child(div().w(px(70.)).text_center().child(angle_text(i, self.vals[i]))),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(small("Acc:"))
                    .child(div().w(px(130.)).child(NumberInput::new(&c.acc).small()))
                    .child(small("Spd:"))
                    .child(div().w(px(140.)).child(NumberInput::new(&c.spd).small())),
            )
    }

    fn render_table(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let headers: Vec<String> = [self.t("tree_idx", cx), self.t("tree_time", cx)]
            .into_iter()
            .chain((1..=6).map(|i| format!("S{i}")))
            .collect();
        let cell = |s: String, first: bool| div().when(first, |d| d.w(px(44.))).when(!first, |d| d.flex_1()).text_sm().child(s);
        let header = h_flex()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(theme.border)
            .text_color(theme.muted_foreground)
            .children(headers.into_iter().enumerate().map(|(k, h)| cell(h, k == 0)));
        let rows = self.frames.iter().enumerate().map(|(r, f)| {
            let selected = self.selected == Some(r);
            let running = self.run_row == Some(r);
            let vals = [(r + 1).to_string(), f.time.to_string()]
                .into_iter()
                .chain(f.servos.iter().map(|s| s.pos.to_string()));
            h_flex()
                .id(("frame", r))
                .px_2()
                .py_1()
                .cursor_pointer()
                .when(selected && !running, |d| d.bg(theme.secondary_hover))
                .when(running, |d| d.bg(rgba(0xFA8F016E)).text_color(white()))
                .hover(|d| d.bg(theme.secondary_hover))
                .children(vals.enumerate().map(|(k, v)| cell(v, k == 0)))
                .on_click(cx.listener(move |this, _, window, cx| this.select_row(r, window, cx)))
        });
        v_flex()
            .flex_1()
            .min_h(px(160.))
            .rounded_md()
            .border_1()
            .border_color(theme.border)
            .child(header)
            .child(div().id("frames").flex_1().overflow_y_scroll().children(rows))
    }
}

impl Render for ServoPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let lang = self.app.read(cx).lang;
        let t = |k: &str| crate::i18n::tr(lang, k);
        let btn = |id: &'static str, key: &str| Button::new(id).label(t(key));

        let top = h_flex()
            .gap_2()
            .flex_wrap()
            .child(btn("all-home", "btn_all_home").on_click(cx.listener(|this, _, window, cx| this.home(window, cx))))
            .child(btn("all-on", "btn_all_on").on_click(cx.listener(|this, _, _, cx| this.torque_all(true, cx))))
            .child(btn("all-off", "btn_all_off").on_click(cx.listener(|this, _, _, cx| this.torque_all(false, cx))))
            .child(btn("read-pos", "btn_read_pos").on_click(cx.listener(|this, _, _, cx| this.read_and_sync(cx))))
            .child(btn("cali", "btn_cali_pos").on_click(cx.listener(|this, _, window, cx| this.calibrate(window, cx))));

        let picture = v_flex()
            .w(px(220.))
            .flex_none()
            .gap_2()
            .items_center()
            .child(img(crate::assets_dir().join("0.842.png")).w(px(210.)).h(px(300.)).object_fit(ObjectFit::Contain))
            .child(div().text_sm().text_center().text_color(cx.theme().muted_foreground).child(t("hint_servo_id")));

        let grid = v_flex().flex_1().min_w_0().gap_2().children((0..3).map(|row| {
            h_flex().gap_2().child(self.render_servo(row * 2, cx)).child(self.render_servo(row * 2 + 1, cx))
        }).collect::<Vec<_>>());

        let control = section(t("grp_servo_ctrl"))
            .flex_none()
            .child(top)
            .child(h_flex().gap_3().items_start().child(picture).child(grid));

        let edit_buttons: [(&'static str, &str, EditAction); 6] = [
            ("add", "btn_add", Self::act_add),
            ("update", "btn_update", Self::act_update),
            ("del", "btn_delete", Self::act_del),
            ("clear", "btn_clear", Self::act_clear),
            ("up", "btn_up", |p, cx| p.act_move(true, cx)),
            ("down", "btn_down", |p, cx| p.act_move(false, cx)),
        ];
        let edit_grid = v_flex().gap_1().children(edit_buttons.chunks(2).map(|pair| {
            h_flex().gap_1().children(pair.iter().map(|&(id, key, f)| {
                btn(id, key).flex_1().on_click(cx.listener(move |this, _, _, cx| f(this, cx)))
            }))
        }).collect::<Vec<_>>());

        let editor = section(t("grp_action_editor"))
            .child(self.render_table(cx))
            .child(field(t("lbl_frame_time"), NumberInput::new(&self.delay).small(), cx))
            .child(div().text_xs().text_color(cx.theme().muted_foreground).child(t("lbl_action_speed_note")))
            .child(edit_grid)
            .child(
                h_flex()
                    .gap_1()
                    .child(btn("save", "btn_save").flex_1().on_click(cx.listener(|this, _, window, cx| this.act_save(window, cx))))
                    .child(btn("load", "btn_load").flex_1().on_click(cx.listener(|this, _, window, cx| this.act_load(window, cx)))),
            )
            .child(div().text_sm().child(t("lbl_total_time").replace("{0}", &self.total_time().to_string())));

        let run = section(t("grp_run_download"))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        btn("run-online", "btn_run_online")
                            .primary()
                            .flex_1()
                            .loading(self.online_running)
                            .on_click(cx.listener(|this, _, window, cx| this.run_online(window, cx))),
                    )
                    .child(div().text_sm().child(t("lbl_loop")))
                    .child(div().w(px(110.)).child(NumberInput::new(&self.loops).small()))
                    .child(btn("stop-online", "btn_stop").on_click(cx.listener(|this, _, _, _| this.stop_online = true))),
            )
            .child(field(t("lbl_group_id"), NumberInput::new(&self.gid).small(), cx))
            .child(
                h_flex()
                    .gap_1()
                    .child(btn("download", "btn_download").flex_1().on_click(cx.listener(|this, _, window, cx| this.download(window, cx))))
                    .child(btn("run-board", "btn_run_board").flex_1().on_click(cx.listener(|this, _, _, cx| {
                        let gid = widgets::read_num(&this.gid, cx).clamp(0., 255.) as u8;
                        this.app.update(cx, |a, cx| a.send_sys(cmd::ACTION_GROUP_RUN, [gid], cx));
                    }))),
            )
            .child(
                h_flex()
                    .gap_1()
                    .child(btn("erase", "btn_erase").flex_1().on_click(cx.listener(|this, _, _, cx| {
                        let gid = widgets::read_num(&this.gid, cx).clamp(0., 255.) as u8;
                        this.app.update(cx, |a, cx| a.send_sys(cmd::ACTION_GROUP_ERASE, [gid], cx));
                    })))
                    .child(btn("stop-board", "btn_stop").flex_1().on_click(cx.listener(|this, _, _, cx| {
                        this.app.update(cx, |a, cx| a.send_sys(cmd::ACTION_GROUP_STOP, [], cx));
                    }))),
            );

        v_flex()
            .id("servo-page")
            .size_full()
            .p_3()
            .gap_3()
            .overflow_y_scroll()
            .child(control)
            .child(h_flex().gap_3().items_start().child(editor.flex_1().min_w_0()).child(run.flex_1().min_w_0()))
    }
}

#[cfg(test)]
mod tests {
    use super::{ActionFrame, ServoTarget, frame_requests, parse_d6a, protocol};

    #[test]
    fn d6a_accepts_current_and_legacy_layouts() {
        let f = parse_d6a(r#"[{"time":500,"servos":[{"pos":2100,"acc":5}]},{"time":-3,"servos":[1000,2000]},[1500]]"#)
            .unwrap();
        assert_eq!(f.len(), 3);
        assert_eq!((f[0].time, f[0].servos[0].pos, f[0].servos[0].acc), (500, 2100, 5));
        assert_eq!(f[0].servos.len(), 6);
        assert_eq!(f[0].servos[5].pos, 2048);
        assert_eq!((f[1].time, f[1].servos[1].pos, f[1].servos[1].acc), (0, 2000, 0));
        assert_eq!((f[2].time, f[2].servos[0].pos), (1000, 1500));
    }

    #[test]
    fn keyframe_writes_use_time_based_speed_from_current() {
        let f = ActionFrame { time: 1000, servos: (0..6).map(|_| ServoTarget { pos: 2148, acc: 7 }).collect() };
        let mut current = [2048; 6];
        let reqs = frame_requests(&f, &mut current);
        assert_eq!(current, [2148; 6]);
        // 100 counts in 1 s with +20% margin = 120.
        assert_eq!(reqs[0].bytes(), protocol::servo_move(1, 2148, 7, 120).bytes());
        assert_eq!(reqs[5].id, 6);
    }
}
