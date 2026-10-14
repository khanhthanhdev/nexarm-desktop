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

## Lab pages (the policy loop)

Five more pages run the full imitation-learning loop. They talk to the local **engine**, `lerobot-nexarm-service` from [lerobot-nexarm](../../../lerobot-nexarm/docs/service/README.md), over HTTP and one WebSocket. The original eight pages do not need the engine.

| Page         | What it does                                                                                                                                                                      |
| ------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Lab: Session | Connect to (or start) the engine, edit the robot profile (ports, cameras), check leader and follower alignment, run teleoperation.                                                 |
| Lab: Record  | Record episodes with pause, next, re-record and change-task controls. Output matches `lerobot-record`, so the existing training scripts accept it.                                 |
| Lab: Datasets | Browse datasets; view an episode (decoded frame plus joint curves); exclude episodes without deleting anything; delete, split, merge or rename into new datasets; Hub import and upload. |
| Lab: Train   | Queue training jobs, follow the loss and learning-rate curves and the log, and run on this machine or on a GPU server over SSH.                                                    |
| Lab: Deploy  | Roll out a checkpoint, evaluate it into an `eval_*` dataset, or coach it (DAgger): take over, hand back, hold, reset to rest, drop a correction.                                    |
| Lab: Remote  | Host the follower for a remote operator (a station), or drive a remote station with the leader on this machine.                                                                    |

Setup: open **Lab: Session**, press **Connect**. With the default settings the app starts the engine (`lerobot-nexarm-service` on `PATH`; set the start command and working folder in the same panel, for example `uv run lerobot-nexarm-service` in a checkout). It refuses an engine whose API version differs from the app's. Create a robot profile (follower and leader ports, one entry per camera) before recording or deploying. Cameras are per profile; there are no defaults.

**Links during a session.** The engine opens the robot's USB ports, so the app first releases its own connection (the left panel shows Disconnected) and reconnects when the engine reports the session is over, retrying for about 25 s while the COM port re-enumerates. If the follower is connected over WiFi, the same applies to the TCP link.

**Safety.** Every session returns the follower to the pose it started in before torque is released. If the app closes or goes silent for 60 s, the engine stops the session safely. Soft limits and the gripper clamp are enforced by the engine. Zero offsets are fixed in the servos (follower: Servo Advanced page; leader: Synchronizer page); the engine's calibration files are identity.

**GPU servers.** In Lab: Train add a server (host, SSH user, optional key file; keys otherwise stay with your SSH agent), press Test connection, then pick it under "Run on". Prepare a fresh Linux server once with `scripts/nexarm-gpu-bootstrap.sh` from lerobot-nexarm. No port is exposed: the app uses an SSH tunnel to the server's loopback engine.

**Remote teleoperation.** On the station machine open Lab: Remote, set a listen address (your tailnet or LAN address) and press Apply: the engine restarts listening there and requires the generated token. Give the operator the shown address and the token. The operator enters them under "Operator" and presses Connect. Operator tokens are kept in the OS keychain.

**Settings.** `settings.json` gains `engine` (url, start command, working folder, auto start, bind address, selected profile), `servers` and `remote_station`. The engine token and the station tokens are kept in the OS keychain, never in the file. Set `NEXARM_CONFIG_DIR` to use another settings folder (the keychain is then not used, which is what the UI tests rely on).

Hardware checks that have not been run yet are in [`hardware-checklist.md`](hardware-checklist.md).

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
cargo test --test engine_contract                  # routes the client uses exist in the engine's OpenAPI copy
cargo test --test protocol_vectors                 # CommProtocol bytes shared with the lerobot-nexarm driver
cargo test --test lab_ui                           # headless UI tests of the Lab pages against a mock engine
sh scripts/sync-engine-contract ../../lerobot-nexarm    # refresh tests/fixtures/openapi.json and events.json
sh scripts/sync-protocol-vectors ../../lerobot-nexarm   # refresh tests/fixtures/nexarm_protocol_vectors.json
# with an engine running (lerobot-nexarm-service) and a dataset such as me/demo under $HF_LEROBOT_HOME:
cargo test --test engine_live -- --ignored         # the real client against a real engine
python -I tools/gen_golden_bytes.py                # regenerate tests/golden_bytes.json (needs PyQt5 + ../recovered/NexArm)
```

`tests/golden_bytes.rs` checks that every send site of the original client produces the same bytes as the Rust request builders. `tests/sync_golden_bytes.rs` does the same for the Synchronizer page. It compares each task's full frame sequence, including the branches that depend on replies, against `sync_host_gui`. To regenerate its data, run `py -I tools/gen_sync_golden_bytes.py`, which needs only the standard library.

CI (`.github/workflows/ci.yml`) runs clippy, the tests and a release build on Windows, macOS and Linux.
