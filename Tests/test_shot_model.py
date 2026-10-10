#!/usr/bin/env python3
"""shot model — the screenshot suite's non-CG cases, ported 1:1 from
Tests/test_screenshot.swift (ring, snap, pixelate, files, args, colors).

    python3 Tests/test_shot_model.py
"""
from __future__ import annotations

import json
import os
import sys
import tempfile
import time
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(ROOT, "pylib"))

import shot_model as sm


def near(a, b, eps=0.01):
    return abs(a - b) <= eps


def no_overlap(frames):
    for i in range(len(frames)):
        for j in range(i + 1, len(frames)):
            if sm.intersects(sm.inset(frames[i], 0.5, 0.5), frames[j]):
                return False
    return True


class Ring(unittest.TestCase):
    def test_default_ring(self):
        r = sm.ring(sm.DEFAULT_BUTTONS, badge=True)
        self.assertEqual(len(r), 21, "default ring = 20 buttons + the size badge")
        self.assertEqual(r.index("size"), 11, "badge right after the last drawing tool")
        self.assertEqual(r[:11], ["pencil", "line", "arrow", "selection", "rectangle", "circle",
                                  "marker", "text", "counter", "pixelate", "invert"],
                         "bottom-row tools in Flameshot's order")
        self.assertNotIn("size", sm.ring(sm.DEFAULT_BUTTONS, badge=False),
                         "show-size-badge = false drops it")
        self.assertNotIn("accept", r)
        self.assertNotIn("size-increase", r)
        self.assertEqual(sm.ring("copy, nonsense, copy, exit", badge=False), ["copy", "exit"],
                         "unknown + duplicate names dropped")
        self.assertIn("copy-text", sm.ring(sm.DEFAULT_BUTTONS, badge=False))
        self.assertTrue(sm.tool_finishes("copy-text"))
        self.assertEqual(sm.tool_kind("copy-text"), "action")


