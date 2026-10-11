"""ONE JSON data manager for the shipped JSON homes.

Every shipped ``*.json`` file (15 homes: pylib, jira, confluence, notify,
settings_hub) loads through this module: one parse per home per process,
one error type (:class:`JsonError`), an always-on access ledger (which
module reads which file/field) and an optional runtime trace
(``WS_JSON_TRACE=1``).

The registry (``_HOMES``) is static and explicit; the logical name is the
repo-relative path minus ``.json`` - ``load("pylib/shelf")``. The root is
the directory beside/above pylib (repo checkout, the app bundle's
``Contents/Resources``, or the app home - all share that layout);
``WS_JSON_ROOT`` overrides it per call for tests/throwaway installs.

Stdlib only, 3.9-compatible, imports no other pylib module: it is safe to
import from the worker's guarded import loop, the CLI scripts and
settings_hub before any path bootstrap beyond the existing ones.

Shared objects: ``load`` returns the cached dict itself (no copy). The
data is shared and read-only - a consumer that needs to change values must
copy first. A failure is never cached: the stale snapshot is dropped, the
error is only kept for the report, and the next ``load``/``field``
retries.
"""
from __future__ import annotations

import hashlib
import json
import os
import sys
import threading
import time

_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

# The static registry: logical name -> shipped file (repo-relative). One
# name, one file, one report row. ``register()`` extends it from a JSON's
# home module; the central table is authoritative so the report works
# without importing any consumer.
_HOMES = {
    "pylib/paths": "pylib/paths.json",
    "pylib/shot_model": "pylib/shot_model.json",
    "pylib/paneshot": "pylib/paneshot.json",
    "pylib/ansi": "pylib/ansi.json",
    "pylib/shelf": "pylib/shelf.json",
    "pylib/prose_pdf": "pylib/prose_pdf.json",
    "pylib/doc_templates": "pylib/doc_templates.json",
    "pylib/ai_format": "pylib/ai_format.json",
    "pylib/compare_text": "pylib/compare_text.json",
    "jira/defaults": "jira/defaults.json",
    "confluence/defaults": "confluence/defaults.json",
    "notify/defaults": "notify/defaults.json",
    "settings_hub/data/tables": "settings_hub/data/tables.json",
    "settings_hub/data/hub_defaults": "settings_hub/data/hub_defaults.json",
    "settings_hub/data/nvim_lua": "settings_hub/data/nvim_lua.json",
}

# one RLock guards _CACHE/_LEDGER/_LAST_ERROR/_WARNINGS and the parse; a
# cache hit is a stat (~1 us) and the files are a few KB
_LOCK = threading.RLock()

_CACHE = {}     # name -> {"path", "key": (st_mtime_ns, st_size), "data", "parses"}
_LEDGER = {}    # (consumer_module, home, "a.b.c") -> [count, first_ts]
_LAST_ERROR = {}   # name -> last failure message (report only; never a cache)
_WARNINGS = []     # soft-load warnings (report only), deduped + bounded
_LEDGER_MAX = 4096
_LEDGER_TRUNCATED = [False]
_WARN_MAX = 64
_EMPTY = {}     # the shared soft fallback


class JsonError(Exception):
    """One error style for every home: .name, .path, .kind ("missing" |
    "unreadable" | "malformed" | "shape"), .cause. Deliberately not a
    ValueError subclass, so old ``except ValueError`` handlers cannot
    silently swallow it."""

    def __init__(self, name, path, kind, cause=None, detail=None):
        self.name = name
        self.path = path
        self.kind = kind
        self.cause = cause
        phrase = "malformed JSON" if kind == "malformed" else kind
        if detail:
            phrase += ": " + detail
        super().__init__('json data "%s" (%s): %s' % (name, path, phrase))


def _root():
    return os.environ.get("WS_JSON_ROOT") or _ROOT


def _tracing():
    return os.environ.get("WS_JSON_TRACE") == "1"


def _consumer():
    """The first module outside jsonmgr on the call stack."""
    here = __name__
    frame = sys._getframe(1)
    while frame is not None:
        if frame.f_globals.get("__name__", "") != here:
            return frame.f_globals.get("__name__") or "<unknown>"
        frame = frame.f_back
    return "<unknown>"


