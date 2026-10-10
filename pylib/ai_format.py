from __future__ import annotations

import math
import os
import re
import subprocess

CODE_INSTRUCTION = "Tokens like [[CODE1]] stand for code: keep every one exactly as written, in place."

FILLER = {
    "first", "second", "third", "fourth", "fifth", "firstly", "secondly", "thirdly", "then", "next",
    "finally", "lastly", "and", "also", "is", "are", "was", "were", "has", "have", "had", "with", "the",
    "a", "an", "of", "at", "in", "on", "it", "to", "for", "that", "which", "or",
}

_SEPARATOR = re.compile(r"^\s*\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)*\|?\s*$")
_RULE_LINE = re.compile(r"^[-=]{3,}$")
_NUMBERED = re.compile(r"^\d+[.)] ")
_INLINE_CODE = re.compile(r"`[^`\n]+`")
_SHQ_OK = re.compile(r"^[A-Za-z0-9_@./:=+-]+$")


def _is_separator(s: str) -> bool:
    return _SEPARATOR.match(s) is not None


def _cells(row: str) -> list:
    s = row.strip()
    if s.startswith("|"):
        s = s[1:]
    if s.endswith("|") and not s.endswith("\\|"):
        s = s[:-1]
    parts = s.replace("\\|", "\x01").split("|")
    return [re.sub(r"\*\*|__|`", "", p.replace("\x01", "|")).strip() for p in parts]


def tables_as_text(md: str) -> str:
    lines = md.split("\n")
    out = []
    i = 0
    in_fence = False
    while i < len(lines):
        t = lines[i].strip()
        if t.startswith("```") or t.startswith("~~~"):
            in_fence = not in_fence
        if not in_fence and t.startswith("|") and i + 1 < len(lines) and _is_separator(lines[i + 1]):
            rows = [_cells(lines[i])]
            j = i + 2
            while j < len(lines) and lines[j].strip().startswith("|"):
                rows.append(_cells(lines[j]))
                j += 1
            n = max((len(r) for r in rows), default=0)
            rows2 = [r + [""] * (n - len(r)) for r in rows]
            w = [max((len(r[c]) for r in rows2), default=0) for c in range(n)]

            def line(r):
                return "  ".join(cell.ljust(w[o]) for o, cell in enumerate(r)).strip()

            out.append("```")
            out.append(line(rows2[0]))
            out.append("  ".join("-" * max(1, x) for x in w))
            out += [line(r) for r in rows2[1:]]
            out.append("```")
            i = j
            continue
        out.append(lines[i])
        i += 1
    return "\n".join(out)


def markdown_for(md: str, target: str) -> str:
    return tables_as_text(md) if target == "webex" else md


def pandoc_html(md: str, highlight: bool, pandoc_bin: str):
    if not pandoc_bin or not os.access(os.path.expanduser(pandoc_bin), os.X_OK):
        return None
    p = subprocess.run([os.path.expanduser(pandoc_bin), "-f", "gfm", "-t", "html",
                        "--syntax-highlighting=%s" % ("default" if highlight else "none"),
                        "--wrap=none"],
                       input=md, capture_output=True, text=True)
    return p.stdout if p.returncode == 0 else None


