#!/usr/bin/env python3
"""prose_pdf — the notes PDF logic, ported from ProsePDF.swift.

    python3 Tests/test_prose_pdf.py
"""
from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)
sys.path.insert(0, os.path.join(ROOT, "pylib"))

import prose_pdf as pp

HAVE_TOOLS = (os.access(pp.DEFAULTS["pandoc"], os.X_OK)
              and os.access(pp.DEFAULTS["engine"], os.X_OK))

FENCE = "```cpp\n#include <iostream>\nint main(){ std::cout << \"hello\"; }\n```\n"


def call(method, params):
    env = dict(os.environ)
    env["PYTHONPATH"] = os.path.join(ROOT, "pylib")
    env["PYTHONDONTWRITEBYTECODE"] = "1"
    r = subprocess.run(
        [sys.executable, "-B", "-m", "helper", "--call", method, "--params",
         json.dumps(params)],
        capture_output=True, text=True, cwd=ROOT, env=env,
    )
    return r.returncode, json.loads(r.stdout)


class Args(unittest.TestCase):
    def test_output_path(self):
        self.assertEqual(
            pp.output_path("/n/My Note.md", pp.config({"outDir": "~/Out"})),
            os.path.expanduser("~/Out") + "/My Note.pdf")

    def test_empty_stem_is_note(self):
        self.assertEqual(pp.output_path("/n/", pp.config({"outDir": "/o"})), "/o/note.pdf")

    def test_pandoc_argv(self):
        args = pp.pandoc_args("/n/a.md", "/c.html", "/t/pdf.html", pp.config({}))
        self.assertIn("--syntax-highlighting=tango", args)
        self.assertIn("--include-in-header=/c.html", args)
        self.assertIn("--resource-path=/n", args)
        self.assertEqual(args[-1], "/n/a.md")

    def test_diagrams_lua_beside_pdf_css(self):
        tmp = tempfile.mkdtemp(prefix="ws-filter-test")
        self.addCleanup(shutil.rmtree, tmp, True)
        with open(os.path.join(tmp, "diagrams.lua"), "w") as f:
            f.write("return {}")
        args = pp.pandoc_args("/n/a.md", "/c.html", "/t/pdf.html",
                              pp.config({"css": tmp + "/style.css"}))
        self.assertIn("--lua-filter=%s/diagrams.lua" % tmp, args)

    def test_filter_none(self):
        args = pp.pandoc_args("/n/a.md", "/c.html", "/t/pdf.html",
                              pp.config({"filter": "none"}))
        self.assertFalse([a for a in args if a.startswith("--lua-filter")])

    def test_missing_filter_is_skipped(self):
        self.assertEqual(pp.filter_paths(pp.config({"filter": "/nope/missing.lua"})), [])

    def test_engine_argv(self):
        self.assertEqual(pp.engine_args("/n/a.md", "/t/p.html", "/o.pdf"),
                         ["--pdf-tags", "-u", "file:///n/", "/t/p.html", "/o.pdf"])


@unittest.skipUnless(os.access(pp.DEFAULTS["pandoc"], os.X_OK), "pandoc missing")
class Export(unittest.TestCase):
    def test_missing_engine_names_it(self):
        r = pp.export("/n/a.md", pp.config({"engine": "/nonexistent/weasyprint"}))
        self.assertFalse(r["ok"])
        self.assertIn("weasyprint", r["error"])

    def test_missing_note_names_it(self):
        r = pp.export("/nope/missing.md", pp.config({}))
        self.assertFalse(r["ok"])
        self.assertIn("note not found", r["error"])


@unittest.skipUnless(HAVE_TOOLS, "pandoc / weasyprint missing")
class Real(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="prose-pdf-")
        self.addCleanup(shutil.rmtree, self.tmp, True)
        self.cfg = pp.config({"outDir": self.tmp + "/out", "cacheDir": self.tmp + "/cache"})

    def write(self, name, text):
        path = os.path.join(self.tmp, name)
        with open(path, "w", encoding="utf-8") as f:
            f.write(text)
        return path

    def test_export_is_a_pdf(self):
        note = self.write("demo note.md", "# Demo\n\n> [!NOTE]\n> hi\n\n" + FENCE)
        r = pp.export(note, self.cfg)
        self.assertTrue(r["ok"], r.get("error"))
        self.assertEqual(r["out"], self.tmp + "/out/demo note.pdf")
        with open(r["out"], "rb") as f:
            self.assertEqual(f.read(5), b"%PDF-")
        self.assertEqual(os.listdir(self.cfg["cacheDir"]), [])

    def test_sourcepos_task_list(self):
        tasks = self.write("tasks.md", "- [ ] todo\n- [x] done\n  - [ ] nested\n- plain\n")
        sh = pp.screen_html(tasks, self.cfg)
        self.assertIsNotNone(sh)
        self.assertIn('class="task-list"', sh)
        self.assertIn('<input type="checkbox" />', sh)
        self.assertIn('checked=""', sh)
        self.assertNotIn("☐", sh)
        self.assertNotIn("☒", sh)
        self.assertIn("data-pos=", sh)

    def test_missing_pandoc_is_none(self):
        self.assertIsNone(pp.screen_html("/n/a.md", pp.config({"pandoc": "/nonexistent/pandoc"})))


class Protocol(unittest.TestCase):
    def test_css_content(self):
        code, reply = call("prose.css_content", {"config": {}})
        self.assertEqual(code, 0)
        self.assertIn("@page", reply["result"]["css"])

    def test_pdf_export_failure_is_data(self):
        code, reply = call("prose.pdf_export",
                           {"note": "/n/a.md", "config": {"engine": "/nonexistent/weasyprint"}})
        self.assertEqual(code, 0)
        self.assertFalse(reply["result"]["ok"])
        self.assertIn("not found", reply["result"]["error"])

    def test_pandoc_args(self):
        code, reply = call("prose.pandoc_args",
                           {"note": "/n/a.md", "css": "/c.html", "html": "/t/p.html",
                            "config": {}, "sourcepos": False})
        self.assertEqual(code, 0)
        self.assertEqual(reply["result"]["args"][-1], "/n/a.md")
        self.assertIn("--syntax-highlighting=tango", reply["result"]["args"])


if __name__ == "__main__":
    unittest.main(verbosity=2)
