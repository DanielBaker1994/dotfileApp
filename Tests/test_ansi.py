#!/usr/bin/env python3
"""ansi — the Swift suite's sgr/grid/width/theme cases, ported 1:1.

    python3 Tests/test_ansi.py
"""
from __future__ import annotations

import os
import sys
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(ROOT, "pylib"))

import ansi

ESC = "\u001b"


def style(params, from_=None):
    s = from_.copy() if from_ else ansi.Style()
    ansi.apply_sgr(params, s)
    return s


def text(row):
    return "".join(c.text for c in row)


class SGR(unittest.TestCase):
    def test_extended_colors(self):
        self.assertEqual(style("38;5;12").fg, ansi.index(12), "256-color fg")
        self.assertEqual(style("48;5;8").bg, ansi.index(8), "256-color bg")
        self.assertEqual(style("38;2;10;20;30").fg, ansi.rgb(10, 20, 30), "truecolor fg")
        self.assertEqual(style("38:2:10:20:30").fg, ansi.rgb(10, 20, 30), "colon-separated truecolor")

    def test_basic_and_bright(self):
        self.assertEqual(style("31").fg, ansi.index(1))
        self.assertEqual(style("94").fg, ansi.index(12), "basic + bright fg")
        self.assertEqual(style("41").bg, ansi.index(1))
        self.assertEqual(style("103").bg, ansi.index(11), "basic + bright bg")

    def test_attributes(self):
        all_on = style("1;2;3;4;7;9")
        self.assertTrue(all_on.bold and all_on.dim and all_on.italic
                        and all_on.underline and all_on.inverse and all_on.strike, "attributes on")
        self.assertEqual(style("0", from_=all_on), ansi.Style(), "0 resets")
        self.assertEqual(style("", from_=all_on), ansi.Style(), "empty = reset")
        off = style("22;23;24;27;29", from_=style("1;2;3;4;7;9"))
        self.assertFalse(off.bold or off.dim or off.italic or off.underline
                         or off.inverse or off.strike, "attributes off")
        self.assertEqual(style("39", from_=style("31")).fg, ansi.NONE, "39 = default fg")
        mixed = style("1;38;5;200;4")
        self.assertEqual(mixed.fg, ansi.index(200))
        self.assertTrue(mixed.underline, "params after an extended color")
        self.assertEqual(style("38;5").fg, ansi.NONE, "truncated extended color ignored")


class Grids(unittest.TestCase):
    def test_styled_rows(self):
        g = ansi.parse("%s[0m%s[38;5;7mab%s[0m\r\n\r\ncd%s[1me\r\n" % (ESC, ESC, ESC, ESC))
        self.assertEqual(len(g.rows), 3, "rows")
        self.assertEqual(text(g.rows[0]), "ab")
        self.assertEqual(g.rows[0][0].style.fg, ansi.index(7), "styled row")
        self.assertEqual(g.rows[1], [], "empty row kept")
        self.assertTrue(g.rows[2][2].style.bold and not g.rows[2][1].style.bold,
                        "style starts mid-row")
        self.assertEqual(g.columns, 3, "columns = widest row")

    def test_blank_row_trimming(self):
        self.assertEqual(len(ansi.parse("x\n   \n\n").rows), 1, "trailing blank rows trimmed")
        kept = ansi.parse("x\n%s[41m  %s[0m\n" % (ESC, ESC))
        self.assertEqual(len(kept.rows), 2, "a blank row with a background is content")

    def test_cr_tab_and_escapes(self):
        self.assertEqual(text(ansi.parse("hello\rJ\n").rows[0]), "Jello",
                         "lone CR overwrites from column 0")
        self.assertEqual(text(ansi.parse("a\tb").rows[0]), "a       b", "tab to column 8")
        osc = ansi.parse("%s]8;;https://x.y\u0007link%s]8;;%s\\ done" % (ESC, ESC, ESC))
        self.assertEqual(text(osc.rows[0]), "link done", "OSC 8 links dropped")
        csi = ansi.parse("a%s[2Kb%s[?25lc%s(Bd" % (ESC, ESC, ESC))
        self.assertEqual(text(csi.rows[0]), "abcd", "other CSI / charset escapes dropped")
        self.assertEqual(text(ansi.parse("a\u0007b\u0008c").rows[0]), "abc",
                         "control characters dropped")


class Widths(unittest.TestCase):
    def test_cell_widths(self):
        self.assertEqual(ansi.cell_width("a"), 1)
        self.assertEqual(ansi.cell_width("─"), 1, "narrow")
        self.assertEqual(ansi.cell_width("中"), 2)
        self.assertEqual(ansi.cell_width("한"), 2, "CJK wide")
        self.assertEqual(ansi.cell_width("😀"), 2, "emoji wide")
        self.assertEqual(ansi.cell_width("❤\ufe0f"), 2, "VS16 makes it wide")
        self.assertEqual(ansi.cell_width("\uf121"), 1, "nerd font icon (private use) narrow")

    def test_wide_cells_occupy_two_columns(self):
        g = ansi.parse("中x")
        self.assertEqual(len(g.rows[0]), 3)
        self.assertEqual(g.rows[0][1].text, "")
        self.assertEqual(g.rows[0][2].text, "x", "wide cell + its right half")
        self.assertEqual(len(ansi.parse("e\u0301x").rows[0]), 2,
                         "combining mark rides in its grapheme")


class Themes(unittest.TestCase):
    def ghostty(self):
        return ansi.ghostty("""
font-family = JetBrainsMono Nerd Font
font-family = Fallback Font
font-size = 15
background = #1a1b26
foreground = #c0caf5
palette = 4=#7aa2f7
palette = 200=#123456
bold-is-bright = true
window-colorspace = display-p3
junk line
""")

    def test_xterm256(self):
        x = ansi.xterm256()
        self.assertEqual(len(x), 256, "256 colors")
        self.assertEqual(x[16], ansi.RGB(0, 0, 0))
        self.assertEqual(x[21], ansi.RGB(0, 0, 255))
        self.assertEqual(x[196], ansi.RGB(255, 0, 0), "cube")
        self.assertEqual(x[232], ansi.RGB(8, 8, 8))
        self.assertEqual(x[255], ansi.RGB(238, 238, 238), "grays")

    def test_ghostty(self):
        t = self.ghostty()
        self.assertEqual(t.font_name, "JetBrainsMono Nerd Font", "first font-family wins")
        self.assertEqual(t.font_size, 15.0, "font-size")
        self.assertEqual(t.background, ansi.RGB(0x1A, 0x1B, 0x26))
        self.assertEqual(t.foreground, ansi.RGB(0xC0, 0xCA, 0xF5), "fg / bg")
        self.assertEqual(t.palette[4], ansi.RGB(0x7A, 0xA2, 0xF7))
        self.assertEqual(t.palette[200], ansi.RGB(0x12, 0x34, 0x56), "palette")
        self.assertTrue(t.bold_is_bright and t.display_p3, "bold-is-bright + colorspace")

    def test_colors(self):
        t = self.ghostty()
        s = ansi.Style()
        fg, bg = t.colors(s)
        self.assertEqual(fg, t.foreground)
        self.assertIsNone(bg, "defaults")
        s.fg = ansi.index(1)
        s.bold = True
        self.assertEqual(t.colors(s)[0], t.palette[9], "bold-is-bright")
        inv = ansi.Style()
        inv.inverse = True
        fg, bg = t.colors(inv)
        self.assertEqual(fg, t.background)
        self.assertEqual(bg, t.foreground, "inverse swaps")


if __name__ == "__main__":
    unittest.main(verbosity=2)
