# PRD: keyboard parity — pane navigation, focus ring, sidebars, VS Code gaps

Status: phase 1 built (2026-10-06), phase 2 = the owner's picks from §7.4 · Written: 2026-10-06 · Target: kitchen-sink (every shared-window view) + `ws-settings`

> **For the implementing agent.** Read `AGENT_CONTEXT.md` (= `CLAUDE.md`, section
> "Pane navigation") and `rule.md` first. They are binding. §7.1–7.3 describe
> what is built; §7.4 is the open gap list the owner picks from.

---

## 1. Summary

Treat every view of the shared window like a tmux / herdr window made of
panes. **Ctrl+H / J / K / L** moves the keyboard to the pane on that side.
A thin silver ring shows which pane has the keys. The left sidebars can be
driven from the keyboard. A checker (`ws-settings parity`) compares every
view against VS Code's standard keys, so gaps show up on their own instead
of being found one at a time.

## 2. Contacts

| Name | Role | Comment |
|---|---|---|
| Daniel Baker | Owner, only user | Set the key scheme (§3); picks phase 2 from §7.4. |
| Implementing agent | Builds it | Follows `AGENT_CONTEXT.md` + `rule.md`. |

## 3. Background

- The owner lives in herdr / tmux, where Ctrl+H/J/K/L (and prefix+H/J/K)
  move between panes. Inside the app's window only notes had anything like
  it: Ctrl+J/K cycled editor → files drawer → terminal.
- The sidebars (`PopupTabsBar`, vertical) took no keyboard focus at all.
  Ctrl+B B could collapse one, but nothing could get *into* it.
- The keys the owner fixed (not open for debate):
  - **Ctrl+H / J / K / L** = pane navigation.
  - **Ctrl+B L** = previous view (window).
  - **Ctrl+B W** = the view switcher (window search).
  - **Ctrl+B B** = sidebar ⇄ icon rail.
  - **Ctrl+B T** = the terminal panel.
- What they replace: Ctrl+H (backspace) and Ctrl+K (kill line) in text
  fields, Ctrl+L in a shell, notes' Ctrl+J/K cycle, and Compare's Ctrl+L
  "copy to left". The nvim pane keeps its own splits (§7.1).
- Earlier pane maps (one per view) are in the owner's artifacts: the
  overview at https://claude.ai/artifact/A3WAq9pAMcpLC8h3ti4hqh. The pane
  lists in §7.1 follow them, simplified to big areas only.

## 4. Objective

- No more "I noticed a missing key" round trips: the standard keys of a
  pane kind (list, sidebar, editor, preview, diff, terminal) exist
  everywhere that kind of pane exists, or are waived on purpose.
- Every pane is reachable without the mouse, and you can always see
  which one has the keys.

Key results:
- KR1: every shared-window view lists ≥ 2 panes, and Ctrl+H/J/K/L walks them.
- KR2: `ws-settings parity` reports **0 missing** after phase 2. After
  phase 1 it reports 13 (§7.4).
- KR3: no regression in `bin/ui-test-focus.py` (show / hide / focus).

## 5. Market segment

One user, keyboard-first, fluent in tmux / herdr / vim / VS Code.

## 6. Value proposition

- One mental model: the window is a tmux window and its areas are panes.
- VS Code habits work: Cmd+F / G, Cmd+Z, Home / End, Space to page, F2.
- Gaps are listed by a tool, not found by accident.

## 7. Solution

### 7.1 Pane navigation (built)

| View | Panes (ids) |
|---|---|
| Notes | `sidebar`, `editor` (nvim / text / reading view), files drawer (`files-filter`, `files-list`, `files-preview`), `terminal` |
| Files | `files-sidebar`, `files-filter`, `files-list`, `files-preview` |
| Jira list | `sidebar`, `search` (live-search strip), `list` (its filter box drives it), `issue` (Cmd+I panel) |
| Jira detail | `page` (the ticket page) |
| Confluence | `sidebar`, `search`, `results`, `preview` |
| Compare (text) | `sidebar`, `left`, `right`. Start page: `left-path`, `right-path`, `recent-filter`, `recent` |
| Compare (folders) | `sidebar`, `names` (when shown), `left`, `right` |
| AI | `sidebar`, `input`, `answer` |
| Jira Config | `sidebar`, `page` |

Rules:
- **Neighbour.** The nearest pane in that direction that overlaps on the
  other axis. A pane that is only diagonally away doesn't count. Gaps
  within 16 pt are a tie, and the bigger overlap wins.
- **Way back.** Going the opposite way returns where you came from.
- **Edges.** No wrapping. With nothing that way the key is still used, so
  Ctrl+H never becomes a backspace by surprise.
- **nvim.** With a vim split in that direction, vim moves first
  (`wincmd`), vim-tmux-navigator style.
- **Real key.** Ctrl+B then Ctrl+H / J / K / L sends the real key to the
  pane: a shell's Ctrl+L clear, a field's Ctrl+H.
