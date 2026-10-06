#!/usr/bin/env python3
"""ws-settings (settings_hub/) tests — PRD-settings-hub.md §9.

Everything runs against throwaway copies: a temp commands.toml whose
[settings-hub] points at fixture aerospace / herdr / ghostty / vim files, a
stub app binary (config-schema / config-check / reload), and stub
aerospace / herdr / pbcopy / pbpaste first on PATH. The live
classes are opt-in: WS_LIVE=1 (the running daemon) and REAL_BINARY (the
built app's own validation, skipped when it isn't built).

    python3 Tests/test_settings_hub.py
"""
from __future__ import annotations

import hashlib
import json
import os
import pty
import select
import shutil
import struct
import subprocess
import sys
import tempfile
import termios
import fcntl
import time
import tomllib
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)
sys.path.insert(0, os.path.join(ROOT, "pylib"))

import config_text as ct  # noqa: E402

COMMANDS = r'''# fixture commands.toml
#   label      any command: text the palette shows
[app]
# hide when another app gets focus
#   hide-on-focus-loss  true/false (default true); false = stays up
hide-on-focus-loss = false
shell = "/bin/bash"   # the runner's shell

# /screenshot
#   contrast-opacity    0-255 veil over the unselected area
#   return              what Return does: copy (default) | save | pin
[screenshot]
enabled = true
contrast-opacity = 188
return = "copy"

# /paths
#   return       what Return does: file (default) | path | open
[paths]
enabled = true
  label = "Find Recent Files"
return = "file"

[notes]
vim-mode = true
# vim-init = ~/.config/nvim/init.lua
font-size = 16

[shortcuts]
"all: Hyper+X" = "screenshot"
"all: Cmd+W" = "hide the window"
"all: Ctrl+Shift+H / J / K / L" = "resize"
"files: Cmd+K" = "copy the selected file's path"
"files: Esc Esc" = "hide"
"files: Shift+click / Cmd+click / Cmd+A" = "select several files"
"screenshot: P D A" = "pencil, line, arrow"
"screenshot: Mouse drag" = "select an area"
"jira: 1-3" = "in the Cmd+K menu: run that action"
"all: Ctrl+B L" = "previous view"
"all: Ctrl+B W" = "view switcher"
"sidebar: ↑ ↓" = "move through the rows"

[settings-hub]
layers = "app, aerospace, herdr, ghostty, vim"
aerospace-config = "@DIR@/aerospace.toml"
herdr-config = "@DIR@/herdr.toml"
ghostty-config = "@DIR@/ghostty.conf"
vim-init = "@DIR@/init.vim"
vim-bin = "/nonexistent/nvim"
colors = "terminal"
'''

AEROSPACE = r'''# fixture aerospace.toml
[mode.main.binding]
    # Screenshot: Hyper+X
    alt-cmd-ctrl-shift-x = 'exec-and-forget /bin/echo workspace-switcher screenshot'
    alt-h = 'focus left'
    cmd-k = 'focus up'   # clashes with files: Cmd+K
    alt-shift-slash = ['layout v_tiles', 'balance-sizes']
    alt-shift-semicolon = 'mode service'
    alt-g = 'exec-and-forget /nonexistent/tool'

[mode.service.binding]
    esc = ['reload-config', 'mode main']
    r = ['flatten-workspace-tree', 'mode main']
'''

HERDR = r'''[keys]
focus_pane_left = ["prefix+h", "ctrl+h"]
resize_pane_left = "alt+h"
# Built-in workspace picker disabled
workspace_picker = ""

[[keys.command]]
key = "prefix+w"
type = "popup"
command = "~/x.sh"

[ui]
sidebar_width = 26

# copy a path
[[keys.command]]
key = "prefix+f"
type = "plugin_action"
command = "local.copy-path.open"
description = "copy path"
'''

GHOSTTY = '''keybind = global:alt+space=toggle_quick_terminal
font-size = 14
keybind = ctrl+s=unbind
'''

VIM = '''" fixture
xnoremap p P
nnoremap <C-x> :q<CR>
'''

STUB_APP = r'''#!/usr/bin/env python3
import json, os, sys
a = sys.argv[1:]
log = os.environ.get("STUB_LOG")
if log:
    with open(log, "a") as fh:
        fh.write("app " + " ".join(a) + "\n")
if a[:1] == ["config-schema"]:
    print(json.dumps({"version": 1, "numberRanges": {"contrast-opacity": [0, 255], "font-size": [6, 96]},
                      "boolKeys": ["enabled", "hide-on-focus-loss", "vim-mode"], "colorKeys": [],
                      "enumKeys": {}}))
elif a[:1] == ["config-check"]:
    if a[1] == "--file":
        print(json.dumps({"ok": True, "issues": []}))
    else:
        sec, key, val = a[1:4]
        prob = None
        if key == "contrast-opacity":
            try:
                if not 0 <= float(val) <= 255:
                    prob = f"{val} is outside 0…255"
            except ValueError:
                prob = f"'{val}' is not a number"
        if sec == "screenshot" and key == "return" and val not in ("copy", "save", "pin"):
            prob = f"'{val}' is not one of copy | pin | save"
        print(json.dumps({"ok": prob is None, **({"problem": prob} if prob else {})}))
elif a[:1] in (["reload"], ["restart"]):
    if os.environ.get("STUB_DAEMON") == "down":
        print("workspace-switcher is not running", file=sys.stderr)
        sys.exit(1)
    print('{"commands":3,"issues":[],"ok":true,"usingBackup":false}')
elif a[:1] == ["jira-poll"]:
    sys.exit(0)
else:
    sys.exit(2)
'''

