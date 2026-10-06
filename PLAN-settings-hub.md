# PLAN: `ws-settings` implementation

Companion to `PRD-settings-hub.md` (the spec). This file holds the decisions
and the build order. Written 2026-10-03 from a planning pass that checked
every claim against the real files and CLIs (AeroSpace 0.21.3-Beta, herdr
0.9.1, Ghostty 1.3.1, python 3.14 at /opt/homebrew/bin/python3; /usr/bin/python3
is the CLT 3.9 without `tomllib`).

## Owner decisions (2026-10-03)

| # | Question | Decision |
|---|---|---|
| 1 | Home of the shared python codec | NEW top-level `pylib/config_text.py` (jira / confluence / notify import it; `jira_config` re-exports `config_entry` / `config_line`) |
| 2 | Favorites file | `~/.config/kitchen-sink/settings-hub.json`, gitignored |
| 3 | Setting a commented-out key | un-comment it IN PLACE (keeps it beside its doc) |
| 4 | `[theme]` / launch-only `[app]` changes | add a `kitchen-sink restart` verb |
| 5 | Confluence's hard-coded shortcuts alert | switch to the shared `[shortcuts]` card |
| 6 | Dead `config = "~/.config/confluence/fake.json"` at the end of `[ai]` | delete it |
| 7 | herdr default bindings | NOT shown (only what the config sets) |
| 8 | vim reader | headless nvim (`nvim_get_keymap`, cached by mtime), static parse as fallback |
| 9 | Ghostty `global:` keybinds | included as a global clash layer |
| 10 | Value outside a doc-comment's allowed list | warning, `--force` to write; Swift-side validation stays a hard refusal |

## Corrections to the PRD (apply when editing it)

1. herdr path = `~/.config/herdr/config.toml` (what herdr reads; a link into dotfiles).
2. Mod toggles = Alt+1..4, not Ctrl+1..4 (Ctrl+3 is ESC, Ctrl+2 NUL in a terminal).
3. Ghostty eats Cmd+C/X/A/Z: the launcher remaps them with `--keybind=super+c=csi:99;9u` etc.
   Cmd / Hyper chords are typed when rebinding, never captured.
4. Launch via `ws-settings open` (single instance: focus the window if listed),
   `--quit-after-last-window-closed=true --confirm-close-surface=false`; the float
   rule goes BEFORE the Ghostty → workspace 1 rule.
5. Apply: `[theme]` + launch-only `[app]` keys need a restart; `[confluence]` /
   `[ai]` / `[setup]` apply on open; `jira.enabled` goes through
   `kitchen-sink jira-poll on|off`; herdr reloads with `herdr server reload-config`
   after `herdr config check`; aerospace `reload-config --dry-run --no-gui` first.
6. Validation = a `config-check` CLI verb wrapping Swift `configValueProblem`
   (section rules too), plus `config-schema` JSON for offline hints.
7. AeroSpace Hyper actions come from the `[shortcuts]` `all: Hyper+…` rows (join by chord), no python map.
8. `doctor` flags keys that are undocumented AND not in the schema (no full key list exists).
9. aerospace path = what AeroSpace reads (`~/.aerospace.toml` or `~/.config/aerospace/aerospace.toml`), realpath for writes.
10. §9.6 live test snapshots + restores commands.toml.
11. Stale docs: AGENT_CONTEXT /paths "Cmd+R reveal" (code: Cmd+R rename, Cmd+Shift+R reveal); `[shortcuts] "ai: Esc"` "never closes".

## Layout

```
bin/ws-settings              bash launcher: python ≥ 3.11 ($WS_PYTHON, /opt/homebrew, /usr/local, PATH),
                             PYTHONDONTWRITEBYTECODE=1, exec python -B -m settings_hub
pylib/config_text.py         THE python codec (moved from jira_config.py, 3.9-compatible)
settings_hub/                cli, paths, model, chords, readers/{aerospace,app_shortcuts,herdr,vim,ghostty},
                             settings + docs, schema, writer, undo, apply, conflicts, doctor, export,
                             favorites, search, tui/, data/system_shortcuts.toml
Tests/test_settings_hub.py   + Tests/fixtures/settings_hub/
```

install.conf: `pylib settings_hub` → `RESOURCE_LINK_DIRS`, `ws-settings` → `RESOURCE_BIN`.

## Key rules

- `commands.toml` lines only through `config_text` (`config_entry`, `config_line`,
  `config_line_parts` for indent + trailing comment). `text.split("\n")`, never `splitlines()`.
- Writes: realpath → one-line edit → assert exactly one line differs → (v1.1)
  `config-check --file` → re-read, retry once on sha change → mkstemp in the
  target dir + fsync + chmod + `os.replace` (the link is never replaced).
- Undo: `~/.cache/kitchen-sink/settings-undo.json`, last 20, byte-identical.
- Swift (v1.1): socket `reload` (reply JSON), CLI `reload` / `restart` /
  `config-schema` / `config-check` with explicit `main.swift` cases (an unknown
  verb would start a daemon); `"settings-hub"` in `loadCommands`' skip list.

## Phases

**v1 read**: codec move (+ jira / confluence / notify tests stay green) → paths,
model, chords → readers → settings catalog + doc parser → CLI `keys settings get
export doctor open` → TUI read-only → launcher, `[settings-hub]`, Hyper+/ binding +
float rule, install.conf, .gitignore → fill `[shortcuts]` (paths, confluence,
palette, Cmd+K menu, files extras; fix `ai: Esc`) → delete the dead `[ai] config`
line → tests.

**v1.1 write**: Swift `reload` / `restart` / `config-schema` / `config-check`,
`configSectionRules` + `configApplyModes` tables → schema, writer, undo, apply,
`set` / `undo` → picker editing → conflicts (incl. Ghostty globals) → Confluence
alert → shared card → tests §9.4-9.7.

**v1.2 rebind**: aerospace + herdr rebinding with clash refusal, dry-run +
rollback; picker chord capture (Ctrl/Alt/Shift only).

## Risks

- KR2 (≤300 ms hotkey → picker): a cold `open -na Ghostty` is likely slower — measure first
  (`WS_SETTINGS_T0`); fallback = warm window or run inside herdr.
- Window-title race for the float rule: `open` polls `list-windows` ≤1 s and floats by id.
- `reloadConfig` rebuilds the notes window (vim restarts): send `reload` only when needed.
- Swift `writeConfigText` and symlinks: unverified, check in v1.1.
