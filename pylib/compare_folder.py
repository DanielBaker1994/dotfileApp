from __future__ import annotations

import fnmatch
import os
import re
import stat
import time
import unicodedata

import compare_text
import ignore_rules

_TREES = {}
_TREE_NEXT = [1]
_SESSIONS = {}
_SESSION_NEXT = [1]


def _stat_side(path: str, name: str):
    try:
        st = os.lstat(path)
    except OSError:
        return None
    is_link = stat.S_ISLNK(st.st_mode)
    is_dir = stat.S_ISDIR(st.st_mode)
    info = {"name": name, "isDir": is_dir, "isLink": is_link, "link": None,
            "size": 0 if is_dir else st.st_size, "mtime": st.st_mtime_ns / 1e9,
            "targetSize": None, "targetMtime": 0.0}
    if is_link:
        try:
            info["link"] = os.readlink(path)
        except OSError:
            pass
        try:
            target = os.stat(path)
        except OSError:
            target = None
        if target is not None and stat.S_ISREG(target.st_mode):
            info["targetSize"] = target.st_size
            info["targetMtime"] = target.st_mtime_ns / 1e9
    return info


def _list(dir_path: str, hidden: bool):
    try:
        names = os.listdir(dir_path)
    except OSError:
        return None
    out = []
    for name in names:
        if not hidden and name.startswith("."):
            continue
        info = _stat_side(os.path.join(dir_path, name), name)
        if info is not None:
            out.append(info)
    return out


def _excluded(name: str, patterns: list) -> bool:
    for pattern in patterns:
        if any(c in pattern for c in "*?["):
            if fnmatch.fnmatchcase(name, pattern):
                return True
        elif pattern == name:
            return True
    return False


def _fold(name: str, ci: bool) -> str:
    folded = unicodedata.normalize("NFC", name)
    return folded.lower() if ci else folded


def _natural(name: str) -> list:
    return [(0, int(p)) if p.isdigit() else (1, p.casefold())
            for p in re.split(r"(\d+)", name)]


def matches_name(name: str, filter_: str) -> bool:
    parts = [p for p in re.split(r"[,\s]+", filter_) if p]
    if not parts:
        return True
    inc = [p for p in parts if not p.startswith("!")]
    exc = [p[1:] for p in parts if p.startswith("!")]

    def hit(g: str) -> bool:
        pat = g if any(c in g for c in "*?[") else "*%s*" % g
        return fnmatch.fnmatchcase(name.lower(), pat.lower())

    if any(hit(g) for g in exc):
        return False
    return not inc or any(hit(g) for g in inc)


def _is_dir(n: dict) -> bool:
    return bool((n["left"] and n["left"]["isDir"]) or (n["right"] and n["right"]["isDir"]))


def _kind_mismatch(n: dict) -> bool:
    return bool(n["left"] and n["right"] and n["left"]["isDir"] != n["right"]["isDir"])


def _passes(n: dict, f: str) -> bool:
    status = n["status"]
    if f == "all":
        return True
    if f == "diffs":
        return status != "same"
    if f == "same":
        return status == "same"
    if f == "orphans":
        return status in ("leftOnly", "rightOnly")
    if f == "leftNewer":
        return n["newer"] == "left"
    if f == "rightNewer":
        return n["newer"] == "right"
    return True


def scan_start(left: str, right: str, options: dict, case_insensitive: bool) -> int:
    rules = None
    if options.get("useGitignore"):
        rules = ignore_rules.Rules(shelf_file=options.get("ignoreFile") or None)
        rules.set_recheck(float(options.get("recheck") or 30))
    session = {
        "left": left, "right": right, "options": options, "ci": case_insensitive,
        "rules": rules, "nodes": [], "roots": [], "errors": [], "truncated": False,
        "stack": [(left, right, "", None, 0)],
    }
    handle = _SESSION_NEXT[0]
    _SESSION_NEXT[0] += 1
    _SESSIONS[handle] = session
    return handle


def _keep(session: dict, info: dict, dir_path) -> bool:
    if _excluded(info["name"], session["options"].get("exclude") or []):
        return False
    rules = session["rules"]
    if rules and dir_path:
        if rules.ignored(os.path.join(dir_path, info["name"]), info["isDir"]):
            return False
    return True


