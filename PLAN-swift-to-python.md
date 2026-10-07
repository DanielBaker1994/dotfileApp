# Refactor plan: move cold-path logic from Swift to Python

Local plan (the GitHub issue is skipped by request). Owner decisions are
locked below; no open questions.

## Progress

- [x] Phase A — helper package, Swift client, protocol tests, launch wiring
- [x] Phase B — doc templates ported; Swift copy + its swift test deleted
- [x] Phase C — prose PDF/preview ported; Swift copy + its swift test deleted
      (the RichText half of that suite moved to its own swift test)
- [x] Phase D — commands.toml codec: python is the single source; Swift
      shims + content-hash cache keep the hotkey path python-free; the
      Swift codec + its swift test are gone; AGENT_CONTEXT updated
- [x] Phase E — ported on owner's call (budgets knowingly relaxed): the
      whole text engine lives in `pylib/compare_text.py`; `CompareText.swift`
      is a mirror/facade over a handle-based model. `ws test compare`:
      29 python cases incl. git `--histogram` parity (60/60, 100%) and the
      windowed-rediff fuzz; 10k lines diff in ~35 ms (9 ms in-process).
      Folder: classification + content equality in `pylib/compare_folder.py`
      (one batched call per scan); the walker/tree/`SyncPlan`/gitignore
      stay Swift; 68/68 folder tests green.
- [x] Extras (owner call, after the plan): `AIFormat`'s pure logic moved to
      `pylib/ai_format.py` — rules + chains, code guard, token budget,
      reflow, answer cleanup, CSV tables, word guard, and the pandoc HTML +
      inline styling; `AIFormat.swift` keeps the enums, the facade, and
      rtf/pasteboard. `ws test ai` is python (26 cases, first-run green).
- [x] Extras 2 (owner call): `FileOps` mutations + undo stacks moved to
      `pylib/file_ops.py` (handle-based stacks; trash via `~/.Trash`),
      facade in `FileOps.swift`; `ws test fileops` python (7 cases) and the
      folder suite still 68/68 through the facade. `SwitcherStatus`'s
      CPU/RAM/battery/unread samplers moved to `pylib/status.py`
      (Mach host_statistics via ctypes, vm_stat, pmset, notify script);
      `ws test status` (8 cases). All suites + full Swift sweep green;
      app builds.
- [x] Extras 3 (owner call): folder compare completed —
      `pylib/ignore_rules.py` (gitignore, `git check-ignore` parity 30/30)
      and `pylib/compare_folder.py` now own the scan walker (chunked,
      progress + cancel per step), the paired tree, settle/roll-up,
      rows/filters/counts/paths, rule candidates and `SyncPlan`;
      `FolderTree`/`FolderNode` are mirrors over a handle. `ws test ignore`
      (5 cases) + the folder suite 68/68 through the new path. All suites
      and the full Swift sweep green; app builds.
- [x] Wrap-up — docs updated for everything that moved; nothing committed.

## Problem Statement

Most of the app's non-UI logic is Swift, but Python is the more flexible
place to hold it: scriptable, testable without compiling, and able to share
one implementation with the CLI tools (settings_hub, jira, confluence) that
already live in Python. Today the only such logic is duplicated by hand:
the commands.toml codec exists twice (Swift `ConfigText` / python
`config_text`), and keeping the copies in sync is a standing maintenance
risk with parity only caught by eye.

## Solution

Stand up ONE persistent Python worker ("the helper"): a long-lived child
process the app talks to over JSON-lines on stdin/stdout. Then move
cold-path, Foundation-only logic into it, module by module:

    add the python module + python tests  ->  switch the app call sites
    to the helper (Swift copy kept)      ->  delete the Swift copy + its
    swift test (python tests are the tests now)

Each step is a tiny commit that leaves the app working. Python is expected
to be present: the helper is discovered like `bin/ws-settings` discovers
its interpreter, and the parts of the app that need it show a clear Setup
error when it is missing. Ported code is stdlib-only.

First modules: doc templates, prose PDF, the commands.toml codec
(deduplication), then text/folder compare. Everything AppKit stays Swift.

## Commits

Phase A - the helper (no behavior change):

1. Add the python helper package: a JSON-lines protocol (one request per
   line, one response per line, ids echoed, error envelope, unknown
   methods reported, `shutdown`), a `ping`, and one-shot modes (`--once`
   on stdin, `--call` for scripts) plus `--version`. Its own python test
   drives the protocol and the one-shot modes. Nothing in the app uses it
   yet.
2. Add the Swift helper client: discovers its interpreter (WS_PYTHON, the
   brew paths, then PATH; version check), spawns the worker lazily off
   the main thread, matches responses to pending calls by id, times out,
   restarts a dead worker, and delivers completions on the main thread.
   No feature uses it yet.
3. Add the Swift/py protocol test: ping, several calls in flight,
   unknown-method error, worker killed mid-flight -> next call restarts
   it and succeeds.
4. Start the helper lazily at app launch, off the main thread; log
   lifecycle to the app's debug log. App behavior is unchanged.

Phase B - doc templates:

5. Add the python doc-templates module (builtin names, palette discovery
   from css, parse/current/marker/edit/apply, configured names) and port
   the Swift unit test case-for-case to python; green.
