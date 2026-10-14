"""Generate tests/golden_bytes.json from the ORIGINAL NexArm client.

Each case evaluates a payload expression copied verbatim from an original send
call site (../recovered/NexArm/nexarm_qt/ui/*.py) with fixed inputs, sends it
through the original CommManager, and records the exact frame bytes.

Run: <python with PyQt5> -I tools/gen_golden_bytes.py
"""
import json, os, struct, sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.normpath(os.path.join(HERE, "..", "..", "recovered", "NexArm"))
sys.path.insert(0, ROOT)

from PyQt5.QtCore import QCoreApplication  # noqa: E402

app = QCoreApplication([])
from nexarm_qt.comm_manager import CommManager  # noqa: E402
from nexarm_qt.constants import *  # noqa: E402,F401,F403


class FakeSer:
    def __init__(self):
        self.out = []

    def write(self, b):
        self.out.append(bytes(b))


cm = CommManager()
cm.ser = FakeSer()
cm.is_connected = True
cm.connection_type = "serial"
cases = []


def case(name, fn):
    """fn() performs exactly one original send call."""
    before = len(cm.ser.out)
    fn()
    assert len(cm.ser.out) == before + 1, name
    cases.append({"name": name, "hex": cm.ser.out[-1].hex(" ").upper()})


sys_ = cm.send_sys
pkt = cm.send_packet

# --- coord_tab / arm_3d_widget: send_coordinate_move, _send_coordinate
for tag, (pitch, x, y, z, r, c, t) in {
    "home": (0.0, 200, 0, 200, 0, 0, 1000),
    "neg": (-12.5, -150, 75, 30, -90, 45, 2500),
    "frac": (45.56, 1, -1, 0, 0, 0, 0),
    "negfrac": (-0.37, 0, 0, 0, 0, 0, 65535),
}.items():
    p = int(pitch * 10)
    args = list(struct.pack("<hhhhhhH", p, x, y, z, r, c, t))
    case(f"coordinate_set_{tag}", lambda a=args: sys_(CMD_COORDINATE_SET, a))

# --- coord_tab.adjust_coord (step is an int QSpinBox, direction ±1)
for axis, direction, step in [("X", 1, 10), ("Y", -1, 25), ("Z", 1, 1), ("Pitch", -1, 10),
                              ("Roll", 1, 100), ("Claw", -1, 5)]:
    dx = dy = dz = dp = dr = dc = 0
    if axis == "X":
        dx = direction * step
    elif axis == "Y":
        dy = direction * step
    elif axis == "Z":
        dz = direction * step
    elif axis == "Pitch":
        dp = direction * step
    elif axis == "Roll":
        dr = direction * step
    elif axis == "Claw":
        dc = direction * step
    dur_ms = 200
    dp_val = int(dp * 10)
    args = list(struct.pack("<hhhhhhH", int(dx), int(dy), int(dz), dp_val, int(dr), int(dc), dur_ms))
    case(f"move_inc_{axis.lower()}", lambda a=args: sys_(CMD_ARM_MOVE_INC, a))

case("arm_reset", lambda: sys_(CMD_ARM_RESET, [232, 3]))
case("get_cur_coords", lambda: sys_(CMD_GET_CUR_COORDS))
case("read_all_servos", lambda: sys_(CMD_READ_ALL_SERVOS))

# --- system_tab
for tag, (on_t, off_t, cnt, freq) in {"basic": (100, 100, 3, 2000), "big": (70000, 1, 65535, 4000)}.items():
    args = list(struct.pack("<IIHH", on_t, off_t, cnt, freq))
    case(f"buzzer_{tag}", lambda a=args: sys_(CMD_BUZZER_SET, a))
