# Plan: one JSON data manager — `pylib/jsonmgr.py`

Owner mandate: **ONE unified JSON data manager that registers and manages ALL
loading and field access of the shipped JSON homes** — so we can see which
module loads/accesses which file or field, and the 28 hand-rolled
`open(...) + json.load` sites collapse to one implementation.

Base: `eabf8ac` (clean tree). Sibling record: `PLAN-data-out-of-code.md`
(the sweep that created the 15 homes — this plan routes them through one
manager; it is a follow-on, not a re-plan of the sweep).

Read with: `REVIEW-architecture.md` #6a (never cache a failure), #8
(import-time coupling), `PLAN-py-core.md` (contexts, hot boundary).

---

## 1. Inventory — every shipped JSON home, loader and consumer

"Loader" = the code that opens the file today. All loaders except
`settings_hub/paths.py` are hard `with open(...) as fh: json.load(fh)` at
import (a missing/corrupt file raises `FileNotFoundError` /
`JSONDecodeError` at import). In the worker those imports are per-module
guarded (`pylib/helper/methods.py:13-55` — was ~~9-54~~; commit `80101b9`): a bad module's
group answers `HelperError … unavailable`, `ping` stays up. In the CLIs a bad
file dies with a traceback at process start.

Py = Python suite(s) that guard the values; all suites assert **through the
old symbol names**, so `jsonmgr.load()` must keep values byte-identical.
`./ws test helper` (worker imports every pylib module + dispatches methods)
is a guard for every pylib row and is not repeated below.

### 1a. pylib — module-data homes (one loader each)

| JSON home | Loader | Module-level names kept | Other consumers | Today's error handling | Guards |
| --- | --- | --- | --- | --- | --- |
| `pylib/paths.json` | `pylib/jira_paths.py:10-12` (env override per key) | `CONFIG_JSON`, `TEAM_JSON`, `LEGACY_CONFIG`, `CACHE_DIR`, `OUT_DIR_DEFAULT`, `CACHE`, `TABS`, `SIDE_DIRS`, `cache_file()` | `jira_config`, `jira_api`, `jira_poll`, `jira_log`, worker `jira.paths` method | hard import failure | `test_jira_poll.py`; `test_helper.py` |
| `pylib/shot_model.json` | `pylib/shot_model.py:19-35` | `SYMBOLS`, `TOOLTIPS`, `LETTERS`, `DEFAULT_SIZES`, `FINISHES`, `DEFAULT_BUTTONS`, `ALL_TOOLS`, `DRAW_KINDS` (+ `_TOOLS`, `Args`/`State` defaults) | worker `shot.*` methods; Swift `ShotTool` is a mirror (SW1) | hard import failure | `test_shot_model.py` (`ws test shot-model`) |
| `pylib/paneshot.json` | `pylib/paneshot.py:13-16` | `USAGE`, `MAX_LINES`, `Config` defaults | worker `pane-shot` arg/config methods; Swift `PaneShotConfig` mirror | hard import failure | `test_paneshot.py` |
| `pylib/ansi.json` | `pylib/ansi.py:16-19` | `Theme` defaults, base-16 palette (`_BASE16`) | worker `ansi.*` methods; Swift `AnsiTheme` mirror (SW2) | hard import failure | `test_ansi.py` (`ansi-parse`) |
| `pylib/shelf.json` | `pylib/shelf.py:11-17` | `MAX_LIMIT`, `WHYS`, `ACTIVITY_WHYS` | worker `shelf.*` (`methods.py:985,1001`); Swift `PathShelf.maxLimit` mirror | hard import failure | `test_shelf.py`; `./ws test paths` |
| `pylib/prose_pdf.json` | `pylib/prose_pdf.py:11-20` | `DEFAULTS` (cacheDir expanded), `SOURCEPOS_FILTER`, `BUILTIN_CSS` | worker `prose.*` methods; `test_prose_pdf.swift` via helper | hard import failure | `test_prose_pdf.py`; `./ws test prose` |
| `pylib/doc_templates.json` | `pylib/doc_templates.py:7-12` | `BUILTIN` | worker `doc_templates.*` methods | hard import failure | `test_doc_templates.py` |
| `pylib/ai_format.json` | `pylib/ai_format.py:9-16` | `CODE_INSTRUCTION`, `FILLER` | worker `ai.*` methods | hard import failure | `test_ai_format.py` |
| `pylib/compare_text.json` | `pylib/compare_text.py:8-14` | `EOL_LABEL`, `ENCODINGS` | worker `compare.*` methods | hard import failure | `test_compare.py` (`ws test compare`) |

### 1b. jira — one home, ten loaders

