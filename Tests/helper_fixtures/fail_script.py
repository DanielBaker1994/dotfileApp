#!/usr/bin/env python3
"""script.run fixture: exits 3 with a message on stderr."""
import sys

sys.stderr.write("boom: fixture failure\n")
sys.exit(3)
