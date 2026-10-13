# PRD — Screenshot (Hyper+X): closing the Flameshot parity gaps

Companion to `PRD-screenshot.md` (the original spec). That document says what
the tool should be; this one records what the Rust port actually lacks today,
checked against Flameshot's own source and docs (Oct 12 2026), and orders the
work to close it.

## 1. Problem

The owner opens Screenshot (Hyper+X), presses **Space** or clicks **Tool
Settings**, and the Flameshot-style panel on the left never appears. Nothing
else tells them it is missing: the flag flips, no pixels change.

**Root cause (confirmed in code).** `ShotSession::toggle_side_panel`
(`rust/ws-rs/src/views/screenshot.rs`) only sets `side_panel_open`. The
module header marks the form `todo!()`: *"the side-panel settings form
(sliders/steppers/table) keeps its layout model only — its AppKit controls are
not rebuilt in the Rust overlay."* `ShotSidePanel` holds just `WIDTH = 250` and
a list-height helper. `render_chrome` paints the help card, button ring,
colour wheel, loupe and save card, but no panel. So the panel was lost in the
Swift → Rust cutover, not regressed by a recent change.

Screenshot was also dropped from the Hyper+S palette (Hyper+X is the trigger),
so the panel is the main visible gap left in this tool.

## 2. Objective

Everything Flameshot offers in its capture screen is either present and
working, or listed below as an explicit non-goal. Success: the side panel
exists and drives real state; no Flameshot sidebar control is missing; the
config surface matches Flameshot's drawing/capture options (§5).

## 3. Flameshot reference (what "parity" means)

Sources: `flameshot-org/flameshot` README and docs (key bindings, CLI),
`src/widgets/panel/sidepanelwidget.cpp`, `src/utils/confighandler.h`.

**Sidebar (Space).** Every control exists in the widget at all times:

| Control | Detail |
|---|---|
| Active tool size | label + spin box + horizontal slider, range 1…max, kept in sync |
| Active Color | swatch of the current draw colour |
| Grab Color | button; fires on press, hides the panel, picks a colour from the screenshot with live preview, Esc/abort restores the previous colour, then re-shows the panel |
| Colour wheel | selecting emits a colour change |
| Hex field | centred; invalid input reverts to the current hex; Esc inside it only leaves the field |
| Display grid | checkbox + grid size spin box (5…50, step 5, default 10, disabled until ticked) |

Flameshot's sidebar has **no layers list** (the original PRD added one as a
kitchen-sink extension; keep it, but it is not parity).

**Other capture-screen features:** tools P D A S R C M T B (+ counter, invert
in our set), colour picker `G`, right-click colour wheel, wheel = thickness,
Ctrl-constrain while drawing, Shift/Ctrl on handles, arrow-key move/resize,
Ctrl+M move selection, Ctrl+Backspace cancel selection, Ctrl+O open with app
(not on macOS), size badge, magnifier, copy on double-click.

**CLI:** `gui` (`-p -c -d --region --last-region -r -g -u --pin -s`), `screen`
(`-n`…), `full`, `launcher`, `config` (`-a -n -f -t -s -m -k --check`).
Exit codes 0 / 2 aborted / 3 DBus lost / 130 / 143.

## 4. Gap table

Status: **Missing** = confirmed absent in code; **Audit** = not verified in
this pass (check before building); **Have** = present per config/code.

