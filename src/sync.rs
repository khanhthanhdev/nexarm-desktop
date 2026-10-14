//! Leader-arm ("synchronizer", Nex_Arm_sys firmware) service tasks, ported from
//! `sync_host_gui.exe` (../recovered/sync_host_gui/sync_host_gui.py).
//!
//! Tasks are sequential request/reply exchanges with fixed timeouts and pauses, so
//! they run on a worker thread against a [`Bus`] rather than through page events.

use crate::link::Link;
use crate::protocol::{BROADCAST_ID, Frame, Request, SYS_ID, cmd, servo};
use anyhow::bail;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const SERVO_IDS: std::ops::RangeInclusive<u8> = 1..=6;
pub const DEFAULT_CENTER_POS: u16 = 2048;
/// Status byte of a successful direct servo reply.
const SERVO_OK: u8 = 0;

/// Frame transport for a task.
pub trait Bus {
    /// Send one request; an error aborts the task.
    fn send(&mut self, req: &Request) -> anyhow::Result<()>;
    /// Next received frame, or `None` once `timeout` elapses. An error aborts the task.
    fn recv(&mut self, timeout: Duration) -> anyhow::Result<Option<Frame>>;
    /// Discard frames received so far.
    fn drain(&mut self);
}

/// The app's link plus a frame tap from [`crate::ui::state::AppState::open_frame_tap`].
pub struct LinkBus {
    pub link: Arc<Link>,
    pub frames: flume::Receiver<Frame>,
}

impl Bus for LinkBus {
    fn send(&mut self, req: &Request) -> anyhow::Result<()> {
        // The app drops the tap on disconnect; stop writing to a port the user closed.
        if self.frames.is_disconnected() {
            bail!("disconnected");
        }
        self.link.send(req)
    }

    fn recv(&mut self, timeout: Duration) -> anyhow::Result<Option<Frame>> {
        match self.frames.recv_timeout(timeout) {
            Ok(f) => Ok(Some(f)),
            Err(flume::RecvTimeoutError::Timeout) => Ok(None),
            Err(flume::RecvTimeoutError::Disconnected) => bail!("disconnected"),
        }
    }

    fn drain(&mut self) {
        self.frames.drain();
    }
}

/// Progress reported by a running task.
#[derive(Clone, Debug, PartialEq)]
pub enum Note {
    Log(String),
    /// Offset of a servo, as read back or as about to be written.
    Offset(u8, i16),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Task {
    ReadPositions,
    RecoverBaud1M,
    CenterAll(u16),
    CenterOne(u8, u16),
    CalibrateAll,
    CalibrateOne(u8),
    ReadOffset(u8),
    /// Also used for "zero" with an offset of 0.
    WriteOffset(u8, i16),
    ReadAllOffsets,
    WriteAllOffsets([i16; 6]),
    ZeroAllOffsets,
    Torque(bool),
}

/// Sign-magnitude encoding used by the servo registers (`sign_bit` 15 for positions, 11 for offsets).
pub fn encode_signed(value: i32, sign_bit: u8) -> u16 {
    if value < 0 { ((-value) as u32 | 1 << sign_bit) as u16 } else { value as u16 }
}

pub fn decode_signed(raw: u16, sign_bit: u8) -> i32 {
    let bit = 1u16 << sign_bit;
    if raw & bit != 0 { -((raw & !bit) as i32) } else { raw as i32 }
}

/// Servo ids whose bit is set in a firmware reply mask.
fn ids_in_mask(mask: u8) -> Vec<String> {
    SERVO_IDS.filter(|sid| mask & 1 << (sid - 1) != 0).map(|sid| sid.to_string()).collect()
}

fn positions_text(p: &[i16; 6]) -> String {
    p.iter().enumerate().map(|(i, v)| format!("ID{}={v}", i + 1)).collect::<Vec<_>>().join(", ")
}

/// Which received frames a request waits for.
enum Match {
    /// Only replies with the request's command code.
    SameCmd,
    Any,
}

pub struct Session<B: Bus, N: FnMut(Note)> {
    bus: B,
    note: N,
    pause: fn(Duration),
}

impl<B: Bus, N: FnMut(Note)> Session<B, N> {
    pub fn new(bus: B, note: N) -> Self {
        Self { bus, note, pause: std::thread::sleep }
    }

