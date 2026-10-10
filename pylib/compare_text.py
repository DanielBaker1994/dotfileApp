from __future__ import annotations

import json
import os

_DATA_FILE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "compare_text.json")
# EOL display labels + supported-encoding names are data; the byte codes and
# ROWS_*/OP_* wire codes stay code (contracts with stored data / the Swift peer)
with open(_DATA_FILE, encoding="utf-8") as _fh:
    _DATA = json.load(_fh)

EOL_BYTES = {0: b"", 1: b"\n", 2: b"\r\n", 3: b"\r"}
EOL_LABEL = {int(k): v for k, v in _DATA["eol_labels"].items()}
ENCODINGS = tuple(_DATA["encodings"])

ROWS_SAME, ROWS_CHANGED, ROWS_LEFT_ONLY, ROWS_RIGHT_ONLY = 0, 1, 2, 3
LEFT, RIGHT = "left", "right"
OP_SAME, OP_DEL, OP_INS = 0, 1, 2


def is_binary(data: bytes) -> bool:
    if data[:2] in (b"\xff\xfe", b"\xfe\xff"):
        return False
    return 0 in data[:8192]


def _split(text: str) -> tuple:
    u = text.encode("utf-8")
    lines, eols = [], []
    start, i, n = 0, 0, len(u)
    while i < n:
        c = u[i]
        if c == 10 or c == 13:
            lines.append(u[start:i].decode("utf-8", "replace"))
            if c == 13 and i + 1 < n and u[i + 1] == 10:
                eols.append(2)
                i += 1
            else:
                eols.append(1 if c == 10 else 3)
            start = i + 1
        i += 1
    if start < n:
        lines.append(u[start:n].decode("utf-8", "replace"))
        eols.append(0)
    return lines, eols


def decode(data: bytes):
    if is_binary(data):
        return None
    if data[:2] in (b"\xff\xfe", b"\xfe\xff"):
        le = data[0] == 0xFF
        try:
            text = data[2:].decode("utf-16-le" if le else "utf-16-be")
        except UnicodeDecodeError:
            text = None
        if text is not None:
            lines, eols = _split(text)
            return {"lines": lines, "eols": eols, "encoding": "utf16LE" if le else "utf16BE"}
    body, encoding = data, "utf8"
    if data[:3] == b"\xef\xbb\xbf":
        body, encoding = data[3:], "utf8BOM"
    try:
        body.decode("utf-8")
        lines, eols = _split(body.decode("utf-8"))
    except UnicodeDecodeError:
        encoding = "latin1"
        lines, eols = _split(data.decode("latin-1"))
    return {"lines": lines, "eols": eols, "encoding": encoding}


def encode(side: dict):
    out = bytearray()
    for line, eol in zip(side["lines"], side["eols"]):
        out += line.encode("utf-8")
        out += EOL_BYTES[eol]
    encoding = side["encoding"]
    if encoding == "utf8":
        return bytes(out)
    if encoding == "utf8BOM":
        return b"\xef\xbb\xbf" + bytes(out)
    if encoding in ("utf16LE", "utf16BE"):
        text = bytes(out).decode("utf-8")
        return ((b"\xff\xfe" if encoding == "utf16LE" else b"\xfe\xff")
                + text.encode("utf-16-le" if encoding == "utf16LE" else "utf-16-be"))
    try:
        return bytes(out).decode("utf-8").encode("latin-1")
    except (UnicodeEncodeError, UnicodeDecodeError):
        return None


def dominant_eol(side: dict) -> int:
    counts = [0, 0, 0, 0]
    for e in side["eols"][:5000]:
        counts[e] += 1
    best = 1
    for i in (1, 2, 3):
        if counts[i] > counts[best]:
            best = i
    return best if counts[best] else 1


def eol_label(side: dict) -> str:
    kinds = {e for e in side["eols"] if e != 0}
    if len(kinds) > 1:
        return "mixed"
    if kinds:
        return EOL_LABEL[kinds.pop()]
    return EOL_LABEL[dominant_eol(side)]


def side_text(side: dict) -> str:
    out = []
    for line, eol in zip(side["lines"], side["eols"]):
        out.append(line)
        out.append({0: "", 1: "\n", 2: "\r\n", 3: "\r"}[eol])
    return "".join(out)


def side_replace(side: dict, start: int, count: int, new: list) -> dict:
    lines, eols = side["lines"], side["eols"]
    eol = dominant_eol(side)
    new_eols = [eol] * len(new)
    end = start + count
    if end == len(lines):
        if count == 0 and start > 0 and eols[start - 1] == 0 and new:
            start -= 1
            count += 1
            new = [lines[start]] + new
            new_eols = [eol] + new_eols
            new_eols[-1] = 0
        elif count > 0 and eols[end - 1] == 0 and new:
            new_eols[-1] = 0
    edit = {"start": start, "old": lines[start:end], "old_eols": eols[start:end],
            "new": new, "new_eols": new_eols}
    side_apply(side, edit)
    return edit


def side_apply(side: dict, edit: dict, reverse: bool = False) -> None:
    r = slice(edit["start"], edit["start"] + (len(edit["new"]) if reverse else len(edit["old"])))
    side["lines"][r] = list(edit["old"] if reverse else edit["new"])
    side["eols"][r] = list(edit["old_eols"] if reverse else edit["new_eols"])


def _ws(c: int) -> bool:
    return c in (32, 9, 11, 12)