- **Overlays.** In-window confirm cards, sheets and popovers keep their own
  keys (rule.md #6).

### 7.2 Focus ring + sidebars (built)

**Focus ring.**
- A 1 pt hairline in light silver (`[app] pane-focus-color = "8cc8ced8"`,
  `pane-focus-width = 1`) around the pane that has the keys.
- It follows clicks as well as keys.
- It shows only while the window is key and the view has 2+ panes.
- It replaces notes' old 3 pt accent drawer borders.

**Sidebar keys.** Ctrl+H from the content puts the keys in the sidebar.
Clicks keep their old behaviour: a note click still lands in the editor.

| Key | Action |
|---|---|
| ↑ ↓, Ctrl+N / Ctrl+P | move the cursor (silver outline) over the pinned rows + rows |
| Home / End / PgUp / PgDn | jump |
| A letter | the next row starting with it |
| Return | open the row and go back to the content |
| Space | open the row and stay in the sidebar |
| Esc | back to the content |
| Delete | close the tab (notes, compare sessions) |

**Read-only panes.**
- The Files text preview pages with Space / Shift+Space.
- Web pages (Jira ticket, Confluence, AI preview, notes reading view)
  scroll natively once a pane key has focused them.
- The generic list (palette, Jira list) gained Home / End / PgUp / PgDn.

**Compare.**
- Copy across moved to **Opt+→ / Opt+←** (WinMerge's keys). Ctrl+R still
  copies right; Ctrl+Opt+L / R copy one line (text) or move (folders).
- Folder expand / collapse all below is now **Shift+→ / ←**.

### 7.3 Parity checker (built)

- `ws-settings parity [--view V] [--all] [--json]`
  (`settings_hub/parity.py`).
- Data lives in `settings_hub/data/parity.toml`:
  - `[views]`: pane kinds per view
  - `[[expect]]` per kind: concept, VS Code's key, the keys expected here
    (`"A | B"` = either)
  - `[[waive]]`: a gap that is fine, with the reason
- An expectation is met when its keys appear in `[shortcuts]` for the view,
  for `all:`, or for the pane kind itself (`sidebar:` / `preview:` rows
  apply to every view; every Cmd+/ sheet shows them).
- It matches **keys, not meaning**. For example, notes' Cmd+P (export PDF)
  counts as notes having "quick open". Treat a "have" whose label means
  something else as a gap (§7.4 G1).
- Fixed along the way: `[shortcuts]` labels like "Ctrl+B L" now parse as a
  prefix sequence, so the false Ctrl+B clash warnings are gone.
- Not built: a ⚠ "missing" filter in the picker (the TUI lists keys, and
  parity gaps are not keys). The CLI is the interface for now.

### 7.4 Open gaps — owner picks (phase 2)

From `ws-settings parity` on 2026-10-06 (13 missing), plus semantic gaps
the checker can't see:

| # | Gap | Views | VS Code | Proposal |
|---|---|---|---|---|
| G1 | Quick open by name | all | Cmd+P | Cmd+P = a filter-as-you-type picker over the view's own items: files (recursive), jira issues, Confluence favorites, Compare recents, AI rules, notes tabs. Notes' Cmd+P export PDF moves to Cmd+Opt+P. |
| G2 | Every action by name | Confluence, AI | Cmd+Shift+P | Cmd+K = an NSMenu of every action, as Compare already has. |
| G3 | Go to view N | all | Cmd+1…9 | **Ctrl+B 1…9** = view N in header order (tmux's prefix + window number). Cmd+1…n stays Compare's filters. |
| G4 | Sidebar toggle alias | all | Cmd+B | Cmd+B = Cmd+\\ (nothing uses Cmd+B today; nvim gets Ctrl keys, not Cmd). |
| G5 | Find in your text / the answer | AI | Cmd+F, Cmd+G | Turn on the NSTextView find bar (`usesFindBar`) for input and answer, and the web view's find for the previews. |
| G6 | Find in the preview | Files | Cmd+F in a preview | The preview's find bar while the preview pane has focus. Cmd+F elsewhere keeps focusing the filter. |
| G7 | Jump a page / to the ends | Confluence results | PgUp/PgDn/Home/End | Move the selection, not just the scroll. |
| G8 | Shortcuts card | Jira Config | Cmd+K Cmd+S | Cmd+/ card + `"jira-config: …"` rows (Cmd+R refresh, Cmd+S save, Return edit, Delete remove). |

Don't build these without the owner's pick. Each lands with its
`[shortcuts]` row, so the checker turns green on its own.

## 8. Release

- Phase 1 (this change): §7.1–7.3.
- Phase 2: the owner's picks from §7.4, one at a time.

## 9. Definition of done (phase 1)

- `bin/run-tests.sh panes`: geometry on the maps' layouts plus the live
  notes layout (42 checks).
- `bin/run-tests.sh compare`: engine unchanged.
- `bin/run-tests.sh settings`: prefix-sequence parsing, no false clash,
  parity have / missing / waived.
- Live through the daemon (`do:key:SPEC`, `do:pane:*`, state `pane`):
  - Files: sidebar ↔ list ↔ filter ↔ list; sidebar ↓ ↓ Return lands in the list.
  - Notes: editor ↔ sidebar; editor ↓ terminal ↑ editor; the nvim split moves first.
  - Compare: left ↔ right ↔ sessions; Opt+→ / Opt+← copy (temp files, not saved).
  - AI and Jira: walks and returns.
- Owner's eye: the ring reads as a thin silver line on Nightfox, not a
  frame.
