from __future__ import annotations

import traceback

VERSION = 1

METHODS: dict = {}


class HelperError(Exception):
    pass


def method(name: str):
    def register(fn):
        METHODS[name] = fn
        return fn
    return register


def dispatch(request) -> dict:
    if not isinstance(request, dict):
        return _error(None, "request must be a JSON object")
    rid = request.get("id")
    name = request.get("method")
    params = request.get("params")
    if params is None:
        params = {}
    if not isinstance(params, dict):
        return _error(rid, "params must be a JSON object")
    fn = METHODS.get(name)
    if fn is None:
        return _error(rid, "unknown method: %s" % (name,))
    try:
        return {"id": rid, "ok": True, "result": fn(params)}
    except HelperError as e:
        return _error(rid, str(e))
    except Exception as e:
        return {"id": rid, "ok": False, "error": {
            "message": "%s: %s" % (type(e).__name__, e),
            "traceback": traceback.format_exc(),
        }}


def _error(rid, message: str) -> dict:
    return {"id": rid, "ok": False, "error": {"message": message}}


@method("ping")
def _ping(params: dict) -> dict:
    return {"version": VERSION}
