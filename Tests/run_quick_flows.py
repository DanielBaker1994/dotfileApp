#!/usr/bin/env python3
"""Replay Tests/quick_flows.json against the quick agent with real keys (in the VM).

Runs INSIDE the VM (`bin/ks-vm quick [--ids a,b]` copies the tree in and calls this),
never on the owner's screen. Each flow gets a fresh agent over a blank TextEdit
document (the "app underneath"), real key presses through ks-input (sent to
whatever is frontmost, as a keyboard would), and oracles on the quick socket's
`state live`, on disk, and on what TextEdit saved. A full-screen PNG is kept per
check under --out.

Step kinds:   {"press": "hyper+s"}  {"type": "text"} (real key codes)  {"key": "esc"} / {"key": ["j", "j"]}
              {"wait": 0.4}  {"shell": "cmd"}  {"check": [oracle, ...], "name": "..."}
Oracles:      {"state": "live.palette.query", "equals"|"not_equals"|"contains"|"min": V}
              {"disk": "~/qf/x.txt", "exists": true|false}
              {"textedit": "contains"|"absent", "text": "qqq"}   (end of flow only:
              it hides the agent, saves TextEdit and reads the file)
"""
import argparse, json, os, shutil, socket, subprocess, sys, time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
APP = os.path.join(ROOT, "kitchen-sink.app")
BIN = os.path.join(APP, "Contents/MacOS/kitchen-sink")
KI = os.path.join(ROOT, ".build/ks-input/ks-input")
QS = os.path.join(os.environ.get("TMPDIR", "/tmp"), "ks-quick.sock")
HOME = os.path.expanduser("~")
FIX = os.path.join(HOME, "qf")
TE_FILE = "/tmp/qf-te.txt"
MODS = {"hyper": ["cmd", "ctrl", "alt", "shift"]}


def sh(*a, check=False, **kw):
    return subprocess.run(list(a), capture_output=True, text=True, check=check, **kw)


def state():
    try:
        s = socket.socket(socket.AF_UNIX)
        s.settimeout(3)
        s.connect(QS)
        s.sendall(b"state live\n")
        out = b""
        while True:
            b = s.recv(65536)
            if not b:
                break
            out += b
        return json.loads(out or b"{}")
    except Exception:
        return {}


def resolve(obj, path):
    for part in path.split("."):
        if isinstance(obj, dict):
            obj = obj.get(part)
        elif isinstance(obj, list) and part.isdigit() and int(part) < len(obj):
            obj = obj[int(part)]
        else:
            return None
    return obj


def front_pid():
    out = sh(KI, "front").stdout.split("\t")
    return out[0] if out and out[0].isdigit() else None


def combo(spec):
    parts = spec.split("+")
    out = []
    for p in parts[:-1]:
        out += MODS.get(p, [p])
    return "+".join(out + [parts[-1]])


SHIFTED = {"~": "`", "_": "-", "+": "=", "!": "1", "@": "2", "#": "3", "$": "4", "%": "5",
           "^": "6", "&": "7", "*": "8", "(": "9", ")": "0", ":": ";", '"': "'", "<": ",",
           ">": ".", "?": "/", "{": "[", "}": "]", "|": "\\"}


def keys(names):
    pid = front_pid()
    # Everything after `--` is a combo, so a bare "-" key works too.
    r = sh(KI, "key", "--pid", pid, "--", *[combo(n) for n in names])
    if r.returncode:
        raise RuntimeError(f"ks-input key {names}: {r.stderr.strip()}")


def type_text(text):
    """Type with real key codes, like a keyboard. (`ks-input type` posts Unicode
    events with key code 0, which the agent's key routing reads as 'a'.)"""
    combos = []
    for ch in text:
        if ch == " ":
            combos.append("space")
        elif ch in SHIFTED:
            combos.append("shift+" + SHIFTED[ch])
        elif ch.isupper():
            combos.append("shift+" + ch.lower())
        else:
            combos.append(ch)
    keys(combos)


def textedit_pid():
    r = sh("pgrep", "-x", "TextEdit")
    return r.stdout.split()[0] if r.stdout.strip() else None


