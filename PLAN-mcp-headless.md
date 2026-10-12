# PLAN — kitchen-sink MCP server + fully headless interaction

**Status:** **executed 2026-10-12 at `2554c6e`** (per `PROMPT-mcp-headless-impl.md` + its review addendum, which superseded §15): Lane A (`headless` in `rust/ws-rs/src/main.rs`) and Lane C (`bin/ks-mcp-sandbox.sh`) landed and passed their gates; after owner sign-off the **thin** MCP v1 landed (`bin/ks-mcp`/`bin/ks-mcp.py` with `ks_state`/`ks_do`/`ks_raw`, `Tests/test_ks_mcp.py` + `./ws test mcp`, `opencode.json` live-read-only/sandbox-disabled, AGENTS/README/BACKLOG/HANDOFF docs). The §7.2 full catalog (helper/jira/confluence/notify/settings), the L1/L2 suites and modern protocol support are **deferred** — see `BACKLOG.md` "MCP / headless control". Read **"UPDATE — 2026-10-11 re-audit"** (below) next: it supersedes several §1–§14 claims (raw socket verbs, isolation recipe, notify helpers, docs list, line anchors). Everything after that section is the original 608d07d snapshot — re-grep every `file:line` before editing.
**Provenance:** planning session 2026-10-10. Produced from repo research by a fleet of
DeepSeek V4.1 Flash subagents plus direct source reading. Every code claim below cites
`file:line` — line numbers drift, so re-grep the named symbol before editing.
**This session made no code changes.** The only artifact is this file. When it was
written the working tree was clean at commit `608d07d` with only this file untracked —
run `git status` before you start, and do not commit unless the owner asks.

---

## UPDATE — 2026-10-11 re-audit (HEAD `7b90dbd`, +45 commits since `608d07d`)

> **Provenance:** added by an orchestrated **read-only** audit — 8 DeepSeek V4.1 Flash research
> lanes (socket/raw verbs; host `do:` + state; per-view verbs; headless readiness; Python cold
> path; build/isolation/tests; docs + UI-element inventory; OpenCode/MCP registration) plus
> direct spot checks. **No code was touched.** This section supersedes the claims it corrects;
> everything after it is the original 608d07d snapshot. Re-grep every `file:line` before use.
> At audit time the tree was dirty (owner/parallel work — leave it alone): `bin/build-app.sh`,
> `commands.toml`, `rust/ws-rs/src/app/host.rs`, `rust/ws-rs/src/views/screenshot.rs` modified;
> `useability_hyperkey_x.md` untracked.

### U0. Macro-level

- The Swift app was deleted; the app is Rust + AppKit (`rust/`, cargo workspace) only. Every
  `*.swift` anchor in this plan (`kitchen_sink.swift:*`, `PopupWindow.swift`, `Screenshot.swift`,
  `CompareWindow.swift`) is dead, and so is `PROMPT-rust-port-phase4b.md`.
- Build tooling replaced: **`bin/build-rust-app.sh`, `.build/rust-app/`, `WS_RUST_KILL` are gone**;
  `bin/ui-test.sh` / `bin/ui-test-focus.py` deleted. `bin/rs-parity.py:59-77` is the surviving
  socket-client variant to copy. Current builder is `bin/build-app.sh` (target `.build/rust-target`,
  repo-root `kitchen-sink.app`, `--dist` → `.build/dist`; a non-dist build `pkill`s the running app,
  `build-app.sh:184`).
- **Nothing from this plan is implemented yet:** no `bin/ks-mcp*`, no `Tests/*ks_mcp*`, no `mcp`
  block in `opencode.json`, no `headless`/`WS_HEADLESS` in `main.rs` (grep-verified).

### U1. Socket contract (§1) — all raw verbs now reply in Rust

- `RAW_VERBS = reload, restart, screenshot, screenshot-permission, compare, pane-shot`
  (`socket.rs:31-38`). Dispatch order: `ping` (`socket.rs:259-261`) → `state` (263-266) →
  `do:` (268-274) → raw verbs (`raw_deferred` `host.rs:2519-2534`; `raw_request`
  `host.rs:2536-2542`) → launch/hotkey message (`socket.rs:293-295`).
- The four "Swift only / no reply" rows in §1 are **now false** — replace them:
  - `screenshot-permission` → `granted`/`denied` (`host.rs:2521-2527`)
  - `screenshot[\t…]` → `--raw` raw PNG bytes to EOF; `-g` → `"W H X Y"`; interactive defers
    (`host.rs:1836-1873`; `Reply::send_bytes` `socket.rs:59-62`)
  - `compare\t…` → `--wait` deferred `done`; else silent (`host.rs:1878-1928`)
  - `pane-shot[\t…]` → `path` / `copied` / `error: …` (`host.rs:1933-1993`)
- **Late replies exist now** (§1 modeled raw verbs as synchronous strings only): `Reply` is a
  dup'd fd answerable later from any thread (`socket.rs:40-69`) — that is what powers
  `compare --wait` and async screenshot/pane-shot.
- Read cap is still **2048** bytes per request (`socket.rs:28`); accepted fds are forced blocking
  with `SO_SNDTIMEO=5s`, so **replies >8 KB are safe** (200 KB `state` test, `socket.rs:200-245`,
  `:549-558`). Keep MCP requests short; replies unbounded.
- Unknown `do:` **still returns full state** (`socket.rs:268-274`) — the parity gap stands; the MCP
  server cannot detect typos. Optionally validate view/verb shape client-side where cheap.
- No auth beyond filesystem perms; lock file `<sock>.lock` is `flock`ed (`socket.rs:326-357`).
- **Consequence for §7.2:** implement `ks_screenshot`, `ks_compare`, `ks_pane_shot`,
  `ks_screenshot_permission` as real tools against the sandbox; delete the
  "no reply from Rust → isError" mapping. Keep the "daemon down" actionable error.

### U2. Isolation recipe (§2) — corrected, verified against today's tree

```bash
ROOT=/Users/danielbaker/.config/kitchen-sink
S=$ROOT/.build/mcp-sandbox
# Build, live-safe (no pkill; mirrors build-app.sh's cargo invocation):
( cd "$ROOT/rust" && CARGO_TARGET_DIR="$ROOT/.build/rust-target" \
    cargo build --release -p ws-rs -p ws-helpers )
# Launch candidate — today this is still a GUI daemon (headless mode not implemented):
mkdir -p "$S/tmp" "$S/home"
[ -f "$S/home/commands.toml" ] || cp "$ROOT/commands.toml" "$S/home/commands.toml"
TMPDIR="$S/tmp" WS_HOME="$S/home" WS_COMMANDS_CONF="$S/home/commands.toml" \
WS_RS_ASSET_DIR="$ROOT" WS_SKIP_LAUNCH_AGENT=1 PYTHONDONTWRITEBYTECODE=1 \
  "$ROOT/.build/rust-target/release/kitchen-sink" show >"$S/daemon.log" 2>&1 &
echo $! > "$S/pid"
printf 'state\n' | nc -U -w3 "$S/tmp/ws-notes.sock" | jq .pid
# Stop only by pid/socket — never pkill:
kill "$(cat "$S/pid")"    # or: kill "$(lsof -t "$S/tmp/ws-notes.sock")"
```

- `WS_HOME` / `WS_COMMANDS_CONF` **do** isolate config now (`config.rs:1543-1558`) — the
  "repo build reads repo `commands.toml` regardless" caveat is stale. `Paths::user_dir()`
  (AI rules dir, `paths.ignore`) stays repo-bound.
- `[app] notes-socket` is parsed (`config.rs:470-472`) but **ignored** by the socket path
  (`paths.rs` hardcodes `NOTES_SOCKET_NAME`) — isolate via `TMPDIR` only.
- **NEW env `WS_SKIP_LAUNCH_AGENT=1`** (`jira_poll.rs:175`): must be set or an isolated daemon can
  write the real `~/Library/LaunchAgents/com.jira.poll.plist`.
- Not isolated by `TMPDIR`/`WS_HOME`: `~/.cache/kitchen-sink`, `~/.cache/jira`,
  `~/.cache/notifications`, Jira legacy-config migration → also set `JIRA_CONFIG_JSON`,
  `JIRA_CONFIG_FILE`, `JIRA_TEAM_JSON`, `JIRA_CACHE_DIR`, `NOTIFY_CACHE_DIR`,
  `WS_SETTINGS_CACHE` where relevant.
- `WS_PING_ONLY`, `WS_RS_ASSET_DIR`, `WS_PYTHON` still work. `sun_path` hard-fails at ≥104 bytes
  (`socket.rs:145`); the sandbox path above is 76 — keep `KS_MCP_SANDBOX` shallow.
- A direct shell spawn runs under the shell's TCC identity (fine headless; use `open -n -g` only
  when Screen Recording grants matter).

### U3. Headless readiness (§3, §8) — corrected line map

- `ui_queue: Option<UiQueue>` (`host.rs:239`, `None` at `:610`); `ui_push` drops when `None`
  (`host.rs:299-303`); host test helper `fn controller()` at `:9484`, `no_ui_queue_stays_headless`
  at **`:9982`** (36 `#[test]`s in host.rs). No `todo!()`/`unimplemented!()` calls remain, and
  **AGENTS.md no longer has a "Remaining seams" bullet** — drop §12's "fix the stale seams" item.