STUB_TOOL = r'''#!/bin/bash
echo "$(basename "$0") $*" >> "${STUB_LOG:-/dev/null}"
case "$(basename "$0") $1 $2" in
  "aerospace reload-config --dry-run") exit "${STUB_AERO_DRYRUN_RC:-0}" ;;
  "herdr config check") exit "${STUB_HERDR_CHECK_RC:-0}" ;;
esac
case "$(basename "$0")" in
  pbcopy) cat > "$STUB_CLIP" ;;
  pbpaste) printf '%s' "${STUB_PASTE:-}" ;;
  aerospace) [ "$1" = list-windows ] && exit 0; [ "$1" = list-workspaces ] && echo 1 ;;
esac
exit 0
'''


def sha(path: str) -> str:
    with open(path, "rb") as fh:
        return hashlib.sha256(fh.read()).hexdigest()


class Env(unittest.TestCase):
    """A temp world: fixtures, stubs, env. `run(...)` = the CLI."""

    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="ws-settings-test-")
        d = self.tmp
        self.fx = os.path.join(d, "fx")
        os.makedirs(self.fx)
        for name, text in (("aerospace.toml", AEROSPACE), ("herdr.toml", HERDR),
                           ("ghostty.conf", GHOSTTY), ("init.vim", VIM)):
            with open(os.path.join(self.fx, name), "w") as fh:
                fh.write(text)
        # commands.toml is a LINK to the real file (the install's layout)
        self.real = os.path.join(self.fx, "commands.real.toml")
        with open(self.real, "w") as fh:
            fh.write(COMMANDS.replace("@DIR@", self.fx))
        self.conf = os.path.join(d, "commands.toml")
        os.symlink(self.real, self.conf)
        stubs = os.path.join(d, "stubs")
        os.makedirs(stubs)
        self.app = os.path.join(stubs, "workspace-switcher")
        with open(self.app, "w") as fh:
            fh.write(STUB_APP.replace("#!/usr/bin/env python3", "#!" + sys.executable, 1))
        os.chmod(self.app, 0o755)
        for tool in ("aerospace", "herdr", "pbcopy", "pbpaste"):
            p = os.path.join(stubs, tool)
            with open(p, "w") as fh:
                fh.write(STUB_TOOL)
            os.chmod(p, 0o755)
        self.log = os.path.join(d, "stub.log")
        self.clip = os.path.join(d, "clip.txt")
        self.env = dict(os.environ,
                        WS_HOME=os.path.join(d, "home"), WS_COMMANDS_CONF=self.conf,
                        WS_SETTINGS_BIN=self.app, WS_SETTINGS_CACHE=os.path.join(d, "cache"),
                        WS_SETTINGS_FAVORITES=os.path.join(d, "fav.json"),
                        PATH=stubs + ":/usr/bin:/bin", STUB_LOG=self.log, STUB_CLIP=self.clip,
                        TMPDIR=os.path.join(d, "tmp") + "/", PYTHONPATH=ROOT,
                        PYTHONDONTWRITEBYTECODE="1")
        os.makedirs(self.env["WS_HOME"])
        os.makedirs(self.env["TMPDIR"])

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def run_cli(self, *args, env=None, flags=()):
        e = dict(self.env, **(env or {}))
        p = subprocess.run([sys.executable, "-B", *flags, "-m", "settings_hub", *args],
                           capture_output=True, text=True, env=e, cwd=ROOT, timeout=60)
        return p.returncode, p.stdout, p.stderr

    def json_cli(self, *args):
        rc, out, err = self.run_cli(*args, "--json")
        self.assertEqual(rc, 0, err)
        return json.loads(out)

    def stub_calls(self) -> list:
        try:
            with open(self.log) as fh:
                return fh.read().splitlines()
        except OSError:
            return []