class RingLayout(unittest.TestCase):
    SCREEN = sm.Rect(0, 0, 1440, 900)
    B = 34.0
    SEL = sm.Rect(500, 300, 400, 250)

    def setUp(self):
        self.count = len(sm.ring(sm.DEFAULT_BUTTONS, badge=True))

    def layout(self, sel):
        return sm.ring_layout(selection=sel, screen=self.SCREEN, count=self.count, button=self.B)

    def test_centered(self):
        l = self.layout(self.SEL)
        frames = l["frames"]
        self.assertEqual(len(frames), self.count)
        self.assertFalse(l["inside"])
        self.assertTrue(all(sm.contains_rect(self.SCREEN, f) for f in frames), "all on screen")
        self.assertTrue(all(sm.intersection(f, self.SEL) is None for f in frames),
                        "none covers the selection")
        self.assertTrue(no_overlap(frames), "no two buttons overlap")
        per_row = int((self.SEL.w + (self.B / 4) // 1) / (self.B + (self.B / 4) // 1))
        self.assertTrue(all(f.minY >= self.SEL.maxY for f in frames[:per_row]), "bottom row first")
        row = frames[:per_row]
        self.assertTrue(all(a.minX < b.minX for a, b in zip(row, row[1:])),
                        "bottom row runs left to right")
        right = frames[per_row]
        self.assertGreaterEqual(right.minX, self.SEL.maxX, "then the right column")
        self.assertLess(frames[per_row + 1].minY, right.minY, "right column runs bottom up")

    def test_edges(self):
        edges = [
            ("left", sm.Rect(0, 300, 300, 200)),
            ("right", sm.Rect(1140, 300, 300, 200)),
            ("top", sm.Rect(500, 0, 300, 200)),
            ("bottom", sm.Rect(500, 700, 300, 200)),
            ("top-left corner", sm.Rect(0, 0, 200, 150)),
            ("bottom-right corner", sm.Rect(1240, 750, 200, 150)),
        ]
        for name, s in edges:
            e = self.layout(s)["frames"]
            self.assertEqual(len(e), self.count, name)
            self.assertTrue(all(f.w == self.B for f in e), name + ": every button placed")
            self.assertTrue(all(sm.contains_rect(self.SCREEN, f) for f in e), name + ": all on screen")
            self.assertTrue(no_overlap(e), name + ": no overlap")

    def test_full_screen_goes_inside(self):
        full = self.layout(self.SCREEN)
        frames = full["frames"]
        self.assertTrue(full["inside"])
        self.assertTrue(all(sm.contains_rect(self.SCREEN, f) and f.w == self.B for f in frames))
        self.assertTrue(no_overlap(frames))
        self.assertGreaterEqual(frames[0].maxY, self.SCREEN.maxY - self.B - 10,
                                "first row at the bottom edge")

    def test_tiny_and_empty(self):
        for name, sel in [("tiny", sm.Rect(700, 400, 4, 4)), ("tiny corner", sm.Rect(0, 0, 6, 6))]:
            t = self.layout(sel)["frames"]
            self.assertTrue(all(sm.contains_rect(self.SCREEN, f) and f.w == self.B for f in t), name)
            self.assertTrue(no_overlap(t), name)
        self.assertEqual(self.layout(self.SEL) and
                         sm.ring_layout(selection=self.SEL, screen=self.SCREEN, count=0,
                                        button=self.B)["frames"], [])

    def test_button_size(self):
        self.assertEqual(sm.default_button_size(15.5), 34, "button size = line height × 2.2")


class Snap(unittest.TestCase):
    O = (0.0, 0.0)

    def test_near_axis_snaps(self):
        a = sm.snap_angle(self.O, (100, 7))
        self.assertTrue(near(a[1], 0) and near(a[0], (100 ** 2 + 7 ** 2) ** 0.5),
                        "near-horizontal snaps to 0°")
        b = sm.snap_angle(self.O, (50, 47))
        self.assertTrue(near(b[0], b[1]), "near-diagonal snaps to 45°")
        c = sm.snap_angle(self.O, (-3, -80))
        self.assertTrue(near(c[0], 0) and c[1] < 0, "near-vertical snaps to 90°")

    def test_eight_directions(self):
        import math
        for k in range(8):
            ang = k * math.pi / 4 + 0.1
            s = sm.snap_angle(self.O, (math.cos(ang) * 50, math.sin(ang) * 50))
            got = math.atan2(s[1], s[0])
            want = k * math.pi / 4
            self.assertLess(abs(math.remainder(got - want, 2 * math.pi)), 1e-6, "%d°" % (k * 45))

    def test_square_and_non_snapping_tools(self):
        self.assertEqual(sm.snap_square((10, 10), (40, -5)), (40.0, -20.0),
                         "square keeps the signs, longer side")
        self.assertEqual(sm.snap("pencil", self.O, (3, 4)), (3, 4), "pencil never snaps")


def pixels(w, h, interior, inner, outer):
    data = bytearray(w * h * 4)
    for y in range(h):
        for x in range(w):
            c = inner(x, y) if sm.contains_point(interior, x + 0.5, y + 0.5) else outer(x, y)
            i = (y * w + x) * 4
            data[i], data[i + 1], data[i + 2], data[i + 3] = c[0], c[1], c[2], 255
    return sm.Pixels(w, h, data)


class Pixelate(unittest.TestCase):
    def test_grid_formula(self):
        g = sm.pixelate_grid(sm.Rect(0, 0, 300, 120), size=2)
        self.assertEqual(g, (50, 20), "block resolution = rect × 0.5 / (size + 1)")

    def test_no_interior_leak(self):
        interior = sm.Rect(30, 30, 40, 40)
        px = pixels(100, 100, interior,
                    lambda x, y: [0, 255, 0],
                    lambda x, y: [120 + x, 60 + y // 2, 60])
        for size in (1, 2, 5, 20):
            blocks = sm.secure_blocks(px, interior, size=size)
            flat = [c for row in blocks for c in row]
            cols, rows = sm.pixelate_grid(interior, size=size)
            self.assertEqual(len(blocks), rows, "size %d: rows" % size)
            self.assertTrue(all(len(r) == cols for r in blocks), "size %d: cols %dx%d" % (size, cols, rows))
            leaked = any(c.g > 0.6 and c.r < 0.3 for c in flat)
            self.assertFalse(leaked, "size %d: no block carries the hidden interior color" % size)

    def test_corner_rect_no_leak(self):
        edge = pixels(60, 60, sm.Rect(0, 0, 30, 30),
                      lambda x, y: [0, 255, 0],
                      lambda x, y: [200, 40, 40])
        eb = [c for row in sm.secure_blocks(edge, sm.Rect(0, 0, 30, 30), size=2) for c in row]
        self.assertFalse(any(c.g > 0.6 for c in eb), "rect at the image corner: no leak")

    def test_retina_grid_is_in_points(self):
        interior = sm.Rect(30, 30, 40, 40)
        px = pixels(100, 100, interior, lambda x, y: [0, 255, 0], lambda x, y: [10, 10, 10])
        retina = sm.secure_blocks(px, interior, size=2, scale=2)
        pts = sm.pixelate_grid(sm.Rect(0, 0, 20, 20), size=2)
        self.assertEqual(len(retina), pts[1])
        self.assertEqual(len(retina[0]), pts[0])


class Files(unittest.TestCase):
    def date(self):
        return time.mktime((2026, 10, 2, 14, 5, 0, 0, 0, -1))

    def test_expand_patterns(self):
        d = self.date()
        self.assertEqual(sm.expand_pattern("%F_%H-%M", d), "2026-10-02_14-05",
                         "Flameshot's default pattern")
        self.assertEqual(sm.expand_pattern("shot %Y/%m", d), "shot 2026-10",
                         "no '/' in a name")

    def test_unique_path(self):
        taken = {"/tmp/x/a.png", "/tmp/x/a 2.png"}
        self.assertEqual(sm.unique_path("/tmp/x", "a", "png", exists=taken.__contains__),
                         "/tmp/x/a 3.png", "clash → ' 3'")
        self.assertEqual(sm.unique_path("/tmp/x/", "b", "png", exists=taken.__contains__),
                         "/tmp/x/b.png", "free name kept")

    def test_target(self):
        d = self.date()
        no = lambda _: False
        self.assertEqual(sm.target("/tmp/x", pattern="%F", fmt="png", date=d,
                                   is_dir=lambda _: True, exists=no),
                         "/tmp/x/2026-10-02.png", "-p DIR → the pattern inside it")
        self.assertEqual(sm.target("/tmp/y/shot", pattern="%F", fmt="jpg", date=d,
                                   is_dir=lambda _: False, exists=no),
                         "/tmp/y/shot.jpg", "-p FILE gets the format's extension")
        self.assertEqual(sm.target("/tmp/y/s.png", pattern="%F", fmt="png", date=d,
                                   is_dir=lambda _: False, exists=no),
                         "/tmp/y/s.png", "-p FILE.png kept")


class Args(unittest.TestCase):
    def test_no_args(self):
        self.assertEqual(sm.parse_args([]), sm.Args(), "no args = gui")

    def test_every_gui_flag(self):
        a = sm.parse_args(["gui", "-p", "/tmp", "-c", "-d", "500", "--region", "300x200+10+20",
                           "-s", "--pin", "-r", "-g"])
        self.assertEqual(a.mode, "gui")
        self.assertEqual(a.path, "/tmp")
        self.assertTrue(a.clipboard)
        self.assertEqual(a.delayMs, 500)
        self.assertEqual(a.region, "300x200+10+20")
        self.assertTrue(a.acceptOnSelect and a.pin and a.raw and a.printGeometry)
        self.assertTrue(a.wantsReply)

    def test_screen_and_text_modes(self):
        a = sm.parse_args(["screen", "-n", "1", "-c"])
        self.assertEqual((a.mode, a.screenNumber, a.clipboard, a.wantsReply),
                         ("screen", 1, True, False))
        a = sm.parse_args(["full", "--region", "screen0"])
        self.assertEqual((a.mode, a.region), ("full", "screen0"))
        a = sm.parse_args(["text", "-r"])
        self.assertEqual((a.mode, a.isOverlay, a.raw), ("text", True, True))

    def test_failures(self):
        for words in (["-d", "x"], ["--bogus"], ["--region", "12"], ["-p"]):
            with self.assertRaises(sm.ArgsProblem, msg=words):
                sm.parse_args(words)

    def test_parse_region(self):
        self.assertEqual(sm.parse_region("300x200+10+20"), sm.Rect(10, 20, 300, 200), "WxH+X+Y")
        self.assertEqual(sm.parse_region("300x200"), sm.Rect(0, 0, 300, 200), "WxH")
        self.assertIsNone(sm.parse_region("0x200+1+1"), "zero width rejected")


class Colors(unittest.TestCase):
    def test_hex_round_trip_and_darkness(self):
        self.assertEqual(sm.Color.from_hex("#740096").hex, "#740096", "hex round trip")
        self.assertTrue(sm.Color.from_hex("#740096").is_dark, "Flameshot purple is dark")
        self.assertFalse(sm.Color.from_hex("#ffff00").is_dark, "yellow is light")
        self.assertIsNone(sm.Color.from_hex("zz"))
        self.assertIsNone(sm.Color.from_hex("#12345"))

    def test_hsv_round_trip(self):
        c = sm.Color.from_hex("#3366cc")
        h, s, v = c.hsv
        self.assertEqual(sm.Color.from_hsv(h, s, v).hex, "#3366cc")


class StateFile(unittest.TestCase):
    def test_round_trip_and_size_defaults(self):
        root = tempfile.mkdtemp(prefix="shot-state-")
        self.addCleanup(lambda: __import__("shutil").rmtree(root, True))
        path = os.path.join(root, "s", "state.json")
        st = sm.State()
        st.sizes["pencil"] = 7
        st.lastRegion = {"display": 1, "x": 0, "y": 0, "w": 10, "h": 10}
        st.save(path)
        back = sm.State.load(path)
        self.assertEqual(back.sizes.get("pencil"), 7)
        self.assertEqual(back.size("pencil"), 7)
        self.assertEqual(back.size("text"), 8, "tool defaults")
        self.assertEqual(back.size("marker"), 5)
        self.assertEqual(back.size("counter"), 1)
        self.assertEqual(back.size("circle"), 3)
        self.assertEqual(back.gridSize, 10)
        self.assertFalse(back.grid)
        self.assertNotIn("color", back.json())
        self.assertEqual(json.loads(json.dumps(back.json()))["lastRegion"]["w"], 10)


if __name__ == "__main__":
    unittest.main(verbosity=2)
