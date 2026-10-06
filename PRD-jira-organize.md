# PRD — Jira: search, organize, group (boards, labels, group-by)

Status: phases 0–5 built, follow-ups done (2026-10-06) · Written: 2026-10-06 · Target: kitchen-sink Jira window (Swift/AppKit) + `jira/*.py`

> **For the implementing agent.** Read `AGENT_CONTEXT.md` (= `CLAUDE.md`) and
> `rule.md` first; they are binding. Especially the Jira poller rules: SCOPE =
> team.json `project_keys`, never list or query outside it. Grep the symbols
> named here and read about 60 lines around each; don't read the big Swift
> files whole. Visual options + mocks:
> https://claude.ai/artifact/MGBhcRTJACmXBiyXwqbVMb

---

## 1. Summary

The Jira window today is a poll-fed table with tabs per poll job, column
filters, a live search strip, ☆ favorites, favorite releases in the sidebar,
and a ticket page. On the owner's work machine (thousands of labels, dozens
of statuses, hundreds of boards, 5k–20k issues per tab) two things hurt:
long flat lists of values, and no way to organize the data the way Jira
does (by board, by label, grouped by a field). This PRD lists the Jira
features that organize issues, maps what we have, and specs the gaps in
phases, each sized for a corporate site.

## 2. Design rules for corporate scale

1. **Never render an unbounded list.** Any list of values (statuses, labels,
   users, boards, releases) shows the values that matter here first (used in
   the current tab, by count), caps the rest behind "Show all N", and is
   searchable.
2. **Group values by their Jira category.** Statuses by status category
   (To Do / In Progress / Done); labels by frequency; boards by project.
3. **Pins are local.** Jira's own "star" (boards, filters) is not a stable
   public API across Cloud and Server/DC. Pinning lives in config.json like
   `favorites` / `favoriteReleases`.
4. **Local first.** If it can be answered from `~/.cache/jira/jiras.json`
   (label, status, assignee, release), filter locally. Only things the cache
   can't answer (a board's own JQL, watchers, sprints) cost a request, and
   those run as poll jobs, never on a click.
5. **Scope.** Board discovery is per scoped project
   (`/rest/agile/1.0/board?projectKeyOrId=KEY`), never a global listing.

## 3. Jira's organizing features vs. us

| Jira feature | What it is | We have | Gap / plan |
|---|---|---|---|
| Status + status category | Every status belongs to To Do / In Progress / Done | Status column, filter ▾ | Filter picker grouped by category (P1). Ticket page bar ✅ P0 |
| Labels | Free-text tags, unbounded on big sites | Directory labels (capped by `labels_max_issues`), live-search picker, ticket pills | Pinned labels in the sidebar, picker with counts (P2). Pill cap ✅ P0 |
| Boards (Scrum / Kanban) | A saved filter + columns that map statuses + quick filters + swimlanes | `team.json boards` + CLI `--board` only | Discover per project, pin, board view with its columns (P3) |
| Sprints | Active / future / closed per Scrum board | — | Sprint field + "current sprint" view on pinned Scrum boards (P3) |
| Saved filters | Named JQL, favourite filters | Poll jobs with custom JQL | Import favourite filters as tabs, like Confluence `--import-saved` (P4) |
| Group by / swimlanes | Board swimlanes; list view "group by" | — | Group-by in the table: collapsible groups with counts (P5) |
| Epics / parent | Hierarchy | — | `parent` as a group-by field (P5) |
| Components | Per-project buckets | — | Add as field + group-by (P5) |
| Fix versions | Releases | Releases tab, ☆ favorite releases, blacklist | Done |
| Quick personal views | Assigned to me, Reported by me, Watching, Recently viewed | ☆ favorites | Sidebar "MY WORK" from cache; Watching as a JQL job (P4) |
| Comments | Thread per issue | Cached (`fetchComments`), not shown | ✅ P0: ticket page reads them from the cache |
| Dashboards / gadgets | Charts | — | Out of scope |

