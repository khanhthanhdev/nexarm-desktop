# NexArm desktop port to Rust + GPUI

Status: implemented on Windows (2026-10-13); macOS/Linux via CI matrix, not yet executed

## Outcome
A native app for Windows, macOS and Linux that replaces `NexArm.exe` (PyQt5), with full feature parity across its 7 pages. The wire protocol stays byte-identical. Source of truth for behavior: `../recovered/NexArm/nexarm_qt/**` (verified decompilation) and `../docs/nexarm-gui-reverse-engineering.md`.

## Decisions (user-confirmed)
- **UI stack:** `gpui-kit` 0.7.3. It bundles `gpui-pre` 0.3.9, its platform, and `gpui-component` widgets.
- **3D view:** a pop-out Rerun viewer, run as a separate process the app streams to. The app does URDF forward kinematics and logs the 11 STL link meshes once, then logs per-link transforms on each pose update. If the `rerun` binary is missing, the app shows a hint.
- **WiFi:**
  - Join per OS: Windows `netsh`, Linux `nmcli`, macOS `networksetup`.
  - Then TCP to 192.168.4.1:8080.
  - Commands use argument vectors (no shell), and XML is escaped.
- **Fixes over the original:**
  - Validate the RX checksum, also accepting the leader firmware's short `ID,LEN,CMD` checksum variant.
  - Store passwords in the OS keychain (`keyring`).
  - Honor the baud selector.
- **Scope:** Servo (with action groups), Coordinates, Peripherals, System, AI (with calibration), Servo Advanced, Teach. Also the log (text and hex) view and zh/en strings (294 keys exported from the original).

## Architecture
```
src/protocol.rs     frames, checksum, typed Response parsing, request builders   [done]
src/link.rs         serial/TCP transport; reader thread -> flume channel of LinkEvent; send(Request)
src/wifi/           scan_ssids(), join(ssid, pw); windows.rs / linux.rs / macos.rs
src/settings.rs     JSON settings in OS config dir; secrets via keyring
src/i18n.rs         embedded assets/i18n/strings.json, tr(lang, key)
src/kinematics.rs   URDF (roxmltree) + binary STL + FK (port of urdf_parser/_compute_link_transforms)
src/viewer3d.rs     Rerun spawn + log meshes/transforms
src/ui/state.rs     AppState entity: link, status, lang, last pose/joints; emits DeviceEvent
src/ui/shell.rs     sidebar nav, connection panel, page host, log panel
src/ui/widgets.rs   shared row/field helpers
src/ui/pages/{servo,coord,peripheral,system,ai,servo_adv,teach}.rs
```
Event flow:
1. The link thread pushes events into a flume channel.
2. A foreground task (`cx.spawn`) drains the channel and updates `AppState`.
3. `AppState` emits `DeviceEvent`, and pages subscribe to it.

## Phases
1. Protocol. Done.
2. In parallel:
   - Shell, link, state and log, plus the Coordinates page as the reference pattern.
   - WiFi and settings/keyring.
   - Kinematics and the Rerun viewer.
3. The remaining 6 pages, built in parallel, one file each, against the reference page.
4. Integration:
   - `cargo clippy`, tests, a headless-safe smoke run, and a protocol loopback against a fake serial device.
   - A README section in `docs/`.

## Acceptance criteria
- Builds with `cargo build --release` on Windows (verified here). No platform-specific code is left unguarded, so macOS and Linux compile by construction; that is checked with `cargo check --target` where toolchains allow.
- Every send call site in the original has a Rust equivalent that produces identical bytes (unit tests for the builders).
- All 7 pages are reachable, the zh/en toggle works, connect/disconnect works over serial and TCP, the log shows TX/RX hex and device text, and the 3D button streams to Rerun.

## Non-goals
- Folding `sync_host_gui` into the app.
- Firmware analysis or flashing.
- Mobile or web targets.