| JSON home | Loader | Names kept (selection) | Consumers | Today's error handling | Guards |
| --- | --- | --- | --- | --- | --- |
| `jira/defaults.json` | `jira/jira_config.py:102-124` (+`:509` criteria) | `DEFAULTS`, `DEFAULT_ENDPOINTS`, `API_BASE`, `AGILE_BASE`, `CLOUD_SEARCH`, `DEFAULT_SEARCH`, `DEFAULT_JQL_TEMPLATES`, `FIELD_SOURCES`, `*_KEYS`, `TEAM_KEYS`, `_CRITERIA` | `jira_api` / `jira_poll` / `jira_log` + `confluence_config` / `notify_poll` (all import it) | hard import failure | `python3 Tests/test_jira_poll.py`; `jira-setup`, `jira-dashboard` |
| | `jira/jira_api.py:99-140` | `RETRY_CODES`, `TRANSIENT_CURL`, `SHRINK_*`, `MIN_PAGE`, `MAX_*`, `AUTH_401_BACKOFF`, `BACKOFF_CAP`, `PAGE_*`, `QUERY_FIELDS`, `CURL_LOG_MAX` | `jira_poll` + `confluence_api` (import it) | hard import failure | `test_jira_poll.py` |
| | `jira/jira_poll.py:153-165`, `:1580`, `:2125` | `POLL_LOG_MAX`, `ERROR_RETRY`, `_POLL_DEFAULTS`, `SPRINT_RANK`, `MY_WORK`, `_WATCHING` | launchd poller; app `JiraPoll.run` (script bridge) | hard import failure | `test_jira_poll.py` |
| | `jira/jira_log.py:42-48` | `DEBUG_LOG_MAX`, `DEBUG_LOG_BACKUPS` | `jira_api`, `jira_poll`, `confluence_api` (import it) | hard import failure | `test_jira_poll.py` |
| | `pylib/jira_data.py:16-26` | `WORDS_DEFAULTS`, `_LIST_KEYS`, `DIM_FIELDS` | `jira_directory` / `jira_pages` / `jira_boards` (import it); worker `jira.style`, `jira.comments` | hard import failure (guarded group) | `test_jira_data.py`, `test_helper.py` |
| | `pylib/jira_pages.py:20-25`, `:87` | `CATEGORY_NAMES`, `_HEX_KEYS` | `jira_boards` / `jira_directory` (import it); worker `jira.ticket_html` | hard import failure | `test_jira_pages.py` |
| | `pylib/jira_boards.py:16-23`, `:85` | `DONE_LIMIT`, `OTHER_COLUMN`, `_HOT_PATTERN`, `_COLOR_KEYS` | worker `jira.board_columns`, `jira.board_page` | hard import failure | `test_jira_boards.py` |
| | `pylib/jira_directory.py:18-26` | `TOP_N` | worker `jira.directory`, `jira.options` | hard import failure | `test_jira_directory.py` |
| | `pylib/jira_search.py:10-13`, `:33` | `SKIP_CATALOG_FIELDS`, `_SEARCH_UI` | worker `jira.filter_kinds`, `jira.criteria` | hard import failure | `test_jira_search.py` |
| | `pylib/jira_fields.py:15-18` | `BASE_FIELD_LABELS` (`from jira_fields import BASE_FIELD_LABELS` at `jira_config.py:362`) | `jira_config` (from-import); worker `jira.field_labels`, `jira.columns_*` | hard import failure | `test_jira_fields.py` |

### 1c. confluence / notify

| JSON home | Loader | Names kept | Consumers | Today's error handling | Guards |
| --- | --- | --- | --- | --- | --- |
| `confluence/defaults.json` | `confluence/confluence_config.py:48-54` | `TYPES`, `MODES`, `MODIFIED`, `DEFAULTS` | `confluence_api` + the confluence CLI (import it) | hard import failure | `python3 Tests/test_confluence.py` |
| | `confluence/confluence_api.py:57-66`, `:356` | `SEARCH_EXPAND`, `FAV_KEYS` | confluence CLI | hard import failure | `test_confluence.py` |
| | `pylib/confluence_glue.py:12-17`, `:42-46` | `MODES`, `_SEARCH`, rate-limit gate numbers | worker `confluence.auth`, `confluence.rate_limit`, `confluence.criteria` | hard import failure | `test_confluence_glue.py` |
| | `pylib/confluence_pages.py:105-112` | `_COLOR_KEYS` | worker `confluence.preview_html` | hard import failure | `test_confluence_pages.py` |
| `notify/defaults.json` | `notify/notify_poll.py:41-46` | `DEFAULTS`, `API_SOURCES` | `pylib/status.py` (runs `--json` for the Hyper+S status row); notify CLI | hard import failure | `python3 Tests/test_notifications.py`; `test_status.py` (invokes the script) |
| | `notify/webex_api.py:39-46` | `API`, `SCOPES`, `DEFAULT_PORT`, `MAX_RETRY_WAIT`, `LOGIN_HINT` | `notify_poll` (`--poll` subprocess), `--login` | hard import failure | `test_notifications.py` |

### 1d. settings_hub

| JSON home | Loader | Names kept | Consumers | Today's error handling | Guards |
| --- | --- | --- | --- | --- | --- |
| `settings_hub/data/tables.json` | `settings_hub/tables.py:9-32` | `LAYERS`, `LAYER_TITLE`, `LAYER_NAME`, `REBIND_LAYERS`, `MODS`, `MOD_*`, `LABEL_MOD`, `KEY_NAMES`, `ARROWS`, `VIM_*`, `AERO_KEY`, `HERDR_KEY`, `AERO_MOD`, `COLOR_KEYS`, `NOT_SETTINGS`, `APPLY`, `APP_LAUNCH_ONLY` | `model` / `export` / `chords` / `readers` / `rebind` / `settings` / `catalog` / `cli` / `tui/app` (`from .tables import …`) | hard import failure | `test_settings_hub.py` (`test_table_pins` + reader/TUI cases) |
| `settings_hub/data/hub_defaults.json` | `settings_hub/paths.py:19-24` | `HUB_DEFAULTS` (internal only, verified) | `paths.hub(key)` / `paths.hub_default(key)` callers (`catalog`, `cli`, `undo`, `tui/app`) | **soft**: `except (OSError, ValueError) → {}` | `test_settings_hub.py` |
| `settings_hub/data/nvim_lua.json` | `settings_hub/readers.py:251-255` | `_LUA` | `readers._vim_maps_nvim` (headless nvim query) | hard import failure | `test_settings_hub.py` (vim reader cases) |

### 1e. Counts and verification status

- **15 homes, 28 load sites, ~30 consumer modules.** `jira/defaults.json` is
  parsed 10× per process today; `confluence/defaults.json` 4×;
  `notify/defaults.json` 2×. The manager reduces each to one parse.
- Every home is loaded with `os.path.dirname(os.path.abspath(__file__))`
  (`+ "/.."` for the pylib→jira / pylib→confluence cross-reads) — cwd-proof,
  bundle-safe (`PLAN-data-out-of-code` risks 2–3 verified).