def importance_normalized(s: str, imp: dict) -> str:
    if not (imp["leadingWS"] or imp["trailingWS"] or imp["embeddedWS"] or imp["ignoreCase"]):
        return s
    u = s.encode("utf-8")
    if not u:
        return s
    if not imp["embeddedWS"]:
        if not (imp["leadingWS"] and _ws(u[0])) and not (imp["trailingWS"] and _ws(u[-1])):
            out = s
        else:
            lo, hi = 0, len(u)
            if imp["leadingWS"]:
                while lo < hi and _ws(u[lo]):
                    lo += 1
            if imp["trailingWS"]:
                while hi > lo and _ws(u[hi - 1]):
                    hi -= 1
            out = u[lo:hi].decode("utf-8")
    else:
        lo, hi = 0, len(u)
        first, last = 0, len(u)
        while first < len(u) and _ws(u[first]):
            first += 1
        while last > first and _ws(u[last - 1]):
            last -= 1
        if imp["leadingWS"]:
            lo = first
        if imp["trailingWS"]:
            hi = max(last, lo)
        out = bytes(b for i, b in enumerate(u) if lo <= i < hi and not (first <= i < last and _ws(b))).decode("utf-8")
    return out.lower() if imp["ignoreCase"] else out


def importance_key(s: str, eol: int, imp: dict) -> str:
    k = importance_normalized(s, imp)
    if not imp["lineEndings"]:
        k += chr(0xE000 + eol)
    return k


def importance_blank(s: str) -> bool:
    return all(_ws(c) for c in s.encode("utf-8"))


def _myers_flags(a: list, b: list):
    n, m = len(a), len(b)
    if n == 0 or m == 0:
        return [False] * n, [False] * m
    v = {1: 0}
    trace = []
    found_d = -1
    for d in range(n + m + 1):
        trace.append(dict(v))
        for k in range(-d, d + 1, 2):
            if k == -d or (k != d and v.get(k - 1, -1) < v.get(k + 1, -1)):
                x = v.get(k + 1, 0)
            else:
                x = v.get(k - 1, 0) + 1
            y = x - k
            while x < n and y < m and a[x] == b[y]:
                x += 1
                y += 1
            v[k] = x
            if x >= n and y >= m:
                found_d = d
                break
        if found_d >= 0:
            break
    matched_a = [False] * n
    matched_b = [False] * m
    x, y = n, m
    for d in range(found_d, 0, -1):
        vprev = trace[d]
        k = x - y
        if k == -d or (k != d and vprev.get(k - 1, -1) < vprev.get(k + 1, -1)):
            prev_k = k + 1
        else:
            prev_k = k - 1
        prev_x = vprev.get(prev_k, 0)
        prev_y = prev_x - prev_k
        while x > prev_x and y > prev_y:
            x -= 1
            y -= 1
            matched_a[x] = True
            matched_b[y] = True
        x, y = prev_x, prev_y
    while x > 0 and y > 0:
        x -= 1
        y -= 1
        matched_a[x] = True
        matched_b[y] = True
    return [not m for m in matched_a], [not m for m in matched_b]