## 4. Phase 0 — built 2026-10-06

- **Comments show.** They are cache-only by design (`jira_config.publish_keys`
  keeps them out of tabs), but the ticket page read them from the tab row,
  so it always said "No comments". `JiraTicketPage.cachedComments` now
  indexes `jiras.json` by key (once per mtime, off the main thread) and
  fills the Comments tab in place.
- **Status bar.** No more step per directory status. Unless `[jira] workflow`
  sets an explicit list, the bar is To Do › In Progress › Done with the
  issue's real status in its segment ("In Progress · Code Review").
  Category from directory.json `statusCategories` (new — the statuses stage
  now keeps each status's `statusCategory.key`), else lifecycle words.
- **Label pills** capped at 6 + "+N" (tooltip lists the rest).

## 5. Phase 1 — status filter grouped by category (built 2026-10-06)

As built: `JiraMultiPicker.Option` has `group` + `unused`; the picker draws a
header per group (`groupOrder`, `groupDetail`; its box picks the whole
group) and folds unused options under "Show N unused" (searching shows
everything flat). The table's status ▾ groups the tab's statuses with row
counts per category; the live search's Status picker groups the directory's
statuses; new live-search row "Status category" (`statusCategory in (…)`,
checked against a live site). The column ▾ only lists statuses present in
the tab, so nothing there is "unused".

Original spec:

- `JiraMultiPicker` for `status` (column ▾ and live search): section
  headers To Do / In Progress / Done (from `statusCategories`), each value
  with its count in the current tab. Values with 0 rows in the tab fold
  under "Show N unused" per section. Header click = select the whole
  category.
- Live search gets a "Status category" row (`statusCategory in (…)` JQL).
- Done when: on a 40-status site the picker opens showing ≤ 15 rows.

## 6. Phase 2 — labels (built 2026-10-06)

As built: directory labels carry `count` (issues among the sampled
`labels_max_issues`); the live search's Labels picker and the table's
labels ▾ sort by use and show the top 50 (`JiraMultiPicker.topN` /
`foldTail`), the rest behind "Show all N labels" (search covers all). The
table's labels picker has **Pin to Sidebar** (ticked labels) →
`jira_poll.py --pin-label add|remove NAME…` (config.json `pinnedLabels`).
Sidebar: one pinned list, sections per row (`PopupTabsBar.pinnedSection`):
FAVORITE RELEASES then LABELS (`label:NAME` ids, tag icon, right-click
Unpin Label). A label click = `--label-view NAME` → `jira_labels/label-NAME.json`
from the cache, shown in place (`pinView`), refreshed when the cache changes.
Cmd+K on issue rows: "Pin labels to sidebar" (their labels not pinned yet).

Original spec:

- Picker: labels sorted by count in the current tab, top 50, then "Show
  all N" (search covers all). Directory keeps a per-label count from its
  `labels` stage.
- **Pin a label**: Cmd+K "Pin label…" / ☆ in the picker → config.json
  `pinnedLabels` (via `jira_poll.py --pin-label add|remove NAME`). Sidebar
  section LABELS (`setSidebarPinned`, like FAVORITE RELEASES): clicking one
  shows the cache's issues with that label in place (`ListSession.pinView`,
  local filter, no request).

## 7. Phase 3 — boards (built 2026-10-06)

