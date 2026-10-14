//! Joining the robot's WiFi access point and scanning for SSIDs.
//!
//! All functions block; call them from a background thread. OS tools are run with
//! argument vectors (never through a shell).

use std::net::{TcpStream, ToSocketAddrs};
use std::process::Command;
use std::time::Duration;

// Parsers in each module are platform-independent so they are unit-tested everywhere;
// only the command-running parts are gated on the target OS.
mod linux;
mod macos;
mod windows;

#[cfg(target_os = "linux")]
use linux as platform;
#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(target_os = "windows")]
use windows as platform;

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
mod platform {
    pub fn scan_ssids() -> Result<Vec<String>, String> {
        Err("WiFi scanning is not supported on this OS; type the SSID manually".into())
    }
    pub fn join(_ssid: &str, _password: &str) -> anyhow::Result<()> {
        anyhow::bail!("Joining WiFi is not supported on this OS; join the robot's network manually")
    }
}

/// Visible SSIDs, de-duplicated in discovery order. `Err` carries a human-readable diagnosis.
pub fn scan_ssids() -> Result<Vec<String>, String> {
    platform::scan_ssids()
}

/// Join the robot access point `ssid` (no-op when already associated with it).
pub fn join(ssid: &str, password: &str) -> anyhow::Result<()> {
    if ssid.is_empty() {
        anyhow::bail!("SSID is empty");
    }
    platform::join(ssid, password)
}

/// Probe `host:port` with TCP connects (500 ms timeout, 1 s between tries).
pub fn wait_for_host(host: &str, port: u16, attempts: u32) -> bool {
    for i in 0..attempts {
        if i > 0 {
            std::thread::sleep(Duration::from_secs(1));
        }
        let Ok(addrs) = (host, port).to_socket_addrs() else { continue };
        for addr in addrs {
            if TcpStream::connect_timeout(&addr, Duration::from_millis(500)).is_ok() {
                return true;
            }
        }
    }
    false
}

/// `Command` that never flashes a console window on Windows.
#[allow(dead_code)]
fn command(program: &str) -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(program);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}

#[allow(dead_code)]
fn push_unique(list: &mut Vec<String>, ssid: &str) {
    let ssid = ssid.trim();
    if !ssid.is_empty() && !list.iter().any(|s| s == ssid) {
        list.push(ssid.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_unique_dedupes_and_trims() {
        let mut v = Vec::new();
        push_unique(&mut v, " NexArm ");
        push_unique(&mut v, "NexArm");
        push_unique(&mut v, "");
        assert_eq!(v, ["NexArm"]);
    }

    #[test]
    fn unreachable_host_times_out() {
        // TEST-NET-1 address: never routable.
        assert!(!wait_for_host("192.0.2.1", 9, 1));
    }
}
