# Synchronizer page (port of sync_host_gui.exe)

Status: implemented (2026-10-13). Tests and clippy pass, and the app launches. The page has not been exercised against a real leader board.

## Outcome
The app gets an 8th sidebar page, "Synchronizer", that does everything `sync_host_gui.exe` does, over the app's existing connection and log. This reverses the "Folding `sync_host_gui` into the app" non-goal of [`../261013-2123-gpui-port/plan.md`](../261013-2123-gpui-port/plan.md). The user chose "Synchronizer page only". A firmware flasher stays out of scope.

Source of truth: `../../recovered/sync_host_gui/sync_host_gui.py`.

## Design
- `src/sync.rs` holds the task logic, ported 1:1 from `SyncHostApp`. It covers:
  - request/wait with retries;
  - the 74 → 99 → register-write baud-recovery chain;
  - centring, calibration, offsets and torque.

  The logic runs against a `Bus` trait, so it is testable without hardware. It reports progress as `Note::{Log, Offset}`.
- `AppState::open_frame_tap()` hands a task the `Arc<Link>` and a frame channel. While the tap is alive:
  - frames go only to the task, and the hex log still records them;
  - no `Response` or `DeviceEvent::Frame` is emitted, because the 2-byte register-read replies would otherwise be misparsed as `ServoPosition`.

  The tap is dropped on disconnect or link loss, and the task then aborts and releases the port.
- `src/ui/pages/sync.rs` runs each task on its own thread. It disables its buttons while a task runs or while disconnected. It saves the centre position, the selected servo and the offsets in `Settings.sync`.

## Acceptance criteria
- Every frame sequence that sync_host_gui sends for each task, including scripted-reply branches, is byte-identical. This is checked by `tests/sync_golden_bytes.rs` against `tests/sync_golden_bytes.json`, which `tools/gen_sync_golden_bytes.py` generates from the recovered Python.
- `cargo test` and `cargo clippy --all-targets -- -D warnings` pass.
- The page is reachable and its strings exist in both en and vi.
- `docs/README.md` documents the page.

## Deliberate differences
- The original's "move time" field is dropped: it was saved but never sent.
- The duplicate "all torque on/off" buttons in the single-servo group are dropped.
- Calibrate asks for confirmation, as the Action Group page already does.
- The baud rate is whatever the connection panel uses. The page hints that the leader board needs 1,000,000.
