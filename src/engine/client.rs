//! Blocking HTTP client for the engine. Every call runs on a background thread (never the UI thread).
//!
//! Each route the app uses is declared once in [`routes`]; `tests/engine_contract.rs` checks that the
//! list is a subset of the engine's committed `openapi.json`, so a renamed route fails `cargo test`.

use super::api::*;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::fmt;
use std::time::Duration;

/// One HTTP route: method and path template with `{placeholders}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Route {
    pub method: &'static str,
    pub path: &'static str,
}

macro_rules! define_routes {
    ($($name:ident = $method:literal $path:literal;)*) => {
        /// Every engine route this client calls.
        pub mod routes {
            use super::Route;
            $(pub const $name: Route = Route { method: $method, path: $path };)*
            pub const ALL: &[Route] = &[$($name),*];
        }
    };
}

define_routes! {
    HEALTH = "GET" "/api/v1/health";
    SHUTDOWN = "POST" "/api/v1/shutdown";
    TEST_SERVER = "POST" "/api/v1/servers/test";
    PROFILES = "GET" "/api/v1/profiles";
    PUT_PROFILE = "PUT" "/api/v1/profiles/{name}";
    DELETE_PROFILE = "DELETE" "/api/v1/profiles/{name}";
    PORTS = "GET" "/api/v1/ports";
    CAMERAS = "GET" "/api/v1/cameras";
    START_SESSION = "POST" "/api/v1/sessions";
    CURRENT_SESSION = "GET" "/api/v1/sessions/current";
    HEARTBEAT = "POST" "/api/v1/sessions/{session_id}/heartbeat";
    CONTROL = "POST" "/api/v1/sessions/{session_id}/control";
    STOP_SESSION = "DELETE" "/api/v1/sessions/{session_id}";
    CALIBRATION_CHECK = "POST" "/api/v1/calibration/check";
    DATASETS = "GET" "/api/v1/datasets";
    DATASETS_HUB = "GET" "/api/v1/datasets/hub";
    DATASETS_IMPORT = "POST" "/api/v1/datasets/import";
    DATASETS_MERGE = "POST" "/api/v1/datasets/merge";
    DATASET = "GET" "/api/v1/datasets/{owner}/{name}";
    EPISODES = "GET" "/api/v1/datasets/{owner}/{name}/episodes";
    JOINTS = "GET" "/api/v1/datasets/{owner}/{name}/episodes/{episode}/joints";
    FRAME = "GET" "/api/v1/datasets/{owner}/{name}/episodes/{episode}/frames/{camera}/{frame}";
    EXCLUDED = "GET" "/api/v1/datasets/{owner}/{name}/excluded";
    SET_EXCLUDED = "PUT" "/api/v1/datasets/{owner}/{name}/excluded";
    DELETE_EPISODES = "POST" "/api/v1/datasets/{owner}/{name}/delete-episodes";
    DROP_EXCLUDED = "POST" "/api/v1/datasets/{owner}/{name}/drop-excluded";
    SPLIT = "POST" "/api/v1/datasets/{owner}/{name}/split";
    MODIFY_FEATURES = "POST" "/api/v1/datasets/{owner}/{name}/modify-features";
    RENAME = "POST" "/api/v1/datasets/{owner}/{name}/rename";
    UPLOAD = "POST" "/api/v1/datasets/{owner}/{name}/upload";
    OPERATIONS = "GET" "/api/v1/operations";
    OPERATION = "GET" "/api/v1/operations/{op_id}";
    SUBMIT_JOB = "POST" "/api/v1/jobs";
    JOBS = "GET" "/api/v1/jobs";
    JOB = "GET" "/api/v1/jobs/{job_id}";
    CANCEL_JOB = "DELETE" "/api/v1/jobs/{job_id}";
    JOB_LOGS = "GET" "/api/v1/jobs/{job_id}/logs";
    JOB_METRICS = "GET" "/api/v1/jobs/{job_id}/metrics";
    JOB_CHECKPOINTS = "GET" "/api/v1/jobs/{job_id}/checkpoints";
    MODELS = "GET" "/api/v1/models";
    MODELS_HUB = "GET" "/api/v1/models/hub";
    MODELS_IMPORT = "POST" "/api/v1/models/import";
}

