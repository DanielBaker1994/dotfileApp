# PLAN — kitchen-sink MCP server + fully headless interaction

**Status:** proposed, NOT started.
**Provenance:** planning session 2026-10-10. Produced from repo research by a fleet of
DeepSeek V4.1 Flash subagents plus direct source reading. Every code claim below cites
`file:line` — line numbers drift, so re-grep the named symbol before editing.
**This session made no code changes.** The only artifact is this file. When it was
written the working tree was clean at commit `608d07d` with only this file untracked —
run `git status` before you start, and do not commit unless the owner asks.

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

Transport: Unix socket at `$TMPDIR/<notes-socket>` (default `ws-notes.sock`);
`[app] notes-socket` in `commands.toml:27`; path built by
`popupTmpDir() + settings.notesSocketName` (`kitchen_sink.swift:355,398,3165`;
`popupTmpDir()` = `$TMPDIR` + trailing slash, `PopupWindow.swift:302-306`).
One newline-terminated request per connection; read cap 2048 bytes
(`rust/ws-rs/src/app/socket.rs:27,181-224`). Dispatch order (`kitchen_sink.swift:3164-3330`,
Rust `socket.rs:181-224`):

| Request | Reply |
|---|---|
| `ping` | **none** — server closes; treat clean close as success |
| `screenshot-permission` | `granted\n` \| `denied\n` (Swift only; Rust: no reply) |
| `screenshot[\t<ShotArgs>]` | `-r/--raw` → raw PNG bytes (read to EOF); `-g/--print-geometry` → `"W H X Y\n"`; else no reply (Swift only; Rust: no reply) |
| `compare\t[--wait][--title1 T][--title2 T] LEFT [RIGHT]` | `--wait` → `done\n`; else no reply (Swift only; Rust: no reply) |
| `pane-shot[\t<args>]` | `path\n` \| `copied\n` \| `error: msg\n` (Swift only; Rust: no reply) |
| `reload` | `{"ok","commands","usingBackup","issues":[{line,message,fatal}]}\n` |
| `restart` | `{"ok":true,"restarting":true}\n` |
| `state` | full state JSON + `\n` |
| `do:ACTION` | action error JSON, else the **same full state JSON** + `\n` |
| anything else | launch/hotkey message; **no reply** |

Rust `raw_request` implements only `reload`/`restart` (`host.rs:1694-1700`) — every
other raw verb silently returns nothing. Rust unknown `do:` returns full state, Swift
returns `{"error":"unknown action …"}` (`kitchen_sink.swift:3107`) — parity gap; the MCP
server cannot detect unknown actions on the Rust daemon (document, don't fix in v1).

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

### `do:` action families (dispatch: Swift `kitchen_sink.swift:3013-3108`; Rust
`host.rs:1172-1381` + `registry.dispatch_test_do` `registry.rs:267-274`)

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
    (Swift `kitchen_sink.swift:8771-8784`; **Rust errors** — jira registers no `test_do`)
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

### `state` JSON keys (Swift `testQuery` `kitchen_sink.swift:3110-3162`; Rust
`state_with_registry` `host.rs:384-477`)

Top level: `view, visible, active, keyWindow, windows, palette, paletteCommands,
views, hideOnFocusLoss, escHides, pid, terminalPanel, viewSwitcher, paths,
headerStyle, pane, screenshot, compare, paneShot, activations, frontmostPid, tools`.
Per-view `views.<name>` (Swift `PopupWindow.testState` `PopupWindow.swift:10484-10501`):
`name, shown, key, frame, drawerInset, terminal, browser, findBar, accessory, pane,
tabs, selectedTab, selection, rowCount, query, board, sidebarCursor, responder, header`
+ `wid` (added by `testQuery`). Jira `board` extra: `kitchen_sink.swift:8786-8797`.
Rust emits `shown,key,wid,frame` + notes/files/jira extras; `tools`,
`terminalPanel`, `viewSwitcher.selection` are shallow placeholders.

