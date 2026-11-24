# Teardown handoff — what's left and how to run it

Written 2026-11-23 for the next agent. Delete this file when the work below is done
(fold anything still open into BACKLOG.md).

## Hard rules (owner)

1. **Never remove a working feature or view unless the owner names it and says yes.**
   "Keep going" / "simplify aggressively" covers dead code, duplicate code, stale docs and
   unused scripts only. (Confluence + AI were removed without approval once — 9acf789 —
   and had to be restored in c71766e.) Kept features: main window with Files, Jira,
   Confluence, AI views; Hyper+S quick agent (palette, Pretty Print / Create File / Text to
   Table, Find Recent Files, Hyper+X screenshot); notify/, settings_hub/, health checks.
2. Commit straight to `main`, one commit per theme, evidence in the message. Stage explicit
   paths when a worker is editing in parallel.
3. Don't run `./ws build`, `run_flows.py` or anything `--gui` unprompted: it kills /
   relaunches the owner's app or takes over the screen. `cargo build --release` is fine.
4. DeepSeek workers: `opencode-go/deepseek-v4.1-flash` only (v4-pro is blacklisted); 5+ in
   parallel is fine.

## State now

- Commits this round: `3e469bc..44b6db2` (egui UI, terminal/SwiftTerm, ~20k lines of dead
  Rust, 34 dead pylib helper methods, stale docs/scripts, dead config fields, DRY helpers).
- Baselines: `cd rust && cargo test -p ks-core -p ws-rs -q` → ks-core **485** (+4 ignored),
  ws-rs **195**. All `Tests/test_*.py` pass. Release build OK, no new warnings.
- Sizes: ws-rs 33.7k lines (host.rs 12.9k), ks-core 54.9k, 34 tracked markdown files.
- Stash `ks-wip-before-confluence-restore-20261123` (`git stash list`): pre-restore WIP —
  a half-done Confluence/AI deletion (UNWANTED), half-done docs edits (assumed Confluence
  gone — don't reuse blindly), and the uncommitted non-macOS code strip (~1k lines, needs
  owner approval, see Decisions). Drop it once nothing in it is wanted.

## Known failures (not caused by this round)

- `bin/linux-check.sh check` fails: `ks-core/src/views/files.rs` `copy_contents_entry`
  calls macOS-only `ui::toast::show` (already broken at eb76b0e).
- `Tests/test_mcp_verbs.py`: `test_every_do_host_action_verb_is_in_the_table` fails at the
  base too; the `cycle` verb row likely too (unverified — the sandbox only runs from the
  main checkout).
- `engines::path_shelf::tests::pure_add_merges_with_another_writers_entries_atomically`
  is flaky under parallel test runs (passes alone).

## Tasks, in order

