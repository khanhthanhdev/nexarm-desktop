//! Talks to a real running engine (no robot needed). Run on request:
//!
//! ```sh
//! lerobot-nexarm-service &        # with a LeRobot dataset such as me/demo under $HF_LEROBOT_HOME
//! NEXARM_ENGINE_URL=http://127.0.0.1:8765 NEXARM_LIVE_DATASET=me/demo cargo test --test engine_live -- --ignored
//! ```

use nexarm_desktop::engine::API_VERSION;
use nexarm_desktop::engine::api::{CameraProfile, RobotProfile};
use nexarm_desktop::engine::client::EngineClient;
use nexarm_desktop::engine::events::{EngineEvent, listen};
use std::time::Duration;

fn client() -> EngineClient {
    EngineClient::new(std::env::var("NEXARM_ENGINE_URL").unwrap_or_else(|_| "http://127.0.0.1:8765".into()))
}

#[test]
#[ignore = "needs a running engine"]
fn health_profiles_sessions_and_jobs_decode() {
    let c = client();
    let health = c.health().expect("health");
    assert_eq!(health.api_version, API_VERSION);
    assert!(health.capabilities.iter().any(|x| x == "sessions"));

    let profile = RobotProfile {
        name: "live-test".into(),
        follower_port: "COM19".into(),
        leader_port: Some("COM20".into()),
        baudrate: 1_000_000,
        cameras: [("front".to_string(), CameraProfile::default())].into(),
    };
    assert_eq!(c.put_profile(&profile).unwrap(), profile);
    assert!(c.profiles().unwrap().contains(&profile));
    c.delete_profile("live-test").unwrap();
    assert_eq!(c.delete_profile("live-test").unwrap_err().status(), Some(404));

    c.ports().expect("ports");
    assert!(c.current_session().unwrap().is_none());
    c.jobs().expect("jobs");
    c.models().expect("models");
    c.operations().expect("operations");
    assert_eq!(c.start_session("rollout", "no-such-profile", serde_json::json!({})).unwrap_err().status(), Some(404));
}

#[test]
#[ignore = "needs a running engine and a dataset"]
fn dataset_view_calls_decode() {
    let c = client();
    let repo = std::env::var("NEXARM_LIVE_DATASET").unwrap_or_else(|_| "me/demo".into());
    let summary = c.datasets().unwrap().into_iter().find(|d| d.repo_id == repo).expect("dataset listed");
    let episodes = c.episodes(&repo).unwrap();
    assert_eq!(episodes.len() as u32, summary.total_episodes);
    let joints = c.joints(&repo, 0).unwrap();
    assert_eq!(joints.action.len(), episodes[0].length as usize);
    assert_eq!(joints.names.len(), 6);
    let jpeg = c.frame_jpeg(&repo, 0, &summary.cameras[0], 0, 320).unwrap();
    assert_eq!(&jpeg[..2], &[0xFF, 0xD8], "a JPEG");
    assert_eq!(c.set_excluded(&repo, &[0]).unwrap(), vec![0]);
    assert_eq!(c.excluded(&repo).unwrap(), vec![0]);
    c.set_excluded(&repo, &[]).unwrap();
}

#[test]
#[ignore = "needs a running engine"]
fn the_event_stream_says_hello() {
    let c = client();
    let (tx, rx) = flume::unbounded();
    let _listener = listen(c.events_url(), tx);
    let first = rx.recv_timeout(Duration::from_secs(10)).expect("an event");
    assert_eq!(first, EngineEvent::Hello { api_version: API_VERSION.into() });
}

/// Needs an engine started with `--token $NEXARM_LIVE_TOKEN` (the engine then refuses unauthenticated calls).
#[test]
#[ignore = "needs a running engine started with --token"]
fn a_token_protected_engine_refuses_callers_without_the_token() {
    let token = std::env::var("NEXARM_LIVE_TOKEN").expect("set NEXARM_LIVE_TOKEN");
    let url = std::env::var("NEXARM_ENGINE_URL").unwrap_or_else(|_| "http://127.0.0.1:8765".into());
    let denied = EngineClient::new(url.clone()).health().unwrap_err();
    assert_eq!(denied.status(), Some(401), "{denied}");
    let wrong = EngineClient::new(url.clone()).with_token("nope").health().unwrap_err();
    assert_eq!(wrong.status(), Some(401));
    let client = EngineClient::new(url).with_token(&token);
    assert_eq!(client.health().unwrap().api_version, API_VERSION);

    let (tx, rx) = flume::unbounded();
    let _listener = listen(client.events_url(), tx);
    let first = rx.recv_timeout(Duration::from_secs(10)).expect("an event");
    assert_eq!(first, EngineEvent::Hello { api_version: API_VERSION.into() });
}
