//! NexArm wire protocol: `FF FF | ID | LEN | CMD | ARGS.. | CHK`.
//!
//! `LEN = 2 + args.len()`, `CHK = !(ID + LEN + CMD + sum(args))`. Multi-byte
//! fields are little-endian. Layouts mirror the original NexArm Qt client
//! (see docs/nexarm-gui-reverse-engineering.md).

/// System controller address used by every `send_sys` call.
pub const SYS_ID: u8 = 0xFF;
/// Secondary MCU that may also answer system requests.
pub const AT32_ID: u8 = 0x5A;
/// Servo bus broadcast address.
pub const BROADCAST_ID: u8 = 0xFE;

/// System command codes (sent to [`SYS_ID`]).
pub mod cmd {
    pub const FIRMWARE_VERSION_CHECK: u8 = 1;
    pub const CHECK_BAT_LEVEL: u8 = 2;
    pub const ACTION_GROUP_RUN: u8 = 3;
    pub const ACTION_GROUP_STOP: u8 = 4;
    pub const ACTION_GROUP_DOWNLOAD: u8 = 5;
    pub const FKINE_RESULT_GET: u8 = 6;
    pub const COORDINATE_SET: u8 = 8;
    pub const BUZZER_SET: u8 = 9;
    pub const GET_CUR_COORDS: u8 = 11;
    pub const SET_SINGLE_MOTOR: u8 = 13;
    pub const STOP_ALL_MOTOR: u8 = 14;
    pub const SET_MOTOR_SPEED: u8 = 15;
    pub const CONVEYOR_SET: u8 = 16;
    pub const STEPPER_RESET: u8 = 17;
    pub const STEPPER_DIV: u8 = 18;
    pub const STEPPER_RUN: u8 = 19;
    pub const ACTION_GROUP_ERASE: u8 = 23;
    pub const SET_ESPNOW_CHANNEL: u8 = 30;
    pub const SET_GLOBAL_ACC: u8 = 31;
    pub const ESPNOW_SYNC_CTRL: u8 = 33;
    pub const MECANUM_CONTROL: u8 = 34;
    pub const TANK_CONTROL: u8 = 35;
    pub const SET_PEER_MAC: u8 = 36;
    pub const COLOR_TRACK: u8 = 40;
    pub const FACE_TRACK: u8 = 41;
    pub const APRILTAG_TRACK: u8 = 43;
    pub const APRILTAG_GRAB: u8 = 44;
    pub const APRILTAG_SET_OFFSET: u8 = 45;
    pub const COLOR_GRAB: u8 = 46;
    pub const LLM_CONTROL: u8 = 47;
    pub const GARBAGE_GRAB: u8 = 48;
    pub const CALIBRATION: u8 = 49;
    pub const ARM_MOVE_INC: u8 = 50;
    pub const SET_SERVO_ID: u8 = 52;
    pub const SET_SERVO_MODE: u8 = 53;
    pub const ARM_RESET: u8 = 54;
    pub const READ_ALL_SERVOS: u8 = 55;
    pub const SET_MOVE_ACC: u8 = 56;
    pub const SET_POS_OFFSET: u8 = 57;
    pub const GET_POS_OFFSET: u8 = 58;
    pub const SET_PID_PARAM: u8 = 59;
    pub const GET_PID_PARAM: u8 = 60;
    pub const SET_TORQUE: u8 = 61;
    pub const SET_BT_MODE: u8 = 62;
    pub const SET_KINEMATICS_PARAM: u8 = 63;
    pub const GET_KINEMATICS_PARAM: u8 = 64;
    pub const GET_REAL_JOINT_ANGLES: u8 = 65;
    pub const LEROBOT_MODE: u8 = 68;
    pub const READ_OVERLOAD: u8 = 71;
    pub const SET_OVERLOAD: u8 = 72;
    pub const READ_BAUD: u8 = 73;
    pub const SET_BAUD: u8 = 74;
    pub const READ_MAX_TORQUE: u8 = 75;
    pub const SET_MAX_TORQUE: u8 = 76;
    pub const READ_ANGLE_LIMIT: u8 = 77;
    pub const SET_COORD_LIMITS: u8 = 79;
    pub const GET_COORD_LIMITS: u8 = 80;
    pub const GESTURE_TRACK: u8 = 81;
    pub const SET_PS3_MAC: u8 = 83;
    pub const FACTORY_RESET: u8 = 84;
    pub const SET_CHASSIS_CONFIG: u8 = 85;
    pub const GET_CHASSIS_CONFIG: u8 = 86;
    pub const SCAN_WIFI_CHANNELS: u8 = 87;
    pub const SERVO_CALI_POS: u8 = 88;
    pub const ACTION_EDIT_ENTER: u8 = 120;
    pub const ACTION_EDIT_EXIT: u8 = 121;
    pub const ACTION_EDIT_START: u8 = 122;
    pub const ACTION_EDIT_STOP: u8 = 123;
    pub const ACTION_EDIT_PLAY: u8 = 124;
    pub const ACTION_EDIT_PLAY_STOP: u8 = 125;
    pub const ACTION_EDIT_CLEAR: u8 = 126;
    pub const ACTION_EDIT_QUERY: u8 = 127;
    pub const SYNC_TEACH_ENTER: u8 = 128;
    pub const SYNC_TEACH_EXIT: u8 = 129;
    pub const SYNC_TEACH_REC_START: u8 = 130;
    pub const SYNC_TEACH_REC_STOP: u8 = 131;
    pub const SYNC_TEACH_PLAY: u8 = 132;
    pub const SYNC_TEACH_PLAY_STOP: u8 = 133;
    pub const SYNC_TEACH_CLEAR: u8 = 134;
    pub const SYNC_TEACH_QUERY: u8 = 135;
    /// Leader-arm ("synchronizer") firmware extensions.
    pub const LR_READ_POS: u8 = 0x60;
    pub const LR_WRITE_POS: u8 = 0x61;
    pub const LR_TORQUE: u8 = 0x62;
    pub const SYNC_RECOVER_BAUD_1M: u8 = 0x63;
}