As built: directory stage `boards` (per scoped project, `/rest/agile/1.0/board?projectKeyOrId=`)
→ directory.json `boards` [{id, name, type, projects}] + `statusIds`.
`jira_poll.py --pin-board add|remove ID…`: `/board/ID/configuration` +
`/filter/FID` + `/board/ID/quickfilter` once → `~/.cache/jira/boards.json`
(columns as status names, quick filters) + a `board-ID` custom-jql job
(`boardJql`, scrum → `sprint in openSprints()`, 15m) publishing into
`jira_boards/` (no tab). `--board-sprint ID on|off`. Jira window: icon menu
▸ Pin Boards… (picker grouped by project), sidebar BOARDS (right-click: Poll
Board Now, Open Sprints Only, Unpin), a board opens grouped by its columns.
Quick filters: a **Quick filters** pill in the filter bar on a board (picker,
ANDed like Jira) → `--board-quickfilter ID QF…` = one key-only search
(board JQL AND the picked filters' JQL, scope) → the rows narrow to those keys.

Original spec:

- Directory stage `boards` (per project): `GET /rest/agile/1.0/board?projectKeyOrId=KEY`
  (paginated) → `{id, name, type, project}`; merged with team.json `boards`.
- **Pin a board** from Jira Config ▸ Definitions ▸ Boards (searchable list,
  grouped by project) → config.json `pinnedBoards`. Pinning adds a poll job
  of type `board` (`/board/{id}/configuration` once for its filter JQL +
  column → status mapping; then the filter's JQL ANDed with the scope,
  fetched like any job) → tab file `board-ID.json`.
- Sidebar BOARDS: pinned boards; selecting one shows its tab grouped by the
  board's own columns (P5 group-by with the board's column order). Scrum
  boards add a "Current sprint" toggle (`sprint in openSprints()`).
- Quick filters (`/board/{id}/quickfilter`) appear as chips over the table.

## 8. Phase 4 — saved filters + my work (built 2026-10-06)

As built: icon menu ▸ Import Favourite Filters → `--import-filters`
(`filter-ID` jobs, file = the filter's name, scope ANDed in by the sync;
idempotent, a changed JQL restarts the job). Sidebar MY WORK (`[jira]
my-work`, default true): `--my-work` → `jira_mywork/{mine,reported,today}.json`
from the cache; "me" = /myself once → `~/.cache/jira/me.json`. Watching =
a custom-jql job `mywork-watching` (`watcher = currentUser()`, 15m,
`sideDir: jira_mywork`), added by the first `--my-work`, fetched on first open.

Original spec:

- `jira_poll.py --import-filters`: `/filter/favourite` → one custom-JQL job
  per filter (scope ANDed in), the same flow as Confluence's
  `--import-saved`.
- Sidebar MY WORK: Assigned to me, Reported by me, Updated today — local
  filters over the cache (`/myself` id). Watching = a JQL job
  (`watcher = currentUser()`).

## 9. Phase 5 — group by (built 2026-10-06)

As built: icon menu / table header right-click (outside a filterable title)
▸ Group By: Status category, Status, Assignee, Priority, Release, Labels,
Project, other filterable columns, Board column (on a board). Group header
rows = `FieldRow` with `__group` (`PopupRow.groupHeader`: no checkbox / star,
drawn as a band ▾/▸ NAME count); click / Return toggles, Collapse / Expand
All. Remembered per window (UserDefaults `listGroupBy.jira`); boards default
to their columns. Paging is off while grouped. Epic / parent and Components:
`GROUP_FIELDS` are synced for every issue (`parent`, Server "Epic Link" via
`epic_link_ids`); an existing cache gets them once by `backfill_fields`
(only the new fields, key order; status `syncedFields` records what the
cache holds).

Original spec:

- Table header right-click ▸ Group by: Status category, Status, Assignee,
  Label, Release, Priority, Epic/parent, Component, Board column. Groups are
  collapsible headers with counts (`PopupRowView` group row; collapsed state
  per tab in UserDefaults). Sort stays within groups.
- Multi-value cells (labels) put an issue in each of its groups.
- Speed: grouping runs with the `FuzzyIndex` rebuild in the background;
  5k–20k rows must regroup in < 50 ms.

## 10. Checks

- `python3 Tests/test_jira_poll.py` green (directory `statusCategories`
  asserted).
- On a fake site with 40 statuses (`bin/fake-jira-tab.sh`), the ticket
  page bar shows 3 segments; the status picker opens with ≤ 15 rows.
- Ticket page Comments tab shows the cached comments and their count.
- No request outside `project_keys`.
