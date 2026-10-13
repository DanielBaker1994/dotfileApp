#!/usr/bin/env python3
"""Deterministic verb-table test for the kitchen-sink daemon's socket `do:` verbs.

    python3 -B Tests/test_mcp_verbs.py
    ./ws test mcp-verbs

Walks Tests/mcp_verbs.json against ONE headless sandbox (no AppKit, no window),
reusing the daemon's real socket contract. No LLM judgement: the table is pure
data and every assertion is a concrete reply / state check.

For every table row (see Tests/mcp_verbs.json for the schema):

  headless "ok"      -> each happy action replies with the full state (no
                        "error" key) and every dotted `expect` path holds; each
                        bad action replies {"error": …} containing `bad_error`.
  headless "refused" -> each action replies {"error": "<action> unavailable in
                        headless mode"} and `state` is byte-identical before and
                        after (the verb touches nothing).
  headless "error"   -> the well-formed happy action itself replies
                        {"error": …} containing `bad_error`.

Each row is reset with `do:home` first (order must not matter), plus an optional
`setup` list of actions run before the happy call (for verbs like `back` that
need a prior view history).

A separate coverage test greps `do_host_action` in host.rs for every
`strip_prefix("<x>")`, `starts_with("<x>")` and `"<literal>" =>` arm, and fails
listing any token that has no table row and no `ignore` entry.
"""
from __future__ import annotations

import json
import os
import re
import socket
import subprocess
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SANDBOX_SCRIPT = os.path.join(ROOT, "bin", "ks-mcp-sandbox.sh")
HOST_RS = os.path.join(ROOT, "rust", "ws-rs", "src", "app", "host.rs")
TABLE_PATH = os.path.join(ROOT, "Tests", "mcp_verbs.json")
SANDBOX = os.path.join(ROOT, ".build", "mh", "verbs")

with open(TABLE_PATH, "r", encoding="utf-8") as _f:
    _table = json.load(_f)
VERBS = _table["verbs"]
IGNORE = {t["token"] for t in _table.get("ignore", [])}


def get_path(obj, path):
    """Resolve a dotted path ('views.notes.shown') into a JSON document."""
    for part in path.split("."):
        if isinstance(obj, dict):
            if part not in obj:
                raise KeyError(f"{path!r} (missing {part!r} in {sorted(obj)[:20]})")
            obj = obj[part]
        elif isinstance(obj, list):
            obj = obj[int(part)]
        else:
            raise KeyError(f"{path!r} (no such part {part!r} in {obj!r})")
    return obj


def _sandbox_env():
    env = dict(os.environ)
    env["KS_MCP_SANDBOX"] = SANDBOX
    env["KS_MCP_SANDBOX_NO_BUILD"] = "1"
    return env


class SandboxTestCase(unittest.TestCase):
    """Runs against a single headless sandbox (one per test process)."""

    SOCK = None

    @classmethod
    def setUpClass(cls):
        env = _sandbox_env()
        # Clear any state a previous run left behind so `start` re-seeds the
        # sandbox commands.toml (jira-poll-off persists enabled=false there).
        subprocess.run(
            [SANDBOX_SCRIPT, "reset"], env=env, capture_output=True, text=True
        )
        out = subprocess.run(
            [SANDBOX_SCRIPT, "start"],
            env=env,
            capture_output=True,
            text=True,
            timeout=90,
        )
        if out.returncode != 0:
            raise AssertionError(
                f"sandbox start failed ({out.returncode}):\n{out.stdout}\n{out.stderr}"
            )
        # start prints "running (pid N, sock /path)": capture up to the closing
        # paren (the path itself never contains one).
        m = re.search(r"sock\s+(\S+?)\)", out.stdout)
        if not m:
            raise AssertionError(f"no socket path in start output: {out.stdout!r}")
        cls.SOCK = m.group(1)
        # Wait until the daemon answers, then verify the socket is live.
        for _ in range(50):
            try:
                cls._request("state")
                break
            except (OSError, ValueError):
                import time

                time.sleep(0.1)
        else:
            raise AssertionError("sandbox socket never answered")

    @classmethod
    def tearDownClass(cls):
        if cls.SOCK is None:
            return
        subprocess.run(
            [SANDBOX_SCRIPT, "stop"],
            env=_sandbox_env(),
            capture_output=True,
            text=True,
        )

    @classmethod
    def _request(cls, line):
        """One request per connection: send `line\\n`, read the reply line to EOF."""
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        s.settimeout(15)
        try:
            s.connect(cls.SOCK)
            s.sendall(line.encode("utf-8") + b"\n")
            data = b""
            while True:
                chunk = s.recv(65536)
                if not chunk:
                    break
                data += chunk
            return json.loads(data.decode("utf-8", "replace"))
        finally:
            s.close()

    def reset(self):
        self._request("do:home")