case("firmware_version", lambda: sys_(CMD_FIRMWARE_VERSION_CHECK))
case("battery", lambda: sys_(CMD_CHECK_BAT_LEVEL_CHECK))
case("bt_mode_ps3", lambda: sys_(CMD_SET_BT_MODE, [1]))
case("bt_mode_ble", lambda: sys_(CMD_SET_BT_MODE, [0]))
case("factory_reset", lambda: sys_(CMD_FACTORY_RESET))
case("move_acc", lambda: sys_(CMD_SET_MOVE_ACC, [200]))
case("get_coord_limits", lambda: sys_(CMD_GET_COORD_LIMITS))
vals = [-300, 300, -300, 300, -50, 400, -90, 90, -180, 180, 0, 100]
vals[6] = vals[6] * 10
vals[7] = vals[7] * 10
case("set_coord_limits", lambda: sys_(CMD_SET_COORD_LIMITS, list(struct.pack("<hhhhhhhhhhhh", *vals))))
case("get_chassis", lambda: sys_(CMD_GET_CHASSIS_CONFIG))
chassis_type, wheel_dia, wheel_base, track_width, max_speed = 1, 65.0, 150.5, 120.25, 80
args = [chassis_type]
args += list(struct.pack("<f", wheel_dia))
args += list(struct.pack("<f", wheel_base))
args += list(struct.pack("<f", track_width))
args += [0, 0, 0, 0]
args += [max_speed & 255]
case("set_chassis", lambda: sys_(CMD_SET_CHASSIS_CONFIG, args))
case("get_kinematics", lambda: sys_(CMD_GET_KINEMATICS_PARAM))
kin = [105.0, 98.5, 12.25, 160.0, -3.5, 75.0, 20.0, 33.5, 88.0]
kargs = []
for val in kin:
    kargs += list(struct.pack("<f", val))
case("set_kinematics", lambda: sys_(CMD_SET_KINEMATICS_PARAM, kargs))

# --- servo_tab
for tag, (id_val, p, a, s) in {"mid": (1, 2048, 50, 1000), "edge": (6, 4095, 255, 3400)}.items():
    args = [SERVO_REG_ACC, a & 255, p & 255, p >> 8 & 255, 0, 0, s & 255, s >> 8 & 255]
    case(f"servo_move_{tag}", lambda i=id_val, a=args: pkt(i, SERVO_CMD_WRITE, a))
case("servo_torque_on", lambda: pkt(3, SERVO_CMD_WRITE, [SERVO_REG_TORQUE, 1 if True else 0]))
case("servo_torque_off", lambda: pkt(4, SERVO_CMD_WRITE, [SERVO_REG_TORQUE, 1 if False else 0]))
case("servo_read_position", lambda: pkt(2, SERVO_CMD_READ, [SERVO_REG_PRESENT_POS, 2]))
case("set_torque_all_off", lambda: sys_(CMD_SET_TORQUE, [0]))
case("set_torque_all_on", lambda: sys_(CMD_SET_TORQUE, [1]))
case("get_real_joint_angles", lambda: sys_(CMD_GET_REAL_JOINT_ANGLES))
case("action_group_run", lambda: sys_(CMD_ACTION_GROUP_RUN, [7]))
case("action_group_erase", lambda: sys_(CMD_ACTION_GROUP_ERASE, [7]))
case("action_group_stop", lambda: sys_(CMD_ACTION_GROUP_STOP))


def _calc_time_based_speed(start_pos, target_pos, move_time_ms):
    distance = abs(int(target_pos) - int(start_pos))
    if move_time_ms > 0:
        calc_speed = int(distance * 1000.0 / float(move_time_ms) * 1.2)
    else:
        calc_speed = 3400
    if calc_speed < 10:
        calc_speed = 10
    if calc_speed > 3400:
        calc_speed = 3400
    return calc_speed