6. Make the notes template menu call the helper for the current template
   and for apply (completion on main; menu closes immediately). Swift
   implementation stays as a fallback.
7. Delete the Swift doc-templates implementation and its swift test;
   point the test runner's suite at the python test. The menu still
   behaves exactly as before.

Phase C - prose PDF:

8. Add the python prose module (output path, pandoc argv, filter
   discovery, weasyprint argv, header/css assembly, preview HTML,
   export, error messages) and port the Swift test's prose-PDF cases to
   python; green. The RichText/AIFormat cases stay Swift.
9. Route the notes preview and the PDF export through the helper; keep
   the Swift implementation as a fallback.
10. Delete the Swift prose-PDF implementation and its swift test; point
    the suite at the python test. The AIFormat cases in that suite stay
    in their own Swift test.

Phase D - commands.toml codec dedup (owner decision: python-only):

11. Grow the python codec's tests to mirror every Swift test case (entry
    parsing, quoting, bare scalars, section setting, comment/indent
    preservation, spans) and add a temporary corpus parity check that
    runs both implementations and compares their outputs.
12. Route every Swift reader/writer of commands.toml through the helper
    (config load/save/reload, screenshot, compare); the Swift codec stays
    in the tree for this commit.
13. Delete the Swift codec and its swift test; python is the single
    source of truth. Keep the non-codec helpers that live in the same
    Swift file (the tri-state parser and the binary resolver) by moving
    them next to their users. The Setup/preflight wording notes that
    python is now required.

Phase E - compare: dropped after inspection. The engines are interactive
and budget-enforced; they stay in Swift (see the Decision Document).

Wrap-up:

17. Update the repo's context docs: the helper, the python >= 3.11 rule,
    the new module home, and "one codec, in python".

## Decision Document

- Integration: a persistent python worker, JSON-lines over stdin/stdout,
  one request per line, ids echoed, completions on the app's main thread,
  crash restart, request timeouts. Not a socket, not a new launchd agent.
- Scope: cold-path logic only. No per-keystroke logic moves (filtering,
  vim search, layout math stay Swift). No AppKit, no SwiftTerm/terminal,
  no screen capture, no speech, no Vision.
- Interpreter: python >= 3.11, stdlib only, discovered the way the
  settings launcher discovers it (WS_PYTHON, /opt/homebrew/bin/python3,
  /usr/local/bin/python3, PATH), version-checked before use.
- Deduplication: python is the single source of truth for the
  commands.toml codec; the Swift copy is deleted (owner choice 1A).
  Python therefore becomes a hard requirement for the app's config.
- Homes: the helper and every ported module live in the existing python
  library directory that the installer already links into the app bundle;
  no installer, build or DMG changes.
- The Swift client is the only new Swift-side concept; feature code talks
  to it with method + params and gets results on the main thread.
- Testing: python for everything that moves; a Swift test remains only
  for the client's own protocol behavior. A suite is "ported" only after
  its python test covers the Swift test's cases case-for-case.
- Every commit leaves the app working: additions land before deletions,
  and deletions happen only after the switch is in place.
- Compare: owner overruled the cold-path exclusion (see Progress). The
  text engine is python behind a handle; Swift keeps a mirror for drawing
  plus an in-process CharDiff memo cache and a 10-line BinaryCompare on
  in-memory Data. The folder classifier and content equality moved; the
  walker, the mutable tree, gitignore and `SyncPlan` stay Swift. The
  in-process budgets were relaxed knowingly.
- Out of nothing: no GitHub issue, no network calls in the helper, no
  runtime dependency beyond python itself.

## Testing Decisions

- Good tests here exercise external behavior: request in, response out.
  Ported tests keep the exact case lists of the Swift tests they replace.
- The helper's python test drives the real protocol (subprocesses), not
  just the in-process dispatch, so line framing and exit behavior are
  covered.
- The Swift client test covers discovery, ping, concurrency, timeouts and
  restart-after-kill.
- Ported module tests run on the discovered python >= 3.11; the test
  runner learns one new suite per phase (`ws test helper`, and so on).
- The parity check for the config codec is temporary scaffolding: it
  exists to prove equivalence before the Swift copy is deleted, and is
  deleted with it.

## Out of Scope

- All AppKit/UI code and window frameworks.
- SwiftTerm and everything terminal-related (the fetched dependency).
- Screen capture, annotations, OCR, speech, on-device AI formatting.
- File operations (mutation-heavy; revisited later if wanted).
- Hot-path logic: live filtering, vim search, pane geometry, nvim RPC.
- The folder walker, tree walking, gitignore matching and `SyncPlan`:
  interactive/UI-coupled (see the Decision Document); automation reaches
  the compare engine through the app's `compare` CLI.
- The jira/confluence/notify/settings python code itself: unchanged.
- Migrating the settings launcher off its own interpreter discovery.

## Further Notes

- The app already sets PYTHONDONTWRITEBYTECODE and WS_COMMANDS_CONF for
  python; the helper inherits both. A __pycache__ inside the signed
  bundle breaks the signature.
- The helper must never write anything but protocol lines to stdout:
  ported modules return values, they do not print.
- The app's PATH for children prepends the brew paths, which the helper
  needs so pandoc/weasyprint resolve when it runs them in phase C.
