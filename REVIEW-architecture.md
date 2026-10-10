# Architecture review — Swift→Python port session (origin/main..HEAD)

Scope: everything since the last push (28 commits, ~15.5k insertions), i.e. both
PLAN-swift-to-python (helper + first ports) and PLAN-py-core phases A–C, plus
the settings-hub/data JSON commits. Read-only review; findings are things that
will bite later, not style. Owner-locked decisions are not challenged; where one
is objectively risky it is flagged once.

Counts: **0 blockers, 8 major, 5 minor.**

---

## 1. Compare-folder refresh does 2–3 synchronous helper round trips per cursor key — major

**What/where:** `CompareFolderView.swift:573` `sync()` is called from `move(to:)`
(per arrow key) and calls `summaryText` (~:594, `t.counts()`), again for the
summary colour (`s.tree.map { $0.counts() }`), and `statusText` (~:621,
`t.counts()` again). `CompareFolder.swift:161` `FolderTree.counts()` → `cfBox("folder.counts")`
`callSync`.

**Why wrong:** PLAN-py-core's Further Notes explicitly says this call site
"must not get worse and gets a client-side memo while we are in the area". It is
now 2–3 blocking IPC round trips per key that used to be an in-process scan, and
no memo was added. With a busy helper (finding 3) each key can stall for the
callSync timeout.

**Fix (small, testable):** memoise `Counts` inside the `FolderTree` mirror, keyed
by a version bumped in `scan_step`/`settle`/`recompute`; `sync()` reads the memo
once. Assert in `Tests/test_compare_folder.swift` (or extend the UI-test hook).

**Overlap:** none (PLAN-py-core audit item); ship as its own tiny commit.

## 2. Blocking IPC inside compare draw for char marks — major (owner-locked area, flagged once)

**What/where:** `ComparePane.swift:158-170` `marks()` is called from `drawRow`
(:240); on a cache miss it calls `CompareText.swift:422-435` `CharDiff.marks`,
which does `PythonHelper.callSync("compare.marks", timeout: 30)`.

**Why wrong:** the owner knowingly relaxed compare latency, but the first sight
of any changed line pair now does an IPC round trip **inside draw**, and a
saturated helper freezes the main thread for up to 35 s mid-render. The plan's own
note said this path must not get worse; in-process → put-in-draw is strictly worse.

**Fix:** prefetch `compare.marks` asynchronously for visible changed rows when
the viewport/rows change; draw uses the memo only and `setNeedsDisplay` when a
prefetch lands. No Swift rule re-implementation, no reversal of the port.

**Overlap:** none; defer as a small standalone fix.

## 3. One 8-worker pool serves interactive calls and multi-minute `script.run` jobs; children have no timeout/cancel — major

**What/where:** `pylib/helper/__main__.py:15` `MAX_WORKERS = 8`; all methods
share the pool; `script_bridge.py:26-44` runs children with no timeout;
`kitchen_sink.swift:9550` `JiraPoll.run` calls `script.run` with a 1800 s client
timeout.

**Why wrong:** jira list windows `watchTick`, `--release-view`, `--my-work`,
poll-now and user actions can put 5–8 `jira_poll.py` runs (seconds to minutes) on
the pool at once. Then every other helper call — including `config.setting`
during a settings toggle — times out at its 10 s callSync; `saveConfigValues`
then silently no-ops (`kitchen_sink.swift` save path). The claim "a slow script
call cannot stall other requests" only holds below 8 slow calls. On client
timeout the child and the pool thread keep running forever (nothing passes a
timeout to the child), so repeated timeouts permanently shrink the pool.

**Fix:** dedicated bounded executor for `script.run` (isolated from protocol
calls), pass `timeout` through and kill the child on expiry, and add a helper test
that saturates the script lane and asserts `ping` still answers.

**Overlap:** none; defer (small helper + `JiraPoll.run` change).

## 4. Worker-death retry replays non-idempotent calls — major

**What/where:** `PythonHelper.swift:171-195` `stopLocked` re-sends **every**
pending call once after a restart (`retry.attempts < 2`), including
`fileops.*` mutations (`pylib/helper/methods.py:442-517`) and `script.run`.

**Why wrong:** when the worker exits right after performing a side effect but
before the reply is ingested (termination handler races ahead of the stdout
read), the retry repeats it: duplicate file copy/trash, a second poll, a second
export. Nothing in `Pending` marks retryability.

**Fix:** add a retryability flag/allowlist (retry read-only methods only);
`script.run` and file-op mutations are never replayed. Test: kill the worker
mid-`fileops.duplicate` and assert one copy.

**Overlap:** none; defer.

## 5. Main-thread first-use helper calls with 10–120 s timeouts — major