def styled(fragment: str, target: str) -> str:
    font = ("Aptos,Calibri,'Segoe UI',Helvetica,Arial,sans-serif" if target == "outlook"
            else "-apple-system,'Segoe UI',Helvetica,Arial,sans-serif")
    size = "11pt" if target == "outlook" else "10.5pt"
    mono = "Menlo,Consolas,'Courier New',monospace"
    cell = "border:1px solid #bfbfbf;padding:4px 10px;vertical-align:top;"
    h = fragment

    def sub(pattern, repl):
        nonlocal h
        h = re.sub(pattern, repl, h)

    sub(r"<table[^>]*>",
        '<table style="border-collapse:collapse;margin:6px 0 10px 0;font-family:%s;font-size:%s">' % (font, size))
    sub(r'<th( style="([^"]*)")?>',
        '<th style="%sbackground:#f2f2f2;font-weight:bold;text-align:left;\\g<2>">' % cell)
    sub(r'<td( style="([^"]*)")?>', '<td style="%s\\g<2>">' % cell)
    sub(r"<pre[^>]*>\s*<code[^>]*>",
        '<pre style="background:#f6f8fa;border:1px solid #d0d7de;border-radius:4px;'
        'padding:8px 10px;margin:6px 0 10px 0;white-space:pre-wrap;font-family:%s;font-size:9.5pt;'
        'color:#1f2328"><code style="font-family:%s;font-size:9.5pt">' % (mono, mono))
    sub(r"<code>",
        '<code style="font-family:%s;font-size:9.5pt;background:#f0f1f3;padding:1px 4px;'
        'border-radius:3px;color:#1f2328">' % mono)
    sub(r"<p>", '<p style="margin:0 0 8px 0">')
    sub(r"<(ul|ol)>", '<\\g<1> style="margin:0 0 8px 0;padding-left:22px">')
    for name, color, tint in [("note", "#0969da", "#eef5fd"), ("tip", "#1a7f37", "#eef8f0"),
                              ("important", "#8250df", "#f4effc"), ("warning", "#9a6700", "#fdf6e6"),
                              ("caution", "#cf222e", "#fdeff0")]:
        sub(r'<div class="%s">\s*<div class="title">\s*<p[^>]*>' % name,
            '<div style="margin:0 0 8px 0;padding:6px 10px;border-left:3px solid %s;background:%s">'
            '<div><p style="margin:0 0 4px 0;font-weight:bold;color:%s">' % (color, tint, color))
    sub(r"<blockquote>",
        '<blockquote style="margin:0 0 8px 0;padding-left:10px;border-left:3px solid #c8c8c8;color:#555">')
    sub(r"<h1([^>]*)>", '<h1\\g<1> style="font-size:16pt;margin:10px 0 6px 0">')
    sub(r"<h2([^>]*)>", '<h2\\g<1> style="font-size:14pt;margin:10px 0 6px 0">')
    sub(r"<h([3-6])([^>]*)>", '<h\\g<1>\\g<2> style="font-size:12pt;margin:8px 0 4px 0">')
    return '<div style="font-family:%s;font-size:%s;color:#1f1f1f;line-height:1.35">%s</div>' % (font, size, h)


def html(md: str, target: str, pandoc_bin: str):
    raw = pandoc_html(markdown_for(md, target), False, pandoc_bin)
    return None if raw is None else styled(raw, target)


def code_token(n: int) -> str:
    return "[[CODE%d]]" % n


def code_guard(s: str) -> dict:
    lines = []
    block = None
    fence = ""
    codes = []
    for line in s.split("\n"):
        t = line.strip()
        if block is not None:
            block.append(line)
            if fence and t.startswith(fence) and t.strip(fence[0] + " ") == "":
                codes.append("\n".join(block))
                lines.append(code_token(len(codes)))
                block = None
        elif t.startswith("```") or t.startswith("~~~"):
            fence = t[:len(t) - len(t.lstrip(t[0]))] if t else ""
            block = [line]
        else:
            out = ""
            rest = line
            while True:
                m = _INLINE_CODE.search(rest)
                if not m:
                    break
                out += rest[:m.start()]
                codes.append(rest[m.start():m.end()])
                out += code_token(len(codes))
                rest = rest[m.end():]
            lines.append(out + rest)
    if block is not None:
        lines += block
    return {"text": "\n".join(lines), "codes": codes}


def code_restore(codes: list, s: str) -> dict:
    out = s
    missing = 0
    for i, code in enumerate(codes):
        tok = code_token(i + 1)
        if tok not in out:
            missing += 1
            continue
        out = out.replace("`" + tok + "`", code).replace(tok, code)
    return {"text": out, "missing": missing}


def estimate(s: str) -> int:
    return math.ceil(len(s.encode("utf-8")) / 3.2)


