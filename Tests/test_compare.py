#!/usr/bin/env python3
"""compare_text — the text compare engine, ported from CompareText.swift.

Budgets are relaxed vs the Swift suite (the engine now runs through the
python worker): timings print, correctness asserts.
"""
from __future__ import annotations

import os
import random
import subprocess
import sys
import tempfile
import time
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)
sys.path.insert(0, os.path.join(ROOT, "pylib"))

import compare_text as ct

IMP = {"leadingWS": True, "trailingWS": True, "embeddedWS": False,
       "ignoreCase": False, "lineEndings": True, "blankLines": False}
EXACT = {"leadingWS": False, "trailingWS": False, "embeddedWS": False,
         "ignoreCase": False, "lineEndings": False, "blankLines": False}


def side(text: str) -> dict:
    return ct.decode(text.encode())


def kinds(t) -> list:
    return [r["kind"] for r in t.rows]


def consistent(t: ct.TextCompare) -> bool:
    l = r = 0
    for row in t.rows:
        if row["l"] >= 0:
            if row["l"] != l:
                return False
            l += 1
        if row["r"] >= 0:
            if row["r"] != r:
                return False
            r += 1
        if row["kind"] == 0 and (row["l"] < 0 or row["r"] < 0
                                 or t.left["lines"][row["l"]] != t.right["lines"][row["r"]]):
            return False
        if row["kind"] == 2 and (row["l"] < 0 or row["r"] >= 0):
            return False
        if row["kind"] == 3 and (row["r"] < 0 or row["l"] >= 0):
            return False
    return l == len(t.left["lines"]) and r == len(t.right["lines"])


class RoundTrips(unittest.TestCase):
    def test_decodes_encode_byte_exact(self):
        cases = [
            ("utf8 lf", b"a\nb\nc\n", "utf8"),
            ("utf8 no final newline", b"a\nb\nc", "utf8"),
            ("crlf", b"a\r\nb\r\n", "utf8"),
            ("cr", b"a\rb\r", "utf8"),
            ("mixed endings", b"a\r\nb\nc\rd", "utf8"),
            ("blank lines", b"\n\n\n", "utf8"),
            ("empty", b"", "utf8"),
            ("unicode", "héllo wörld\n日本語\n😀 emoji\r\n".encode(), "utf8"),
            ("nfd", b"e\xcc\x81\n", "utf8"),
            ("utf8 bom", b"\xef\xbb\xbfx = 1\r\ny = 2\r\n", "utf8BOM"),
            ("latin1", bytes([0x63, 0x61, 0x66, 0xE9, 0x0A, 0xFF, 0x0A]), "latin1"),
            ("utf16 le", b"\xff\xfe" + "line one\r\nline two\nsmile 😀\n".encode("utf-16-le"), "utf16LE"),
            ("utf16 be", b"\xfe\xff" + "line one\r\nline two\nsmile 😀\n".encode("utf-16-be"), "utf16BE"),
        ]
        for name, data, encoding in cases:
            s = ct.decode(data)
            self.assertIsNotNone(s, name)
            self.assertEqual(s["encoding"], encoding, name)
            self.assertEqual(ct.encode(s), data, "%s: decode -> encode byte-exact" % name)
            if s["lines"]:
                t = ct.TextCompare(s, s, IMP)
                t.replace("left", 0, 1, ["changed", "two"])
                t.undo()
                self.assertEqual(ct.encode(t.left), data, "%s: edit + undo byte-exact" % name)

    def test_binary_detection(self):
        self.assertTrue(ct.is_binary(bytes([0x50, 0x4B, 0x03, 0x04, 0x00, 0x00])))
        self.assertFalse(ct.is_binary(bytes([0xFF, 0xFE, 0x41, 0x00])))

    def test_append_after_no_newline_last_line(self):
        s = ct.decode(b"a\r\nb")
        ct.side_replace(s, 2, 0, ["c"])
        self.assertEqual(ct.encode(s), b"a\r\nb\r\nc")

    def test_new_lines_take_the_sides_crlf(self):
        s = ct.decode(b"a\r\nb\r\n")
        ct.side_replace(s, 1, 1, ["x", "y"])
        self.assertEqual(ct.encode(s), b"a\r\nx\r\ny\r\n")

    def test_latin1_refuses_characters_it_cannot_hold(self):
        s = ct.decode(bytes([0x61, 0xE9, 0x0A]))
        ct.side_replace(s, 0, 1, ["日本"])
        self.assertIsNone(ct.encode(s))