- Not homes (out of scope, different semantics — user/runtime data):
  `commands.toml`/TOML, `~/.config/jira/*`, `team.json`, `config.json`,
  caches/state/undo/favorites/geometry/compare-recent, `shot_model`
  state file, `paneshot` herdr envelope, notify state. Also Swift-side reads:
  `vim/snippets/markdown.json` (kitchen_sink.swift:7142) and the Resources
  mirrors (SW1/SW2) — this manager is Python-only.
- Unregistered JSON in the shipped dirs: `jira/team.example.json` (docs
  example, never loaded by code) and `vim/snippets/markdown.json` (Swift-side
  snippet template, `kitchen_sink.swift:7142`; shipped via `RESOURCE_LINK_DIRS`)
  — was ~~only team.example.json~~; both allowlisted by the registry test, to
  match §7's "15 homes + 2 non-homes".

## 2. Proposed design

### 2.1 Home and shape

- **Module:** `pylib/jsonmgr.py`. Stdlib-only, 3.9-compatible, **no imports
  of other pylib modules** (it must be safe to import from the worker's
  guarded import, the CLI scripts and settings_hub before any path
  bootstrap beyond the existing ones).
- **One instance:** a module is the singleton (`import jsonmgr` is one object
  per process in every context). No class/constructor, no threading of an
  instance through call sites.
- **Root resolution:** `_ROOT = dirname(dirname(abspath(__file__)))` — repo
  in repo mode, `Contents/Resources` in the app bundle, the home in app-home
  mode (all four are the same parent-of-pylib layout; symlinks are NOT
  resolved, so `home/jira/defaults.json` resolves through the home's links).
  `WS_JSON_ROOT` overrides for tests/throwaway installs (read per call).
- **Logical name = repo-relative path minus `.json`**:
  `load("jira/defaults")`, `field("pylib/shelf", "max_limit")`,
  `settings_hub/data/tables`. One name, one file, one report row.

### 2.2 Registry (static, explicit)

A single `_HOMES` dict in `jsonmgr.py` listing all 15 names (pre-seeded so no
consumer churn and so the report can speak about unimported/broken homes).
`register(name, path=None)` is the extension API for new homes: by default
`<root>/<name>.json`; a package may call it from the JSON's home module, but
the central table is authoritative (the report must work without importing
anything, incl. under 3.9).

### 2.3 API surface

```python
class JsonError(Exception):        # .name, .path, .kind ("missing" | "unreadable"
                                   #  | "malformed" | "shape"), .cause
def load(name, *, soft=False, refresh=False) -> dict
def get(name, *path, default=None)             # missing path -> default
def field(name, *path)                          # missing path -> JsonError, no default
def register(name, path=None)
def reload(name=None)                           # drop snapshot(s), re-stat+parse
def report(*, with_data=False) -> dict          # registry + snapshot + ledger + warnings
def ledger() -> list
def lazy_module(module_name, namespace, exports)  # wave M4, see 2.6
```

Usage at a loader site (the migration diff is 2–4 lines):

```python
# before
_DATA_FILE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "shelf.json")
with open(_DATA_FILE, encoding="utf-8") as _fh:
    _DATA = json.load(_fh)
MAX_LIMIT = _DATA["max_limit"]
# after
import jsonmgr
_DATA = jsonmgr.load("pylib/shelf")
MAX_LIMIT = _DATA["max_limit"]           # symbol name and lines below unchanged
```

`load` returns the cached object itself (no copy) — the structures are shared
and read-only; consumers that need to change values copy first (`jira_config`
`DEFAULTS`, `prose_pdf` `DEFAULTS` already do; verified no consumer mutates a
loaded dict today). `field`/`get` walk `*path` and are the per-key access
that the ledger records.

### 2.4 Provenance / insight

Three layers, cheapest first:

1. **Static registry** — `_HOMES` + the `load("…")` call at each loader site:
   the file's "home declaration" is the call itself (one name, no second
   path math). The report shows home → resolved path → exists/size/mtime/
   sha256/top-level keys/parse count/last error.
2. **Always-on ledger** — every `load`/`get`/`field` records
   `(consumer_module, home, "a.b.c" keypath, count, first_ts)` deduped under
   the lock. Caller module = first frame outside `jsonmgr` (`sys._getframe`).
   This is the "which module reads which file/field" answer, no env needed;
   bounded (4096 distinct tuples, report notes truncation).
