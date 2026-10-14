//! The engine's event stream (`WS /api/v1/ws/events`) on a background thread.
//!
//! Events are refetch hints, not state: pages re-read what they show when told something changed.
//! The schema is in the engine's `docs/service/events.json`.

use serde_json::Value;
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tungstenite::{Message, WebSocket, stream::MaybeTlsStream};

pub const JOINT_NAMES: [&str; 6] = ["shoulder_pan", "shoulder_lift", "elbow_flex", "wrist_flex", "wrist_roll", "gripper"];

#[derive(Clone, Debug, PartialEq)]
pub enum EngineEvent {
    /// Connected to the stream (also sent again after every reconnect).
    Hello { api_version: String },
    /// Raw servo ticks in [`JOINT_NAMES`] order.
    JointUpdate { ticks: [f64; 6] },
    SessionChanged { id: String, kind: Option<String>, state: String, stop_reason: Option<String>, error: Option<String> },
    JobsChanged { kind: String, id: String, state: String },
    JobProgress { id: String, step: u32, total_steps: u32, loss: Option<f64> },
    CoachingState { phase: String, intervening: bool, corrections: u32 },
    /// The stream dropped; the listener is reconnecting.
    Disconnected,
}

impl EngineEvent {
    /// True for the `session_changed` that means the arm is released and the slot is free.
    pub fn is_session_idle(&self) -> bool {
        matches!(self, EngineEvent::SessionChanged { state, .. } if state == "idle")
    }
}

fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

/// Decode one `{"type": ..., "data": {...}}` message; `None` for unknown or malformed events.
pub fn parse_event(text: &str) -> Option<EngineEvent> {
    let msg: Value = serde_json::from_str(text).ok()?;
    let data = msg.get("data")?;
    Some(match msg.get("type")?.as_str()? {
        "hello" => EngineEvent::Hello { api_version: str_field(data, "api_version")? },
        "joint_update" => {
            let ticks = data.get("ticks")?;
            let mut out = [0.0; 6];
            for (slot, name) in out.iter_mut().zip(JOINT_NAMES) {
                *slot = ticks.get(name)?.as_f64()?;
            }
            EngineEvent::JointUpdate { ticks: out }
        }
        "session_changed" => EngineEvent::SessionChanged {
            id: str_field(data, "id").unwrap_or_default(),
            kind: str_field(data, "kind"),
            state: str_field(data, "state")?,
            stop_reason: str_field(data, "stop_reason"),
            error: str_field(data, "error"),
        },
        "jobs_changed" => EngineEvent::JobsChanged {
            kind: str_field(data, "kind")?,
            id: str_field(data, "id")?,
            state: str_field(data, "state")?,
        },
        "job_progress" => EngineEvent::JobProgress {
            id: str_field(data, "id")?,
            step: data.get("step")?.as_u64()? as u32,
            total_steps: data.get("total_steps")?.as_u64()? as u32,
            loss: data.get("loss").and_then(Value::as_f64),
        },
        "coaching_state" => EngineEvent::CoachingState {
            phase: str_field(data, "phase")?,
            intervening: data.get("intervening").and_then(Value::as_bool).unwrap_or(false),
            corrections: data.get("corrections").and_then(Value::as_u64).unwrap_or(0) as u32,
        },
        _ => return None,
    })
}

/// Handle to a running listener; dropping it stops the thread.
pub struct EventListener {
    stop: Arc<AtomicBool>,
}

impl Drop for EventListener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

const READ_POLL: Duration = Duration::from_millis(250);
const RECONNECT_MIN: Duration = Duration::from_secs(1);
const RECONNECT_MAX: Duration = Duration::from_secs(5);

/// The WebSocket upgrade request with `Authorization: Bearer <token>`. The token goes in a header, not the
/// URL, because query strings end up in access logs.
pub fn authed_request(url: &str, token: Option<&str>) -> Option<tungstenite::handshake::client::Request> {
    use tungstenite::client::IntoClientRequest as _;
    let mut request = url.into_client_request().ok()?;
    if let Some(t) = token.filter(|t| !t.is_empty()) {
        request.headers_mut().insert("Authorization", format!("Bearer {t}").parse().ok()?);
    }
    Some(request)
}

/// Listen to `url` and forward parsed events, reconnecting with a bounded backoff until dropped.
pub fn listen(url: String, token: Option<String>, out: flume::Sender<EngineEvent>) -> EventListener {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    std::thread::Builder::new()
        .name("nexarm-engine-events".into())
        .spawn(move || {
            let mut backoff = RECONNECT_MIN;
            while !flag.load(Ordering::Relaxed) {
                if let Some(request) = authed_request(&url, token.as_deref())
                    && let Ok((mut ws, _)) = tungstenite::connect(request)
                {
                    backoff = RECONNECT_MIN;
                    set_poll_timeout(&mut ws);
                    if !pump(&mut ws, &flag, &out) || out.send(EngineEvent::Disconnected).is_err() {
                        return;
                    }
                }
                sleep_unless_stopped(backoff, &flag);
                backoff = (backoff * 2).min(RECONNECT_MAX);
            }
        })
        .expect("spawn engine event thread");
    EventListener { stop }
}

