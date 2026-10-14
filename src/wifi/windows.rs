//! Windows backend: `netsh wlan` (same flow as the original client, without a shell).
#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

/// Escape text for an XML element body.
pub(super) fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

fn ssid_hex(ssid: &str) -> String {
    ssid.bytes().map(|b| format!("{b:02X}")).collect()
}

/// WLAN profile XML (WPA2-PSK/AES with a password, open otherwise).
pub(super) fn profile_xml(ssid: &str, password: &str) -> String {
    let (auth, encrypt, key_section) = if password.is_empty() {
        ("open", "none", String::new())
    } else {
        (
            "WPA2PSK",
            "AES",
            format!(
                "            <sharedKey>\n                <keyType>passPhrase</keyType>\n                <protected>false</protected>\n                <keyMaterial>{}</keyMaterial>\n            </sharedKey>",
                xml_escape(password)
            ),
        )
    };
    let name = xml_escape(ssid);
    format!(
        "<?xml version=\"1.0\"?>\n<WLANProfile xmlns=\"http://www.microsoft.com/networking/WLAN/profile/v1\">\n    <name>{name}</name>\n    <SSIDConfig>\n        <SSID>\n            <hex>{hex}</hex>\n            <name>{name}</name>\n        </SSID>\n        <nonBroadcast>false</nonBroadcast>\n    </SSIDConfig>\n    <connectionType>ESS</connectionType>\n    <connectionMode>auto</connectionMode>\n    <MSM>\n        <security>\n            <authEncryption>\n                <authentication>{auth}</authentication>\n                <encryption>{encrypt}</encryption>\n                <useOneX>false</useOneX>\n            </authEncryption>\n{key_section}\n        </security>\n    </MSM>\n</WLANProfile>",
        hex = ssid_hex(ssid)
    )
}

/// SSIDs from `netsh wlan show networks|interfaces` output (English or Chinese labels).
pub(super) fn parse_ssids(output: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in output.lines().map(str::trim) {
        let is_ssid_line = (line.contains("SSID") || line.contains("网络名")) && !line.contains("BSSID");
        if !is_ssid_line {
            continue;
        }
        // English and Chinese consoles use ":" or the full-width "：".
        if let Some((_, value)) = line.split_once(':').or_else(|| line.split_once('：')) {
            super::push_unique(&mut out, value);
        }
    }
    out
}

fn wlan_service_running(sc_output: &str) -> bool {
    sc_output.contains("RUNNING")
        || sc_output.contains("运行")
        || (sc_output.contains("STATE") && sc_output.contains('4'))
}

fn has_wireless_interface(interfaces_output: &str) -> bool {
    ["GUID", "WLAN", "Wireless", "接口"].iter().any(|k| interfaces_output.contains(k))
}

#[derive(Default, Debug, PartialEq)]
struct ScanHints {
    need_location: bool,
    need_admin: bool,
}

fn classify_error(text: &str, hints: &mut ScanHints) {
    let lower = text.to_lowercase();
    if text.contains("位置") || lower.contains("location") {
        hints.need_location = true;
    }
    if text.contains("管理员") || text.contains("提升") || lower.contains("admin") || lower.contains("elevat") {
        hints.need_admin = true;
    }
}

fn diagnosis(hints: &ScanHints, wlan_ok: bool, has_iface: bool) -> String {
    let mut diag = Vec::new();
    if hints.need_location {
        diag.push("Windows Location Service disabled - open Settings > Privacy > Location, turn ON");
    }
    if hints.need_admin {
        diag.push("Need Run as Administrator");
    }
    if !wlan_ok {
        diag.push("WlanSvc not running");
    }
    if !has_iface {
        diag.push("No wireless adapter");
    }
    if diag.is_empty() {
        diag.push("Unknown reason");
    }
    format!("[WiFi] SCAN FAILED: {}", diag.join("; "))
}

#[cfg(target_os = "windows")]
mod os {
    use std::process::Output;

