//! Client side of the NexArm engine (`lerobot-nexarm-service`): typed HTTP client, event stream,
//! engine supervisor and the serial/TCP link handoff.

pub mod api;
pub mod client;
pub mod events;
pub mod handoff;
pub mod supervisor;

/// API version this app speaks. The engine reports its own in `/health`; a mismatch is refused.
pub const API_VERSION: &str = "1";

/// How often the desktop renews a session lease. The engine stops a session after 60 s of silence.
pub const HEARTBEAT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(20);
