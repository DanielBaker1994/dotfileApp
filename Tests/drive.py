#!/usr/bin/env python3
"""Drive the GUI sandbox app by hand, one command per call (runs INSIDE the test VM).

Agents call it through the host wrapper `bin/ks-drive`, which runs it over `bin/ks-vm ssh`.
It reuses run_flows.py's Sandbox, so the app, fixtures (Tests/app_flows.json), activation
and real-input path are exactly what the flow runner uses.

    drive.py start                 fresh fixtures + launch the app (+ open notes)
    drive.py state [dotted.path]   the state JSON, or one value from it
    drive.py do VERB               a do: verb (e.g. open:files), prints the reply
    drive.py key COMBO             real key event(s), e.g. cmd+n | 'ctrl+b w' | esc
    drive.py text TEXT             literal text into whatever has focus
    drive.py click X Y [2|right]   click at points from the window's top-left
    drive.py shot NAME [VIEW]      PNG of the shared window -> /tmp/ks-drive/NAME.png
    drive.py log [N]               last N lines of the daemon log (default 40)
    drive.py stop
"""
from __future__ import annotations

import json
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import run_flows as rf  # noqa: E402

APP = os.path.join(rf.ROOT, ".build", "vm-app", "kitchen-sink.app")
SHOTS = "/tmp/ks-drive"


def sandbox() -> rf.Sandbox:
    return rf.Sandbox(APP)


def ensure_front(sb: rf.Sandbox) -> None:
    st = sb.state(quiet=True) or {}
    if not (st.get("active") and st.get("keyWindow")):
        sb.activate()


def settle_report(sb: rf.Sandbox) -> None:
    time.sleep(0.5)
    st = sb.state(quiet=True) or {}
    inp = st.get("input") or {}
    print(json.dumps({"view": st.get("view"), "keyWindow": st.get("keyWindow"),
                      "focused": (st.get("pane") or {}).get("focused"),
                      "toast": st.get("toast"), "dropped": inp.get("dropped"),
                      "lastError": inp.get("lastError")}, ensure_ascii=False))


def main(argv: list[str]) -> int:
    if not argv:
        print(__doc__)
        return 2
    cmd, args = argv[0], argv[1:]
    sb = sandbox()
    if cmd == "start":
        sb.stop()
        sb.reset()
        rf.make_fixtures(json.load(open(rf.TABLE)), sb.real_home)
        sb.start()
        print(f"started; sandbox HOME is {sb.real_home} (fixtures from Tests/app_flows.json)")
        return 0
    if cmd == "stop":
        sb.stop()
        print("stopped")
        return 0
    if sb.state(quiet=True) is None:
        print("error: the app is not answering (run: start)")
        return 1
    if cmd == "state":
        st = sb.state() or {}
        print(json.dumps(rf.resolve(st, args[0]) if args else st, ensure_ascii=False, indent=1))
    elif cmd == "do":
        # A do: reply is the whole state (~5 KB): print only an error or a short summary.
        raw = sb.send("do:" + " ".join(args).removeprefix("do:"), timeout=20)
        try:
            v = json.loads(raw)
        except json.JSONDecodeError:
            v = {"raw": raw[:300]}
        if isinstance(v, dict) and ("error" in v or "raw" in v):
            print(json.dumps(v, ensure_ascii=False)[:400])
        else:
            settle_report(sb)
    elif cmd == "key":
        ensure_front(sb)
        sb.send("do:input:key:" + " ".join(args))
        settle_report(sb)
    elif cmd == "text":
        ensure_front(sb)
        sb.send("do:input:text:" + " ".join(args))
        settle_report(sb)
    elif cmd == "click":
        ensure_front(sb)
        extra = f",{args[2]}" if len(args) > 2 else ""
        sb.send(f"do:input:click:{args[0]},{args[1]}{extra}")
        settle_report(sb)
    elif cmd == "shot":
        os.makedirs(SHOTS, exist_ok=True)
        out = os.path.join(SHOTS, args[0].removesuffix(".png") + ".png")
        words = ["window-shot", "--out", out, "--scale", "1"] + (["--view", args[1]] if len(args) > 1 else [])
        print(sb.send("\t".join(words), timeout=25))
    elif cmd == "log":
        n = int(args[0]) if args else 40
        lines = []
        for p in sb.logs():
            if os.path.exists(p):
                lines += open(p, errors="replace").read().splitlines()
        print("\n".join(lines[-n:]))
    else:
        print(f"unknown command {cmd!r}")
        print(__doc__)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
