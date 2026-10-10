# Refactor plan: Python core extraction — port everything but UI, platform and hot paths

Local plan; the GitHub issue was skipped by request (same as the previous
plan — no GitHub login). This file is the record. Owner decisions are locked
below.

## Progress

- [x] Phase A — one integration surface: helper gains concurrent dispatch + a
      script bridge; every JiraPoll/ConfluenceAPI spawn goes through it
      (long jobs — setup/rebuild — keep spawning, per the decision)
- [ ] Phase B — Jira/Confluence glue moved to Python (paths, categories,
      comments, ticket/board pages, directory, search, dashboard, configs)
- [ ] Phase C — test-backed islands (screenshot model pieces, ANSI, pane-shot,
      recent files, path shelf, compare sessions, setup checks)
- [ ] Phase D — config validation + typed parsing single-homed in Python
- [ ] Phase E — theme/colour algebra
- [ ] Phase F — bridge finish, leftovers, docs, final boundary

## Problem Statement

The app is ~41k lines of Swift (~2.3k more in Swift tests). The previous
refactor moved cold-path logic into a persistent Python worker and Python
already owns the Jira/Confluence/notify tooling and the settings hub. But a
large amount of non-UI logic still lives in Swift — and much of it is
duplicated with Python that already exists: Jira config parsing, status
mapping, search-criteria and CQL/JQL building, auth derivation, config
validation, markdown/HTML rendering, option shaping.

The duplication is a standing correctness risk (two copies drift, parity is
caught by eye), the Swift copies can't be tested without compiling the app,
and none of it is reachable from anything but AppKit — blocking the owner's
goal of a reusable, cross-platform core.

Owner goal, in their words: port as much code as possible from Swift to
Python for cross-platform support and long-term maintainability. UI and
performance-sensitive items are the acknowledged exceptions.

## Solution

Continue the established pattern, widened: Python (through the existing
persistent helper) becomes the home for every piece of logic that is
(a) not UI/AppKit, (b) not macOS-platform-bound, (c) not on a per-keystroke /
per-frame / interactive-latency path.

Two structural moves make this safe:

1. **The helper becomes the single integration surface.** Today the app talks
   to Python two ways: the JSON-lines worker (cold-path logic) and one-shot
   script spawns (JiraPoll.run / ConfluenceAPI.run, ~190 call sites). The
   script spawns move behind the helper without touching call sites. The
   helper becomes concurrent first, so a slow script call cannot stall other
   requests (status gather, timestamp, compare calls).
2. **Python becomes authoritative wherever both copies exist.** For every
   duplicated behaviour, the Python copy is the implementation; the Swift
   copy is deleted. New logic lands in the existing Python library; the
   jira/confluence CLI scripts keep their identities and import shared pieces
   where duplication is removed.

Each module follows the previous plan's safe sequence: add the Python module
+ tests → switch the app call sites → delete the Swift copy + its Swift test
(the Python tests are the tests now). Every commit leaves the app working.

## Commits

Phase A — one integration surface (no behavior change):

1. Helper concurrent dispatch: serve requests on a bounded worker pool,
   write replies under a stdout lock, echo ids, allow out-of-order
   completion, drain in-flight requests on shutdown. Helper tests: slow and
   fast calls interleaved, error isolation, shutdown mid-flight, one-shot
   modes unchanged. (The Swift client already matches by id and supports
   several calls in flight; the helper-client suite gains a concurrency
   parity case.)
2. Script bridge in the helper: a method that runs an asset script
   (folder + name + args + stdin) with the helper's own interpreter, captures
   stdout/stderr/exit code with the same PATH env prep as today. Helper tests
   drive a fixture script (stdin echo, failure code, missing script, big
   output).
3. Route the app's script bridge (JiraPoll.run and its ConfluenceAPI wrapper)
   through the helper internally: same Swift API, same background-queue /
   completion-on-main semantics, all call sites untouched. Verified by the
   existing suites plus a protocol test with several script calls in flight
   alongside other helper methods.

Phase B — Jira/Confluence glue (dedup; Python authoritative):

4. Jira path resolution: one implementation (Python, honouring the same env
   overrides and ~ expansion); Swift mirror deleted; Python tests for
   override precedence and defaults.
5. Status category + config word lists + workflow parsing → Python; the
   Swift cell/ticket call sites switch; Swift copies deleted; Python tests
   including the word-fallback path.
6. Ticket comments: Python reads + parses the poller's issue cache with
   mtime-based caching and returns rows; Swift asks the helper; the Swift
   index is deleted; Python tests.
7. Ticket page HTML (page template, comments HTML, date formatting) →
   Python template; the Swift view host stays; Swift template deleted;
   Python tests render fixture tickets and assert structure + escaping.
8. Jira directory: load + option shaping (projects / users / statuses /
   labels / versions / fields, folding, ordering) → Python; Swift pickers
   consume the shaped options; Python tests on directory fixtures.
