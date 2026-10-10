"""confluence_config.py - Confluence settings + the CQL a search runs.

config.json (chmod 600; its own site + token - Confluence is NOT the Jira
site): $CONFLUENCE_CONFIG_JSON, else commands.toml `[confluence] config`,
else ~/.config/confluence/config.json.

  site       full base URL incl. the context path: https://x.atlassian.net/wiki
             (added for *.atlassian.net when missing) or https://conf.corp/confluence
  auth       bearer (Server/DC PAT) | basic (Cloud email + API token); unset =
             basic iff an email is set
  email, token
  spaces     [{key, name}] - what the Space picker offers, typed by the
             user (validated with GET /rest/api/space/KEY; spaces are never
             listed). Empty = the whole site. No space picked = the whole
             site too; the scope is never injected as a default.
  favorites  [{id, title, space, spaceName, type, url, added, missing?}],
             newest first - bookmarks (enough to draw the list offline), not
             a content cache
  search     {limit, types}

Nothing else is written to disk: no result or page cache.
"""
from __future__ import annotations

import json
import os
import re
import sys
import urllib.parse
from typing import Any

CONF_DIR = os.path.dirname(os.path.abspath(__file__))
WS_ROOT = os.path.dirname(CONF_DIR)
sys.path.insert(0, os.path.join(WS_ROOT, "jira"))
import jira_config  # type: ignore  # noqa: E402  (write_json_600, ConfigError)
sys.path.insert(0, os.path.join(WS_ROOT, "pylib"))
import config_text  # type: ignore  # noqa: E402  (the commands.toml codec)

HOME = os.path.expanduser("~")
DEFAULT_CONFIG = os.path.join(HOME, ".config/confluence/config.json")
CACHE_DIR = os.environ.get("CONFLUENCE_CACHE_DIR") or os.path.join(HOME, ".cache/confluence")
COMMANDS_CONF = os.environ.get("WS_COMMANDS_CONF") or os.path.join(WS_ROOT, "commands.toml")

# ---------------------------------------------------------------- defaults
# the static tables (content types, search modes, the modified windows and
# the config defaults) live in confluence/defaults.json and load here;
# behaviour stays code.
_DEFAULTS_FILE = os.path.join(CONF_DIR, "defaults.json")
with open(_DEFAULTS_FILE, encoding="utf-8") as _fh:
    _D = json.load(_fh)
TYPES = tuple(_D["types"])
MODES = tuple(_D["modes"])
MODIFIED = _D["modified"]
DEFAULTS = _D["defaults"]

ConfigError = jira_config.ConfigError


def section_value(section: str, key: str, path: str = COMMANDS_CONF) -> str | None:
    """`key = value` inside `[section]` of commands.toml (None = absent)."""
    try:
        with open(path, encoding="utf-8") as fh:
            lines = fh.read().splitlines()
    except OSError:
        return None
    cur = None
    for ln in lines:
        s = ln.strip()
        if s.startswith("[") and s.endswith("]"):
            cur = s[1:-1].strip()
        elif cur == section:
            e = jira_config.config_entry(s)
            if e and e[0] == key:
                return e[1]
    return None


def set_section_value(section: str, key: str, value: str | None, path: str = COMMANDS_CONF) -> None:
    """Set (None = remove) `key = value` in `[section]`, keeping every other
    byte (pylib/config_text.config_setting = the app's configSetting); a
    symlinked commands.toml is written through, never replaced."""
    real = os.path.realpath(path)
    with open(real, encoding="utf-8") as fh:
        lines = fh.read().split("\n")
    new = config_text.config_setting(lines, section, [(key, value)])
    if new == lines:
        return
    tmp = real + ".tmp"
    with open(tmp, "w", encoding="utf-8") as fh:
        fh.write("\n".join(new))
    os.chmod(tmp, os.stat(real).st_mode & 0o7777)
    os.replace(tmp, real)


def config_path() -> str:
    env = os.environ.get("CONFLUENCE_CONFIG_JSON")
    if env:
        return env
    v = section_value("confluence", "config")
    return os.path.expanduser(v) if v else DEFAULT_CONFIG


def norm_site(site: str) -> str:
    s = (site or "").strip().rstrip("/")
    if s and "://" not in s:
        s = "https://" + s
    u = urllib.parse.urlsplit(s)
    if (u.hostname or "").endswith(".atlassian.net") and not u.path.startswith("/wiki"):
        s += "/wiki"
    return s


class Config:
    def __init__(self, data: dict, path: str):
        self.data = data
        self.path = path

    def __getitem__(self, k) -> Any:
        v = self.data.get(k)
        return DEFAULTS.get(k) if v is None else v

    def get(self, k, default=None):
        v = self[k]
        return default if v is None else v

    @property
    def site(self) -> str:
        return norm_site(self["site"])

    @property
    def auth(self) -> str:
        a = (self["auth"] or "").lower()
        return a if a in ("bearer", "basic") else ("basic" if self["email"] else "bearer")

    def scope(self) -> list:
        return [s["key"] for s in self["spaces"] if isinstance(s, dict) and s.get("key")]

    def problems(self) -> list:
        out = []
        if not self.site:
            out.append("no Confluence site set")
        elif not re.match(r"^https?://[^/\s]+", self.site):
            out.append(f"site is not a URL: {self.site}")
        if not self["token"]:
            out.append("no API token set")
        if self.auth == "basic" and not self["email"]:
            out.append("basic auth needs the account email")
        return out

    def save(self) -> None:
        os.makedirs(os.path.dirname(self.path), exist_ok=True)
        jira_config.write_json_600(self.path, self.data)


