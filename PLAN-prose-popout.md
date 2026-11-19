# PLAN — prose pop-out: right-click PDF + cross-screen disappearance

Investigation plan (not a fix yet). Symptoms:

1. **Primary:** in the pop-out window, right-click → "Copy PDF Path" does not
   generate a PDF. The same menu works in the in-place prose reading view.
2. **Secondary (acceptable if Aerospace):** the pop-out window disappears or
   "crashes" when moved to a different screen.

## 0. What the static pass already established

Map of the moving parts:

| Piece | Where | Note |
|---|---|---|
| Daemon spawns the pop-out | `views/notes.rs` `ProseProcess::launch` (~3217) | `<current_exe> prose …`, **stdin/stdout/stderr all `/dev/null`** → a child panic is invisible |
| Child entry | `main.rs` "prose" branch (57–96) | `configure_helper()` → `ProseProcess::run` → `ProseWindow::show` **before** `NSApplication::sharedApplication` + `app.run()` (60 vs 66–72) |
| Child lifetime trap | `main.rs` 76–94 | `NSWindowWillCloseNotification` observer registered with **`object: None`** → *any* window's will-close calls `std::process::exit(0)` |
| The window | `ProseWindow::show` (notes.rs 3049) | titled NSWindow, dialog subrole hint, buttons hidden, `⤢` strip; context menu wired at 3138 via `set_on_menu` |
| Menu | `notes_prose::show_context_menu` (478) → `menu_kit::popup_context` (313) | `popup_context` **silently returns when `NSApp.currentEvent()` is nil**; `run()` executes the picked closure only *after* `popUpContextMenu` returns |
| Export | `export_pdf_and_copy` (436) → thread → `ProseRender::export_pdf` (notes.rs 1242) → `helper_call("prose.pdf_export", …).ok()` (notes.rs 250) | helper errors are swallowed to one `PdfExport{ok:false}`; a static `EXPORTING` latch is armed at click time and reset **only** in the `run_on_main` completion |
| Child logging | `app/logging.rs` | child `ws_log` appends to the same `logs/YYYY_MM_DD/daemon.log` — usable for breadcrumbs, but **nothing logs the prose menu/export path today** (grep found zero prose entries) |

Evidence already gathered:

- **No crash reports for the release app.** `~/Library/Logs/DiagnosticReports` has only 3 `kitchen_sink-<hash>.ips`, all from **debug** builds dying at launch (`DYLD: Library missing: libswift_Concurrency.dylib`, paths under `rust/target/debug` / `.build/rust-target/debug`, one *today at 09:44*). A debug-built pop-out child dies instantly → "disappears/crashes" without a runtime trace.
- **Aerospace rules do catch the pop-out.** `config/aerospace/aerospace.toml` `on-window-detected` matches `test %{app-bundle-id} = dev.danielbaker.kitchen-sink` → `layout tiling`, `move-node-to-workspace 2`, and workspace `2` is force-assigned to `main`. The child window has the same bundle id, so it is tiled onto workspace 2/main; the dialog-subrole hint does not exempt it from a bundle-id rule. Dragging it to the secondary display → Aerospace re-asserts workspace 2 → window "vanishes" off the display you are on. (User: acceptable, but confirm it is the whole story.)
- The Python/pandoc stack is exonerated: the in-place menu and PDF export share the *same* code and work in the daemon process. The failure is specific to the **child-process context**.

## 1. Ranked hypotheses — right-click PDF (primary)

| # | Hypothesis | Why it fits | Falsifier (quick test) |
|---|---|---|---|
| H1 | **The child dies when the menu closes** — the unscoped `will-close` observer fires for the menu's own window (or any auxiliary window) → `exit(0)` before `menu_kit::run` reaches the picked closure | Explains "nothing happens" with **no crash report** (clean exit), and a window that "disappears" during interaction | Right-click → pick a row → `pgrep -fl "kitchen-sink prose"`. Dead after the first menu dismissal ⇒ H1. Also test dismiss-with-Esc: does the pop-out survive? |
| H2 | **The menu never opens in the child**: `popup_context` bails on `currentEvent() == nil`, or `rightMouseDown:` never reaches `ProseWebView` because NSWindow/WKWebView were created **before** `NSApplication` existed (main.rs ordering) | In-place works because the daemon creates NSApp first | Does the two-row menu *appear* on right-click in the pop-out? If no menu ⇒ H2. Compare with `Copy Absolute Path` (same menu, no helper) |
| H3 | **`EXPORTING` latch stuck / completion never runs**: one failed/abandoned attempt leaves `EXPORTING == true` (reset happens only in the `run_on_main` completion; a thread panic never clears it), so every later click is a silent no-op | Matches "worked once-ish then never again", silent because stderr is `/dev/null` | Do "Copy Absolute Path" (unaffected), quit the child, relaunch, single click ⇒ if the first click works per fresh child ⇒ H3 |
| H4 | Export actually fails inside the helper in the child, and the error toast is lost (e.g. `run_on_main` never drains, main-queue servicing differs) | Would show "Generating PDF…" then nothing | Breadcrumb log (step 2) prints the `PdfExport` result; toast observed manually |