    /// Replace the pauses between frames (tests skip them).
    pub fn with_pause(mut self, pause: fn(Duration)) -> Self {
        self.pause = pause;
        self
    }

    fn log(&mut self, text: impl Into<String>) {
        (self.note)(Note::Log(text.into()));
    }

    fn sleep(&self, secs: f64) {
        (self.pause)(Duration::from_secs_f64(secs));
    }

    pub fn run(&mut self, task: &Task) -> anyhow::Result<()> {
        match *task {
            Task::ReadPositions => match self.read_positions(1.0)? {
                Some(p) => self.log(format!("[POS] {}", positions_text(&p))),
                None => self.log("[POS] read failed; check that the synchronizer runs Nex_Arm_sys firmware"),
            },
            Task::RecoverBaud1M => self.recover_baud_1m()?,
            Task::CenterAll(pos) => {
                let pos = pos.min(4095);
                self.send_lr_positions([pos; 6])?;
                self.sleep(0.04);
                self.send_sync_positions([pos; 6])?;
                self.log(format!("[CENTER] all servos pos={pos}"));
            }
            Task::CenterOne(sid, pos) => self.center(sid, pos)?,
            Task::CalibrateAll => {
                for sid in SERVO_IDS {
                    self.calibrate(sid)?;
                    self.sleep(0.08);
                }
            }
            Task::CalibrateOne(sid) => self.calibrate(sid)?,
            Task::ReadOffset(sid) => {
                self.read_offset(sid)?;
            }
            Task::WriteOffset(sid, offset) => self.write_offset(sid, offset)?,
            Task::ReadAllOffsets => {
                for sid in SERVO_IDS {
                    self.read_offset(sid)?;
                    self.sleep(0.05);
                }
            }
            Task::WriteAllOffsets(offsets) => {
                for (sid, offset) in SERVO_IDS.zip(offsets) {
                    self.write_offset(sid, offset)?;
                    self.sleep(0.06);
                }
            }
            Task::ZeroAllOffsets => {
                for sid in SERVO_IDS {
                    self.write_offset(sid, 0)?;
                    self.sleep(0.06);
                }
            }
            Task::Torque(on) => self.set_torque(on)?,
        }
        Ok(())
    }

    // ------------------------------------------------------------ exchange

    /// Send `req` up to `attempts` times; return the first reply accepted by `accept`.
    fn request(
        &mut self,
        req: Request,
        which: Match,
        timeout: f64,
        attempts: u32,
        accept: impl Fn(&Frame) -> bool,
    ) -> anyhow::Result<Option<Frame>> {
        let wanted = |f: &Frame| matches!(which, Match::Any) || f.cmd == req.cmd;
        for _ in 0..attempts.max(1) {
            self.bus.drain();
            self.bus.send(&req)?;
            let deadline = Instant::now() + Duration::from_secs_f64(timeout);
            loop {
                let left = deadline.saturating_duration_since(Instant::now()).max(Duration::from_millis(1));
                match self.bus.recv(left)? {
                    Some(f) if wanted(&f) && accept(&f) => return Ok(Some(f)),
                    Some(_) if Instant::now() < deadline => {}
                    _ => break,
                }
            }
        }
        Ok(None)
    }

    fn write_register(&mut self, sid: u8, addr: u8, data: &[u8], delay_s: f64) -> anyhow::Result<()> {
        let mut args = vec![addr];
        args.extend_from_slice(data);
        self.bus.send(&Request { id: sid, cmd: servo::WRITE, args })?;
        if delay_s > 0.0 {
            self.sleep(delay_s);
        }
        Ok(())
    }

    fn read_register(&mut self, sid: u8, addr: u8, size: u8, timeout: f64, attempts: u32) -> anyhow::Result<Option<Vec<u8>>> {
        let req = Request { id: sid, cmd: servo::READ, args: vec![addr, size] };
        let reply = self.request(req, Match::Any, timeout, attempts, |f| {
            f.id == sid && f.cmd == SERVO_OK && f.data.len() >= size as usize
        })?;
        Ok(reply.map(|f| f.data[..size as usize].to_vec()))
    }

