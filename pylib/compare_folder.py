from __future__ import annotations

import compare_text


def _as_file(info):
    if info is None:
        return None
    if info.get("isLink"):
        target = info.get("targetSize")
        return None if target is None else (target, info.get("targetMtime") or 0.0)
    if info.get("isDir"):
        return None
    return (info.get("size") or 0, info.get("mtime") or 0.0)


def classify(left, right, options: dict) -> dict:
    out = {"status": "same", "sameByMetadata": False, "newer": "none"}
    if left is not None and right is None:
        out["status"] = "leftOnly"
        return out
    if right is not None and left is None:
        out["status"] = "rightOnly"
        return out
    if left is None and right is None:
        out["status"] = "error"
        return out
    if left["isDir"] != right["isDir"]:
        out["status"] = "different"
        return out
    if left["isDir"]:
        return out
    if left["isLink"] and right["isLink"]:
        out["status"] = "same" if left.get("link") == right.get("link") else "different"
        return out
    lf, rf = _as_file(left), _as_file(right)
    if lf is None or rf is None:
        out["status"] = "different"
        return out
    tolerance = options.get("timeTolerance", 2)
    if lf[1] > rf[1] + tolerance:
        out["newer"] = "left"
    elif rf[1] > lf[1] + tolerance:
        out["newer"] = "right"
    if lf[0] != rf[0]:
        out["status"] = "different"
        return out
    times_match = abs(lf[1] - rf[1]) <= tolerance
    content = options.get("content") or "auto"
    if content == "always":
        out["status"] = "unknown"
    elif content == "never":
        out["status"] = "same" if times_match else "different"
    else:
        out["status"] = "same" if times_match else "unknown"
    out["sameByMetadata"] = out["status"] == "same"
    return out


def classify_many(items: list, options: dict) -> list:
    return [classify(item.get("left"), item.get("right"), options) for item in items]


def content_check(left: str, right: str, size_left: int, size_right: int, imp: dict) -> str:
    if size_left == size_right:
        same = _same_bytes(left, right)
        if same is True:
            return "same"
        if same is None:
            return "error"
    return "unimportant" if compare_text.text_equal_under_rules(left, right, imp) else "different"


def _same_bytes(a: str, b: str):
    try:
        fa = open(a, "rb")
    except OSError:
        return None
    try:
        fb = open(b, "rb")
    except OSError:
        fa.close()
        return None
    with fa, fb:
        while True:
            da = fa.read(1 << 20)
            db = fb.read(1 << 20)
            if da != db:
                return False
            if not da:
                return True