# ---------------------------------------------------------------- §9.1
class Catalog(Env):
    def test_every_source_listed(self):
        keys = self.json_cli("keys")
        by = {}
        for k in keys:
            by.setdefault(k["layer"], []).append(k)
        aero = tomllib.loads(AEROSPACE)["mode"]
        self.assertEqual(len(by["aerospace"]), len(aero["main"]["binding"]) + len(aero["service"]["binding"]))
        shortcuts = ct.config_section_entries(COMMANDS.split("\n"), "shortcuts")
        self.assertEqual(len(by["app"]), len(shortcuts))
        self.assertEqual([k["action"] for k in by["app"]], [v for _, _, v in shortcuts])
        # herdr: a list = one row per chord, "" = unbound (no row), commands included
        herdr = {(k["keys"], k["action"]) for k in by["herdr"]}
        self.assertEqual(herdr, {("Ctrl+B, H", "focus pane left"), ("Ctrl+H", "focus pane left"),
                                 ("Opt+H", "resize pane left"), ("Ctrl+B, W", "popup: ~/x.sh"),
                                 ("Ctrl+B, F", "copy path")})
        self.assertEqual({(k["view"], k["keys"]) for k in by["ghostty"]},
                         {("global", "Opt+Space"), ("terminal", "Ctrl+S")})
        self.assertEqual({k["source"] for k in by["vim"]}, {"p", "<C-x>"})
        # line numbers point at the real lines
        line = next(k for k in by["aerospace"] if k["source"] == "alt-h")["line"]
        self.assertEqual(AEROSPACE.split("\n")[line - 1].strip(), "alt-h = 'focus left'")
        cmd_f = next(k for k in by["herdr"] if k["source"] == "prefix+f")
        self.assertEqual(HERDR.split("\n")[cmd_f["line"] - 1], 'key = "prefix+f"')

    def test_service_mode_keys_carry_their_entry_key(self):
        keys = self.json_cli("keys", "--layer", "aerospace", "--view", "service")
        self.assertIn("Opt+Shift+;, Esc", [k["keys"] for k in keys])

    def test_label_parsing(self):
        keys = {k["source"]: k for k in self.json_cli("keys", "--layer", "app")}
        self.assertEqual(keys["Ctrl+Shift+H / J / K / L"]["chords"],
                         ["Ctrl+Shift+H", "Ctrl+Shift+J", "Ctrl+Shift+K", "Ctrl+Shift+L"])
        self.assertEqual(keys["P D A"]["chords"], ["P", "D", "A"])
        self.assertEqual(keys["Esc Esc"]["chords"], ["Esc, Esc"])
        self.assertEqual(keys["1-3"]["chords"], ["1", "2", "3"])
        self.assertEqual(keys["Shift+click / Cmd+click / Cmd+A"]["chords"], ["Cmd+A"])
        self.assertEqual(keys["Mouse drag"]["kind"], "gesture")
        # a prefix, then a key: ONE two-stroke chord (no bare Ctrl+B row)
        self.assertEqual(keys["Ctrl+B L"]["chords"], ["Ctrl+B, L"])

    def test_prefix_keys_are_no_clash(self):
        rc, out, err = self.run_cli("conflicts", "--no-system")
        self.assertNotIn("previous view", out + err)


# ---------------------------------------------------------------- parity
PARITY = r'''
[views]
files = ["window", "sidebar", "list"]
jira = ["window", "list"]

[[expect]]
kind = "window"
concept = "hide"
vscode = "Cmd+W"
keys = ["Cmd+W"]

[[expect]]
kind = "window"
concept = "views"
keys = ["Ctrl+B W"]

[[expect]]
kind = "sidebar"
concept = "rows"
keys = ["Up", "Down"]

[[expect]]
kind = "list"
concept = "copy path"
keys = ["Cmd+K | Cmd+C"]

[[expect]]
kind = "list"
concept = "jump"
keys = ["Home", "End"]

[[waive]]
view = "jira"
concept = "jump"
reason = "not yet"
'''


class Parity(Env):
    def setUp(self):
        super().setUp()
        self.parity = os.path.join(self.fx, "parity.toml")
        with open(self.parity, "w") as fh:
            fh.write(PARITY)
        self.env["WS_PARITY"] = self.parity

    def test_have_missing_waived(self):
        got = {(c["view"], c["concept"]): c for c in self.json_cli("parity", "--all")}
        # "all:" rows count everywhere, the prefix sequence too
        self.assertEqual(got[("files", "hide")]["status"], "have")
        self.assertEqual(got[("jira", "views")]["status"], "have")
        # "sidebar:" rows count for every view's sidebar
        self.assertEqual(got[("files", "rows")]["status"], "have")
        # either alternative: files lists Cmd+K, jira lists neither
        self.assertEqual(got[("files", "copy path")]["status"], "have")
        self.assertEqual(got[("jira", "copy path")]["status"], "missing")
        self.assertEqual(got[("jira", "copy path")]["missing"], ["Cmd+K | Cmd+C"])
        self.assertEqual(got[("files", "jump")]["status"], "missing")
        self.assertEqual(got[("jira", "jump")]["status"], "waived")
        self.assertEqual(got[("jira", "jump")]["reason"], "not yet")

    def test_default_shows_only_gaps(self):
        rows = self.json_cli("parity")
        self.assertEqual({(c["view"], c["concept"]) for c in rows},
                         {("jira", "copy path"), ("files", "jump")})
        rc, out, _ = self.run_cli("parity", "--view", "files")
        self.assertEqual(rc, 0)
        self.assertIn("1 missing of 5", out)
        rc, _, err = self.run_cli("parity", "--view", "nope")
        self.assertEqual(rc, 2)