action_data_list = [
    {"time": 1000, "servos": [{"pos": 2048, "acc": 0}, {"pos": 1500, "acc": 10}, {"pos": 2600, "acc": 0},
                              {"pos": 1198, "acc": 5}, {"pos": 3000, "acc": 0}, {"pos": 2048, "acc": 255}]},
    {"time": 300, "servos": [{"pos": 100, "acc": 0}, {"pos": 4000, "acc": 0}, {"pos": 2600, "acc": 1},
                             {"pos": 1238, "acc": 0}, {"pos": 2048, "acc": 0}, {"pos": 0, "acc": 0}]},
]
gid, tot = 3, len(action_data_list)
current_pos = [2048, 2048, 1198, 1238, 2048, 2048]
for (i, f) in enumerate(action_data_list):
    t = max(0, int(f.get("time", 1000)))
    args = [gid, tot, i + 1, 6, t & 255, t >> 8 & 255]
    for (j, s) in enumerate(f["servos"]):
        p = s.get("pos", 2048)
        a = s.get("acc", 0)
        sp = _calc_time_based_speed(current_pos[j], p, t)
        args.extend([j + 1, p & 255, p >> 8 & 255, a & 255, sp & 255, sp >> 8 & 255])
        current_pos[j] = p
    case(f"action_download_{i + 1}", lambda a=args: sys_(CMD_ACTION_GROUP_DOWNLOAD, a))

# --- servo_advanced_tab
case("set_servo_id", lambda: sys_(CMD_SET_SERVO_ID, [1, 5]))
case("set_servo_mode", lambda: sys_(CMD_SET_SERVO_MODE, [2, 3]))
for tag, (sid, offset) in {"pos": (1, 300), "neg": (6, -2047), "clamp": (2, -5000)}.items():
    offset = max(-2047, min(2047, offset))
    case(f"set_pos_offset_{tag}", lambda s=sid, o=offset: sys_(CMD_SET_POS_OFFSET, list(struct.pack("<Bh", s, o))))
case("get_pos_offset", lambda: sys_(CMD_GET_POS_OFFSET, [4]))
case("cali_pos", lambda: sys_(CMD_SERVO_CALI_POS, [3]))
case("set_pid", lambda: sys_(CMD_SET_PID_PARAM, list(struct.pack("<BBBBH", 2, 32, 0, 16, 1000))))
case("get_pid", lambda: sys_(CMD_GET_PID_PARAM, [2]))
case("read_overload", lambda: sys_(CMD_READ_OVERLOAD, [5]))
case("set_overload", lambda: sys_(CMD_SET_OVERLOAD, [5, 80, 200, 25]))
case("read_baud", lambda: sys_(CMD_READ_BAUD, [1]))
case("set_baud", lambda: sys_(CMD_SET_BAUD, [1, int("0: 1000000".split(":")[0])]))
case("set_baud_all", lambda: sys_(CMD_SET_BAUD, [254, int("3: 128000".split(":")[0])]))
case("read_max_torque", lambda: sys_(CMD_READ_MAX_TORQUE, [6]))
val = 1000
case("set_max_torque", lambda: sys_(CMD_SET_MAX_TORQUE, [6, val & 255, val >> 8 & 255]))

# --- peripheral_tab
for tag, v in {"fwd": 50, "rev": -100}.items():
    case(f"conveyor_{tag}", lambda v=v: sys_(CMD_CONVEYOR_SET, [v & 255]))
case("conveyor_stop", lambda: sys_(CMD_CONVEYOR_SET, [0]))
case("stepper_reset", lambda: sys_(CMD_STEPPER_RESET))
for tag, steps in {"pos": 3200, "neg": -123456}.items():
    case(f"stepper_run_{tag}", lambda s=steps: sys_(CMD_STEPPER_RUN, list(struct.pack("<i", s))))
div_map = {1: 0, 2: 1, 4: 2, 8: 3, 16: 7, 32: 7}
for div in [1, 2, 4, 8, 16, 32]:
    code = div_map.get(div, 0)
    case(f"stepper_div_{div}", lambda c=code: sys_(CMD_STEPPER_SET_DIV, [c]))
