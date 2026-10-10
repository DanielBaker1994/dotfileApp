#!/usr/bin/env python3
"""script.run fixture: echoes stdin + argv as one JSON line."""
import json
import sys

json.dump({"echo": sys.stdin.read(), "argv": sys.argv[1:]}, sys.stdout)
sys.stdout.write("\n")