    fn read_positions(&mut self, timeout: f64) -> anyhow::Result<Option<[i16; 6]>> {
        let req = Request::sys(cmd::LR_READ_POS, []);
        let reply = self.request(req, Match::SameCmd, timeout, 2, |f| f.id == SYS_ID && f.data.len() >= 12)?;
        Ok(reply.map(|f| std::array::from_fn(|k| i16::from_le_bytes([f.data[2 * k], f.data[2 * k + 1]]))))
    }

    fn read_baud_code(&mut self, sid: u8) -> anyhow::Result<Option<u8>> {
        let req = Request::sys(cmd::READ_BAUD, [sid]);
        let reply = self.request(req, Match::SameCmd, 0.7, 1, |f| f.data.len() >= 2 && f.data[0] == sid)?;
        if let Some(f) = reply {
            return Ok(Some(f.data[1]));
        }
        Ok(self.read_register(sid, servo::REG_BAUD_RATE, 1, 0.5, 1)?.map(|d| d[0]))
    }

    // --------------------------------------------------------------- tasks

    /// Try the main-controller command (74), then the Nex_Arm_sys command (0x63),
    /// then write the baud register directly and read it back.
    fn recover_baud_1m(&mut self) -> anyhow::Result<()> {
        let req = Request::sys(cmd::SET_BAUD, [BROADCAST_ID, 0]);
        let reply = self.request(req, Match::SameCmd, 10.0, 1, |f| {
            f.data.len() >= 3 && f.data[0] == BROADCAST_ID && f.data[1] == 0
        })?;
        match reply {
            Some(f) => {
                let ok = ids_in_mask(f.data[2]);
                if !ok.is_empty() {
                    self.log(format!("[BAUD] main controller restored 1M: {}", ok.join(", ")));
                    return Ok(());
                }
                self.log("[BAUD] main controller ran the recovery but saw no servos; trying Nex_Arm_sys command 0x63");
            }
            None => self.log("[BAUD] no reply to command 74; trying Nex_Arm_sys command 0x63"),
        }

        let req = Request::sys(cmd::SYNC_RECOVER_BAUD_1M, []);
        if let Some(f) = self.request(req, Match::SameCmd, 4.0, 1, |f| f.id == SYS_ID && f.data.len() >= 2)? {
            let ok = ids_in_mask(f.data[1]);
            if ok.is_empty() {
                self.log("[BAUD] firmware ran the recovery but saw no servos");
            } else {
                self.log(format!("[BAUD] firmware restored/verified 1M: {}", ok.join(", ")));
            }
            return Ok(());
        }

        self.log("[BAUD] firmware did not answer; writing the baud register directly (broadcast and per id)");
        self.write_register(BROADCAST_ID, servo::REG_BAUD_RATE, &[0], 0.08)?;
        for sid in SERVO_IDS {
            self.write_register(sid, servo::REG_BAUD_RATE, &[0], 0.03)?;
        }
        let (mut ok, mut failed) = (Vec::new(), Vec::new());
        for sid in SERVO_IDS {
            match self.read_baud_code(sid)? {
                Some(0) => ok.push(sid.to_string()),
                Some(code) => failed.push(format!("{sid}:{code}")),
                None => failed.push(format!("{sid}:no reply")),
            }
        }
        if !ok.is_empty() {
            self.log(format!("[BAUD] read back 1M: {}", ok.join(", ")));
        }
        if !failed.is_empty() {
            self.log(format!("[BAUD] not confirmed at 1M: {}", failed.join(", ")));
            self.log("[BAUD] servos not at 1M can only be recovered by firmware that supports the 74/0x63 cross-baud scan");
        }
        Ok(())
    }

    fn center(&mut self, sid: u8, pos: u16) -> anyhow::Result<()> {
        let pos = pos.min(4095);
        let [lo, hi] = encode_signed(pos as i32, 15).to_le_bytes();
        self.write_register(sid, servo::REG_ACC, &[0, lo, hi, 0, 0, 0, 0], 0.02)?;
        self.log(format!("[CENTER] servo {sid}: pos={pos}"));
        Ok(())
    }

    fn send_lr_positions(&mut self, positions: [u16; 6]) -> anyhow::Result<()> {
        let args: Vec<u8> = positions.iter().flat_map(|p| (p.min(&4095)).to_le_bytes()).collect();
        self.bus.send(&Request::sys(cmd::LR_WRITE_POS, args))
    }

