#!/usr/bin/env python3
"""ui-test-focus.py — show / hide / focus / follow-the-workspace, measured.

Drives the REAL paths against the running daemon: the hotkey binary exactly
as AeroSpace runs it (`workspace-switcher window`), `aerospace workspace` /
`aerospace focus`, a real Esc keypress (System Events). State comes from the
daemon socket (`state` / `do:ACTION`, polled every ~5 ms — no sleeps, no
AppleScript), window placement from AeroSpace itself.

Checks:
  single    one daemon; a second launch hands over and exits
  show      hotkey -> visible + key: latency; AeroSpace floats it, on the
            focused workspace; the frame holds still for 1 s (no shudder)
  hide      hotkey while in it -> hidden: latency
  follow    hidden on A, hotkey on B -> shows on B, same frame, still
  stranded  left up on A, hotkey on B -> shows on B with no slide / resize
  swap      a view last hidden on A, switched to on B -> stays on B (no jump)
  focus     another window focused, `aerospace focus` ours -> key: latency
            (the "slow focus while it's open" path)
  esc       Esc with the view's "Esc Hides Window" off = stays; on = hides
  tools     the "/" tool panels (prettyprint, health-checks, filefast,
            paths) act like their own apps: opening, re-running and
            clicking one never activates the app (the shared window never
            comes along), AeroSpace never lists it, and the hotkey from one
            focuses the shared window instead of hiding it

It moves your workspaces / focus while it runs (~15 s) and puts them back
(workspace, focused window, view, commands.toml byte for byte).

  bin/ui-test-focus.py            all checks
  bin/ui-test-focus.py show esc   just those
  bin/ui-test-focus.py --verbose
"""
import json, os, shutil, socket, subprocess, sys, time

ROOT = os.path.dirname(os.path.dirname(os.path.realpath(__file__)))
APP = os.path.join(ROOT, "workspace-switcher.app")
BIN = os.path.join(APP, "Contents/MacOS/workspace-switcher")
SOCK = os.path.join(os.environ.get("TMPDIR", "/tmp"), "ws-notes.sock")
CONF = os.path.join(ROOT, "commands.toml")
VERBOSE = "--verbose" in sys.argv
ONLY = [a for a in sys.argv[1:] if not a.startswith("-")]

PASS = FAIL = SKIP = 0
TIMES = []


def ok(msg):
    global PASS
    PASS += 1
    print(f"PASS: {msg}")


def bad(msg):
    global FAIL
    FAIL += 1
    print(f"FAIL: {msg}")


def skip(msg):
    global SKIP
    SKIP += 1
    print(f"SKIP: {msg}")


def vlog(msg):
    if VERBOSE:
        print(f"  → {msg}")


def query(q, timeout=3.0):
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(timeout)
    try:
        s.connect(SOCK)
        s.sendall((q + "\n").encode())
        buf = b""
        while not buf.endswith(b"\n"):
            chunk = s.recv(65536)
            if not chunk:
                break
            buf += chunk
        return json.loads(buf) if buf.strip() else {}
    except (OSError, ValueError):
        return {}
    finally:
        s.close()


def state():
    return query("state")


def do(action):
    return query("do:" + action)


def wait(pred, timeout=3.0, poll=0.005):
    """poll state until pred(state) — (ms it took, last state) or (None, state)"""
    t0 = time.perf_counter()
    st = {}
    while time.perf_counter() - t0 < timeout:
        st = state()
        try:
            if st and pred(st):
                return (time.perf_counter() - t0) * 1000, st
        except (KeyError, TypeError):
            pass
        time.sleep(poll)
    return None, st


def aero(*args):
    r = subprocess.run(["aerospace", *args], capture_output=True, text=True, timeout=5)
    return r.stdout.strip()


