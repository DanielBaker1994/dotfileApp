from __future__ import annotations

import os
import re
import time


def _components(path: str) -> list:
    if path.startswith("/"):
        return ["/"] + [c for c in path.split("/") if c]
    return [c for c in path.split("/") if c]


def relative(path: str, base: str):
    if base == "/":
        return path[1:]
    if not path.startswith(base + "/"):
        return None
    return path[len(base) + 1:]


def glob_to_regex(glob: str) -> str:
    c = glob
    out = ""
    i = 0
    n = len(c)
    while i < n:
        ch = c[i]
        if ch == "*":
            if i + 1 < n and c[i + 1] == "*":
                at_start = i == 0 or c[i - 1] == "/"
                at_end = i + 2 == n
                slash_after = i + 2 < n and c[i + 2] == "/"
                if at_start and slash_after:
                    out += "(?:.*/)?"
                    i += 3
                    continue
                if at_start and at_end:
                    out += ".*"
                    i += 2
                    continue
                out += "[^/]*"
                i += 2
                continue
            out += "[^/]*"
        elif ch == "?":
            out += "[^/]"
        elif ch == "[":
            j = i + 1
            if j < n and c[j] in "!^":
                j += 1
            if j < n and c[j] == "]":
                j += 1
            while j < n and c[j] != "]":
                j += 1
            if j >= n:
                out += "\\["
            else:
                body = c[i + 1:j]
                if body.startswith("!"):
                    body = "^" + body[1:]
                body = body.replace("\\", "\\\\")
                out += "[" + body + "]"
                i = j
        elif ch == "\\":
            if i + 1 < n:
                out += re.escape(c[i + 1])
                i += 1
            else:
                out += "\\\\"
        else:
            out += re.escape(ch)
        i += 1
    return out


def compile_pattern(line: str, global_: bool = False, home=None):
    s = line
    while s.endswith(" ") and not s.endswith("\\ "):
        s = s[:-1]
    if not s or s.startswith("#"):
        return None
    negate = False
    if s.startswith("!"):
        negate = True
        s = s[1:]
    elif s.startswith("\\!") or s.startswith("\\#"):
        s = s[1:]
    dir_only = False
    if s.endswith("/") and not s.endswith("\\/"):
        dir_only = True
        s = s[:-1]
    if not s:
        return None
    pat = s
    if global_ and pat.startswith("~/"):
        pat = (home or os.path.expanduser("~")) + pat[1:]
    anchored = "/" in pat
    if pat.startswith("/"):
        pat = pat[1:]
    rx = ("^" if anchored else "^(?:.*/)?") + glob_to_regex(pat) + "$"
    try:
        compiled = re.compile(rx)
    except re.error:
        return None
    return {"regex": compiled, "negate": negate, "dirOnly": dir_only}


def git_excludes_file(home: str) -> str:
    try:
        with open(home + "/.gitconfig", encoding="utf-8") as f:
            text = f.read()
    except OSError:
        text = ""
    in_core = False
    for raw in text.splitlines():
        line = raw.strip()
        if line.startswith("["):
            in_core = line.lower().startswith("[core")
            continue
        if not in_core or "=" not in line:
            continue
        key, _, value = line.partition("=")
        if key.strip().lower() == "excludesfile":
            v = value.strip()
            if len(v) > 1 and v.startswith("\"") and v.endswith("\""):
                v = v[1:-1]
            if v.startswith("~/"):
                return home + v[1:]
            return v
    xdg = os.environ.get("XDG_CONFIG_HOME") or (home + "/.config")
    return xdg + "/git/ignore"


class Rules:
    def __init__(self, home=None, shelf_file=None, git_excludes=None):
        self.home = home or os.path.expanduser("~")
        self.shelf_file = shelf_file
        self.git_excludes = git_excludes
        self.recheck = 2.0
        self._file_cache = {}
        self._dir_cache = {}

    def set_shelf(self, path) -> None:
        old = self.shelf_file
        self.shelf_file = path
        if old != path:
            self._file_cache.pop(old or "", None)

    def set_git_excludes(self, path) -> None:
        self.git_excludes = path

    def set_recheck(self, seconds: float) -> None:
        self.recheck = seconds

    def ignored(self, path: str, is_dir: bool = False) -> bool:
        comps = _components(path)
        if len(comps) <= 1:
            return False
        glob = self._global_set()
        shelf = self._shelf_set()
        sets = []
        directory = "/"
        in_repo = False
        for i in range(1, len(comps)):
            d = self._folder(directory)
            if d["repo"]:
                in_repo = True
            sets += [s for s in d["sets"] if in_repo or not s["isGit"]]
            directory = (directory + "/" + comps[i]) if directory != "/" else "/" + comps[i]
            last = i == len(comps) - 1
            check_sets = ([glob] if glob else []) + sets + ([shelf] if shelf else [])
            if self._decide(directory, is_dir if last else True, check_sets):
                return True
        return False

    def _decide(self, path: str, is_dir: bool, sets: list) -> bool:
        ignored = False
        for s in sets:
            rel = relative(path, s["base"])
            if rel is None:
                continue
            for p in s["patterns"]:
                if p["dirOnly"] and not is_dir:
                    continue
                if p["regex"].search(rel):
                    ignored = not p["negate"]
        return ignored

    def _folder(self, directory: str) -> dict:
        now = time.time()
        c = self._dir_cache.get(directory)
        if c and now - c["checked"] < self.recheck:
            return c
        repo = os.path.exists(os.path.join(directory, ".git"))
        sets = []
        for name in (".gitignore", ".ignore", ".rgignore"):
            f = os.path.join(directory, name)
            s = self._file(f, directory)
            if s:
                s["isGit"] = name == ".gitignore"
                sets.append(s)
        entry = {"checked": now, "repo": repo, "sets": sets}
        self._dir_cache[directory] = entry
        return entry

    def _global_set(self):
        return self._file(self.git_excludes or git_excludes_file(self.home), "/", global_=True)

    def _shelf_set(self):
        if not self.shelf_file:
            return None
        return self._file(self.shelf_file, "/", global_=True)

    def _file(self, path: str, base: str, global_: bool = False):
        now = time.time()
        c = self._file_cache.get(path)
        if c and now - c["checked"] < self.recheck:
            return c["set"]
        try:
            st = os.stat(path)
            mtime = st.st_mtime_ns / 1e9
        except OSError:
            self._file_cache[path] = {"mtime": 0.0, "checked": now, "set": None}
            return None
        if c and c["mtime"] == mtime:
            c["checked"] = now
            return c["set"]
        try:
            with open(path, encoding="utf-8") as f:
                text = f.read()
        except OSError:
            text = ""
        patterns = []
        for line in text.splitlines():
            p = compile_pattern(line, global_, self.home)
            if p:
                patterns.append(p)
        entry = {"mtime": mtime, "checked": now,
                 "set": {"base": base, "patterns": patterns, "isGit": False}}
        self._file_cache[path] = entry
        return entry["set"]