    fn send_sync_positions(&mut self, positions: [u16; 6]) -> anyhow::Result<()> {
        let mut args = vec![servo::REG_ACC, 7];
        for (sid, pos) in SERVO_IDS.zip(positions) {
            let [lo, hi] = encode_signed(pos.min(4095) as i32, 15).to_le_bytes();
            args.extend([sid, 0, lo, hi, 0, 0, 0, 0]);
        }
        self.bus.send(&Request { id: BROADCAST_ID, cmd: servo::SYNC_WRITE, args })
    }

    /// Writing 128 to the torque register makes the servo take its current position as centre.
    fn calibrate(&mut self, sid: u8) -> anyhow::Result<()> {
        self.bus.send(&Request { id: sid, cmd: servo::WRITE, args: vec![servo::REG_TORQUE, 128] })?;
        self.log(format!("[CALI] servo {sid}: calibrated centre"));
        Ok(())
    }

    fn read_offset(&mut self, sid: u8) -> anyhow::Result<Option<i16>> {
        let Some(d) = self.read_register(sid, servo::REG_POS_OFFSET, 2, 0.8, 2)? else {
            self.log(format!("[OFFSET] servo {sid}: no reply"));
            return Ok(None);
        };
        let offset = decode_signed(u16::from_le_bytes([d[0], d[1]]), 11) as i16;
        (self.note)(Note::Offset(sid, offset));
        self.log(format!("[OFFSET] servo {sid}: offset={offset}"));
        Ok(Some(offset))
    }

    fn write_offset(&mut self, sid: u8, offset: i16) -> anyhow::Result<()> {
        let offset = offset.clamp(-2047, 2047);
        (self.note)(Note::Offset(sid, offset));
        let raw = encode_signed(offset as i32, 11).to_le_bytes();
        self.write_register(sid, servo::REG_POS_OFFSET, &raw, 0.03)?;
        self.sleep(0.05);
        self.read_offset(sid)?;
        Ok(())
    }

