//! Headless UI tests for the Lab pages: they render without panicking and drive a (mock) engine.
//!
//! The engine is a tiny in-process HTTP server, so the tests need neither Python nor a robot. Settings
//! go to a temporary `NEXARM_CONFIG_DIR`, never to the user's real settings file.

use gpui::{AppContext as _, Entity, TestAppContext, VisualTestContext};
use nexarm_desktop::ui::engine_state::{EngineNote, EngineState, EngineStatus, teleop_params};
use nexarm_desktop::ui::pages::{
    lab_datasets::LabDatasetsPage, lab_deploy::LabDeployPage, lab_record::LabRecordPage, lab_remote::LabRemotePage,
    lab_session::LabSessionPage,
    lab_train::LabTrainPage,
};
use nexarm_desktop::ui::state::AppState;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

// ------------------------------------------------------------------ mock engine
/// Serves canned JSON by `METHOD path` (query string ignored) and records every request.
struct MockEngine {
    url: String,
    seen: Arc<Mutex<Vec<String>>>,
}

impl MockEngine {
    fn saw(&self, prefix: &str) -> bool {
        self.seen.lock().unwrap().iter().any(|l| l.starts_with(prefix))
    }

    fn count(&self, prefix: &str) -> usize {
        self.seen.lock().unwrap().iter().filter(|l| l.starts_with(prefix)).count()
    }
}

fn mock_engine(routes: &[(&str, &str)]) -> MockEngine {
    let routes: BTreeMap<String, String> = routes.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let log = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() {
                continue;
            }
            let mut parts = line.split_whitespace();
            let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
            let key = format!("{method} {}", target.split('?').next().unwrap_or(""));
            let mut length = 0usize;
            loop {
                let mut h = String::new();
                if reader.read_line(&mut h).unwrap_or(0) == 0 || h.trim().is_empty() {
                    break;
                }
                if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = v.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0u8; length];
            let _ = reader.read_exact(&mut body);
            log.lock().unwrap().push(format!("{key} {}", String::from_utf8_lossy(&body)));
            let (status, payload) = match routes.get(&key) {
                Some(p) => ("200 OK", p.as_str()),
                None => ("404 Not Found", r#"{"detail":"not found"}"#),
            };
            let reply = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                payload.len()
            );
            let _ = stream.write_all(reply.as_bytes());
        }
    });
    MockEngine { url, seen }
}

const HEALTH: &str = r#"{"status":"ok","version":"0.6.1","api_version":"1","capabilities":["sessions","teleoperation"],"pid":1}"#;
const PROFILES: &str = r#"[{"name":"bench","follower_port":"COM19","leader_port":"COM18","baudrate":1000000,"cameras":{"front":{"type":"opencv","source":"0","width":640,"height":480,"fps":30}}}]"#;
const DATASETS: &str = r#"[{"repo_id":"me/demo","fps":30,"robot_type":"nexarm_follower","total_episodes":2,"total_frames":24,"cameras":["observation.images.front"],"excluded_episodes":0}]"#;
const SESSION: &str = r#"{"id":"s1","kind":"teleoperation","profile":"bench","state":"running","started_at":1.0,"params":{},"status":{"ticks":3}}"#;
const STOPPED: &str = r#"{"id":"s1","kind":"teleoperation","profile":"bench","state":"stopped","started_at":1.0}"#;

