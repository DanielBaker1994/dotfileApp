#!/usr/bin/env python3
"""Webex REST source for the Hyper+S unread counts (notify_poll.py).

  webex_api.py --login          one-time OAuth sign-in (Webex Integration)
  webex_api.py --poll [--print] one poll → JSON (unread spaces, @mentions)
  webex_api.py --status         token state, no request

Auth = a Webex *Integration* (developer.webex.com → My Webex Apps), redirect
URI http://127.0.0.1:PORT/callback. Client id/secret + tokens live in
~/.config/notifications/webex.json (0600, `$NOTIFY_WEBEX_JSON` overrides);
never in commands.toml. The access token is refreshed with the refresh token
(~90 days, renewed on use), so one sign-in lasts as long as the app polls.

Unread = a space whose lastActivity is newer than MY membership's
lastSeenDate. @mentions = messages in those spaces with mentionedPeople=me,
newer than lastSeenDate (or than `max_age_hours` when Webex gives no read
status). stdlib only.
"""
from __future__ import annotations

import base64
import datetime as dt
import getpass
import http.server
import json
import os
import secrets
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request

API = "https://webexapis.com/v1"
SCOPES = "spark:rooms_read spark:messages_read spark:memberships_read spark:people_read"
DEFAULT_PORT = 8765
MAX_RETRY_WAIT = 30          # seconds; a longer Retry-After fails this poll
LOGIN_HINT = "sign in: python3 ~/.config/kitchen-sink/notify/webex_api.py --login"

# swapped by tests
SLEEP = time.sleep
URLOPEN = urllib.request.urlopen
NOW = lambda: dt.datetime.now(dt.timezone.utc)  # noqa: E731


def creds_path() -> str:
    return os.environ.get("NOTIFY_WEBEX_JSON") or os.path.expanduser(
        "~/.config/notifications/webex.json")


def load_creds() -> dict:
    try:
        with open(creds_path(), encoding="utf-8") as fh:
            return json.load(fh)
    except (OSError, ValueError):
        return {}


def save_creds(c: dict) -> None:
    path = creds_path()
    os.makedirs(os.path.dirname(path), exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=os.path.dirname(path), prefix=".webex-")
    with os.fdopen(fd, "w", encoding="utf-8") as fh:
        json.dump(c, fh, indent=2)
    os.chmod(tmp, 0o600)
    os.replace(tmp, path)


def parse_time(s: str | None):
    if not s:
        return None
    try:
        return dt.datetime.fromisoformat(s.replace("Z", "+00:00"))
    except ValueError:
        return None


class AuthError(Exception):
    pass


class ApiError(Exception):
    pass


class Client:
    def __init__(self, creds: dict | None = None):
        self.creds = creds if creds is not None else load_creds()
        self.requests = 0

    # -- tokens --------------------------------------------------------------
    def _token_request(self, form: dict) -> None:
        body = urllib.parse.urlencode({
            "client_id": self.creds.get("client_id", ""),
            "client_secret": self.creds.get("client_secret", ""),
            **form,
        }).encode()
        req = urllib.request.Request(API + "/access_token", data=body, method="POST",
                                     headers={"Content-Type": "application/x-www-form-urlencoded",
                                              "Accept": "application/json"})
        try:
            with URLOPEN(req, timeout=20) as r:
                got = json.load(r)
        except urllib.error.HTTPError as e:
            raise AuthError(f"token request failed ({e.code}): {LOGIN_HINT}") from e
        now = time.time()
        self.creds["access_token"] = got["access_token"]
        self.creds["expires_at"] = now + int(got.get("expires_in", 0)) - 60
        if got.get("refresh_token"):
            self.creds["refresh_token"] = got["refresh_token"]
            self.creds["refresh_expires_at"] = now + int(got.get("refresh_token_expires_in", 0))
        save_creds(self.creds)

    def refresh(self) -> None:
        if not self.creds.get("refresh_token"):
            raise AuthError(f"not signed in: {LOGIN_HINT}")
        self._token_request({"grant_type": "refresh_token",
                             "refresh_token": self.creds["refresh_token"]})

    def token(self) -> str:
        if not self.creds.get("access_token") or time.time() >= self.creds.get("expires_at", 0):
            self.refresh()
        return self.creds["access_token"]

    # -- requests ------------------------------------------------------------
    def get(self, path: str, params: dict | None = None) -> dict:
        url = API + path + ("?" + urllib.parse.urlencode(params) if params else "")
        refreshed = retried = False
        while True:
            req = urllib.request.Request(url, headers={
                "Authorization": "Bearer " + self.token(), "Accept": "application/json"})
            self.requests += 1
            try:
                with URLOPEN(req, timeout=20) as r:
                    return json.load(r)
            except urllib.error.HTTPError as e:
                if e.code == 401 and not refreshed:
                    refreshed = True
                    self.refresh()
                    continue
                if e.code == 401:
                    raise AuthError(f"token rejected: {LOGIN_HINT}") from e
                if e.code in (429, 502, 503, 504) and not retried:
                    wait = int(e.headers.get("Retry-After") or 5) if e.headers else 5
                    if wait <= MAX_RETRY_WAIT:
                        retried = True
                        SLEEP(wait)
                        continue
                raise ApiError(f"GET {path} → {e.code}") from e
            except urllib.error.URLError as e:
                raise ApiError(f"GET {path}: {e.reason}") from e


