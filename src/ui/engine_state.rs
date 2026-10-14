//! Shared Lab state: engine connection, robot profiles, the live session and its heartbeat.
//!
//! Blocking engine calls run on the background executor; results come back through [`EngineState::run`].
//! Pages subscribe to [`EngineNote`] to refresh what they show.

use super::state::AppState;
use crate::engine::api::{CalibrationReport, Health, PortInfo, RobotProfile, SessionInfo};
use crate::engine::client::{ClientError, EngineClient};
use crate::engine::events::{self, EngineEvent, EventListener};
use crate::engine::handoff::{Action, Handoff};
use crate::engine::supervisor::{Attach, Supervisor};
use crate::engine::{API_VERSION, HEARTBEAT_INTERVAL};
use gpui_kit::*;
use serde_json::{Value, json};
use std::fmt::Display;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, PartialEq)]
pub enum EngineStatus {
    Unknown,
    Starting,
    Ready(Health),
    /// An engine answers but speaks another API version.
    Mismatch(String),
    Failed(String),
}

/// What changed, for pages to refresh from.
#[derive(Clone, Debug)]
pub enum EngineNote {
    Status,
    Profiles,
    Session,
    Calibration,
    Event(EngineEvent),
    /// A user-facing error to show once.
    Error(String),
}

pub struct EngineState {
    pub app: Entity<AppState>,
    pub client: EngineClient,
    pub status: EngineStatus,
    pub profiles: Vec<RobotProfile>,
    pub ports: Vec<PortInfo>,
    pub session: Option<SessionInfo>,
    /// A session start or stop call is in flight.
    pub busy: bool,
    pub last_ticks: Option<[f64; 6]>,
    pub calibration: Option<CalibrationReport>,
    supervisor: Arc<Mutex<Supervisor>>,
    handoff: Handoff,
    listener: Option<EventListener>,
    events_tx: flume::Sender<EngineEvent>,
    _pump: Task<()>,
    _heartbeat: Option<Task<()>>,
    /// Re-reads the live session status (phase, frame counts) once a second while one runs.
    _poll: Option<Task<()>>,
}

impl EventEmitter<EngineNote> for EngineState {}

impl EngineState {
    pub fn new(app: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let url = app.read(cx).settings.engine.url.clone();
        let (tx, rx) = flume::unbounded::<EngineEvent>();
        let pump = cx.spawn(async move |this: WeakEntity<EngineState>, cx: &mut AsyncApp| {
            while let Ok(first) = rx.recv_async().await {
                let batch: Vec<EngineEvent> = std::iter::once(first).chain(rx.try_iter()).collect();
                if this.update(cx, |s, cx| batch.into_iter().for_each(|e| s.on_event(e, cx))).is_err() {
                    break;
                }
            }
        });
        Self {
            app,
            client: EngineClient::new(url),
            status: EngineStatus::Unknown,
            profiles: Vec::new(),
            ports: Vec::new(),
            session: None,
            busy: false,
            last_ticks: None,
            calibration: None,
            supervisor: Arc::new(Mutex::new(Supervisor::default())),
            handoff: Handoff::default(),
            listener: None,
            events_tx: tx,
            _pump: pump,
            _heartbeat: None,
            _poll: None,
        }
    }

    pub fn is_ready(&self) -> bool {
        matches!(self.status, EngineStatus::Ready(_))
    }

    pub fn owns_engine(&self) -> bool {
        self.supervisor.lock().map(|s| s.owns_engine()).unwrap_or(false)
    }

    pub fn has_capability(&self, name: &str) -> bool {
        matches!(&self.status, EngineStatus::Ready(h) if h.capabilities.iter().any(|c| c == name))
    }

    pub fn selected_profile(&self, cx: &App) -> Option<RobotProfile> {
        let name = self.app.read(cx).settings.engine.profile.clone();
        self.profiles.iter().find(|p| p.name == name).cloned()
    }

    pub fn select_profile(&mut self, name: &str, cx: &mut Context<Self>) {
        self.app.update(cx, |a, _| {
            a.settings.engine.profile = name.to_string();
            a.persist();
        });
        cx.emit(EngineNote::Profiles);
        cx.notify();
    }

