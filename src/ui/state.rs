//! Shared application state: connection, device events, log, language, 3D viewer.

use crate::i18n::{Lang, tr};
use crate::kinematics::ArmModel;
use crate::link::{Link, LinkEvent};
use crate::protocol::{self, Frame, Pose, Request, Response, cmd};
use crate::settings::{self, Settings};
use crate::viewer3d::Viewer3d;
use gpui_kit::*;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq)]
pub enum ConnStatus {
    Disconnected,
    Connecting(String),
    Connected(String),
}

/// Events pages subscribe to.
#[derive(Clone, Debug)]
pub enum DeviceEvent {
    /// Parsed reply (also emitted as [`DeviceEvent::Frame`]).
    Response(Response),
    /// Every valid frame, for pages that inspect raw replies.
    Frame(Frame),
    Connected,
    Disconnected,
    LangChanged,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Tx,
    Rx,
    RxBad,
}

pub struct HexLine {
    pub time: String,
    pub dir: Dir,
    pub bytes: Vec<u8>,
}

const LOG_CAP: usize = 2000;

/// Text and hex log, kept in its own entity so logging does not re-render pages.
#[derive(Default)]
pub struct LogStore {
    pub text: VecDeque<String>,
    pub hex: VecDeque<HexLine>,
}

impl LogStore {
    pub fn text(&mut self, line: impl Into<String>, cx: &mut Context<Self>) {
        let line = format!("[{}] {}", now(), line.into().trim_end());
        push_capped(&mut self.text, line);
        cx.notify();
    }
    fn hex(&mut self, dir: Dir, bytes: Vec<u8>, cx: &mut Context<Self>) {
        push_capped(&mut self.hex, HexLine { time: now(), dir, bytes });
        cx.notify();
    }
    pub fn clear_text(&mut self, cx: &mut Context<Self>) {
        self.text.clear();
        cx.notify();
    }
    pub fn clear_hex(&mut self, cx: &mut Context<Self>) {
        self.hex.clear();
        cx.notify();
    }
}

fn push_capped<T>(q: &mut VecDeque<T>, v: T) {
    if q.len() >= LOG_CAP {
        q.pop_front();
    }
    q.push_back(v);
}

/// Local wall-clock time `HH:MM:SS.mmm`.
fn now() -> String {
    chrono::Local::now().format("%H:%M:%S%.3f").to_string()
}

/// Longest unterminated text kept before it is logged anyway.
const TEXT_BUF_MAX: usize = 512;

/// Remove and return the complete, non-blank lines from `buf`, keeping any partial tail.
fn take_lines(buf: &mut String) -> Vec<String> {
    let mut lines = Vec::new();
    while let Some(i) = buf.find('\n') {
        let line: String = buf.drain(..=i).collect();
        if !line.trim().is_empty() {
            lines.push(line.trim_end().to_string());
        }
    }
    if buf.len() > TEXT_BUF_MAX {
        lines.push(std::mem::take(buf));
    }
    lines
}

pub fn hex_string(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02X}")).collect::<Vec<_>>().join(" ")
}

pub struct AppState {
    pub lang: Lang,
    pub settings: Settings,
    pub status: ConnStatus,
    pub pose: Option<Pose>,
    pub log: Entity<LogStore>,
    link: Option<Arc<Link>>,
    events: flume::Sender<LinkEvent>,
    model: Option<Arc<ArmModel>>,
    viewer: Option<Viewer3d>,
    /// Device text not yet terminated by a newline (lines can span several reads).
    text_buf: String,
    /// Receives every valid frame while a synchronizer task owns the link.
    tap: Option<flume::Sender<Frame>>,
    _pump: Task<()>,
}

impl EventEmitter<DeviceEvent> for AppState {}