# ---------------------------------------------------------------- §9.2
class HyperKeys(Env):
    def test_hyper_query(self):
        keys = self.json_cli("keys", "hyper")
        self.assertTrue(keys)
        self.assertTrue(all(any(c.startswith("Hyper+") for c in k["chords"]) for k in keys))
        aero = next(k for k in keys if k["layer"] == "aerospace")
        # the AeroSpace binding that runs the app reads like the app's own row
        self.assertEqual(aero["action"], "screenshot")
        self.assertEqual(aero["mirrorOf"], "app:all:Hyper+X")


# ---------------------------------------------------------------- §9.3
class Settings(Env):
    def test_section_with_docs(self):
        rows = {r["key"]: r for r in self.json_cli("settings", "--section", "screenshot")}
        self.assertEqual(set(rows), {"enabled", "contrast-opacity", "return"})
        self.assertEqual(rows["contrast-opacity"]["value"], "188")
        self.assertIn("veil", rows["contrast-opacity"]["doc"])
        self.assertEqual(rows["return"]["allowed"], ["copy", "save", "pin"])

    def test_commented_out_and_indented(self):
        rows = {(r["section"], r["key"]): r for r in self.json_cli("settings")}
        self.assertFalse(rows[("notes", "vim-init")]["set"])
        self.assertEqual(rows[("paths", "label")]["value"], "Find Recent Files")
        self.assertIn("text the palette shows", rows[("paths", "label")]["doc"])   # file-top docs
        self.assertEqual(rows[("app", "hide-on-focus-loss")]["type"], "bool")


# ---------------------------------------------------------------- §9.4 / 9.5
class Writes(Env):
    def test_refused_written_undone(self):
        before = sha(self.real)
        rc, out, err = self.run_cli("set", "screenshot.contrast-opacity", "999")
        self.assertEqual(rc, 2)
        self.assertIn("outside 0…255", err)
        self.assertEqual(sha(self.real), before)
        rc, out, err = self.run_cli("set", "screenshot.contrast-opacity", "150")
        self.assertEqual(rc, 0, err)
        with open(self.real) as fh:
            new = fh.read().split("\n")
        old = COMMANDS.replace("@DIR@", self.fx).split("\n")
        diff = [(a, b) for a, b in zip(old, new) if a != b]
        self.assertEqual(len(new), len(old))
        self.assertEqual(diff, [("contrast-opacity = 188", "contrast-opacity = 150")])
        rc, out, err = self.run_cli("undo")
        self.assertEqual(rc, 0, err)
        self.assertEqual(sha(self.real), before)

    def test_symlink_kept_and_mode(self):
        os.chmod(self.real, 0o640)
        rc, _, err = self.run_cli("set", "app.hide-on-focus-loss", "true")
        self.assertEqual(rc, 0, err)
        self.assertTrue(os.path.islink(self.conf))
        self.assertEqual(os.stat(self.real).st_mode & 0o777, 0o640)
        with open(self.real) as fh:
            self.assertIn("hide-on-focus-loss = true", fh.read())

    def test_trailing_comment_kept(self):
        rc, _, err = self.run_cli("set", "app.shell", "/bin/zsh", "--no-apply")
        self.assertEqual(rc, 0, err)
        with open(self.real) as fh:
            self.assertIn('shell = "/bin/zsh"   # the runner\'s shell', fh.read())

    def test_uncomment_in_place_and_new_section(self):
        rc, _, err = self.run_cli("set", "notes.vim-init", "~/x.lua", "--no-apply")
        self.assertEqual(rc, 0, err)
        rc, _, err = self.run_cli("set", "brand-new.key", "1", "--no-apply")
        self.assertEqual(rc, 0, err)
        with open(self.real) as fh:
            text = fh.read()
        self.assertIn('vim-mode = true\nvim-init = "~/x.lua"\nfont-size = 16', text)
        self.assertTrue(text.rstrip("\n").endswith("[brand-new]\nkey = 1"))
        before = COMMANDS.replace("@DIR@", self.fx)
        self.run_cli("undo", "--no-apply")
        self.run_cli("undo", "--no-apply")
        with open(self.real) as fh:
            self.assertEqual(fh.read(), before)

    def test_doc_enum_is_a_warning(self):
        rc, _, err = self.run_cli("set", "paths.return", "clipboard", "--no-apply")
        self.assertEqual(rc, 2)
        self.assertIn("--force", err)
        rc, _, err = self.run_cli("set", "paths.return", "clipboard", "--no-apply", "--force")
        self.assertEqual(rc, 0, err)

    def test_app_rule_is_a_refusal(self):
        rc, _, err = self.run_cli("set", "screenshot.return", "clip", "--force")
        self.assertEqual(rc, 2)
        self.assertIn("copy | pin | save", err)

    def test_one_line_only(self):
        rc, _, err = self.run_cli("set", "app.shell", "a\nb")
        self.assertEqual(rc, 2)