fn base_routes() -> Vec<(&'static str, &'static str)> {
    vec![
        ("GET /api/v1/health", HEALTH),
        ("GET /api/v1/profiles", PROFILES),
        ("GET /api/v1/ports", r#"[{"device":"COM19","description":"USB Serial"}]"#),
        ("GET /api/v1/sessions/current", "null"),
        ("GET /api/v1/datasets", DATASETS),
        ("GET /api/v1/operations", "[]"),
        ("GET /api/v1/jobs", "[]"),
        ("GET /api/v1/models", "[]"),
    ]
}

// -------------------------------------------------------------------- fixtures
/// Tests share one process-wide config dir and environment variable, so they run one at a time.
static SERIAL: Mutex<()> = Mutex::new(());
static RENDERS: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    engine: Entity<EngineState>,
    app: Entity<AppState>,
    dir: std::path::PathBuf,
    _serial: MutexGuard<'static, ()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn fixture(cx: &mut TestAppContext, engine_url: &str) -> Fixture {
    let serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("nexarm-ui-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // SAFETY: the SERIAL lock keeps every other test in this binary from touching the environment.
    unsafe { std::env::set_var("NEXARM_CONFIG_DIR", &dir) };
    std::fs::write(
        dir.join("settings.json"),
        format!(r#"{{"lang":"en","engine":{{"url":"{engine_url}","auto_start":false,"profile":"bench"}}}}"#),
    )
    .unwrap();
    cx.update(gpui_kit::init);
    let app = cx.new(AppState::new);
    let engine = cx.new(|cx| EngineState::new(app.clone(), cx));
    Fixture { engine, app, dir, _serial: serial }
}

/// Renders every Lab page at once so one window draws them all.
struct Harness {
    pages: Vec<gpui::AnyView>,
}

impl gpui::Render for Harness {
    fn render(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> impl gpui::IntoElement {
        use gpui::{ParentElement as _, Styled as _};
        RENDERS.fetch_add(1, Ordering::Relaxed);
        gpui::div().size_full().children(self.pages.iter().cloned())
    }
}

/// A window (inside GPUI Kit's `Root`, so notifications work) holding the pages built by `build`.
fn open(
    cx: &mut TestAppContext,
    build: impl FnOnce(&mut gpui::Window, &mut gpui::Context<Harness>) -> Vec<gpui::AnyView> + 'static,
) -> &mut VisualTestContext {
    let (_root, vcx) = cx.add_window_view(move |window, cx| {
        let harness = cx.new(|cx| Harness { pages: build(window, cx) });
        gpui_kit::base::Root::new(gpui::AnyView::from(harness), window, cx)
    });
    vcx
}

fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        let arena = window.draw(cx);
        arena.clear(cx);
    });
}