class LineDiff:
    MAX_CHAIN = 64

    @staticmethod
    def hunks(A: list, B: list, textA: list, textB: list) -> list:
        ca = [False] * len(A)
        cb = [False] * len(B)
        LineDiff._histogram(A, B, ca, cb)
        LineDiff._compact(ca, cb, A, textA)
        LineDiff._compact(cb, ca, B, textB)
        return LineDiff._build(ca, cb)

    @staticmethod
    def _build(ca: list, cb: list) -> list:
        out, i, j = [], 0, 0
        n, m = len(ca), len(cb)
        while i < n or j < m:
            if (i < n and ca[i]) or (j < m and cb[j]):
                a, b = i, j
                while i < n and ca[i]:
                    i += 1
                while j < m and cb[j]:
                    j += 1
                out.append({"a": a, "n": i - a, "b": b, "m": j - b})
            else:
                i += 1
                j += 1
        return out

    @staticmethod
    def _histogram(A: list, B: list, ca: list, cb: list) -> None:
        n, m = len(A), len(B)
        pre = 0
        while pre < n and pre < m and A[pre] == B[pre]:
            pre += 1
        suf = 0
        while suf < n - pre and suf < m - pre and A[n - 1 - suf] == B[m - 1 - suf]:
            suf += 1
        work = [(pre, n - suf - pre, pre, m - suf - pre)]
        while work:
            l1, c1, l2, c2 = work.pop()
            if c1 <= 0 and c2 <= 0:
                continue
            if c1 <= 0:
                for k in range(l2, l2 + c2):
                    cb[k] = True
                continue
            if c2 <= 0:
                for k in range(l1, l1 + c1):
                    ca[k] = True
                continue
            kind, b1, e1, b2, e2 = LineDiff._find_lcs(A, B, l1, c1, l2, c2)
            if kind == 2:
                da, db = _myers_flags(A[l1:l1 + c1], B[l2:l2 + c2])
                for k, flag in enumerate(da):
                    if flag:
                        ca[l1 + k] = True
                for k, flag in enumerate(db):
                    if flag:
                        cb[l2 + k] = True
            elif kind == 0:
                for k in range(l1, l1 + c1):
                    ca[k] = True
                for k in range(l2, l2 + c2):
                    cb[k] = True
            else:
                work.append((e1 + 1, l1 + c1 - 1 - e1, e2 + 1, l2 + c2 - 1 - e2))
                work.append((l1, b1 - l1, l2, b2 - l2))

    @staticmethod
    def _find_lcs(A: list, B: list, l1: int, c1: int, l2: int, c2: int):
        end1, end2 = l1 + c1 - 1, l2 + c2 - 1
        rec_of, rec_ptr, rec_cnt = {}, [], []
        nxt = [-1] * c1
        line_rec = [0] * c1
        ptr = end1
        while ptr >= l1:
            rid = A[ptr]
            r = rec_of.get(rid)
            if r is not None:
                nxt[ptr - l1] = rec_ptr[r]
                rec_ptr[r] = ptr
                rec_cnt[r] += 1
                line_rec[ptr - l1] = r
            else:
                r = len(rec_ptr)
                rec_ptr.append(ptr)
                rec_cnt.append(1)
                rec_of[rid] = r
                line_rec[ptr - l1] = r
            ptr -= 1
        best = LineDiff.MAX_CHAIN + 1
        has_common = found = False
        lb1 = le1 = lb2 = le2 = 0
        bptr = l2
        while bptr <= end2:
            bnext = bptr + 1
            r = rec_of.get(B[bptr])
            if r is not None:
                if rec_cnt[r] > best:
                    has_common = True
                else:
                    has_common = True
                    as_ = rec_ptr[r]
                    while True:
                        np_ = nxt[as_ - l1]
                        bs, ae, be = bptr, as_, bptr
                        rc = rec_cnt[r]
                        while l1 < as_ and l2 < bs and A[as_ - 1] == B[bs - 1]:
                            as_ -= 1
                            bs -= 1
                            if 1 < rc:
                                rc = min(rc, rec_cnt[line_rec[as_ - l1]])
                        while ae < end1 and be < end2 and A[ae + 1] == B[be + 1]:
                            ae += 1
                            be += 1
                            if 1 < rc:
                                rc = min(rc, rec_cnt[line_rec[ae - l1]])
                        if bnext <= be:
                            bnext = be + 1
                        if le1 - lb1 < ae - as_ or rc < best:
                            lb1, le1, lb2, le2 = as_, ae, bs, be
                            best = rc
                            found = True
                        if np_ < 0:
                            break
                        stop = False
                        while np_ <= ae:
                            np_ = nxt[np_ - l1]
                            if np_ < 0:
                                stop = True
                                break
                        if stop:
                            break
                        as_ = np_
            bptr = bnext
        if has_common and LineDiff.MAX_CHAIN < best:
            return 2, 0, 0, 0, 0
        return (1, lb1, le1, lb2, le2) if found else (0, 0, 0, 0, 0)

    @staticmethod
    def _compact(ch: list, other: list, ids: list, text: list) -> None:
        n, no = len(ids), len(other)
        r = [False] * (n + 2)
        for i in range(n):
            r[i + 1] = ch[i]
        ro = [False] * (no + 2)
        for i in range(no):
            ro[i + 1] = other[i]
        indents = [None] * n

        def indent(i: int) -> int:
            if indents[i] is not None:
                return indents[i]
            ret, v = 0, -1
            for c in text[i].encode("utf-8"):
                if c == 32:
                    ret += 1
                elif c == 9:
                    ret += 8 - ret % 8
                elif c in (11, 12, 13, 10):
                    pass
                else:
                    v = ret
                    break
                if ret >= 200:
                    v = 200
                    break
            indents[i] = v
            return v

        gs = ge = os_ = oe = 0
        while r[ge + 1]:
            ge += 1
        while ro[oe + 1]:
            oe += 1

        def nxt(s, e, rr, cnt):
            if e == cnt:
                return s, e, False
            s = e + 1
            e = s
            while rr[e + 1]:
                e += 1
            return s, e, True

        def prv(s, e, rr):
            if s == 0:
                return s, e, False
            e = s - 1
            s = e
            while rr[s]:
                s -= 1
            return s, e, True

        def slide_down(gs, ge):
            if ge < n and ids[gs] == ids[ge]:
                r[gs + 1] = False
                gs += 1
                r[ge + 1] = True
                ge += 1
                while r[ge + 1]:
                    ge += 1
                return gs, ge, True
            return gs, ge, False

        def slide_up(gs, ge):
            if gs > 0 and ids[gs - 1] == ids[ge - 1]:
                gs -= 1
                r[gs + 1] = True
                ge -= 1
                r[ge + 1] = False
                while r[gs]:
                    gs -= 1
                return gs, ge, True
            return gs, ge, False

        def measure(split: int) -> dict:
            m = {"eof": False, "indent": -1, "preBlank": 0, "preIndent": -1,
                 "postBlank": 0, "postIndent": -1}
            if split >= n:
                m["eof"] = True
                m["indent"] = -1
            else:
                m["indent"] = indent(split)
            i = split - 1
            while i >= 0:
                m["preIndent"] = indent(i)
                if m["preIndent"] != -1:
                    break
                m["preBlank"] += 1
                if m["preBlank"] == 20:
                    m["preIndent"] = 0
                    break
                i -= 1
            i = split + 1
            while i < n:
                m["postIndent"] = indent(i)
                if m["postIndent"] != -1:
                    break
                m["postBlank"] += 1
                if m["postBlank"] == 20:
                    m["postIndent"] = 0
                    break
                i += 1
            return m

        def score(m: dict, s: dict) -> None:
            if m["preIndent"] == -1 and m["preBlank"] == 0:
                s["penalty"] += 1
            if m["eof"]:
                s["penalty"] += 21
            post_blank = 1 + m["postBlank"] if m["indent"] == -1 else 0
            total_blank = m["preBlank"] + post_blank
            s["penalty"] += -30 * total_blank
            s["penalty"] += 6 * post_blank
            ind = m["indent"] if m["indent"] != -1 else m["postIndent"]
            any_ = total_blank != 0
            s["indent"] += ind
            if ind == -1 or m["preIndent"] == -1:
                pass
            elif ind > m["preIndent"]:
                s["penalty"] += 10 if any_ else -4
            elif ind == m["preIndent"]:
                pass
            elif m["postIndent"] != -1 and m["postIndent"] > ind:
                s["penalty"] += 17 if any_ else 24
            else:
                s["penalty"] += 17 if any_ else 23

        def cmp(a: dict, b: dict) -> int:
            ci = (1 if a["indent"] > b["indent"] else 0) - (1 if a["indent"] < b["indent"] else 0)
            return 60 * ci + (a["penalty"] - b["penalty"])

        while True:
            if ge != gs:
                size, earliest, matching = 0, 0, -1
                while True:
                    size = ge - gs
                    matching = -1
                    while True:
                        gs2, ge2, ok = slide_up(gs, ge)
                        if not ok:
                            break
                        gs, ge = gs2, ge2
                        os_, oe, _ = prv(os_, oe, ro)
                    earliest = ge
                    if oe > os_:
                        matching = ge
                    while True:
                        gs2, ge2, ok = slide_down(gs, ge)
                        if not ok:
                            break
                        gs, ge = gs2, ge2
                        os_, oe, _ = nxt(os_, oe, ro, no)
                        if oe > os_:
                            matching = ge
                    if size == ge - gs:
                        break
                if ge == earliest:
                    pass
                elif matching != -1:
                    while oe == os_:
                        gs, ge, _ = slide_up(gs, ge)
                        os_, oe, _ = prv(os_, oe, ro)
                else:
                    shift = earliest
                    if ge - size - 1 > shift:
                        shift = ge - size - 1
                    if ge - 100 > shift:
                        shift = ge - 100
                    best_shift, best_score = -1, {"indent": 0, "penalty": 0}
                    while shift <= ge:
                        s = {"indent": 0, "penalty": 0}
                        score(measure(shift), s)
                        score(measure(shift - size), s)
                        if best_shift == -1 or cmp(s, best_score) <= 0:
                            best_score = s
                            best_shift = shift
                        shift += 1
                    while ge > best_shift:
                        gs, ge, _ = slide_up(gs, ge)
                        os_, oe, _ = prv(os_, oe, ro)
            gs, ge, ok = nxt(gs, ge, r, n)
            if not ok:
                break
            os_, oe, _ = nxt(os_, oe, ro, no)
        for i in range(n):
            ch[i] = r[i + 1]