9. Search panel: criteria assembly + filter-field menu model → Python; the
   search run keeps its current call; Python tests (keys, aliases, limits).
10. Dashboard: poll-job draft validation + payload building → Python
    (delegating to the same module the jira config script uses); Swift draft
    builder deleted; Python tests.
11. Dashboard: team/user/field definition edits (payload building, id
    normalisation, alias derivation, default checks) → Python; Python tests.
12. Dashboard: progress/overview/header and definitions-table text builders
    → Python; Swift copies deleted; Python tests.
13. Board: column/card mapping + board page HTML → Python; Python tests
    using the existing boards/directory fixtures.
14. Shared Jira config pieces (column-spec parse/serialize, base field
    labels, label helpers): canonical copy moves to the Python library, the
    CLI scripts import it; Swift mirrors deleted; Python tests port the
    existing parse cases and add round-trips.
15. Setup window project-key parsing → Python; Python test.
16. Confluence auth-header derivation + request cooldown state → Python
    single source (the image loader asks the helper for the header); Swift
    copies deleted; Python tests.
17. Confluence search criteria assembly → Python (same module that builds
    the CQL); Python tests.
18. Confluence preview assembly (URL absolutisation, wsconf rewriting,
    preview template, highlight JS, header/meta text) → Python; the custom
    scheme handler stays; Python tests for rewrites and injection.
19. Phase cleanup: remaining Jira/Confluence/kitchen-sink glue trimmed where
    the ports landed.

Phase C — test-backed islands (port suites case-for-case):

20. Screenshot model (part 1): Python module for tool specs + button ring,
    geometry/snap/arrow, files, args, state, and the pixelate grid maths
    (fed sampled edge bands from Swift). Port the matching cases of the
    Swift screenshot suite to Python; green.
21. Screenshot model (part 2): Swift switches to the helper for those
    pieces (render reads returned geometry), Swift copies deleted; the
    interactive document/undo core and all CoreGraphics/CoreText/image
    rendering stay Swift, and the Swift suite keeps the document/render/
    colour/OCR cases.
22. ANSI render: parser/grid/themes → Python; the Swift renderer consumes
    the grid; port the parser cases to Python; the CoreText/pixel cases
    stay Swift.
23. Pane-shot: args/config/Herdr JSON handling/line cap/env sanitising →
    Python; port its cases; process spawning stays Swift.
24. Recent files: model/store/event pairing/canonicalisation/snapshot →
    Python; the FSEvents pump, Spotlight seed, xattr origin reads and
    change notifications stay Swift; port the non-live suite cases to
    Python; the live FSEvents test stays Swift.
25. Path shelf: the store (JSON persistence, dedup, sort, canonical,
    normalise) → Python; clipboard timer and UI stay Swift; port the store
    cases.
26. Compare sessions: recent-files JSON store + session persistence and
    recovery → Python; new Python tests; the window switches.
27. Setup checks: preflight JSON → checks model, summary, fix titles →
    Python; the window renders results; new Python tests.

Phase D — config and validation:

28. Config validation, schema and the config-check behaviour → Python
    single home; the app's CLI modes delegate; Python tests; the settings
    hub's contract stays byte-compatible.
29. Tri-state parsing + binary resolution → Python; Swift shims; port the
    Swift config test cases to Python.
30. Typed config parsing (app settings, commands, shortcuts, theme keys) →
    Python returning values; Swift structs become mirrors filled from those
    values; the hotkey fast path still only reads cached fields; Python
    tests for csv/num/hex/marker/derived-path semantics.

Phase E — theme and colour algebra:

31. Colour algebra (opaque/luminance/contrast/readable, derived shades,
    tone mapping) + built-in theme presets + ANSI/Vim palette derivations →
    Python returning components; Swift builds NSColor; Python tests for
    contrast invariants, preset parsing and derivation.

Phase F — wrap-up:

32. Structured methods replace the generic script calls where the ports
    landed; retire the generic bridge if it has no users; delete JiraPoll
    leftovers.
33. Prose fallback renderer: delete if unreachable (preferred), otherwise
    port; the prose process/CLI path stays Swift.
34. Docs: AGENT_CONTEXT (helper concurrency + script bridge, new modules,
    suite list, validation home, the final Swift boundary), README check,
    plan progress closed.

## Decision Document

- **Integration**: one persistent Python worker, JSON-lines over
  stdin/stdout. The helper becomes concurrent: bounded worker threads,
  replies keyed by id written under a stdout lock, out-of-order completion,
  graceful drain on shutdown. Not a socket, not a new daemon.
- **Script bridge**: a generic helper method runs asset scripts as
  subprocesses (explicit folder, args, stdin; returns code/stdout/stderr).
  The interpreter is the helper's own discovered Python (>= 3.11), so script
  and worker versions cannot drift. The app's script bridge switches to it
  internally — all ~190 call sites unchanged.
- **Long jobs**: Jira setup/rebuild/cancel keep their current spawn-based
  flow in this pass; migrating them onto the helper's surface is a later,
  separate step.
