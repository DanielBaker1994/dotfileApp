"""Confluence glue: the image loader's auth header and the UI rate-limit
gate's decision (the timer/replay stays in Swift)."""
from __future__ import annotations

import base64


def auth_header(config: dict) -> str:
    """The Authorization header for fetch requests, from a config dict:
    explicit bearer/basic (any other value derives from the email), empty
    when no token."""
    config = config if isinstance(config, dict) else {}
    token = config.get("token") if isinstance(config.get("token"), str) else ""
    email = config.get("email") if isinstance(config.get("email"), str) else ""
    auth = (config.get("auth") or "").lower() if isinstance(config.get("auth"), str) else ""
    if auth not in ("bearer", "basic"):
        auth = "bearer" if not email else "basic"
    if not token:
        return ""
    if auth == "basic":
        return "Basic " + base64.b64encode(("%s:%s" % (email, token)).encode()).decode()
    return "Bearer " + token


def rate_limit(response: dict) -> dict:
    """An API response -> the cooldown decision: pause for max(3, retryIn)
    seconds (30 when the answer has no usable number)."""
    if not isinstance(response, dict) or response.get("rateLimited") is not True:
        return {"limited": False}
    secs = response.get("retryIn")
    if not isinstance(secs, int) or isinstance(secs, bool):
        secs = 30
    return {"limited": True, "seconds": max(3, secs)}


MODES = ["all", "phrase", "any"]


def criteria(params: dict) -> dict:
    """The search panel's live state -> the criteria JSON the
    confluence_api.py --search call consumes."""
    idx = params.get("modeIndex")
    idx = idx if isinstance(idx, int) and 0 <= idx < len(MODES) else 0
    types_raw = params.get("types")
    if types_raw is None:
        types_raw = "page,blogpost"
    types = [t for t in str(types_raw).split(",") if t]
    contributors = [c for c in (params.get("contributors") or []) if isinstance(c, str)]
    spaces = [s for s in (params.get("spaces") or []) if isinstance(s, str)]
    c = {"query": (params.get("query") or "").strip(),
         "mode": MODES[idx],
         "titleOnly": bool(params.get("titleOnly")),
         "spaces": [] if params.get("spacesAll") else spaces,
         "types": types,
         "modified": params.get("modified") or "",
         "contributors": [] if params.get("contributorsAll") else contributors,
         "sort": params.get("sort") or "relevance"}
    if params.get("favorites"):
        c["favorites"] = True
    return c