def part_budget(instructions: str, context: int) -> int:
    return max(200, (int(context * 0.9) - estimate(instructions) - 64) // 2)


def parts(s: str, budget: int) -> list:
    if estimate(s) <= budget:
        return [s]
    out = []
    cur = ""
    for para in s.split("\n\n"):
        nxt = para if not cur else cur + "\n\n" + para
        if estimate(nxt) > budget and cur:
            out.append(cur)
            cur = para
        else:
            cur = nxt
    if cur:
        out.append(cur)
    return out


def reflow_instructions(s: str) -> str:
    out = []
    in_fence = False
    for raw in s.split("\n"):
        t = raw.strip()
        if t.startswith("```") or t.startswith("~~~"):
            in_fence = not in_fence
            out.append(raw)
            continue
        starts = (not t or in_fence or t.startswith("- ") or t.startswith("* ")
                  or t.startswith("#") or t.startswith("|") or _NUMBERED.match(t) is not None)
        if not starts and out and out[-1].strip() and not out[-1].strip().startswith("```"):
            out[-1] = out[-1] + " " + t
        else:
            out.append(raw if in_fence else t)
    return "\n".join(out)


def unwrap_fence(s: str) -> str:
    lines = s.split("\n")
    first = next((i for i, l in enumerate(lines) if l.strip()), None)
    if first is None:
        return s
    opening = lines[first].strip().lower()
    if opening not in ("```", "```markdown", "```md", "~~~"):
        return s
    del lines[first]
    fences = [i for i, l in enumerate(lines) if l.strip().startswith("```") or l.strip().startswith("~~~")]
    if len(fences) % 2 == 1:
        last = fences[-1]
        if lines[last].strip() in ("```", "~~~") and all(not l.strip() for l in lines[last + 1:]):
            del lines[last]
    return "\n".join(lines).strip()


def rule_load(path: str) -> dict:
    try:
        with open(path, encoding="utf-8") as f:
            text = f.read()
    except OSError:
        text = ""
    rule = {"path": path, "name": os.path.splitext(os.path.basename(path))[0],
            "output": "plain", "placeholder": "", "flags": [], "instructions": "",
            "warnings": [], "protectCodeSet": None, "chunkSet": None,
            "prompt": "", "then": "", "keepWords": False, "csvTables": False}
    lines = text.split("\n")
    end = None
    if lines and lines[0].strip() == "---":
        end = next((i for i in range(1, len(lines)) if lines[i].strip() == "---"), None)
    if end is not None:
        for raw in lines[1:end]:
            line = re.sub(r"(^|\s)#.*$", "", raw, count=1)
            if ":" not in line:
                continue
            colon = line.index(":")
            key = line[:colon].strip().lower()
            val = line[colon + 1:].strip()
            if len(val) >= 2 and val[0] in "\"'" and val[-1] == val[0]:
                val = val[1:-1]
            if not key or not val:
                continue
            on = val.lower() in ("true", "yes", "1", "on")
            if key == "name":
                rule["name"] = val
            elif key == "output":
                rule["output"] = val.lower()
            elif key == "placeholder":
                rule["placeholder"] = val
            elif key == "greedy":
                if on:
                    rule["flags"].append("--greedy")
            elif key == "guardrails":
                rule["flags"] += ["--guardrails", val]
            elif key in ("use-case", "use_case", "usecase"):
                rule["flags"] += ["--use-case", val]
            elif key == "model":
                rule["flags"] += ["-m", val]
            elif key == "protect-code":
                rule["protectCodeSet"] = on
            elif key == "chunk":
                rule["chunkSet"] = on
            elif key == "prompt":
                rule["prompt"] = val
            elif key == "then":
                rule["then"] = val if val.lower().endswith(".md") else val + ".md"
            elif key == "keep-words":
                rule["keepWords"] = on
            elif key == "csv-tables":
                rule["csvTables"] = on
            else:
                rule["warnings"].append("unknown key \u201c%s\u201d" % key)
        lines = lines[end + 1:]
    rule["instructions"] = "\n".join(lines).strip()
    return rule


def rule_chain(path: str) -> list:
    out = [rule_load(path)]
    directory = os.path.dirname(path)
    while out[-1]["then"] and len(out) < 6:
        nxt = out[-1]["then"]
        if any(os.path.basename(r["path"]) == nxt for r in out):
            out[0]["warnings"].append("then: \u201c%s\u201d loops" % nxt)
            break
        p = directory + "/" + nxt
        if not os.path.exists(p):
            out[0]["warnings"].append("then: no \u201c%s\u201d" % nxt)
            break
        out.append(rule_load(p))
    return out


def rule_instructions(rule: dict, guarded: bool) -> str:
    i = reflow_instructions(rule["instructions"])
    return i + "\n- " + CODE_INSTRUCTION if guarded else i


def rule_wrap(rule: dict, text: str) -> str:
    return text if not rule["prompt"] else rule["prompt"] + "\n\n" + text


def rule_prepare(rule: dict, text: str) -> str:
    return csv_convert(text) if rule["csvTables"] else text


def rule_accept(rule: dict, input_text: str, answer: str) -> dict:
    if not rule["keepWords"]:
        return {"text": answer, "note": None}
    c = word_check(input_text, answer)
    if c["ok"]:
        return {"text": answer, "note": None}
    what = []
    if c["added"]:
        what.append("added \u201c%s\u201d" % " ".join(c["added"][:3]))
    if c["dropped"]:
        what.append("dropped \u201c%s\u201d" % " ".join(c["dropped"][:3]))
    return {"text": input_text,
            "note": "\u201c%s\u201d skipped: it %s" % (rule["name"], ", ".join(what))}


def rule_arguments(rule: dict, guarded: bool) -> list:
    i = rule_instructions(rule, guarded)
    return ["respond", "--stream"] + ([] if not i else ["-i", i]) + rule["flags"]


def rule_preview(rule: dict) -> str:
    args = (["fm", "respond"]
            + ([] if not rule["instructions"] else ["-i", "@rules/" + os.path.basename(rule["path"])])
            + [shq(f) for f in rule["flags"]])
    s = " ".join(args) + " < input"
    if rule["then"]:
        s += "  \u2192 then " + rule["then"]
    return s


def rule_runnable(rule: dict, input_text: str) -> str:
    args = (["fm", "respond"]
            + ([] if not rule["instructions"] else ["-i", shq(reflow_instructions(rule["instructions"]))])
            + [shq(f) for f in rule["flags"]])
    return " ".join(args) + " <<'WS_INPUT'\n" + rule_wrap(rule, rule_prepare(rule, input_text)) + "\nWS_INPUT"


def shq(s: str) -> str:
    return s if _SHQ_OK.match(s) else "'" + s.replace("'", "'\\''") + "'"


def csv_cells(line: str):
    t = line.strip()
    if "," not in t or any(t.startswith(p) for p in ("|", "#", "- ", "* ", "> ")):
        return None
    c = [x.strip() for x in t.split(",")]
    if len(c) < 2:
        return None
    for cell in c:
        if not cell or len(cell) > 40 or len(cell.split()) > 5:
            return None
        if len(cell) > 1 and any(cell.endswith(x) for x in (".", "?", "!")):
            return None
    return c


def _is_rule(line: str) -> bool:
    return _RULE_LINE.match(line.strip()) is not None


def csv_convert(s: str) -> str:
    lines = s.split("\n")
    out = []
    i = 0
    in_fence = False
    while i < len(lines):
        t = lines[i].strip()
        if t.startswith("```") or t.startswith("~~~"):
            in_fence = not in_fence
        head = None if in_fence else csv_cells(lines[i])
        if head is not None:
            j = i + 1
            ruled = j < len(lines) and _is_rule(lines[j])
            if ruled:
                j += 1
            rows = []
            while j < len(lines):
                r = csv_cells(lines[j])
                if r is None or len(r) != len(head):
                    break
                rows.append(r)
                j += 1
            if (ruled and rows) or (not ruled and (len(rows) >= 2 or (len(rows) == 1 and len(head) >= 3))):
                def row(c):
                    return "| " + " | ".join(x.replace("|", "\\|") for x in c) + " |"

                if out and out[-1].strip():
                    out.append("")
                out.append(row(head))
                out.append(row(["---"] * len(head)))
                out += [row(r) for r in rows]
                if j < len(lines) and lines[j].strip():
                    out.append("")
                i = j
                continue
        out.append(lines[i])
        i += 1
    return "\n".join(out)


def word_words(s: str) -> list:
    lines = s.split("\n")
    kept = []
    for i, l in enumerate(lines):
        sep = _SEPARATOR.match(l) is not None
        next_sep = (i + 1 < len(lines) and "|" in lines[i + 1]
                    and _SEPARATOR.match(lines[i + 1]) is not None)
        if (sep and "|" in l) or (next_sep and "|" in l):
            continue
        kept.append(re.sub(r"^\s*\d+[.)]\s", "", l))
    words = []
    cur = ""
    for ch in "\n".join(kept).lower():
        if ch.isalnum():
            cur += ch
        elif cur:
            words.append(cur)
            cur = ""
    if cur:
        words.append(cur)
    return words


def word_check(input_text: str, output: str) -> dict:
    count = {}
    for w in word_words(input_text):
        count[w] = count.get(w, 0) + 1
    added = []
    for w in word_words(output):
        n = count.get(w, 0)
        if n > 0:
            count[w] = n - 1
        elif w not in FILLER:
            added.append(w)
    dropped = []
    for w in word_words(input_text):
        n = count.get(w, 0)
        if n > 0 and w not in FILLER:
            count[w] = n - 1
            dropped.append(w)
    return {"ok": not added and not dropped, "added": added, "dropped": dropped}