**What/where:** `kitchen_sink.swift:238` `JiraStyle.fetch` (reached per-cell via
`jiraCellStyle`; 30 s), `:601`/`:628` `ListColumn.parse/serialize` (config load,
board/dashboard open; 30 s), `:9290` `JiraPaths` static let (first access
anywhere; 30 s), `:9500`/`:9526` `baseFieldLabels`/`fieldLabels` (list window
open; 30 s), `JiraSearch.swift:50` directory (60 s), `Confluence.swift:1316`
`rate_limit` on **every** API response, `ConfigText.swift:40/104/116` decode/tri/
resolveBinary on config load.

**Why wrong:** PLAN-py-core's hot boundary says "no new blocking helper calls on
the main thread on interactive paths". These are cached, but the first call per
config generation happens while the popup opens; if the pool is busy (finding 3)
or the worker is starting up, the UI hangs for the timeout and then caches the
failure (finding 6).

**Fix:** prewarm `jira.style`/`jira.paths`/labels/directory with
`pythonHelper.call` at launch/config reload and keep last-good; cap synchronous
call sites to a couple of seconds and treat failure as "not ready" (retry
later), never callSync from draw/render.

**Overlap:** none; defer, but touches the same call sites as finding 6.

## 6. Silent empty fallbacks + cached failures; deleted Swift mirrors leave no behaviour — major

**What/where:** `JiraStyle.fetch` caches `[:]` on failure; `baseLabelsCache`
caches `{}`; `ConfigText.swift:110` memoises failure as `NSNull` forever;
`JiraSearch.swift:53-58` caches an empty `JiraDirectory`; `JiraPaths` fallback is
all-`""` — `cacheFile` then appends to `""`, i.e. cwd-relative paths;
`ListColumn.parse` → `[]` (empty tables); `JiraSetupWindow.parseProjectKeys` →
`([], [])`; board columns guard-return leaves the old board; `wsLog` is the only
signal.

**Why wrong:** the ports deleted the Swift copies, so the fallback is no longer
a mirror — it is silently degraded behaviour, and several sites make a transient
startup failure permanent (never retried). `configSettingText` failing means a
settings toggle is silently not saved.

**Fix:** (a) never memoise a failure — retry on next call; (b) give `JiraPaths` an
explicit failed state that disables Jira with a visible message (never write
relative); (c) surface "python helper unavailable" once (Setup row / toast),
not only in the log.

**Overlap:** data plan risk 1 / X1 for (a); (b) and (c) separate. Fold (a) into
the data sweep for the affected loaders; the rest deferred.

## 7. `compare.new` handles are never dropped — major

**What/where:** `CompareText.swift:179-201` creates a handle per `TextCompare`;
`pylib/helper/methods.py:50-62` stores full models in `_COMPARE_MODELS`; there is
no `compare.drop` and no Swift `close()`/`deinit`; `CompareWindow.swift:977/1021/1032/1044`
replaces models freely.

**Why wrong:** every compare session/file load leaks the full left/right text,
rows, sections inside the long-lived worker; repeated use grows the helper
unboundedly until restart.

**Fix:** add `compare.drop` + `TextCompare.close()`, call it when a session's
model is replaced and on window close; Python test asserts the handle is gone.

**Overlap:** none; defer (one method + call sites + test).

## 8. Eager import graph: one bad module/JSON bricks every method, and stderr is discarded — major

**What/where:** `pylib/helper/methods.py:1-31` imports all ~25 modules;
`pylib/jira_fields.py:12-18` and `pylib/jira_paths.py:11-12` hard-open files at
import; `PythonHelper.swift:145` sends the worker's stderr to `nullDevice`, so the
app only ever sees "helper exited". `__main__.py:31` also raises `IndexError` for
a trailing `--params`.

**Why wrong:** a missing/malformed JSON (or one 3.11-incompatible module) kills
`ping` and every method, with zero diagnostics anywhere (writing is swallowed,
fallbacks are empty per finding 6). Startup cost itself is fine (~66 ms measured).

**Fix:** lazy per-method imports (`importlib` + typed error) or per-module
try/except; keep the last N stderr lines and append them to "helper exited";
bounds-check `--params`.

**Overlap:** data plan risk 1 + X1 — fold the lazy-import part; the stderr capture
and `--params` check can ride along in the same checkpoint.

## 9. Duplicated rules left behind beyond the plans — minor

**What/where:** `JiraTicket.swift:7-21` `category()` re-implements
`pylib/jira_data.py:53-68`; `pylib/jira_boards.py:63` hardcodes
`/highest|high|critical|blocker/i` for "hot" cards while
`priority-urgent-words` is configurable; `CompareText.swift:9-22` (EOL labels,
encoding display names) vs `pylib/compare_text.py:4-5`; `PathShelf.swift:28`
`maxLimit = 25` vs `shelf.MAX_LIMIT`.