- **Homes**: new shared logic lives in the existing Python library (the
  installer already links it into the bundle). Jira/Confluence CLI scripts
  keep their identities and import shared pieces from the library where
  duplication is removed. No installer, launchd, DMG or symlink changes.
- **Hot boundary** (owner rule): anything hit per keystroke or animation
  frame stays Swift — popup filtering/row building, vim keys/search, pane
  geometry application, file-browser filtering, per-key find bars, sidebar
  and favourite filters, and all drawing paths. No new blocking helper
  calls on the main thread on interactive paths; the two blocking call
  sites found during the audit (folder counts per cursor key, compare
  marks inside draw on cache miss) must not get worse and get a client-side
  memo while we are in the area.
- **Presentation** (owner call): HTML/text presentation code moves too —
  ticket page, board page, Confluence preview and meta lines, status
  summaries. View hosting (web views, text views) stays Swift.
- **Typed config parsing** (owner call): parse in Python, Swift structs
  remain as familiar mirrors filled from returned values. The hotkey fast
  path continues to read only the cached fields.
- **Validation**: one home, Python. The app's `config-check`/`config-schema`
  CLI modes delegate through the helper; the settings hub's existing
  "shell into the app" contract stays byte-compatible.
- **Screenshot model**: pure cold pieces move (tool specs, ring, geometry,
  snap, files, args, state, pixelate grid maths from sampled bands). The
  interactive document/undo core and every CoreGraphics/CoreText/image
  pipeline stay Swift in this pass; a latency benchmark decides whether the
  document core ever follows — it is explicitly not required by this plan.
- **Recent files**: Python owns the model, store, event pairing and
  canonicalisation; the FSEvents pump, Spotlight seed, xattr origin reads
  and UI notifications stay Swift. Batches are forwarded as data; snapshots
  come back.
- **Small helpers** (owner call, stay Swift): path expansion, timestamps,
  dismissed-notes store, note file helpers, file preview text, ages and
  rename validation.
- **Python runtime**: unchanged discovery (WS_PYTHON, brew paths, PATH) and
  version floor. No bundling in this pass.
- **Commit rule**: additions land before deletions; every commit leaves the
  app building and working; deletions happen only after the switch is in
  place.

## Testing Decisions

- A good test here exercises external behaviour: request in, response out.
  Moved code is tested in Python only — no implementation-detail tests, no
  source greps.
- Existing Swift suites are ported case-for-case to Python before their
  Swift implementation is deleted (screenshot pieces, ANSI parser,
  pane-shot, recent files, path shelf, config tri/binary). A suite counts
  as ported only when every case is covered.
- Previously untested logic that moves (Jira/Confluence glue, dashboard,
  pages, validation, typed parsing, colours, sessions, setup checks) gets
  new Python tests written against its public behaviour as part of the move.
- The helper's protocol tests gain concurrency and script-bridge cases
  (interleaving, error isolation, shutdown drain, fixture scripts).
- Swift keeps only facade/protocol/platform suites and the UI suites; new
  Python suites are wired into the repo's test runner one per phase.
- Where two implementations must briefly coexist (column parsing, config
  validation), a temporary parity check compares outputs before the Swift
  copy is deleted, mirroring the commands.toml codec approach.

## Out of Scope

- All AppKit/UI: popup framework drawing and key handling, window chrome,
  menus, terminal/SwiftTerm, screen capture, overlay, pinning, OCR/speech,
  AX/AeroSpace/window management, hotkeys/sockets/TCC/installer/launchd.
- FSEvents, Spotlight, xattr origin reads (the pump side of recent files).
- Hot paths: popup filter/row building, vim keys/search, pane geometry
  application, file-browser filtering, per-key find/filters, compare marks
  render memo, pixelate/text/render pipelines.
- Long-running Jira job migration (later step).
- Repository/package reorganisation, runtime bundling, distribution work.
- The settings hub, jira, confluence and notify CLIs' own behaviour.
- Small helpers per owner call: path expansion, compact timestamps,
  dismissed notes, note helpers, file preview text, ages, rename validation.
- The screenshot interactive document core (benchmark-gated, later).
- Optional facade-mirror trimming in the compare view (not required).

## Further Notes

- The helper must never write anything but protocol lines to stdout; the
  script bridge captures child output and must not leak it.
- Concurrent dispatch needs a stdout lock; JSON writes are single lines.
- The helper inherits PYTHONDONTWRITEBYTECODE and WS_COMMANDS_CONF today —
  the script bridge must preserve both for children.
- Audit findings worth fixing opportunistically while in the area:
  CompareFolderView issuing a blocking folder-counts helper call per cursor
  key on the main thread, and ComparePane calling compare marks inside draw
  on memo misses.
- At wrap-up, AGENT_CONTEXT gets the final boundary: exactly what remains
  Swift and why (UI, platform, hot), plus the port map so future work can
  see the shape at a glance.