def load(path: str | None = None) -> Config:
    p = path or config_path()
    data = {}
    if os.path.exists(p):
        try:
            with open(p, encoding="utf-8") as fh:
                data = json.load(fh) or {}
        except (OSError, ValueError) as err:
            raise ConfigError(f"cannot read {p}: {err}")
        if not isinstance(data, dict):
            raise ConfigError(f"{p} is not a JSON object")
    # env overrides fill blanks only (like JIRA_TOKEN)
    for k, env in (("site", "CONFLUENCE_SITE"), ("email", "CONFLUENCE_EMAIL"), ("token", "CONFLUENCE_TOKEN")):
        if not data.get(k) and os.environ.get(env):
            data[k] = os.environ[env]
    return Config(data, p)


# ------------------------------------------------------------------- query

WORD_JUNK = re.compile(r'[+!(){}\[\]^~?:\\/&|<>=,;]')


def parse_query(q: str) -> list:
    """The search box -> [(text, is_phrase)]: "quoted parts" are phrases,
    everything else single words. Lucene operators are dropped from words;
    a word may END in * (prefix) but not start with one."""
    out = []
    for m in re.finditer(r'"([^"]*)"|(\S+)', q or ""):
        if m.group(1) is not None:
            words = [w for w in (WORD_JUNK.sub(" ", m.group(1)).replace("*", " ")).split() if w]
            if len(words) > 1:
                out.append((" ".join(words), True))
            elif words:
                out.append((words[0], False))
            continue
        w = WORD_JUNK.sub("", m.group(2).replace('"', "")).lstrip("*-")
        w = re.sub(r"\*+$", "*", w)
        if "*" in w[:-1]:
            w = w.replace("*", "")
        if w and w != "*" and w.upper() not in ("AND", "OR", "NOT"):
            out.append((w, False))
    return out


def cql_str(s: str) -> str:
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"') + '"'


def text_clauses(q: str, mode: str, field: str) -> str:
    parts = parse_query(q)
    if not parts:
        return ""
    if mode == "phrase":
        words = " ".join(t.rstrip("*") for t, _ in parts).split()
        if len(words) > 1:
            return f'{field} ~ {cql_str(chr(34) + " ".join(words) + chr(34))}'
        parts = [(words[0], False)] if words else []
    clauses = [f"{field} ~ {cql_str(chr(34) + t + chr(34) if ph else t)}" for t, ph in parts]
    if len(clauses) == 1:
        return clauses[0]
    return "(" + (" OR " if mode == "any" else " AND ").join(clauses) + ")"


def criteria_cql(crit: dict, cfg: Config) -> str:
    """criteria (the search strip) -> CQL. Lists are ORed with `in`, criteria
    ANDed; no spaces picked = the whole site, picked spaces are clamped to the
    configured scope."""
    mode = crit.get("mode") or "all"
    if mode not in MODES:
        raise ConfigError(f"unknown match mode {mode!r} (all, phrase, any)")
    field = "title" if crit.get("titleOnly") else "text"
    ands = []
    text = text_clauses(crit.get("query") or "", mode, field)
    if text:
        ands.append(text)
    scope = cfg.scope()
    want = [s for s in (crit.get("spaces") or []) if s]
    if want and scope:
        want = [s for s in want if s in scope] or scope
    if want:
        ands.append(f"space in ({', '.join(cql_str(s) for s in want)})")
    types = [t for t in (crit.get("types") or (cfg["search"] or {}).get("types") or []) if t in TYPES]
    if types:
        ands.append(f"type in ({', '.join(types)})")
    mod = MODIFIED.get(crit.get("modified") or "", None)
    if crit.get("modified") and crit["modified"] not in MODIFIED:
        raise ConfigError(f"unknown modified window {crit['modified']!r}")
    if mod:
        ands.append(f'lastmodified >= now("{mod}")')
    # contributor = created or edited; "me" = currentUser(); old "mine" = me
    people = [str(u) for u in (crit.get("contributors") or []) if str(u).strip()]
    if crit.get("mine") and "me" not in people:
        people.append("me")
    if people:
        parts = (["contributor = currentUser()"] if "me" in people else [])
        others = [u for u in people if u != "me"]
        if others:
            parts.append(f"contributor in ({', '.join(cql_str(u) for u in others)})")
        ands.append(parts[0] if len(parts) == 1 else "(" + " OR ".join(parts) + ")")
    ids = [str(i) for i in (crit.get("ids") or []) if str(i).isdigit()]
    if crit.get("ids") is not None:
        if not ids:
            raise ConfigError("no favorites yet - star a result (☆ or Cmd+D) to add one")
        ands.append(f"id in ({', '.join(ids)})")
    if crit.get("saved"):
        ands.append("favourite = currentUser()")
    if not text and not (crit.get("modified") or people or crit.get("ids") or crit.get("saved")
                         or crit.get("spaces")):
        raise ConfigError("type something to search for")
    cql = " AND ".join(ands)
    if crit.get("sort") == "recent" or not text:
        cql += " ORDER BY lastmodified DESC"
    return cql


def terms(crit: dict) -> list:
    """What the preview highlights: [{text, phrase, prefix}] (case-insensitive)."""
    parts = parse_query(crit.get("query") or "")
    if (crit.get("mode") or "all") == "phrase":
        words = " ".join(t.rstrip("*") for t, _ in parts).split()
        parts = [(" ".join(words), len(words) > 1)] if words else []
    return [{"text": t.rstrip("*"), "phrase": ph, "prefix": t.endswith("*")} for t, ph in parts]
