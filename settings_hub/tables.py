"""The static tables (name maps, symbol maps, layer lists, apply modes),
loaded from data/tables.json — data, not code. Commands.toml overrides
stay where they were: these are only the shipped defaults."""
from __future__ import annotations

from . import paths  # noqa: F401  sys.path bootstrap for jsonmgr
import jsonmgr

T = jsonmgr.load("settings_hub/data/tables")

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
