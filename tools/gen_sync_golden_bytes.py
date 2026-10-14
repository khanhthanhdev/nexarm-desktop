"""Generate tests/sync_golden_bytes.json from the ORIGINAL sync_host_gui tool.

Each case runs one SyncHostApp task (../recovered/sync_host_gui/sync_host_gui.py)
against a fake serial port and records every frame it writes. A case may script
replies: whenever the tool writes a frame whose bytes match, the reply bytes are fed
through the tool's own parser, exactly as if the board had answered.

Run: py -I tools/gen_sync_golden_bytes.py   (needs only the standard library)
"""
import json, os, sys, types

HERE = os.path.dirname(os.path.abspath(__file__))
SRC = os.path.normpath(os.path.join(HERE, "..", "..", "recovered", "sync_host_gui"))
OUT = os.path.join(HERE, "..", "tests", "sync_golden_bytes.json")

# The tool imports pyserial at module level; only its framing code is exercised here.
serial = types.ModuleType("serial")
serial.tools = types.ModuleType("serial.tools")
serial.tools.list_ports = types.ModuleType("serial.tools.list_ports")
sys.modules.update({"serial": serial, "serial.tools": serial.tools, "serial.tools.list_ports": serial.tools.list_ports})
sys.path.insert(0, SRC)
import sync_host_gui as g  # noqa: E402

g.time.sleep = lambda s: None


def hexs(b):
    return " ".join(f"{x:02X}" for x in b)


def frame(target, cmd, args, short_chk=False):
    """Encode a reply; `short_chk` sums only ID, LEN, CMD, as the leader firmware does."""
    body = [target, 2 + len(args), cmd] + list(args)
    return bytes([0xFF, 0xFF] + body + [g.checksum_inv8(body[:3] if short_chk else body)])


class Var:
    def __init__(self, v):
        self.v = v

    def get(self):
        return self.v

    def set(self, v):
        self.v = v


class FakeSer:
    is_open = True

    def __init__(self, client, replies):
        self.client, self.replies, self.out = client, replies, []

    def write(self, b):
        b = bytes(b)
        self.out.append(hexs(b))
        for rx in self.replies.get(hexs(b), []):
            self.client._buffer.extend(rx)
            self.client._parse_buffer()


def run(task, replies=None, center=2048, sid=1, offsets=None):
    replies = {hexs(k): v for k, v in (replies or {}).items()}
    client = g.SerialClient()
    client._ser = FakeSer(client, replies)
    # Replies are queued before the wait starts; a short timeout keeps silent waits fast.
    for name in ("wait_response", "wait_any_response"):
        orig = getattr(g.SerialClient, name)
        setattr(client, name, types.MethodType(
            lambda self, *a, _o=orig, timeout=0.6, predicate=None: _o(self, *a, timeout=0.05, predicate=predicate),
            client))
    app = types.SimpleNamespace(
        client=client,
        var_center_pos=Var(center),
        var_sid=Var(sid),
        servo_rows={i: {"offset": Var((offsets or [0] * 6)[i - 1])} for i in g.SERVO_IDS},
        ui_queue=types.SimpleNamespace(put=lambda item: None),
        log=lambda text: None,
        run_task=lambda name, fn: fn(),
        _hex_list=g.SyncHostApp._hex_list,
    )
    for name, fn in vars(g.SyncHostApp).items():
        if callable(fn) and not name.startswith("__") and name not in ("_hex_list", "log", "run_task"):
            setattr(app, name, types.MethodType(fn, app))
    task(app)
    return {"replies": [[k, [hexs(r) for r in v]] for k, v in replies.items()], "tx": client._ser.out}


SYS, BC = g.TARGET_ID, g.BROADCAST_ID
positions = b"".join(int.to_bytes(p, 2, "little", signed=True) for p in [2048, 1000, -5, 4095, 0, 3000])
cases = {
    "read_positions": run(lambda a: a.read_all_positions_task(),
                          {frame(SYS, 0x60, []): [frame(SYS, 0x60, positions)]}),
    "read_positions_short_chk": run(lambda a: a.read_all_positions_task(),
                                    {frame(SYS, 0x60, []): [frame(SYS, 0x60, positions, short_chk=True)]}),
    "torque_on_short_chk": run(lambda a: a.set_torque(True),
                               {frame(SYS, 0x62, [1]): [frame(SYS, 0x62, [1], short_chk=True)]}),
    "read_positions_silent": run(lambda a: a.read_all_positions_task()),
    "recover_main_ok": run(lambda a: a.recover_all_baud_1m(),
                           {frame(SYS, 74, [BC, 0]): [frame(SYS, 74, [BC, 0, 0b101])]}),
    "recover_sys_ok": run(lambda a: a.recover_all_baud_1m(),
                          {frame(SYS, 74, [BC, 0]): [frame(SYS, 74, [BC, 0, 0])],
                           frame(SYS, 0x63, []): [frame(SYS, 0x63, [0, 0x3F])]}),
    "recover_silent": run(lambda a: a.recover_all_baud_1m(),
                          {frame(SYS, 73, [2]): [frame(SYS, 73, [2, 0])],
                           frame(4, 2, [6, 1]): [frame(4, 0, [3])]}),
    "center_all": run(lambda a: a.move_all_to_center(), center=2048),
    "center_all_clamped": run(lambda a: a.move_all_to_center(), center=5000),
    "center_one": run(lambda a: a.move_one_to_center(), center=1000, sid=3),
    "calibrate_all": run(lambda a: a.calibrate_all_center()),
    "calibrate_one": run(lambda a: a.calibrate_one_center(), sid=2),
    "read_offset_reply": run(lambda a: a.read_one_offset(),
                             {frame(5, 2, [31, 2]): [frame(SYS, 11, [0] * 24), frame(5, 0, [0x64, 0x08])]}, sid=5),
    "read_all_offsets": run(lambda a: a.read_all_offsets()),
    "write_all_offsets": run(lambda a: a.write_all_offsets(), offsets=[-2047, -1, 0, 1, 2047, -3000]),
    "zero_all_offsets": run(lambda a: a.zero_all_offsets()),
    "zero_offset_row": run(lambda a: a.zero_offset(6), offsets=[0, 0, 0, 0, 0, 77]),
    "torque_on": run(lambda a: a.set_torque(True)),
    "torque_off_reply": run(lambda a: a.set_torque(False),
                            {frame(SYS, 0x62, [0]): [frame(SYS, 0x62, [0])]}),
}
for v in [-2047, -1, 0, 1, 2047, -3000, 3000]:
    cases[f"write_offset_{v}"] = run(lambda a: a.write_one_offset(), sid=4, offsets=[0, 0, 0, v, 0, 0])

with open(OUT, "w", encoding="utf-8", newline="\n") as f:
    json.dump([{"name": k, **v} for k, v in cases.items()], f, indent=1)
    f.write("\n")
print(f"{len(cases)} cases -> {os.path.relpath(OUT)}")