/// Bus-servo instruction set and registers (Feetech STS-compatible).
pub mod servo {
    pub const READ: u8 = 2;
    pub const WRITE: u8 = 3;
    /// Broadcast multi-servo write (`addr, len_per_servo, [id, data..]..`).
    pub const SYNC_WRITE: u8 = 0x83;
    pub const REG_BAUD_RATE: u8 = 6;
    pub const REG_POS_OFFSET: u8 = 31;
    pub const REG_TORQUE: u8 = 40;
    pub const REG_ACC: u8 = 41;
    pub const REG_PRESENT_POS: u8 = 56;
}

pub fn checksum(body: &[u8]) -> u8 {
    !body.iter().fold(0u8, |a, b| a.wrapping_add(*b))
}

/// Encode one frame. `args` must fit in a u8 length (≤ 253 bytes).
pub fn encode(id: u8, cmd: u8, args: &[u8]) -> Vec<u8> {
    let len = u8::try_from(args.len() + 2).expect("frame payload too long");
    let mut f = Vec::with_capacity(args.len() + 6);
    f.extend_from_slice(&[0xFF, 0xFF, id, len, cmd]);
    f.extend_from_slice(args);
    f.push(checksum(&f[2..]));
    f
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub id: u8,
    pub cmd: u8,
    pub data: Vec<u8>,
    pub raw: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decoded {
    Frame(Frame),
    /// A well-delimited frame whose checksum matched neither accepted form.
    BadChecksum(Vec<u8>),
}

/// Leader firmware computes the checksum over `ID, LEN, CMD` only for these replies.
fn short_checksum_allowed(id: u8, cmd: u8) -> bool {
    id == SYS_ID
        && matches!(
            cmd,
            cmd::LEROBOT_MODE
                | cmd::LR_READ_POS
                | cmd::LR_TORQUE
                | cmd::READ_BAUD
                | cmd::SET_BAUD
                | cmd::SYNC_RECOVER_BAUD_1M
        )
}

/// Streaming frame decoder that resynchronises on `FF FF`.
#[derive(Default)]
pub struct Decoder {
    buf: Vec<u8>,
}

impl Decoder {
    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    pub fn next_decoded(&mut self) -> Option<Decoded> {
        loop {
            if self.buf.len() < 6 {
                return None;
            }
            if self.buf[0] != 0xFF || self.buf[1] != 0xFF {
                self.buf.remove(0);
                continue;
            }
            let len = self.buf[3] as usize;
            if !(2..=250).contains(&len) {
                self.buf.remove(0);
                continue;
            }
            let total = len + 4;
            if self.buf.len() < total {
                return None;
            }
            let raw = &self.buf[..total];
            let (id, cmd, chk) = (raw[2], raw[4], raw[total - 1]);
            let ok = checksum(&raw[2..total - 1]) == chk
                || (short_checksum_allowed(id, cmd) && checksum(&raw[2..5]) == chk);
            if !ok {
                // Drop one byte and resync so a false header cannot swallow real frames.
                let bad = raw.to_vec();
                self.buf.remove(0);
                return Some(Decoded::BadChecksum(bad));
            }
            let raw: Vec<u8> = self.buf.drain(..total).collect();
            return Some(Decoded::Frame(Frame { id, cmd, data: raw[5..total - 1].to_vec(), raw }));
        }
    }
}

/// Firmware prints plain text on the same link; chunks without 0xFF are shown as log text.
pub fn printable_text(chunk: &[u8]) -> Option<String> {
    if chunk.contains(&0xFF) {
        return None;
    }
    let text: String = String::from_utf8_lossy(chunk)
        .chars()
        .filter(|c| *c as u32 >= 32 || *c == '\r' || *c == '\n')
        .collect();
    let keepalive = text.chars().all(|c| "Z \r\n".contains(c));
    (!text.trim().is_empty() && !keepalive).then_some(text)
}

// ---------------------------------------------------------------- responses

#[derive(Clone, Debug, PartialEq)]
pub struct Pose {
    pub x: i16,
    pub y: i16,
    pub z: i16,
    /// Degrees (wire value is ×10).
    pub pitch: f32,
    pub roll: i16,
    pub claw: i16,
    pub servos: [i16; 6],
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChassisConfig {
    pub kind: u8,
    pub wheel_diameter: f32,
    pub wheel_base: f32,
    pub track_width: f32,
    pub max_speed: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TeachStatus {
    pub mode: u8,
    pub recording: u8,
    pub playing: u8,
    pub count: u16,
    /// Only reported by the sync-teach query.
    pub overflow: Option<u8>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Response {
    Pose(Pose),
    FirmwareVersion { esp: String, at32: Option<String> },
    Battery(u16),
    ServoOffset { id: u8, offset: i16 },
    ServoPid { id: u8, p: u8, i: u8, d: u8, min_force: u16 },
    ServoOverload { id: u8, torque: u8, time: u8, threshold: u8 },
    ServoBaud { id: u8, code: u8 },
    ServoMaxTorque { id: u8, torque: u16 },
    ServoAngleLimit { id: u8, a: u8, b: u8 },
    CoordLimits([i16; 12]),
    Chassis(ChassisConfig),
    Kinematics([f32; 9]),
    ChannelScan([(u8, i8); 13]),
    SyncTeach(TeachStatus),
    ActionEdit(TeachStatus),
    /// Raw servo counts from GET_REAL_JOINT_ANGLES (angle×10 values are ignored).
    JointPositions([Option<u16>; 6]),
    /// Direct servo READ reply of PRESENT_POSITION.
    ServoPosition { id: u8, pos: u16 },
}

fn i16_at(d: &[u8], i: usize) -> i16 {
    i16::from_le_bytes([d[i], d[i + 1]])
}
fn u16_at(d: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([d[i], d[i + 1]])
}
fn f32_at(d: &[u8], i: usize) -> f32 {
    f32::from_le_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]])
}

/// Servo offsets may arrive as int16 or as 11-bit sign-magnitude.
fn decode_offset(raw: u16) -> i16 {
    let signed = raw as i16;
    let v = if (-2047..=2047).contains(&signed) {
        signed
    } else {
        let r = raw & 0x0FFF;
        let mag = (r & 0x07FF) as i16;
        if r & 0x0800 != 0 { -mag } else { mag }
    };
    v.clamp(-2047, 2047)
}

fn servo_count(raw: i16) -> Option<u16> {
    let v = if raw < 0 { raw as i32 + 65536 } else { raw as i32 };
    (0..=4096).contains(&v).then_some(v as u16)
}

pub fn parse(f: &Frame) -> Option<Response> {
    let d = f.data.as_slice();
    let n = d.len();
    if (1..=6).contains(&f.id) && f.cmd == 0 && n == 2 {
        return servo_count(i16_at(d, 0)).map(|pos| Response::ServoPosition { id: f.id, pos });
    }
    if f.id != SYS_ID && f.id != AT32_ID {
        return None;
    }
    Some(match f.cmd {
        cmd::GET_CUR_COORDS | cmd::FKINE_RESULT_GET if n >= 24 => {
            let mut servos = [0i16; 6];
            for (k, s) in servos.iter_mut().enumerate() {
                *s = i16_at(d, 12 + 2 * k);
            }
            Response::Pose(Pose {
                x: i16_at(d, 0),
                y: i16_at(d, 2),
                z: i16_at(d, 4),
                pitch: i16_at(d, 6) as f32 / 10.0,
                roll: i16_at(d, 8),
                claw: i16_at(d, 10),
                servos,
            })
        }
        cmd::GET_REAL_JOINT_ANGLES if n >= 24 => {
            let mut p = [None; 6];
            for (k, s) in p.iter_mut().enumerate() {
                *s = servo_count(i16_at(d, 4 * k));
            }
            Response::JointPositions(p)
        }
        // Remaining replies are only honoured from the system controller.
        _ if f.id != SYS_ID && f.id != AT32_ID => return None,
        cmd::FIRMWARE_VERSION_CHECK if n >= 3 => Response::FirmwareVersion {
            esp: format!("{}.{}.{}", d[0], d[1], d[2]),
            at32: (n >= 6).then(|| format!("{}.{}.{}", d[3], d[4], d[5])),
        },
        cmd::CHECK_BAT_LEVEL if n >= 2 => Response::Battery(u16_at(d, 0)),
        cmd::GET_POS_OFFSET if n >= 3 => {
            Response::ServoOffset { id: d[0], offset: decode_offset(u16_at(d, 1)) }
        }
        cmd::GET_PID_PARAM if n >= 6 => {
            Response::ServoPid { id: d[0], p: d[1], i: d[2], d: d[3], min_force: u16_at(d, 4) }
        }
        cmd::GET_PID_PARAM if n >= 1 => {
            Response::ServoPid { id: d[0], p: 0, i: 0, d: 0, min_force: 0 }
        }
        cmd::READ_OVERLOAD if n >= 3 => Response::ServoOverload {
            id: d[0],
            torque: d[1],
            time: d[2],
            threshold: d.get(3).copied().unwrap_or(0),
        },
        cmd::READ_BAUD if n >= 2 => Response::ServoBaud { id: d[0], code: d[1] },
        cmd::READ_MAX_TORQUE if n >= 3 => Response::ServoMaxTorque { id: d[0], torque: u16_at(d, 1) },
        cmd::READ_ANGLE_LIMIT if n >= 3 => Response::ServoAngleLimit { id: d[0], a: d[1], b: d[2] },
        cmd::GET_COORD_LIMITS if n >= 24 => {
            let mut v = [0i16; 12];
            for (k, s) in v.iter_mut().enumerate() {
                *s = i16_at(d, 2 * k);
            }
            Response::CoordLimits(v)
        }
        cmd::GET_CHASSIS_CONFIG if n >= 18 => Response::Chassis(ChassisConfig {
            kind: d[0],
            wheel_diameter: f32_at(d, 1),
            wheel_base: f32_at(d, 5),
            track_width: f32_at(d, 9),
            max_speed: d[17],
        }),
        cmd::GET_KINEMATICS_PARAM if n >= 36 => {
            let mut v = [0f32; 9];
            for (k, s) in v.iter_mut().enumerate() {
                *s = f32_at(d, 4 * k);
            }
            Response::Kinematics(v)
        }
        cmd::SCAN_WIFI_CHANNELS if n >= 26 => {
            let mut v = [(0u8, 0i8); 13];
            for (k, s) in v.iter_mut().enumerate() {
                *s = (d[2 * k], d[2 * k + 1] as i8);
            }
            Response::ChannelScan(v)
        }
        cmd::SYNC_TEACH_QUERY if n >= 6 => Response::SyncTeach(TeachStatus {
            mode: d[0],
            recording: d[1],
            playing: d[2],
            count: u16_at(d, 3),
            overflow: Some(d[5]),
        }),
        cmd::ACTION_EDIT_QUERY if n >= 5 => Response::ActionEdit(TeachStatus {
            mode: d[0],
            recording: d[1],
            playing: d[2],
            count: u16_at(d, 3),
            overflow: None,
        }),
        _ => return None,
    })
}

// ----------------------------------------------------------------- requests

/// One outgoing message: destination id, command, payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub id: u8,
    pub cmd: u8,
    pub args: Vec<u8>,
}