def reset(flow):
    sh(BIN, "quick-quit")
    for _ in range(50):
        if sh("pgrep", "-f", "MacOS/kitchen-sink quick").returncode:
            break
        time.sleep(0.1)
    sh("pkill", "-f", "MacOS/kitchen-sink quick")
    sh("pkill", "-x", "TextEdit")
    shutil.rmtree(FIX, ignore_errors=True)
    os.makedirs(FIX)
    for name in flow.get("files", []):
        with open(os.path.join(FIX, name), "w") as f:
            f.write(name + "\n")
    open(TE_FILE, "w").close()
    time.sleep(0.5)
    sh("open", "-a", "TextEdit", TE_FILE)
    for _ in range(50):
        if textedit_pid():
            break
        time.sleep(0.1)
    time.sleep(1.5)
    sh("open", "-n", "-g", APP, "--args", "quick")
    for _ in range(100):
        if state().get("pid"):
            break
        time.sleep(0.1)
    time.sleep(flow.get("warm", 1.5))
    r = sh(KI, "activate", "--pid", textedit_pid())
    if r.returncode:  # a just-booted VM can be slow to hand over the first time
        time.sleep(1)
        r = sh(KI, "activate", "--pid", textedit_pid())
    if r.returncode:
        raise RuntimeError("cannot activate TextEdit: " + r.stderr.strip())
    # Let the activation finish (window ordering lags `activate`): a palette
    # shown before it lands loses key to TextEdit and closes.
    time.sleep(1.0)


def textedit_text():
    sh(BIN, "quick-quit")  # nothing of ours may hold the keyboard
    time.sleep(0.8)
    pid = textedit_pid()
    sh(KI, "activate", "--pid", pid)
    time.sleep(0.3)
    sh(KI, "key", "--pid", pid, "cmd+s")
    time.sleep(1.0)
    with open(TE_FILE) as f:
        return f.read()


def check(o, st):
    if "state" in o:
        v = resolve(st, o["state"])
        for op in ("equals", "not_equals", "contains", "min"):
            if op in o:
                want = o[op]
                ok = {"equals": v == want, "not_equals": v != want,
                      "contains": isinstance(v, (str, list)) and want in v,
                      "min": isinstance(v, (int, float)) and v >= want}[op]
                return ok, f"{o['state']} = {json.dumps(v)} ({op} {json.dumps(want)})"
    if "disk" in o:
        p = os.path.expanduser(o["disk"])
        ok = os.path.exists(p) == o.get("exists", True)
        return ok, f"{o['disk']} exists={os.path.exists(p)}"
    if "textedit" in o:
        txt = textedit_text()
        has = o["text"] in txt
        ok = has if o["textedit"] == "contains" else not has
        return ok, f"TextEdit holds {txt!r} ({o['textedit']} {o['text']!r})"
    return False, f"unknown oracle {o}"


def run_flow(flow, out):
    results, n = [], 0
    reset(flow)
    for step in flow["steps"]:
        if "press" in step:
            keys([step["press"]])
        elif "type" in step:
            type_text(step["type"])
        elif "key" in step:
            keys(step["key"] if isinstance(step["key"], list) else [step["key"]])
        elif "wait" in step:
            time.sleep(step["wait"])
        elif "shell" in step:
            subprocess.run(step["shell"].replace("$FIX", FIX), shell=True)
        elif "check" in step:
            n += 1
            time.sleep(step.get("settle", 0.4))
            png = os.path.join(out, f"{flow['id']}-{n}.png")
            sh("screencapture", "-x", png)
            st = state()
            with open(os.path.join(out, f"{flow['id']}-{n}.json"), "w") as f:
                json.dump(st, f, indent=1)
            for o in step["check"]:
                ok, detail = check(o, st)
                results.append((ok, step.get("name", f"check {n}"), detail))
    return results


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--table", default=os.path.join(ROOT, "Tests/quick_flows.json"))
    ap.add_argument("--ids")
    ap.add_argument("--out", default=os.path.join(ROOT, ".build/quick-runs", time.strftime("%Y%m%d-%H%M%S")))
    a = ap.parse_args()
    flows = json.load(open(a.table))["flows"]
    if a.ids:
        want = a.ids.split(",")
        flows = [f for f in flows if f["id"] in want]
    os.makedirs(a.out, exist_ok=True)
    failed = 0
    for flow in flows:
        try:
            res = run_flow(flow, a.out)
        except Exception as e:  # a step that could not run is an error, not a pass
            res = [(False, "error", str(e))]
        bad = [r for r in res if not r[0]]
        failed += bool(bad)
        print(f"{'PASS' if not bad else 'FAIL'} {flow['id']}  ({flow.get('from', '')})")
        for ok, name, detail in res:
            print(f"   {'ok ' if ok else 'BAD'} {name}: {detail}")
    sh(BIN, "quick-quit")
    sh("pkill", "-x", "TextEdit")
    shutil.rmtree(FIX, ignore_errors=True)
    print(f"out: {a.out}")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