def _record(consumer, name, keypath):
    """Dedupe (consumer, home, keypath) under the lock; emit a trace line on
    the first touch when WS_JSON_TRACE=1. Bounded, notes truncation."""
    key = (consumer, name, keypath)
    first = False
    with _LOCK:
        row = _LEDGER.get(key)
        if row is None:
            if len(_LEDGER) >= _LEDGER_MAX:
                _LEDGER_TRUNCATED[0] = True
                return
            row = [0, 0.0]
            _LEDGER[key] = row
            first = True
        row[0] += 1
        if first:
            row[1] = time.time()
    if first and keypath and _tracing():
        sys.stderr.write("jsonmgr: %s -> %s -> %s\n" % (consumer, name, keypath))


def _warn(message):
    with _LOCK:
        if message in _WARNINGS:
            return
        if len(_WARNINGS) >= _WARN_MAX:
            message = "…more warnings suppressed"
        _WARNINGS.append(message)


def _resolve(name):
    rel = _HOMES.get(name)
    if rel is None:
        raise JsonError(name, "", "shape", None,
                        "unknown home (not in the registry)")
    if os.path.isabs(rel):
        return rel
    return os.path.join(_root(), rel)


def _fail(name, path, err, soft):
    """Never cache a failure: drop any snapshot, keep the message for the
    report, retry next call (or answer the shared {} for soft homes)."""
    with _LOCK:
        _CACHE.pop(name, None)
        _LAST_ERROR[name] = str(err)
    if soft:
        _warn(str(err))
        return _EMPTY
    raise err


def _data_locked(name, path, refresh, soft):
    entry = _CACHE.get(name)
    if entry is not None and entry["path"] == path and not refresh:
        try:
            st = os.stat(path)
        except OSError as e:
            kind = "missing" if isinstance(e, FileNotFoundError) else "unreadable"
            return _fail(name, path, JsonError(name, path, kind, e), soft)
        if (st.st_mtime_ns, st.st_size) == entry["key"]:
            return entry["data"]
    try:
        st = os.stat(path)
    except OSError as e:
        kind = "missing" if isinstance(e, FileNotFoundError) else "unreadable"
        return _fail(name, path, JsonError(name, path, kind, e), soft)
    try:
        with open(path, encoding="utf-8") as fh:
            data = json.load(fh)
    except OSError as e:
        return _fail(name, path, JsonError(name, path, "unreadable", e, str(e)), soft)
    except ValueError as e:
        return _fail(name, path, JsonError(name, path, "malformed", e, str(e)), soft)
    if not isinstance(data, dict):
        return _fail(name, path, JsonError(
            name, path, "shape", None,
            "root is %s, expected object" % type(data).__name__), soft)
    parses = ((entry["parses"] if entry and entry["path"] == path else 0) + 1)
    _CACHE[name] = {"path": path, "key": (st.st_mtime_ns, st.st_size),
                    "data": data, "parses": parses}
    _LAST_ERROR.pop(name, None)
    return data


def load(name, *, soft=False, refresh=False):
    """The home's parsed dict (one parse per process while the file's
    mtime_ns+size stand still; ``refresh=True`` forces a re-parse).
    ``soft=True`` answers a shared {} on failure, recorded as a warning -
    still never cached."""
    consumer = _consumer()
    _record(consumer, name, "")
    path = _resolve(name)
    with _LOCK:
        data = _data_locked(name, path, refresh, soft)
    if _tracing() and data is not _EMPTY:
        return _wrap(data, consumer, name, ())
    return data


def _walk(name, path):
    with _LOCK:
        entry = _CACHE.get(name)
    if entry is None:
        load(name)
        with _LOCK:
            entry = _CACHE.get(name)
        if entry is None:   # only reachable for a soft home
            return None, _resolve(name), False
    value = entry["data"]
    for i, key in enumerate(path):
        if isinstance(value, dict) and key in value:
            value = value[key]
        else:
            return value, entry["path"], "%s" % ".".join(str(p) for p in path[:i + 1])
    return value, entry["path"], None


def get(name, *path, default=None):
    """Walk *path in the home's snapshot; a missing key answers ``default``
    (a missing/malformed home still raises - that is a hard failure)."""
    consumer = _consumer()
    _record(consumer, name, ".".join(str(p) for p in path))
    value, _path, missing = _walk(name, path)
    return default if missing else value


