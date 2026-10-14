//! Link handoff: the engine takes over the robot's serial ports for a session, so the desktop first
//! releases every link it holds and reclaims them when the engine says the session is over.
//!
//! The decisions live here as a small state machine so they are tested without hardware; the UI layer
//! performs the returned [`Action`]s.

use std::time::Duration;

/// How to reopen a link that was released (an address, never a live handle).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkSpec {
    Serial { port: String, baud: u32 },
    Tcp { host: String, port: u16 },
}

impl LinkSpec {
    pub fn label(&self) -> String {
        match self {
            LinkSpec::Serial { port, baud } => format!("{port} @ {baud}"),
            LinkSpec::Tcp { host, port } => format!("{host}:{port}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Close every held link and wait until each is closed, then start the session.
    ReleaseLinks,
    /// Reopen the released links (with [`reclaim_delay`] between attempts).
    ReclaimLinks(Vec<LinkSpec>),
    /// Nothing to do for this event.
    None,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Idle,
    /// Links were released and the engine is starting or running a session.
    Engine,
}

#[derive(Debug, Default)]
pub struct Handoff {
    phase: Phase,
    released: Vec<LinkSpec>,
}

impl Handoff {
    pub fn phase(&self) -> &Phase {
        &self.phase
    }

    /// A session is about to start. Returns what to do first; refuses while one is already in progress.
    pub fn begin(&mut self, held: Vec<LinkSpec>) -> Action {
        if self.phase != Phase::Idle {
            return Action::None;
        }
        self.phase = Phase::Engine;
        self.released = held;
        Action::ReleaseLinks
    }

    /// The session failed to start: give the links straight back.
    pub fn start_failed(&mut self) -> Action {
        self.finish()
    }

    /// An engine `session_changed` event. The `idle` event means the arm is released.
    pub fn on_session_state(&mut self, state: &str) -> Action {
        if state == "idle" { self.finish() } else { Action::None }
    }

    /// The event stream was lost or the engine exited while a session was active: do not leave the
    /// desktop without its links forever.
    pub fn on_engine_gone(&mut self) -> Action {
        self.finish()
    }

    fn finish(&mut self) -> Action {
        if self.phase == Phase::Idle {
            return Action::None;
        }
        self.phase = Phase::Idle;
        let specs = std::mem::take(&mut self.released);
        if specs.is_empty() { Action::None } else { Action::ReclaimLinks(specs) }
    }
}

/// Delay before reclaim attempt `attempt` (0-based): COM ports re-enumerate slowly after the follower
/// ESP32 restarts, so early attempts are quick and later ones back off to a cap.
pub fn reclaim_delay(attempt: u32) -> Duration {
    Duration::from_millis(match attempt {
        0 => 500,
        1 => 1000,
        2 => 2000,
        _ => 3000,
    })
}

/// Attempts before the desktop gives up and tells the user to reconnect by hand.
pub const RECLAIM_ATTEMPTS: u32 = 10;

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> LinkSpec {
        LinkSpec::Serial { port: "COM19".into(), baud: 1_000_000 }
    }

    #[test]
    fn links_are_released_first_and_reclaimed_on_idle() {
        let mut h = Handoff::default();
        assert_eq!(h.begin(vec![serial()]), Action::ReleaseLinks);
        assert_eq!(h.on_session_state("starting"), Action::None);
        assert_eq!(h.on_session_state("running"), Action::None);
        assert_eq!(h.on_session_state("stopped"), Action::None);
        assert_eq!(h.on_session_state("idle"), Action::ReclaimLinks(vec![serial()]));
        assert_eq!(h.phase(), &Phase::Idle);
        assert_eq!(h.on_session_state("idle"), Action::None, "reclaim happens once");
    }

    #[test]
    fn a_failed_start_gives_the_links_back() {
        let mut h = Handoff::default();
        h.begin(vec![serial()]);
        assert_eq!(h.start_failed(), Action::ReclaimLinks(vec![serial()]));
    }

    #[test]
    fn a_second_begin_is_refused_while_a_session_is_active() {
        let mut h = Handoff::default();
        h.begin(vec![serial()]);
        assert_eq!(h.begin(vec![]), Action::None);
        assert_eq!(h.on_session_state("idle"), Action::ReclaimLinks(vec![serial()]));
    }

    #[test]
    fn nothing_to_reclaim_when_no_link_was_held() {
        let mut h = Handoff::default();
        assert_eq!(h.begin(vec![]), Action::ReleaseLinks);
        assert_eq!(h.on_session_state("idle"), Action::None);
    }

    #[test]
    fn losing_the_engine_still_returns_the_links() {
        let mut h = Handoff::default();
        h.begin(vec![serial(), LinkSpec::Tcp { host: "192.168.4.1".into(), port: 8080 }]);
        assert!(matches!(h.on_engine_gone(), Action::ReclaimLinks(v) if v.len() == 2));
    }

    #[test]
    fn reclaim_backs_off_to_a_cap() {
        let delays: Vec<u64> = (0..RECLAIM_ATTEMPTS).map(|a| reclaim_delay(a).as_millis() as u64).collect();
        assert_eq!(delays[..4], [500, 1000, 2000, 3000]);
        assert!(delays.iter().all(|d| *d <= 3000));
        let total: u64 = delays.iter().sum();
        assert!(total >= 20_000, "bounded wait still covers a slow re-enumeration: {total} ms");
    }
}