    use super::*;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetOEMCP() -> u32;
        fn MultiByteToWideChar(
            code_page: u32,
            flags: u32,
            src: *const u8,
            src_len: i32,
            dst: *mut u16,
            dst_len: i32,
        ) -> i32;
    }

    /// Console tools write UTF-8 or the OEM code page (e.g. CP936 on Chinese Windows).
    fn decode(bytes: &[u8]) -> String {
        if let Ok(s) = std::str::from_utf8(bytes) {
            return s.to_owned();
        }
        let Ok(len) = i32::try_from(bytes.len()) else {
            return String::from_utf8_lossy(bytes).into_owned();
        };
        // SAFETY: buffers are sized from the first call; pointers/lengths match their slices.
        unsafe {
            let cp = GetOEMCP();
            let n = MultiByteToWideChar(cp, 0, bytes.as_ptr(), len, std::ptr::null_mut(), 0);
            if n <= 0 {
                return String::from_utf8_lossy(bytes).into_owned();
            }
            let mut wide = vec![0u16; n as usize];
            let n = MultiByteToWideChar(cp, 0, bytes.as_ptr(), len, wide.as_mut_ptr(), n);
            String::from_utf16_lossy(&wide[..n.max(0) as usize])
        }
    }

    fn text(o: &Output) -> String {
        let mut s = decode(&o.stdout);
        s.push_str(&decode(&o.stderr));
        s
    }

    /// Run `netsh` with a raw tail so `name="..."` reaches it exactly as netsh expects.
    fn netsh(fixed: &[&str], raw_tail: Option<&str>) -> std::io::Result<Output> {
        use std::os::windows::process::CommandExt;
        let mut c = super::super::command("netsh");
        c.args(fixed);
        if let Some(t) = raw_tail {
            c.raw_arg(t);
        }
        c.output()
    }

    fn quoted(value: &str) -> anyhow::Result<String> {
        anyhow::ensure!(!value.contains('"'), "netsh cannot handle '\"' in {value:?}");
        Ok(format!("\"{value}\""))
    }

    fn associated_with(ssid: &str) -> bool {
        netsh(&["wlan", "show", "interfaces"], None)
            .map(|o| parse_ssids(&text(&o)).iter().any(|s| s == ssid))
            .unwrap_or(false)
    }

    pub fn scan_ssids() -> Result<Vec<String>, String> {
        let wlan_ok = match super::super::command("sc").args(["query", "WlanSvc"]).output() {
            Ok(o) if wlan_service_running(&text(&o)) => true,
            _ => super::super::command("net")
                .args(["start", "WlanSvc"])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false),
        };
        let has_iface = netsh(&["wlan", "show", "interfaces"], None)
            .map(|o| has_wireless_interface(&text(&o)))
            .unwrap_or(false);

        let mut hints = ScanHints::default();
        for args in [&["wlan", "show", "networks", "mode=bssid"][..], &["wlan", "show", "networks"][..]] {
            let Ok(o) = netsh(args, None) else { continue };
            let out = text(&o);
            let found = if o.status.success() { parse_ssids(&out) } else { Vec::new() };
            if !found.is_empty() {
                return Ok(found);
            }
            // Location/elevation refusals may exit 0 with no networks listed.
            classify_error(&out, &mut hints);
        }
        Err(diagnosis(&hints, wlan_ok, has_iface))
    }

    pub fn join(ssid: &str, password: &str) -> anyhow::Result<()> {
        if associated_with(ssid) {
            return Ok(());
        }
        let name = quoted(ssid)?;
        let _ = netsh(&["wlan", "delete", "profile"], Some(&format!("name={name}")));

        // The profile holds the key in plain text; the guard deletes it on every exit path.
        struct TempFile(std::path::PathBuf);
        impl Drop for TempFile {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
        let tmp = TempFile(std::env::temp_dir().join(format!("nexarm_wifi_{}.xml", std::process::id())));
        let path = &tmp.0;
        let mut xml = vec![0xEF, 0xBB, 0xBF]; // UTF-8 BOM, as netsh expects
        xml.extend_from_slice(profile_xml(ssid, password).as_bytes());
        std::fs::write(path, xml)?;
        let file_arg = format!("filename={}", quoted(&path.to_string_lossy())?);
        let added = netsh(&["wlan", "add", "profile"], Some(&format!("{file_arg} user=all")))
            .map(|o| o.status.success())
            .unwrap_or(false);
        let added = added
            || netsh(&["wlan", "add", "profile"], Some(&file_arg))
                .map(|o| o.status.success())
                .unwrap_or(false);
        drop(tmp);
        anyhow::ensure!(added, "netsh could not add a WLAN profile for {ssid:?}");

        let o = netsh(&["wlan", "connect"], Some(&format!("name={name}")))?;
        anyhow::ensure!(o.status.success(), "netsh wlan connect failed: {}", text(&o).trim());
        Ok(())
    }
}