- Forwarding is **already correct**: the lock retry loop moved to `app/cli.rs:193-215`; when the
  lock is held, `open_command` stays `None` → `msg = "ping"` (`cli.rs:199`). Do NOT special-case
  `headless` there. `main.rs:73` binds the returned `DaemonLock`.
- `run_daemon` is now **`main.rs:107-151`**. Keep: 108-111 (controller + `register_views` — verified
  AppKit-free) and 113-119 (`CommandServer::start`). Skip: 121-124 (`MainThreadMarker`/`NSApplication`),
  127 (`setActivationPolicy`), 128 (`install_main_menu`), 129 (`install_status_item`), 131-137
  (`UiQueue`/`MainQueue`/`install_ui`/`install_daemon_ui`), 139-147 (launch presentation — never pass
  `"headless"` to `launch_with_prep`). Replace `app.run()` (`:150`) with
  `loop { std::thread::park(); }`; keep `_server` and the caller's `_lock` in scope — the accept
  thread is detached, so a returned `main` exits the process.
- Mode parsing is `main.rs:24-62` (the new `prose` one-shot at 41-59 builds its own NSApplication;
  irrelevant here). Add `headless`/`--headless` + `WS_HEADLESS=1` (no argv mode) detection here and
  thread a bool into `run_daemon`.
- Headless risk to smoke-test first: `screenshot:*` (`host.rs:1562-1576`) and raw `screenshot`
  now run inline on the socket thread when there is no main queue (`run_on_main` inline,
  `host.rs:883-892`); overlay `NSPanel`s are guarded by `MainThreadMarker::new()` but that path is
  unverified headless. `compare_message` is pure model (safe), `pane-shot` is CG off-main,
  `screenshot-permission` reads CG state. `ensure_home` (`main.rs:66`) is AppKit-free (may run
  `setup-home.sh` for non-repo bundles — point `WS_HOME` at the sandbox). SIGTERM shutdown is fine;
  a stale socket is unlinked on the next bind (`socket.rs:141`).
- `open:<path>` is drained now when the GUI presents notes (`host.rs:8691-8697`); still undrained
  headless — keep the §9 route-around.

### U4. `do:` / `state` deltas (§1, §3)

- New/changed host verbs: `hide-on-focus-loss:on|off` (`host.rs:1676-1684`, NEW);
  `jira-poll-on|off|setup` (`:1786-1789`, NEW); registry stub `notes:state` (`notes.rs:3701-3709`,
  NEW); `tool:`/`tool-close:` real (ToolKind panels, `host.rs:1685-1726`); `reset-size` pushes
  `UiCommand::ResetSize` (`:1790-1794`; model no-op headless); `rebuild-card:VIEW` is
  validation-only (`:1534-1546`); `notes-find`/`notes-grep`/`jira-jump` push real `UiCommand`s
  (`:1777-1785`, dropped headless); `board:*` still errors `{"error":"unhandled action …"}`
  (`:1800-1806`); unknown `do:` still echoes state.
- Registry namespaces actually registered: **`notes:`, `compare:`, `screenshot:` only**
  (`registry.rs:260-274`; call sites `host.rs:687-711`). Jira registers a palette row only
  (`jira.rs:3084`). `ai.rs:1532` / `confluence.rs:1077` have `register()` functions that are
  **never called** → `do:ai:*` / `do:confluence:*` are unreachable dead code (a cheap wiring win
  if MCP must drive those views; otherwise note out-of-scope).
- All 22 top-level state keys unchanged. Real rich per-view state only for `views.notes` /
  `views.files` (`host.rs:8548-8561`); `views.jira.board` stays `{}` placeholder (the real
  `JiraView::test_state` `jira.rs:3069` is never pushed); confluence/ai model `test_state`s are not
  wired. GUI-only defaults headless (do not assert): `windows`, `keyWindow`, `active`,
  `frontmostPid`, `activations`, `viewSwitcher.selection`, compare overlay keys, `tools.*` frames.
  `state.screenshot` is real and includes `modePill`; the recents grid is not represented.

### U5. UI element → control entry; coverage gaps (feeds §7.2)

Covered today (spot-check): all 11 views via `do:open:VIEW`; terminal/drawers; tool panels
(`tool:`/`tool-close:`); paths shelf; palette (launch `show`, `state.palette/paletteCommands`);
view switcher; compare (full text + folder verb set incl.
`folder-copy|folder-move:SIDE:[:replace|keep|skip]`); screenshot verb set + `mode:` + `key:tab`;
jira setup (`do:jira-setup`); pins (`screenshot:pin|unpin`).

**GAPS — no programmatic entry today** (headless/MCP cannot drive these; decide scope):

- Right-click menus: files, notes editor, compare (pane/tree/session), jira column header,
  screenshot pin, recent grid.
- Find bars: notes native find, prose search, jira live search.
- Cmd+K action pickers (files/jira/compare/AI); Cmd+/ shortcuts sheets (`ui/shortcuts.rs` exists,
  no hook).
- Jira `board:*` (list/sidebar/tick/pin — errors today) and Fit-Columns/column menus.
- Confluence in-view keys (favorite/copy link/find/refresh); AI run-rule & find; files in-view ops
  (rename/QuickLook/reveal/hidden — model is reachable via helper `fileops.*`); notes editor ops
  (partial).
- Screenshot internals: ring-button clicks, colour wheel, Tool Settings side-panel contents,
  in-place text editor, save-card field, Cmd+/ card, cursor/grab colour, CLI `full`/`screen`,
  `-d` delay, `--region`, post-capture preview; pin menu beyond pin/unpin; **recents grid**
  (`key:tab` only, no state field).
- pane-shot preview; prose own-window (Cmd+Shift+O); FilePopup; Setup & Health window actions;
  theme menu extras (only `header-style:STYLE` is reachable).

`do:key:SPEC` remains **record-only** (`host.rs:1611-1614`) ⇒ every key-only surface above has no
headless entry. `useability_hyperkey_x.md` (untracked) is the current screenshot-parity truth;
`FEATURES.md`'s "ring buttons have no glyphs" was fixed (`e854b74`).

### U6. Python cold path (§4, §7.3) — mostly unchanged; notify rebuilt

- **Hash-identical to 608d07d** (line refs stay valid): `pylib/helper/*`, `settings_hub/{paths,cli}.py`,
  `pylib/jira_paths.py`, `jira/jira_{poll,status}.py`, `confluence/confluence_{api,config}.py`.
- Helper registry = **150 methods + `ping` = 151**, 18 namespaces incl. `jsonmgr` and `script`
  (plan's "~110" was already an undercount). No new methods since the plan.
- The app **never calls `script.run`** (Swift-only claim); it direct-spawns the CLIs
  (`jira_poll.rs:52-84`, `views/jira.rs:2461`, `views/confluence.rs:2206`).
- **notify changed:** `notify/helpers/*.swift` deleted; helpers are prebuilt Rust bins found via
  `WS_HELPERS_BIN` (**a directory**) › bundle `Resources/helpers-bin` › `.build/rust-target/release`
  (`notify_poll.py:80-92`). Delete the "first `--json` may compile Swift helpers (300 s)" note.
  `notify_poll.py` with no args prints its docstring and exits **2** (`:270-271`).
- Exit codes: add **130** (jira cancel; settings `KeyboardInterrupt`). `ws-settings` verbs:
  `keys settings get set undo bind conflicts export parity doctor fav tui open`; `--json` on
  keys/settings/get/conflicts/parity/doctor; env adds `WS_SETTINGS_FAVORITES`.
- jira flags are a superset of §4's list (`jira_poll.py:250-339`: add `--window`, `--projects`,
  `--import-filters`, `--favorite*`, `--pin-*`, `--board-*`, `--cached`, …); confluence adds
  `--detect-auth`, `--add-space`, `--remove-space`, `--import-saved`.
- **Discovery mismatch to guard:** helper (`python_helper.rs:231`) and confluence
  (`views/confluence.rs:2227`) prefer `WS_PYTHON`; jira's direct spawn tries brew pythons first
  (`jira_poll.rs:52-58`) → `WS_PYTHON` can be ignored for jira spawns on brew machines.
- New wrappable script: `bin/parity-audit.sh` (writes `.build/parity-audit.md` — not read-only;
  candidate `ks_parity` tool or leave out of read-only mode).

### U7. MCP / OpenCode registration (§5, §12)

- §12 is un-implemented: `opencode.json` currently has only `$schema` + `permission` — no `mcp`
  block anywhere (global configs too). The §12 block can be added as written, with one fix:
  **use an absolute `command[0]`** (`/Users/danielbaker/.config/kitchen-sink/bin/ks-mcp`) —
  cwd-relative executable resolution is undocumented (unverified in OpenCode v2.0.26).
- §5.2 claims re-verified against OpenCode v2.0.26 + docs: `mcp.servers` (not names under `mcp`),
  `disabled` (no `enabled`), `codemode` default true, `protocol` default `legacy`
  (`legacy|auto|2026-07-28`), `{env:}` substitution, timeouts `{startup 30s, catalog 30s,
  execution 12h}`, tools exposed as `<server>_<tool>`, Code Mode `tools.<server>.<tool>`.
  stdout = protocol only; stderr captured separately (binary-verified).
- Decision to (re)confirm when registering: the live entry is read-write by default; consider
  `"environment": {"KS_MCP_READ_ONLY": "1"}` on the live server and leave the sandbox read-write.
  (V2 also accepts a `permissions` array; the v1 object form is still normalized — no change needed.)