impl Request {
    pub fn sys(cmd: u8, args: impl Into<Vec<u8>>) -> Self {
        Self { id: SYS_ID, cmd, args: args.into() }
    }
    pub fn bytes(&self) -> Vec<u8> {
        encode(self.id, self.cmd, &self.args)
    }
}

fn le16(v: impl Into<i32>) -> [u8; 2] {
    (v.into() as u16).to_le_bytes()
}

/// Absolute Cartesian move. Pitch first on the wire, ×10.
pub fn coordinate_set(x: i16, y: i16, z: i16, pitch_deg: f64, roll: i16, claw: i16, time_ms: u16) -> Request {
    let mut a = Vec::with_capacity(14);
    for v in [(pitch_deg * 10.0) as i16, x, y, z, roll, claw] {
        a.extend(v.to_le_bytes());
    }
    a.extend(time_ms.to_le_bytes());
    Request::sys(cmd::COORDINATE_SET, a)
}

/// Relative Cartesian jog. Pitch is the fourth field here, ×10.
pub fn arm_move_inc(dx: i16, dy: i16, dz: i16, dpitch_deg: f64, droll: i16, dclaw: i16, time_ms: u16) -> Request {
    let mut a = Vec::with_capacity(14);
    for v in [dx, dy, dz, (dpitch_deg * 10.0) as i16, droll, dclaw] {
        a.extend(v.to_le_bytes());
    }
    a.extend(time_ms.to_le_bytes());
    Request::sys(cmd::ARM_MOVE_INC, a)
}

