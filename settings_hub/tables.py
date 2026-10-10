"""The static tables (name maps, symbol maps, layer lists, apply modes),
loaded from data/tables.json — data, not code. Commands.toml overrides
stay where they were: these are only the shipped defaults."""
from __future__ import annotations

import json
import os

_PATH = os.path.join(os.path.dirname(os.path.abspath(__file__)), "data", "tables.json")
with open(_PATH, encoding="utf-8") as _fh:
    T = json.load(_fh)

LAYERS = tuple(T["layers"])
LAYER_TITLE = T["layer_titles"]
LAYER_NAME = T["layer_names"]
REBIND_LAYERS = tuple(T["rebind_layers"])
MODS = tuple(T["mods"]["order"])
MOD_NAME = T["mods"]["names"]
MOD_SYMBOL = T["mods"]["symbols"]
MOD_WORDS = T["mod_words"]
LABEL_MOD = T["label_mods"]
KEY_NAMES = T["key_names"]
ARROWS = T["arrows"]
VIM_SPECIAL = T["vim_special"]
VIM_MODES = T["vim_modes"]
AERO_KEY = T["aero_key"]
HERDR_KEY = T["herdr_key"]
AERO_MOD = T["aero_mod"]
COLOR_KEYS = set(T["color_keys"])
NOT_SETTINGS = set(T["not_settings"])
APPLY = T["apply"]
APP_LAUNCH_ONLY = set(T["app_launch_only"])
