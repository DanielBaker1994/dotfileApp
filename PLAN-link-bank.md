# PLAN — link bank: herdr ↔ kitchen-sink on `prefix+w`

**Status (2026-10-20): steps 1–5 implemented** (see §8 for where and what
differs from this plan); step 6 (context pairing, cross-app "came from") is
still open. Goal: `prefix+w` in each app — herdr inside
Ghostty, and kitchen-sink's Ctrl+B W — shows the *other* app's places as a
collapsed group at the bottom of its own list, so either picker can jump
across. Each app keeps its own picker; they only share a row contract.

## 0. What exists today

| Piece | Where | Note |
|---|---|---|
| herdr `prefix+w` | `~/.dotfiles/herdr/config.toml` `[[keys.command]]` → `~/.dotfiles/sesh/connect-herdr.sh` → `sesh/bin/herdr-pick` | Rust TUI (`sesh/herdr-pick/src`, ~4.7k lines); every command/string/key lives in `sesh/herdr-pick.json`; CLI grammar `list [MODE] \| state \| preview LINE \| label LINE \| kill LINE \| go LINE\|PATH`; already themes from `commands.toml [theme]` |
| herdr data | `herdr workspace list` | JSON: `workspace_id`, `label`, `number`, `focused`, `agent_status`, `tab_count`, `pane_count`, `active_tab_id`. Focus: `herdr workspace focus <id>` |
| kitchen-sink Ctrl+B W | `rust/ws-rs/src/views/switcher.rs` | switcher palette — commands left, windows/AeroSpace workspaces right (`list-workspaces` gather ~1137) |
| kitchen-sink socket | `rust/ws-rs/src/app/socket.rs` (`RAW_VERBS` :43), `do_host_action` (`app/host.rs` :1771) | `state`, `do:open:<view>`, raw verbs with late replies |
| Focus hand-back | `palette_hand_back` (`app/host.rs` :3336), `PALETTE_RETURN` | the palette is non-activating and returns focus to the prior app on popup picks |
| AeroSpace strips | `~/.dotfiles/ghostty-strip/bar`, ks header strip, switcher | already three AeroSpace lists — **do not add a fourth**; AeroSpace stays out of this contract |

## 1. The row contract (`links.v1`)

One JSON object per row; each provider answers *list* and *go*:

```json
{"source":"herdr","kind":"workspace","id":"wB","title":"kitchen-sink",
 "subtitle":"5 tabs · working","status":"working","cwd":"~/.config/kitchen-sink",
 "recent":0,"preview":null}
```