/// Write acc, goal position and speed to one bus servo (register 41 onwards).
pub fn servo_move(id: u8, pos: u16, acc: u8, speed: u16) -> Request {
    let [pl, ph] = pos.to_le_bytes();
    let [sl, sh] = speed.to_le_bytes();
    Request { id, cmd: servo::WRITE, args: vec![servo::REG_ACC, acc, pl, ph, 0, 0, sl, sh] }
}

pub fn servo_torque(id: u8, on: bool) -> Request {
    Request { id, cmd: servo::WRITE, args: vec![servo::REG_TORQUE, on as u8] }
}

pub fn servo_read_position(id: u8) -> Request {
    Request { id, cmd: servo::READ, args: vec![servo::REG_PRESENT_POS, 2] }
}

pub fn buzzer(on_ms: u32, off_ms: u32, count: u16, freq_hz: u16) -> Request {
    let mut a = Vec::with_capacity(12);
    a.extend(on_ms.to_le_bytes());
    a.extend(off_ms.to_le_bytes());
    a.extend(count.to_le_bytes());
    a.extend(freq_hz.to_le_bytes());
    Request::sys(cmd::BUZZER_SET, a)
}

pub fn set_pos_offset(id: u8, offset: i16) -> Request {
    let mut a = vec![id];
    a.extend(offset.clamp(-2047, 2047).to_le_bytes());
    Request::sys(cmd::SET_POS_OFFSET, a)
}