/// Let background calls and the mock server's thread finish.
fn settle(cx: &mut VisualTestContext, mut until: impl FnMut(&mut VisualTestContext) -> bool) {
    for _ in 0..100 {
        cx.run_until_parked();
        if until(cx) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    cx.run_until_parked();
}

// ----------------------------------------------------------------------- tests
#[gpui::test]
fn every_lab_page_renders_before_and_after_connecting(cx: &mut TestAppContext) {
    let mock = mock_engine(&base_routes());
    let fx = fixture(cx, &mock.url);
    let (app, engine) = (fx.app.clone(), fx.engine.clone());
    let vcx = open(cx, move |window, cx| {
        vec![
            cx.new(|cx| LabSessionPage::new(app.clone(), engine.clone(), window, cx)).into(),
            cx.new(|cx| LabRecordPage::new(app.clone(), engine.clone(), window, cx)).into(),
            cx.new(|cx| LabDatasetsPage::new(app.clone(), engine.clone(), window, cx)).into(),
            cx.new(|cx| LabTrainPage::new(app.clone(), engine.clone(), window, cx)).into(),
            cx.new(|cx| LabDeployPage::new(app.clone(), engine.clone(), window, cx)).into(),
            cx.new(|cx| LabRemotePage::new(app.clone(), engine.clone(), window, cx)).into(),
        ]
    });
    draw(vcx);
    let before = RENDERS.load(Ordering::Relaxed);
    assert!(before > 0, "the harness window was never drawn");

    fx.engine.update(vcx, |e, cx| e.connect_engine(cx));
    settle(vcx, |_| mock.saw("GET /api/v1/datasets") && mock.saw("GET /api/v1/jobs"));
    assert!(mock.saw("GET /api/v1/health") && mock.saw("GET /api/v1/profiles"));
    assert!(fx.engine.read_with(vcx, |e, _| matches!(e.status, EngineStatus::Ready(_))), "the mock engine is attached");
    assert_eq!(fx.engine.read_with(vcx, |e, _| e.profiles.len()), 1);
    draw(vcx);
    assert!(RENDERS.load(Ordering::Relaxed) > before, "the pages re-render when the engine state changes");
}

#[gpui::test]
fn a_session_starts_and_stops_through_the_engine(cx: &mut TestAppContext) {
    let mut routes = base_routes();
    routes.extend([("POST /api/v1/sessions", SESSION), ("DELETE /api/v1/sessions/s1", STOPPED)]);
    let mock = mock_engine(&routes);
    let fx = fixture(cx, &mock.url);
    let vcx = open(cx, |_, _| Vec::new());
    fx.engine.update(vcx, |e, cx| e.connect_engine(cx));
    settle(vcx, |c| fx.engine.read_with(c, |e, _| e.profiles.len() == 1));

    fx.engine.update(vcx, |e, cx| e.start_session("teleoperation", teleop_params(), cx));
    settle(vcx, |c| fx.engine.read_with(c, |e, _| e.session.is_some()));
    let posts: Vec<String> =
        mock.seen.lock().unwrap().iter().filter(|l| l.starts_with("POST /api/v1/sessions ")).cloned().collect();
    assert_eq!(posts.len(), 1, "{posts:?}");
    let compact: String = posts[0].chars().filter(|c| !c.is_whitespace()).collect();
    assert!(compact.contains(r#""profile":"bench""#) && compact.contains(r#""kind":"teleoperation""#), "{posts:?}");
    assert_eq!(fx.engine.read_with(vcx, |e, _| e.session.as_ref().map(|s| s.id.clone())), Some("s1".into()));
    assert!(!fx.engine.read_with(vcx, |e, _| e.busy));

    // A second start is refused locally while one is active.
    fx.engine.update(vcx, |e, cx| e.start_session("recording", serde_json::json!({}), cx));
    vcx.run_until_parked();
    assert_eq!(mock.count("POST /api/v1/sessions "), 1);

    fx.engine.update(vcx, |e, cx| e.stop_session(cx));
    settle(vcx, |c| fx.engine.read_with(c, |e, _| e.session.is_none() && !e.busy));
    assert!(mock.saw("DELETE /api/v1/sessions/s1"));
    assert!(fx.engine.read_with(vcx, |e, _| e.session.is_none() && !e.busy));
}

#[gpui::test]
fn a_refused_session_start_reports_the_error_and_frees_the_slot(cx: &mut TestAppContext) {
    let mock = mock_engine(&base_routes()); // POST /sessions is not routed: the mock answers 404
    let fx = fixture(cx, &mock.url);
    let vcx = open(cx, |_, _| Vec::new());
    let errors = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = errors.clone();
    let _sub = vcx.update(|_, cx| {
        cx.subscribe(&fx.engine, move |_, note: &EngineNote, _| {
            if let EngineNote::Error(m) = note {
                sink.lock().unwrap().push(m.clone());
            }
        })
    });
    fx.engine.update(vcx, |e, cx| e.connect_engine(cx));
    settle(vcx, |c| fx.engine.read_with(c, |e, _| e.profiles.len() == 1));

    fx.engine.update(vcx, |e, cx| e.start_session("teleoperation", teleop_params(), cx));
    settle(vcx, |_| !errors.lock().unwrap().is_empty());
    assert!(errors.lock().unwrap()[0].contains("not found"), "{:?}", errors.lock().unwrap());
    assert!(fx.engine.read_with(vcx, |e, _| e.session.is_none() && !e.busy));

    // The slot is free again: a later start reaches the engine.
    fx.engine.update(vcx, |e, cx| e.start_session("teleoperation", teleop_params(), cx));
    settle(vcx, |_| mock.count("POST /api/v1/sessions ") == 2);
    assert_eq!(mock.count("POST /api/v1/sessions "), 2);
}