    fn set_torque(&mut self, on: bool) -> anyhow::Result<()> {
        let v = on as u8;
        let req = Request::sys(cmd::LR_TORQUE, [v]);
        let reply = self.request(req, Match::SameCmd, 0.8, 1, |f| f.id == SYS_ID && !f.data.is_empty())?;
        self.bus.send(&Request { id: BROADCAST_ID, cmd: servo::WRITE, args: vec![servo::REG_TORQUE, v] })?;
        let label = if on { "on" } else { "off" };
        if reply.is_none() {
            self.log(format!("[TORQUE] sent torque {label} to all servos; no LeRobot reply"));
        } else {
            self.log(format!("[TORQUE] all servos torque {label}"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::encode;
    use std::collections::VecDeque;

    /// Replies to a sent frame with the scripted frames for its exact bytes.
    #[derive(Default)]
    struct Script {
        sent: Vec<Request>,
        replies: Vec<(Vec<u8>, Vec<Frame>)>,
        inbox: VecDeque<Frame>,
    }

    fn frame(id: u8, cmd: u8, data: &[u8]) -> Frame {
        Frame { id, cmd, data: data.to_vec(), raw: encode(id, cmd, data) }
    }

    impl Bus for &mut Script {
        fn send(&mut self, req: &Request) -> anyhow::Result<()> {
            let bytes = req.bytes();
            self.sent.push(req.clone());
            if let Some((_, r)) = self.replies.iter().find(|(b, _)| *b == bytes) {
                self.inbox.extend(r.iter().cloned());
            }
            Ok(())
        }
        fn recv(&mut self, _: Duration) -> anyhow::Result<Option<Frame>> {
            Ok(self.inbox.pop_front())
        }
        fn drain(&mut self) {
            self.inbox.clear();
        }
    }

    fn run(script: &mut Script, task: Task) -> Vec<Note> {
        let mut notes = Vec::new();
        Session::new(script, |n| notes.push(n)).with_pause(|_| {}).run(&task).unwrap();
        notes
    }

    fn cmds(s: &Script) -> Vec<(u8, u8)> {
        s.sent.iter().map(|r| (r.id, r.cmd)).collect()
    }

    #[test]
    fn signed_codec_matches_original() {
        assert_eq!(encode_signed(-1, 11), 0x0801);
        assert_eq!(encode_signed(-2047, 11), 0x0FFF);
        assert_eq!(encode_signed(2047, 11), 2047);
        for v in [-2047, -1, 0, 1, 2047] {
            assert_eq!(decode_signed(encode_signed(v, 11), 11), v);
        }
    }

    #[test]
    fn baud_recovery_stops_at_main_controller_reply() {
        let mut s = Script::default();
        let req = Request::sys(cmd::SET_BAUD, [BROADCAST_ID, 0]).bytes();
        s.replies.push((req, vec![frame(SYS_ID, cmd::SET_BAUD, &[BROADCAST_ID, 0, 0b0000_0101])]));
        let notes = run(&mut s, Task::RecoverBaud1M);
        assert_eq!(cmds(&s), [(SYS_ID, cmd::SET_BAUD)]);
        assert_eq!(notes, [Note::Log("[BAUD] main controller restored 1M: 1, 3".into())]);
    }

    #[test]
    fn baud_recovery_falls_back_to_sys_command() {
        let mut s = Script::default();
        let req = Request::sys(cmd::SET_BAUD, [BROADCAST_ID, 0]).bytes();
        s.replies.push((req, vec![frame(SYS_ID, cmd::SET_BAUD, &[BROADCAST_ID, 0, 0])]));
        let req = Request::sys(cmd::SYNC_RECOVER_BAUD_1M, []).bytes();
        s.replies.push((req, vec![frame(SYS_ID, cmd::SYNC_RECOVER_BAUD_1M, &[0, 0b11_1111])]));
        let notes = run(&mut s, Task::RecoverBaud1M);
        assert_eq!(cmds(&s), [(SYS_ID, cmd::SET_BAUD), (SYS_ID, cmd::SYNC_RECOVER_BAUD_1M)]);
        assert!(matches!(notes.last(), Some(Note::Log(t)) if t.ends_with("1, 2, 3, 4, 5, 6")));
    }

    #[test]
    fn baud_recovery_writes_registers_when_firmware_is_silent() {
        let mut s = Script::default();
        // Servo 2 answers the baud query; servo 4 answers only the register read.
        s.replies.push((Request::sys(cmd::READ_BAUD, [2]).bytes(), vec![frame(SYS_ID, cmd::READ_BAUD, &[2, 0])]));
        let reg = Request { id: 4, cmd: servo::READ, args: vec![servo::REG_BAUD_RATE, 1] }.bytes();
        s.replies.push((reg, vec![frame(4, SERVO_OK, &[3])]));
        let notes = run(&mut s, Task::RecoverBaud1M);
        let c = cmds(&s);
        assert_eq!(c[2], (BROADCAST_ID, servo::WRITE));
        assert_eq!(c[3..9].iter().map(|x| x.0).collect::<Vec<_>>(), [1, 2, 3, 4, 5, 6]);
        // 6 baud queries, plus a register read for each id that did not answer the query.
        assert_eq!(c.len(), 9 + 6 + 5);
        let logs: Vec<_> = notes.iter().map(|n| format!("{n:?}")).collect();
        assert!(logs.iter().any(|l| l.contains("read back 1M: 2\"")));
        assert!(logs.iter().any(|l| l.contains("1:no reply, 3:no reply, 4:3, 5:no reply, 6:no reply")));
    }

    #[test]
    fn requests_ignore_unrelated_frames() {
        let mut s = Script::default();
        let req = Request { id: 3, cmd: servo::READ, args: vec![servo::REG_POS_OFFSET, 2] }.bytes();
        let reply = encode_signed(-100, 11).to_le_bytes();
        s.replies.push((req, vec![frame(SYS_ID, cmd::GET_CUR_COORDS, &[0; 24]), frame(3, SERVO_OK, &reply)]));
        let notes = run(&mut s, Task::ReadOffset(3));
        assert_eq!(s.sent.len(), 1);
        assert_eq!(notes[0], Note::Offset(3, -100));
    }

    #[test]
    fn silent_offset_read_is_sent_twice() {
        let mut s = Script::default();
        let notes = run(&mut s, Task::ReadOffset(5));
        assert_eq!(s.sent.len(), 2);
        assert_eq!(notes, [Note::Log("[OFFSET] servo 5: no reply".into())]);
    }
}