class ImportanceRules(unittest.TestCase):
    def test_normalized(self):
        self.assertEqual(ct.importance_normalized("  foo bar  ", IMP), "foo bar")
        e = dict(IMP, embeddedWS=True)
        self.assertEqual(ct.importance_normalized("  a  b\tc ", e), "abc")
        e2 = dict(IMP, embeddedWS=True, leadingWS=False)
        self.assertEqual(ct.importance_normalized("  a  b ", e2), "  ab")
        c = dict(EXACT, ignoreCase=True)
        self.assertEqual(ct.importance_normalized("FooBAR", c), "foobar")

    def test_key_line_endings(self):
        self.assertEqual(ct.importance_key("x", 2, IMP), ct.importance_key("x", 1, IMP))
        self.assertNotEqual(ct.importance_key("x", 2, EXACT), ct.importance_key("x", 1, EXACT))

    def test_indent_only_change_is_unimportant(self):
        t = ct.TextCompare(side("a\n  b\nc\n"), side("a\nb\nc\n"), IMP)
        self.assertEqual(len(t.rows), 3)
        self.assertEqual(t.rows[1]["kind"], 1)
        self.assertFalse(t.rows[1]["important"])
        self.assertEqual(len(t.sections), 1)
        self.assertFalse(t.sections[0]["important"])
        t.set_ignore_unimportant(True)
        self.assertEqual(t.sections, [])

    def test_blank_lines(self):
        bl = dict(IMP, blankLines=True)
        tb = ct.TextCompare(side("a\n\nb\n"), side("a\nb\n"), bl)
        self.assertEqual(len(tb.sections), 1)
        self.assertFalse(tb.sections[0]["important"])
        tb2 = ct.TextCompare(side("a\n\nb\n"), side("a\nb\n"), IMP)
        self.assertTrue(tb2.sections[0]["important"])

    def test_identical_text(self):
        crlf = ct.TextCompare(side("a\r\nb\r\n"), side("a\nb\n"), IMP)
        self.assertTrue(crlf.identical_text())
        crlf_exact = ct.TextCompare(side("a\r\nb\r\n"), side("a\nb\n"), EXACT)
        self.assertFalse(crlf_exact.identical_text())
        nf = ct.TextCompare(side("caf\u00e9\n"), side("cafe\u0301\n"), EXACT)
        self.assertFalse(nf.identical_text())

    def test_text_equal_under_rules(self):
        tmp = tempfile.mkdtemp(prefix="compare-eq-")
        self.addCleanup(lambda: __import__("shutil").rmtree(tmp, True))
        a = os.path.join(tmp, "a"); b = os.path.join(tmp, "b")
        open(a, "w").write("x = 1\r\ny = 2\r\n")
        open(b, "w").write("x = 1\ny = 2\n")
        self.assertTrue(ct.text_equal_under_rules(a, b, IMP))
        self.assertFalse(ct.text_equal_under_rules(a, b, EXACT))


class Rows(unittest.TestCase):
    def test_row_kinds(self):
        t = ct.TextCompare(side("one\ntwo\nthree\nfour\n"),
                           side("one\nTWO\nthree\nfour\nfive\n"), IMP)
        self.assertEqual(kinds(t), [0, 1, 0, 0, 3])
        self.assertEqual(len(t.sections), 2)
        self.assertEqual(t.next_section(0), 0)
        self.assertEqual(t.next_section(1), 1)
        self.assertEqual(t.prev_section(4), 0)

    def test_deleted_lines_get_fillers(self):
        d = ct.TextCompare(side("a\nb\nc\nd\n"), side("a\nd\n"), IMP)
        self.assertEqual(kinds(d), [0, 2, 2, 0])

    def test_port_lines_paired(self):
        p = ct.TextCompare(side("x\nport = 8080\ny\n"), side("x\nnew line here\nport = 9090\ny\n"), IMP)
        ports = [r for r in p.rows if r["l"] == 1]
        self.assertTrue(ports and ports[0]["r"] == 2 and ports[0]["kind"] == 1, p.rows)

    def test_visible_rows(self):
        big = "".join("line %d\n" % i for i in range(40))
        big2 = "".join(("LINE 20" if i == 20 else "line %d" % i) + "\n" for i in range(40))
        c = ct.TextCompare(side(big), side(big2), IMP)
        self.assertEqual(c.visible_rows("context", 3), list(range(17, 24)))
        self.assertEqual(c.visible_rows("diffs", 3), [20])
        self.assertEqual(len(c.visible_rows("same", 3)), 39)

    def test_char_marks(self):
        m = ct.char_marks("port = 8080", "port = 9090", IMP)
        self.assertEqual(m["left"], [{"location": 7, "length": 4, "important": True}])
        self.assertEqual(m["right"][0]["location"], 7)
        self.assertEqual(m["right"][0]["length"], 4)
        ws = ct.char_marks("a  b", "a b", IMP)
        self.assertTrue(all(not x["important"] for x in ws["left"]))

    def test_char_diff(self):
        ops = ct.char_diff("their going home", "They're gone home")
        self.assertEqual(ct.char_changes(ops), 1)

    def test_binary_first_difference(self):
        self.assertIsNone(ct.binary_first_difference(bytes([1, 2, 3]), bytes([1, 2, 3])))
        self.assertEqual(ct.binary_first_difference(bytes([7] * 100 + [1]), bytes([7] * 100 + [2])), 100)
        self.assertEqual(ct.binary_first_difference(bytes([1, 2]), bytes([1, 2, 3])), 2)


