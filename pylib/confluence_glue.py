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