case("espnow_on", lambda: sys_(CMD_ESPNOW_ENABLE, [1]))
case("espnow_off", lambda: sys_(CMD_ESPNOW_ENABLE, [0]))
case("espnow_scan", lambda: sys_(CMD_ESPNOW_SCAN))
case("espnow_channel", lambda: sys_(CMD_ESPNOW_SET_CHANNEL, [11]))
case("espnow_acc", lambda: sys_(CMD_ESPNOW_SET_ACC, [30]))
mac_str = "24:6F:28:aa:0B:ff"
case("peer_mac", lambda: sys_(CMD_ESPNOW_SET_MAC, [int(x, 16) for x in mac_str.split(":")]))
case("ps3_mac", lambda: sys_(CMD_PS3_SET_MAC, [int(x, 16) for x in "00:1a:7D:DA:71:13".split(":")]))
mapping = {'up': (0, -50, 0), 'down': (0, 50, 0), 'left': (0, 0, 50), 'right': (0, 0, -50),
           'left_strafe': (-50, 0, 0), 'right_strafe': (50, 0, 0)}
for tag, active in {"up_left": ["up", "left"], "strafe_l_r": ["left_strafe", "right_strafe"],
                    "all_neg": ["up", "right", "left_strafe"]}.items():
    vx, vy, vz = (0, 0, 0)
    for name in active:
        dx, dy, dz = mapping[name]
        vx += dx
        vy += dy
        vz += dz
    vx = max(-100, min(100, vx))
    vy = max(-100, min(100, vy))
    vz = max(-100, min(100, vz))
    case(f"mecanum_{tag}", lambda a=[vx & 255, vy & 255, vz & 255]: sys_(CMD_MECANUM_RUN, a))
tank = {'up': (50, 0), 'down': (-50, 0), 'left': (0, 50), 'right': (0, -50)}
for name in ["down", "right"]:
    s, t = tank[name]
    case(f"tank_{name}", lambda s=s, t=t: sys_(CMD_TANK_RUN, [s & 255, t & 255]))
case("motor_stop", lambda: sys_(CMD_MOTOR_STOP))
case("single_motor", lambda: sys_(CMD_SET_SINGLE_MOTOR, [2 + 1, -75 & 255]))
case("multi_motors", lambda: sys_(CMD_SET_MOTOR_SPEED, [sp & 255 for sp in [100, -100, 0, 37]]))

# --- ai_tab / ai_calibration_widget
case("color_track_red", lambda: pkt(255, 40, [1, 1]))
case("color_track_stop", lambda: pkt(255, 40, [0, 0]))
for nm, c in [("face", 41), ("apriltag_track", 43), ("apriltag_grab", 44), ("garbage", 48), ("llm", 47),
              ("gesture", 81)]:
    case(f"{nm}_start", lambda c=c: pkt(255, c, [1]))
    case(f"{nm}_stop", lambda c=c: pkt(255, c, [0]))
case("color_grab_blue", lambda: pkt(255, 46, [1, 3]))
case("color_grab_stop", lambda: pkt(255, 46, [0]))
for sub in [1, 2, 3, 0]:
    case(f"calibration_{sub}", lambda s=sub: pkt(255, 49, [s]))
x, y, z = 12.5, -3.25, 100.0
data_list = list(struct.pack("<fff", x, y, z))
case("apriltag_offset", lambda: pkt(255, 45, data_list + [0, 0]))

# --- teach_tab
for nm in ["ACTION_EDIT_ENTER", "ACTION_EDIT_EXIT", "ACTION_EDIT_START", "ACTION_EDIT_STOP", "ACTION_EDIT_PLAY",
           "ACTION_EDIT_PLAY_STOP", "ACTION_EDIT_CLEAR", "ACTION_EDIT_QUERY", "SYNC_TEACH_ENTER", "SYNC_TEACH_EXIT",
           "SYNC_TEACH_REC_START", "SYNC_TEACH_REC_STOP", "SYNC_TEACH_PLAY", "SYNC_TEACH_PLAY_STOP",
           "SYNC_TEACH_CLEAR", "SYNC_TEACH_QUERY"]:
    case(nm.lower(), lambda c=globals()["CMD_" + nm]: sys_(c))

out = os.path.join(HERE, "..", "tests", "golden_bytes.json")
with open(out, "w", encoding="utf-8") as f:
    json.dump(cases, f, indent=1)
print(f"{len(cases)} cases -> {os.path.normpath(out)}")