def field(name, *path):
    """Walk *path in the home's snapshot; a missing key raises JsonError."""
    consumer = _consumer()
    keypath = ".".join(str(p) for p in path)
    _record(consumer, name, keypath)
    value, path, missing = _walk(name, path)
    if missing:
        raise JsonError(name, path, "shape", None, "no key %r" % missing)
    return value


def register(name, path=None):
    """Add (or override) a home; ``path=None`` means ``<root>/<name>.json``.
    An absolute path is used as-is (it does not follow the root)."""
    with _LOCK:
        _HOMES[name] = path or (name + ".json")
        _CACHE.pop(name, None)


def reload(name=None):
    """Drop the snapshot(s) and re-read: one home, or all when name is
    None. (Values already cached in a lazy module's namespace by
    ``lazy_module`` are not touched - imports behave like today.)"""
    with _LOCK:
        if name is None:
            _CACHE.clear()
            return
        _CACHE.pop(name, None)
    load(name)


def ledger():
    """The deduped access ledger as report rows (stable insertion order)."""
    with _LOCK:
        return _ledger_rows_locked()


def _ledger_rows_locked():
    rows = []
    for (consumer, home, keypath), (count, first) in _LEDGER.items():
        rows.append({"consumer": consumer, "home": home, "keypath": keypath,
                     "count": count, "first": first})
    return rows


def _sha256(path):
    try:
        h = hashlib.sha256()
        with open(path, "rb") as fh:
            for chunk in iter(lambda: fh.read(65536), b""):
                h.update(chunk)
        return h.hexdigest()
    except OSError:
        return None


def report(*, with_data=False):
    """Registry + snapshot + ledger + warnings. Every home in the table gets
    a row (resolved path, exists/size/mtime/sha256, top-level keys, parse
    count, last error), whether or not it was ever loaded."""
    with _LOCK:
        cache = {n: dict(e) for n, e in _CACHE.items()}
        rows = _ledger_rows_locked()
        errors = dict(_LAST_ERROR)
        warnings = list(_WARNINGS)
        truncated = _LEDGER_TRUNCATED[0]
    homes = []
    for name in sorted(_HOMES):
        path = _resolve(name)
        try:
            st = os.stat(path)
        except OSError:
            st = None
        entry = cache.get(name)
        row = {
            "name": name,
            "path": path,
            "exists": st is not None,
            "size": st.st_size if st is not None else None,
            "mtime": st.st_mtime if st is not None else None,
            "sha256": _sha256(path) if st is not None else None,
            "loaded": entry is not None,
            "parses": entry["parses"] if entry else 0,
            "keys": sorted(entry["data"].keys()) if entry else None,
            "last_error": errors.get(name),
        }
        if with_data:
            row["data"] = entry["data"] if entry else None
        homes.append(row)
    return {
        "version": 1,
        "root": _root(),
        "homes": homes,
        "ledger": rows,
        "ledger_truncated": truncated,
        "warnings": warnings,
    }


# --------------------------------------------------------------- tracing

def _wrap(value, consumer, home, prefix):
    if isinstance(value, dict):
        return _TracedDict(value, consumer, home, prefix)
    if isinstance(value, list):
        return _TracedList(value, consumer, home, prefix)
    return value


class _TracedDict(dict):
    """The dict subclass ``load`` returns when WS_JSON_TRACE=1: every
    first ``d["key"]`` touch records the keypath and writes one stderr
    line (the eager modules' own indexing, no rewrite needed)."""

    __slots__ = ("_jm_consumer", "_jm_home", "_jm_prefix", "_jm_wrapped")

    def __init__(self, data, consumer, home, prefix):
        dict.__init__(self, data)
        self._jm_consumer = consumer
        self._jm_home = home
        self._jm_prefix = prefix
        self._jm_wrapped = {}
        for key, value in data.items():
            self._jm_wrapped[key] = _wrap(value, consumer, home,
                                          prefix + (str(key),))

    def _jm_touch(self, key):
        value = self._jm_wrapped[key]
        _record(self._jm_consumer, self._jm_home,
                ".".join(self._jm_prefix + (str(key),)))
        return value

    def __getitem__(self, key):
        return self._jm_touch(key)

    def get(self, key, default=None):
        try:
            return self._jm_touch(key)
        except KeyError:
            return default


