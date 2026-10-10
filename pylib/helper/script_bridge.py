from __future__ import annotations

import os
import subprocess
import sys

from . import HelperError

# The app's script bridge has always run its asset scripts with the brew
# paths ahead of PATH (pandoc/weasyprint/curl resolution). Keep that exact
# contract now that the runs happen as children of the helper.
DEFAULT_PATH = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin"


def run(params: dict) -> dict:
    folder = params.get("folder") or ""
    script = params.get("script") or ""
    if not isinstance(script, str) or not script:
        raise HelperError("script.run: script is required")
    args = params.get("args") or []
    if not isinstance(args, list):
        raise HelperError("script.run: args must be a list")
    args = [a if isinstance(a, str) else str(a) for a in args]
    stdin = params.get("stdin")
    stdin = "" if stdin is None else (stdin if isinstance(stdin, str) else str(stdin))
    timeout = params.get("timeout")
    timeout = float(timeout) if timeout else None

    path = script if os.path.isabs(script) else os.path.join(folder, script)
    if not os.path.isfile(path):
        raise HelperError("script.run: no such script: %s" % path)

    env = dict(os.environ)
    env["PATH"] = DEFAULT_PATH + ":" + (env.get("PATH") or "")
    # A __pycache__ inside the signed bundle breaks the signature.
    env["PYTHONDONTWRITEBYTECODE"] = "1"

    try:
        proc = subprocess.run(
            [sys.executable, "-B", path] + args,
            input=stdin, capture_output=True, encoding="utf-8", errors="replace",
            cwd=folder or None, env=env, timeout=timeout)
    except subprocess.TimeoutExpired:
        raise HelperError("script.run: timed out: %s" % script)
    except OSError as e:
        raise HelperError("script.run: cannot run %s: %s" % (script, e))
    return {"code": proc.returncode, "stdout": proc.stdout, "stderr": proc.stderr}
