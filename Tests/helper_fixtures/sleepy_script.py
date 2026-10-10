#!/usr/bin/env python3
"""script.run fixture: sleeps, then answers (the concurrency tests)."""
import json
import sys
import time

time.sleep(float(sys.argv[1]) if len(sys.argv) > 1 else 1.0)
json.dump({"slept": True}, sys.stdout)
sys.stdout.write("\n")