#[derive(Debug)]
pub enum ClientError {
    /// The engine could not be reached, or the reply was not understood.
    Transport(String),
    /// The engine answered with an error status; `detail` is its message.
    Http { status: u16, detail: String },
}

impl ClientError {
    pub fn status(&self) -> Option<u16> {
        match self {
            ClientError::Http { status, .. } => Some(*status),
            ClientError::Transport(_) => None,
        }
    }
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClientError::Transport(m) => write!(f, "engine unreachable: {m}"),
            ClientError::Http { status, detail } => write!(f, "{detail} (HTTP {status})"),
        }
    }
}

impl std::error::Error for ClientError {}

pub type Result<T> = std::result::Result<T, ClientError>;

/// Replace `{name}` placeholders in a route template with percent-encoded values.
pub fn fill_path(template: &str, params: &[(&str, &str)]) -> String {
    let mut path = template.to_string();
    for (key, value) in params {
        path = path.replace(&format!("{{{key}}}"), &encode_segment(value));
    }
    path
}

fn encode_segment(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Split `owner/name` as used in dataset URLs.
pub fn split_repo(repo_id: &str) -> (&str, &str) {
    repo_id.split_once('/').unwrap_or(("", repo_id))
}

#[derive(Clone)]
pub struct EngineClient {
    base: String,
    token: Option<String>,
    short: ureq::Agent,
    /// For calls that block on hardware or a policy load (session start and stop).
    long: ureq::Agent,
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(timeout)).http_status_as_error(false).build().into()
}

impl EngineClient {
    pub fn new(base: impl Into<String>) -> Self {
        Self {
            base: base.into().trim_end_matches('/').to_string(),
            token: None,
            short: agent(Duration::from_secs(15)),
            long: agent(Duration::from_secs(900)),
        }
    }

    /// Send `Authorization: Bearer <token>` on every call (an engine reached beyond loopback requires it).
    pub fn with_token(mut self, token: &str) -> Self {
        self.token = (!token.trim().is_empty()).then(|| token.trim().to_string());
        self
    }

    fn auth<B>(&self, req: ureq::RequestBuilder<B>) -> ureq::RequestBuilder<B> {
        match &self.token {
            Some(t) => req.header("Authorization", format!("Bearer {t}")),
            None => req,
        }
    }