**GUI-only fields that stay default headless** (do not assert on them in headless tests):
`windows`, `keyWindow`, `frontmostPid`, `activations`, palette rows,
`viewSwitcher.rows`, `terminalPanel.wid/frame`, `tools.*.wid`.

---

## 2. Ground truth — isolation recipe (proven in-repo)

From `PROMPT-rust-port-phase4b.md:50-66` (already used by the parity harness):

```bash
D=$(mktemp -d)
TMPDIR="$D" WS_RS_ASSET_DIR="$PWD" \
  .build/rust-app/kitchen-sink.app/Contents/MacOS/kitchen-sink &   # GUI today
printf 'state\n' | nc -U "$D/ws-notes.sock" | jq
```

| Env var | Effect | Evidence |
|---|---|---|
| `TMPDIR=$D` | Socket + `.lock` + focus files live under it → full isolation | `PopupWindow.swift:302-306`, `kitchen_sink.swift:427`, `paths.rs:135,176-190` |
| `WS_HOME=$D/home` | Config home; honored by non-repo builds | `kitchen_sink.swift:52-55,61`; `paths.rs:99-105,121-125` |
| `WS_RS_ASSET_DIR=$PWD` | Points helper at checkout `pylib` (dev bundle has no Resources) | `main.rs:98-104`, `paths.rs:107-119` |
| `WS_RUST_KILL=0` | `build-rust-app.sh` default; **never** set 1 while the live app is up | `build-rust-app.sh:128` |
| `WS_PING_ONLY=1 <bin> [mode]` | Liveness probe, exit 0/1 | `main.swift:27-30` |
| `WS_PYTHON` | Python ≥3.11 override for helper/CLI discovery | `bin/ws-settings:11-20`, `run-tests.sh:157-164` |
| `PYTHONDONTWRITEBYTECODE=1` / `python -B` | **Always** — `__pycache__` in a signed bundle breaks its signature | `AGENTS.md`, `bin/ws-settings:22-24` |

**Repo-build caveat:** the repo-root `kitchen-sink.app` is a repo build
(`isRepoBuild` = `commands.toml` + `bin/build-app.sh` beside the binary,
`kitchen_sink.swift:47-51`), so its `userDir` is the repo regardless of `WS_HOME`
(`kitchen_sink.swift:61`) — a Swift sandbox instance reads/writes the **repo**
`commands.toml`. The `.build/rust-app` bundle is NOT a repo build
(`paths.rs:96-97`), so `WS_HOME` is honored there. This is why the sandbox uses the
Rust binary.

Kill an isolated daemon **by pid** (`lsof -U | grep "$D/ws-notes.sock"` or a pidfile) —
never `pkill -f kitchen-sink`, which can hit the live app.

---

## 3. Ground truth — Rust headless readiness

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
  `{"code","stdout","stderr"}` (`pylib/helper/script_bridge.py:15-47`); the app calls it
  with `timeout: 1800` (`kitchen_sink.swift:9550`).
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
| `ks_screenshot` | `{args?: string}` | `screenshot[\t<args>]` | `-r` → image content (base64 PNG); `-g` → `"W H X Y"` text; else `"ok"`; **no reply from Rust → `isError` with note** | MUT |
| `ks_compare` | `{left, right?, wait?, title1?, title2?, timeout_ms?}` | `compare\t…` | `--wait` → `"done"`; else `"ok"` | MUT |
| `ks_pane_shot` | `{args: string}` | `pane-shot\t<args>` | `path`/`copied`/`error: …` text | MUT |
| `ks_screenshot_permission` | `{}` | `screenshot-permission` | `granted`/`denied`; Rust no reply → `isError` | RO |
| `ks_helper` | `{method: string, params?: object, timeout_ms?}` | persistent helper worker (§7.3) | helper `result` JSON text; `ok:false` → `isError` (message + traceback excerpt) | MUT |
| `ks_settings` | `{argv: string[]}` | `python -B -m settings_hub <argv>` (PYTHONPATH=repo) | `{code, stdout, stderr}` JSON text | MUT |
| `ks_jira` | `{script?: "poll"\|"status", argv: string[]}` | `jira/jira_poll.py` / `jira/jira_status.py` | `{code, stdout, stderr}` | MUT |
| `ks_confluence` | `{argv: string[], stdin?: string}` | `confluence/confluence_api.py` | `{code, stdout, stderr}` | MUT |
| `ks_notify` | `{argv?: string[]}` (default `["--json"]`) | `notify/notify_poll.py` | `{code, stdout, stderr}` | MUT |