**Why wrong:** these tables/rules exist twice with no parity check; the data plan
does not list any of them (it lists `category_names`, not the category
algorithm).

**Fix:** return an explicit category-rule table plus priority "hot" words in the
`jira.style`/board payloads (hot path keeps evaluating the mirrored table); read
EOL/encoding labels from the P8 JSON; derive the Swift shelf cap from the helper.

**Overlap:** J1/J2/J3, P4, P8 — fold.

## 10. Test-architecture gaps — minor

**What/where:** (a) `bin/run-tests.sh` `all` loops only `Tests/test_*.swift`, so
all newly ported Python suites are invisible to the default gate; (b) Swift
suites that now need the helper (`test_screenshot.swift`, `test_ansi_render.swift`,
`test_path_shelf.swift`, `test_python_helper.swift`) are not version-gated, so a
missing python reads as a Swift failure; (c) `test_python_helper.swift` has no
timeout case and never exercises `script.run`, though the plan promised both.

**Fix:** extract the python discovery from the `helper)` case into a function and
run it over `Tests/test_*.py` in `all`; print a clear "needs python 3.11+" skip/error
for the helper-dependent Swift suites; add the client timeout + `script.run` cases.

**Overlap:** data plan's Python-sweep note — fold (a) into X1; (b)/(c) separate.

## 11. Path shelf: per-event IPC, no batching — minor

**What/where:** `PathShelf.swift:66-72` `observe` → `canonical` callSync +
`IgnoreRules.ignored` callSync + `bump` callSync per kept file event
(`RecentFiles.shared.onKept`, `kitchen_sink.swift:5643`).

**Why wrong:** the plan says recent-file "batches are forwarded as data"; here up
to 3 round trips per FSEvent, and the path-shelf queue backs up unboundedly if
the helper is slow.

**Fix:** add `shelf.bump_many` and coalesce events over a ~0.2 s window; memo
canonical/ignore per path within the window. Test with a burst of events.

**Overlap:** P4 — fold.

## 12. `status.py` bypasses the helper's interpreter — minor

**What/where:** `pylib/status.py:89` runs the notify script via
`/usr/bin/env python3`; `script_bridge.py:40` correctly uses `sys.executable`.

**Why wrong:** contradicts the locked "scripts run with the helper's own
discovered interpreter, so versions cannot drift" — with `WS_PYTHON` set and no
brew python on PATH this resolves the CLT stub / wrong version.

**Fix:** use `sys.executable, "-B"` in `read_unread`; covered by
`Tests/test_status.py`.

**Overlap:** none; defer (one line).

## 13. `script.run` changed the child cwd — minor

**What/where:** old `JiraPoll.run` spawned via `runProcess` without setting a
working directory (app cwd); the new `pylib/helper/script_bridge.py:42` sets
`cwd=folder` (`jira/`, `confluence/`). Not mentioned in the plan's "same contract"
claim.

**Why wrong:** any script relying on relative paths behaves differently —
unverified. Fix: confirm all asset scripts use absolute paths from `jira_paths`,
then either match the old cwd or document the new contract in the bridge.

**Overlap:** none; same commit as finding 3's script-bridge edits.

---

## Examined and fine

- Helper protocol: stdout lock, out-of-order replies, EOF/shutdown drain;
  `Tests/test_helper.py` covers interleaving, error isolation, drain, one-shot
  modes.
- Concurrent access to shared Python state: compare-handle allocation,
  comment index, directory cache are all locked; other modules are pure.
- Deployment of new files is safe by construction (`RESOURCE_LINK_DIRS`,
  `git ls-files`), `-B`/`PYTHONDONTWRITEBYTECODE` preserved for worker and
  children; no bundler/signing change needed.
- Swift client core: single serial queue, id matching, restart semantics,
  semaphore memory safety; `callSync` delivery contract documented.
- Async conversions that were done well: ticket page, comments, board columns,
  Confluence preview — completions on main with generation/staleness guards.
- Hot paths kept in Swift as locked: ring/args mirrors, `JiraStyle` local
  evaluation, pixelate/render pipelines, FSEvents pump/Spotlight/xattrs.
- Settings-hub JSON extraction (653bd4b): tables one-home, `hub_defaults.json`
  soft-load under `[settings-hub]`; remaining gaps are the data plan's S1.
- Pylib module homes/naming and `__file__`-relative loads match the data plan's
  established pattern; the plan's inventory already covers the Swift table
  mirrors (`ShotTool`, `AnsiTheme`, `PaneShotConfig`, `category_names`, etc.).
- Ported suite case counts look like supersets (compare 467→467, file-ops
  142→237, ai 92→212); the three unwired suites (`test_jira_poll`,
  `test_confluence`, `test_notifications`) pre-date this work and are flagged by
  the data plan.