| # | Feature | Status | Priority |
|---|---|---|---|
| G1 | **Side panel rendering + all six controls above** | **Done** (Oct 12; + Tool Settings tab) | P0 |
| G2 | Per-tool size memory (`drawThickness`, `drawFontSize`, counter, pixelate, rectangle, marker sizes) shown by the size control | **Done** (`draw-thickness` / `draw-font-size` seed sizes; per-tool memory in `ShotState`) | P0 (part of G1) |
| G3 | Text tool options in the panel: font family, B / I / U / S, align (in the original PRD §7.2.3) | **Done** except font family (B/I/U/S + align; live on the draft / selected text) | P1 |
| G4 | Grid overlay (`Display grid`, size 5…50) | **Done** (overlay-only, not in the output image) | P1 |
| G5 | Layers list: select/delete/reorder objects (PRD extension) | **Done** (rows select + dashed box, 🗑 / ▲ / ▼) | P2 |
| G6 | Grab Color flow wired to the panel (model `start_grab` exists, panel button does not) | **Done** (live preview, Esc restores, panel returns) | P1 |
| G7 | Selection geometry readout options: `showSelectionGeometry`, `…HideTime` | Missing (only `show-size-badge`) | P2 |
| G8 | `useJpgForClipboard`, `predefinedColorPaletteLarge`, `antialiasingPinZoom`, `showAbortNotification`, `showQuitPrompt` | Missing | P3 |
| G9 | Configurable shortcuts (Flameshot's Shortcuts tab) | Audit (we have `help-*` strings, not rebindable) | P3 |
| G10 | CLI: `config` subcommand with `--check`, `launcher`, exit codes 2/3 | Audit | P3 |
| G11 | Ctrl+M move selection, Ctrl+Backspace cancel selection, Ctrl-constrain, Shift/Ctrl handle modifiers | Audit (`PARITY-shortcuts.md` has "per-key verification ❓") | P1 |
| G12 | Already present, no work: buttons list, colours, magnifier, copy-on-double-click, save path/format/quality, filename pattern, save-after-copy, copy-path-after-save, last region, undo limit, arrow style, counter outline, insecure pixelate, delay | Have | — |

Non-goals (same as `PRD-screenshot.md`): Imgur upload and history, tray icon,
Linux/Wayland paths, "open with app" on macOS.

## 5. Solution

**Rendering approach.** The Rust overlay paints all chrome into one CoreGraphics
context (`render_chrome`); do the same for the panel instead of reviving
AppKit controls. This keeps the overlay a single non-activating surface and
matches how the save card is built (fields edited through the key router).

1. **Model first** (pure, unit-tested in `ShotSidePanel`): layout rects for
   the six controls at `WIDTH = 250`, hit-testing, spin/slider value mapping,
   hex validate-or-revert, grid step clamping. No drawing code in the model.
2. **Draw** in `render_chrome` when `side_panel_open`: translucent dark panel
   on the left edge, full height, above the selection but below the button
   ring; theme colours from `ui-color` / `contrast-color` / `contrast-opacity`.
3. **Input**: mouse (drag slider, click ± on the spinner, click swatch row,
   click the wheel) in the overlay's existing mouse path; keyboard for the hex
   field through the key router (rule 1: Cmd+C/V/X/A/Z and Ctrl+C/V work
   there; Esc inside the field leaves the field, never the overlay).
4. **State**: size, colour and grid write to `ShotSession` (`size`, `color`,
   new `grid_on` / `grid_size`) and persist per-tool sizes. Expose them in the
   socket state (`sidePanel` already exists; add `toolSize`, `color`, `grid`).
5. **Grab Color** button reuses `start_grab` / `cancel_grab`; the panel hides
   during the grab and returns afterwards.
6. **Config over code** (rule 3): all labels (`Active tool size:`, `Active
   Color:`, `Grab Color`, `Display grid`) and the width live in
   `commands.toml [screenshot]`; add `draw-thickness`, `draw-font-size`,
   `grid-size`, `side-panel-width`.

## 6. Phases

| Phase | Scope | Done when |
|---|---|---|
| 1 | G1, G2, G6: panel with size, colour, grab, wheel, hex | Space opens it; changing size/colour changes the next stroke; grab works; Esc order unchanged |
| 2 | G4, G3: grid + text options | Grid draws at the set pitch; text tool shows its widget |
| 3 | G11, G7: shortcut audit and fixes | Every key in §3 verified; `PARITY-shortcuts.md` ❓ → ✅/❌ |
| 4 | G5, G8–G10 | As time allows |

## 6a. Status (Oct 12 2026)

Phase 1 + the grid half of Phase 2 landed. Verified live: `do:screenshot:side-panel`
→ a screen capture shows the panel; `do:screenshot:panel-click:X,Y` (coordinates from
`sidePanelLayout` / `toolTab` in the state) toggled the grid, and it draws. State adds
`toolSize`, `color`, `grid`, `hexEdit`, `sidePanelLayout`, `toolTab`. Unit tests:
`cargo test -p ws-rs side_panel` (layout, hit-test, slider/grid/hex/wheel mapping,
grab hide/return, Esc order, hex edit shortcuts, config keys).

Known limits: the wheel is hue/saturation at full brightness (dark colours via
the hex field); the hex field has no caret movement (select-all / append /
backspace only); `grid-size` is not a config key (the grid size persists in the
screenshot state file). Open: G3 text options, G5 layers, G7–G11.

## 6b. Live annotations (Oct 12 2026, evening)

The overlay never painted annotation objects: `ShotOverlayView::render` drew only
the text draft, so arrows, counters, shapes and committed text appeared only in the
output image after Enter. It now draws every object, the one being drawn, and a
dashed box around the selected object (`shot_cg::draw_objects`, shared with the final
render). Verified with real CGEvent mouse drags/clicks and System Events keys (not
`do:` hooks, which only touch the model): arrow mid-drag and after release, counters
1-2-3, text typing + commit, object move, scroll resize, selection handle resize,
undo/redo, Cmd+C copy (clipboard PNG matches the screen), paste into a note, and a
single click on the pasted image opening the preview in nvim and Prose modes.

## 7. Acceptance checks

- `do:` / state probe: after `side-panel` the state reports `sidePanel: true`,
  and a `pane-shot` PNG shows the panel (the only proof that counts; the flag
  alone was already true while nothing drew).
- Unit tests for the pure model (layout, hit-test, hex revert, grid clamp).
- Rule 1 test: paste into the hex field.
- Rule 6: Esc closes the panel/hex field first, the overlay last.
- Manual: Hyper+X → drag a region → Space → change size → draw → colour →
  Grab Color → grid on.

## 8. Risks

- Slider/spinner hand-drawn controls need careful hit-testing; mitigate by
  keeping them in the pure model with tests.
- Overlay windows are per display; the panel belongs to the display holding
  the selection (`session.active`), as the button ring does.
- Reviving AppKit controls would conflict with the non-activating overlay,
  so not recommended.