def hotkey(mode="window"):
    # exactly what aerospace.toml's binding runs
    subprocess.Popen([BIN, mode], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def cur(st):
    v = st.get("view") or ""
    return st.get("views", {}).get(v, {}) if v else {}


def shown_and_key(st):
    return st.get("visible") and st.get("active") and cur(st).get("key")


def our_windows():
    """{wid: (workspace, layout)} for the daemon's windows, per AeroSpace"""
    pid = str(state().get("pid", ""))
    out = {}
    for line in aero("list-windows", "--all", "--format",
                     "%{window-id}|%{app-pid}|%{workspace}|%{window-layout}").splitlines():
        f = line.split("|")
        if len(f) == 4 and f[1] == pid:
            out[int(f[0])] = (f[2], f[3])
    return out


def still(seconds=1.0):
    """frames of the visible view sampled for `seconds`: (moved?, samples)"""
    frames = []
    t0 = time.perf_counter()
    while time.perf_counter() - t0 < seconds:
        f = cur(state()).get("frame")
        if f:
            frames.append(tuple(f))
        time.sleep(0.015)
    return len(set(frames)) > 1, frames


def timed(label, ms, budget):
    TIMES.append((label, ms))
    if ms is None:
        bad(f"{label}: never happened")
    elif ms <= budget:
        ok(f"{label}: {ms:.0f} ms (budget {budget} ms)")
    else:
        bad(f"{label}: {ms:.0f} ms — over the {budget} ms budget")


def ensure_shown():
    if not state().get("visible"):
        hotkey()
        wait(shown_and_key, 3)


def ensure_hidden():
    if state().get("visible"):
        do("hide")
        wait(lambda s: not s["visible"], 2)
        # a hide hands focus back (aerospace focus, async): let it land
        # before the next workspace switch, like a human would
        time.sleep(0.3)


def want(name):
    return not ONLY or name in ONLY


# ---------------------------------------------------------------- setup
if not os.path.exists(SOCK) or not state():
    print(f"no daemon answering on {SOCK} — start it (./build.sh) first")
    sys.exit(1)
if not shutil.which("aerospace"):
    print("aerospace not on PATH")
    sys.exit(1)

conf_snapshot = open(CONF, "rb").read()
orig_ws = aero("list-workspaces", "--focused")
orig_focus = aero("list-windows", "--focused", "--format", "%{window-id}")
st0 = state()
orig_view, orig_visible = st0.get("view") or "", st0.get("visible")
others = [w for w in aero("list-workspaces", "--monitor", "focused").split() if w != orig_ws]
other_ws = others[0] if others else None
print(f"== focus / workspace tests (daemon pid {st0.get('pid')}, workspace {orig_ws} -> {other_ws}, "
      f"hideOnFocusLoss {st0.get('hideOnFocusLoss')}) ==")


orig_esc = st0.get("escHides", {})


TOOLS = ("prettyprint", "health-checks", "filefast", "paths")


def restore():
    for n in TOOLS:
        do("tool-close:" + n)
    for v, on in orig_esc.items():
        if state().get("escHides", {}).get(v) != on:
            do(f"esc-hides:{v}:{'on' if on else 'off'}")
    if open(CONF, "rb").read() != conf_snapshot:
        open(CONF, "wb").write(conf_snapshot)
        vlog("commands.toml restored")
    if orig_ws:
        aero("workspace", orig_ws)
    if orig_visible:
        if orig_view:
            do("open:" + orig_view)
    else:
        ensure_hidden()
    if orig_focus:
        aero("focus", "--window-id", orig_focus)


try:
    # ------------------------------------------------------------ single
    if want("single"):
        def daemons():
            r = subprocess.run(["pgrep", "-f", "workspace-switcher.app/Contents/MacOS/workspace-switcher"],
                               capture_output=True, text=True)
            pids = []
            for p in r.stdout.split():
                ps = subprocess.run(["ps", "-o", "command=", "-p", p], capture_output=True, text=True).stdout
                if "workspace-switcher.app/Contents/MacOS/workspace-switcher" in ps and "nvim" not in ps:
                    pids.append(p)
            return pids
        before = daemons()
        pid = st0.get("pid")
        subprocess.run(["open", "-n", "-g", APP], capture_output=True)
        time.sleep(2.0)
        after = daemons()
        if len(after) == 1 and state().get("pid") == pid:
            ok(f"single daemon: a second launch handed over and exited (pid {pid})")
        else:
            bad(f"single daemon: before {before}, after {after}, socket pid {state().get('pid')} (was {pid})")

    # ------------------------------------------------------------ show / hide
    if want("show") or want("hide"):
        ensure_hidden()
        if orig_focus:
            aero("focus", "--window-id", orig_focus)
        time.sleep(0.2)
        hotkey()
        ms, st = wait(shown_and_key, 3)
        timed("hotkey -> shown + key", ms, 250)
        if ms is not None:
            wid = cur(st).get("wid")
            wins = our_windows()
            ws_now = aero("list-workspaces", "--focused")
            if wid in wins:
                ws, layout = wins[wid]
                (ok if layout == "floating" else bad)(f"AeroSpace layout of the window: {layout}")
                (ok if ws == ws_now else bad)(f"on the focused workspace ({ws} vs {ws_now})")
            else:
                bad(f"AeroSpace doesn't list window {wid}: {wins}")
            moved, frames = still()
            (bad if moved else ok)(f"frame holds still for 1 s after show ({len(set(frames))} distinct of {len(frames)})")
            vlog(f"frames: {sorted(set(frames))}")
        if want("hide"):
            hotkey()
            ms, _ = wait(lambda s: not s["visible"], 2)
            timed("hotkey while in it -> hidden", ms, 200)

    # ------------------------------------------------------------ follow
    if want("follow") and other_ws:
        ensure_shown()
        frame = cur(state()).get("frame")
        ensure_hidden()
        aero("workspace", other_ws)
        time.sleep(0.25)
        hotkey()
        ms, st = wait(shown_and_key, 3)
        timed(f"hidden on {orig_ws}, hotkey on {other_ws} -> shown + key", ms, 250)
        if ms is not None:
            wins = our_windows()
            wid = cur(st).get("wid")
            ws = wins.get(wid, ("?", "?"))[0]
            (ok if ws == other_ws else bad)(f"it came to {other_ws} (AeroSpace: {ws})")
            f = cur(st).get("frame")
            (ok if f == frame else bad)(f"same frame as on {orig_ws}: {f} vs {frame}")
            moved, frames = still()
            (bad if moved else ok)(f"no resize / move after the follow ({len(set(frames))} distinct frames)")
    elif want("follow"):
        skip("follow: no second workspace on this monitor")

    # ------------------------------------------------------------ stranded
    if want("stranded") and other_ws:
        aero("workspace", other_ws)
        ensure_shown()
        frame = cur(state()).get("frame")
        aero("workspace", orig_ws)          # the window stays up on other_ws (or hides on focus loss)
        time.sleep(0.4)
        left_up = state().get("visible")
        hotkey()
        t0 = time.perf_counter()
        ms, st = wait(shown_and_key, 3)
        first = cur(st).get("frame") if ms is not None else None
        timed(f"{'left up' if left_up else 'hidden (focus loss)'} on {other_ws}, hotkey on {orig_ws} -> shown + key", ms, 250)
        if ms is not None:
            wid = cur(st).get("wid")
            ws = our_windows().get(wid, ("?", "?"))[0]
            (ok if ws == orig_ws else bad)(f"it came to {orig_ws} (AeroSpace: {ws})")
            (ok if first == frame else bad)(f"first frame on screen is the remembered one (no slide): {first} vs {frame}")
            moved, frames = still()
            (bad if moved else ok)(f"no resize / move after it arrived ({len(set(frames))} distinct frames)")
    elif want("stranded"):
        skip("stranded: no second workspace on this monitor")

    # ------------------------------------------------------------ swap
    if want("swap") and other_ws:
        # notes last hidden on orig_ws, then shown again on other_ws via a
        # view switch: AeroSpace must not "restore the world" (jump back)
        aero("workspace", orig_ws)
        ensure_shown()
        do("open:notes")
        wait(lambda s: s["view"] == "notes" and shown_and_key(s), 3)
        do("open:files")
        wait(lambda s: s["view"] == "files" and shown_and_key(s), 3)
        ensure_hidden()
        aero("workspace", other_ws)
        time.sleep(0.25)
        hotkey()
        wait(shown_and_key, 3)
        t0 = time.perf_counter()
        do("open:notes")
        ms, st = wait(lambda s: s["view"] == "notes" and shown_and_key(s), 3)
        timed(f"view switch to notes on {other_ws} (notes last hidden on {orig_ws})", ms, 150)
        time.sleep(0.5)
        ws_now = aero("list-workspaces", "--focused")
        (ok if ws_now == other_ws else bad)(f"no jump back: focused workspace {ws_now} (want {other_ws})")
        wid = cur(state()).get("wid")
        ws = our_windows().get(wid, ("?", "?"))[0]
        (ok if ws == other_ws else bad)(f"notes window bound to {other_ws} (AeroSpace: {ws})")
        do("open:files")
        wait(lambda s: s["view"] == "files" and shown_and_key(s), 3)
    elif want("swap"):
        skip("swap: no second workspace on this monitor")

    # ------------------------------------------------------------ focus
    if want("focus"):
        if orig_ws:
            aero("workspace", orig_ws)
        ensure_shown()
        st = state()
        wid = cur(st).get("wid")
        other = [l.split("|")[0] for l in aero("list-windows", "--workspace", "focused", "--format",
                                                "%{window-id}|%{app-pid}").splitlines()
                 if l.split("|")[1] != str(st.get("pid"))]
        if not other or not wid:
            skip("focus: no other window on this workspace to bounce through")
        else:
            aero("focus", "--window-id", other[0])
            wait(lambda s: not s["active"] or not cur(s).get("key"), 2)
            time.sleep(0.4)  # longer than focus-loss-delay: a real "left it"
            if not state().get("visible"):
                skip("focus: hide-on-focus-loss hid the window — nothing to refocus")
            else:
                t0 = time.perf_counter()
                aero("focus", "--window-id", str(wid))
                ms, _ = wait(shown_and_key, 3)
                timed("aerospace focus (alt-hjkl) -> ours is key", ms, 200)

    # ------------------------------------------------------------ esc
    if want("esc"):
        def esc():
            # System Events: cliclick's synthetic keys don't reach other apps
            # from every session (no Accessibility for its parent)
            subprocess.run(["osascript", "-e", 'tell application "System Events" to key code 53'],
                           capture_output=True)
        if subprocess.run(["osascript", "-e", 'tell application "System Events" to return 1'],
                          capture_output=True).returncode != 0:
            skip("esc: System Events not scriptable from here (Accessibility / Automation permission)")
        else:
            do("open:files")
            ensure_shown()
            wait(lambda s: s["view"] == "files" and shown_and_key(s), 2)
            do("esc-hides:files:off")
            esc()
            time.sleep(0.5)
            (ok if state().get("visible") else bad)("Esc with \"Esc Hides Window\" off: the window stays")
            do("esc-hides:files:on")
            wait(shown_and_key, 2)
            esc()
            ms, _ = wait(lambda s: not s["visible"], 2)
            timed("Esc with \"Esc Hides Window\" on -> hidden", ms, 150)

    # ------------------------------------------------------------ tools
    if want("tools"):
        def screen_h():
            # main screen height: AppKit frames (bottom-left) -> cliclick (top-left)
            r = subprocess.run(["osascript", "-l", "JavaScript", "-e",
                                'ObjC.import("AppKit"); $.NSScreen.screens.objectAtIndex(0).frame.size.height'],
                               capture_output=True, text=True)
            try:
                return float(r.stdout.strip())
            except ValueError:
                return None

        def tool(st, n):
            return st.get("tools", {}).get(n, {})

        # summon it onto the starting workspace (hidden + hotkey there, like
        # `follow`): the checks need another app's window beside it
        ensure_hidden()
        if orig_ws:
            aero("workspace", orig_ws)
            time.sleep(0.25)
        ensure_shown()
        st = state()
        shared_view, shared_frame = st.get("view"), cur(st).get("frame")
        pid = str(st.get("pid"))
        other = [l.split("|")[0] for l in aero("list-windows", "--workspace", "focused", "--format",
                                                "%{window-id}|%{app-pid}").splitlines()
                 if l.split("|")[1] != pid]
        sh = screen_h() if shutil.which("cliclick") else None

        def away():
            # another app's window takes focus: our app inactive
            aero("focus", "--window-id", other[0])
            wait(lambda s: s.get("frontmostPid") != s.get("pid"), 2)
            time.sleep(0.4)  # longer than focus-loss-delay

        def unmoved(label, a0):
            time.sleep(0.4)  # the 0.25 s key take-back + any late activation
            s = state()
            # (`active` reads true while a non-activating panel is key: the
            # activation count + the frontmost app are what tell)
            front = s.get("frontmostPid")
            (ok if s.get("activations") == a0 and front != s.get("pid") else bad)(
                f"{label}: app not activated (activations {a0} -> {s.get('activations')}, frontmost pid {front})")
            v = s.get("views", {}).get(shared_view, {})
            (ok if not v.get("key") and v.get("frame") == shared_frame else bad)(
                f"{label}: shared {shared_view} untouched (key {v.get('key')}, frame {v.get('frame')} vs {shared_frame})")
            return s

        if not other:
            skip("tools: no other app's window on this workspace to stand in for 'another app'")
        else:
            for n in TOOLS:
                away()
                a0 = state().get("activations")
                r = do("tool:" + n)
                if "error" in r:
                    skip(f"tools: {n}: {r['error']}")
                    continue
                ms, st = wait(lambda s: tool(s, n).get("shown") and tool(s, n).get("key"), 3)
                timed(f"/{n} -> shown + key", ms, 300)
                if ms is None:
                    continue
                st = unmoved(f"/{n} opened", a0)
                wid = tool(st, n).get("wid")
                (bad if wid in our_windows() else ok)(f"/{n}: AeroSpace doesn't list it")
                do("tool:" + n)  # re-run, as from Hyper+S again
                unmoved(f"/{n} re-run", a0)
                if sh:
                    away()
                    a0 = state().get("activations")
                    x, y, w, h = tool(state(), n).get("frame", [0, 0, 0, 0])
                    subprocess.run(["cliclick", f"c:{int(x + w / 2)},{int(sh - (y + h / 2))}"], capture_output=True)
                    wait(lambda s: tool(s, n).get("key"), 2)
                    s = unmoved(f"/{n} clicked", a0)
                    (ok if tool(s, n).get("key") else bad)(f"/{n} clicked: it has the keyboard")
                do("tool-close:" + n)
                wait(lambda s: not tool(s, n).get("shown"), 2)
            if not sh:
                skip("tools: click checks need cliclick + the screen height")

            # /screenshot: full-screen overlays at .screenSaver, still a tool
            # panel — the app never activates, the shared view stays put
            def shot(s):
                return s.get("screenshot", {})
            if not shot(state()).get("permission"):
                skip("tools: screenshot: no Screen Recording permission")
            else:
                away()
                a0 = state().get("activations")
                do("screenshot:show")
                ms, st = wait(lambda s: shot(s).get("shown")
                              and any(d.get("key") for d in shot(s).get("displays", [])), 3)
                timed("/screenshot -> overlay shown + key", ms, 400)
                if ms is not None:
                    st = unmoved("/screenshot opened", a0)
                    wids = {d.get("wid") for d in shot(st).get("displays", [])}
                    (bad if wids & set(our_windows()) else ok)("/screenshot: AeroSpace doesn't list the overlay")
                    do("screenshot:select:200,200,300,200")
                    st = state()
                    names = [b["name"] for b in shot(st).get("buttons", [])]
                    (ok if names[:3] == ["pencil", "line", "arrow"] else bad)(f"/screenshot: the ring starts pencil, line, arrow ({names[:3]})")
                    do("screenshot:key:esc")   # one Esc with nothing open = exit
                    ms, _ = wait(lambda s: not shot(s).get("shown"), 2)
                    timed("/screenshot Esc -> closed", ms, 300)
                    unmoved("/screenshot closed", a0)

        # in the shared window (app active), a tool on top: the hotkey
        # focuses the shared window, it doesn't hide it
        ensure_shown()
        if shared_view:
            do("open:" + shared_view)
        wait(shown_and_key, 2)
        do("tool:prettyprint")
        ms, _ = wait(lambda s: tool(s, "prettyprint").get("key"), 3)
        if ms is None:
            skip("tools: prettyprint didn't open for the hotkey check")
        else:
            time.sleep(0.4)
            hotkey()
            ms, _ = wait(shown_and_key, 3)
            timed("hotkey from a tool panel -> shared window shown + key (not hidden)", ms, 300)
            do("tool-close:prettyprint")
finally:
    restore()

print()
if TIMES:
    print("timings: " + ", ".join(f"{l} {'—' if m is None else f'{m:.0f} ms'}" for l, m in TIMES))
print(f"{PASS} passed, {FAIL} failed, {SKIP} skipped")
sys.exit(1 if FAIL else 0)
