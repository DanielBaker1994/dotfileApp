# Logs

Every log kitchen-sink and its helpers write lives in **one** directory with
**date-based rolling**:

```
logs/
  2026_10_14/
    confluence-curl.log     every Confluence HTTP request (chmod 600 — real token)
    confluence-debug.log    Confluence debug/full trace
    daemon.log              the app's own log (wsLog: startup, window/focus, errors)
    jira-curl.log           every Jira HTTP request (chmod 600 — real token)
    jira-debug.log          Jira full debug trace
    jira-poll.log           Jira poll summary (stages, pages, ETA, errors)
    jira-poll-stdout.log    launchd stdout for the poller
    jira-poll-stderr.log    launchd stderr for the poller
    launch.log              app launch trace (only with WS_DEBUG=1)
    notifications-poll.log  unread-count poll events
    settings-hub.log        settings hub TUI log
  2026_10_15/
    ...
```

## How it works

- `pylib/log_hub.py` owns the layout: `log_path("jira-debug")` →
  `logs/YYYY_MM_DD/jira-debug.log` (the directory is created on first write).
- The Rust app writes its own log (`daemon.log`) through `app/logging.rs`
  (`ws_log`), the restored Swift `wsLog`: every line goes to stderr (so
  `log stream --predicate 'process == "kitchen-sink"'` still shows it) **and**
  the file.
- The directory is `<repo>/logs` in a checkout and `~/.config/kitchen-sink/logs`
  in an installed app. `WS_LOG_DIR` overrides it (tests, the MCP sandbox).
- Each debug log gets a one-line header naming its absolute path when it opens,
  so a `tail`ed line always tells you which file you are looking at.
- `./ws build` echoes the log directory path and deletes the old
  `~/.cache` / `/tmp` log files.

## Reading them

```bash
bin/ws-logs                 # the inventory: size + mtime + how to read each
bin/ws-logs --logs-only     # (kept for the health check — same list)
tail -f logs/$(date +%Y_%m_%d)/jira-debug.log
```

`./ws doctor` shows the same list in its `== logs ==` section.

## Security

`jira-curl.log` and `confluence-curl.log` hold the **real** auth token (that is
what makes each line re-runnable) and are written `chmod 600`. Never share or
commit them.