### U8. Tests / runner (§11)

- `bin/run-tests.sh` cases today: `config, recent, fileops, status, ai, nvim, filter, paths, ignore,
  jsonmgr, screenshot, ansi, ansi-parse, paneshot, prose, prose-pdf, snippets, panes, vim-keys,
  compare, settings, jira-data, jira-pages, jira-search, jira-directory, jira-dashboard, jira-boards,
  jira-fields, jira-setup, confluence-glue, confluence-pages, setup-checks, shot-model, shelf,
  helper, helper-client, doc-templates, rust, all|*`. `all` = 4 Python suites + the whole rust
  workspace (`run-tests.sh:171-180`); `ui|vim|focus` die in `lib.sh`.
- Adding `mcp` / `mcp-sandbox` / `mcp-data` cases follows the existing idiom unchanged. Note
  `run-tests.sh` does **not** export `PYTHONDONTWRITEBYTECODE` — new suites must invoke python
  with `-B` themselves.
- Two cargo target dirs coexist: `rust/target` (test runner) vs `.build/rust-target` (app build) —
  don't run both cargo builds concurrently.
- L0/L2 suites are unblocked; **L1 (real sandbox) is blocked on §8 + §10**.

### U9. Docs list correction (§12)

§12 names `AGENTS.md`; it also needs `README.md` (docs map + `bin/ks-mcp` / `kitchen-sink headless`),
`HANDOFF-rust-port-next.md` (new control plane + gaps), `BACKLOG.md` (§9 deferrals), and a review of
`PRODUCT.md`, `FEATURES.md`, `useability_hyperkey_x.md`. Drop the "Remaining seams" fix (no such
bullet exists).

### U10. Issues to resolve before implementation

1. **Headless mode is the prerequisite** — §8 must land before `bin/ks-mcp-sandbox.sh` can work as
   specced (today an isolated instance is a full GUI daemon with a hidden window).
2. **All raw verbs now reply** — refresh §1/§3/§9/§7.2 before coding tools; this is the biggest
   correctness risk to the MCP server spec.
3. Smoke-test `screenshot:*` / interactive raw `screenshot` headless (off-main overlay path
   unverified) and guard if it faults.
4. `board:*` errors; key-only surfaces (Cmd+K, Cmd+/, find bars) have no entry — document as
   coverage limits or extend later (§9-style deferral).
5. `do:ai:*` / `do:confluence:*` are dead code — wire the registers or declare out of scope.
6. `WS_PYTHON` ordering mismatch for jira spawns — pin/verify in sandbox tests.
7. Sandbox env checklist: `TMPDIR` (socket isolation), `WS_HOME` + `WS_COMMANDS_CONF` (config),
   `WS_RS_ASSET_DIR` (assets), `WS_SKIP_LAUNCH_AGENT=1` (Jira LaunchAgent), `sun_path` ≤ 103 bytes.
8. `opencode.json`: absolute launcher path; validate JSON before saving; owner commits.
9. Tree is dirty with parallel work — during implementation: no `./ws build` (kills the live app),
   serialize file writers, never mutate the live socket in tests (§13 unchanged).

**Inline amendments applied (2026-10-11):** §1 (transport prose, raw-verb table rows, dispatch
note), §2 (recipe + env table + config caveat), §7.2 (the four raw-verb tool rows), §9
(table + future-work paragraph), §10 (`start` steps 1-3), §14.1 (build/smoke lines). **Still to
amend at implementation time:** §3/§8 (corrected line map lives in U3; markers added),
§11.5 (`-B` note), §12 (docs list above). Execution kickoff: `PROMPT-mcp-headless-impl.md`.

---

## 0. Mission

Build an MCP server that exposes the kitchen-sink app's **complete** control surface so a
model (OpenCode, Claude, any MCP client) can drive the app **completely headlessly** —
no synthetic input, no computer control, no touching the owner's live session during
tests. Two halves:

1. **`bin/ks-mcp` + `bin/ks-mcp.py`** — a zero-dependency Python MCP server (stdio)
   wrapping (a) the daemon's Unix-socket grammar and (b) the Python cold path
   (helper protocol, settings, jira, confluence, notify).
2. **Rust `headless` daemon mode + `bin/ks-mcp-sandbox.sh`** — a real daemon with real
   `state`/`do:` semantics that never creates a window, status item, menu, or Dock icon,
   bound to an isolated socket so the live app is untouched.

### Locked decisions (owner answered; do not re-litigate)

| Decision | Choice |
|---|---|
| Server implementation | Zero-dep Python, stdio JSON-RPC, repo-style plain script |
| Real-daemon sandbox | Add `headless` mode to the **Rust** daemon (`rust/ws-rs`), isolated via `TMPDIR` + `WS_HOME` |
| Rust headless no-ops | Route around via CLI/helper tools in v1; Rust parity fixes deferred (§9) |
| `ks_do` default | Read-write; `--read-only`/`KS_MCP_READ_ONLY=1` hides mutating tools |
| Tool surface | Curated daemon + data-plane tools + generic `ks_helper` escape hatch |
| Registration | Both servers in repo `opencode.json`: live + sandbox (sandbox `disabled: true`) |

---

## 1. Ground truth — socket contract

Transport: Unix socket at `$TMPDIR/ws-notes.sock` (name hardcoded by `paths.rs`;
`[app] notes-socket` is parsed (`config.rs:470-472`) but **not** consulted by the socket
path). One newline-terminated request per connection; single `read()` cap 2048 bytes
(`rust/ws-rs/src/app/socket.rs:28`). Dispatch order (`socket.rs:247-296`):

| Request | Reply |
|---|---|
| `ping` | **none** — server closes; treat clean close as success |
| `screenshot-permission` | `granted\n` \| `denied\n` (`host.rs:2521-2527`) |
| `screenshot[\t<ShotArgs>]` | `-r/--raw` → raw PNG bytes (read to EOF); `-g/--print-geometry` → `"W H X Y\n"`; interactive overlays defer the reply; other args close silently (`host.rs:1836-1873`) |
| `compare\t[--wait][--title1 T][--title2 T] LEFT [RIGHT]` | `--wait` → deferred `done\n`; else silent close (`host.rs:1878-1928`) |
| `pane-shot[\t<args>]` | `path\n` \| `copied\n` \| `error: msg\n` (`host.rs:1933-1993`) |
| `reload` | `{"ok","commands","usingBackup","issues":[{line,message,fatal}]}\n` |
| `restart` | `{"ok":true,"restarting":true}\n` |
| `state` | full state JSON + `\n` |
| `do:ACTION` | action error JSON, else the **same full state JSON** + `\n` |
| anything else | launch/hotkey message; **no reply** |

All six raw verbs are implemented in Rust: `RAW_VERBS = reload, restart, screenshot,
screenshot-permission, compare, pane-shot` (`socket.rs:31-38`); late replies use a
dup'd-fd `Reply` (`socket.rs:40-69`) — dispatch: `raw_deferred` (`host.rs:2519-2534`)
then `raw_request` (`host.rs:2536-2542`). Replies are safe beyond 8 KB (accepted fds are
forced blocking with `SO_SNDTIMEO=5s`, `socket.rs:200-245`; 200 KB state test `:549-558`).
Unknown `do:` returns the full state with no error — the MCP server cannot detect typos
on `ks_do`; document, don't fix in v1 (`socket.rs:268-274`).

**Canonical Python socket client** (copy from `bin/rs-parity.py:58-76`):

```python
def query(sock_path, msg, timeout=3.0):
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(timeout)
    try:
        s.connect(sock_path)
        s.sendall((msg + "\n").encode())
        buf = b""
        while not buf.endswith(b"\n"):
            chunk = s.recv(65536)
            if not chunk:
                break
            buf += chunk
        return json.loads(buf.decode() or "null")
    except (FileNotFoundError, ConnectionRefusedError, socket.timeout, OSError):
        return None
    finally:
        s.close()
```

Variants to reuse: `bin/ui-test.sh:73-85` (`ws_query/ws_state/ws_do/wait_state` — polls
every 50 ms), `bin/ui-test-focus.py:77-93`, `Tests/test_settings_hub.py:845-857`.
Shell smoke: `printf 'state\n' | nc -U -w3 "$TMPDIR/ws-notes.sock"`.

### `do:` action families (dispatch: Rust `host.rs:1524-1821` + `registry.dispatch_test_do`
`registry.rs:267-274`)

- Shared window: `cycle`, `cycle-back`, `hide`, `back`, `home`, `toggle`,
  `open:VIEW` (VIEW ∈ `notes,files,jira,detail,releases,config,output,confluence,ai,compare,compareText`;
  bad view → error), `reset-size` (Rust no-op)
- Panels/tools: `toggle-terminal`, `toggle-browser` (Rust only), `term`,
  `tool:NAME`, `tool-close:NAME`, `notes-find`, `notes-grep` (Rust no-op),
  `jira-jump` (Rust no-op), `switcher`, `switcher-hide`
- Paths shelf: `paths:show|hide|return|select:N`
- Config/keys: `esc-hides:VIEW:on|off`, `header-style:STYLE`, `key:SPEC`
  (Rust records `last_key` only)