### 1. Markdown consolidation (owner asked for it) — Sonnet or a DeepSeek write worker
Target set: AGENTS.md, README.md, rule.md, BACKLOG.md (all open items), TESTING.md (trim to
~200 lines), DESIGN.md + PRODUCT.md (read by the owner's design skill), `CLAUDE.md` →
symlink to AGENTS.md, delete AGENT_CONTEXT.md. Keep `.claude/skills/**`, `templates/**`,
`Tests/usability/`, `rules/` (AI rules — the AI view is kept!).
Delete after carrying open items into BACKLOG.md: HANDOFF-rust-port-next.md, APP-BUGS.md,
FEATURES.md, PRD-screenshot-gaps.md, PRD-screenshot.md, PRD-settings-hub.md, PRD-keyboard.md,
GAPS-shortcuts.md, PARITY-shortcuts.md (also update the `shortcut-parity-audit` memory),
PLAN-*.md (cross-platform, windows, windows-scouting, appkit-seams-findings, prose-popout,
quick-daemon), PORTS-STATUS.md, LOG-FILES-SUMMARY.md, UI-TEST-FINDINGS.md,
Tests/app_flows.md (merge its oracle format into TESTING.md), samples/graphviz-showcase.md,
.impeccable/critique/*.md.
Purge only text about things that are really gone: egui/Linux UI, SwiftTerm/terminal
drawer, Notes view, Compare view, prose/PDF pop-out, Swift parity. **Confluence and AI
stay documented.**
Fix pointers to deleted docs: `bin/lib.sh` (HANDOFF mention), `ks-core/src/views/screenshot.rs`
~line 51 (PRD-screenshot-gaps), `settings_hub/__init__.py`, `bin/ws-settings`,
`Tests/test_settings_hub.py` (PRD-settings-hub), `Tests/night_run.py` (PRODUCT/PRD prompts),
`Tests/app_flows.json` "sources" fields.
Gates: `git grep` for every deleted filename returns nothing; `readlink CLAUDE.md` = AGENTS.md;
`python3 Tests/test_settings_hub.py` passes; `bash -n bin/lib.sh bin/ws-settings`.

### 2. Low-risk shell DRY — Sonnet
- Python ≥3.11 picker copied in `bin/ws-settings`, `bin/ks-mcp`, `bin/text-table` → one
  `ws_python` in `bin/lib.sh` (lib.sh is bundled; check install.conf RESOURCE_BIN).
- `UNINSTALL.sh` `warn()` prints a red ✘ (inconsistent with INSTALL.sh) — align.
- Dead text in scripts: `ws:19` mentions a `compare` suite; `bin/lib.sh` `clean --all` still
  wipes `.build/SwiftTerm`; `bin/preflight.sh` swiftc "terminal shim" check (~208-213);
  `jira/jira-doctor.sh` Nerd-Font "terminal drawer glyphs" check (~56-60);
  `bin/grant-permissions.sh` microphone/speech grants (no Rust user — verify first).
Gates: `bash -n` each edited script, `./ws help`, `./ws test config`.

### 3. Riskier DRY — Opus, or Sonnet with owner sign-off and a live UI check
- Code-signing identity create/validate duplicated inside `bin/lib.sh`
  (`ws_ensure_signing_identity` vs the block in `ws_permissions_fix`), TCC.db path block in
  `lib.sh` + `grant-permissions.sh`, app-kill logic in ~7 scripts. Touches signing/TCC and
  kills the app: needs the owner around.
- Palette key grammar duplicated between `ks-core/src/app/quick.rs` (~925-1010) and
  `ws-rs/src/app/host.rs` `route_overlay_key` (~80 lines) → one `palette_key()` in
  `views/switcher.rs`. Needs real-key testing (see AGENTS.md / ks-input), so owner approval.
- Split host.rs giants (`do_host_action` ~390 lines, `route_key_event` ~300, `jira_event`,
  `files_event`, `files_key`, `jira_actions_or`, `install_daemon_ui`): structure only, keep
  behaviour; tests must stay at the baseline counts.

### 4. Decisions only the owner can make (ask plainly, one at a time)
- Drop Linux/Windows support entirely (non-mac is headless-only now; Linux check already
  broken)? If yes: the stash has a ready non-mac strip (`strip_cfg.py`), plus delete
  `bin/linux-check.sh`, `bin/windows-check.sh`, `bin/build-linux.sh`,
  `Tests/test_linux_headless.sh`, `rust/docker/`, the `ws test linux*|windows` cases.
- Keep the GUI test harness (`bin/ks-vm`, `ks-drive`, `ks-input`, `ks-ocr`,
  `Tests/run_flows.py`, `drive.py`, `haiku_run.py`, `night_run.py`, ~3k lines)?
- Anything about the Jira / Files / Confluence / AI views or the main window.

## How to orchestrate

Use the `codebase-teardown` skill (`~/.claude/skills/codebase-teardown/SKILL.md`) with
`opencode-delegate` for dispatch. Its `scripts/` folder holds the tools used this round:
`prune.py` (delete items from rustc dead_code JSON), `defonly.py` (pub items that occur
once), `delfns.py` / `delblock.py` (delete fns / brace blocks by name / line),
`prune_tests.py`, `restore_tests.py` + `restore_helpers.py` (recover tests of live code
lost with test-only helpers), `strip_cfg.py` (non-macOS cfg strip).

Loop per task:
1. Brief a worker (it sees nothing else): repo context, the kept-features list above, exact
   scope, files it must not touch, the real gate commands, report format, "do not commit".
   Read-only audits: `relay.mjs --read-only`; edits: no flag. Background it.
2. Never trust its report: rerun every gate yourself, read the diff, grep for dangling
   references, compare test counts to the baselines and explain any drop.
3. Commit per theme. Report to the owner in plain words; one recommendation, not a menu.

Dead-code recipe (Rust): `RUSTFLAGS="--force-warn dead_code" CARGO_TARGET_DIR=<scratch>/target
cargo check -p ws-rs --message-format=json`, feed to `prune.py`, loop. An item that is dead
only outside tests can be a test-only helper on a live type — keep it under `#[cfg(test)]`
rather than deleting it and the tests that use it.

## Model choice

Sonnet is a good fit for tasks 1 and 2: mechanical, fully checkable with grep, `bash -n` and
the test suites, and DeepSeek can still do the audits and bulk edits underneath it. Keep
task 3 on Opus (or Sonnet with the owner present): those changes are structural or touch
signing/TCC, and the risk is judgment, not typing. Task 4 is the owner's call.