**Implementation notes**
- `ks_compare`: mirror the exact wire format from `handleCompareMessage`
  (`kitchen_sink.swift:9758-9771`) — read it first; paths are tab-separated, optional
  `--wait`/`--title1`/`--title2`. Lock the byte format with an L0 recording test.
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
| `board:*` errors | `ks_jira` CLI (`--board-*`, `--pin-*`, `--import-filters`, `--my-work`) + `--describe` JSON | `jira.rs:3084`, `host.rs:1370-1376` |
| `open:<path>` pending | `ks_launch "open:<path>"` only works on Swift; on Rust use `ks_helper` `fileops`/notes data or accept no-op | `host.rs:1119-1123` |
| `reset-size`, `rebuild-card:*`, `jira-jump`, `notes-find`, `notes-grep` | Avoid on headless; document | `host.rs:1182-1194,1364` |
| `key:SPEC` records only | Use `do:` verbs instead of key synthesis | `host.rs:1217-1220` |
| raw `screenshot`/`compare`/`pane-shot`/`screenshot-permission` no reply | Tools detect no-reply and return `isError` with "unsupported by this daemon (Swift-only)" | `host.rs:1694-1700` |
| unknown `do:` returns state | Cannot detect; document | `socket.rs:202-207` |

Future work (separate from MCP): register jira `test_do` for `board:*`, drain
`pending_note_open` headless, `reset-size`. Track in `BACKLOG.md`.

---

## 10. Component spec — `bin/ks-mcp-sandbox.sh`

Subcommands: `start | stop | status | sock | reset`. Config:
`SANDBOX="${KS_MCP_SANDBOX:-$ROOT/.build/mcp-sandbox}"` (`.build/` is gitignored).

- `start`:
  1. If `bin/build-rust-app.sh --stale` exits 0 (needs build), run
     `WS_RUST_KILL=0 bin/build-rust-app.sh` (never kills the live app;
     `build-rust-app.sh:128`).
  2. `mkdir -p "$SANDBOX/tmp" "$SANDBOX/home"`; if `home/commands.toml` missing, copy
     the repo `commands.toml` (sandbox config edits never touch the repo copy).
  3. Launch:
     ```bash
     BIN="$ROOT/.build/rust-app/kitchen-sink.app/Contents/MacOS/kitchen-sink"
     TMPDIR="$SANDBOX/tmp" WS_HOME="$SANDBOX/home" WS_RS_ASSET_DIR="$ROOT" \
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
- `ks_compare`: recorded wire line matches the `handleCompareMessage` grammar
  (read `kitchen_sink.swift:9758-9771` and lock it in); `--wait` reads `done`.
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
4. **`WS_RUST_KILL` must stay 0** for every `build-rust-app.sh` invocation.
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
WS_RUST_KILL=0 bin/build-rust-app.sh --stale; [ $? -ne 0 ] || WS_RUST_KILL=0 bin/build-rust-app.sh
D=$(mktemp -d)
TMPDIR="$D" WS_RS_ASSET_DIR="$PWD" .build/rust-app/kitchen-sink.app/Contents/MacOS/kitchen-sink headless &
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
> commit, do not run `./ws build` or UI suites, keep `WS_RUST_KILL=0`. Gate: the §14.1
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
