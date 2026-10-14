//! The desktop's engine client must only call routes the engine documents.
//!
//! `fixtures/openapi.json` and `fixtures/events.json` are copies of the engine's committed
//! `docs/service/` snapshots, refreshed by `scripts/sync-engine-contract`.

use nexarm_desktop::engine::API_VERSION;
use nexarm_desktop::engine::client::routes;
use serde_json::Value;

fn openapi() -> Value {
    serde_json::from_str(include_str!("fixtures/openapi.json")).expect("valid openapi.json")
}

#[test]
fn every_route_the_client_uses_exists_in_the_engine_contract() {
    let spec = openapi();
    let paths = spec["paths"].as_object().expect("paths");
    let mut missing = Vec::new();
    for route in routes::ALL {
        let ok = paths
            .get(route.path)
            .and_then(|item| item.get(route.method.to_ascii_lowercase()))
            .is_some();
        if !ok {
            missing.push(format!("{} {}", route.method, route.path));
        }
    }
    assert!(missing.is_empty(), "routes missing from fixtures/openapi.json: {missing:?}");
}

#[test]
fn the_contract_is_for_the_api_version_this_app_speaks() {
    assert_eq!(openapi()["info"]["version"], API_VERSION);
}

#[test]
fn the_event_schema_declares_the_session_idle_state_the_handoff_waits_for() {
    let events: Value = serde_json::from_str(include_str!("fixtures/events.json")).unwrap();
    let state = events["session_changed"]["state"].as_str().unwrap();
    assert!(state.contains("idle"), "{state}");
}