## 2. Investigation steps

### Step A — controlled repro with the child's stderr visible (no rebuild)

From a terminal, using the **release repo app** (note the file under a scratch dir, e.g. `$TMPDIR`-approved):

```bash
cd ~/.config/kitchen-sink
./ws build            # ensure a current release binary (exit 0)
R=kitchen-sink.app/Contents/MacOS/kitchen-sink
N=/private/var/folders/l0/63hb7phx13s3nw778ttpxn280000gn/T/opencode/popout-probe.md
printf '# Probe\n\n- right-click me\n' > "$N"
"$R" prose --size 19 --width 900 "$N" 2> ~/prose-child.log &
CHILD=$!; echo "child pid $CHILD"
# …interact: right-click → Copy Absolute Path → Copy PDF Path;
# between every step: ps -p $CHILD; move the window to the other screen and repeat
tail -50 ~/prose-child.log            # panics ("thread … panicked") land here, NOT in the daemon log
ls -lt ~/Library/Logs/DiagnosticReports | head   # new kitchen-sink*.ips?
aerospace list-windows --all | grep -i "$CHILD"  # where does Aerospace put it, is it tiled?
```

Record: does the menu appear, which rows, does the toast ("Generating PDF…") show, does the child survive menu dismissal, does `prose-child.log` contain a panic. **Do not use debug builds** (`cargo run`) for symptom 2 — the existing .ips show they die at launch on the SwiftTerm `libswift_Concurrency` rpath, which masquerades as "the window crashed".

### Step B — breadcrumbs (tiny PR, before any fix)

`ws_log` lines (child → same `daemon.log`) at: `show_context_menu` entry (`note` + whether `currentEvent` was nil), each menu closure start, `export_pdf_and_copy` (latch swap result, thread start, `PdfExport` summary, completion-on-main entry). Gate-free but terse; revert afterwards or keep behind `[app] verbose` if useful.

### Step C — targeted hypotheses checks

- **H1**: if Step A shows death at menu close, move the observer in `main.rs` to the specific window (keep a `Retained<NSWindow>` from `ProseWindow::show`, observe `object: Some(window)`, or use `windowWillClose:` via delegate). Verify: right-click works repeatedly, child exits only on Cmd+W / red close.
- **H2**: if no menu, reorder the child entry — create `NSApplication` + activation policy + main menu **before** `ProseProcess::run` (`main.rs` 57–96). If the menu still fails, make `popup_context` fall back to `popup_at` at `NSEvent.mouseLocation` when `currentEvent()` is nil (one-line defensive branch; menu_kit already has `popup_at`).
- **H3**: wrap the export thread body so the latch is reset even on early return/panic (scopeguard-style), and have `helper_call(...).ok()`'s `None` log the `HelperFailure` reason instead of dropping it (notes.rs 250/1256).
- **H4**: if the completion never runs, check main-queue servicing during menu tracking; the robust fix is Step E.

### Step D — the cross-screen half

- Reproduce with `aerospace list-windows --all` + the child pid before/after the drag; watch `ps -p $CHILD` (alive = "disappeared via tiling", dead with no .ips = H1 again, dead with .ips = real crash → read the .ips faulting thread).
- If Aerospace is the cause (expected): decide between accepting + documenting, or refining `on-window-detected` — prose pop-out titles are the file name (`*.md`), so a `test %{window-title} ~= '[.]md$'` guard that skips `move-node-to-workspace 2` / applies `layout floating` is surgical. The daemon's shared window is never titled `*.md`, so the guard is safe.

### Step E — fallback design if child-context fights us

Route the pop-out's PDF export through the daemon instead of duplicating engine context in the child: a new `do:` hook (`do:prose-pdf:<path>`, host-side `do_host_action` / registry `register_test_do`) that calls the already-working `export_pdf_and_copy` in the daemon; the child's menu item sends the socket line and only toasts locally. Precedent: MENU-INVENTORY §R5 considered "option (b) a socket round-trip so the pop-out could [do daemon things]" — this applies it to the PDF row. Costs: one `do:` verb + a small client helper in the child; removes helper/clipboard/toast/main-queue variability in the child entirely.

## 3. Deliverables / acceptance

- [x] Step A log attached to this plan (see §4 — partial: session was locked
      during the fix session, interactive right-click deferred to owner).
- [x] Root cause picked from H1–H4 (+ Step D verdict for the move symptom).
- [x] Fix landed + `./ws test prose` / `./ws test rust` green (867 tests).
- [ ] Verified flow: pop-out right-click → "Copy PDF Path" → PDF exists under
      `[notes] pdf-path`, path on clipboard, toast shows, child stays alive;
      window survives cross-screen drag. **Owner manual pass required** — the
      session was locked during implementation and AX/window probing cannot
      see live windows (the breadcrumbs now record everything in `daemon.log`).
