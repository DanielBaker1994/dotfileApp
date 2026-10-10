# Agent prompt: refresh the repo's agent context (AGENTS.md)

You are a senior engineer + technical writer dropping into an unfamiliar but
large codebase. Your ONE deliverable is a fresh, accurate agent-context document
at the repo root. You are NOT changing product code.

Repo: `/Users/danielbaker/.config/kitchen-sink` (the REAL git repo — verify with
`git rev-parse --show-toplevel` before any git command; rule.md says never run
git in a backup/duplicate).

## What this repo is

A macOS menu-bar app ("kitchen-sink") that is **mid-migration**:

1. **The original app** is Swift/AppKit — ~47k lines across ~55 top-level
   `*.swift` files (`PopupWindow.swift` ~11.5k, `kitchen_sink.swift` ~10.2k),
   built by one hand-rolled `swiftc` invocation in `bin/build-app.sh`. `./ws` is
   the human entry point (`./ws build`, `./ws test`, `./ws doctor`, …).
2. **A Rust port is in flight** under `rust/` — a cargo workspace
   (`ws-rs`, `ws-helpers`, `swiftterm-shim`) that mirrors the Swift files module
   for module using `objc2` (AppKit/WebKit/Vision/CoreText/ScreenCaptureKit).
   It builds a signed `.app` via `bin/build-rust-app.sh` (output isolated at
   `.build/rust-app/`), runs a daemon that answers the same Unix-socket
   `state`/`do:ACTION` contract, and has ~490 passing tests. See
   `PLAN-rust-port.md` (the port plan + progress) and
   `/tmp/rust-ui-test/PORTING-TO-RUST.md` (the feasibility PoC).
3. **Python is the frozen "cold path"** — `pylib/` (engines + a persistent
   JSON-lines helper), `jira/`, `confluence/`, `notify/`, `settings_hub/`. The
   Rust port talks to it through the same helper protocol; do not propose
   rewriting it unless there is a hard blocker.

The existing context docs are huge and partly stale: `AGENT_CONTEXT.md` (~120 KB;
`CLAUDE.md` is a symlink to it) plus several `PLAN-*.md`, `PRD-*.md`,
`REVIEW-architecture.md`, `BACKLOG.md`. Read them, but treat the CODE as truth.

## Non-negotiable rules you must respect (from `rule.md`)

- Edit shortcuts (Cmd+C/V/X/A/Z, Ctrl+C/V) must work in every text input.
- Right-click menus in file browsers / terminal must offer obvious actions.
- Config over code: user-facing strings/sizes/paths live in `commands.toml`.
- NEVER run git commands in a backup/duplicate workspace.
- NEVER rebuild or modify the SwiftTerm checkout (it is fetched, not committed,
  outside the repo at `../SwiftTerm`; read-only).
- Transient overlays own the keyboard; Esc closes only them.
- Python is treated as frozen for the Rust port.

## What to do

1. **Explore, don't assume.** Map the real architecture from the code:
   - the Swift app's entry point, window framework, views, and socket contract
     (`main.swift`, `kitchen_sink.swift`, `PopupWindow.swift`,
     `SharedWindow.swift`, `CardWindow.swift` — grep symbols, read ~60 lines);
   - the Rust workspace layout and what is ported vs still `todo!()`
     (`grep -rn "todo!()" rust/ws-rs/src`);
   - the build/install/signing/TCC flow (`install.conf`, `bin/build-app.sh`,
     `bin/build-rust-app.sh`, `bin/lib.sh`, `INSTALL.sh`, `bin/setup-home.sh`);
   - the Python boundary (`pylib/helper/`, `PythonHelper.swift`, the
     JSON-lines protocol) and the `jsonmgr` data manager;
   - the test surface (`bin/run-tests.sh`, `Tests/`, `bin/ui-test*.sh`,
     `bin/ui-test-focus.py`, the socket `state`/`do:` hooks), including
     `cargo test` for the Rust side and `python3 Tests/test_*.py`.
   - Where the two stacks meet: the socket protocol, `commands.toml` codec,
     the `[app]` config, and `ws` command names.

2. **Write `AGENTS.md`** at the repo root (`/Users/danielbaker/.config/kitchen-sink/AGENTS.md`)
   as the single, current, high-signal context doc for an AI agent working here.
   Reconcile it with the existing `AGENT_CONTEXT.md`/`CLAUDE.md` (do NOT delete
   them; if you can, also replace `AGENT_CONTEXT.md`'s body so `CLAUDE.md`
   symlink followers benefit — but the primary deliverable is `AGENTS.md`).

## What `AGENTS.md` must contain

Keep it dense and factual, in the style the repo already uses: symbol names over
line numbers (they drift), "grep the symbol, then read ~60 lines", a code map,
and explicit rules. Sections:

- **One-paragraph overview**: what the app is, the Swift→Rust migration state,
  what is authoritative right now (the Swift app; the Rust port is behind a
  parity gate), and how the two coexist.
- **Non-negotiable rules** (condensed from `rule.md`).
- **Build / run / test** — the exact commands: `./ws build`, `./ws test SUITE`,
  `bin/build-rust-app.sh`, `cargo test` (from `rust/`), `python3 Tests/test_*.py`,
  the UI suite, the `rs-parity` tool. Note what each actually does and any
  gotchas (TCC/Screen Recording, the `borders` daemon chicken-and-egg,
  `PYTHONDONTWRITEBYTECODE`, the signed-bundle `__pycache__` hazard).
- **Repo layout / code map**: top-level Swift files grouped by role; the
  `rust/` workspace modules and their Swift counterparts; the Python packages;
  key config files (`commands.toml`, `install.conf`).
- **Architecture**: the shared-window model, the socket daemon + `state`/`do:`
  contract + hotkey fast path, the pane/vim navigation, the theme system, the
  Python helper boundary, and (for Rust) the workspace + the parity approach.
- **The Rust port**: current progress, the module→Swift mapping, remaining
  `todo!()` seams (deep AppKit drawing), and where `PLAN-rust-port.md` lives.
- **Common tasks / how-tos**: e.g. add a `[app]` setting, add a keyboard
  shortcut, add a view, add a socket `do:` hook, run/repair the stack.
- **Gotchas / footguns**: things that have bitten people (ordering of window
  setup, main-thread discipline in objc2, retain cycles, `handleKey` ordering,
  config write path, sign/TCC). Pull these from `rule.md`, `REVIEW-architecture.md`
  and `FINDINGS-*.md`.

## Constraints

- **Do not modify product code, configs, or tests.** You may write only
  `AGENTS.md` (and, if you choose, replace the body of `AGENT_CONTEXT.md`).
- Verify claims against code; where the docs and code disagree, trust the code
  and call out the drift briefly.
- Keep it useful and scannable: prefer short bullets, symbol names, and
  commands over prose. Aim for something an agent can load in full and act on,
  not an exhaustive spec.
- Note anything you could not determine, and mark uncertainties explicitly
  rather than inventing facts.
- Do not commit. Leave `AGENTS.md` uncommitted for review.

## Output

- `AGENTS.md` at the repo root.
- A short summary (5-15 lines) of: what you changed, the biggest staleness you
  found in the old docs, and any open questions you could not resolve.
