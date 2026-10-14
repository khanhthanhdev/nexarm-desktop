# NexArm Desktop

A native, cross-platform controller for the NexArm robot arm, written in Rust with [GPUI](https://www.gpui.rs/) (`gpui-kit` 0.7.3). It replaces the original PyQt5 `NexArm.exe` and keeps all 7 of its pages (Action Group, Coordinates, Peripheral & Chassis, System, AI, Servo Advanced, Teach Edit) and its HEX and text log. Its UI is in English and Vietnamese. An 8th page, **Synchronizer**, replaces the separate `sync_host_gui.exe` service tool. The wire protocol is byte-identical to the original. The protocol and the original client are documented in [`../../docs/nexarm-gui-reverse-engineering.md`](../../docs/nexarm-gui-reverse-engineering.md).

## Build and run

```sh
cargo run --release
```

The app looks for its assets (STL meshes, URDF, images) in this order:
1. `$NEXARM_ASSETS`
2. `assets/` next to the executable
3. `../Resources/assets` (macOS bundle)
4. the source tree

To ship it, copy `assets/` next to the binary.

### Linux system packages

```sh
sudo apt-get install pkg-config libudev-dev libxkbcommon-dev libxkbcommon-x11-dev \
  libwayland-dev libvulkan-dev libx11-xcb-dev libxcb1-dev libfontconfig-dev libfreetype-dev
```

To open serial ports without root, add your user to the `dialout` group (on Arch, the `uucp` group).

## Connecting

- **Serial (USB):** pick the port and baud rate (default 1,000,000), then flip the switch.
- **WiFi:** type or scan the robot's SSID and enter the password. The app then joins the access point and opens TCP to `192.168.4.1:8080`. The host and port are stored in the settings file as `wifi_host` and `wifi_port`.
  - Joining uses the OS tool, run without a shell:
    - Windows: `netsh wlan`
    - Linux: `nmcli`
    - macOS: `networksetup`
  - Scanning is not available on macOS. Type the SSID instead.
  - On Linux and macOS, the WiFi password is passed to `nmcli` or `networksetup` as a command argument, so it is briefly visible in the process list.

Passwords are stored in the OS keychain (Windows Credential Manager, macOS Keychain, or Secret Service on Linux), never in the settings file. Settings are stored in `<OS config dir>/NexArm/settings.json`.

## 3D view

The **3D View** button opens a [Rerun](https://rerun.io) viewer window and streams the arm pose to it. The viewer must be version **0.38.1** and on `PATH`. Install it with one of:

```sh
cargo binstall --force rerun-cli@0.38.1
pip install rerun-sdk==0.38.1
```

On Windows machines without Vulkan or OpenGL drivers (for example a VM or Windows Server), the viewer exits with "No graphics adapter was found". In that case, start the app with `WGPU_BACKEND=dx12` set. The viewer inherits it and renders through DirectX 12, falling back to the software rasterizer if there is no GPU.

## Synchronizer page

This page services the leader arm (the "synchronizer" board running Nex_Arm_sys firmware). It does everything `sync_host_gui.exe` did:
- set every servo's baud rate back to 1M (trying command 74, then 0x63, then direct register writes);
- move one or all servos to a centre position;
- calibrate the centre;
- switch torque on or off for all servos;
- read the current positions;
- read, write and zero each servo's position offset.

Connect to the leader board over USB serial at 1,000,000 baud as usual.

One task runs at a time, and the page's buttons are disabled while it runs. Progress goes to the text log, and the frames go to the HEX log. While a task runs it owns the link, so other pages don't react to its replies. Disconnecting stops the task.

The centre position, selected servo and offsets are saved in the settings file under `sync`, both after each task and when you leave a field.

## Differences from the original

- Received frames are checksum-checked; frames that fail are shown in red in the HEX log and are not acted on. The short checksum that the leader firmware uses for some replies is accepted.
- The baud selector is honored. The original always used 1,000,000.
- WiFi passwords are kept in the OS keychain instead of plain-text `QSettings`.
- OS commands never go through a shell, so an SSID cannot inject commands.
- Synchronizer page, compared with `sync_host_gui.exe`:
  - The "move time" field is gone, because the original saved it but never sent it.
  - Calibrating asks for confirmation first.
  - The duplicate "all torque on/off" buttons in the single-servo group are gone.
  - Every action, including the per-row ones, runs as a task that is logged, one at a time. The original ran per-row actions directly on the UI thread.
  - The serial baud rate comes from the connection panel instead of being fixed at 1,000,000.

## Development

```sh
cargo test                                         # unit tests + golden bytes
cargo test --lib keychain -- --ignored             # real OS keychain round trip
python -I tools/gen_golden_bytes.py                # regenerate tests/golden_bytes.json (needs PyQt5 + ../recovered/NexArm)
```

`tests/golden_bytes.rs` checks that every send site of the original client produces the same bytes as the Rust request builders. `tests/sync_golden_bytes.rs` does the same for the Synchronizer page. It compares each task's full frame sequence, including the branches that depend on replies, against `sync_host_gui`. To regenerate its data, run `py -I tools/gen_sync_golden_bytes.py`, which needs only the standard library.

CI (`.github/workflows/ci.yml`) runs clippy, the tests and a release build on Windows, macOS and Linux.
