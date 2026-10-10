"""Jira file locations, from pylib/paths.json - the ONE list the poller and
the app share (the app asks the helper; there is no Swift copy).
Stdlib only, 3.9-compatible.

Env overrides (tests, throwaway installs): JIRA_CONFIG_JSON, JIRA_TEAM_JSON,
JIRA_CONFIG_FILE, JIRA_CACHE_DIR."""
import json
import os

_HERE = os.path.dirname(os.path.abspath(__file__))
with open(os.path.join(_HERE, "paths.json"), encoding="utf-8") as _fh:
    _P = json.load(_fh)


def _path(env: str, key: str) -> str:
    return os.environ.get(env) or os.path.expanduser(_P[key])


CONFIG_JSON = _path("JIRA_CONFIG_JSON", "configJson")
TEAM_JSON = _path("JIRA_TEAM_JSON", "teamJson")
LEGACY_CONFIG = _path("JIRA_CONFIG_FILE", "legacyConfig")
CACHE_DIR = _path("JIRA_CACHE_DIR", "cacheDir")
OUT_DIR_DEFAULT = os.path.expanduser(_P["outDir"])

CACHE = _P["cache"]       # file names inside CACHE_DIR
TABS = _P["tabs"]         # tab file names inside outDir
SIDE_DIRS = _P["sideDirs"]  # folders next to outDir (never tabs)


def cache_file(name: str) -> str:
    """CACHE_DIR/<the paths.json cache entry `name`>."""
    return os.path.join(CACHE_DIR, CACHE[name])