- `source`: `herdr` | `kitchen-sink`
- `kind`: herdr `workspace`; kitchen-sink `view` | `note` | `folder`
- `status`: optional, `working` | `waiting` | `idle` | `unknown` (status hues)
- `recent`: rank, 0 = most recent (provider's own notion)
- `cwd`: optional; used later for context pairing (step 6)
- `preview`: optional short text (note excerpt / folder summary)

List envelope: `{"version":1,"rows":[…]}`; errors `{"error":"…"}`.

## 2. Providers

**kitchen-sink** — new raw socket verb `links` → envelope above (views first
by previous-view order, then open notes, then folder sessions). New
`do:go:<id>` presents the target (`open:<view>`, open note, bind folder).
Cold-start rule: listing never launches the daemon; only `go` may.

**herdr** — `herdr-pick list --json` (wrap `herdr workspace list` into
`links.v1`) and `herdr-pick go-id <workspace_id>`, which:
1. focuses the Ghostty window via AeroSpace (`aerospace list-windows
   --all --app-bundle-id com.mitchellh.ghostty` → `aerospace focus
   --window-id`), then
2. `herdr workspace focus <id>`.

Keeping the Ghostty-reach logic in `herdr-pick` means kitchen-sink just
shells `herdr-pick list --json` / `go-id`. Assumption: one herdr session in
one Ghostty window; if more appear, pick the window whose title/session
matches and document the rule.

## 3. The collapsed group (both pickers)

- **Default:** one header row at the very bottom —
  `▸ kitchen-sink · 9 places` (herdr-pick) / `▸ Terminal spaces · 6` (ks).
- **Expand/collapse:** Return / `l` / → on the header expands; `h` / ←
  collapses; herdr-pick also accepts its existing `^e`. `^k` anywhere jumps
  to the header and expands it. Expanded state persists for the picker's
  lifetime only; every fresh open starts collapsed.
- **Filtering:** a query auto-expands the group to just its matching rows,
  ranked below local matches, still under the header. Clearing the query
  re-collapses.
- **Navigation:** ↓ past the last local row lands on the header; wraparound
  skips the group (↓ from the bottom goes to the top of the local list);
  digit jumps 1–9 count local rows only.
- **Order inside:** provider `recent` first; herdr rows with
  `working`/`waiting` agents next.
- **Header states:**
  | State | Header |
  |---|---|
  | loading | `kitchen-sink · …` — fetched in the background, never blocks the list |
  | offline | `kitchen-sink · not running` (dim; Return launches) |
  | error/timeout | `· unavailable` (dim; reason to the log only) |
- **Timeout:** provider list call ≤ 300 ms budget; cache the last good
  result and show it stale-while-revalidate (herdr-pick already does this
  for previews).

## 4. Per-picker specifics

**herdr-pick** (Ghostty → kitchen-sink)
- New row type `group_header` + `link` rows in `model.rs`; strings, keys,
  icons and the provider argv (`nc -U $TMPDIR/ws-notes.sock` ← `links`) in
  `herdr-pick.json`, per its no-strings-in-code rule.
- Preview pane: `preview` field, else title + path.
- Return → `do:go:<id>` over the socket, close the popup; kitchen-sink
  activates and AeroSpace jumps to its workspace.

**kitchen-sink switcher** (kitchen-sink → Ghostty)
- Group in the right pane under the AeroSpace windows; rows show label, tab
  count, agent status dot (theme status tokens, never system colors).
- Background gather like the header workspace strip; never on the main
  thread, never blocking the panel's first draw.
- Return on a herdr row: **do not** run `palette_hand_back` to the prior
  app and **do not** `activate_app`; run `herdr-pick go-id` (which focuses
  Ghostty). Add a `pick_keeps_app`-style exemption for link rows.
- `state` JSON gains `viewSwitcher.linkGroup` (`expanded`, `status`,
  `rows`) so the UI can be verified over the socket (`ui-check` skill).

## 5. Build order (each step ships on its own)

1. `links.v1` contract (this doc §1) + fixtures for both sides.
2. kitchen-sink: `links` raw verb + `do:go:<id>` + unit tests.
3. herdr-pick: `list --json` + `go-id` (Ghostty focus + herdr focus).
4. herdr-pick: collapsed group (§3, §4). → **usable herdr → ks jump here.**
5. kitchen-sink switcher: collapsed group + focus hand-off exemption.
6. Later: context pairing by `cwd` (herdr row `^n` → that project's notes/
   folder in ks; ks "terminal here" → focus/create the matching herdr
   workspace) and a cross-app "came from" record so prefix+l / Ctrl+B L
   can return across the jump.

## 6. Verification

- Unit: contract (de)serialisation both sides; group model (collapse,
  auto-expand on filter, wraparound skip, digit jumps local-only).
- Socket: `printf 'links\n' | nc -U -w3 "$KS_SOCKET"` against the headless
  sandbox (`bin/ks-mcp-sandbox.sh`); `state` shows `viewSwitcher.linkGroup`.
- Real input (per the testing rules): open each picker with the real chord,
  expand the group, filter, jump both directions; screenshot each state
  incl. offline (stop the daemon / herdr server).
- Focus: after a ks → herdr jump, Ghostty is frontmost and kitchen-sink is
  not active; after herdr → ks, the shared window is key.

## 7. Risks

- Focus regressions in the non-activating palette (the Hyper+S rule in
  AGENTS.md) — highest risk; step 5 needs the real-input check, not just
  `do:` hooks.
- Multiple Ghostty windows / herdr sessions → wrong window focused.
- Latency creep in either picker if a provider call blocks.
- Repo split: herdr-pick lives in `~/.dotfiles`, kitchen-sink here — two
  commits, both on `main`.

## 8. As built (2026-10-20)

| Piece | Where |
|---|---|
| Contract + group model (ks) | `rust/ws-rs/src/engines/links.rs` (`LinkRow`, `parse_envelope`, `order_rows`, `LinkGroup`, `entries`, `step`) |
| Fixtures (both sides) | `Tests/fixtures/links/{herdr,kitchen-sink}.json`; copies in `~/.dotfiles/sesh/herdr-pick/tests/fixtures/links/` |
| `links` raw verb, `do:go:<id>` | `SwitcherController::links_json` / `link_go` (`app/host.rs`), `socket::RAW_VERBS` |
| Ctrl+B W group | `views/switcher.rs` (`ViewSwitcherPanel.links`, `LinkPick`, `draw_header` / `draw_link`); host wiring `fetch_switcher_links`, `link_pick`, `push_link_state` |
| Config | `commands.toml [links]` (`enabled`, `label`, `loading`, `offline`, `unavailable`, `herdr-pick`, `timeout-ms`, `go-timeout-ms`, `launch`); `LinksConfig` |
| herdr side | `~/.dotfiles/sesh/herdr-pick/src/links.rs`; `herdr-pick.json` `links`, `keys.group_keymap` / `link_keymap`, `rows.group` / `link`, `texts.group_*` / `link_*` |

Contract details settled while building:

- kitchen-sink ids: `view:<view>` (`notes`, `files`, `jira`, …; the Jira
  sub-views fold into `jira`), `note:<absolute path>` (the open notes tabs,
  selected tab first; `preview` = the first lines), `folder:<compare session
  id>` (open folder-compare sessions). `recent` is the list order.
- An offline provider answers `{"error":"…","offline":true}`; any other
  `{"error"}`, bad JSON, an empty reply (a daemon too old to know `links`)
  or a timeout reads as *unavailable*.
- herdr status → contract: `working` → working, `blocked` → waiting,
  `done` / `idle` → idle, else unknown. herdr rows carry `cwd` (the active
  pane's) and rank by the picker's MRU.
- Ghostty window rule: one window is assumed; with several, `go-id` raises
  the first whose title contains a `links.ghostty_title_match` entry
  (empty by default), else the first AeroSpace lists.

Differences from the plan:

- **Ctrl+B W is the single-list `ViewSwitcherPanel`** (Files / Notes / Jira /
  Confluence), not the two-pane Hyper+S palette (that is where "AeroSpace
  windows on the right" lives). The group sits at the bottom of that list,
  and the AeroSpace list stays untouched, as §0 asks.
- `h` / `l` act only while the cursor is on the group (header, or a row for
  `h` / ←); everywhere else they type into the search field. `^e` / `^m` on
  herdr-pick's header toggle the group; elsewhere they keep expanding tabs.
- herdr-pick has no digit jumps, so §3's "digits count local rows" applies
  to Ctrl+B W only. herdr-pick's ↑/↓ used to clamp at the ends; with the
  group present they now follow §3 (↓ from the bottom → top, ↑ from the top
  → last local row). Page up/down still clamp.
- herdr-pick shows the group in `spaces` and `all` (`links.modes`), not in
  `agents` / `dirs`. Its last good kitchen-sink rows persist in
  `~/.cache/herdr-pick/ks-links.json` (stale-while-revalidate across opens);
  Ctrl+B W keeps them on the panel for the daemon's lifetime.
- Return on an *unavailable* header asks the provider again; on an
  *offline* one it launches (`[links] launch` = `open -a Ghostty`;
  herdr-pick runs `kitchen_sink.sh window`).

Verified 2026-10-20 with real keys on the live app (System Events): Ctrl+B W
→ collapsed `▸ Terminal spaces · 6`, ↓×4 + Return expands, Return on
`dotfiles` → Ghostty frontmost, herdr on `dotfiles`, kitchen-sink inactive,
switcher closed; typing `dot` auto-expands to the one match, clearing
re-collapses. herdr prefix+w → `▸ kitchen-sink · 6 places`, `^k` expands,
Return on Files → kitchen-sink key on Files. Offline header checked in a pty
against a dead socket.

### Revision (2026-10-20, owner feedback): pinned on top, quick switch

- The group is **pinned on top** of both lists and **the cursor starts on
  its header** (`[links] start-on-group` / herdr-pick `links.start_on_group`,
  default true). So Ctrl+B W, Return → Ghostty on whatever herdr workspace
  it is on (`herdr-pick go-id` with no id); prefix+w, Return → kitchen-sink
  on its last view (`do:go:app`). Measured ~0.3 s Ctrl+B W → Ghostty.
- Return on the header no longer toggles; → / l lists the places, ← / h
  hides them, Ctrl+K jumps to the header opened (herdr-pick: `^e` / `^m` on
  the header still toggle). ↑ / ↓ wrap through the whole list.
- A query puts the cursor on the best local match, else the best group
  match — never on the header.

### Install / availability strategy

The two apps never share code or a build: each repo carries its own small
copy of the contract (`engines/links.rs`, `herdr-pick/src/links.rs`), kept
honest by the same fixtures (`Tests/fixtures/links/`, copied into
herdr-pick's `tests/`) and a `version` field. Either app works alone:

| Situation | kitchen-sink (Ctrl+B W) | herdr-pick (prefix+w) |
|---|---|---|
| Other side not installed (no `herdr-pick` binary / no kitchen-sink socket or `kitchen_sink.sh`) | no group at all | no group at all |
| Installed, not running (herdr server down / daemon down) | `· not running`, Return → `[links] launch` | `· not running`, Return → `ks_launch` |
| Running but too old / slow / bad reply | `· unavailable`, Return → `launch` | `· unavailable`, Return → `ks_launch` |
| `[links] enabled = false` | no group | — |

- Every provider call is off the UI thread with a budget (300 ms list,
  socket timeouts); failures are logged, never shown as errors.
- `go-id`: no AeroSpace or no Ghostty window → `open -b <bundle>`
  (`commands.ghostty_open`); herdr down → it only raises Ghostty.
- The Ctrl+B W group is on from the first frame (cursor on the header);
  the background fetch hides it if `[links]` says off or herdr-pick is gone.