def scan_step(handle: int, max_dirs: int = 24) -> dict:
    session = _SESSIONS.get(handle)
    if session is None:
        return {"count": 0, "done": True}
    nodes = session["nodes"]
    stack = session["stack"]
    processed = 0
    while stack and processed < max_dirs:
        lp, rp, rel, parent_id, depth = stack.pop()
        processed += 1
        le = _list(lp, session["options"].get("hidden", True)) if lp else None
        re_ = _list(rp, session["options"].get("hidden", True)) if rp else None
        if lp and le is None:
            session["errors"].append(lp)
            le = []
        if rp and re_ is None:
            session["errors"].append(rp)
            re_ = []
        left_entries = [e for e in (le or []) if _keep(session, e, lp)]
        right_entries = [e for e in (re_ or []) if _keep(session, e, rp)]
        by_key = {}
        order = []
        for e in left_entries:
            k = _fold(e["name"], session["ci"])
            if k not in by_key:
                order.append(k)
                by_key[k] = {"l": None, "r": None}
            by_key[k]["l"] = e
        for e in right_entries:
            k = _fold(e["name"], session["ci"])
            if k not in by_key:
                order.append(k)
                by_key[k] = {"l": None, "r": None}
            by_key[k]["r"] = e
        order.sort(key=lambda k: (0 if ((by_key[k]["l"] or by_key[k]["r"])["isDir"]) else 1,
                                  _natural((by_key[k]["l"] or by_key[k]["r"])["name"])))
        children = []
        for k in order:
            if len(nodes) >= 400_000:
                session["truncated"] = True
                break
            e = by_key[k]
            name = (e["l"] or e["r"])["name"]
            node = {"id": len(nodes), "key": k if not rel else rel + "/" + k,
                    "rel": name if not rel else rel + "/" + name, "name": name,
                    "parent": parent_id, "depth": depth,
                    "left": e["l"], "right": e["r"], "children": [],
                    "status": "same", "sameByMetadata": False, "newer": "none",
                    "diffBelow": 0, "importantBelow": 0, "unknownBelow": 0}
            nodes.append(node)
            children.append(node["id"])
            if parent_id is not None:
                nodes[parent_id]["children"].append(node["id"])
            if _is_dir(node) and not _kind_mismatch(node):
                stack.append((os.path.join(lp, node["left"]["name"]) if (node["left"] and lp) else None,
                              os.path.join(rp, node["right"]["name"]) if (node["right"] and rp) else None,
                              node["key"], node["id"], depth + 1))
        if parent_id is None:
            session["roots"] += children
    return {"count": len(nodes), "done": not stack,
            "errors": len(session["errors"]), "truncated": session["truncated"]}


def _settle(session: dict) -> None:
    nodes = session["nodes"]

    def walk(n: dict) -> None:
        for cid in n["children"]:
            walk(nodes[cid])
        if not _is_dir(n) or _kind_mismatch(n):
            return
        diff = unknown = important = 0
        for cid in n["children"]:
            c = nodes[cid]
            if _is_dir(c) and not _kind_mismatch(c):
                diff += c["diffBelow"]
                unknown += c["unknownBelow"]
                important += c["importantBelow"]
            else:
                if c["status"] != "same":
                    diff += 1
                if c["status"] == "unknown":
                    unknown += 1
                if c["status"] not in ("same", "unimportant", "unknown"):
                    important += 1
        n["diffBelow"] = diff
        n["unknownBelow"] = unknown
        n["importantBelow"] = important
        if n["left"] is not None and n["right"] is None:
            n["status"] = "leftOnly"
            return
        if n["right"] is not None and n["left"] is None:
            n["status"] = "rightOnly"
            return
        if important > 0:
            n["status"] = "different"
        elif unknown > 0:
            n["status"] = "unknown"
        elif diff > 0:
            n["status"] = "unimportant"
        else:
            n["status"] = "same"
        n["newer"] = "none"

    for rid in session["roots"]:
        walk(nodes[rid])