pub fn set_pid(id: u8, p: u8, i: u8, d: u8, min_force: u16) -> Request {
    let mut a = vec![id, p, i, d];
    a.extend(min_force.to_le_bytes());
    Request::sys(cmd::SET_PID_PARAM, a)
}

pub fn set_max_torque(id: u8, torque: u16) -> Request {
    let mut a = vec![id];
    a.extend(torque.to_le_bytes());
    Request::sys(cmd::SET_MAX_TORQUE, a)
}

pub fn set_coord_limits(values: [i16; 12]) -> Request {
    Request::sys(cmd::SET_COORD_LIMITS, values.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>())
}

pub fn set_chassis(c: &ChassisConfig) -> Request {
    let mut a = vec![c.kind];
    for v in [c.wheel_diameter, c.wheel_base, c.track_width] {
        a.extend(v.to_le_bytes());
    }
    a.extend([0, 0, 0, 0, c.max_speed]);
    Request::sys(cmd::SET_CHASSIS_CONFIG, a)
}

pub fn set_kinematics(params: [f32; 9]) -> Request {
    Request::sys(cmd::SET_KINEMATICS_PARAM, params.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>())
}

pub fn stepper_run(steps: i32) -> Request {
    Request::sys(cmd::STEPPER_RUN, steps.to_le_bytes())
}