- Pane nav: `pane:h|j|k|l|focus:ID`
- Card rebuild: `rebuild-card:VIEW` (Rust no-op)
- Sub-verb families (first `Some` wins in Rust):
  - `board:*` — jira: `open:B, list, list-open:B, list-star:B, view:V, sprint:S, mode:M, pin`
    (**Rust errors** — `host.rs:1800-1806`; jira registers a palette row only, `jira.rs:3084`)
  - `compare:*` — `open:LEFT|RIGHT`, `open-sub:…`, `next`, `prev`, `copy-right`,
    `copy-left`, `swap`, `reload`, `undo`, `redo`, `start`, `start-paste`,
    `edit-begin`, `edit-commit`, `filter:all|diffs|same|context`, `save:left|right`,
    `select:N`, `close-session[:force]`, `close-all`, `align:L,R`, `align-clear`,
    `trim:left|right`, `eol:left|right:lf|crlf|cr`, `whitespace:on`,
    `sheet-cancel`, `paste:SIDE:TEXT`, `edit:SIDE:TEXT`, `cursor:N`, `key:SPEC`,
    `folder-*` (`CompareWindow.swift:2628-2704`, `CompareFolderView.swift:1323-1343`)
  - `screenshot:*` — `show[:MS]`, `show-text[:MS]`, `select:X,Y,W,H`, `tool:NAME|none`,
    `draw:X1,Y1,X2,Y2`, `key:SPEC`, `copy`, `text`, `mode:text|screenshot`, `accept`,
    `pin`, `save:PATH`, `save-ok`, `close`, `unpin`, `side-panel`
    (`Screenshot.swift:905-957`)

### `state` JSON keys (Rust `state_with_registry` `host.rs:452-550`, augmented by
`state_json` `host.rs:2457-2505`)

Top level: `view, visible, active, keyWindow, windows, palette, paletteCommands,
views, hideOnFocusLoss, escHides, pid, terminalPanel, viewSwitcher, paths,
headerStyle, pane, screenshot, compare, paneShot, activations, frontmostPid, tools`.
Per-view `views.<name>`: `name, shown, key, frame, drawerInset, terminal, browser,
findBar, accessory, pane, tabs, selectedTab, selection, rowCount, query, board,
sidebarCursor, responder, header` + `wid`. The rich extras are injected for
notes/files only; jira keeps a `board: {}` placeholder; `windows`, `keyWindow`,
`active`, `frontmostPid`, `activations`, `viewSwitcher.selection` and `tools.*`
frames are GUI-only and stay default headless.

**GUI-only fields that stay default headless** (do not assert on them in headless tests):
`windows`, `keyWindow`, `frontmostPid`, `activations`, palette rows,
`viewSwitcher.rows`, `terminalPanel.wid/frame`, `tools.*.wid`.

---

## 2. Ground truth — isolation recipe (proven in-repo)

Proven in-repo recipe, updated 2026-10-11 (the old `PROMPT-rust-port-phase4b.md` version
is gone — UPDATE U2 has the full corrected recipe):

```bash
( cd rust && CARGO_TARGET_DIR=../.build/rust-target cargo build --release -p ws-rs -p ws-helpers )
D=$(mktemp -d)
TMPDIR="$D" WS_RS_ASSET_DIR="$PWD" WS_SKIP_LAUNCH_AGENT=1 \
  .build/rust-target/release/kitchen-sink show &   # until §8 lands this is a GUI daemon
printf 'state\n' | nc -U "$D/ws-notes.sock" | jq
```

| Env var | Effect | Evidence |
|---|---|---|
| `TMPDIR=$D` | Socket + `.lock` + focus files live under it → full isolation | `paths.rs:18-26,184-190` |
| `WS_HOME=$D/home` | Config home (`WS_COMMANDS_CONF` takes precedence) — **honored even by the repo build now** | `config.rs:1543-1558`; `paths.rs:121-125` |
| `WS_RS_ASSET_DIR=$PWD` | Points helper at checkout `pylib` (dev bundle has no Resources) | `main.rs:98-104`, `paths.rs:107-119` |
| `WS_SKIP_LAUNCH_AGENT=1` | Keeps an isolated daemon from writing the real Jira LaunchAgent — **add to every sandbox env** | `jira_poll.rs:175` |
| `WS_PING_ONLY=1 <bin> [mode]` | Liveness probe, exit 0/1 | `main.swift:27-30` |
| `WS_PYTHON` | Python ≥3.11 override for helper/CLI discovery | `bin/ws-settings:11-20`, `run-tests.sh:157-164` |
| `PYTHONDONTWRITEBYTECODE=1` / `python -B` | **Always** — `__pycache__` in a signed bundle breaks its signature | `AGENTS.md`, `bin/ws-settings:22-24` |

**Config caveat (updated 2026-10-11):** config resolution is env-driven —
`WS_COMMANDS_CONF` › `WS_HOME` › `~/.config/kitchen-sink` (`config.rs:1543-1558`),
so `WS_HOME`/`WS_COMMANDS_CONF` isolate config even for the repo build. `Paths::user_dir()`
(AI rules dir, `paths.ignore`) stays repo-bound. `[app] notes-socket` is ignored by the
socket path, so isolate the socket via `TMPDIR`. Prefer the raw cargo binary
`.build/rust-target/release/kitchen-sink` for sandbox instances (not a repo bundle ⇒
`ensure_home` stays a no-op).

Kill an isolated daemon **by pid** (`lsof -U | grep "$D/ws-notes.sock"` or a pidfile) —
never `pkill -f kitchen-sink`, which can hit the live app.

---

## 3. Ground truth — Rust headless readiness

> *(Line anchors in this section are the 608d07d snapshot — use **UPDATE U3** for the corrected map. Intent unchanged.)*

The controller is already headless-capable: `ui_queue: Option<UiQueue>` (`host.rs:201-202`),
`ui_push` silently drops when `None` (`host.rs:246-250`), and ~40 cargo tests run the
controller with no UI (`host.rs:2651`, `no_ui_queue_stays_headless` `host.rs:2980-2984`).
There are **no real `todo!()` invocations left** (AGENTS.md's "Remaining seams" bullet is
stale). Socket dispatch runs on the accept thread (`socket.rs:63,132-153`) and
`SwitcherController` is thread-safe (`Mutex` inner).

**Headless: works now** — `state`/`state_with_registry`, all model-level `do:` verbs,
`reload`/`restart`, registry verbs (`notes:`/`compare:`/`screenshot:`), PythonHelper
(pure std, `python_helper.rs:152-181`), hotkey prep (shells out to aerospace),
`paths:*` model, screenshot model + capture (CG, not AppKit UI).

**Headless no-ops / gaps** (route around in v1; §9 lists deferred fixes):
`board:*` errors; `key:` records only; `jira-jump`/`notes-find`/`notes-grep`/
`reset-size`/`rebuild-card` no-ops; `open:<path>` sets `pending_note_open` but nothing
drains it; window-presentation verbs flip model state with no surface; raw
`screenshot`/`compare`/`pane-shot`/`screenshot-permission` have no reply; GUI-only state
fields stay default.

**`run_daemon` wiring** (`rust/ws-rs/src/main.rs:107-149`) — a headless branch must skip
everything from `NSApplication` (line 121) through `app.run()` (line 148), i.e. skip:
`setActivationPolicy` (127), `install_main_menu` (128), `install_status_item` (129),
`install_ui` (134), `install_daemon_ui` (135), and the launch presentation (141-145).
Process liveness: nothing but `app.run()` keeps `main` alive — the headless branch must
block (e.g. `loop { std::thread::park(); }`). The `DaemonLock` guard (`main.rs:78-88`,
`socket.rs:254-285`) is held in the match arm for the process lifetime. Forwarding when
the lock is held: `main.rs:82` sends `mode.unwrap_or("ping")` — special-case `headless`
to forward `ping` instead of a nonsense launch message.

---

## 4. Ground truth — Python cold path invocation recipes

(Researched from source; all verified.)

### 4.1 Helper (persistent JSON-lines worker)

```bash
<python> -B -m helper            # cwd=<repo>/pylib ; PYTHONPATH=<repo>/pylib
```
Spawn mirrors `PythonHelper.swift:136-146` / `python_helper.rs:153-167`; interpreter
discovery prefers `WS_PYTHON` then brew pythons, requires ≥3.11
(`PythonHelper.swift:241-260`, `python_helper.rs:229-252`).

Protocol (`pylib/helper/__main__.py`, `pylib/helper/__init__.py`):
- Request: `{"id": <int>, "method": "<str>", "params": {…}}` — one compact JSON line.
- Reply ok: `{"id", "ok": true, "result": <any>}`;
  error: `{"id", "ok": false, "error": {"message", "traceback"?}}`.
- Replies are matched by **id** (out-of-order fine); 8-worker pool (`__main__.py:15,42`);
  writes are locked+flushed (`__main__.py:101-109`).
- `shutdown` method → `{"id","ok":true,"result":{}}`, drains, exits (`__main__.py:54-63`).
- One-shot modes: `--version`, `--call NAME [--params JSON]`, `--once`
  (`__main__.py:20-38`).
- `script.run` params `{folder, script, args[], stdin, timeout}` →
  `{"code","stdout","stderr"}` (`pylib/helper/script_bridge.py:15-47`); the app does NOT
  call it (direct spawns: `jira_poll.rs:52-84`) — it works, but is app-untested.
- Method registry: `pylib/helper/methods.py` (~110 methods across
  compare/folder/ai/fileops/status/ignore/doc_templates/prose/config/jira/confluence/
  shot/ansi/paneshot/shelf/setup; `ping` in `__init__.py:49-51`).
