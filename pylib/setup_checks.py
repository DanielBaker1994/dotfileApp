"""Setup window: preflight.sh's checks JSON -> the check models and the
one-line summary (the window stays Swift)."""
from __future__ import annotations


def parse_checks(raw) -> list:
    out = []
    for d in raw if isinstance(raw, list) else []:
        if not isinstance(d, dict):
            continue

        def s(k):
            v = d.get(k)
            return v if isinstance(v, str) else ""

        out.append({"id": s("id"), "group": s("group"), "level": s("level"),
                    "title": s("title"), "detail": s("detail"), "fix": s("fix"),
                    "action": s("action"), "ok": d.get("ok") is True})
    return out


def summarize(checks) -> dict:
    checks = checks if isinstance(checks, list) else []

    def req(c):
        return c.get("level") == "required"

    failed = [c for c in checks if not c.get("ok") and req(c)]
    warned = [c for c in checks if not c.get("ok") and not req(c) and c.get("group") != "stack"]
    stack_missing = [c for c in checks if not c.get("ok") and c.get("group") == "stack"]
    if failed:
        f = failed[0]
        return {"message": "%s: %s" % (f.get("title") or "", f.get("detail") or ""),
                "tone": "danger"}
    if warned:
        n = len(warned)
        return {"message": "Ready. %d optional feature%s off — see the list."
                           % (n, " is" if n == 1 else "s are"),
                "tone": "warning"}
    if stack_missing:
        return {"message": "Ready. Hotkeys and window borders are not set up (optional).",
                "tone": "dim"}
    return {"message": "Everything is in place.", "tone": "success"}