/// Microstep divisor → firmware code (1→0, 2→1, 4→2, 8→3, 16/32→7, unknown→0).
pub fn stepper_div(div: u32) -> Request {
    let code = match div {
        1 => 0,
        2 => 1,
        4 => 2,
        8 => 3,
        16 | 32 => 7,
        _ => 0,
    };
    Request::sys(cmd::STEPPER_DIV, [code])
}

pub fn apriltag_offset(x: f32, y: f32, z: f32) -> Request {
    let mut a: Vec<u8> = [x, y, z].iter().flat_map(|v| v.to_le_bytes()).collect();
    a.extend([0, 0]);
    Request::sys(cmd::APRILTAG_SET_OFFSET, a)
}

/// Signed i8 values sent as raw bytes (mecanum/tank/motor speeds).
pub fn i8s(cmd: u8, vals: &[i32]) -> Request {
    Request::sys(cmd, vals.iter().map(|v| (*v).clamp(-128, 127) as i8 as u8).collect::<Vec<_>>())
}

pub fn arm_reset(time_ms: u16) -> Request {
    Request::sys(cmd::ARM_RESET, le16(time_ms))
}

/// One keyframe of an action group.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ServoTarget {
    pub pos: u16,
    pub acc: u8,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ActionFrame {
    pub time: u16,
    pub servos: Vec<ServoTarget>,
}

/// Speed that makes a servo cover `from → to` in `time_ms` (+20% margin), 10..=3400.
pub fn time_based_speed(from: u16, to: u16, time_ms: u16) -> u16 {
    let dist = (to as f64 - from as f64).abs();
    let s = if time_ms > 0 { (dist * 1000.0 / time_ms as f64 * 1.2) as i64 } else { 3400 };
    s.clamp(10, 3400) as u16
}