    /// Run blocking engine work off the UI thread and hand the result to `done` on it.
    pub fn run<T: Send + 'static>(
        &self,
        cx: &mut Context<Self>,
        work: impl FnOnce(EngineClient) -> T + Send + 'static,
        done: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
    ) {
        let client = self.client.clone();
        let task = cx.background_executor().spawn(async move { work(client) });
        cx.spawn(async move |this: WeakEntity<EngineState>, cx: &mut AsyncApp| {
            let out = task.await;
            let _ = this.update(cx, |s, cx| done(s, out, cx));
        })
        .detach();
    }

    pub fn report(&mut self, err: impl Display, cx: &mut Context<Self>) {
        let msg = err.to_string();
        self.app.update(cx, |a, cx| a.log_text(format!("Engine: {msg}"), cx));
        cx.emit(EngineNote::Error(msg));
    }

    // ------------------------------------------------------------- engine

    /// Attach to the engine at the configured URL, or start one (when allowed).
    pub fn connect_engine(&mut self, cx: &mut Context<Self>) {
        if matches!(self.status, EngineStatus::Starting) {
            return;
        }
        let settings = self.app.read(cx).settings.engine.clone();
        self.client = EngineClient::new(settings.url.clone());
        self.status = EngineStatus::Starting;
        cx.emit(EngineNote::Status);
        cx.notify();
        let supervisor = self.supervisor.clone();
        self.run(
            cx,
            move |client| -> Result<Attach, String> {
                let mut sup = supervisor.lock().map_err(|_| "supervisor poisoned".to_string())?;
                match crate::engine::supervisor::probe(&client) {
                    Attach::Unreachable(_) if settings.auto_start && !sup.owns_engine() => {
                        sup.spawn(&settings).map_err(|e| format!("{e:#}"))?;
                        sup.wait_ready(&client).map_err(|e| format!("{e:#}"))
                    }
                    Attach::Unreachable(e) => Err(e),
                    other => Ok(other),
                }
            },
            |s, res, cx| {
                s.status = match res {
                    Ok(Attach::Ready(h)) => EngineStatus::Ready(h),
                    Ok(Attach::Mismatch { engine }) => EngineStatus::Mismatch(engine),
                    Ok(Attach::Unreachable(e)) | Err(e) => EngineStatus::Failed(e),
                };
                if let EngineStatus::Failed(e) = &s.status {
                    let e = e.clone();
                    s.report(e, cx);
                }
                if s.is_ready() {
                    s.listener = Some(events::listen(s.client.events_url(), s.events_tx.clone()));
                    s.refresh(cx);
                } else {
                    s.listener = None;
                }
                cx.emit(EngineNote::Status);
                cx.notify();
            },
        );
    }

    /// Stop the engine, but only one this app started.
    pub fn stop_engine(&mut self, cx: &mut Context<Self>) {
        if !self.owns_engine() {
            return;
        }
        self.listener = None;
        let supervisor = self.supervisor.clone();
        self.status = EngineStatus::Unknown;
        cx.notify();
        self.run(
            cx,
            move |client| {
                if let Ok(mut sup) = supervisor.lock() {
                    sup.stop(&client);
                }
            },
            |s, (), cx| {
                s.session = None;
                s.release_handoff(Action::None, cx);
                cx.emit(EngineNote::Status);
                cx.emit(EngineNote::Session);
                cx.notify();
            },
        );
    }

    /// Re-read profiles, ports and the current session (after connecting and on every event reconnect).
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if !self.is_ready() {
            return;
        }
        self.run(
            cx,
            |c| (c.profiles(), c.ports(), c.current_session()),
            |s, (profiles, ports, session), cx| {
                match profiles {
                    Ok(p) => s.profiles = p,
                    Err(e) => s.report(e, cx),
                }
                if let Ok(p) = ports {
                    s.ports = p;
                }
                if let Ok(session) = session {
                    s.apply_session(session, cx);
                }
                if s.selected_profile(cx).is_none()
                    && let Some(first) = s.profiles.first().map(|p| p.name.clone())
                {
                    s.select_profile(&first, cx);
                }
                cx.emit(EngineNote::Profiles);
                cx.notify();
            },
        );
    }

    pub fn save_profile(&mut self, profile: RobotProfile, cx: &mut Context<Self>) {
        let name = profile.name.clone();
        self.run(
            cx,
            move |c| c.put_profile(&profile),
            move |s, res, cx| match res {
                Ok(_) => {
                    s.select_profile(&name, cx);
                    s.refresh(cx);
                }
                Err(e) => s.report(e, cx),
            },
        );
    }

    pub fn delete_profile(&mut self, name: String, cx: &mut Context<Self>) {
        self.run(
            cx,
            move |c| c.delete_profile(&name),
            |s, res, cx| {
                if let Err(e) = res {
                    s.report(e, cx);
                }
                s.refresh(cx);
            },
        );
    }

    pub fn check_calibration(&mut self, cx: &mut Context<Self>) {
        let Some(profile) = self.selected_profile(cx) else {
            return self.report("choose a robot profile first", cx);
        };
        if self.session.is_some() || self.busy {
            return self.report("stop the running session before checking calibration", cx);
        }
        // The check opens both ports, so the app's own link must be released first.
        let held = self.app.update(cx, |a, cx| a.release_for_session(cx));
        self.handoff.begin(held);
        let name = profile.name;
        self.run(
            cx,
            move |c| c.calibration_check(&name),
            |s, res, cx| {
                match res {
                    Ok(report) => s.calibration = Some(report),
                    Err(e) => s.report(e, cx),
                }
                // No session ran, so no idle event will come: give the links back now.
                let action = s.handoff.start_failed();
                s.release_handoff(action, cx);
                cx.emit(EngineNote::Calibration);
                cx.notify();
            },
        );
    }

    // ------------------------------------------------------------ sessions

    /// Start a session of `kind` for the selected profile. Releases the app's links first.
    pub fn start_session(&mut self, kind: &str, params: Value, cx: &mut Context<Self>) {
        if self.busy || self.session.is_some() {
            return self.report("a session is already active", cx);
        }
        if !self.is_ready() {
            return self.report("the engine is not connected", cx);
        }
        let Some(profile) = self.selected_profile(cx) else {
            return self.report("choose a robot profile first", cx);
        };
        let held = self.app.update(cx, |a, cx| a.release_for_session(cx));
        if matches!(self.handoff.begin(held), Action::None) {
            return;
        }
        self.busy = true;
        cx.emit(EngineNote::Session);
        cx.notify();
        let (kind, name) = (kind.to_string(), profile.name);
        self.run(
            cx,
            move |c| c.start_session(&kind, &name, params),
            |s, res, cx| {
                s.busy = false;
                match res {
                    Ok(info) => s.apply_session(Some(info), cx),
                    Err(e) => {
                        s.report(e, cx);
                        let action = s.handoff.start_failed();
                        s.release_handoff(action, cx);
                    }
                }
                cx.emit(EngineNote::Session);
                cx.notify();
            },
        );
    }

    pub fn stop_session(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.session.as_ref().map(|s| s.id.clone()) else { return };
        if self.busy {
            return;
        }
        self.busy = true;
        cx.emit(EngineNote::Session);
        cx.notify();
        self.run(
            cx,
            move |c| c.stop_session(&id),
            |s, res, cx| {
                s.busy = false;
                match res {
                    Ok(info) => {
                        if let Some(err) = info.error.clone() {
                            s.report(format!("the session ended with an error: {err}"), cx);
                        }
                        // The engine also sends session_changed idle; handling it here too is harmless.
                        s.on_session_idle(cx);
                    }
                    Err(e) => s.report(e, cx),
                }
                cx.emit(EngineNote::Session);
                cx.notify();
            },
        );
    }

    /// Send a live control (pause, takeover, ...) to the running session.
    pub fn control(&mut self, action: &str, payload: Value, cx: &mut Context<Self>) {
        let Some(id) = self.session.as_ref().map(|s| s.id.clone()) else { return };
        let action = action.to_string();
        self.run(
            cx,
            move |c| c.control(&id, &action, payload),
            |s, res, cx| {
                if let Err(e) = res {
                    s.report(e, cx);
                }
                s.refresh_session(cx);
            },
        );
    }

    pub fn refresh_session(&mut self, cx: &mut Context<Self>) {
        if !self.is_ready() {
            return;
        }
        self.run(
            cx,
            |c| c.current_session(),
            |s, res, cx| {
                if let Ok(session) = res {
                    s.apply_session(session, cx);
                }
            },
        );
    }

    fn apply_session(&mut self, session: Option<SessionInfo>, cx: &mut Context<Self>) {
        let was = self.session.as_ref().map(|s| s.id.clone());
        match session {
            Some(info) => {
                let new = was.as_deref() != Some(info.id.as_str());
                let id = info.id.clone();
                self.session = Some(info);
                if new {
                    self.start_heartbeat(id, cx);
                    self.start_poll(cx);
                }
            }
            None if self.session.is_some() => self.on_session_idle(cx),
            None => {}
        }
        cx.emit(EngineNote::Session);
        cx.notify();
    }

    /// The arm is released: forget the session and give the app's links back.
    fn on_session_idle(&mut self, cx: &mut Context<Self>) {
        self.session = None;
        self._heartbeat = None;
        self._poll = None;
        let action = self.handoff.on_session_state("idle");
        self.release_handoff(action, cx);
    }

    fn release_handoff(&mut self, action: Action, cx: &mut Context<Self>) {
        if let Action::ReclaimLinks(specs) = action {
            self.app.update(cx, |a, cx| a.reclaim_links(specs, cx));
        }
    }

    fn start_poll(&mut self, cx: &mut Context<Self>) {
        self._poll = Some(cx.spawn(async move |this: WeakEntity<EngineState>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(std::time::Duration::from_secs(1)).await;
                let keep = this
                    .update(cx, |s, cx| {
                        let active = s.session.is_some();
                        if active && !s.busy {
                            s.refresh_session(cx);
                        }
                        active
                    })
                    .unwrap_or(false);
                if !keep {
                    return;
                }
            }
        }));
    }

    /// Keep the session's lease alive; the engine safety-stops a session whose client goes silent.
    fn start_heartbeat(&mut self, id: String, cx: &mut Context<Self>) {
        self._heartbeat = Some(cx.spawn(async move |this: WeakEntity<EngineState>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(HEARTBEAT_INTERVAL).await;
                let Ok(Some(client)) = this.update(cx, |s, _| {
                    (s.session.as_ref().map(|x| x.id.as_str()) == Some(id.as_str())).then(|| s.client.clone())
                }) else {
                    return;
                };
                let beat_id = id.clone();
                let res = cx.background_executor().spawn(async move { client.heartbeat(&beat_id) }).await;
                if let Err(e) = res {
                    let gone = matches!(e, ClientError::Http { status: 404, .. });
                    let _ = this.update(cx, |s, cx| {
                        if gone {
                            s.on_session_idle(cx);
                            cx.emit(EngineNote::Session);
                        } else {
                            s.report(e, cx);
                        }
                    });
                    if gone {
                        return;
                    }
                }
            }
        }));
    }

    // -------------------------------------------------------------- events

    fn on_event(&mut self, ev: EngineEvent, cx: &mut Context<Self>) {
        match &ev {
            EngineEvent::Hello { api_version } => {
                if api_version != API_VERSION {
                    self.status = EngineStatus::Mismatch(api_version.clone());
                    cx.emit(EngineNote::Status);
                } else {
                    // After a reconnect anything may have changed while the stream was down.
                    self.refresh(cx);
                }
            }
            EngineEvent::JointUpdate { ticks } => {
                self.last_ticks = Some(*ticks);
                let ticks = *ticks;
                self.app.update(cx, |a, _| a.push_pose_ticks(ticks));
            }
            EngineEvent::SessionChanged { state, .. } => {
                if state == "idle" {
                    self.on_session_idle(cx);
                } else {
                    self.refresh_session(cx);
                }
            }
            EngineEvent::Disconnected => self.check_engine_gone(cx),
            EngineEvent::CoachingState { .. } => self.refresh_session(cx),
            EngineEvent::JobsChanged { .. } | EngineEvent::JobProgress { .. } => {}
        }
        cx.emit(EngineNote::Event(ev));
        cx.notify();
    }

    /// The event stream dropped: if the engine is really gone, return the links the session was holding.
    fn check_engine_gone(&mut self, cx: &mut Context<Self>) {
        self.run(
            cx,
            |c| c.health().is_err(),
            |s, gone, cx| {
                if gone {
                    s.status = EngineStatus::Failed("the engine stopped answering".into());
                    s.session = None;
                    s._heartbeat = None;
                    s._poll = None;
                    let action = s.handoff.on_engine_gone();
                    s.release_handoff(action, cx);
                    cx.emit(EngineNote::Status);
                    cx.emit(EngineNote::Session);
                    cx.notify();
                }
            },
        );
    }

    /// Short text for the engine status line.
    pub fn status_text(&self, t: impl Fn(&str) -> String) -> String {
        match &self.status {
            EngineStatus::Unknown => t("lab_engine_unknown"),
            EngineStatus::Starting => t("lab_engine_starting"),
            EngineStatus::Ready(h) => {
                t("lab_engine_ready").replace("{version}", &h.version).replace("{api}", &h.api_version)
            }
            EngineStatus::Mismatch(v) => t("lab_engine_mismatch").replace("{engine}", v).replace("{app}", API_VERSION),
            EngineStatus::Failed(e) => format!("{}: {e}", t("lab_engine_failed")),
        }
    }
}

/// Params for a teleoperation session.
pub fn teleop_params() -> Value {
    json!({ "fps": 30 })
}
