"""ws-settings: every shortcut and setting of the kitchen-sink setup
in one place (PRD-settings-hub.md)."""
import sys

sys.dont_write_bytecode = True   # a __pycache__ in the app bundle breaks its signature
if sys.version_info < (3, 11):
    sys.exit("ws-settings needs python 3.11 or newer (tomllib); "
             "set WS_PYTHON or install python3 with Homebrew")