# ---------------------------------------------------------------- §9.6 apply (stub) + §9.9 no daemon
class Apply(Env):
    def test_reload_sent(self):
        rc, out, err = self.run_cli("set", "app.hide-on-focus-loss", "true")
        self.assertEqual(rc, 0, err)
        self.assertIn("app reload", self.stub_calls())
        self.assertIn("re-read by the running app", out)

    def test_trigger_section_needs_no_reload(self):
        rc, out, _ = self.run_cli("set", "screenshot.contrast-opacity", "100")
        self.assertEqual(rc, 0)
        self.assertNotIn("app reload", self.stub_calls())
        self.assertIn("next use", out)

    def test_no_daemon(self):
        rc, out, err = self.run_cli("set", "app.hide-on-focus-loss", "true", env={"STUB_DAEMON": "down"})
        self.assertEqual(rc, 0, err)
        self.assertIn("applies on next start", out)

    def test_everything_reads_without_a_daemon_or_binary(self):
        env = {"WS_SETTINGS_BIN": "/nonexistent/app", "STUB_DAEMON": "down"}
        for verb in (("keys",), ("settings",), ("get", "screenshot.return"), ("export", "md"),
                     ("conflicts",), ("doctor",)):
            rc, out, err = self.run_cli(*verb, env=env)
            self.assertIn(rc, (0, 1), (verb, err))
            self.assertTrue(out.strip(), verb)
        rc, out, err = self.run_cli("set", "screenshot.contrast-opacity", "50", env=env)
        self.assertEqual(rc, 0, err)

    def test_jira_switch_goes_through_the_app(self):
        rc, out, err = self.run_cli("set", "jira.enabled", "true")
        self.assertEqual(rc, 0, err)
        self.assertIn("app jira-poll on", self.stub_calls())


# ---------------------------------------------------------------- §9.7
class Clashes(Env):
    def test_seeded(self):
        got = self.json_cli("conflicts")
        pairs = {(c["winner"]["layer"], c["winner"]["source"], c["loser"]["layer"], c["loser"]["source"])
                 for c in got}
        self.assertIn(("aerospace", "cmd-k", "app", "Cmd+K"), pairs)
        self.assertIn(("aerospace", "alt-h", "herdr", "alt+h"), pairs)
        # the AeroSpace binding that RUNS the app's Hyper+X is not a clash
        self.assertFalse([p for p in pairs if "alt-cmd-ctrl-shift-x" in p])

    def test_ghostty_global(self):
        got = self.json_cli("conflicts")
        self.assertFalse([c for c in got if c["loser"]["layer"] == "ghostty" and c["level"] == "error"])


# ---------------------------------------------------------------- rebind (v1.2)
class Rebind(Env):
    def test_aerospace_rebind_and_undo(self):
        aero = os.path.join(self.fx, "aerospace.toml")
        before = sha(aero)
        rc, out, err = self.run_cli("bind", "aerospace", "alt-h", "alt-shift-y")
        self.assertEqual(rc, 0, err)
        with open(aero) as fh:
            text = fh.read()
        self.assertIn("    alt-shift-y = 'focus left'\n", text)
        self.assertNotIn("alt-h =", text)
        self.assertIn("aerospace reload-config --dry-run --no-gui", self.stub_calls())
        self.run_cli("undo")
        self.assertEqual(sha(aero), before)

    def test_taken_key_refused(self):
        rc, _, err = self.run_cli("bind", "aerospace", "alt-h", "cmd+k")
        self.assertEqual(rc, 2)
        self.assertIn("already bound", err)

    def test_clash_with_another_layer_needs_force(self):
        rc, _, err = self.run_cli("bind", "aerospace", "alt-h", "ctrl+s")
        self.assertEqual(rc, 2, err)
        self.assertIn("--force", err)

    def test_rollback_when_aerospace_rejects(self):
        aero = os.path.join(self.fx, "aerospace.toml")
        before = sha(aero)
        rc, _, err = self.run_cli("bind", "aerospace", "alt-h", "alt-shift-y", env={"STUB_AERO_DRYRUN_RC": "1"})
        self.assertEqual(rc, 1)
        self.assertIn("undone", err)
        self.assertEqual(sha(aero), before)

    def test_herdr_list_value(self):
        herdr = os.path.join(self.fx, "herdr.toml")
        rc, out, err = self.run_cli("bind", "herdr", "ctrl+h", "ctrl+y")
        self.assertEqual(rc, 0, err)
        with open(herdr) as fh:
            self.assertIn('focus_pane_left = ["prefix+h", "ctrl+y"]', fh.read())
        rc, out, err = self.run_cli("bind", "herdr", "prefix+f", "prefix+g")
        self.assertEqual(rc, 0, err)
        with open(herdr) as fh:
            self.assertIn('key = "prefix+g"', fh.read())
        self.assertIn("herdr server reload-config", self.stub_calls())