impl AppState {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let settings = Settings::load();
        let (tx, rx) = flume::unbounded::<LinkEvent>();
        let pump = cx.spawn(async move |this: WeakEntity<AppState>, cx: &mut AsyncApp| {
            while let Ok(first) = rx.recv_async().await {
                let batch: Vec<LinkEvent> = std::iter::once(first).chain(rx.try_iter()).collect();
                if this.update(cx, |s, cx| batch.into_iter().for_each(|e| s.on_link_event(e, cx))).is_err() {
                    break;
                }
            }
        });
        Self {
            lang: Lang::from_code(&settings.lang),
            settings,
            status: ConnStatus::Disconnected,
            pose: None,
            log: cx.new(|_| LogStore::default()),
            link: None,
            events: tx,
            model: None,
            viewer: None,
            text_buf: String::new(),
            tap: None,
            _pump: pump,
        }
    }

    pub fn t(&self, key: &str) -> String {
        tr(self.lang, key)
    }

    pub fn is_connected(&self) -> bool {
        matches!(self.status, ConnStatus::Connected(_))
    }

    pub fn log_text(&self, line: impl Into<String>, cx: &mut Context<Self>) {
        let line = line.into();
        self.log.update(cx, |l, cx| l.text(line, cx));
    }

    pub fn toggle_lang(&mut self, cx: &mut Context<Self>) {
        self.lang = self.lang.toggled();
        self.settings.lang = self.lang.code().into();
        self.persist();
        cx.emit(DeviceEvent::LangChanged);
        cx.notify();
    }

    pub fn persist(&self) {
        if let Err(e) = self.settings.save() {
            eprintln!("settings save failed: {e}");
        }
    }

    // ------------------------------------------------------------ sending

    /// Send one request; silently ignored while disconnected (like the original).
    pub fn send(&self, req: Request, cx: &mut Context<Self>) {
        let Some(link) = &self.link else { return };
        if let Err(e) = link.send(&req) {
            self.log_text(format!("TX Error: {e}"), cx);
        }
    }

    pub fn send_sys(&self, command: u8, args: impl Into<Vec<u8>>, cx: &mut Context<Self>) {
        self.send(Request::sys(command, args), cx);
    }

    /// Send requests one after another with `gap` between them, off the UI thread.
    pub fn send_paced(&self, reqs: Vec<Request>, gap: Duration, cx: &mut Context<Self>) {
        let Some(link) = self.link.clone() else { return };
        let exec = cx.background_executor().clone();
        cx.background_executor()
            .spawn(async move {
                for (i, r) in reqs.iter().enumerate() {
                    if i > 0 {
                        exec.timer(gap).await;
                    }
                    if link.send(r).is_err() {
                        break;
                    }
                }
            })
            .detach();
    }

    /// Hand the link to a synchronizer task. Until the returned receiver is dropped, valid
    /// frames go only to it (they are still hex-logged) and are not parsed for the pages:
    /// raw servo register replies would otherwise be mistaken for position replies.
    /// The tap is dropped on disconnect, which makes the task stop.
    pub fn open_frame_tap(&mut self) -> Option<(Arc<Link>, flume::Receiver<Frame>)> {
        let link = self.link.clone()?;
        let (tx, rx) = flume::unbounded();
        self.tap = Some(tx);
        Some((link, rx))
    }

    // --------------------------------------------------------- connection

    pub fn connect_serial(&mut self, port: String, baud: u32, cx: &mut Context<Self>) {
        if self.link.is_some() {
            return;
        }
        self.settings.serial_port = Some(port.clone());
        self.settings.baud = baud;
        self.persist();
        let events = self.events.clone();
        self.open_with(format!("{port} @ {baud}"), cx, move || Link::open_serial(&port, baud, events));
    }

    pub fn connect_tcp(&mut self, host: String, port: u16, cx: &mut Context<Self>) {
        if self.link.is_some() {
            return;
        }
        let events = self.events.clone();
        self.open_with(format!("{host}:{port}"), cx, move || Link::open_tcp(&host, port, events));
    }

    /// Join the robot's access point, wait for it to answer, then open TCP.
    pub fn connect_wifi_ap(&mut self, ssid: String, password: String, cx: &mut Context<Self>) {
        if self.link.is_some() {
            return;
        }
        self.settings.last_ssid = ssid.clone();
        self.persist();
        if !password.is_empty()
            && let Err(e) = settings::save_wifi_password(&ssid, &password) {
                self.log_text(format!("Keychain: {e}"), cx);
            }
        let (host, port) = (self.settings.wifi_host.clone(), self.settings.wifi_port);
        let events = self.events.clone();
        self.open_with(format!("{ssid} → {host}:{port}"), cx, move || {
            // Skip joining when the robot already answers (e.g. the AP was joined by hand).
            if !crate::wifi::wait_for_host(&host, port, 1) {
                crate::wifi::join(&ssid, &password)?;
                if !crate::wifi::wait_for_host(&host, port, 20) {
                    anyhow::bail!("robot at {host}:{port} did not answer after joining {ssid}");
                }
            }
            Link::open_tcp(&host, port, events)
        });
    }

    fn open_with(
        &mut self,
        label: String,
        cx: &mut Context<Self>,
        open: impl FnOnce() -> anyhow::Result<Link> + Send + 'static,
    ) {
        self.status = ConnStatus::Connecting(label);
        cx.notify();
        let task = cx.background_executor().spawn(async move { open() });
        cx.spawn(async move |this: WeakEntity<AppState>, cx: &mut AsyncApp| {
            let res = task.await;
            let _ = this.update(cx, |s, cx| s.on_opened(res, cx));
        })
        .detach();
    }

    fn on_opened(&mut self, res: anyhow::Result<Link>, cx: &mut Context<Self>) {
        match res {
            Ok(link) => {
                let desc = link.description.clone();
                self.link = Some(Arc::new(link));
                self.status = ConnStatus::Connected(desc.clone());
                self.log_text(format!("Info: Connected to {desc}"), cx);
                // Same start-up reads as the original: servo offsets 1..6, 50 ms apart.
                let reqs = (1..=6).map(|id| Request::sys(cmd::GET_POS_OFFSET, [id])).collect();
                self.send_paced(reqs, Duration::from_millis(50), cx);
                cx.emit(DeviceEvent::Connected);
            }
            Err(e) => {
                self.status = ConnStatus::Disconnected;
                self.log_text(format!("Connection Error: {e:#}"), cx);
            }
        }
        cx.notify();
    }

    pub fn disconnect(&mut self, cx: &mut Context<Self>) {
        if let Some(link) = self.link.take() {
            link.close();
        }
        self.mark_disconnected(cx);
    }

    /// Log any unterminated device text and announce the disconnect once.
    fn mark_disconnected(&mut self, cx: &mut Context<Self>) {
        self.tap = None;
        let tail = std::mem::take(&mut self.text_buf);
        if !tail.trim().is_empty() {
            self.log_text(tail, cx);
        }
        if self.status != ConnStatus::Disconnected {
            self.status = ConnStatus::Disconnected;
            cx.emit(DeviceEvent::Disconnected);
            cx.notify();
        }
    }

    fn on_link_event(&mut self, ev: LinkEvent, cx: &mut Context<Self>) {
        match ev {
            LinkEvent::Tx(b) => self.log.update(cx, |l, cx| l.hex(Dir::Tx, b, cx)),
            LinkEvent::BadFrame(b) => self.log.update(cx, |l, cx| l.hex(Dir::RxBad, b, cx)),
            LinkEvent::Text(t) => {
                self.text_buf.push_str(&t);
                for line in take_lines(&mut self.text_buf) {
                    self.log_text(line, cx);
                }
            }
            LinkEvent::Frame(mut f) => {
                self.log.update(cx, |l, cx| l.hex(Dir::Rx, f.raw.clone(), cx));
                if let Some(tap) = self.tap.take() {
                    match tap.send(f) {
                        Ok(()) => {
                            self.tap = Some(tap);
                            return;
                        }
                        // The task finished; handle the frame normally from now on.
                        Err(flume::SendError(back)) => f = back,
                    }
                }
                if let Some(resp) = protocol::parse(&f) {
                    if let Response::Pose(p) = &resp {
                        self.pose = Some(p.clone());
                        self.update_viewer(p.servos);
                        cx.notify();
                    }
                    cx.emit(DeviceEvent::Response(resp));
                }
                cx.emit(DeviceEvent::Frame(f));
            }
            // `None` means we closed it ourselves (possibly an old link after a reconnect).
            LinkEvent::Closed(None) => {}
            LinkEvent::Closed(Some(reason)) => {
                self.link = None;
                self.mark_disconnected(cx);
                self.log_text(format!("Connection Error: {reason}"), cx);
            }
        }
    }

    // ---------------------------------------------------------- 3D viewer

    /// Spawn (or re-spawn) the Rerun viewer and stream the current pose to it.
    /// Loading meshes and spawning the viewer process block, so both run off the UI thread.
    pub fn open_3d_viewer(&mut self, cx: &mut Context<Self>) {
        let model = self.model.clone();
        let task = cx.background_executor().spawn(async move {
            let model = match model {
                Some(m) => m,
                None => Arc::new(ArmModel::load(&crate::assets_dir())?),
            };
            let viewer = Viewer3d::open(&model)?;
            anyhow::Ok((model, viewer))
        });
        cx.spawn(async move |this: WeakEntity<AppState>, cx: &mut AsyncApp| {
            let res = task.await;
            let _ = this.update(cx, |s, cx| s.on_viewer_opened(res, cx));
        })
        .detach();
    }

    fn on_viewer_opened(&mut self, res: anyhow::Result<(Arc<ArmModel>, Viewer3d)>, cx: &mut Context<Self>) {
        match res {
            Ok((model, v)) => {
                self.model = Some(model);
                self.viewer = Some(v);
                let servos = self.pose.as_ref().map(|p| p.servos).unwrap_or([2048, 2048, 1198, 1238, 2048, 2048]);
                self.update_viewer(servos);
                if self.is_connected() {
                    self.send_sys(cmd::GET_CUR_COORDS, [], cx);
                }
            }
            Err(e) => {
                self.log_text(format!("3D viewer: {e:#}"), cx);
                window_note(cx, format!("{e:#}"));
            }
        }
    }

    fn update_viewer(&self, servos: [i16; 6]) {
        if let (Some(v), Some(m)) = (&self.viewer, &self.model) {
            v.update(m, servos);
        }
    }
}

/// Surface an error to the user via any open window's notification area.
fn window_note(cx: &mut App, msg: String) {
    use gpui_kit::component::WindowExt as _;
    if let Some(w) = cx.windows().first().copied() {
        let _ = w.update(cx, |_, window, cx| {
            window.push_notification(gpui_kit::component::notification::Notification::error(msg), cx)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::take_lines;

    #[test]
    fn lines_split_across_reads_are_joined() {
        let mut buf = String::from("[cal] X off");
        assert!(take_lines(&mut buf).is_empty());
        buf.push_str(":3\r\n\r\nnext");
        assert_eq!(take_lines(&mut buf), vec!["[cal] X off:3"]);
        assert_eq!(buf, "next");
        buf.push_str(&"x".repeat(600));
        assert_eq!(take_lines(&mut buf).len(), 1);
        assert!(buf.is_empty());
    }
}