/// Encode one ACTION_GROUP_DOWNLOAD packet. `current` is updated to the frame's targets.
pub fn action_download(group: u8, total: u8, frame_no: u8, f: &ActionFrame, current: &mut [u16; 6]) -> Request {
    let mut a = vec![group, total, frame_no, 6];
    a.extend(f.time.to_le_bytes());
    for (j, s) in f.servos.iter().take(6).enumerate() {
        let sp = time_based_speed(current[j], s.pos, f.time);
        a.push(j as u8 + 1);
        a.extend(s.pos.to_le_bytes());
        a.push(s.acc);
        a.extend(sp.to_le_bytes());
        current[j] = s.pos;
    }
    Request::sys(cmd::ACTION_GROUP_DOWNLOAD, a)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02X}")).collect::<Vec<_>>().join(" ")
    }

    // Byte examples captured from the original client's encoder.
    #[test]
    fn encodes_like_original_client() {
        assert_eq!(hex(&Request::sys(cmd::GET_CUR_COORDS, []).bytes()), "FF FF FF 02 0B F3");
        assert_eq!(
            hex(&coordinate_set(200, 0, 200, 0.0, 0, 0, 1000).bytes()),
            "FF FF FF 10 08 00 00 C8 00 00 00 C8 00 00 00 00 00 E8 03 6D"
        );
        assert_eq!(
            hex(&buzzer(100, 100, 3, 2000).bytes()),
            "FF FF FF 0E 09 64 00 00 00 64 00 00 00 03 00 D0 07 47"
        );
        assert_eq!(hex(&servo_move(1, 2048, 50, 1000).bytes()), "FF FF 01 0A 03 29 32 00 08 00 00 E8 03 A3");
    }

    const POSE_FRAME: &str = "FF FF FF 1A 0B C8 00 F1 FF B4 00 C7 01 1E 00 0C 00 00 08 D0 07 AE 04 D6 04 00 08 34 08 CE";

    fn unhex(s: &str) -> Vec<u8> {
        s.split(' ').map(|h| u8::from_str_radix(h, 16).unwrap()).collect()
    }

    #[test]
    fn decodes_pose_after_garbage() {
        let mut d = Decoder::default();
        d.push(b"\x00junk");
        d.push(&unhex(POSE_FRAME));
        let Some(Decoded::Frame(f)) = d.next_decoded() else { panic!("no frame") };
        assert_eq!(
            parse(&f),
            Some(Response::Pose(Pose {
                x: 200,
                y: -15,
                z: 180,
                pitch: 45.5,
                roll: 30,
                claw: 12,
                servos: [2048, 2000, 1198, 1238, 2048, 2100]
            }))
        );
        assert!(d.next_decoded().is_none());
    }

    #[test]
    fn rejects_bad_checksum() {
        let mut raw = unhex(POSE_FRAME);
        *raw.last_mut().unwrap() ^= 0xFF;
        let mut d = Decoder::default();
        d.push(&raw);
        assert!(matches!(d.next_decoded(), Some(Decoded::BadChecksum(_))));
    }

    fn frames(d: &mut Decoder) -> Vec<Frame> {
        std::iter::from_fn(|| d.next_decoded())
            .filter_map(|x| match x {
                Decoded::Frame(f) => Some(f),
                Decoded::BadChecksum(_) => None,
            })
            .collect()
    }

    #[test]
    fn stray_ff_before_frame_is_not_fatal() {
        let mut d = Decoder::default();
        d.push(&[0xFF]);
        d.push(&unhex(POSE_FRAME));
        assert_eq!(frames(&mut d).len(), 1);
    }

    #[test]
    fn corrupted_frame_does_not_eat_next_frame() {
        let good = unhex(POSE_FRAME);
        let mut bad = good.clone();
        *bad.last_mut().unwrap() ^= 0x55;
        let mut d = Decoder::default();
        d.push(&bad);
        d.push(&good);
        d.push(&encode(SYS_ID, cmd::CHECK_BAT_LEVEL, &[0x10, 0x27]));
        let got = frames(&mut d);
        assert_eq!(got.len(), 2);
        assert_eq!(parse(&got[1]), Some(Response::Battery(10000)));
    }

    #[test]
    fn accepts_short_checksum_for_leader_replies_only() {
        let mut f = encode(SYS_ID, cmd::READ_BAUD, &[3, 0]);
        let n = f.len();
        f[n - 1] = checksum(&f[2..5]);
        let mut d = Decoder::default();
        d.push(&f);
        let Some(Decoded::Frame(fr)) = d.next_decoded() else { panic!() };
        assert_eq!(parse(&fr), Some(Response::ServoBaud { id: 3, code: 0 }));

        let mut g = encode(SYS_ID, cmd::GET_CUR_COORDS, &[1, 2]);
        let n = g.len();
        g[n - 1] = checksum(&g[2..5]);
        d.push(&g);
        assert!(matches!(d.next_decoded(), Some(Decoded::BadChecksum(_))));
    }

    #[test]
    fn offset_decoding_matches_both_encodings() {
        assert_eq!(decode_offset((-100i16) as u16), -100);
        assert_eq!(decode_offset(0x0800 | 100), -100);
        assert_eq!(decode_offset(2047), 2047);
    }

    #[test]
    fn text_filter() {
        assert_eq!(printable_text(b"hello\r\n").as_deref(), Some("hello\r\n"));
        assert_eq!(printable_text(b"ZZ \r\n"), None);
        assert_eq!(printable_text(b"\xFF\xFF"), None);
    }

    #[test]
    fn speed_clamps() {
        assert_eq!(time_based_speed(2048, 2048, 1000), 10);
        assert_eq!(time_based_speed(0, 4000, 100), 3400);
        assert_eq!(time_based_speed(1000, 2000, 1000), 1200);
    }
}