def scan_finish(handle: int) -> dict:
    session = _SESSIONS.pop(handle, None)
    if session is None:
        return {}
    nodes = session["nodes"]
    results = classify_many([{"left": n["left"], "right": n["right"]} for n in nodes],
                            session["options"])
    for n, r in zip(nodes, results):
        n["status"] = r["status"]
        n["sameByMetadata"] = r["sameByMetadata"]
        n["newer"] = r["newer"]
    _settle(session)
    tree_handle = _TREE_NEXT[0]
    _TREE_NEXT[0] += 1
    _TREES[tree_handle] = session
    return {"handle": tree_handle, "nodes": nodes, "roots": session["roots"],
            "errors": session["errors"], "truncated": session["truncated"],
            "caseInsensitive": session["ci"]}


def tree_drop(handle: int) -> None:
    _TREES.pop(handle, None)


def _tree(handle):
    return _TREES.get(handle)


def _node(handle, node_id):
    tree = _tree(handle)
    if tree is None:
        return None
    nodes = tree["nodes"]
    return nodes[node_id] if 0 <= node_id < len(nodes) else None


def _path(tree: dict, n: dict, side: str) -> str:
    root = tree["left"] if side == "left" else tree["right"]
    parts = []
    cur = n
    while cur is not None:
        info = cur["left"] if side == "left" else cur["right"]
        parts.append((info or {}).get("name") or cur["name"])
        cur = tree["nodes"][cur["parent"]] if cur["parent"] is not None else None
    for part in reversed(parts):
        root = os.path.join(root, part)
    return root


def tree_path(handle: int, node_id: int, side: str) -> dict:
    tree = _tree(handle)
    n = _node(handle, node_id)
    if tree is None or n is None:
        return {"path": ""}
    return {"path": _path(tree, n, side)}


def tree_pending(handle: int) -> dict:
    tree = _tree(handle)
    if tree is None:
        return {"ids": []}
    return {"ids": [n["id"] for n in tree["nodes"]
                    if n["status"] == "unknown" and not _is_dir(n)]}


def tree_rule_candidates(handle: int) -> dict:
    tree = _tree(handle)
    if tree is None:
        return {"ids": []}
    ids = []
    for n in tree["nodes"]:
        if n["status"] != "different" or not n["left"] or not n["right"]:
            continue
        l, r = n["left"], n["right"]
        if l["isLink"] and r["isLink"]:
            continue
        lf, rf = _as_file(l), _as_file(r)
        if lf is None or rf is None:
            continue
        if lf[0] <= 4 << 20 and rf[0] <= 4 << 20:
            ids.append(n["id"])
    return {"ids": ids}


def tree_count(handle: int) -> dict:
    tree = _tree(handle)
    counts = {"different": 0, "unimportant": 0, "leftOnly": 0, "rightOnly": 0,
              "same": 0, "sameByMetadata": 0, "unknown": 0, "error": 0}
    if tree is None:
        return counts
    for n in tree["nodes"]:
        if _is_dir(n) and not _kind_mismatch(n):
            continue
        status = n["status"]
        if status in counts:
            counts[status] += 1
        if status == "same" and n["sameByMetadata"]:
            counts["sameByMetadata"] += 1
    return counts


def tree_rows(handle: int, f: str, name_filter: str, flatten: bool, expanded: list) -> dict:
    tree = _tree(handle)
    if tree is None:
        return {"rows": []}
    nodes = tree["nodes"]
    expanded_set = set(expanded or [])
    out = []
    narrowing = f != "all" or bool(name_filter)
    if flatten:
        for n in nodes:
            if not _is_dir(n) or _kind_mismatch(n):
                if _passes(n, f) and matches_name(n["name"], name_filter):
                    out.append({"id": n["id"], "depth": 0})
        return {"rows": out}

    def shows(n: dict) -> bool:
        if not _is_dir(n) or _kind_mismatch(n):
            return _passes(n, f) and matches_name(n["name"], name_filter)
        if not narrowing:
            return True
        return any(shows(nodes[cid]) for cid in n["children"])

    def walk(n: dict, depth: int) -> None:
        if not shows(n):
            return
        out.append({"id": n["id"], "depth": depth})
        if _is_dir(n) and not _kind_mismatch(n) and (n["id"] in expanded_set or narrowing):
            for cid in n["children"]:
                walk(nodes[cid], depth + 1)

    for rid in tree["roots"]:
        walk(nodes[rid], 0)
    return {"rows": out}


def _statuses(tree: dict) -> list:
    return [[n["id"], n["status"], n["sameByMetadata"], n["newer"]] for n in tree["nodes"]]