class Edits(unittest.TestCase):
    def test_copy_and_undo(self):
        L = "a\nb\nc\nd\ne\nf\n"
        R = "a\nB\nc\nd\nx\ny\nf\n"
        t = ct.TextCompare(side(L), side(R), IMP)
        self.assertEqual(len(t.sections), 2)
        t.copy_section(0, "left")
        self.assertEqual(t.right["lines"], ["a", "b", "c", "d", "x", "y", "f"])
        self.assertEqual(len(t.sections), 1)
        t.copy_section(0, "right")
        self.assertEqual(t.left["lines"], ["a", "b", "c", "d", "x", "y", "f"])
        self.assertEqual(t.sections, [])
        self.assertTrue(t.identical_text())
        t.undo()
        self.assertEqual(t.left["lines"], ["a", "b", "c", "d", "e", "f"])
        t.undo()
        self.assertEqual(t.right["lines"], ["a", "B", "c", "d", "x", "y", "f"])
        t.redo()
        self.assertEqual(t.right["lines"][1], "b")
        self.assertTrue(consistent(t))

    def test_per_side_undo(self):
        u = ct.TextCompare(side("1\n2\n3\n"), side("1\n2\n3\n"), IMP)
        u.replace("left", 1, 1, ["two"])
        u.replace("right", 0, 1, ["uno", "one"])
        u.undo("left")
        self.assertEqual(u.left["lines"], ["1", "2", "3"])
        self.assertEqual(u.right["lines"], ["uno", "one", "2", "3"])

    def test_copying_a_filler_deletes_the_line(self):
        c = ct.TextCompare(side("a\nb\nc\n"), side("a\nc\n"), IMP)
        filler = next(i for i, r in enumerate(c.rows) if r["kind"] == 2)
        c.copy_rows(filler, filler + 1, "right")
        self.assertEqual(c.left["lines"], ["a", "c"])

    def test_fuzz_windowed_matches_full(self):
        rng = random.Random(42)
        base = ["" if i % 7 == 0 else "    let v%d = compute(%d)" % (i % 50, i) for i in range(400)]
        f = ct.TextCompare(side("\n".join(base) + "\n"), side("\n".join(base) + "\n"), IMP)
        same_as_full = edits = 0
        for _ in range(120):
            s = "left" if rng.randrange(2) == 0 else "right"
            n = len(f.side(s)["lines"])
            a = rng.randrange(max(1, n))
            length = rng.randrange(4)
            new = ["edit %d" % rng.randrange(1000) for _ in range(rng.randrange(4))]
            f.replace(s, a, min(length, n - a), new)
            edits += 1
            self.assertTrue(consistent(f), "rows inconsistent after edit %d" % edits)
            full = ct.TextCompare(f.left, f.right, IMP)
            if full.rows == f.rows:
                same_as_full += 1
            if rng.randrange(5) == 0:
                f.undo()
                self.assertTrue(consistent(f), "rows inconsistent after undo")
        self.assertGreaterEqual(same_as_full * 100, edits * 90,
                                "windowed re-diff = full diff in %d/%d" % (same_as_full, edits))


