//! Linux backend: NetworkManager's `nmcli`.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

/// Split one `nmcli -t` line into fields, honouring `\:` and `\\` escapes.
pub(super) fn split_terse(line: &str) -> Vec<String> {
    let mut fields = vec![String::new()];
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(next) = chars.next() {
                    fields.last_mut().unwrap().push(next);
                }
            }
            ':' => fields.push(String::new()),
            _ => fields.last_mut().unwrap().push(c),
        }
    }
    fields
}

/// SSIDs from `nmcli -t -f SSID dev wifi list` output.
pub(super) fn parse_ssid_list(output: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in output.lines() {
        if let Some(ssid) = split_terse(line).first() {
            super::push_unique(&mut out, ssid);
        }
    }
    out
}

/// The active SSID from `nmcli -t -f ACTIVE,SSID dev wifi` output.
pub(super) fn active_ssid(output: &str) -> Option<String> {
    output.lines().find_map(|line| match split_terse(line).as_slice() {
        [active, ssid] if active == "yes" && !ssid.is_empty() => Some(ssid.clone()),
        _ => None,
    })
}

#[cfg(target_os = "linux")]
mod os {
    use std::process::Output;

    use super::*;

    fn nmcli(args: &[&str]) -> std::io::Result<Output> {
        super::super::command("nmcli").args(args).output()
    }

    fn text(o: &Output) -> String {
        let mut s = String::from_utf8_lossy(&o.stdout).into_owned();
        s.push_str(&String::from_utf8_lossy(&o.stderr));
        s
    }

    pub fn scan_ssids() -> Result<Vec<String>, String> {
        let o = nmcli(&["-t", "-f", "SSID", "dev", "wifi", "list", "--rescan", "yes"])
            .map_err(|e| format!("[WiFi] SCAN FAILED: nmcli not available ({e}); install NetworkManager"))?;
        if !o.status.success() {
            return Err(format!("[WiFi] SCAN FAILED: {}", text(&o).trim()));
        }
        let found = parse_ssid_list(&String::from_utf8_lossy(&o.stdout));
        if found.is_empty() {
            return Err("[WiFi] SCAN FAILED: no networks visible (is the WiFi radio on?)".into());
        }
        Ok(found)
    }

    pub fn join(ssid: &str, password: &str) -> anyhow::Result<()> {
        let current = nmcli(&["-t", "-f", "ACTIVE,SSID", "dev", "wifi"])
            .map_err(|e| anyhow::anyhow!("nmcli not available ({e}); install NetworkManager"))?;
        if active_ssid(&String::from_utf8_lossy(&current.stdout)).as_deref() == Some(ssid) {
            return Ok(());
        }
        let mut args = vec!["dev", "wifi", "connect", ssid];
        if !password.is_empty() {
            args.extend(["password", password]);
        }
        let o = nmcli(&args)?;
        anyhow::ensure!(o.status.success(), "nmcli could not join {ssid:?}: {}", text(&o).trim());
        Ok(())
    }
}

#[cfg(target_os = "linux")]
pub(super) use os::{join, scan_ssids};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_escaped_colons_and_backslashes() {
        assert_eq!(split_terse(r"yes:Nex\:Arm"), ["yes", "Nex:Arm"]);
        assert_eq!(split_terse(r"a\\b:c"), [r"a\b", "c"]);
    }

    #[test]
    fn parses_ssid_list() {
        let out = "NexArm_AP\n\nHome\\:5G\nNexArm_AP\n";
        assert_eq!(parse_ssid_list(out), ["NexArm_AP", "Home:5G"]);
    }

    #[test]
    fn finds_active_ssid() {
        assert_eq!(active_ssid("no:Home\nyes:Nex\\:Arm\n").as_deref(), Some("Nex:Arm"));
        assert_eq!(active_ssid("no:Home\n"), None);
    }
}