# ---------------------------------------------------------------- favorites / export / doctor
class Misc(Env):
    def test_favorites(self):
        rc, out, _ = self.run_cli("fav", "app:files:Cmd+K")
        self.assertEqual(rc, 0)
        keys = self.json_cli("keys", "--favorites")
        self.assertEqual([k["id"] for k in keys], ["app:files:Cmd+K"])

    def test_export_md(self):
        rc, out, _ = self.run_cli("export", "md")
        self.assertEqual(rc, 0)
        self.assertIn("| `Cmd+K` | copy the selected file's path |", out)
        self.assertIn("### [screenshot]", out)

    def test_doctor_finds_dead_binding(self):
        rc, out, _ = self.run_cli("doctor")
        self.assertEqual(rc, 1)
        self.assertIn("/nonexistent/tool", out)

    def test_runs_without_site_packages(self):
        # -I -S: no site-packages, no PYTHONPATH — stdlib only
        code = ("import sys; sys.path.insert(0, %r); from settings_hub.cli import main; "
                "sys.exit(main(['keys', '--json']))" % ROOT)
        p = subprocess.run([sys.executable, "-I", "-S", "-B", "-c", code], capture_output=True,
                           text=True, env=self.env, cwd=self.tmp)
        self.assertEqual(p.returncode, 0, p.stderr)
        self.assertTrue(json.loads(p.stdout))

    def test_launcher(self):
        p = subprocess.run([os.path.join(ROOT, "bin", "ws-settings"), "get", "screenshot.return"],
                           capture_output=True, text=True, env=dict(self.env, WS_PYTHON=sys.executable))
        self.assertEqual((p.returncode, p.stdout.strip()), (0, "copy"), p.stderr)


# ---------------------------------------------------------------- units
class Units(unittest.TestCase):
    def test_codec_parts(self):
        self.assertEqual(ct.config_line_parts('  a = "x # y"  # note'), ("  ", "  # note"))
        self.assertEqual(ct.config_line_parts("a = 5 # n"), ("", " # n"))
        self.assertEqual(ct.config_set_line('  a = "x"  # keep', "a", "y z"), '  a = "y z"  # keep')
        self.assertEqual(ct.config_entry_span("    alt-h = 'focus left'"), (4, 9))

    def test_config_setting_mirrors_swift(self):
        lines = ["[a]", "x = 1", "x = 2", "", "# doc for b", "[b]", "y = 1"]
        self.assertEqual(ct.config_setting(lines, "a", [("x", "3")])[2], "x = 3")      # last dup
        self.assertEqual(ct.config_setting(lines, "a", [("z", "4")])[3], "z = 4")      # after last entry
        self.assertEqual(ct.config_setting(lines, "c", [("k", "v")])[-3:], ["", "[c]", 'k = "v"'])
        self.assertNotIn("x = 2", ct.config_setting(lines, "a", [("x", None)]))

    def test_chords(self):
        from settings_hub import chords
        self.assertEqual(chords.aerospace("alt-cmd-ctrl-shift-slash").text, "Hyper+/")
        self.assertEqual(chords.aerospace("alt-minus").text, "Opt+-")
        p = chords.plus_stroke("ctrl+b")
        self.assertEqual(chords.herdr("prefix+shift+a", p).text, "Ctrl+B, Shift+A")
        self.assertEqual(chords.ghostty("ctrl+`").text, "Ctrl+`")
        self.assertEqual(chords.vim("<C-x>").text, "Ctrl+X")
        self.assertEqual(chords.query_mods("⌘⇧k"), ({"cmd", "shift"}, "k"))

    def test_doc_parser(self):
        from settings_hub.settings import parse_docs
        docs = parse_docs(["#   a / b     shared text", "#             goes on",
                           "#   c         three | four", "# prose"])
        self.assertEqual(docs["a"], "shared text goes on")
        self.assertEqual(docs["b"], docs["a"])
        self.assertEqual(docs["c"], "three | four")

    def test_key_decoder(self):
        from settings_hub.tui.keys import decode
        self.assertEqual(decode("\x1b[200~a\nb\x1b[201~")[0].text, "a\nb")
        e = decode("\x1b[122;10u")[0]
        self.assertEqual((e.name, e.mods), ("z", frozenset({"cmd", "shift"})))
        self.assertEqual(decode("\x1b[27u")[0].name, "esc")
        self.assertEqual(decode("\x1b")[0].name, "esc")
        self.assertEqual(decode("\x1b1")[0].mods, frozenset({"alt"}))

    def test_lineedit(self):
        from settings_hub.tui.keys import Ev
        from settings_hub.tui.lineedit import LineEdit
        f = LineEdit("hello")
        f.handle(Ev("char", "a", frozenset({"cmd"})))
        f.handle(Ev("char", "x"))
        self.assertEqual(f.text, "x")
        f.handle(Ev("char", "z", frozenset({"cmd"})))
        self.assertEqual(f.text, "hello")
        f.handle(Ev("char", "z", frozenset({"cmd", "shift"})))
        self.assertEqual(f.text, "x")
        f.handle(Ev("paste", text="1\n2"))
        self.assertEqual(f.text, "x1 2")