    /// GET a URL with query string already in it; an error status becomes [`ClientError::Http`].
    fn raw_get(&self, url: &str) -> Result<ureq::http::Response<ureq::Body>> {
        let mut resp = self.auth(self.short.get(url)).call().map_err(|e| ClientError::Transport(e.to_string()))?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let text = resp.body_mut().read_to_string().unwrap_or_default();
            return Err(ClientError::Http { status, detail: error_detail(&text) });
        }
        Ok(resp)
    }

    fn get_text(&self, url: &str) -> Result<String> {
        self.raw_get(url)?.body_mut().read_to_string().map_err(|e| ClientError::Transport(e.to_string()))
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    /// WebSocket URL of the event stream.
    pub fn events_url(&self) -> String {
        let ws = self.base.replacen("http://", "ws://", 1).replacen("https://", "wss://", 1);
        format!("{ws}/api/v1/ws/events")
    }

    /// The bearer token, for the event stream (which sends it as a header).
    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    fn send(&self, agent: &ureq::Agent, route: Route, params: &[(&str, &str)], body: Option<Value>) -> Result<ureq::http::Response<ureq::Body>> {
        let url = format!("{}{}", self.base, fill_path(route.path, params));
        let transport = |e: ureq::Error| ClientError::Transport(e.to_string());
        let mut resp = match (route.method, body) {
            ("GET", _) => self.auth(agent.get(&url)).call(),
            ("DELETE", _) => self.auth(agent.delete(&url)).call(),
            ("POST", Some(b)) => self.auth(agent.post(&url)).send_json(&b),
            ("POST", None) => self.auth(agent.post(&url)).send_empty(),
            ("PUT", Some(b)) => self.auth(agent.put(&url)).send_json(&b),
            (m, _) => return Err(ClientError::Transport(format!("unsupported call {m} {}", route.path))),
        }
        .map_err(transport)?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let text = resp.body_mut().read_to_string().unwrap_or_default();
            return Err(ClientError::Http { status, detail: error_detail(&text) });
        }
        Ok(resp)
    }

    fn json<T: DeserializeOwned>(&self, route: Route, params: &[(&str, &str)], body: Option<Value>) -> Result<T> {
        self.json_with(&self.short, route, params, body)
    }

    fn json_with<T: DeserializeOwned>(
        &self,
        agent: &ureq::Agent,
        route: Route,
        params: &[(&str, &str)],
        body: Option<Value>,
    ) -> Result<T> {
        let mut resp = self.send(agent, route, params, body)?;
        resp.body_mut().read_json::<T>().map_err(|e| ClientError::Transport(format!("bad reply: {e}")))
    }

    // ---------------------------------------------------------------- system
    pub fn health(&self) -> Result<Health> {
        self.json(routes::HEALTH, &[], None)
    }

    pub fn shutdown(&self) -> Result<()> {
        self.send(&self.short, routes::SHUTDOWN, &[], None).map(|_| ())
    }

    /// Open the SSH tunnel to a GPU server and ask its engine for its version. Starts nothing remotely.
    pub fn test_server(&self, server: &RemoteServer) -> Result<ServerTest> {
        let body = serde_json::to_value(server).map_err(|e| ClientError::Transport(e.to_string()))?;
        self.json_with(&self.long, routes::TEST_SERVER, &[], Some(body))
    }

    pub fn profiles(&self) -> Result<Vec<RobotProfile>> {
        self.json(routes::PROFILES, &[], None)
    }

    pub fn put_profile(&self, profile: &RobotProfile) -> Result<RobotProfile> {
        let body = serde_json::to_value(profile).map_err(|e| ClientError::Transport(e.to_string()))?;
        self.json(routes::PUT_PROFILE, &[("name", &profile.name)], Some(body))
    }

    pub fn delete_profile(&self, name: &str) -> Result<()> {
        self.send(&self.short, routes::DELETE_PROFILE, &[("name", name)], None).map(|_| ())
    }

    pub fn ports(&self) -> Result<Vec<PortInfo>> {
        self.json(routes::PORTS, &[], None)
    }

    pub fn cameras(&self) -> Result<Vec<Value>> {
        self.json(routes::CAMERAS, &[], None)
    }

    pub fn calibration_check(&self, profile: &str) -> Result<CalibrationReport> {
        self.json_with(&self.long, routes::CALIBRATION_CHECK, &[], Some(json!({ "profile": profile })))
    }

    // -------------------------------------------------------------- sessions
    pub fn start_session(&self, kind: &str, profile: &str, params: Value) -> Result<SessionInfo> {
        let body = json!({ "kind": kind, "profile": profile, "params": params });
        self.json_with(&self.long, routes::START_SESSION, &[], Some(body))
    }

    pub fn current_session(&self) -> Result<Option<SessionInfo>> {
        self.json(routes::CURRENT_SESSION, &[], None)
    }

    pub fn heartbeat(&self, session_id: &str) -> Result<SessionInfo> {
        self.json(routes::HEARTBEAT, &[("session_id", session_id)], None)
    }

    pub fn control(&self, session_id: &str, action: &str, payload: Value) -> Result<Value> {
        let body = json!({ "action": action, "payload": payload });
        self.json(routes::CONTROL, &[("session_id", session_id)], Some(body))
    }

    pub fn stop_session(&self, session_id: &str) -> Result<SessionInfo> {
        self.json_with(&self.long, routes::STOP_SESSION, &[("session_id", session_id)], None)
    }

    // -------------------------------------------------------------- datasets
    pub fn datasets(&self) -> Result<Vec<DatasetSummary>> {
        self.json(routes::DATASETS, &[], None)
    }

    pub fn dataset_info(&self, repo_id: &str) -> Result<Value> {
        let (owner, name) = split_repo(repo_id);
        self.json(routes::DATASET, &[("owner", owner), ("name", name)], None)
    }

    pub fn episodes(&self, repo_id: &str) -> Result<Vec<EpisodeSummary>> {
        let (owner, name) = split_repo(repo_id);
        self.json(routes::EPISODES, &[("owner", owner), ("name", name)], None)
    }

    pub fn joints(&self, repo_id: &str, episode: u32) -> Result<JointsData> {
        let (owner, name) = split_repo(repo_id);
        let ep = episode.to_string();
        self.json(routes::JOINTS, &[("owner", owner), ("name", name), ("episode", &ep)], None)
    }

    /// One decoded video frame as JPEG bytes.
    pub fn frame_jpeg(&self, repo_id: &str, episode: u32, camera: &str, frame: u32, width: u32) -> Result<Vec<u8>> {
        let (owner, name) = split_repo(repo_id);
        let (ep, fr) = (episode.to_string(), frame.to_string());
        let params = [("owner", owner), ("name", name), ("episode", ep.as_str()), ("camera", camera), ("frame", fr.as_str())];
        let url = format!("{}{}?width={width}", self.base, fill_path(routes::FRAME.path, &params));
        let mut resp = self.raw_get(&url)?;
        resp.body_mut().read_to_vec().map_err(|e| ClientError::Transport(e.to_string()))
    }

    pub fn excluded(&self, repo_id: &str) -> Result<Vec<u32>> {
        let (owner, name) = split_repo(repo_id);
        self.json(routes::EXCLUDED, &[("owner", owner), ("name", name)], None)
    }

    pub fn set_excluded(&self, repo_id: &str, episodes: &[u32]) -> Result<Vec<u32>> {
        let (owner, name) = split_repo(repo_id);
        self.json(routes::SET_EXCLUDED, &[("owner", owner), ("name", name)], Some(json!({ "episodes": episodes })))
    }

    fn edit(&self, route: Route, repo_id: &str, body: Value) -> Result<Operation> {
        let (owner, name) = split_repo(repo_id);
        self.json(route, &[("owner", owner), ("name", name)], Some(body))
    }

    pub fn delete_episodes(&self, repo_id: &str, episodes: &[u32], output: &str) -> Result<Operation> {
        self.edit(routes::DELETE_EPISODES, repo_id, json!({ "episodes": episodes, "output_repo_id": output }))
    }

    pub fn drop_excluded(&self, repo_id: &str, output: &str) -> Result<Operation> {
        self.edit(routes::DROP_EXCLUDED, repo_id, json!({ "output_repo_id": output }))
    }

    pub fn split(&self, repo_id: &str, splits: Value) -> Result<Operation> {
        self.edit(routes::SPLIT, repo_id, json!({ "splits": splits }))
    }

    pub fn remove_features(&self, repo_id: &str, output: &str, remove: &[String]) -> Result<Operation> {
        self.edit(routes::MODIFY_FEATURES, repo_id, json!({ "output_repo_id": output, "remove": remove }))
    }

    pub fn rename_dataset(&self, repo_id: &str, new_repo_id: &str) -> Result<Value> {
        let (owner, name) = split_repo(repo_id);
        self.json(routes::RENAME, &[("owner", owner), ("name", name)], Some(json!({ "output_repo_id": new_repo_id })))
    }

    pub fn upload_dataset(&self, repo_id: &str, private: bool) -> Result<Operation> {
        self.edit(routes::UPLOAD, repo_id, json!({ "private": private }))
    }

    pub fn merge_datasets(&self, repo_ids: &[String], output: &str) -> Result<Operation> {
        self.json(routes::DATASETS_MERGE, &[], Some(json!({ "repo_ids": repo_ids, "output_repo_id": output })))
    }

    pub fn import_dataset(&self, repo_id: &str) -> Result<Operation> {
        self.json(routes::DATASETS_IMPORT, &[], Some(json!({ "repo_id": repo_id })))
    }

    pub fn hub_datasets(&self, author: &str) -> Result<Vec<Value>> {
        let url = format!("{}{}?author={}", self.base, routes::DATASETS_HUB.path, encode_segment(author));
        let text = self.get_text(&url)?;
        serde_json::from_str(&text).map_err(|e| ClientError::Transport(format!("bad reply: {e}")))
    }

    pub fn operations(&self) -> Result<Vec<Operation>> {
        self.json(routes::OPERATIONS, &[], None)
    }

    pub fn operation(&self, op_id: &str) -> Result<Operation> {
        self.json(routes::OPERATION, &[("op_id", op_id)], None)
    }

    // ------------------------------------------------------------------ jobs
    pub fn submit_job(&self, request: &TrainRequest) -> Result<Job> {
        let body = serde_json::to_value(request).map_err(|e| ClientError::Transport(e.to_string()))?;
        self.json(routes::SUBMIT_JOB, &[], Some(body))
    }

    pub fn jobs(&self) -> Result<Vec<Job>> {
        self.json(routes::JOBS, &[], None)
    }

    pub fn job(&self, job_id: &str) -> Result<Job> {
        self.json(routes::JOB, &[("job_id", job_id)], None)
    }

    pub fn cancel_job(&self, job_id: &str) -> Result<Job> {
        self.json(routes::CANCEL_JOB, &[("job_id", job_id)], None)
    }

    pub fn job_logs(&self, job_id: &str, tail: u32) -> Result<Vec<String>> {
        let url = format!("{}{}?tail={tail}", self.base, fill_path(routes::JOB_LOGS.path, &[("job_id", job_id)]));
        let text = self.get_text(&url)?;
        serde_json::from_str(&text).map_err(|e| ClientError::Transport(format!("bad reply: {e}")))
    }

    pub fn job_metrics(&self, job_id: &str) -> Result<Vec<MetricPoint>> {
        self.json(routes::JOB_METRICS, &[("job_id", job_id)], None)
    }

    pub fn job_checkpoints(&self, job_id: &str) -> Result<Vec<Checkpoint>> {
        self.json(routes::JOB_CHECKPOINTS, &[("job_id", job_id)], None)
    }

    // ---------------------------------------------------------------- models
    pub fn models(&self) -> Result<Vec<ModelInfo>> {
        self.json(routes::MODELS, &[], None)
    }

    pub fn import_model(&self, repo_id: &str) -> Result<Operation> {
        self.json(routes::MODELS_IMPORT, &[], Some(json!({ "repo_id": repo_id })))
    }
}