def tree_settle(handle: int, statuses: list = None) -> dict:
    tree = _tree(handle)
    if tree is None:
        return {"statuses": []}
    for entry in statuses or []:
        try:
            node_id = int(entry[0])
        except (TypeError, ValueError, IndexError):
            continue
        if 0 <= node_id < len(tree["nodes"]):
            n = tree["nodes"][node_id]
            n["status"] = entry[1] if entry[1] in ("same", "different", "unimportant",
                                                  "leftOnly", "rightOnly", "unknown", "error") else n["status"]
            n["sameByMetadata"] = bool(entry[2]) if len(entry) > 2 else False
    _settle(tree)
    return {"statuses": _statuses(tree)}


def tree_apply_answers(handle: int, answers: dict) -> dict:
    tree = _tree(handle)
    if tree is None:
        return {"statuses": []}
    for key, answer in answers.items():
        try:
            node_id = int(key)
        except (TypeError, ValueError):
            continue
        if 0 <= node_id < len(tree["nodes"]):
            n = tree["nodes"][node_id]
            n["sameByMetadata"] = False
            n["status"] = answer if answer in ("same", "different", "unimportant", "error") else n["status"]
    _settle(tree)
    return {"statuses": _statuses(tree)}


def tree_restat(handle: int, node_id: int, options: dict) -> dict:
    tree = _tree(handle)
    n = _node(handle, node_id)
    if tree is None or n is None:
        return {}
    lp = _path(tree, n, "left")
    rp = _path(tree, n, "right")
    n["left"] = _stat_side(lp, os.path.basename(lp))
    n["right"] = _stat_side(rp, os.path.basename(rp))
    n["newer"] = "none"
    result = classify(n["left"], n["right"], options)
    n["status"] = result["status"]
    n["sameByMetadata"] = result["sameByMetadata"]
    n["newer"] = result["newer"]
    return {"left": n["left"], "right": n["right"], "status": n["status"],
            "sameByMetadata": n["sameByMetadata"], "newer": n["newer"]}


def tree_sync_plan(handle: int, mode: str, name_filter: str = "") -> dict:
    tree = _tree(handle)
    if tree is None:
        return {"copies": [], "trash": [], "skipped": []}
    nodes = tree["nodes"]
    copies = []
    trash = []
    skipped = []
    to_right = mode in ("updateRight", "updateBoth", "mirrorRight")
    to_left = mode in ("updateLeft", "updateBoth", "mirrorLeft")
    mirror = "right" if mode == "mirrorRight" else "left" if mode == "mirrorLeft" else None

    def copy(n: dict, side: str) -> None:
        other = n["left"] if side == "left" else n["right"]
        copies.append({"src": _path(tree, n, "right" if side == "left" else "left"),
                       "dst": _path(tree, n, side), "to": side, "rel": n["rel"],
                       "replaces": other is not None})

    def passes(n: dict) -> bool:
        return not name_filter or matches_name(n["name"], name_filter)

    def visit(n: dict) -> None:
        folder = _is_dir(n) and not _kind_mismatch(n)
        if folder and n["left"] is not None and n["right"] is not None:
            for cid in n["children"]:
                visit(nodes[cid])
            return
        if folder and name_filter:
            for cid in n["children"]:
                visit(nodes[cid])
            return
        if not folder and not passes(n):
            return
        status = n["status"]
        if status == "leftOnly":
            if mirror == "left":
                trash.append({"path": _path(tree, n, "left"), "side": "left", "rel": n["rel"]})
            elif to_right:
                copy(n, "right")
        elif status == "rightOnly":
            if mirror == "right":
                trash.append({"path": _path(tree, n, "right"), "side": "right", "rel": n["rel"]})
            elif to_left:
                copy(n, "left")
        elif status in ("different", "unknown"):
            if mirror:
                copy(n, mirror)
                return
            if n["newer"] == "left" and to_right:
                copy(n, "right")
            elif n["newer"] == "right" and to_left:
                copy(n, "left")
            elif n["newer"] == "none":
                skipped.append(n["rel"])

    for rid in tree["roots"]:
        visit(nodes[rid])
    return {"copies": copies, "trash": trash, "skipped": skipped}


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