class TextCompare:
    UNDO_LIMIT = 500
    REDIFF_CONTEXT = 50

    def __init__(self, left=None, right=None, importance=None, ignore_unimportant=False):
        self.left = left or {"lines": [], "eols": [], "encoding": "utf8"}
        self.right = right or {"lines": [], "eols": [], "encoding": "utf8"}
        self.importance = importance or {}
        self.ignore_unimportant = ignore_unimportant
        self.rows = []
        self.sections = []
        self.anchors = []
        self.keys_l = []
        self.keys_r = []
        self.intern = {}
        self.undo_stack = []
        self.redo_stack = []
        if importance is not None or left is not None or right is not None:
            self.recompute()

    def side(self, s: str) -> dict:
        return self.left if s == LEFT else self.right

    def _id(self, k: str) -> int:
        i = self.intern.get(k)
        if i is None:
            i = len(self.intern)
            self.intern[k] = i
        return i

    def _keys(self, t: dict, start: int, end: int) -> list:
        return [self._id(importance_key(t["lines"][i], t["eols"][i], self.importance))
                for i in range(start, end)]

    def recompute(self) -> None:
        self.intern = {}
        self.keys_l = self._keys(self.left, 0, len(self.left["lines"]))
        self.keys_r = self._keys(self.right, 0, len(self.right["lines"]))
        self.anchors = [a for a in self.anchors
                        if a["l"] < len(self.left["lines"]) and a["r"] < len(self.right["lines"])]
        self.rows = self._build_rows(0, len(self.left["lines"]), 0, len(self.right["lines"]))
        self._compute_sections()

    def _build_rows(self, la: int, la_end: int, ra: int, ra_end: int) -> list:
        inside = [a for a in self.anchors if la <= a["l"] < la_end and ra <= a["r"] < ra_end]
        if not inside:
            return self._diff_rows(la, la_end, ra, ra_end)
        out = []
        l0, r0 = la, ra
        for a in inside:
            if a["l"] >= l0 and a["r"] >= r0:
                out += self._diff_rows(l0, a["l"], r0, a["r"])
                out.append({"l": a["l"], "r": a["r"],
                            "kind": ROWS_SAME if self._raw_equal(a["l"], a["r"]) else ROWS_CHANGED,
                            "important": self.keys_l[a["l"]] != self.keys_r[a["r"]]})
                l0, r0 = a["l"] + 1, a["r"] + 1
        out += self._diff_rows(l0, la_end, r0, ra_end)
        return out

    def align(self, left: int, right: int) -> None:
        if not (0 <= left < len(self.left["lines"])) or not (0 <= right < len(self.right["lines"])):
            return
        self.anchors = [a for a in self.anchors
                        if not (a["l"] == left or a["r"] == right or (a["l"] < left) != (a["r"] < right))]
        self.anchors.append({"l": left, "r": right})
        self.anchors.sort(key=lambda a: a["l"])
        self.rows = self._build_rows(0, len(self.left["lines"]), 0, len(self.right["lines"]))
        self._compute_sections()

    def clear_alignment(self, row=None) -> None:
        if row is not None and 0 <= row < len(self.rows):
            rr = self.rows[row]
            self.anchors = [a for a in self.anchors if not (a["l"] == rr["l"] and a["r"] == rr["r"])]
        else:
            self.anchors = []
        self.rows = self._build_rows(0, len(self.left["lines"]), 0, len(self.right["lines"]))
        self._compute_sections()

    def is_anchor(self, row: int) -> bool:
        if not (0 <= row < len(self.rows)):
            return False
        rr = self.rows[row]
        return any(a["l"] == rr["l"] and a["r"] == rr["r"] for a in self.anchors)

    def _diff_rows(self, la: int, la_end: int, ra: int, ra_end: int) -> list:
        A = self.keys_l[la:la_end]
        B = self.keys_r[ra:ra_end]
        hs = LineDiff.hunks(A, B, self.left["lines"][la:la_end], self.right["lines"][ra:ra_end])
        out = []
        i = j = 0

        def matched(i, j):
            li, rj = la + i, ra + j
            same = self._raw_equal(li, rj)
            out.append({"l": li, "r": rj, "kind": ROWS_SAME if same else ROWS_CHANGED, "important": False})

        for h in hs:
            while i < h["a"]:
                matched(i, j)
                i += 1
                j += 1
            self._pair_rows(h, la, ra, out)
            i = h["a"] + h["n"]
            j = h["b"] + h["m"]
        while i < len(A):
            matched(i, j)
            i += 1
            j += 1
        return out

    def _raw_equal(self, li: int, rj: int) -> bool:
        return (self.left["lines"][li].encode("utf-8") == self.right["lines"][rj].encode("utf-8")
                and (self.importance["lineEndings"] or self.left["eols"][li] == self.right["eols"][rj]))

    def _pair_rows(self, h: dict, lo: int, ro: int, out: list) -> None:
        a0, b0 = lo + h["a"], ro + h["b"]

        def row(l, r):
            if l is not None and r is not None:
                if self._raw_equal(l, r):
                    out.append({"l": l, "r": r, "kind": ROWS_SAME, "important": False})
                else:
                    out.append({"l": l, "r": r, "kind": ROWS_CHANGED,
                                "important": self.keys_l[l] != self.keys_r[r]})
            elif l is not None:
                imp = not (self.importance["blankLines"] and importance_blank(self.left["lines"][l]))
                out.append({"l": l, "r": -1, "kind": ROWS_LEFT_ONLY, "important": imp})
            elif r is not None:
                imp = not (self.importance["blankLines"] and importance_blank(self.right["lines"][r]))
                out.append({"l": -1, "r": r, "kind": ROWS_RIGHT_ONLY, "important": imp})

        def zip_rows(a_start, a_end, b_start, b_end):
            count = max(a_end - a_start, b_end - b_start)
            for k in range(count):
                row(a_start + k if k < a_end - a_start else None,
                    b_start + k if k < b_end - b_start else None)

        if h["n"] == 0 or h["m"] == 0 or h["n"] == h["m"] or h["n"] * h["m"] > 4096:
            zip_rows(a0, a0 + h["n"], b0, b0 + h["m"])
            return
        n, m = h["n"], h["m"]
        ga = [self._bigrams(importance_normalized(self.left["lines"][a0 + x], self.importance)) for x in range(n)]
        gb = [self._bigrams(importance_normalized(self.right["lines"][b0 + y], self.importance)) for y in range(m)]
        sim = [0.0] * (n * m)
        for x in range(n):
            for y in range(m):
                sim[x * m + y] = self._dice(ga[x], gb[y])
        w = m + 1
        dp = [0.0] * ((n + 1) * w)
        for x in range(1, n + 1):
            for y in range(1, m + 1):
                v = max(dp[(x - 1) * w + y], dp[x * w + y - 1])
                s = sim[(x - 1) * m + y - 1]
                if s >= 0.5:
                    v = max(v, dp[(x - 1) * w + y - 1] + s)
                dp[x * w + y] = v
        pairs = []
        x, y = n, m
        while x > 0 and y > 0:
            s = sim[(x - 1) * m + y - 1]
            if s >= 0.5 and dp[x * w + y] == dp[(x - 1) * w + y - 1] + s:
                pairs.append((x - 1, y - 1))
                x -= 1
                y -= 1
            elif dp[x * w + y] == dp[(x - 1) * w + y]:
                x -= 1
            else:
                y -= 1
        pairs.reverse()
        pa = pb = 0
        for px, py in pairs:
            zip_rows(a0 + pa, a0 + px, b0 + pb, b0 + py)
            row(a0 + px, b0 + py)
            pa, pb = px + 1, py + 1
        zip_rows(a0 + pa, a0 + n, b0 + pb, b0 + m)

    @staticmethod
    def _bigrams(s: str) -> dict:
        d = {}
        prev = None
        units = s.encode("utf-16-le")
        for i in range(0, len(units) - 1, 2):
            c = units[i] | (units[i + 1] << 8)
            if prev is not None:
                key = (prev << 16) | c
                d[key] = d.get(key, 0) + 1
            prev = c
        return d

    @staticmethod
    def _dice(a: dict, b: dict) -> float:
        ta, tb = sum(a.values()), sum(b.values())
        if ta + tb == 0:
            return 1.0
        common = sum(min(v, b[k]) for k, v in a.items() if k in b)
        return 2.0 * common / (ta + tb)

    def is_diff(self, r: dict) -> bool:
        return r["kind"] != ROWS_SAME and (r["important"] or not self.ignore_unimportant)

    def identical_text(self) -> bool:
        return all(r["kind"] == ROWS_SAME for r in self.rows)

    def important_count(self) -> int:
        return sum(1 for s in self.sections if s["important"])

    def unimportant_count(self) -> int:
        return len(self.sections) - self.important_count()

    def _compute_sections(self) -> None:
        out = []
        i, n = 0, len(self.rows)
        while i < n:
            if self.is_diff(self.rows[i]):
                start = i
                imp = False
                while i < n and self.is_diff(self.rows[i]):
                    imp = imp or self.rows[i]["important"]
                    i += 1
                out.append({"lo": start, "hi": i, "important": imp})
            else:
                i += 1
        self.sections = out

    def replace(self, side: str, start: int, count: int, new: list, eols=None) -> dict:
        t = self.side(side)
        if eols is None:
            e = side_replace(t, start, count, list(new))
        else:
            e = {"start": start, "old": t["lines"][start:start + count],
                 "old_eols": t["eols"][start:start + count], "new": list(new), "new_eols": list(eols)}
            side_apply(t, e)
        self._applied(side, e)
        self.undo_stack.append([side, e])
        if len(self.undo_stack) > self.UNDO_LIMIT:
            self.undo_stack.pop(0)
        self.redo_stack = []
        return e

    def trim_trailing_whitespace(self, side: str) -> int:
        t = self.side(side)
        changed = [i for i, l in enumerate(t["lines"])
                   if l.encode("utf-8")[-1:] in (b" ", b"\t")]

        def trimmed(l: str) -> str:
            u = l.encode("utf-8")
            while u[-1:] in (b" ", b"\t"):
                u = u[:-1]
            return u.decode("utf-8")

        if not changed:
            return 0
        lo, hi = changed[0], changed[-1]
        self.replace(side, lo, hi + 1 - lo, [trimmed(x) for x in t["lines"][lo:hi + 1]],
                     eols=t["eols"][lo:hi + 1])
        return len(changed)

    def convert_line_endings(self, side: str, eol: int) -> int:
        t = self.side(side)
        if eol == 0:
            return 0
        changed = [i for i, e in enumerate(t["eols"]) if e != 0 and e != eol]
        if not changed:
            return 0
        lo, hi = changed[0], changed[-1]
        self.replace(side, lo, hi + 1 - lo, t["lines"][lo:hi + 1],
                     eols=[0 if e == 0 else eol for e in t["eols"][lo:hi + 1]])
        return len(changed)

    def _applied(self, side: str, e: dict, reverse: bool = False) -> None:
        old_count = len(e["new"]) if reverse else len(e["old"])
        new_lines = e["old"] if reverse else e["new"]
        new_eols = e["old_eols"] if reverse else e["new_eols"]
        ks = [self._id(importance_key(l, new_eols[i], self.importance))
              for i, l in enumerate(new_lines)]
        r = slice(e["start"], e["start"] + old_count)
        if side == LEFT:
            self.keys_l[r] = ks
        else:
            self.keys_r[r] = ks
        if not self.anchors:
            self._rediff(side, e["start"], old_count, len(new_lines))
            return
        delta = len(new_lines) - old_count
        kept = []
        for a in self.anchors:
            v = a["l"] if side == LEFT else a["r"]
            if e["start"] <= v < e["start"] + old_count:
                if old_count == len(new_lines):
                    kept.append(a)
                continue
            if v < e["start"] + old_count:
                kept.append(a)
                continue
            kept.append({"l": a["l"] + delta, "r": a["r"]} if side == LEFT
                        else {"l": a["l"], "r": a["r"] + delta})
        self.anchors = kept
        self.rows = self._build_rows(0, len(self.left["lines"]), 0, len(self.right["lines"]))
        self._compute_sections()

    def _rediff(self, side: str, start: int, old_count: int, new_count: int) -> None:
        delta = new_count - old_count
        rows = self.rows
        rs = len(rows)
        for i, r in enumerate(rows):
            if (r["l"] if side == LEFT else r["r"]) >= start:
                rs = i
                break
        re = rs
        if old_count > 0:
            last = start + old_count - 1
            re = rs
            while re < len(rows):
                v = rows[re]["l"] if side == LEFT else rows[re]["r"]
                if v < 0 or v <= last:
                    re += 1
                else:
                    break
        ws, seen = rs, 0
        while ws > 0:
            if rows[ws - 1]["kind"] == ROWS_SAME:
                seen += 1
                if seen > self.REDIFF_CONTEXT:
                    break
            ws -= 1
        we, seen = re, 0
        while we < len(rows):
            if rows[we]["kind"] == ROWS_SAME:
                seen += 1
                if seen > self.REDIFF_CONTEXT:
                    break
            we += 1

        def line_at(s, row, total):
            i = row
            while i < len(rows):
                v = rows[i]["l"] if s == LEFT else rows[i]["r"]
                if v >= 0:
                    return v
                i += 1
            return total

        old_left_total = len(self.left["lines"]) - (delta if side == LEFT else 0)
        old_right_total = len(self.right["lines"]) - (delta if side == RIGHT else 0)
        l0, l1 = line_at(LEFT, ws, old_left_total), line_at(LEFT, we, old_left_total)
        r0, r1 = line_at(RIGHT, ws, old_right_total), line_at(RIGHT, we, old_right_total)
        la_end = l1 + (delta if side == LEFT else 0)
        ra_end = r1 + (delta if side == RIGHT else 0)
        fresh = self._build_rows(l0, la_end, r0, ra_end)
        if delta != 0:
            for i in range(we, len(rows)):
                if side == LEFT and rows[i]["l"] >= 0:
                    rows[i]["l"] += delta
                if side == RIGHT and rows[i]["r"] >= 0:
                    rows[i]["r"] += delta
        rows[ws:we] = fresh
        self._compute_sections()

    def line_index(self, side: str, row: int) -> int:
        i = row
        while i < len(self.rows):
            v = self.rows[i]["l"] if side == LEFT else self.rows[i]["r"]
            if v >= 0:
                return v
            i += 1
        return len(self.side(side)["lines"])

    def line_range(self, side: str, lo: int, hi: int) -> tuple:
        start = self.line_index(side, lo)
        end = (len(self.side(side)["lines"]) if hi >= len(self.rows) else self.line_index(side, hi))
        return start, max(start, end)

    def copy_rows(self, lo: int, hi: int, from_side: str):
        if lo >= hi or hi > len(self.rows):
            return None
        other = RIGHT if from_side == LEFT else LEFT
        src = self.line_range(from_side, lo, hi)
        dst = self.line_range(other, lo, hi)
        lines = self.side(from_side)["lines"][src[0]:src[1]]
        if self.side(other)["lines"][dst[0]:dst[1]] == lines:
            return None
        return self.replace(other, dst[0], dst[1] - dst[0], lines)

    def copy_section(self, index: int, from_side: str):
        if not (0 <= index < len(self.sections)):
            return None
        s = self.sections[index]
        return self.copy_rows(s["lo"], s["hi"], from_side)

    def undo(self, side=None):
        idx = len(self.undo_stack) - 1
        if side is not None:
            match = [i for i, (s, _) in enumerate(self.undo_stack) if s == side]
            if not match:
                return None
            idx = match[-1]
        if idx < 0:
            return None
        s, e = self.undo_stack.pop(idx)
        side_apply(self.side(s), e, reverse=True)
        self._applied(s, e, reverse=True)
        self.redo_stack.append([s, e])
        return e

    def redo(self):
        if not self.redo_stack:
            return None
        s, e = self.redo_stack.pop()
        side_apply(self.side(s), e)
        self._applied(s, e)
        self.undo_stack.append([s, e])
        return e

    def set_importance(self, imp: dict) -> None:
        self.importance = imp
        self.recompute()

    def set_ignore_unimportant(self, on: bool) -> None:
        self.ignore_unimportant = on
        self._compute_sections()

    def swap_sides(self) -> None:
        self.left, self.right = self.right, self.left
        self.anchors = [{"l": a["r"], "r": a["l"]} for a in self.anchors]
        self.undo_stack = [[RIGHT if s == LEFT else LEFT, e] for s, e in self.undo_stack]
        self.redo_stack = [[RIGHT if s == LEFT else LEFT, e] for s, e in self.redo_stack]
        self.recompute()

    def set_side(self, side: str, t: dict) -> None:
        if side == LEFT:
            self.left = t
        else:
            self.right = t
        self.anchors = []
        self.undo_stack = [(s, e) for s, e in self.undo_stack if s != side]
        self.redo_stack = [(s, e) for s, e in self.redo_stack if s != side]
        self.recompute()

    def snapshot(self) -> dict:
        def side_json(t):
            return {"lines": t["lines"], "eols": t["eols"], "encoding": t["encoding"]}

        def counts(stack):
            return [sum(1 for s, _ in stack if s == LEFT), sum(1 for s, _ in stack if s == RIGHT)]

        return {
            "left": side_json(self.left),
            "right": side_json(self.right),
            "rows": [[r["l"], r["r"], r["kind"] | (4 if r["important"] else 0)] for r in self.rows],
            "sections": [[s["lo"], s["hi"], 1 if s["important"] else 0] for s in self.sections],
            "anchors": [[a["l"], a["r"]] for a in self.anchors],
            "undo": counts(self.undo_stack),
            "redo": counts(self.redo_stack),
            "importance": self.importance,
            "ignoreUnimportant": self.ignore_unimportant,
        }

    def visible_rows(self, f: str, context: int):
        if f == "all":
            return None
        if f == "diffs":
            return [i for i, r in enumerate(self.rows) if self.is_diff(r)]
        if f == "same":
            return [i for i, r in enumerate(self.rows) if not self.is_diff(r)]
        keep = [False] * len(self.rows)
        for s in self.sections:
            lo = max(0, s["lo"] - context)
            hi = min(len(self.rows), s["hi"] + context)
            for i in range(lo, hi):
                keep[i] = True
        return [i for i, k in enumerate(keep) if k]

    def section_at(self, row: int):
        lo, hi = 0, len(self.sections) - 1
        while lo <= hi:
            mid = (lo + hi) // 2
            s = self.sections[mid]
            if row < s["lo"]:
                hi = mid - 1
            elif row >= s["hi"]:
                lo = mid + 1
            else:
                return mid
        return None

    def next_section(self, row: int):
        for i, s in enumerate(self.sections):
            if s["lo"] > row:
                return i
        return None

    def prev_section(self, row: int):
        found = None
        for i, s in enumerate(self.sections):
            if s["lo"] < row:
                found = i
        return found


