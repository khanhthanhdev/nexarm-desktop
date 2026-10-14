//! macOS backend: `networksetup` (Apple removed command-line scanning, so no scan).
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

/// The Wi-Fi device (e.g. `en0`) from `networksetup -listallhardwareports` output.
pub(super) fn wifi_device(output: &str) -> Option<String> {
    let mut in_wifi = false;
    for line in output.lines().map(str::trim) {
        if let Some(port) = line.strip_prefix("Hardware Port:") {
            let port = port.trim();
            in_wifi = port == "Wi-Fi" || port == "AirPort";
        } else if let Some(dev) = line.strip_prefix("Device:")
            && in_wifi && !dev.trim().is_empty() {
                return Some(dev.trim().to_string());
            }
    }
    None
}

/// The current network from `networksetup -getairportnetwork <iface>` output.
pub(super) fn current_network(output: &str) -> Option<String> {
    output
        .lines()
        .find_map(|l| l.trim().strip_prefix("Current Wi-Fi Network:"))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(target_os = "macos")]
mod os {
    use std::process::Output;

    use super::*;

    fn networksetup(args: &[&str]) -> std::io::Result<Output> {
        super::super::command("networksetup").args(args).output()
    }

    fn text(o: &Output) -> String {
        let mut s = String::from_utf8_lossy(&o.stdout).into_owned();
        s.push_str(&String::from_utf8_lossy(&o.stderr));
        s
    }

    pub fn scan_ssids() -> Result<Vec<String>, String> {
        Err("[WiFi] Scanning is not available on macOS; type the robot's SSID manually".into())
    }

    pub fn join(ssid: &str, password: &str) -> anyhow::Result<()> {
        let ports = networksetup(&["-listallhardwareports"])?;
        let iface = wifi_device(&String::from_utf8_lossy(&ports.stdout))
            .ok_or_else(|| anyhow::anyhow!("No Wi-Fi hardware port found"))?;
        if let Ok(o) = networksetup(&["-getairportnetwork", &iface]) {
            if current_network(&String::from_utf8_lossy(&o.stdout)).as_deref() == Some(ssid) {
                return Ok(());
            }
        }
        let mut args = vec!["-setairportnetwork", iface.as_str(), ssid];
        if !password.is_empty() {
            args.push(password);
        }
        let o = networksetup(&args)?;
        // networksetup exits 0 on many failures; it reports them in its output instead.
        let out = text(&o);
        let failed = !o.status.success() || out.contains("Error") || out.contains("Could not");
        anyhow::ensure!(!failed, "networksetup could not join {ssid:?}: {}", out.trim());
        Ok(())
    }
}

#[cfg(target_os = "macos")]
pub(super) use os::{join, scan_ssids};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_wifi_device() {
        let out = "\nHardware Port: Ethernet\nDevice: en1\nEthernet Address: aa\n\nHardware Port: Wi-Fi\nDevice: en0\nEthernet Address: bb\n";
        assert_eq!(wifi_device(out).as_deref(), Some("en0"));
        assert_eq!(wifi_device("Hardware Port: Ethernet\nDevice: en1\n"), None);
    }

    #[test]
    fn parses_current_network() {
        assert_eq!(current_network("Current Wi-Fi Network: NexArm_AP\n").as_deref(), Some("NexArm_AP"));
        assert_eq!(current_network("You are not associated with an AirPort network.\n"), None);
    }
}