# ---------------------------------------------------------------- §9.8 the picker, through a PTY
class PtyEnv(Env):
    """Runs the picker in a pseudo-terminal (120×40); `drive` sends key
    bytes and collects the WS_SETTINGS_STATE dump after each step."""

    def drive(self, steps, env=None):
        state = os.path.join(self.tmp, "state.json")
        e = dict(self.env, TERM="xterm-256color", WS_SETTINGS_STATE=state, STUB_PASTE="pasted",
                 **(env or {}))
        pid, fd = pty.fork()
        if pid == 0:
            os.chdir(ROOT)
            os.execvpe(sys.executable, [sys.executable, "-B", "-m", "settings_hub", "tui"], e)
        fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))

        def pump(t):
            end = time.time() + t
            while time.time() < end:
                r, _, _ = select.select([fd], [], [], 0.05)
                if r:
                    try:
                        os.read(fd, 65536)
                    except OSError:
                        return

        def st():
            for _ in range(40):
                try:
                    with open(state) as fh:
                        return json.load(fh)
                except (OSError, ValueError):
                    time.sleep(0.05)
            return {}
        pump(1.5)
        out = []
        for keys in steps:
            os.write(fd, keys)
            pump(0.35)
            out.append(st())
        pump(0.5)
        done, status = os.waitpid(pid, os.WNOHANG)
        if done == 0:
            os.kill(pid, 9)
            os.waitpid(pid, 0)
            code = None
        else:
            code = os.waitstatus_to_exitcode(status)
        os.close(fd)
        return out, code

class Picker(PtyEnv):
    def test_filter_paste_edit_esc_chain(self):
        s, code = self.drive([
            b"hyper",                                   # 0 typing filters
            b"\x1b[27u",                                # 1 Esc clears the query
            b"\x1b[200~contrast\x1b[201~",              # 2 bracketed paste (Cmd+V)
            b"\r",                                      # 3 Return → editor
            b"\x1b[97;9u7",                             # 4 Cmd+A + type → replaced
            b"\x03",                                    # 5 Ctrl+C in the editor: copy, no quit
            b"\x16",                                    # 6 Ctrl+V: pbpaste
            b"\x1b[27u",                                # 7 Esc closes the editor only
            b"\x1b[27u",                                # 8 Esc clears the search
            b"\x1b[27u",                                # 9 Esc quits
        ])
        self.assertEqual(s[0]["query"], "hyper")
        self.assertIn("app:all:Hyper+X", s[0]["rows"])
        self.assertEqual(s[1]["query"], "")
        self.assertEqual(s[2]["query"], "contrast")
        self.assertEqual(s[2]["selected"], "setting:screenshot.contrast-opacity")
        self.assertEqual(s[3]["editor"]["text"], "188")
        self.assertEqual(s[4]["editor"]["text"], "7")
        self.assertFalse(s[5]["quit"])
        with open(self.clip) as fh:
            self.assertEqual(fh.read(), "7")
        self.assertEqual(s[6]["editor"]["text"], "7pasted")
        self.assertIsNone(s[7]["editor"])
        self.assertEqual(s[7]["query"], "contrast")
        self.assertEqual(s[8]["query"], "")
        self.assertTrue(s[9]["quit"])
        self.assertEqual(code, 0)

    def test_edit_writes_and_mod_filter(self):
        before = sha(self.real)
        s, code = self.drive([
            b"contrast-op", b"\r", b"\x1b[97;9u200", b"\r",     # save 200
            b"\x1b[27u",                                       # clear
            b"\x1b[52;3u",                                     # Alt+4 = ⌘ only
            b"\x1b[27u", b"\x1b[27u",
        ])
        self.assertIsNone(s[3]["editor"], s[3])
        self.assertIn("changed", s[3]["status"])
        with open(self.real) as fh:
            self.assertIn("contrast-opacity = 200", fh.read())
        self.assertEqual(s[5]["mods"], ["cmd"])
        self.assertIn("app:all:Cmd+W", s[5]["rows"])
        self.assertTrue(all(r.startswith("app:") or r.startswith("aerospace:") for r in s[5]["rows"]))
        self.assertNotEqual(sha(self.real), before)