def char_tokens(s: str) -> list:
    out, cur, cur_kind = [], "", 0

    def flush():
        nonlocal cur, cur_kind
        if cur:
            out.append(cur)
            cur = ""
        cur_kind = 0

    chars = list(s)
    for i, ch in enumerate(chars):
        is_word = ch.isalpha() or ch.isdigit() or (
            ch in ("'", "\u2019") and cur_kind == 1 and i + 1 < len(chars) and chars[i + 1].isalpha())
        k = 1 if is_word else 2 if ch.isspace() else 3
        if k == 3:
            flush()
            out.append(ch)
            continue
        if k != cur_kind:
            flush()
            cur_kind = k
        cur += ch
    flush()
    return out


def char_lines(s: str) -> list:
    out, cur = [], ""
    for ch in s:
        cur += ch
        if ch == "\n":
            out.append(cur)
            cur = ""
    if cur:
        out.append(cur)
    return out


def _char_raw_ops(a: str, b: str) -> list:
    x, y = char_tokens(a), char_tokens(b)
    if len(x) * len(y) > 6_000_000:
        x, y = char_lines(a), char_lines(b)
    n, m = len(x), len(y)
    pre = 0
    while pre < n and pre < m and x[pre] == y[pre]:
        pre += 1
    suf = 0
    while suf < n - pre and suf < m - pre and x[n - 1 - suf] == y[m - 1 - suf]:
        suf += 1
    xs, ys = x[pre:n - suf], y[pre:m - suf]
    raw = [[OP_SAME, t] for t in x[:pre]]
    r, c = len(xs), len(ys)
    if r > 0 or c > 0:
        w = c + 1
        L = [[0] * w for _ in range(r + 1)]
        if r > 0 and c > 0:
            for i in range(r - 1, -1, -1):
                for j in range(c - 1, -1, -1):
                    L[i][j] = (L[i + 1][j + 1] + 1 if xs[i] == ys[j]
                               else max(L[i + 1][j], L[i][j + 1]))
        i = j = 0
        while i < r or j < c:
            if i < r and j < c and xs[i] == ys[j]:
                raw.append([OP_SAME, xs[i]])
                i += 1
                j += 1
            elif j < c and (i == r or L[i][j + 1] >= L[i + 1][j]):
                raw.append([OP_INS, ys[j]])
                j += 1
            else:
                raw.append([OP_DEL, xs[i]])
                i += 1
    raw += [[OP_SAME, t] for t in x[n - suf:]]
    return raw