3. **`WS_JSON_TRACE=1` (runtime trace)** — `load()` then returns a `dict`
   subclass that records each `_DATA["key"]` index (the eager modules' own
   key access, no rewrite needed) and writes one stderr line per first touch:
   `jsonmgr: jira_boards -> jira/defaults -> boards.done_limit`.
   Trace-off returns a plain dict. (Worker stderr is `nullDevice` per
   REVIEW #8 — for the app, use the helper method below or a traced CLI.)

**Report command for humans** (the CLI is the canonical surface; it works in
every context, needs no app):
`python3 pylib/jsonmgr.py --report [--json] [--check] [--root DIR]`
(`--check` parses every home and exits 1 on any hard failure — usable in a
future preflight). In app-home mode:
`python3 ~/.config/kitchen-sink/pylib/jsonmgr.py --report`.
`python3 -m pylib.jsonmgr` also works from the repo/bundle root — pylib has
no `__init__.py` but imports as a namespace package (verified:
`python3 -m pylib.jira_paths`); the file form is canonical because it is
cwd-independent. `python3 -m jsonmgr` works with `PYTHONPATH=pylib`.

**In the app:** add `jsonmgr.report` as a helper method (wave M5) returning
the same dict — that is where the *runtime* ledger of the long-lived worker
lives; the CLI's ledger only sees the CLI process. Not surfacing via
`ws doctor` (that script is jira-stack health); an optional one-line
`./ws jsons` alias can be added later.

### 2.5 Semantics

- **Path resolution per context** (all identical today; the manager keeps it):
  repo — `_ROOT` = checkout; app bundle — `_ROOT` = `Contents/Resources`
  (jira/confluence/notify/pylib/settings_hub all copied by
  `git ls-files`); app home — `_ROOT` = `~/.config/kitchen-sink` and each
  home dir is a symlink into the bundle (same parent-of-pylib layout);
  launchd/standalone jira scripts — they already insert `jira/` + `pylib/`
  on `sys.path`, so `import jsonmgr` resolves to whichever pylib they used,
  and its siblings are next to it. `WS_JSON_ROOT` (per call) exists only for
  tests/throwaway installs. No per-file env override (data is not user
  config; `WS_COMMANDS_CONF`-style layering stays where it belongs — the
  consumers' user config, which wins over the JSON as today).
- **mtime caching**: `_CACHE[name] = {path, key: (st_mtime_ns, st_size),
  data, parses}`. `load()` always stats and re-parses when the key changed
  (or `refresh=True`); `get`/`field`/lazy `__getattr__` use the snapshot **without
  a stat** once loaded (cold → one `load`). So: one parse per home per
  process, no per-access stat storms; edits to shipped JSON are picked up by
  the next `load()`/`reload()`/report `--check` in that process, and need a
  worker restart otherwise — exactly today's "loaded at import" behaviour.
- **Locking**: one `threading.RLock` guards `_CACHE`/`_LEDGER`/`_LAST_ERROR`
  and the parse (files are ≤ a few KB; a hit is ~1 µs). Ledger dedup is a
  plain dict check-then-set — was ~~(GIL-atomic; a duplicate insert is
  harmless)~~: check-then-set is *not* atomic; it is only safe because it
  runs under the same RLock (keep it there; a duplicate insert is harmless
  if it ever leaks).
  Lazy module attributes are computed idempotently and cached into the
  module dict (`ns[attr] = value`) — a double compute under race is
  harmless.
- **Failure behaviour (REVIEW #6a)**: failures are **never cached** — no
  `_CACHE` entry is written, a stale entry is dropped, and a `_LAST_ERROR`
  diagnostic is kept for the report only. Every later `load`/`field` retries.
  One error style: `JsonError` (not a `ValueError` subclass, so old
  `except ValueError` handlers cannot silently swallow it) with message
  `json data "jira/defaults" (/path): missing` / `…: malformed JSON: …`.
  `soft=True` (used only by `hub_defaults`) returns a shared `{}`, records a
  warning in the report, and still does not cache — the two
  `HUB_DEFAULTS.get` sites (`paths.hub`'s final fallback and
  `paths.hub_default`) move to the manager's soft accessor, so a corrupt file
  retries instead of pinning `{}` forever (its only consumer; `HUB_DEFAULTS`
  has no external references, verified).
- **3.9 compatibility**: `from __future__ import annotations`, no `match`,
  no `tomllib`, no `functools.cache`; only `json/os/sys/threading/hashlib/
  time` — was ~~time/expanduser~~ (`os.path.expanduser` is a function, not a
  module; `math.isfinite`, 3.2+, is fine if numeric validation is added but
  then `math` joins the list). The floor is checked for real: this machine's
  `/usr/bin/python3` (the CLT interpreter launchd scripts land on) is 3.9.6.
  The CLI report also runs on 3.9 (no settings_hub import).
- **Helper integration**: the `jsonmgr.report` method function belongs in
  `pylib/helper/methods.py` (the `_jira_paths` pattern at `:686`, which does
  `import jira_paths` inside the body) so `jsonmgr.py` keeps its
  no-pylib-imports rule and `python3 pylib/jsonmgr.py` / `-m pylib.jsonmgr`
  never import `helper`; also add `"jsonmgr": "jsonmgr"` to `_MODULES`
  (same guard style) so a broken jsonmgr answers `HelperError … unavailable`
  instead of killing the worker. Optionally convert `JsonError` to the
  message-only `HelperError` branch in `helper/__init__.dispatch` (M5
  polish) — until then the generic branch already surfaces the full message
  + traceback. (Corrected: the plan was silent on where the method body
  lives — a top-level `from helper import method` in jsonmgr would break the
  CLI and the no-pylib-import rule.)

### 2.6 Lazy loading — feasibility and the decision

The owner asked to prefer lazy loading keyed by registry entries (REVIEW #8)
while keeping eager module-level names.

- **A clean full-lazy pattern for module constants is not achievable
  because PEP 562 `__getattr__` only serves `module.NAME` and
  `from module import NAME` — a bare global lookup *inside the defining
  module* goes straight to `module.__dict__` and raises `NameError`.**
  Every loader module uses its own constants internally
  (1–4 sites each), so lazy names require per-module rewrites of those uses.
  Additionally `jira_config.py:362` does `from jira_fields import
  BASE_FIELD_LABELS` (a from-import forces the load) and the CLI modules'
  constant fan-out is large (`jira_config`'s `_D` feeds 18 module names).
- **Decision**: two waves.
  - M1–M3 (all homes): **eager-via-manager** — behaviour-identical, zero
    call-site churn, one implementation + error style + insight. Import
    failures stay import failures (guarded group in the worker; startup
    traceback in CLIs).
  - M4 (worker surface only): `jsonmgr.lazy_module(__name__, globals(),
    exports)` converts **15 of the 17 pylib loader modules** to lazy exports
    where each internal-use rewrite is 1–4 lines (`jsonmgr.field(...)`).
    `jira_fields.py` stays eager (the `from jira_fields import
    BASE_FIELD_LABELS` at `jira_config.py:362` would materialise it anyway)
    and `jira_paths.py` stays eager (its constants are path math + env
    overrides that sibling CLIs read at import; it is a table, not module
    data). Then a bad JSON fails **at first method call** and retries on the
    next one — the exact REVIEW #6a/#8 behaviour wanted in the long-lived
    worker. CLI packages (jira/confluence/notify/settings_hub) stay eager:
    one-shot processes already fail loud at start, and the rewrite there is
    not justified.
  - Lazy export spec: `{"DONE_LIMIT": ("jira/defaults", ("boards",
    "done_limit"))}` — `(home, path_tuple[, cast])`, or a callable for
    projections (`prose_pdf` `cacheDir` expands `~` on access);
    `lazy_module` installs `__getattr__`/`__all__`/`__dir__`, caches in the
    module namespace, never caches failure.
  - **Derived module tables become projections** (correction: the plan
    undercounted this). Every lazy module builds module-level derived
    constants at import (`ansi._BASE16`; `shot_model`'s ~12 names;
    `jira_boards.DONE_LIMIT/OTHER_COLUMN/_HOT_PATTERN/_COLOR_KEYS`;
    `jira_pages.CATEGORY_NAMES/_HEX_KEYS`; `jira_data.WORDS_DEFAULTS/
    _LIST_KEYS/DIM_FIELDS`; `confluence_glue.MODES/_SEARCH`;
    `shelf.MAX_LIMIT/WHYS/ACTIVITY_WHYS`; …). A module-level
    `jsonmgr.field(...)` there would load at import, so each becomes a
    callable projection in the export table plus internal-use rewrites.
    Verified no *def-time* capture — `paneshot.Config`, `shot_model.Args`/
    `State`, `ansi.Theme` all read at call time — so nothing is structurally
    un-lazyable; but `shot_model` (~10 projections + ~10 rewrites) should be
    declared eager-by-decision up front. Private test-visible names must be
    exported too: `Tests/test_jira_pages.py:21` reads `jp._HEX_KEYS`.

## 3. Migration batches (each tiny, ordered, guard-green, symbols kept)

`./ws test all` runs Swift + three direct Python suites; named suites below
are the individual gates. Every batch: old symbol names unchanged, values
byte-identical, `git status` clean after commit; new files tracked before any
`ws dmg` (bundle uses `git ls-files`).

| # | Batch | Files | Guard (expected green) |
| --- | --- | --- | --- |
| **M0** | manager + registry + report CLI + tests + runner case | `pylib/jsonmgr.py` (new), `Tests/test_jsonmgr.py` (new), `bin/run-tests.sh` (`jsonmgr)` case) | `./ws test jsonmgr`; `/usr/bin/python3 pylib/jsonmgr.py --report --check`; `./ws test all` unchanged |
| **M1** | pilot: first consumer | `pylib/compare_text.py` | `./ws test compare` + `./ws test helper` |
| **M2a** | pylib small data (1) | `pylib/shelf.py`, `doc_templates.py`, `ai_format.py` | `shelf`, `doc-templates`, `ai`, `helper` |
| **M2b** | pylib small data (2) | `pylib/paneshot.py`, `ansi.py`, `prose_pdf.py` | `paneshot`, `ansi-parse`, `prose-pdf`, `helper` |
| **M2c** | screenshot tables | `pylib/shot_model.py` | `shot-model`, `helper` |
| **M2d** | pylib jira glue (6 modules, one home) | `pylib/jira_data.py`, `jira_pages.py`, `jira_boards.py`, `jira_directory.py`, `jira_search.py`, `jira_fields.py` | `jira-data`, `jira-pages`, `jira-boards`, `jira-directory`, `jira-search`, `jira-fields`, `helper` |
| **M2e** | pylib confluence glue | `pylib/confluence_glue.py`, `confluence_pages.py` | `confluence-glue`, `confluence-pages`, `helper` |
| **M3a** | jira package | `jira/jira_config.py`, `jira_api.py`, `jira_poll.py`, `jira_log.py` (4 loaders, 1 home) | `python3 Tests/test_jira_poll.py`; `jira-setup`, `jira-dashboard` |
| **M3b** | confluence package | `confluence/confluence_config.py`, `confluence_api.py` | `python3 Tests/test_confluence.py` |
| **M3c** | notify package | `notify/notify_poll.py`, `webex_api.py` | `python3 Tests/test_notifications.py` |
| **M3d** | settings_hub | `settings_hub/tables.py`, `readers.py` (`_LUA`), `paths.py` (soft accessor; `from . import paths` bootstrap in `tables.py`) | `./ws test settings` |
| **M3e** | jira path table | `pylib/jira_paths.py` | `python3 Tests/test_jira_poll.py`, `Tests/test_confluence.py`, `Tests/test_notifications.py` (jira_paths is import-reached by both stacks via `jira_config`); `helper` |
| **M4a** | lazy: small data | `shelf.py`, `doc_templates.py`, `ai_format.py`, `compare_text.py`, `prose_pdf.py` | their suites + `helper` (incl. new bad-JSON laziness case) |
| **M4b** | lazy: pane-shot/ANSI | `paneshot.py`, `ansi.py` | `paneshot`, `ansi-parse`, `helper` |
| **M4c** | lazy: screenshot | `shot_model.py` | `shot-model`, `helper` |
| **M4d** | lazy: jira glue | `jira_data.py`, `jira_pages.py`, `jira_boards.py`, `jira_directory.py`, `jira_search.py` | `jira-data`, `jira-pages`, `jira-boards`, `jira-directory`, `jira-search`, `helper` |
| **M4e** | lazy: confluence glue | `confluence_glue.py`, `confluence_pages.py` | `confluence-glue`, `confluence-pages`, `helper` |
| **M5** | report surface | `pylib/helper/methods.py` (`jsonmgr` in `_MODULES` + `jsonmgr.report`), optional `bin/lib.sh` `ws jsons` one-liner | `./ws test helper`; dispatch the method and assert consumers in the report |
| **M6** | checkpoint + docs | `AGENT_CONTEXT.md` ("JSON data" note + Key files), this plan's progress log | `./ws test all`; `./bin/build-app.sh`; `/usr/bin/python3 pylib/jsonmgr.py --report --check` |

End state: **every shipped JSON home loads through `jsonmgr`**; worker-surface
modules are lazy/retryable; one error type; one report; no duplicated path
math or bare `except ValueError → {}` outside `soft=True` for `hub_defaults`.

### New test suite (`Tests/test_jsonmgr.py`, wave M0)

- registry integrity: every `_HOMES` entry exists and parses (dict root);
  scan shipped dirs for unregistered `*.json` (allowlist
  `jira/team.example.json` + `vim/snippets/markdown.json` — was ~~only
  team.example.json~~; §7's "15 homes + 2 non-homes" needs both, and
  `vim/snippets/markdown.json` is a shipped Swift-side template at
  `kitchen_sink.swift:7142`) — pins the end state.
- error style: `WS_JSON_ROOT` temp root → missing file raises `JsonError`
  (name/path in message); writing the file then calling again succeeds
  (failure not cached); malformed likewise.
- mtime: while the stat key is unchanged, plain `load` returns the same
  snapshot object; after an edit plain `load` re-parses and sees it (load
  always stats, §2.5 — was ~~plain `load` keeps the snapshot~~, which
  contradicted §2.5); `load(refresh=True)` forces a re-parse even when the
  key is unchanged; `reload()` drops the snapshot(s) and re-reads.
- ledger: `load`/`get`/`field` from a helper module record consumer+keypath;
  dedupe; `WS_JSON_TRACE=1` records `_DATA["key"]` indexing and emits lines.
- `lazy_module`: export resolves, caches, retries after a fixed failure,
  `dir()` includes names; internal bare use is a `NameError` (documented
  contract, asserted).
- threads: 8 threads × `field` on a cold home → one parse, consistent value.
- CLI: `--report --json` parses; `--check` exit codes; runs under
  `/usr/bin/python3` (3.9).

## 4. Risks and unknowns

1. **Shared objects vs today's per-loader copies.** Each module parsed its
   own dict; the manager shares one. Verified no consumer mutates a loaded
   dict (`prose_pdf` expands `cacheDir` after `dict(...)`; `jira_config`
   builds `DEFAULTS`/`SPRINT_RANK`/`MY_WORK` as copies) — re-verify in M1/M3a
   with a mutation grep (`\[.*\] *=|\.update\(|\.pop\(|\.clear\(`), and state
   "shared, read-only" in the manager docstring.
2. **Cross-package path roots.** Old pylib→jira loads used
   `dirname(__file__) + "/.."`; the manager uses its own `__file__` + name.
   Equivalent in repo/bundle/home; a pylib copy detached from its siblings
   would now fail differently — `WS_JSON_ROOT` covers tests. Verify with
   `test_install.sh`/dist bundle smoke in M6.
3. **Lazy wave latent NameError.** After `lazy_module`, a future author
   writing a bare global inside that module gets `NameError` at runtime.
   Mitigation: M4 batches list their internal uses; the module suites plus a
   `grep` for the converted names catch stragglers; if a module's diff grows
   past ~15 lines, record it as eager-by-decision instead (end state only
   requires manager routing, not laziness).
4. **`WS_JSON_TRACE` changes the returned object type** (dict subclass).
   Grep `type(x) is dict` — none today; trace-off path is plain. Verify
   before M0 lands the trace mode.
5. **Import-order bootstrap in settings_hub.** *Verified:* `readers.py:5`
   imports `.tables` before `readers.py:18` imports `.paths`, and
   `model.py:4` / `chords.py:5` / `settings.py:5` / `catalog.py:4` do the
   same, so a direct `settings_hub.readers`/`model` import reaches `.tables`
   first; `tables.py` must
   `from . import paths  # noqa: F401, sys.path bootstrap` before
   `import jsonmgr` (no cycle: `paths.py` inserts `<root>/pylib` and imports
   only `config_text` + `jsonmgr` after M3d). `cli.py` reaches `.paths`
   first via `apply.py:10`; the `tables.py` fix covers every other entry.
6. **`from jira_fields import BASE_FIELD_LABELS`** (`jira_config.py:362`)
   keeps `jira_fields` eager — accepted; documented so M4d doesn't "fix" it
   halfway.
7. **`hub_defaults` stays soft** (today's behaviour), but the retry fix
   (both `HUB_DEFAULTS.get` sites move to the manager's soft accessor) is
   part of M3d; if the owner wants it hard, that is a one-line change.
8. **Worker env**: `WS_JSON_ROOT`/`WS_JSON_TRACE` are read per call, but the
   app spawns the worker without them — the app-visible trace is
   `jsonmgr.report` (M5), not stderr. Document; a Swift-side debug env is
   out of scope.
9. **New file in the bundle.** `bin/build-app.sh:122` copies per shipped
   dir with `git ls-files -co --exclude-standard`, i.e. cached *and*
   untracked-not-ignored files — was ~~copies `git ls-files -c`~~, so an
   untracked `pylib/jsonmgr.py` would actually ship; `git add` it before
   `ws dmg` anyway (source-of-truth hygiene; the sweep's rule).
10. **`report --check` is not a substitute for tests** — it parses values
    but the per-module suites are what pin byte-compatibility; keep both.

Unknowns to verify during implementation:

- Does any test or helper method assign to a module attribute that M4 turns
  lazy (e.g. `shot_model.X = …`)? Assigning works (module dict write), but
  `del` afterwards would restore laziness — grep for assignments in
  `Tests/` before M4.
- Does `test_python_helper.swift`'s broken-`libDir` case or
  `helper_fixtures` rely on module-import failure text? (Expect: no; check in
  M0.)
- `settings_hub` CLI's `cli.py:342` error text names
  `settings_hub/data/hub_defaults.json` by hand — leave as copy or route
  through `jsonmgr` path in M3d (cosmetic).
- Real-world consumers seen by the ledger after M6 (`jsonmgr.report` on a
  live worker) — use it to find any path we missed and any remaining
  per-access `load()` call (the one anti-pattern the report should surface).
- `reload()` drops `_CACHE` snapshots but not values already cached into
  lazy module namespaces by `lazy_module`; document the boundary (it only
  matters for tests/debug in one process — imports behave like today).

## 5. Out of scope

- Swift-side JSON (Resources mirrors SW1/SW2, `vim/snippets/markdown.json`)
  and any Swift loader changes.
- User/runtime data files (config, team, caches, state, undo, favorites,
  geometry, recent/pasted, status): different semantics (writes, mtime
  meaning, env overrides) — they keep their own I/O.
- REVIEW #6b/#6c (Swift failure caches, `JiraPaths` failed state) and #8's
  stderr capture / `--params` check.
- `ws doctor` integration and a `ws jsons` alias (optional, not required).
- TOML (`commands.toml`, `system_shortcuts.toml`) — this manager is for the
  shipped JSON homes only.
- Performance work beyond the snapshot design (no watchers, no async).
- 3.9 support for settings_hub (it stays 3.11+); jsonmgr itself supports
  both 3.9 (jira/confluence/notify) and 3.11+.

## 6. Progress log

Base `eabf8ac`. Batches land in order M0→M6; append commit + notes here per
batch as the plan executes.

| Batch | Commit | Notes |
| --- | --- | --- |
| M0 | done | `pylib/jsonmgr.py` + `Tests/test_jsonmgr.py` + `ws test jsonmgr` case; 28 tests; 3.9 CLI green |
| M1 | done | pilot `pylib/compare_text.py` via `jsonmgr.load`; `compare` + `helper` green |
| M2a | done | `shelf`, `doc_templates`, `ai_format`; suites + `helper` green; mutation grep clean |
| M2b | done | `paneshot`, `ansi`, `prose_pdf`; suites + `helper` green |
| M2c | done | `shot_model` eager-via-manager (lazy wave keeps it eager-by-decision) |
| M2d | done | 6 pylib jira-glue modules share one `jira/defaults` parse; all 6 suites + `helper` green |
| M2e | done | `confluence_glue`, `confluence_pages` share `confluence/defaults`; suites + `helper` green |
| M3a | done | jira package 4 loaders share one parse; `test_jira_poll.py` + `jira-setup`/`jira-dashboard` + `helper` green |
| M3b | done | confluence package shares `confluence/defaults`; `test_confluence.py` + 3.9 import green |
| M3c | done | notify package shares `notify/defaults`; `test_notifications.py` + `status` + 3.9 import green |
| M3d | done | settings_hub tables/nvim_lua/hub_defaults (soft accessor, retried); `ws test settings` green; all import orders verified |
| … | — | |

## 7. Validation hooks for this plan

- Collection: `grep -rn "json.load\|open(" --include=*.py` over the five
  shipped dirs (28 loaders listed above) + `find` for shipped `*.json`
  (15 homes + 2 non-homes).
- The sweep's verification already proved bundle deployment
  (`RESOURCE_LINK_DIRS`, `git ls-files`, no `.json` gitignore) — unchanged
  by adding `pylib/jsonmgr.py`.
- REVIEW #8 and #6a are the two findings this plan closes for Python
  import-time reads; #6a's Swift-side remainder stays deferred.

## 8. Validation record (this plan re-checked against `eabf8ac`)

- **28 load sites / 15 homes counted**: pylib 9 (`paths`, `shot_model`,
  `paneshot`, `ansi`, `shelf`, `prose_pdf`, `doc_templates`, `ai_format`,
  `compare_text`), jira 4 package loaders + 6 pylib loaders (one home),
  confluence 4, notify 2, settings_hub 3. The `json.load`/`open` grep over
  the five shipped dirs matches; every loader opens through
  `dirname(abspath(__file__))` (+`/..` for the pylib→jira/confluence
  cross-reads).
- **Namespace-package claim corrected**: `python3 -m pylib.<mod>` *does*
  work from the repo root despite no `__init__.py` (namespace package;
  verified `python3 -m pylib.jira_paths`). §2.4 states the corrected form.
- **3.9 claim verified**: `/usr/bin/python3` is 3.9.6 on this machine (the
  same interpreter launchd/CLT scripts land on).
- **No direct mutation of loaded payloads**: no item assignment or
  `update/pop/clear` on `_DATA` / `_D` / `_DEFAULTS` / `_*_DEFAULTS` in any
  shipped dir; sites that add entries copy first (`dict(e)` before appending
  a default endpoint, `jira_config.py:660-664/896`; `dict(...)` projection
  in `:164`). Nested values stay shared shallowly, so M1/M3a must still run
  the per-batch mutation grep (risk 1) with the manager docstring stating
  "shared, read-only".
- **No `type(x) is dict` checks** in the shipped dirs or `Tests/` — the
  `WS_JSON_TRACE` dict-subclass in §2.4 cannot break a consumer.
- **Symbol pinning confirmed in tests**: `test_jira_fields.py:78-86`
  (`jf.BASE_FIELD_LABELS`), `test_doc_templates.py:69/119` (`dt.BUILTIN`),
  `test_jira_boards.py:59` (`jb.DONE_LIMIT`) — keeping the names (eager or
  lazy) keeps the suites green.
- **Line refs spot-checked**: `jira_config.py:509` (`_CRITERIA`),
  `jira_poll.py:1580`/`:2125` (`SPRINT_RANK`/`MY_WORK`),
  `confluence_api.py:356` (`FAV_KEYS`), `cli.py:342` (the hub_defaults path
  in an error string).
- **settings_hub bootstrap chain confirmed**: `settings.py`/`readers.py`
  import `.tables` before `.paths`, so M3d's `from . import paths`
  bootstrap inside `tables.py` (risk 5) is required; no cycle (`paths`
  imports only `jsonmgr` + `config_text` after the change).
- **Bundle deployment unchanged**: `install.conf` `RESOURCE_LINK_DIRS`
  covers bin/jira/confluence/notify/vim/pylib/settings_hub and
  `bin/build-app.sh` copies via `git ls-files -co --exclude-standard`
  (cached + untracked-not-ignored) — `pylib/jsonmgr.py` ships exactly like
  the JSON homes it reads.

## 9. Validation (independent re-check, HEAD `ec0f264`)

Re-verified against the tree with `grep -rn "json.load\|json.loads"` over
pylib/jira/confluence/notify/settings_hub/bin, `git ls-files '*.json'`,
`bin/run-tests.sh`, `install.conf` + `bin/build-app.sh`, `bin/setup-home.sh` +
`symlinks.sh`, `jira/jira-doctor.sh` + the launchd plist, and the helper.

**Confirmed (no change):**

- **Inventory**: 15 homes / 28 load sites / 17 pylib loader modules — exact.
  9 pylib homes (17 pylib load sites incl. 6 jira-glue + 2 confluence-glue);
  `jira/defaults.json` 10×, `confluence/defaults.json` 4×,
  `notify/defaults.json` 2×, settings_hub 3×. All loader line refs
  spot-checked (e.g. `jira_config.py:102`/`:509`, `jira_poll.py:1580`/
  `:2125`, `confluence_api.py:356`, `cli.py:342`). Nothing loads a shipped
  home outside the 28; the many other `json.load` hits are user/runtime data.
- **Path math holds in all four contexts** — repo; bundle
  (`RESOURCE_LINK_DIRS` → `Contents/Resources/{pylib,jira,confluence,notify,
  settings_hub}`); app-home (`setup-home.sh cmd_app` links each shipped dir
  into `$WS_HOME`, and `settings_hub/paths.py:8` says "NOT realpath: the
  home's link"); launchd (`jira-doctor.sh:223` substitutes `__WS_CONFIG__` →
  `$HOME/.config/kitchen-sink`, script path absolute so cwd is irrelevant,
  `jira_config.py:69` inserts `<root>/pylib`). No assumption breaks.
- **3.9**: `/usr/bin/python3` = 3.9.6; `python3 -m pylib.jira_paths` works
  (namespace package, no `pylib/__init__.py`); plan uses no 3.10+ construct
  (PEP 562 `__getattr__` 3.7+, `sys._getframe` CPython-only but both
  interpreters are CPython, `math.isfinite` 3.2+ if used).
- **Never-cache-failure (#6a) is achievable** in the stated flow: parse →
  write `_CACHE` last; failure drops the stale entry, keeps `_LAST_ERROR`,
  raises; next call re-stats/re-parses. Trade-off (intended): a transient
  bad read of a previously good file raises instead of serving the old
  snapshot.
- **Lazy wave structurally safe**: no def-time capture — `paneshot.Config`,
  `shot_model.Args`/`State`, `ansi.Theme` read JSON at call time; verified
  two modules the task flagged plus the rest.
- **Batch guards all exist**: every suite named (`compare` … `settings`,
  `all`, `jira-*`, `confluence-*`, `shot-model`, `ansi-parse`, `prose-pdf`,
  `doc-templates`, `shelf`) is a case in `bin/run-tests.sh`, and
  `test_jira_poll.py` / `test_confluence.py` / `test_notifications.py` exist.
- **settings_hub bootstrap**: `readers.py:5` imports `.tables` before
  `:18` `.paths`; fix + no-cycle claim (paths → config_text/jsonmgr only)
  correct.
- **Helper/report pattern fits**: `_MODULES` guard loop + `_jira_paths`
  precedent; `helper/__init__.dispatch` already has the generic
  message+traceback branch.
- **No `type(x) is dict`, no mutation of loaded payloads, `WS_JSON_ROOT`/
  `WS_JSON_TRACE` unused today, guard commit `80101b9` exists.**

**Corrected (10):** guard ref 9-54→13-55; import list
`expanduser`→`os.path.expanduser`; ledger check-then-set is not GIL-atomic
(keep under the RLock); helper-method body must live in `methods.py`, not
jsonmgr; mtime test bullet vs §2.5 contradiction; registry-test allowlist
must include `vim/snippets/markdown.json`; M3e guard missing
confluence/notify suites; risk 5 made exact; risk 9's `git ls-files -c` →
`-co --exclude-standard`; §8 bundle copy command.

**Added (2):** derived-table projection bullet in §2.6 (all 15 modules build
module-level derived constants; `shot_model` should be pre-declared
eager-by-decision; private `_HEX_KEYS` must be exported — test pin); and the
`reload()` vs lazy namespace-cache caveat (unknowns). The M3e suite extension
is counted under corrected. **Removed: 0.**

**Blockers:** none. Implementation can start as-is (M0–M3 are
correction-independent). M4 needs the one pre-decision above.

**Riskiest unresolved (2-3):** (1) M4's promised "15 of 17 lazy" may shrink:
every converted module needs callable projections + internal rewrites;
`shot_model` (~10 projections, ~10 rewrites) is the likely eager-by-decision
fallback — end state unaffected. (2) `jsonmgr` must stay helper-free; keep
`@method("jsonmgr.report")` in `pylib/helper/methods.py` or
`-m pylib.jsonmgr` and settings_hub imports break. (3) Eager (M2) modules'
field reads are ledger-invisible without `WS_JSON_TRACE`, so the report's
"which field" answer is partial until M4 — acceptable, but don't promise
more.