# -- poll ----------------------------------------------------------------------
def space_link(room_id: str) -> str:
    """API room id (base64 of ciscospark://us/ROOM/UUID) → a Webex app deep link."""
    try:
        raw = base64.urlsafe_b64decode(room_id + "=" * (-len(room_id) % 4)).decode()
        return "webexteams://im?space=" + raw.rsplit("/", 1)[-1]
    except (ValueError, UnicodeDecodeError):
        return ""


def poll(max_rooms: int = 30, max_age_hours: float = 24, client: Client | None = None) -> dict:
    """{ok, unread, mentions, spaces:[{title, link, at, mentions}], items:[{space,
    link, text, at}], requests} or {ok: False, auth: bool, error}. `spaces` =
    unread spaces + spaces with mentions, newest first."""
    c = client or Client()
    try:
        me = c.get("/people/me")["id"]
        since = NOW() - dt.timedelta(hours=max_age_hours)
        rooms = c.get("/rooms", {"sortBy": "lastactivity", "max": max_rooms}).get("items", [])
        unread = mentions = 0
        items, spaces = [], []
        for room in rooms:
            last = parse_time(room.get("lastActivity"))
            if not last or last < since:
                break                      # sorted newest first
            mem = c.get("/memberships", {"roomId": room["id"], "personId": me}).get("items", [])
            seen = parse_time(mem[0].get("lastSeenDate")) if mem else None
            if seen and last <= seen:
                continue
            title, link = room.get("title", ""), space_link(room["id"])
            found = 0
            if room.get("type") == "group":    # mentionedPeople only exists in group spaces
                after = max(seen, since) if seen else since
                msgs = c.get("/messages", {"roomId": room["id"], "mentionedPeople": "me",
                                           "max": 10}).get("items", [])
                for m in msgs:
                    at = parse_time(m.get("created"))
                    if at and at > after:
                        found += 1
                        items.append({"space": title, "link": link,
                                      "text": " ".join((m.get("text") or "").split())[:120],
                                      "at": m.get("created")})
            if seen:
                unread += 1
            mentions += found
            if seen or found:
                spaces.append({"title": title, "link": link, "at": room.get("lastActivity"),
                               "mentions": found})
        return {"ok": True, "unread": unread, "mentions": mentions, "spaces": spaces,
                "items": items, "requests": c.requests}
    except AuthError as e:
        return {"ok": False, "auth": True, "error": str(e)}
    except (ApiError, KeyError, ValueError) as e:
        return {"ok": False, "auth": False, "error": str(e)}


# -- login ---------------------------------------------------------------------
def login() -> int:
    creds = load_creds()
    if not creds.get("client_id"):
        print("Create an Integration at https://developer.webex.com/my-apps")
        port = creds.get("port", DEFAULT_PORT)
        print(f"  redirect URI: http://127.0.0.1:{port}/callback")
        print(f"  scopes: {SCOPES}")
        creds["client_id"] = input("Client ID: ").strip()
        creds["client_secret"] = getpass.getpass("Client Secret: ").strip()
        creds["port"] = port
    port = int(creds.get("port", DEFAULT_PORT))
    redirect = f"http://127.0.0.1:{port}/callback"
    state = secrets.token_urlsafe(16)
    got: dict = {}

    class Handler(http.server.BaseHTTPRequestHandler):
        def do_GET(self):  # noqa: N802
            q = urllib.parse.parse_qs(urllib.parse.urlparse(self.path).query)
            if q.get("state", [""])[0] == state and "code" in q:
                got["code"] = q["code"][0]
                msg = "Webex connected. You can close this tab."
            else:
                got["error"] = q.get("error_description", q.get("error", ["bad callback"]))[0]
                msg = "Sign-in failed: " + got["error"]
            self.send_response(200)
            self.send_header("Content-Type", "text/plain; charset=utf-8")
            self.end_headers()
            self.wfile.write(msg.encode())

        def log_message(self, format, *args):  # noqa: A002 — quiet
            pass

    srv = http.server.HTTPServer(("127.0.0.1", port), Handler)
    srv.timeout = 300
    url = "https://webexapis.com/v1/authorize?" + urllib.parse.urlencode({
        "client_id": creds["client_id"], "response_type": "code",
        "redirect_uri": redirect, "scope": SCOPES, "state": state})
    print("Opening the browser to sign in…")
    subprocess.run(["open", url], check=False)
    srv.handle_request()
    srv.server_close()
    if "code" not in got:
        print("sign-in failed:", got.get("error", "timed out"), file=sys.stderr)
        return 1
    c = Client(creds)
    try:
        c._token_request({"grant_type": "authorization_code", "code": got["code"],
                          "redirect_uri": redirect})
        me = c.get("/people/me")
    except (AuthError, ApiError) as e:
        print(e, file=sys.stderr)
        return 1
    print("Signed in as", me.get("displayName", "?"), "→", creds_path())
    return 0


def status() -> dict:
    c = load_creds()
    now = time.time()
    return {"configured": bool(c.get("client_id")),
            "signedIn": bool(c.get("refresh_token")),
            "refreshExpiresInDays": round((c.get("refresh_expires_at", now) - now) / 86400, 1),
            "path": creds_path()}


def main(argv: list[str]) -> int:
    if "--login" in argv:
        return login()
    if "--status" in argv:
        print(json.dumps(status(), indent=2))
        return 0
    if "--poll" in argv:
        res = poll()
        print(json.dumps(res, indent=2 if "--print" in argv else None))
        return 0 if res["ok"] else 1
    print(__doc__)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