#[cfg(target_os = "windows")]
pub(super) use os::{join, scan_ssids};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_xml_specials() {
        assert_eq!(xml_escape(r#"a&b<c>"d'"#), "a&amp;b&lt;c&gt;&quot;d&apos;");
    }

    #[test]
    fn profile_contains_escaped_values_and_hex() {
        let x = profile_xml("Nex&Arm", "p<w");
        assert!(x.contains("<name>Nex&amp;Arm</name>"));
        assert!(x.contains("<hex>4E65782641726D</hex>"));
        assert!(x.contains("<keyMaterial>p&lt;w</keyMaterial>"));
        assert!(x.contains("<authentication>WPA2PSK</authentication>"));
        let open = profile_xml("Open", "");
        assert!(open.contains("<authentication>open</authentication>") && !open.contains("sharedKey"));
    }

    #[test]
    fn hex_is_uppercase_utf8() {
        assert_eq!(ssid_hex("Ab"), "4162");
        assert_eq!(ssid_hex("网"), "E7BD91");
    }

    #[test]
    fn parses_english_netsh_networks() {
        let out = "Interface name : Wi-Fi\nThere are 2 networks currently visible.\n\nSSID 1 : NexArm_AP\n    Network type            : Infrastructure\n    BSSID 1                 : aa:bb:cc:dd:ee:ff\nSSID 2 : Home Net\nSSID 3 : NexArm_AP\n";
        assert_eq!(parse_ssids(out), ["NexArm_AP", "Home Net"]);
    }

    #[test]
    fn parses_chinese_netsh_networks() {
        let out = "接口名称 : WLAN\n当前有 1 个网络可见。\n\nSSID 1 : 机械臂\n    网络类型            : 结构\n    身份验证            : WPA2 - 个人\n    BSSID 1             : aa:bb:cc:dd:ee:ff\n网络名：Robot2\n";
        assert_eq!(parse_ssids(out), ["机械臂", "Robot2"]);
    }

    #[test]
    fn interfaces_output_gives_current_ssid() {
        let out = "    Name                   : Wi-Fi\n    State                  : connected\n    SSID                   : NexArm_AP\n    BSSID                  : aa:bb:cc:dd:ee:ff\n    Profile                : NexArm_AP\n";
        assert_eq!(parse_ssids(out), ["NexArm_AP"]);
    }

    #[test]
    fn diagnosis_lists_reasons() {
        let mut h = ScanHints::default();
        classify_error("The operation requires elevation (Run as administrator).", &mut h);
        classify_error("Network shell commands need location permission", &mut h);
        assert_eq!(h, ScanHints { need_location: true, need_admin: true });
        let d = diagnosis(&h, false, false);
        assert!(d.starts_with("[WiFi] SCAN FAILED: Windows Location Service disabled"));
        assert!(d.contains("Need Run as Administrator; WlanSvc not running; No wireless adapter"));
        assert_eq!(diagnosis(&ScanHints::default(), true, true), "[WiFi] SCAN FAILED: Unknown reason");
    }

    #[test]
    fn service_and_interface_detection() {
        assert!(wlan_service_running("        STATE              : 4  RUNNING"));
        assert!(!wlan_service_running("        STATE              : 1  STOPPED"));
        assert!(has_wireless_interface("There is 1 interface on the system:\n    GUID : {..}"));
        assert!(!has_wireless_interface("There is no such interface on the system."));
    }
}