class VerbTableTests(SandboxTestCase):
    def test_every_verb_behaves_per_the_table(self):
        for row in VERBS:
            with self.subTest(verb=row["verb"]):
                self._check_row(row)

    def _check_row(self, row):
        cls_ = row["headless"]
        self.assertIn(cls_, ("ok", "refused", "error"), row["verb"])
        happy = row.get("happy", [])
        bad = row.get("bad", [])
        bad_error = row.get("bad_error", "")
        expect = row.get("expect", {})
        setup = row.get("setup", [])

        for h in happy:
            self.reset()
            for s in setup:
                self._request("do:" + s)
            if cls_ == "refused":
                before = self._request("state")
                reply = self._request("do:" + h)
                self.assertEqual(
                    reply.get("error"),
                    f"{h} unavailable in headless mode",
                    f"{h!r} must be refused with the standard headless message",
                )
                after = self._request("state")
                self.assertEqual(
                    before, after, f"{h!r} must not change state when refused"
                )
            elif cls_ == "error":
                reply = self._request("do:" + h)
                self.assertIn("error", reply, f"{h!r} must error")
                self.assertIn(bad_error, reply["error"], f"{h!r} -> {reply!r}")
            else:  # ok
                reply = self._request("do:" + h)
                self.assertNotIn("error", reply, f"{h!r} must not error: {reply!r}")
                for path, val in expect.items():
                    self.assertEqual(
                        get_path(reply, path), val, f"{h!r} -> {path}"
                    )

        for b in bad:
            self.reset()
            reply = self._request("do:" + b)
            if cls_ == "refused":
                self.assertEqual(
                    reply.get("error"),
                    f"{b} unavailable in headless mode",
                    f"{b!r} must be refused",
                )
            else:
                self.assertIn("error", reply, f"{b!r} must error")
                self.assertIn(bad_error, reply["error"], f"{b!r} -> {reply!r}")

    def test_verb_has_a_happy_and_a_bad_case(self):
        for row in VERBS:
            with self.subTest(verb=row["verb"]):
                self.assertTrue(row.get("happy"), f"{row['verb']} needs a happy case")
                if row["headless"] != "error":
                    # exact verbs and record-only/no-op prefixes may have no bad
                    # form; that must be explained.
                    if not row.get("bad"):
                        self.assertTrue(
                            row.get("bad_note"), f"{row['verb']} has no bad case / note"
                        )


# ---------------------------------------------------------------------------
# Coverage: every verb/prefix in do_host_action must have a table row or an
# `ignore` entry. Grep-based, but scoped to the exact function body via
# brace-matching on a string/comment-masked copy of the source.
# ---------------------------------------------------------------------------


def mask_strings_and_comments(text):
    """Neutralize braces inside string/char literals and comments (blank them)."""
    out = list(text)
    i, n = 0, len(text)
    while i < n:
        c = text[i]
        if c == "/" and i + 1 < n and text[i + 1] == "/":
            while i < n and text[i] != "\n":
                out[i] = " "
                i += 1
            continue
        if c == "/" and i + 1 < n and text[i + 1] == "*":
            out[i] = out[i + 1] = " "
            i += 2
            while i + 1 < n and not (text[i] == "*" and text[i + 1] == "/"):
                out[i] = " "
                i += 1
            if i < n:
                out[i] = " "
                i += 1
            if i < n:
                out[i] = " "
                i += 1
            continue
        if c in ('"', "'"):
            out[i] = " "
            i += 1
            while i < n:
                if text[i] == "\\":
                    out[i] = " "
                    i += 1
                    if i < n:
                        out[i] = " "
                        i += 1
                    continue
                if text[i] == c:
                    out[i] = " "
                    i += 1
                    break
                out[i] = " "
                i += 1
            continue
        i += 1
    return "".join(out)


def extract_fn_body(text, signature):
    idx = text.find(signature)
    if idx == -1:
        raise AssertionError(f"signature not found: {signature!r}")
    brace = text.find("{", idx)
    if brace == -1:
        raise AssertionError(f"no opening brace after {signature!r}")
    masked = mask_strings_and_comments(text)
    depth = 0
    for j in range(brace, len(masked)):
        c = masked[j]
        if c == "{":
            depth += 1
        elif c == "}":
            depth -= 1
            if depth == 0:
                return text[brace : j + 1]
    raise AssertionError(f"unbalanced braces after {signature!r}")


def arm_literals(body):
    """All `"lit" =>` / `"lit" | "lit2" =>` arm names in the body."""
    out = set()
    for m in re.finditer(
        r'"(?:[a-z0-9][a-z0-9-]*)"(?:\s*\|\s*"(?:[a-z0-9][a-z0-9-]*)")*\s*=>', body
    ):
        out.update(re.findall(r'"([a-z0-9][a-z0-9-]*)"', m.group(0)))
    return out


def uncovered_tokens(body, table_verbs, ignore_tokens):
    tokens = set(re.findall(r'strip_prefix\("([^"]+)"\)', body))
    tokens.update(re.findall(r'starts_with\("([^"]+)"\)', body))
    tokens.update(arm_literals(body))
    return sorted(t for t in tokens if t not in table_verbs and t not in ignore_tokens)


class CoverageTests(unittest.TestCase):
    table_verbs = {row["verb"] for row in VERBS}

    def test_every_do_host_action_verb_is_in_the_table(self):
        with open(HOST_RS, "r", encoding="utf-8") as f:
            text = f.read()
        body = extract_fn_body(text, "fn do_host_action")
        uncovered = uncovered_tokens(body, self.table_verbs, IGNORE)
        self.assertEqual(
            [],
            uncovered,
            "verbs/prefixes in do_host_action with no table row and no ignore "
            f"entry: {uncovered}",
        )

    def test_ignore_entries_are_not_dead(self):
        # An ignore entry that no longer appears in the source is stale.
        with open(HOST_RS, "r", encoding="utf-8") as f:
            text = f.read()
        body = extract_fn_body(text, "fn do_host_action")
        present = set(re.findall(r'strip_prefix\("([^"]+)"\)', body))
        present.update(re.findall(r'starts_with\("([^"]+)"\)', body))
        present.update(arm_literals(body))
        stale = sorted(IGNORE - present)
        self.assertEqual([], stale, f"stale ignore entries (not in do_host_action): {stale}")

    def test_table_verbs_are_unique(self):
        verbs = [row["verb"] for row in VERBS]
        self.assertEqual(len(verbs), len(set(verbs)), "duplicate verb rows")


if __name__ == "__main__":
    unittest.main(verbosity=2)