def _char_group(raw: list) -> list:
    ops = [list(o) for o in raw]
    k = 1
    while k < len(ops) - 1:
        if (ops[k][0] == OP_SAME and ops[k][1] and all(c == " " for c in ops[k][1])
                and ops[k - 1][0] != OP_SAME and ops[k + 1][0] != OP_SAME):
            t = ops[k][1]
            ops[k:k + 1] = [[OP_DEL, t], [OP_INS, t]]
            k += 2
        else:
            k += 1
    out, dels, ins = [], "", ""

    def flush():
        nonlocal dels, ins
        if all(c.isspace() for c in dels) and all(c.isspace() for c in ins):
            if ins:
                if out and out[-1][0] == OP_SAME:
                    out[-1][1] += ins
                else:
                    out.append([OP_SAME, ins])
        else:
            if dels:
                out.append([OP_DEL, dels])
            if ins:
                out.append([OP_INS, ins])
        dels = ""
        ins = ""

    for o in ops:
        if o[0] == OP_DEL:
            dels += o[1]
        elif o[0] == OP_INS:
            ins += o[1]
        else:
            flush()
            if out and out[-1][0] == OP_SAME:
                out[-1][1] += o[1]
            else:
                out.append([o[0], o[1]])
    flush()
    return out


def char_diff(a: str, b: str) -> list:
    return _char_group(_char_raw_ops(a, b))