- Default app-side call timeout 10 s (+5 s grace) (`python_helper.rs:72-73`) — the MCP
  server should use its own, larger timeouts and **never auto-retry** non-idempotent
  calls (`fileops.*` retry-on-restart is known app-side debt, `AGENTS.md`).

### 4.2 ws-settings (`settings_hub`)

```bash
<python> -B -m settings_hub <verb> [args]     # PYTHONPATH=<repo root>
# or the wrapper: bin/ws-settings <verb> …
```
- Path resolution: `home()` = `$WS_HOME` or `~/.config/kitchen-sink` (`paths.py:22-23`);
  `commands_conf()` = `$WS_COMMANDS_CONF` › repo `commands.toml` when repo build ›
  `$home/commands.toml` (`paths.py:26-35`). **In repo mode, `set`/`undo`/`bind` edit the
  repo `commands.toml`** — tests must set `WS_HOME` + `WS_COMMANDS_CONF` to a temp copy.
- JSON: `keys|settings|get|conflicts|parity|doctor --json`, `export json`
  (`cli.py:65-278`). `set/undo/bind` print text. Exit 0/1/2 (`cli.py:1`).
- `set` applies via the app binary unless `--no-apply`; `jira.enabled` special-cased to
  `kitchen-sink jira-poll on|off` (`cli.py:125-135,142-168`).
- `cache_dir()` = `$WS_SETTINGS_CACHE` or `~/.cache/kitchen-sink` (`paths.py:131-134`);
  `app_binary()` = `$WS_SETTINGS_BIN` or `…/kitchen-sink.app/Contents/MacOS/kitchen-sink`
  (`paths.py:120-128`) — point `WS_SETTINGS_BIN` at the sandbox binary in tests.

### 4.3 jira

```bash
<python> -B <repo>/jira/jira_poll.py <flags…>       # self-adds dir + pylib
<python> -B <repo>/jira/jira_status.py [--json|--path|--note-error MSG]
```
- Env: `JIRA_CONFIG_JSON`, `JIRA_TEAM_JSON`, `JIRA_CONFIG_FILE`, `JIRA_CACHE_DIR`
  (`pylib/jira_paths.py:5-6,18-21`); credentials from `JIRA_SITE/JIRA_EMAIL/JIRA_TOKEN`;
  `[jira]` section from `WS_COMMANDS_CONF` or repo `commands.toml`
  (`jira_config.py:80`).
- Offline-safe: `--dry-run` (text), `--describe` (JSON), `--board-catalog --cached`
  (JSON), `--label-view NAME`, `--release-view [KEY…]`, `jira_status.py --json`
  (0 if non-empty else 1). Network: default tick, `--init/--rebuild/--setup/--directory/
  --sprints/--my-work/--live-search` (stdin JSON) etc. Exit 0/1/2/3 (`jira_poll.py:132-133`).

### 4.4 confluence

```bash
<python> -B <repo>/confluence/confluence_api.py <cmd> [args]   # stdin JSON for some
```
- Config: `$CONFLUENCE_CONFIG_JSON` › `commands.toml [confluence] config` ›
  `~/.config/confluence/config.json` (`confluence_config.py:94-99`); env
  `CONFLUENCE_SITE/EMAIL/TOKEN`; cache `$CONFLUENCE_CACHE_DIR` or `~/.cache/confluence`.
- One JSON object on stdout always; exit 0/1/2. Offline: `--check`, `--save`,
  `--favorites --no-refresh`; network: `--myself/--search/--page/--users/--favorite/…`
  (`confluence_api.py:509-547`).

### 4.5 notify

```bash
<python> -B <repo>/notify/notify_poll.py --json            # {"sources":[…]}
<python> -B <repo>/notify/notify_poll.py --poll [--print]
```
- `[notifications]` from `WS_COMMANDS_CONF`/repo config; cache `$NOTIFY_CACHE_DIR` or
  `~/.cache/notifications`; disabled section → `{"sources":[]}` exit 0, fast
  (`notify_poll.py:249-252`). First `--json` may compile Swift helpers (up to 300 s) —
  tests should set `[notifications] enabled = false` in a temp config.

---

## 5. Ground truth — MCP protocol + OpenCode

### 5.1 Protocol eras (verified against modelcontextprotocol.io, Oct 2026)

- Current revision: **`2026-07-28`** (stateless: no handshake; per-request `_meta` with
  `io.modelcontextprotocol/protocolVersion`; mandatory `server/discover`; results carry
  `resultType:"complete"`; `tools/list` needs `ttlMs`+`cacheScope`).
- Last handshake revision: **`2025-11-25`** (legacy). Earlier: `2025-06-18`,
  `2025-03-26`, `2024-11-05`.
- **OpenCode V2 defaults to legacy** (`protocol` field default `"legacy"`; `"auto"`
  probes `server/discover`, `"2026-07-28"` requires modern). → **v1 implements legacy;
  modern is an optional stretch (§12 phase 6).**

Legacy essentials to implement exactly:
- `initialize` request → result `{protocolVersion, capabilities:{tools:{listChanged:false}},
  serverInfo:{name,version}, instructions?}`. Echo the client's `protocolVersion` if in
  our supported list, else reply with our latest supported (`2025-11-25`).
- `notifications/initialized` — no reply.
- `ping` → `{"result":{}}`.
- `tools/list` → `{tools:[…]}`, no `nextCursor` needed.
- `tools/call` → `{content:[…], isError?:true}`; `content` items: `{type:"text",text}`,
  `{type:"image",data:<base64>,mimeType:"image/png"}`.
- Protocol errors use JSON-RPC `error` (`-32700` parse, `-32600` invalid request,
  `-32601` method not found, `-32602` invalid params, `-32603` internal); **tool
  execution failures** use a normal result with `isError:true` and actionable text.
- Unknown notifications: ignore (no reply). Unknown requests: `-32601`.
- Tool annotations: `readOnlyHint`, `destructiveHint`, `idempotentHint`, `openWorldHint`
  (all optional; treat as hints).

### 5.2 OpenCode V2 config (exact schema — `mcp.servers`, NOT names directly under `mcp`)

Fields for a local server: `type:"local"` (required), `command:[…]` (required), `cwd`,
`environment`, **`disabled`** (there is no `enabled`), `codemode` (default `true`),
`timeout` (`startup` 30 s, `catalog` 30 s, `execution` 12 h defaults), `protocol`
(`legacy` default). OpenCode names tools `<server>_<tool>` (non-alphanumerics → `_`);
Code Mode exposes them as `tools.<server>.<tool>(…)`. `{env:NAME}` substitution is
supported; shell `$NAME` is not. Manage with `opencode mcp list` / `/mcps`.

---

## 6. Deliverables & file ownership

**New files**
| File | Purpose | Owner lane |
|---|---|---|
| `bin/ks-mcp` | bash launcher: Python ≥3.11 discovery (mirror `bin/ws-settings:8-24`), `exec "$py" -B "$here/ks-mcp.py" "$@"` | B |
| `bin/ks-mcp.py` | the MCP server (zero-dep stdlib) | B |
| `bin/ks-mcp-sandbox.sh` | start/stop/status/sock/reset for the isolated headless daemon | C |
| `Tests/ks_mcp_util.py` | shared test helpers (FakeDaemon, MCPClient, socket_query) | D |
| `Tests/test_ks_mcp.py` | L0 fake-daemon protocol suite | D |
| `Tests/test_ks_mcp_sandbox.py` | L1 real headless daemon suite | D |
| `Tests/test_ks_mcp_data.py` | L2 data-plane suite | D |

**Modified files**
| File | Change | Owner lane |
|---|---|---|
| `rust/ws-rs/src/main.rs` | `headless` mode branch | A |
| `bin/run-tests.sh` | cases `mcp`, `mcp-sandbox`, `mcp-data` | E (serialize) |
| `opencode.json` | `mcp.servers` block (live + sandbox) | E (serialize) |
| `AGENTS.md` | MCP/headless/sandbox docs; fix stale seams bullet | E (serialize) |

---

## 7. Component spec — `bin/ks-mcp.py` (+ `bin/ks-mcp`)

### 7.1 Process & transport

- stdio, newline-delimited JSON-RPC 2.0, UTF-8. **stdout = protocol only**; all logs go
  to stderr. EOF on stdin → best-effort helper `shutdown`, exit 0.