/// The engine's `{"detail": ...}` error body as readable text.
pub fn error_detail(body: &str) -> String {
    match serde_json::from_str::<Value>(body) {
        Ok(v) => match v.get("detail") {
            Some(Value::String(s)) => s.clone(),
            Some(other) => other.to_string(),
            None => body.trim().to_string(),
        },
        Err(_) => body.trim().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_fill_and_encode_placeholders() {
        assert_eq!(
            fill_path(routes::FRAME.path, &[("owner", "me"), ("name", "demo"), ("episode", "3"), ("camera", "observation.images.front"), ("frame", "7")]),
            "/api/v1/datasets/me/demo/episodes/3/frames/observation.images.front/7"
        );
        assert_eq!(fill_path("/a/{x}", &[("x", "a b/c")]), "/a/a%20b%2Fc");
    }

    #[test]
    fn repo_ids_split_into_owner_and_name() {
        assert_eq!(split_repo("me/demo"), ("me", "demo"));
    }

    #[test]
    fn events_url_follows_the_base_scheme() {
        assert_eq!(EngineClient::new("http://127.0.0.1:8765/").events_url(), "ws://127.0.0.1:8765/api/v1/ws/events");
    }

    #[test]
    fn error_bodies_show_the_engine_detail() {
        assert_eq!(error_detail(r#"{"detail":"a session is busy"}"#), "a session is busy");
        assert_eq!(error_detail("plain"), "plain");
    }

    #[test]
    fn every_route_is_declared_once() {
        let mut seen = std::collections::BTreeSet::new();
        for r in routes::ALL {
            assert!(seen.insert((r.method, r.path)), "duplicate route {} {}", r.method, r.path);
        }
    }

    #[test]
    fn a_token_is_sent_as_a_bearer_header_and_in_the_event_url() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0u8; 2048];
            let n = s.read(&mut buf).unwrap();
            tx.send(String::from_utf8_lossy(&buf[..n]).to_string()).unwrap();
            let body = r#"{"status":"ok","version":"1","api_version":"1","capabilities":[],"pid":1}"#;
            let _ = s.write_all(
                format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
                    .as_bytes(),
            );
        });
        let client = EngineClient::new(url.clone()).with_token("s3 cret");
        client.health().unwrap();
        let request = rx.recv().unwrap().to_ascii_lowercase();
        assert!(request.contains("authorization: bearer s3 cret"), "{request}");
        assert!(client.events_url().ends_with("/api/v1/ws/events"), "the token never goes in the URL");
        assert_eq!(client.token(), Some("s3 cret"));
        // no token: no header and a plain event URL
        assert!(EngineClient::new(url).with_token("  ").events_url().ends_with("/api/v1/ws/events"));
    }
}