def char_changes(ops: list) -> int:
    n, in_change = 0, False
    for o in ops:
        if o[0] == OP_SAME:
            in_change = False
        elif not in_change:
            n += 1
            in_change = True
    return n


def char_marks(a: str, b: str, imp: dict) -> dict:
    left, right = [], []
    pa = pb = del_start = ins_start = 0
    dels = ins = ""

    def flush():
        nonlocal dels, ins
        if not dels and not ins:
            return
        ws = all(c.isspace() for c in dels) and all(c.isspace() for c in ins)
        unimportant = ((ws and (imp["embeddedWS"] or imp["leadingWS"] or imp["trailingWS"]))
                       or (dels and ins and importance_normalized(dels, imp) == importance_normalized(ins, imp)))
        if dels:
            left.append({"location": del_start, "length": _utf16_len(dels), "important": not unimportant})
        if ins:
            right.append({"location": ins_start, "length": _utf16_len(ins), "important": not unimportant})
        dels = ""
        ins = ""

    for kind, text in _char_raw_ops(a, b):
        length = _utf16_len(text)
        if kind == OP_SAME:
            flush()
            pa += length
            pb += length
        elif kind == OP_DEL:
            if not dels:
                del_start = pa
            if not ins:
                ins_start = pb
            dels += text
            pa += length
        else:
            if not ins:
                ins_start = pb
            if not dels:
                del_start = pa
            ins += text
            pb += length
    flush()
    return {"left": left, "right": right}


def _utf16_len(s: str) -> int:
    return len(s.encode("utf-16-le")) // 2


def binary_first_difference(a: bytes, b: bytes):
    n = min(len(a), len(b))
    for i in range(n):
        if a[i] != b[i]:
            return i
    return None if len(a) == len(b) else n


def text_equal_under_rules(a: str, b: str, imp: dict, limit: int = 4 << 20) -> bool:
    try:
        with open(a, "rb") as f:
            da = f.read()
        with open(b, "rb") as f:
            db = f.read()
    except OSError:
        return False
    if len(da) > limit or len(db) > limit or is_binary(da) or is_binary(db):
        return False
    ta, tb = decode(da), decode(db)
    if ta is None or tb is None:
        return False
    if len(ta["lines"]) != len(tb["lines"]) and not imp["blankLines"]:
        return False
    ka = [importance_key(l, e, imp) for l, e in zip(ta["lines"], ta["eols"])]
    kb = [importance_key(l, e, imp) for l, e in zip(tb["lines"], tb["eols"])]
    if imp["blankLines"]:
        ka = [k for k in ka if k]
        kb = [k for k in kb if k]
    return ka == kb