- CLI/env: `--socket PATH` › `KS_SOCKET` › `$TMPDIR/ws-notes.sock`; `--read-only` ›
  `KS_MCP_READ_ONLY=1`; `--repo PATH` (default = parent of the script's dir);
  `--list-tools` (debug: print tool names, exit).
- Single-threaded request loop in v1 (document; long `--wait`/`script.run` calls block
  other calls).
- Legacy handshake implemented per §5.1; `ping` answered.

### 7.2 Tool catalog (frozen interface for tests)

Annotations: `RO`=readOnlyHint true; `MUT`=not read-only. Read-only mode hides every MUT
tool (keeps `ks_ping`, `ks_state`, `ks_screenshot_permission`).

| Tool | Arguments (JSON Schema) | Sends / does | Returns | Mode |
|---|---|---|---|---|
| `ks_ping` | `{}` | `ping` | text `"ok"` (clean close = alive) | RO |
| `ks_state` | `{}` | `state` | state JSON text | RO |
| `ks_do` | `{action: string}` req | `do:<action>` | state JSON text; `{"error":…}` reply → `isError` | MUT |
| `ks_launch` | `{message: string}` req | launch message | text `"ok"` (no reply expected) | MUT |
| `ks_reload` | `{}` | `reload` | reload JSON text | MUT |
| `ks_restart` | `{}` | `restart` | restart JSON text | MUT |
| `ks_screenshot` | `{args?: string}` | `screenshot[\t<args>]` | `-r` → image content (base64 PNG; read the reply to EOF); `-g` → `"W H X Y"` text; else `"ok"` — interactive forms may defer, so treat a clean close/no reply as `"ok"`; data calls (`-r`/`-g`) return `isError` on missing data | MUT |
| `ks_compare` | `{left, right?, wait?, title1?, title2?, timeout_ms?}` | `compare\t…` | `--wait` → `"done"` (deferred until the session closes/hides — use the long timeout); else `"ok"` | MUT |
| `ks_pane_shot` | `{args: string}` | `pane-shot\t<args>` | `path`/`copied`/`error: …` text | MUT |
| `ks_screenshot_permission` | `{}` | `screenshot-permission` | `granted`/`denied` | RO |
| `ks_helper` | `{method: string, params?: object, timeout_ms?}` | persistent helper worker (§7.3) | helper `result` JSON text; `ok:false` → `isError` (message + traceback excerpt) | MUT |
| `ks_settings` | `{argv: string[]}` | `python -B -m settings_hub <argv>` (PYTHONPATH=repo) | `{code, stdout, stderr}` JSON text | MUT |
| `ks_jira` | `{script?: "poll"\|"status", argv: string[]}` | `jira/jira_poll.py` / `jira/jira_status.py` | `{code, stdout, stderr}` | MUT |
| `ks_confluence` | `{argv: string[], stdin?: string}` | `confluence/confluence_api.py` | `{code, stdout, stderr}` | MUT |
| `ks_notify` | `{argv?: string[]}` (default `["--json"]`) | `notify/notify_poll.py` | `{code, stdout, stderr}` | MUT |

**Implementation notes**
- `ks_compare`: mirror the exact wire format implemented in `compare_message`
  (`host.rs:1878-1928`) and the `cli.rs` compare client — read them first; paths are
  tab-separated, optional `--wait`/`--title1`/`--title2`. Lock the byte format with an L0
  recording test.
- `ks_screenshot -r`: read the reply **to EOF** (binary), not to newline. Base64 it into
  an image content block. `-g`: read one line.
- `ks_do`/`ks_reload` replies are one JSON line; `ks_launch` may produce no reply.
- Timeouts (ms): socket default 5000; `ks_do` 10000 (Swift runs `do:` on main with a 2 s
  semaphore, `AGENTS.md`); `ks_compare --wait` default 300000, cap 1800000; `ks_pane_shot`
  10000; `ks_helper` default 15000, cap 1800000 (script.run); CLI wrappers default 60000,
  cap 600000; `ks_notify` default 300000 (first run may compile Swift helpers).
- Errors: daemon down → `isError` text like
  `kitchen-sink daemon not answering at <path> (start the app or use bin/ks-mcp-sandbox.sh)`.
  Never crash the server on a tool failure.
- Truncate CLI `stdout`/`stderr` over ~100 000 chars with a marker (state JSON can be
  large; pass it through untruncated up to ~1 MB).

### 7.3 Helper worker (`ks_helper`)

- Lazy spawn on first call: `[sys.executable, "-B", "-m", "helper"]` with
  `cwd=<repo>/pylib`, env `PYTHONPATH=<repo>/pylib`,
  `PYTHONDONTWRITEBYTECODE=1`, brew-prefixed PATH.
- Reader thread matches replies by id; monotonic id counter; per-call timeout; on
  EOF/crash mark dead and respawn on the next call (report the restart in the tool
  result). Best-effort `shutdown` on server exit. **No automatic retries.**

### 7.4 Read-only mode

Hides all MUT tools from `tools/list` and rejects them with `-32602` if called anyway
(belt and braces). Intended for "look but don't touch" sessions against the live app.

---

## 8. Component spec — Rust headless mode (`rust/ws-rs/src/main.rs` only)

> *(Steps 1-3 anchors are the 608d07d snapshot — UPDATE U3 has the corrected line map; the intent is unchanged. Forwarding (step 2's original concern) is already handled in `cli.rs` — do not special-case it there.)*

1. **Mode parsing** (`main.rs:25-33`): accept `headless` and `--headless`; if
   `WS_HEADLESS=1` is set and no CLI mode was given, treat mode as `headless`.
2. **Forwarding** (`main.rs:78-88`): when the lock is held and `mode == "headless"`,
   forward `ping` (not `headless`) and exit.
3. **`run_daemon`** (`main.rs:107-149`): after `CommandServer::start` succeeds
   (lines 113-119), if headless:
   ```rust
   eprintln!("kitchen-sink (rust) headless daemon: socket {sock}");
   loop { std::thread::park(); }
   ```
   and return — skipping lines 121-148 entirely (NSApplication, activation policy, menu,
   status item, `install_ui`, `install_daemon_ui`, launch presentation, `app.run()`).
4. **No other files change.** Keep `configure_helper()` (`main.rs:72,93-105`) as-is.
5. Shutdown: plain `SIGTERM` is fine — a stale socket file is unlinked by the next
   `bind_listener` (`socket.rs:95-96`); no signal handler required (optional polish).
6. Verify `cd rust && cargo test` stays green (the flag changes nothing when unset).

**Result:** a daemon that answers `state`/`do:`/`reload`/`restart` with real model
semantics, creates no AppKit objects at all, has no Dock/status/menu presence, and never
shows a window. Document the no-op list (§3) in the script/docstring.

---

## 9. Deferred Rust parity fixes (v1 routes around these — do NOT do them now)

| Gap | v1 route-around | Evidence |
|---|---|---|
| `board:*` errors | `ks_jira` CLI (`--board-*`, `--pin-*`, `--import-filters`, `--my-work`) + `--describe` JSON | `host.rs:1800-1806`; jira registers a palette row only (`jira.rs:3084`) |
| `open:<path>` on headless | On the GUI it drains when notes is presented (`host.rs:8691-8697`); headless still undrained — use `ks_helper` `fileops`/notes data or accept no-op | `take_pending_note_open` `host.rs:962` |
| `reset-size`, `rebuild-card:*`, `jira-jump`, `notes-find`, `notes-grep` | All are wired as `UiCommand`s now, but have no headless surface — avoid headless, document | `host.rs:1534-1546,1777-1794` |
| `key:SPEC` records only | Use `do:` verbs instead of key synthesis | `host.rs:1611-1614` |
| `do:ai:*` / `do:confluence:*` unreachable | Their `register()` fns exist but are never called — wire them, or declare out of scope | `ai.rs:1532`, `confluence.rs:1077` (uncalled) |
| unknown `do:` returns state | Cannot detect; document | `socket.rs:268-274` |

Future work (separate from MCP): register jira `test_do` for `board:*`, drain
`pending_note_open` headless, and wire (or delete) the `ai:`/`confluence:` registers.
Track in `BACKLOG.md`.

---

## 10. Component spec — `bin/ks-mcp-sandbox.sh`

Subcommands: `start | stop | status | sock | reset`. Config:
`SANDBOX="${KS_MCP_SANDBOX:-$ROOT/.build/mcp-sandbox}"` (`.build/` is gitignored).

- `start`:
  1. Build if needed with the current toolchain (live-safe — never run `./ws build`,
     which `pkill`s the live app):
     `( cd "$ROOT/rust" && CARGO_TARGET_DIR="$ROOT/.build/rust-target" cargo build --release -p ws-rs -p ws-helpers )`.
  2. `mkdir -p "$SANDBOX/tmp" "$SANDBOX/home"`; if `home/commands.toml` missing, copy
     the repo `commands.toml` (sandbox config edits never touch the repo copy).
  3. Launch (headless flag per §8 + UPDATE U3; fall back to arg `show` + a warning
     until the flag exists on the binary):
     ```bash
     BIN="$ROOT/.build/rust-target/release/kitchen-sink"
     TMPDIR="$SANDBOX/tmp" WS_HOME="$SANDBOX/home" \
     WS_COMMANDS_CONF="$SANDBOX/home/commands.toml" \
     WS_RS_ASSET_DIR="$ROOT" WS_SKIP_LAUNCH_AGENT=1 \
     WS_HEADLESS=1 PYTHONDONTWRITEBYTECODE=1 \
       nohup "$BIN" headless >"$SANDBOX/daemon.log" 2>&1 &
     echo $! > "$SANDBOX/pid"
     ```
  4. Poll the socket (`ping` via python one-liner or `nc -U -w1`) up to ~10 s; fail with
     the log tail.
- `stop`: read pidfile; verify the process cmdline contains `rust-app` (safety); `kill
  -TERM`, wait ≤5 s, `kill -KILL`; remove `pid`, socket, `ws-notes.sock.lock`.
- `status`: pid alive + socket answers `ping` → `running (pid N, sock PATH)`; else `stopped`.
- `sock`: print `$SANDBOX/tmp/ws-notes.sock` (for `export KS_SOCKET=$(…)`).
- `reset`: `stop`; remove `home/` (fresh copy from repo on next `start`).
- **Never** `pkill -f kitchen-sink`; kill only the pidfile pid.

Sun-path limit: `sun_path` must be < 104 bytes — `$ROOT/.build/mcp-sandbox/tmp/ws-notes.sock`
is ~66; fine. `mktemp -d` paths are also fine.

---

## 11. Tests

### 11.1 `Tests/ks_mcp_util.py` (shared, not a suite)

- `python_bin()` — brew python ≥3.11 (mirror `bin/run-tests.sh:157-164`).
- `socket_query(path, msg, timeout)` — copy of `bin/rs-parity.py:58-76`.
- `FakeDaemon` — thread + `AF_UNIX` listener; per-request scriptable replies; records
  raw request lines; supports binary replies (PNG) and no-reply verbs.
- `MCPClient` — spawns `bin/ks-mcp` as a subprocess with env overrides; sends/receives
  newline JSON-RPC; helpers: `initialize()`, `list_tools()`, `call(name, args)`;
  validates the legacy handshake.

### 11.2 L0 — `Tests/test_ks_mcp.py` (fake daemon; no app; target <5 s)

- handshake: `initialize` → protocolVersion echoed; `notifications/initialized` ignored;
  `ping` → `{}`.
- `tools/list`: exact tool-name set; each `inputSchema` is a valid object schema;
  read-only mode hides the MUT set.
- wire bytes: `ks_state` sends exactly `state\n`; `ks_do {"action":"toggle-terminal"}`
  sends exactly `do:toggle-terminal\n`; `ks_launch` sends the raw message.
- results: state JSON text; `{"error":…}` daemon reply → `isError:true`;
  daemon-down (nonexistent socket) → `isError` with the actionable message, server stays up.
- `ks_screenshot -r`: fake returns PNG bytes → result contains an `image` content block
  (`mimeType: "image/png"`); `-g`: geometry text.
- `ks_compare`: recorded wire line matches the `compare_message` grammar
  (read `host.rs:1878-1928` and lock it in); `--wait` reads `done`.
- `ks_ping`: no-reply clean close → `"ok"`.
- protocol robustness: unknown method → `-32601`; malformed JSON → `-32700`; unknown
  notification → ignored.
- read-only mode: MUT tool call → error.

### 11.3 L1 — `Tests/test_ks_mcp_sandbox.py` (real headless daemon)

- `setUpClass`: `bin/ks-mcp-sandbox.sh start` (builds if stale); `tearDownClass`: `stop`
  (only if we started it).
- `ks_state` equals a direct `nc`/socket `state`.
- `ks_do open:notes` → `views.notes.shown == true`; `ks_do hide` → false;
  `ks_do toggle-terminal` flips `views.notes.terminal`; `ks_do cycle` changes `view`.
- `ks_reload` returns `{"ok": …}`.
- isolation: sandbox `pid` ≠ live `pid` (when the live daemon is running); sandbox socket
  path is the sandbox one; `windows == 0` and `activations == 0` (headless sanity).
- Swift-only tools (`ks_compare`, `ks_pane_shot`, `ks_screenshot -r`,
  `ks_screenshot_permission`) → clean `isError` "unsupported by this daemon" (no hang).
- `ks_do` unknown action on Rust returns full state (documented parity gap — assert
  current behavior so a future parity fix is noticed).

### 11.4 L2 — `Tests/test_ks_mcp_data.py` (cold path; daemon not required)

Env for every test: temp `WS_HOME` + `WS_COMMANDS_CONF` pointing at a copied
`commands.toml` (repo copy untouched), `PYTHONDONTWRITEBYTECODE=1`, plus per-tool
caches (`JIRA_CACHE_DIR`, `NOTIFY_CACHE_DIR`, `WS_SETTINGS_CACHE`).

- `ks_settings`: `get app notes-socket --json`; `set` a throwaway key then `undo`;
  assert only the temp config changed.
- `ks_helper`: `fileops.stack_new` → `create` → `stack_state` → `undo` round trip in a
  temp dir; `config.line`/`decode` on a temp toml; **persistent handle** survives across
  two calls (stateful engine proof).
- `ks_jira`: seed a temp `status.json` → `jira_status.py --json` exit 0 JSON;
  `--describe` JSON parse.
- `ks_confluence`: `--check` (offline) → JSON parses, exit 0/1 acceptable.
- `ks_notify`: with `[notifications] enabled = false` in the temp config → `{"sources":[]}`.
- helper restart: kill the worker mid-test (optional) → next call reports a restart.

### 11.5 `bin/run-tests.sh`

Add three case arms mirroring the existing pattern (`run-tests.sh:27-175`):

```bash
mcp)
    py=/opt/homebrew/bin/python3; [ -x "$py" ] || py=python3
    "$py" "$TESTS/test_ks_mcp.py"
    ;;
mcp-sandbox)
    py=/opt/homebrew/bin/python3; [ -x "$py" ] || py=python3
    "$py" "$TESTS/test_ks_mcp_sandbox.py"
    ;;
mcp-data)
    py=/opt/homebrew/bin/python3; [ -x "$py" ] || py=python3
    "$py" "$TESTS/test_ks_mcp_data.py"
    ;;
```

Leave the `all` case unchanged (it runs three hard-coded Python suites + every Swift
test, `run-tests.sh:176-186`); the new suites run via their own cases.

---

## 12. Registration + docs

### `opencode.json` (project config, V2 shape — `mcp.servers`)

```jsonc
{
  "$schema": "https://opencode.ai/config.json",
  "permission": { /* … existing, unchanged … */ },
  "mcp": {
    "servers": {
      "kitchen-sink": {
        "type": "local",
        "command": ["bin/ks-mcp"],
        "cwd": "."
      },
      "kitchen-sink-sandbox": {
        "type": "local",
        "command": ["bin/ks-mcp", "--socket", ".build/mcp-sandbox/tmp/ws-notes.sock"],
        "cwd": ".",
        "disabled": true
      }
    }
  }
}
```

- Verify OpenCode resolves `command[0]` relative to `cwd`; if not, use the absolute
  repo path for the launcher (this repo always lives at
  `/Users/danielbaker/.config/kitchen-sink`).
- Keep `codemode` at its default (`true`); tools surface as
  `tools.kitchen-sink.ks_state(…)`.
- `protocol` stays default (`legacy`); switch the sandbox entry to `auto` only if the
  modern stretch (§12 phase 6) lands.
- Verify with `opencode mcp list` (expect `✓ kitchen-sink connected`, sandbox disabled).
- **Do not** run `opencode mcp auth` for these (stdio, no OAuth).

### `AGENTS.md` edits (implementation time)

- New section "MCP server / headless control": tools, `bin/ks-mcp-sandbox.sh`, test cases,
  read-only mode, "never drive mutations against the live socket in tests".
- Build/run section: mention `kitchen-sink headless` + `WS_HEADLESS=1`.
- Rust port section: replace the stale "Remaining seams" list (no real `todo!()`s remain)
  with the current headless no-op list (§3) or a pointer to this plan.
- Gotchas: add `sun_path` limit, headless `state` GUI-only fields, Swift-only raw verbs.

---

## 13. Risks & gotchas (read before writing code)

1. **Never mutate the live socket in tests.** All `do:` mutations go to the sandbox.
   Live checks are read-only (`state`, `ping`) only (`PROMPT-rust-port-phase4b.md:72`).
2. **Git state:** when this plan was written the tree was clean at `608d07d` (the Rust
   port handoff commit) with only this file untracked. Repo convention: the owner
   commits. Add new files; edit only the planned files; do not stage or commit anything
   unless the owner asks.
3. **No `./ws build`** (kills/relaunches the live Swift app) and **no UI suites**
   (`./ws test ui|vim|focus|install`). Only Rust builds + the new headless tests.
4. **Build live-safe:** raw cargo into `.build/rust-target`; `./ws build` kills and
   relaunches the running app — only `bin/build-app.sh --dist` skips the pkill.
5. **`PYTHONDONTWRITEBYTECODE=1` / `-B` for every Python spawn** (signed-bundle
   signature).
6. **`sun_path` < 104 bytes** — keep sandbox sockets shallow.
7. **Swift `do:` runs on the main thread with a 2 s semaphore** — generous client
   timeouts, but don't stack slow calls; Rust dispatch is thread-safe.
8. **`compare --wait` can block for minutes** — long timeout, documented; `restart`
   against the live app restarts it (read-only mode hides it).
9. **Helper non-idempotent retries** — never auto-retry `fileops.*`/mutations.
10. **Screenshot capture needs Screen Recording TCC** — tests must not capture real
    screens; use model verbs (`select`/`draw`/`copy` state) only.
11. **OpenCode default protocol is `legacy`** — don't over-engineer modern support in v1.
12. **Rust `do:` unknown action returns state** — `ks_do` can't detect typos on the
    sandbox; validate view names client-side where cheap.
13. **`borders` daemon / focus:** irrelevant headless; don't touch.
14. **`nc -U -w3` may exit non-zero on clean close** — treat "connect + no bytes" as
    success in shell probes.

---

## 14. Verification checklist (run at the end, in order)

```bash
# 1. Rust headless smoke (no GUI: no window/status item should appear)
( cd rust && CARGO_TARGET_DIR=../.build/rust-target cargo build --release -p ws-rs -p ws-helpers )
D=$(mktemp -d)
TMPDIR="$D" WS_RS_ASSET_DIR="$PWD" WS_SKIP_LAUNCH_AGENT=1 \
  .build/rust-target/release/kitchen-sink headless &
P=$!
printf 'state\n'   | nc -U -w3 "$D/ws-notes.sock" | jq -r .pid
echo do:toggle-terminal | nc -U -w3 "$D/ws-notes.sock" | jq .views.notes.terminal
echo do:open:notes      | nc -U -w3 "$D/ws-notes.sock" | jq .views.notes.shown
kill $P

# 2. Sandbox lifecycle
bin/ks-mcp-sandbox.sh start && bin/ks-mcp-sandbox.sh status && bin/ks-mcp-sandbox.sh sock
bin/ks-mcp-sandbox.sh stop

# 3. Suites
./ws test mcp && ./ws test mcp-data && ./ws test mcp-sandbox

# 4. Live read-only smoke (harmless; never do: mutations here)
printf 'state\n' | nc -U -w3 "$TMPDIR/ws-notes.sock" | jq -r .pid

# 5. Rust unit tests still green
cd rust && cargo test && cd ..

# 6. OpenCode sees both servers (after registration)
opencode mcp list
```

Definition of done:
- [ ] `kitchen-sink headless` answers `state`/`do:` with zero GUI objects.
- [ ] `bin/ks-mcp` serves legacy MCP with the §7.2 tool set; read-only mode verified.
- [ ] L0/L1/L2 suites green via `./ws test …`; live socket never mutated by tests.
- [ ] `opencode.json` registers both servers; sandbox disabled by default.
- [ ] `AGENTS.md` updated; stale seams note fixed.
- [ ] `git status` shows only the intended files; nothing committed.

---

## 15. Execution guide for the next agent

> *(Superseded by `PROMPT-mcp-headless-impl.md` — written after the 2026-10-11 audit; updated lanes, corrected recipes and copy-paste prompts. Keep this section only for the model/variant guidance in §15.1.)*

### 15.1 Model

- **Model ID:** `opencode-go/deepseek-v4.1-flash` (provider `opencode-go`, "DeepSeek V4.1
  Flash"). Variants exist: `low`, `high`, `max` — pass as
  `"opencode-go/deepseek-v4.1-flash#high"` when using the `subagent` tool.
- Suggested: `#low` for mechanical edits/tests, default or `#high` for the MCP protocol
  server and the Rust branch, `#max` for tricky debugging (protocol handshake, sandbox
  lifecycle races). Do not switch to other models unless the owner asks.
- The `subagent` tool starts **fresh context** per child: every prompt must be
  self-contained — repo path, the section of this plan to read, files owned, gates,
  and hard rules (read-only research vs implementation, no commits, no UI suites,
  no live-socket mutations, `PYTHONDONTWRITEBYTECODE=1`).

### 15.2 Parallelization plan (file-ownership lanes)

Interfaces are frozen in this document, so lanes can run concurrently **as long as no
two lanes write the same file**.

| Phase | Lane | Owns | Gate |
|---|---|---|---|
| 1 | A — Rust headless | `rust/ws-rs/src/main.rs` | nc smoke (§14.1) + `cd rust && cargo test` |
| 2 | B — MCP server | `bin/ks-mcp`, `bin/ks-mcp.py` | L0 suite green + manual fake-daemon call |
| 2 | C — sandbox script | `bin/ks-mcp-sandbox.sh` | start/status/stop + nc smoke |
| 3 | D — tests | `Tests/ks_mcp_util.py`, `Tests/test_ks_mcp*.py` | all three suites green |
| 4 | E — registration/docs | `opencode.json`, `AGENTS.md`, `bin/run-tests.sh` | `opencode mcp list` + `./ws test mcp*` |
| 5 | Parent — verification | none (read-only) | full §14 checklist |

- **Serialize:** `bin/run-tests.sh`, `opencode.json`, `AGENTS.md`, this plan file. Lane E
  runs alone, last (before final verification).
- **Parallelize:** A and C can start immediately (C's interface is frozen: subcommands +
  socket path). B can start immediately; D can be written against the frozen §7.2 tool
  catalog and run once B lands.
- **Suggested concurrency:** 3–4 subagents max; give each a `description` and a prompt
  that names the lane, the files it may touch, and the gate command. Use background mode
  only for lanes with no shared files.
- **Anti-patterns:** two writers on one file; subagents running `./ws build`, UI suites,
  or `cargo` in the wrong target dir concurrently; subagents committing; subagents
  driving `do:` against the live socket; research prompts that forget "read-only".

### 15.3 Prompt templates

Implementer (Lane A):

> Repo `/Users/danielbaker/.config/kitchen-sink`. Read `AGENTS.md` and
> `PLAN-mcp-headless.md` §3 + §8. Implement the Rust `headless` mode exactly as
> specified in §8 (only `rust/ws-rs/src/main.rs`). Do not touch other files, do not
> commit, do not run `./ws build` or UI suites; build live-safe (raw cargo into
> `.build/rust-target`). Gate: the §14.1
> nc smoke commands and `cd rust && cargo test`. Report the diff summary and gate output.

Implementer (Lane B):

> … Read §1, §5, §7. Implement `bin/ks-mcp` + `bin/ks-mcp.py` (only these files). The
> tool catalog in §7.2 is frozen — do not rename tools or change schemas. Gate: run
> `python3 Tests/test_ks_mcp.py` once Lane D lands, and a manual fake-daemon check.
> …

Tester (Lane D):

> … Read §11. Implement `Tests/ks_mcp_util.py`, `Tests/test_ks_mcp.py`,
> `Tests/test_ks_mcp_sandbox.py`, `Tests/test_ks_mcp_data.py` (only these files). Do not
> modify the server or scripts; if a test fails because of a real bug, report it instead
> of patching the implementation. Gate: `./ws test mcp`, `./ws test mcp-data`,
> `./ws test mcp-sandbox`.

### 15.4 Context hygiene

- Don't paste this whole plan into prompts — point at sections by number.
- Re-grep every `file:line` anchor before editing (they drift).
- After each phase, update this plan's status header (one line) so the next reader knows
  where execution stopped.

---

## 16. Open questions for the owner (decide before or during phase 4)

1. Server names: `kitchen-sink` / `kitchen-sink-sandbox` (tool names become
   `kitchen-sink_ks_state`, …). Keep the `ks_` prefix or drop it?
2. Code Mode on (default) or `codemode: false` to expose tools directly?
3. Enable the sandbox server entry by default, or keep `disabled: true` until wanted?
4. Modern MCP protocol support (`protocol: auto` + `server/discover`, `2026-07-28`) — do
   it now as phase 6, or defer until a client requires it?
5. `ks_notify`/`ks_jira` network calls: leave timeouts as specced, or make the owner
   explicitly opt in to network tools?

---

## Appendix A — exact JSON-RPC shapes to implement (legacy)

Initialize response:

```json
{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-11-25","capabilities":{"tools":{"listChanged":false}},"serverInfo":{"name":"kitchen-sink","version":"0.1.0"},"instructions":"Drive the kitchen-sink daemon and its Python cold path. Prefer ks_state before ks_do. Mutations against the live socket affect the running app; use the sandbox for tests."}}
```

Tool entry example:

```json
{"name":"ks_do","description":"Execute a kitchen-sink `do:ACTION` verb and return the resulting state JSON.","inputSchema":{"type":"object","properties":{"action":{"type":"string","description":"Action verb, e.g. open:notes, hide, cycle, toggle-terminal, pane:h"}},"required":["action"]},"annotations":{"readOnlyHint":false,"destructiveHint":false,"openWorldHint":false}}
```

Tool call result (text):

```json
{"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"{\"view\":\"notes\",…}"}],"isError":false}}
```

Tool call result (image):

```json
{"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":"screenshot captured"},{"type":"image","data":"iVBORw0KGgo…","mimeType":"image/png"}]}}
```

Tool execution error:

```json
{"jsonrpc":"2.0","id":4,"result":{"content":[{"type":"text","text":"kitchen-sink daemon not answering at /var/folders/…/ws-notes.sock (start the app or use bin/ks-mcp-sandbox.sh)"}],"isError":true}}
```

Protocol error:

```json
{"jsonrpc":"2.0","id":5,"error":{"code":-32601,"message":"Method not found: tools/bogus"}}
```

Helper request/reply:

```json
{"id":7,"method":"fileops.stack_new","params":{}}
{"id":7,"ok":true,"result":{"handle":"…"}}
```

`reload` reply:

```json
{"ok":true,"commands":"/Users/danielbaker/.config/kitchen-sink/commands.toml","usingBackup":false,"issues":[]}
```

## Appendix B — research provenance

Produced 2026-10-10 from: 7 parallel read-only DeepSeek V4.1 Flash research agents
(product spec, socket contract, Python cold path, Rust headless readiness, doc
inventory, feature PRDs, test/interaction patterns) + 2 more (CLI invocation recipes,
MCP protocol spec) + direct reads of `rust/ws-rs/src/main.rs`, `app/socket.rs`,
`app/host.rs`, `app/paths.rs`, `app/registry.rs`, `pylib/helper/*`, `bin/ws-settings`,
`bin/build-rust-app.sh`, `PROMPT-rust-port-phase4b.md`, OpenCode V2 docs
(`opencode.ai/v2/docs/mcp-servers`), and modelcontextprotocol.io versioning. Key
surprises worth remembering: OpenCode V2 config is `mcp.servers` with `disabled` (not
`enabled`) and defaults to the legacy handshake; MCP current revision `2026-07-28` is
stateless; the Rust controller already runs headless in tests; Rust raw verbs only
support `reload`/`restart`.