class AlignTrimConvert(unittest.TestCase):
    def test_align(self):
        a = ct.TextCompare(side("a\nx\nb\nc\n"), side("a\nb\nc\ny\n"), IMP)
        before = len(a.rows)
        a.align(1, 3)
        row = next(i for i, r in enumerate(a.rows) if r["l"] == 1)
        self.assertEqual(a.rows[row]["r"], 3)
        self.assertEqual(a.rows[row]["kind"], 1)
        self.assertTrue(a.is_anchor(row))
        self.assertTrue(consistent(a))
        self.assertGreaterEqual(len(a.rows), before)
        a.replace("left", 0, 0, ["new"])
        self.assertEqual(a.anchors[0], {"l": 2, "r": 3})
        self.assertTrue(consistent(a))
        a.undo()
        self.assertEqual(a.anchors[0]["l"], 1)
        a.replace("left", 0, 3, ["A", "X", "B"], eols=[1, 1, 1])
        self.assertEqual(a.anchors[0], {"l": 1, "r": 3})
        a.replace("left", 1, 1, [])
        self.assertEqual(a.anchors, [])

    def test_clear_alignment(self):
        a = ct.TextCompare(side("a\nx\nb\nc\n"), side("a\nb\nc\ny\n"), IMP)
        a.align(1, 3)
        a.clear_alignment()
        self.assertEqual(a.anchors, [])
        self.assertEqual(a.rows, ct.TextCompare(a.left, a.right, IMP).rows)

    def test_crossing_anchor_dropped(self):
        c = ct.TextCompare(side("1\n2\n3\n"), side("1\n2\n3\n"), IMP)
        c.align(0, 2)
        c.align(2, 0)
        self.assertEqual(len(c.anchors), 1)
        self.assertEqual(c.anchors[0]["l"], 2)
        self.assertTrue(consistent(c))

    def test_swap_flips_anchor(self):
        w = ct.TextCompare(side("1\n2\n"), side("1\n2\n"), IMP)
        w.align(1, 0)
        w.swap_sides()
        self.assertEqual(w.anchors[0], {"l": 0, "r": 1})

    def test_trim_and_convert(self):
        raw = "a  \r\nb\t\r\nc\r\nd "
        t = ct.TextCompare(ct.decode(raw.encode()), side("a\nb\nc\nd\n"), IMP)
        orig = ct.encode(t.left)
        self.assertEqual(t.trim_trailing_whitespace("left"), 3)
        self.assertEqual(t.left["lines"], ["a", "b", "c", "d"])
        self.assertEqual(t.left["eols"], [2, 2, 2, 0])
        self.assertEqual(t.trim_trailing_whitespace("left"), 0)
        t.undo()
        self.assertEqual(ct.encode(t.left), orig)
        self.assertEqual(t.convert_line_endings("left", 1), 3)
        self.assertEqual(t.left["eols"], [1, 1, 1, 0])
        self.assertTrue(consistent(t))
        t.undo()
        self.assertEqual(ct.encode(t.left), orig)


def _git_hunks(a, b):
    env = dict(os.environ, GIT_CONFIG_GLOBAL="/dev/null", GIT_CONFIG_NOSYSTEM="1")
    p = subprocess.run(["/usr/bin/git", "diff", "--no-index", "--histogram",
                        "--indent-heuristic", "-U0", "--no-color", "--no-ext-diff", a, b],
                       capture_output=True, text=True, env=env)
    out = []
    for line in p.stdout.split("\n"):
        if not line.startswith("@@ "):
            continue
        parts = line.split(" ")

        def rng(s):
            nums = [int(x) for x in s[1:].split(",") if x.isdigit()]
            return nums[0], (nums[1] if len(nums) > 1 else 1)

        a0, n = rng(parts[1])
        b0, m = rng(parts[2])
        out.append((a0 if n == 0 else a0 - 1, n, b0 if m == 0 else b0 - 1, m))
    return out


def _our_hunks(da, db):
    l, r = ct.decode(da), ct.decode(db)
    ids = {}

    def key(t):
        out = []
        for i, line in enumerate(t["lines"]):
            k = ct.importance_key(line, t["eols"][i], EXACT)
            if k not in ids:
                ids[k] = len(ids)
            out.append(ids[k])
        return out

    hs = ct.LineDiff.hunks(key(l), key(r), l["lines"], r["lines"])
    return [(h["a"], h["n"], h["b"], h["m"]) for h in hs]