class _TracedList(list):
    """The list side of the trace: elements stay wrapped so a later
    ``element["key"]`` records its path."""

    __slots__ = ("_jm_wrapped",)

    def __init__(self, items, consumer, home, prefix):
        list.__init__(self, items)
        self._jm_wrapped = [_wrap(v, consumer, home, prefix) for v in items]

    def __getitem__(self, index):
        return self._jm_wrapped[index]

    def __iter__(self):
        return iter(self._jm_wrapped)


# ---------------------------------------------------------- lazy exports

def lazy_module(module_name, namespace, exports):
    """Convert a module's data constants to lazy, retryable exports (M4).

    ``exports`` maps a name to ``(home, path_tuple[, cast])`` - resolved
    through ``field`` on first touch - or to a callable projection (built
    fresh; e.g. a dict with an expanded ``~``). Installs PEP 562
    ``__getattr__``/``__all__``/``__dir__``; the resolved value is cached
    into the module namespace (idempotent, a double compute is harmless);
    a failure is never cached, so the next touch retries.

    Documented contract: a bare name *inside* the defining module does not
    go through ``__getattr__`` (module.__dict__ lookup) - internal uses
    must call ``jsonmgr.field(...)`` instead.
    """
    def __getattr__(name):
        spec = exports.get(name)
        if spec is None:
            raise AttributeError("module %r has no attribute %r"
                                 % (module_name, name))
        value = _export_value(spec)
        namespace[name] = value
        return value

    def __dir__():
        return sorted(set(namespace) | set(exports))

    namespace["__getattr__"] = __getattr__
    namespace["__jsonmgr_exports__"] = dict(exports)
    allowed = set(k for k in namespace if not k.startswith("_"))
    allowed |= set(exports)
    namespace["__all__"] = sorted(allowed)
    namespace["__dir__"] = __dir__


def _export_value(spec):
    if callable(spec):
        return spec()
    home = spec[0]
    path = tuple(spec[1])
    value = field(home, *path)
    if len(spec) > 2 and spec[2] is not None:
        value = spec[2](value)
    return value


# ------------------------------------------------------------------ CLI

_USAGE = ("usage: python3 pylib/jsonmgr.py --report [--json] [--check] "
          "[--root DIR]   (also: python3 -m pylib.jsonmgr)")


def _print_human(rep):
    print("json manager: root=%s (%d homes)" % (rep["root"], len(rep["homes"])))
    for row in rep["homes"]:
        if not row["exists"]:
            state = "MISS"
        elif row["last_error"]:
            state = "ERR "
        else:
            state = "ok  "
        print("  %s %-34s keys=%-4s parses=%d  %s"
              % (state, row["name"], len(row["keys"]) if row["keys"] else "-",
                 row["parses"], row["path"]))
        if row["last_error"]:
            print("       %s" % row["last_error"])
    print("ledger: %d rows%s" % (len(rep["ledger"]),
                                 " (truncated)" if rep["ledger_truncated"] else ""))
    for warning in rep["warnings"]:
        print("warning: %s" % warning)


def _main(argv):
    want_report = False
    as_json = False
    check = False
    root = None
    args = list(argv)
    i = 0
    while i < len(args):
        arg = args[i]
        if arg == "--report":
            want_report = True
        elif arg == "--json":
            as_json = True
        elif arg == "--check":
            check = True
        elif arg == "--root":
            i += 1
            if i >= len(args):
                sys.stderr.write(_USAGE + "\n")
                return 2
            root = args[i]
        elif arg in ("-h", "--help"):
            print(_USAGE)
            return 0
        else:
            sys.stderr.write("jsonmgr: unknown argument %r\n%s\n" % (arg, _USAGE))
            return 2
        i += 1
    if root:
        os.environ["WS_JSON_ROOT"] = root
    failures = []
    if check:
        for name in sorted(_HOMES):
            try:
                load(name, refresh=True)
            except JsonError as e:
                failures.append(str(e))
    rep = report()
    if want_report or check:
        if as_json:
            print(json.dumps(rep, indent=2))
        else:
            _print_human(rep)
    if failures:
        for failure in failures:
            sys.stderr.write(failure + "\n")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(_main(sys.argv[1:]))