class PickerMouse(PtyEnv):
    """Clicks (SGR 1006): column headers sort, ⚠ toggle, wheel, double-click."""

    def test_clicks(self):
        def click(x, y):          # 0-based cell → SGR press + release
            return f"\x1b[<0;{x + 1};{y + 1}M\x1b[<0;{x + 1};{y + 1}m".encode()
        # 120 columns: toggles ⌃ ⌥ ⇧ ⌘ ★ ⚠ are 3 cells each, ending at x=118
        s, _ = self.drive([
            click(30, 3),                          # 0 "Keys" header → ▲
            click(30, 3),                          # 1 again → ▼
            click(30, 3),                          # 2 again → source order
            click(116, 0),                         # 3 ⚠ toggle → clashes first
            b"\x1b[<65;10;10M",                     # 4 wheel down
            click(60, 4) + click(60, 4),           # 5 double-click the first row → editor
            b"\x1b[27u", b"\x1b[27u", b"\x1b[27u",
        ])
        self.assertEqual(s[0]["sort"]["key"], ["keys", False])
        self.assertEqual(s[1]["sort"]["key"], ["keys", True])
        self.assertIsNone(s[2]["sort"]["key"])
        self.assertTrue(s[3]["clashFirst"])
        # the fixture's clashes (cmd-k vs files Cmd+K, alt-h vs herdr alt+h) lead the list
        top = s[3]["rows"][:4]
        self.assertTrue({"aerospace:main:cmd-k", "app:files:Cmd+K"} <= set(top), top)
        self.assertNotEqual(s[4]["selected"], s[3]["selected"])
        self.assertIsNotNone(s[5]["editor"], s[5])
        self.assertTrue(s[-1]["quit"])


# ---------------------------------------------------------------- live (opt-in)
REAL_BIN = os.path.join(ROOT, "workspace-switcher.app", "Contents", "MacOS", "workspace-switcher")


@unittest.skipUnless(os.access(REAL_BIN, os.X_OK), "app not built")
class RealBinary(unittest.TestCase):
    def test_config_check_and_schema(self):
        out = json.loads(subprocess.run([REAL_BIN, "config-check", "screenshot", "contrast-opacity", "999"],
                                        capture_output=True, text=True).stdout)
        self.assertFalse(out["ok"])
        self.assertIn("255", out["problem"])
        sc = json.loads(subprocess.run([REAL_BIN, "config-schema"], capture_output=True, text=True).stdout)
        self.assertEqual(sc["numberRanges"]["contrast-opacity"], [0, 255])
        out = json.loads(subprocess.run([REAL_BIN, "config-check", "--file", os.path.join(ROOT, "commands.toml")],
                                        capture_output=True, text=True).stdout)
        self.assertTrue(out["ok"], out)


@unittest.skipUnless(os.environ.get("WS_LIVE") == "1", "WS_LIVE=1 drives the running daemon")
class LiveDaemon(unittest.TestCase):
    """§9.6: set → reload → the daemon's state shows it; restored after."""

    def setUp(self):
        self.conf = os.path.realpath(os.path.join(ROOT, "commands.toml"))
        with open(self.conf, "rb") as fh:
            self.snapshot = fh.read()
        self.undo_log = os.path.expanduser("~/.cache/workspace-switcher/settings-undo.json")
        try:
            with open(self.undo_log, "rb") as fh:
                self.undo_snapshot = fh.read()
        except OSError:
            self.undo_snapshot = None

    def tearDown(self):
        with open(self.conf, "wb") as fh:
            fh.write(self.snapshot)
        if self.undo_snapshot is None:
            try:
                os.unlink(self.undo_log)
            except OSError:
                pass
        else:
            with open(self.undo_log, "wb") as fh:
                fh.write(self.undo_snapshot)
        subprocess.run([REAL_BIN, "reload"], capture_output=True)

    def state(self):
        import socket
        sock = os.path.join(os.environ.get("TMPDIR", "/tmp/"), "ws-notes.sock")
        s = socket.socket(socket.AF_UNIX)
        s.connect(sock)
        s.sendall(b"state\n")
        data = b""
        while True:
            d = s.recv(65536)
            if not d:
                break
            data += d
        return json.loads(data)

    def test_hide_on_focus_loss_round_trip(self):
        cur = self.state()["hideOnFocusLoss"]
        want = "false" if cur else "true"
        p = subprocess.run([os.path.join(ROOT, "bin", "ws-settings"), "set", "app.hide-on-focus-loss", want],
                           capture_output=True, text=True)
        self.assertEqual(p.returncode, 0, p.stderr)
        self.assertIn("re-read by the running app", p.stdout)
        self.assertEqual(self.state()["hideOnFocusLoss"], want == "true")


if __name__ == "__main__":
    unittest.main(verbosity=1)