@unittest.skipUnless(os.path.exists("/usr/bin/git"), "git missing")
class Parity(unittest.TestCase):
    def test_parity_with_git_histogram(self):
        exts = {".swift", ".py", ".sh", ".md", ".toml", ".vim"}
        files = []
        for base, dirs, names in os.walk(ROOT):
            if ".git" in base or ".build" in base:
                continue
            for n in sorted(names):
                p = os.path.join(base, n)
                if os.path.splitext(n)[1] in exts and os.path.getsize(p) < 400_000:
                    files.append(p)
        rng = random.Random(2026)
        rng.shuffle(files)
        tmp = tempfile.mkdtemp(prefix="compare-parity-")
        self.addCleanup(lambda: __import__("shutil").rmtree(tmp, True))
        pairs = []
        for f in files:
            data = open(f, "rb").read()
            s = ct.decode(data)
            if s is None or not (5 < len(s["lines"]) < 20000) or 3 in s["eols"]:
                continue
            for k in range(2):
                m = s["lines"][:]
                for _ in range(rng.randint(1, 6)):
                    if len(m) < 3:
                        break
                    at = rng.randrange(len(m))
                    ln = min(len(m) - at, 1 + rng.randrange(12))
                    op = rng.randrange(7)
                    if op == 0:
                        del m[at:at + ln]
                    elif op == 1:
                        src = rng.randrange(len(m))
                        m[at:at] = m[src:src + ln]
                    elif op == 2:
                        for i in range(at, min(len(m), at + ln)):
                            if rng.randrange(2):
                                m[i] = m[i].replace("e", "E") + " // x"
                    elif op == 3:
                        blk = m[at:at + ln]
                        del m[at:at + ln]
                        m[rng.randrange(len(m) + 1):0] = blk
                    elif op == 4:
                        for i in range(at, min(len(m), at + ln)):
                            m[i] = "    " + m[i]
                    elif op == 5:
                        m[at:at] = [""] * (1 + rng.randrange(3))
                    else:
                        m[at:at] = ["new line %d %d" % (i, rng.randrange(100)) for i in range(ln)]
                text = "\n".join(m) + ("" if s["eols"] and s["eols"][-1] == 0 else "\n")
                pairs.append((os.path.basename(f), data, text.encode()))
            if len(pairs) >= 60:
                break
        self.assertGreaterEqual(len(pairs), 30, "corpus too small")
        same = 0
        for i, (name, a, b) in enumerate(pairs):
            pa, pb = os.path.join(tmp, "p%d-a" % i), os.path.join(tmp, "p%d-b" % i)
            open(pa, "wb").write(a)
            open(pb, "wb").write(b)
            g = _git_hunks(pa, pb)
            o = _our_hunks(a, b)
            if g == o:
                same += 1
            else:
                print("  parity miss %s: git %d hunks, ours %d" % (name, len(g), len(o)))
            if a != b:
                t = ct.TextCompare(ct.decode(a), ct.decode(b), EXACT)
                self.assertFalse(t.identical_text(), "%s: differing files never identical" % name)
        pct = 100.0 * same / len(pairs)
        print("  parity with git --histogram: %d/%d (%.1f%%)" % (same, len(pairs), pct))
        self.assertGreaterEqual(pct, 95.0, ">= 95%% hunk parity")


class Timings(unittest.TestCase):
    def test_ten_k_lines(self):
        n = 10_000
        rng = random.Random(n)
        a, b = [], []
        for i in range(n):
            indent = " " * (4 * rng.randrange(4))
            line = "" if rng.randrange(9) == 0 else "%slet value%d = item[%d] + %d" % (indent, rng.randrange(500), i % 997, rng.randrange(100))
            a.append(line)
            b.append(line + " // changed" if rng.randrange(100) == 0 else line)
        t0 = time.time()
        l, r = side("\n".join(a) + "\n"), side("\n".join(b) + "\n")
        t_decode = time.time() - t0
        t1 = time.time()
        t = ct.TextCompare(l, r, IMP)
        t_diff = time.time() - t1
        mid = len(t.rows) // 2
        line = t.line_index("left", mid)
        t2 = time.time()
        t.replace("left", line, 1, ["edited line"])
        t_edit = time.time() - t2
        print("  10k lines: decode %.0f ms, diff %.0f ms (%d sections), edit re-diff %.0f ms"
              % (t_decode * 1000, t_diff * 1000, len(t.sections), t_edit * 1000))
        self.assertTrue(consistent(t))


def _splitmix(state):
    while True:
        state = (state + 0x9E3779B97F4A7C15) & 0xFFFFFFFFFFFFFFFF
        z = state
        z = ((z ^ (z >> 30)) * 0xBF58476D1CE4E5B9) & 0xFFFFFFFFFFFFFFFF
        z = ((z ^ (z >> 27)) * 0x94D049BB133111EB) & 0xFFFFFFFFFFFFFFFF
        yield z ^ (z >> 31)


if __name__ == "__main__":
    unittest.main(verbosity=2)