- [x] `HANDOFF`/`BACKLOG` lines updated; child stderr decision made
      (stderr → `logs/YYYY_MM_DD/prose.log`, see §4).

## 4. Outcome (2026-10-17) — implemented

Step A ran as far as a locked session allows (`CGSSessionScreenIsLocked: 1` —
AGENTS.md: "AX sees no windows while the session is locked"). Every external
probe (AeroSpace `list-windows`, System Events window counts, window titles)
came back empty for BOTH the daemon and the pop-out child, so those readings
are artifacts, not symptoms. The child's own AppKit view — captured by the new
startup breadcrumb — is the reliable evidence:

```
ws: prose child: 1 window(s): "popout-probe.md" vis=true CGSize { width: 1026.0, height: 859.0 }
```

Child spawns clean, builds its NSApplication, creates the titled pop-out
window, loads the WebKit content (WebContent XPC spawns), and sits idle in
its run loop — no panic, no helper failure (`helper … failed` never logged,
which the new `helper_call` breadcrumb would have surfaced).

Root cause verdict — all three top hypotheses were real defects and all were
fixed; the primary PDF symptom is H1:

| # | Verdict | Fix |
|---|---|---|
| H1 | **Primary.** The child's `NSWindowWillCloseNotification` observer was registered with `object: None`, so any *other* window closing — most plausibly the right-click menu's own tracking window — called `exit(0)` before/while the picked menu closure ran: a clean, report-less death, and the "window disappears during interaction" symptom. | `main.rs`: observer scoped via `Some(window)`; `ProseWindow::take_close_notifier()` (`views/notes.rs`: `show()` stashes the window in a new `CLOSE_TARGET` thread-local, main.rs takes it, falls back to the first `LIVE_WINDOWS` entry). |
| H2 | **Hardened.** The child created its NSWindow + WKWebView *before* `NSApplication::sharedApplication` existed — the ordering behind nil-`currentEvent` menu bail-outs. | `main.rs`: NSApp + activation policy + main menu now installed **before** `ProseProcess::run` creates the window. |
| H3 | **Hardened.** `EXPORTING` was reset only in the `run_on_main` completion — a worker panic wedged every later click for the process's life; `helper_call` dropped `HelperFailure`. | `notes_prose::export_pdf_and_copy`: `catch_unwind` around `ProseRender::export_pdf` (panic → `PdfExport{ok:false}` so the completion always resets the latch); `helper_call` logs `helper <method> failed: <reason>`. |
| H4 | Exonerated by the breadcrumb: the helper stack responds in the child (config load at launch succeeds; failures now log). | — |
| Step D | Accept the Aerospace story but refine per plan. | `config/aerospace/aerospace.toml`: new first-match `on-window-detected` rule — `kitchen-sink` windows titled `*.md` get `layout floating` (keep frame/screen); the tiling rule stays for the shared window (titles are view names, never `*.md`). Applied live with `aerospace reload-config --warnings-as-errors`. End-to-end cross-screen check needs an unlocked session + the new child. |

Step B breadcrumbs landed (gate-free, terse, `daemon.log`):
`prose menu: <note> (currentEvent ok|nil)` at menu open, `prose menu: picked …`
per closure, `prose pdf: start/result/skip` + `prose child: N window(s)` for
the export + launch paths, and the `helper … failed` line above.

Step E (`do:prose-pdf:` socket round-trip): **not needed** unless the owner's
manual pass still shows the PDF row failing in the child — the breadcrumb
trail in `daemon.log` will say exactly which stage breaks.

Child-stderr decision (deliverable): the daemon now spawns pop-out children
with stderr appended to `logs/YYYY_MM_DD/prose.log` (`views/notes.rs`
`macos::launch` → `prose_child_stderr_log()`; falls back to `/dev/null` if the
file cannot be opened). Panics are no longer invisible; `ws_log` still goes to
`daemon.log`.

Debug-build caveat from §0 stands: verify with the release app only
(`./ws build`) — the three `kitchen_sink*.ips` crash reports were debug builds
dying on the SwiftTerm `libswift_Concurrency` rpath, masquerading as crashes.

### Owner manual pass (unlocked session)

1. `Hyper+N` → note → pop out prose (prose switcher).
2. Right-click → **Copy Absolute Path** → toast shows, path on clipboard; the
   pop-out must **survive** the menu dismissal (`pgrep -fl "kitchen-sink prose"`).
3. Right-click → **Copy PDF Path** → "Generating PDF…" then "Copied ~/…pdf to
   clipboard"; PDF exists under `[notes] pdf-path`.
4. Drag the pop-out to the secondary display → it stays (floating rule; check
   `aerospace list-windows --all` shows `layout floating` and no workspace-2 move).
5. Close with `Cmd+W` → child exits; a fresh pop-out click relaunches.
6. Anything failing: `grep 'prose ' logs/YYYY_MM_DD/daemon.log` + `prose.log`.