fn set_poll_timeout(ws: &mut WebSocket<MaybeTlsStream<TcpStream>>) {
    if let MaybeTlsStream::Plain(s) = ws.get_mut() {
        let _ = s.set_read_timeout(Some(READ_POLL));
    }
}

/// Read until the stream drops (returns `true`) or the receiver or listener is gone (`false`).
fn pump(ws: &mut WebSocket<MaybeTlsStream<TcpStream>>, stop: &AtomicBool, out: &flume::Sender<EngineEvent>) -> bool {
    while !stop.load(Ordering::Relaxed) {
        match ws.read() {
            Ok(Message::Text(t)) => {
                if let Some(ev) = parse_event(t.as_str())
                    && out.send(ev).is_err()
                {
                    return false;
                }
            }
            Ok(Message::Close(_)) => return true,
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(_) => return true,
        }
    }
    false
}

fn sleep_unless_stopped(total: Duration, stop: &AtomicBool) {
    let step = Duration::from_millis(100);
    let mut waited = Duration::ZERO;
    while waited < total && !stop.load(Ordering::Relaxed) {
        std::thread::sleep(step);
        waited += step;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_documented_event_parses() {
        assert_eq!(
            parse_event(r#"{"type":"hello","data":{"api_version":"1"}}"#),
            Some(EngineEvent::Hello { api_version: "1".into() })
        );
        let joints = r#"{"type":"joint_update","data":{"ticks":{"shoulder_pan":2048,"shoulder_lift":2000.5,"elbow_flex":1,"wrist_flex":2,"wrist_roll":3,"gripper":2833},"radians":{}}}"#;
        assert_eq!(
            parse_event(joints),
            Some(EngineEvent::JointUpdate { ticks: [2048.0, 2000.5, 1.0, 2.0, 3.0, 2833.0] })
        );
        let changed = parse_event(r#"{"type":"session_changed","data":{"id":"ab","kind":"teleoperation","state":"running","stop_reason":null,"error":null}}"#);
        assert!(matches!(changed, Some(EngineEvent::SessionChanged { ref state, .. }) if state == "running"));
        let idle = parse_event(r#"{"type":"session_changed","data":{"id":"ab","state":"idle"}}"#).unwrap();
        assert!(idle.is_session_idle());
        assert_eq!(
            parse_event(r#"{"type":"jobs_changed","data":{"kind":"train","id":"j1","state":"running"}}"#),
            Some(EngineEvent::JobsChanged { kind: "train".into(), id: "j1".into(), state: "running".into() })
        );
        assert_eq!(
            parse_event(r#"{"type":"job_progress","data":{"id":"j1","step":200,"total_steps":1000,"loss":0.5,"lr":1e-5}}"#),
            Some(EngineEvent::JobProgress { id: "j1".into(), step: 200, total_steps: 1000, loss: Some(0.5) })
        );
        assert_eq!(
            parse_event(r#"{"type":"coaching_state","data":{"phase":"autonomous","intervening":false,"corrections":2}}"#),
            Some(EngineEvent::CoachingState { phase: "autonomous".into(), intervening: false, corrections: 2 })
        );
    }

    #[test]
    fn unknown_and_malformed_events_are_ignored() {
        assert_eq!(parse_event(r#"{"type":"nope","data":{}}"#), None);
        assert_eq!(parse_event("not json"), None);
        assert_eq!(parse_event(r#"{"type":"joint_update","data":{"ticks":{"shoulder_pan":1}}}"#), None);
    }

    /// The event names in the engine's committed schema are exactly the ones this client decodes.
    #[test]
    fn schema_snapshot_matches_the_decoded_events() {
        let schema: Value = serde_json::from_str(include_str!("../../tests/fixtures/events.json")).unwrap();
        let mut names: Vec<&str> = schema.as_object().unwrap().keys().map(String::as_str).collect();
        names.sort_unstable();
        assert_eq!(names, ["coaching_state", "hello", "job_progress", "jobs_changed", "joint_update", "session_changed"]);
    }

    #[test]
    fn the_token_goes_in_an_authorization_header_never_the_url() {
        let r = authed_request("ws://127.0.0.1:8765/api/v1/ws/events", Some("s3cret")).unwrap();
        assert_eq!(r.headers().get("Authorization").unwrap(), "Bearer s3cret");
        assert!(!r.uri().to_string().contains("s3cret"));
        let plain = authed_request("ws://127.0.0.1:8765/api/v1/ws/events", None).unwrap();
        assert!(plain.headers().get("Authorization").is_none());
        assert!(authed_request("not a url", Some("t")).is_none());
    }
}
