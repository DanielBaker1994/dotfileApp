from __future__ import annotations

import json
import sys
import threading
from concurrent.futures import ThreadPoolExecutor

from . import VERSION, dispatch
from . import methods as _methods

# The app holds a handful of calls in flight and some of them (the script
# bridge, prose export) run for seconds. A small bounded pool keeps one slow
# call from serialising every other request; replies are matched by id, so
# out-of-order completion is fine.
MAX_WORKERS = 8

_write_lock = threading.Lock()


def main(argv=None) -> int:
    argv = list(sys.argv[1:]) if argv is None else list(argv)
    if "--version" in argv:
        print(VERSION)
        return 0
    if "--call" in argv:
        i = argv.index("--call")
        name = argv[i + 1] if i + 1 < len(argv) else ""
        params = {}
        if "--params" in argv:
            try:
                params = json.loads(argv[argv.index("--params") + 1])
            except ValueError as e:
                return _emit_reply({"id": None, "ok": False,
                                    "error": {"message": "bad --params json: %s" % e}})
        return _emit_reply(dispatch({"id": 1, "method": name, "params": params}))
    if "--once" in argv:
        return _once(sys.stdin.readline())
    return _serve(sys.stdin)


def _serve(stream) -> int:
    pool = ThreadPoolExecutor(max_workers=MAX_WORKERS)
    try:
        pending = []
        shutdown = None
        for line in stream:
            line = line.strip()
            if not line:
                continue
            request, error = _parse(line)
            if error is not None:
                _emit(error)
                continue
            if request.get("method") == "shutdown":
                shutdown = request
                break
            pending.append(pool.submit(_answer, request))
        # EOF or shutdown: every accepted request still gets its reply.
        for fut in pending:
            fut.result()
        if shutdown is not None:
            _emit({"id": shutdown.get("id"), "ok": True, "result": {}})
        return 0
    finally:
        pool.shutdown()


def _answer(request) -> None:
    _emit(dispatch(request))


def _once(line: str) -> int:
    request, error = _parse(line)
    if error is not None:
        _emit(error)
        return 1
    if request.get("method") == "shutdown":
        _emit({"id": request.get("id"), "ok": True, "result": {}})
        return 0
    return _emit_reply(dispatch(request))


def _parse(line: str):
    line = (line or "").strip()
    if not line:
        return None, {"id": None, "ok": False, "error": {"message": "empty request"}}
    try:
        request = json.loads(line)
    except ValueError as e:
        return None, {"id": None, "ok": False, "error": {"message": "bad json: %s" % e}}
    if not isinstance(request, dict):
        return None, {"id": None, "ok": False, "error": {"message": "request must be a JSON object"}}
    return request, None


def _emit_reply(reply: dict) -> int:
    _emit(reply)
    return 0 if reply.get("ok") else 1


def _emit(obj) -> None:
    line = json.dumps(obj, ensure_ascii=False, separators=(",", ":")) + "\n"
    try:
        with _write_lock:
            sys.stdout.write(line)
            sys.stdout.flush()
    except OSError:
        # The parent is gone; EOF on stdin ends the loop on its own.
        pass


if __name__ == "__main__":
    sys.exit(main())
