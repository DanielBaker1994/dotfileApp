# PRD — `/screenshot`: a Flameshot-style capture tool inside kitchen-sink

Status: ready to implement · Written: 2026-10-02 · Target: kitchen-sink (Swift/AppKit, macOS ≥ 26.7, arm64)

> **For the implementing agent.** Read `AGENT_CONTEXT.md` (= `CLAUDE.md`) and
> `rule.md` first. They are binding. This PRD says *what* to build and which
> parts of the codebase to plug into. Section 7 is the spec. Section 9 is the
> list of checks that decide whether you are done. Do not read the big Swift
> files whole. Grep the symbols named here and read about 60 lines around each.

---

## 1. Summary

Add a screenshot tool to kitchen-sink that looks and behaves almost
exactly like **Flameshot**: press a hotkey, the screen freezes and goes dim,
you drag a box around what you want, a ring of round purple buttons appears
around the box, you draw arrows, boxes, text, numbers and blur on it, then copy,
save or pin it. It runs inside the existing daemon as a **tool panel** (like
`/paths` and `/filefast`), so it never pulls the shared window forward and
never needs Flameshot itself.

## 2. Contacts

| Name | Role | Comment |
|---|---|---|
| Daniel Baker | Owner, only user, reviewer | Decides scope and approves the final look. Already uses Flameshot 12.1 (`/Applications/flameshot.app`, config `~/.config/flameshot/flameshot.ini`). |
| Implementing agent | Builds the feature | Follows `AGENT_CONTEXT.md` and `rule.md`. Does not commit unless asked. |
| PRD author (Claude) | Research + spec | Studied the local Flameshot 12.1 app and the upstream v14 source (see §10). |

## 3. Background

**What this is.** kitchen-sink is the owner's macOS menu-bar app. It
holds notes, files, Jira, Confluence and AI views in one shared window, plus a
few "tool panels" (`/filefast`, `/paths`, `/prettyprint`, `/health-checks`)
that behave like small standalone apps. The owner takes screenshots with
Flameshot, marks them up, and pastes them into Webex, Outlook and Jira.

**Why now.**

- **Flameshot is going stale on this Mac.** The Homebrew cask was *disabled
  on 2026-09-01* because it fails Gatekeeper. The installed copy is v12.1.0
  (Qt 5, from 2022). Upstream v13 moved to Qt 6 and v14 changed multi-monitor
  handling. Updates are now a manual, unsigned-app chore.
- **It is a foreign app in a tuned setup.** Flameshot needs its own tray
  icon, its own hotkey daemon, its own Screen Recording grant, and its own
  config file. None of it follows the app's theme, `commands.toml`, the
  AeroSpace hotkey scheme or the `/paths` shelf.
- **The building blocks already exist.** The app has non-activating tool
  panels that AeroSpace ignores, a hotkey path through the daemon socket, a
  toast, a theme system, and `PathShelf` for "files I just made". macOS 26
  ships ScreenCaptureKit's one-shot `SCScreenshotManager`, so a fast,
  native, full-resolution capture is a single API call.

## 4. Objective

**Objective.** Replace Flameshot for daily use with a built-in tool that feels
the same in the hands. Muscle memory (drag, single-letter tool keys, mouse
wheel for size, right-click for color, Cmd+C to copy) must carry over unchanged.

**Why it matters.** One hotkey, one config file and one signed app instead of
two. Captures land where the rest of the workflow already looks: the
clipboard, `/paths` and Quick Look.

**Key results (each must be measurable with the checks in §9):**

| # | Key result | Target |
|---|---|---|
| KR1 | Hotkey to frozen, dimmed overlay on screen | ≤ 250 ms p50, ≤ 400 ms p95 on the owner's Mac (log line `screenshot: N ms to overlay`) |
| KR2 | Feature parity with Flameshot's **macOS** capture mode (§7.2 table) | 100% of the "v1" rows |
| KR3 | Keyboard parity: every Flameshot capture shortcut in §7.5 does the same thing | 100% |
| KR4 | No focus side-effects: opening and closing the tool never activates the app, never moves or raises the shared window, and never changes AeroSpace's view | 0 activations in `bin/ui-test-focus.py tools` |
| KR5 | Output fidelity: a copied/saved image is pixel-for-pixel the selected region at the display's native (Retina) scale, plus annotations | Unit test compares rendered PNG size to `rect × backingScaleFactor` |
| KR6 | Owner stops launching Flameshot | Owner's call after 2 weeks of use |

## 5. Market segment

There is one user, the owner, but the job is a common one:

> "When I see something on screen I need to show or explain to someone, I
> want to grab exactly that part, point at the important bit and hide the
> private bit, then paste it into chat or a ticket. All of that in a few
> seconds, without leaving the keyboard-driven setup I'm in."

**Constraints that shape it:**

- Keyboard-first, AeroSpace tiling WM, Hyper-key hotkeys, sketchybar on top.
  The overlay must cover the sketchybar area and the menu bar too.
- Up to two monitors ("main" and "secondary", see
  `bin/aerospace-monitors.sh`), Retina scale factors that can differ.
- Shares into Webex, Outlook and Jira. All of them accept a PNG pasted from
  the clipboard. Webex has no rich tables. That doesn't matter for images.
- macOS privacy: Screen Recording permission (TCC `kTCCServiceScreenCapture`)
  is mandatory and **cannot** be pre-granted by `bin/grant-permissions.sh`,
  because it lives in the system TCC database.
- The app is unsandboxed and signed with a stable self-signed certificate, so
  a grant survives rebuilds (see "Signing" in `AGENT_CONTEXT.md`).

## 6. Value propositions

| Job / need | Gain | Pain avoided | Better than Flameshot / macOS built-in |
|---|---|---|---|
| Grab a region fast | One hotkey, frozen screen, so menus and tooltips can be captured | Racing a disappearing tooltip | Same as Flameshot. macOS ⇧⌘4 has no annotate step. |
| Point at things | Arrow, box, circle, marker, numbered steps, text | Opening Preview/Skitch afterwards | Same as Flameshot. macOS markup is a second step in a separate window. |
| Hide secrets | Pixelate/blur that can't be reversed (default "secure" mode) | Leaking tokens in a Jira ticket | Same algorithm as Flameshot. |
| Share it | Copy (Cmd+C / Return) lands as PNG; Save also files it in `/paths` | Hunting for "Screenshot 2026-…png" on the Desktop | **Better:** `/paths` shelf + Quick Look integration. |
| Keep a reference visible | Pin the capture as a floating, always-on-top image | Alt-tabbing to a saved file | Same as Flameshot. AeroSpace ignores it, so it never tiles. |
| Stay in one setup | Theme, config, hotkey and permissions all in kitchen-sink | A second unsigned app, tray icon and config file | **Better:** one app, `commands.toml`, the Setup window. |

**Value curve, in short:** match Flameshot on capture and annotation, beat it
on integration (config, `/paths`, focus behavior, signing). Skip what the
owner doesn't use: cloud upload, "open with" app launcher, history dialog,
tray icon.

---

## 7. Solution

### 7.1 UX and user flows

#### Main flow

```
Hyper+X ──► capture every display (ScreenCaptureKit, before anything is drawn)
        ──► one overlay panel per display, showing the frozen image, dimmed
        ──► help card centered on the screen under the mouse
        ──► user drags a rectangle  ─┐
                                     ├─► selection + 8 round handles + button ring
        (or Cmd+A = whole screen) ───┘
        ──► user picks a tool (click a button or press its letter) and draws
        ──► finish:
              Cmd+C / Ctrl+C / Return ─► PNG on clipboard, toast, overlay closes
              Cmd+S                   ─► save dialog (or fixed path), toast, closes
              Pin button              ─► floating pinned image, overlay closes
              Esc / Cmd+Q / ✕         ─► closes, nothing saved ("Screenshot aborted.")
```

#### Wireframe A: before a selection (observed on Flameshot 12.1)

The whole screen shows the frozen capture under a black veil with alpha
`contrast-opacity/255` (default 190). A help card sits in the middle of the
screen that has the mouse. It has rounded corners (~6 pt), is filled with the
UI color, has a thin light border, and uses white text in two columns (key
right-aligned and bold, action left-aligned). A vertical "Tool Settings" tab
is stuck to the left edge, centered vertically, in the UI color.

```
┌──────────────────────────────────────────────────────────────────────┐
│ (frozen screen, dimmed ~75% black)                                   │
│                                                                      │
│ ┃T┃                  ┌───────────────────────────────────┐           │
│ ┃o┃                  │        Mouse  Select screenshot area          │
│ ┃o┃                  │           ⌘S  Save screenshot to a file      │
│ ┃l┃                  │           ⌘C  Copy selection to clipboard     │
│ ┃ ┃                  │  Mouse Wheel  Change tool size                │
│ ┃S┃                  │  Right Click  Show color picker               │
│ ┃e┃                  │        Space  Open side panel                 │
│ ┃t┃                  │          Esc  Exit                            │
│ ┃.┃                  └───────────────────────────────────┘           │
└──────────────────────────────────────────────────────────────────────┘
```

Help card rows, verbatim from Flameshot 12.1 on macOS, in this order:
`Mouse — Select screenshot area`, `⌘S — Save screenshot to a file`,
`⌘C — Copy selection to clipboard`, `Mouse Wheel — Change tool size`,
`Right Click — Show color picker`, `Space — Open side panel`, `Esc — Exit`.
The card is hidden while a selection exists. `[screenshot] show-help = false`
turns it off. Put the strings in `commands.toml` (rule 3) with these as defaults.

#### Wireframe B: with a selection (observed on Flameshot 12.1)

Inside the selection the image is **not** dimmed. The border is 1 px in the
UI color. Eight filled circles in the UI color (diameter = 60% of the button
size) sit on the corners and edge midpoints. Round buttons (filled UI color,
white icon, soft shadow) are placed in a ring **outside** the selection:
bottom first, then right, then top, then left (§7.2.4).

```
                    (pin)(upload*)                       ← top row (overflow)
          ●──────────────────●──────────────────●   (✕)  ← right column:
          │                                     │   (💾)    exit, save, copy,
          │       (undimmed selection)          │   (⧉)     redo, undo, move,
          ●                                     ●   (↻)     size badge "1002
          │                                     │   (↺)                 602"
          │                                     │   (✥)
          ●──────────────────●──────────────────●  (1002/602)
     (✎)(／)(↙)(▢)(■)(◯)(🖍)(T)(①)(▦)(◐)        ← bottom row: drawing tools
```
\* upload is out of scope (§8). Its slot is simply not there.

The size badge is a round button showing the selection's width over height
in **points**, stacked on two lines in a small bold font (observed: `1002` /
`602`). It updates live while you drag or resize.

#### Wireframe C: side panel (Space or click "Tool Settings")

A panel slides in from the left edge, about 250 pt wide and full height, with
a translucent dark background. Top to bottom it shows:
`Active tool size:` (spin box + slider, 1…100) · `Active Color:` (swatch +
name) · `Grab Color` button · color wheel · hex field (`#RRGGBB`, Esc in it =
leave the field, invalid input reverts) · `Display grid` checkbox + grid size
spin box (5…50, step 5, default 10, disabled until the box is ticked) ·
**Layers** list (one row per drawn object, newest on top; select a row to
select the object; Delete button; ↑/↓ buttons reorder).

#### Wireframe D: color wheel (right-click)

Right-click anywhere opens a circular palette centered on the cursor: a ring
of preset color dots plus the user's custom colors (`user-colors`). Moving
the mouse highlights a dot. Releasing the right button (or a left click)
picks it. Esc closes the palette only (rule 6). If an object is selected, its
color changes (undoable). Otherwise the drawing color changes.

#### Wireframe E: pinned capture

A borderless, always-on-top panel holds the image at 1:1, with a 2 px soft
shadow in the UI color that switches to the contrast color on hover.

| Input | Action |
|---|---|
| Drag | move the window |
| Scroll / pinch | zoom, 3% per wheel step, min 100 px on the short side |
| `0`…`9` | opacity: `0` = 100%, `1` = 90% … `9` = 10% |
| Double-click, Esc, Cmd+Q, Cmd+W | close |
| Right-click menu | `Copy to clipboard`, `Save to file`, —, `Rotate Right`, `Rotate Left`, `Increase Opacity`, `Decrease Opacity`, —, `Close` |
| Cmd+C | copy the pinned image |

### 7.2 Key features

#### 7.2.1 Feature table (scope)

| Feature | Flameshot | v1 | Notes |
|---|---|---|---|
| Frozen full-screen capture, dimmed overlay | ✓ | ✓ | §7.3.2 |
| Region select: drag, 8 handles, move by dragging inside | ✓ | ✓ | |
| Keyboard nudge / resize / symmetric resize | ✓ | ✓ | §7.5 |
| Select all (whole screen under the mouse) | ✓ | ✓ | Cmd+A |
| Help card, "Tool Settings" tab, side panel | ✓ | ✓ | |
| Button ring around the selection, emerge animation | ✓ | ✓ | §7.2.4 |
| Size badge (W × H) | ✓ | ✓ | |
| Pencil, Line, Arrow, Rectangle (outline), Rectangle (filled), Circle, Marker, Text, Circle counter, Pixelate, Invert | ✓ | ✓ | §7.2.3 |
| Move selection tool, Undo, Redo | ✓ | ✓ | |
| Copy, Save, Exit, Accept | ✓ | ✓ | |
| Pin | ✓ | ✓ | Wireframe E |
| Size + / Size − buttons | ✓ (hidden by default) | ✓ (hidden by default) | in `buttons` config |
| Right-click color wheel, grab color (`G`) | ✓ | ✓ | |
| Mouse-wheel tool size + on-screen size indicator | ✓ | ✓ | |
| Selecting / moving / deleting / recoloring drawn objects, Layers list | ✓ | ✓ | |
| Display grid | ✓ | ✓ | |
| Magnifier loupe | ✓ (off by default) | ✓ (off by default) | `magnifier = false` |
| Delay (`-d ms`), `--region`, `--last-region`, `--accept-on-select`, `--clipboard`, `--path`, `--pin`, `--raw`, `--print-geometry` | ✓ (CLI) | ✓ | §7.4 |
| Full-screen and single-screen capture with no UI (`full`, `screen -n`) | ✓ | ✓ | §7.4 |
| Copy on double-click | ✓ (off) | ✓ (off) | |
| Imgur upload, upload history | ✓ (off since v13) | ✗ | out of scope |
| "Open with app" launcher | ✗ on macOS | ✗ | Flameshot hides it on macOS too |
| Launcher dialog, screenshot history window, tray icon | ✓ | ✗ | later, maybe |
| Monitor picker before capture (v14) | ✓ v14 | ✗ | v1 overlays every screen at once, like 12.1 on macOS. See §7.3.2. |

#### 7.2.2 Button ring: buttons and default order

Default order, macOS, verbatim from Flameshot's `buttonTypeOrder` (uploader
and open-app left out):

| # | Button | Key | Kind | Tooltip / description |
|---|---|---|---|---|
| 0 | Pencil | `P` | draw | Set the Pencil as the paint tool |
| 1 | Line | `D` | draw | Set the Line as the paint tool |
| 2 | Arrow | `A` | draw | Set the Arrow as the paint tool |
| 3 | Rectangle (outline) — Flameshot calls it "Rectangular Selection" | `S` | draw | Set Selection as the paint tool |
| 4 | Rectangle (filled) | `R` | draw | Set the Rectangle as the paint tool |
| 5 | Circle (ellipse) | `C` | draw | Set the Circle as the paint tool |
| 6 | Marker | `M` | draw | Set the Marker as the paint tool |
| 7 | Text | `T` | draw | Add text to your capture |
| 8 | Pixelate | `B` | draw | Set Pixelate as the paint tool |
| 9 | Invert | `I` | draw | Set Inverter as the paint tool |
| 10 | Circle counter | — | draw | Add an autoincrementing counter bubble |
| 12 | Move selection | `Cmd+M` | mode | Move the selection area |
| 13 | Undo | `Cmd+Z` | action | Undo the last modification |
| 14 | Redo | `Cmd+Shift+Z` | action | Redo the next modification |
| 15 | Copy | `Cmd+C` | action (closes) | Copy selection to clipboard |
| 16 | Save | `Cmd+S` | action (closes) | Save screenshot to a file |
| 18 | Accept | `Return` | action (closes) | Accept the capture |
| 19 | Exit | `Cmd+Q` | action (closes) | Leave the capture screen |
| 20 | Pin | — | action (closes) | Pin image on the desktop |
| 22 | Size + | — | action | Increase tool size (not shown by default) |
| 23 | Size − | — | action | Decrease tool size (not shown by default) |

Observed default ring in 12.1 (what the owner is used to): bottom row = Pencil
… Invert (11 drawing tools), right column = Exit, Save, Copy, Redo, Undo,
Move, then the size badge. Top row = Pin. **Accept and the size buttons are
not shown by default** but stay reachable by keyboard. `[screenshot] buttons`
(comma list of the names above) chooses which buttons show and in what order.

Icons: use SF Symbols that read the same as Flameshot's Material icons
(`pencil`, `line.diagonal`, `arrow.down.left`, `square`, `square.fill`,
`circle`, `highlighter`, `textformat`, `1.circle`,
`square.grid.3x3.fill` (pixelate), `circle.lefthalf.filled` (invert),
`arrow.up.and.down.and.arrow.left.and.right`, `arrow.uturn.backward`,
`arrow.uturn.forward`, `doc.on.doc`, `square.and.arrow.down`, `xmark`,
`pin.fill`, `checkmark`, `plus`, `minus`). White icon on the UI color. If the
UI color is light, use a black icon (Flameshot's `colorIsDark` rule: perceived
luminance < 0.5 is dark).

#### 7.2.3 Drawing tools: behavior spec

Common rules (all drawing tools):

- **Thickness** = "tool size", one value per tool, remembered in
  `~/.cache/kitchen-sink/screenshot-state.json`. Defaults: draw
  thickness 3, font size 8, marker 5, pixelate 2, circle counter 1, rectangle
  corner radius 1 (Flameshot `drawThickness`, `drawFontSize`,
  `drawMarkerSize`, `drawPixelateSize`, `drawCircleCounterSize`,
  `drawRectangleSize`).
- **Mouse wheel** changes the active tool's size by ±1 per notch. Trackpad
  scroll is rate-limited to one step per 200 ms. A small rounded box near the
  top-left of the screen shows the number and fades out about 1 s after the
  last change, which is when the size is saved.
- **Shift while drawing** (two-point tools): Line and Arrow snap to 0/45/90°
  (8 directions). Rectangle, filled rectangle, circle and pixelate snap to a
  square/circle (diagonal snapping).
- **Mouse preview**: Pencil, Marker, Line and Arrow show a dot of the current
  thickness and color under the cursor.
- **Commit**: an object is committed on mouse-up (text: when editing ends),
  pushed onto the undo stack, and added to the Layers list.
- **Drawing is clipped to the selection** when the result is rendered. While
  drawing, strokes may go outside the selection (as in Flameshot).
- **Selecting existing objects.** With a drawing tool active, clicking on an
  existing object selects it (dashed bounding box). Drag moves it. `Delete`
  or `Backspace` deletes it (Flameshot macOS: Backspace). Wheel changes its
  size. Right-click color changes its color. Double-click on text re-enters
  editing. Every change is undoable.

| Tool | Rendering |
|---|---|
| Pencil | Freehand polyline, round caps and joins, antialiased, width = size. |
| Line | Straight line, round caps, width = size. |
| Arrow | Line plus a filled triangular head at the end point. Head length ≈ 3 × size + 10, head width ≈ 2 × size + 6 (tune visually against 12.1). `arrow-style` 0 = filled head, 1 = open "V" head. `reverse-arrow = true` puts the head at the start. |
| Rectangle (outline) | Stroked rect, width = size, square corners. |
| Rectangle (filled) | Filled rect, corner radius = `rectangle-radius` (default 1). |
| Circle | Stroked ellipse inside the dragged rect. |
| Marker | Like Line but semi-transparent (alpha ≈ 0.4), width = size × 2 + marker size, flat caps, `multiply`-like look on light backgrounds. |
| Text | Click starts a text box at that point. Font = `font` (default system), point size = size + 8, color = draw color. Its config widget (shown in the side panel while Text is active) has Font family, **B** / *I* / U / S toggles, and align left / center / right. Editing: Return = new line, **Esc or Cmd+Return or a click outside** = commit, empty text = discarded. The box grows with the text, with 5 pt padding. All edit keys work in the box (rule 1: Cmd+C/V/X/A/Z and Ctrl+C/V). |
| Circle counter | Click places a filled circle, diameter = size + 15 pt, fill = draw color. The number inside is bold white on dark fills and black on light fills. An optional 1 px contrasting outline (`counter-outline = true`). Click and drag draws a tapered "tail" from the bubble toward the drag point. Numbers start at 1 and go +1 per bubble (max 999). Deleting or undoing a bubble renumbers the later ones. The wheel *while placing* changes the next number. |
| Pixelate | Drag a rect. **Secure mode (default)** follows Flameshot: build the pixelated blocks only from colors sampled on the rect's *outer fringe* (top, bottom, left and right borders) with a little noise, interpolated across, so the hidden pixels cannot be recovered. Block resolution = `rect × 0.5 / (size + 1)`. `insecure-pixelate = true` = real downscale and upscale of the pixels inside (size ≤ 1 → Gaussian blur radius ~10). |
| Invert | Drag a rect. Colors inside are inverted. |
| Move selection | While active (`Cmd+M` or its button), dragging anywhere moves the selection itself and the cursor is an open hand. Pressing it again leaves the mode. |

#### 7.2.4 Button ring placement (port of Flameshot's `ButtonHandler`)

- Button size `B` = `[screenshot] button-size`, default **round(lineSpacing ×
  2.2)** of the system font at 13 pt. That's about 34 pt and matches 12.1.
  Gap `g = B / 4`. Pitch = `B + g`.
- Fill the sides in this order: **bottom, right, top, left**. Each side holds
  `(side length + g) / pitch` buttons, centered on that side. Rows sit `g`
  outside the selection edge. Corners take up to 2 extra buttons each when
  room is short.
- A side is **blocked** when the buttons would leave the screen. Blocked
  sides are skipped. If all four are blocked (selection ≈ full screen or a
  very small one in a corner), put the remaining buttons **inside** the
  selection, along its bottom edge, then up.
- If the selection is smaller than `B` in either direction, buttons are laid
  out as if it were `B` (Flameshot's `ensureSelectionMinimumSize`).
- When the left side is blocked, shift the horizontal center right by
  `pitch / 2`, and vice versa (Flameshot's `adjustHorizontalCenter`).
- **Emerge animation:** each button grows from 0 to `B` in 80 ms, ease
  in-out, when the ring is first shown and when a new selection is made.
- The ring hides while you drag or resize the selection and while the mouse
  is over a spot where the ring would cover what you're drawing. It comes back
  on mouse-up.
- Hover: the button gets slightly lighter. The active drawing tool's button
  is shown "pressed" (contrast color ring or inverted fill). Tooltip = the
  description from §7.2.2 after the normal hover delay.

#### 7.2.5 Output

- **Render**: the frozen display image cropped to the selection at the
  display's native pixel scale, plus every committed object drawn on top at
  the same scale, giving one `CGImage`.
- **Copy** (Cmd+C, Ctrl+C, Copy button, Return/Accept by default): write ONE
  pasteboard item holding PNG data (`public.png`) and TIFF for older apps.
  Then show a toast, `[screenshot] copy-toast`, default `Capture saved to
  clipboard`. Closes the overlay. `save-after-copy = true` also saves.
- **Save** (Cmd+S): when `save-path-fixed = true`, write straight to
  `save-path/filename-pattern`. Otherwise show an `NSSavePanel` opened at
  `save-path` with the name pre-filled. It must appear **above** the overlay
  and must not activate the app in a way that raises the shared window. Hide
  the overlay first, then present the panel as a floating, tool-panel-style
  window. Format from the extension (`png` default, `jpg` with
  `jpeg-quality` 75). Toast: `Capture saved as {path}`. Then:
  - `PathShelf.shared.add([path], why: …)` so it tops the `/paths` list
    (add a `screenshot` case to `PathShelf.Why`);
  - `copy-path-after-save = true` → the path as text on the clipboard.
- **Filename pattern** `filename-pattern`: strftime, default
  `%F_%H-%M` (Flameshot's default) + `.png`. On a clash append ` 2`, ` 3`
  (the same rule `FileDrag` uses for "keep both").
- **Abort** (Esc with nothing open, Cmd+Q, Exit): log
  `screenshot: aborted`. No toast.
- **Toast** lives in a tiny non-activating panel at the bottom-center of the
  screen the selection was on. Reuse the look of
  `PopupWindow.showToast` (Raycast-style pill, 1.4 s fade/rise). Don't route
  it through a shared-window view, which would show that window.

### 7.3 Technology

#### 7.3.1 Where it lives (fit into the existing architecture)

| Piece | Location (new file unless stated) | Notes |
|---|---|---|
| Capture + overlay controller | `Screenshot.swift` (`ScreenshotController`) | Owns the session: capture, panels, state, output. |
| Overlay drawing + input | `ScreenshotOverlay.swift` (`ScreenshotOverlayView`, one per display) | One `NSView` with layer-backed drawing. No SwiftUI. |
| Annotation model | `ScreenshotAnnotations.swift` | **AppKit-light, unit-testable**: tool objects, hit-testing, undo stack, counter renumbering, button-ring layout math, pixelate algorithm, filename pattern. |
| Pinned image | `ScreenshotPin.swift` (`PinPanel`) | |
| Config | `commands.toml` `[screenshot]` + `makeCommand` / `configNumberKeys` / `validateConfig` in `kitchen_sink.swift` | Every value through `configEntry` / `configLine`. |
| Hook into tools | `isToolPanel` / `openTool` in `kitchen_sink.swift` | Add `"screenshot"` to the tool-panel name list and a `case "screenshot": showScreenshot(cmd)`. |
| Hotkey | `config/aerospace/aerospace.toml` | `alt-cmd-ctrl-shift-x = 'exec-and-forget ~/.config/kitchen-sink/kitchen-sink.app/Contents/MacOS/kitchen-sink screenshot'`. **Hyper+X** mirrors Flameshot's macOS default `Ctrl+Shift+X`. Hyper+S/N/T are taken. Hyper+X is free (checked). Comment it like the Hyper+N/T lines. |
| CLI / socket | `main.swift` forwards `screenshot …` to the daemon. Socket messages: `screenshot` (= gui), `screenshot:gui|full|screen|pin…` with flags (§7.4), test hooks `do:screenshot:*`, state `screenshot` | Like `window` / `terminal`. Do **not** add it to `hotkeyModes`: that runs `hotkeyPrep` (AeroSpace queries) the tool doesn't need, and costs 20–60 ms. |
| Shortcuts list | `[shortcuts]` in `commands.toml` | Add `"all: Hyper+X" = "screenshot (Flameshot-style)"` and a `"screenshot: …"` block for the capture keys. |
| Setup / health | `bin/preflight.sh` + `SetupWindow.swift` | New warning row "Screen Recording permission" with Fix = open System Settings ▸ Privacy & Security ▸ Screen & System Audio Recording (`url:x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture`). |
| Palette | Hyper+S `/screenshot` | Comes free as a `[screenshot]` section with `label = "Screenshot"`. Palette `accept` already hides with `restore: false` for tool panels. |
| Docs | `AGENT_CONTEXT.md` | Add a "/screenshot" section in the same style as "/paths", and code-map rows. |

#### 7.3.2 Capture and overlay windows

- **Capture first, then show.** On the trigger, call
  `SCShareableContent.current` (cache it and refresh on
  `NSApplication.didChangeScreenParametersNotification`). Then, for **each**
  `SCDisplay` in parallel, call
  `SCScreenshotManager.captureImage(contentFilter: SCContentFilter(display:,
  excludingWindows: []), configuration:)` with `width/height = display points
  × backingScaleFactor`, `showsCursor = false`, `captureResolution = .best`.
  Do not exclude our own windows: Flameshot freezes exactly what is on
  screen. Don't use `CGWindowListCreateImage`, which is obsolete on macOS 15+.
- **One overlay per display**: a borderless `NSPanel` with
  `.nonactivatingPanel`, frame = `screen.frame` (covers the menu bar and the
  sketchybar strip), `level = .screenSaver` (above sketchybar, above the
  menu bar and above `/paths`' floating level), `collectionBehavior =
  [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary,
  .ignoresCycle]`, `hidesOnDeactivate = false`, `isOpaque = true`, no shadow.
  It shows the frozen image of *that* display, so nothing changes visually
  when it appears.
- **Tool-panel rule (AGENT_CONTEXT "Tool panels")**: never call
  `NSApp.activate`. Make the panel under the mouse key with
  `orderFrontRegardless()` + `makeKey()`. No `didBecomeActive` observer. When
  the mouse crosses to another display, make that display's panel key.
  AeroSpace ignores NSPanels, so no AeroSpace IPC is needed (and none is
  allowed: no `focus`, no cache clear).
- **Selection is per display.** The display where the drag starts owns the
  selection. Other displays stay fully dimmed and show no help card. A new
  drag on another display moves the selection there. This keeps Retina scale
  simple and matches what 12.1 does on this Mac. Spanning displays is
  out of scope.
- **Close** = order out every overlay panel, release the images. Keyboard
  goes back to the frontmost app by itself (no focus hand-back, same as the
  other tool panels).
- **Esc order (rule 6 + Flameshot's `deleteToolWidgetOrClose`)**: text being
  edited → color wheel → side panel → selected object (deselect) → active
  drawing tool (back to "no tool") → close the overlay. One step per Esc.
- **Keys**: install a local `keyDown` monitor on the overlay panels, as
  `PopupWindow.installMonitors` does. The app has no Edit menu, so key
  equivalents must be routed by hand (rule 1). Cmd+Q must **never** quit
  the daemon while the overlay is up. It means "exit capture".
- **Delay** (`-d ms` / `delay` config): wait *before* capturing, with no
  overlay shown, so a menu can be opened in the meantime.
- **Last region**: save the last accepted selection (display ID + rect in
  points) to `screenshot-state.json` when `save-last-region = true`.
  `--last-region` uses it.

#### 7.3.3 Permission

- On every trigger, check `CGPreflightScreenCaptureAccess()`. If it is false:
  call `CGRequestScreenCaptureAccess()` once per app run, show a toast
  `Screen Recording permission needed — opening Settings…`, open the Privacy
  pane, and abort. Never show a black or wallpaper-only overlay. That is
  what an ungranted capture returns, and it looks broken.
- `bin/preflight.sh` warning row (also listed in the JSON for the Setup
  window). Document in `AGENT_CONTEXT.md` that `grant-permissions.sh` cannot
  grant it.

#### 7.3.4 Performance budget (KR1)

| Step | Budget |
|---|---|
| Socket → main thread | ≤ 25 ms (existing path) |
| `captureImage` per display (parallel) | ≤ 120 ms |
| Build/show panels (prewarm them hidden at launch, like `[app] preload`) | ≤ 40 ms |
| First frame | ≤ 30 ms |

Log `screenshot: capture N ms, M ms to overlay` to `/tmp/ws-debug.log`. Keep
the drawing cheap: draw the dimmed frozen image once into a layer and redraw
only the dirty rect of the selection and annotations on mouse moves.

#### 7.3.5 Config: `[screenshot]` in `commands.toml`

One-line entries only, read via `configEntry` (see "commands.toml" in
`AGENT_CONTEXT.md`). Defaults = Flameshot's. The owner's
`commands.toml` should be seeded with the values from their
`~/.config/flameshot/flameshot.ini` (`contrastOpacity=188`,
`drawColor=#288000`, `savePath=/private/tmp`). The dist
`commands.default.toml` keeps Flameshot's defaults.

```toml
# /screenshot (Hyper+X): Flameshot-style capture + annotate (Screenshot.swift).
[screenshot]
enabled = true
label = "Screenshot"
type = "shell"
icon = "camera"
ui-color = "#740096"            # Flameshot purple; "theme" = the app's accent
contrast-color = "#270032"
contrast-opacity = 188          # 0-255 veil over the unselected area (Flameshot default 190)
draw-color = "#288000"          # Flameshot default red #ff0000
user-colors = "picker, #800000, #ff0000, #ffff00, #00ff00, #008000, #00ffff, #0000ff, #ff00ff, #800080"
buttons = "pencil, line, arrow, selection, rectangle, circle, marker, text, counter, pixelate, invert, move, undo, redo, copy, save, exit, pin"
button-size = 0                 # 0 = system font line height x 2.2
show-help = true
show-side-panel-button = true
show-size-badge = true
magnifier = false
square-magnifier = false
copy-on-double-click = false
return = "copy"                 # copy | save | pin — what Accept/Return does
save-path = "/private/tmp"
save-path-fixed = false
filename-pattern = "%F_%H-%M"
save-format = "png"             # png | jpg
jpeg-quality = 75
save-after-copy = false
copy-path-after-save = false
save-last-region = false
undo-limit = 100
arrow-style = 0                 # 0 filled head, 1 open head
reverse-arrow = false
counter-outline = true
insecure-pixelate = false
font = ""                       # text tool font ("" = system)
copy-toast = "Capture saved to clipboard"
save-toast = "Capture saved as {}"
help-mouse = "Select screenshot area"
# … one help-* key per help-card row (rule 3: user-facing strings in config)
```

Add numeric keys to `configNumberKeys` with ranges (`contrast-opacity`
0–255, `jpeg-quality` 1–100, `undo-limit` 1–1000, `button-size` 0 or 20–80).
`enabled = false` → no hotkey action (log only), no palette entry.

#### 7.3.6 Test hooks (required: "No source-grep tests")

- Socket `state` → `screenshot`: `{shown, displays:[{id, frame, scale, key}],
  selection:{display, x,y,w,h}|null, tool, size, color, objects:[{type,
  bbox}], buttons:[{name, x,y,w,h}], sidePanel, helpShown, activations,
  frontmostPid}`.
- `do:screenshot:show[:delay]`, `do:screenshot:select:X,Y,W,H`,
  `do:screenshot:tool:NAME`, `do:screenshot:draw:X1,Y1,X2,Y2`,
  `do:screenshot:key:SPEC`, `do:screenshot:copy|save:PATH|pin|close`.
- `bin/run-tests.sh screenshot` → `Tests/test_screenshot.swift` (with a
  `// sources:` header like `test_recent_files.swift`) for the pure parts:
  button-ring layout for selections at every screen edge, the small-selection
  fallback and the all-blocked fallback; counter renumbering after delete and
  undo; undo/redo stack limits; Shift-snapping angles; secure pixelate not
  containing any interior pixel color (feed a unique-color interior); filename
  pattern + clash suffix; output size = rect × scale.
- `bin/ui-test-focus.py tools` gains a `screenshot` case: open it with
  another app frontmost and check that `activations` is unchanged, the
  frontmost app is not us, the shared window is not moved or raised, and
  AeroSpace does not list the overlay.

### 7.4 CLI (mirrors `flameshot` so scripts carry over)

```
kitchen-sink screenshot [gui] [-p PATH] [-c] [-d MS] [--region WxH+X+Y|screen0]
                               [--last-region] [-s|--accept-on-select] [--pin]
                               [-r|--raw] [-g|--print-geometry]
kitchen-sink screenshot full   [-p PATH] [-c] [-d MS] [-r]      # all displays, stitched, no UI
kitchen-sink screenshot screen [-n N] [-p PATH] [-c] [-d MS] [-r] [--pin]   # one display (default: under the mouse)
```

- `-p` saves to PATH (a directory → pattern name), `-c` copies. With neither,
  `gui` uses `return`. `full` / `screen` copy by default.
- `-s` accepts as soon as the mouse is released (no annotation).
- `-r` writes the PNG to stdout. `-g` prints `W H X Y`. Both need a
  reply from the daemon, so use the socket's request-reply path (the `state`
  / `do:` branch already answers on the same connection). Allow a long
  timeout for interactive `gui`. The client must wait until the user
  finishes.
- `--region` coordinates are in points of the global desktop (top-left
  origin, like Flameshot), or `screenN`.

### 7.5 Keyboard map (capture mode; Flameshot Ctrl → Cmd on macOS)

| Key | Action |
|---|---|
| `P` `D` `A` `S` `R` `C` `M` `T` `B` `I` | Pencil, Line, Arrow, Rect outline, Rect filled, Circle, Marker, Text, Pixelate, Invert. Pressing the active tool's key again deselects it. |
| `G` | Grab color from the screen (eyedropper with magnifier, click to pick, Esc cancels and restores the color) |
| `Space` | Toggle side panel |
| `←↑→↓` | Move selection 1 pt |
| `Shift+←↑→↓` | Resize selection 1 pt (move the right/bottom edge) |
| `Cmd+Shift+←↑→↓` | Symmetric resize 1 pt from the center |
| `Cmd+A` | Select the whole display under the mouse |
| `Cmd+M` | Move-selection mode |
| `Cmd+Z` / `Cmd+Shift+Z` | Undo / redo |
| `Cmd+C`, `Ctrl+C` | Copy and close |
| `Cmd+S` | Save and close |
| `Return` | Accept (= `return` config, default copy) |
| `Cmd+Return` | Commit the current tool (e.g. finish text) |
| `Backspace` / `Delete` | Delete the selected object |
| `Cmd+Q`, `Esc` (last in chain) | Exit capture |
| `Cmd+/` | Show the shortcuts card (the app-wide convention), drawn on the overlay |
| Mouse wheel | Tool size ±1 (`Cmd`+wheel: finer, tool-specific, e.g. counter number) |
| Right-click | Color wheel |
| `Shift` while drawing | Snap angles / square aspect |
| `Shift` + drag a handle | Mirror resize (both sides move) |
| `Cmd` + drag a handle | Keep aspect ratio |
| Double-click in selection | Copy (only when `copy-on-double-click = true`) |

Text box (while editing): normal macOS editing plus rule 1: Cmd+C/V/X/A/Z
and Ctrl+C/V. Esc commits (Flameshot behavior). `Cmd+Return` commits.

### 7.6 Assumptions (to check during the build)

| # | Assumption | How to check |
|---|---|---|
| A1 | `SCScreenshotManager` capture of 2 Retina displays fits ≤ 120 ms on the owner's Mac. | Log timing on the first build. If it's slower, prewarm `SCShareableContent` and capture displays in parallel. |
| A2 | A `.nonactivatingPanel` at `.screenSaver` level can become key and get key events without activating the app (as `/filefast` does at a lower level). | First spike: show one panel, type, check that `NSWorkspace.frontmostApplication` is unchanged. |
| A3 | The Screen Recording grant survives rebuilds because the signing cert is stable. macOS 26 may still re-prompt for persistent screen access every so often, as Sequoia did. | Rebuild twice and capture. Document any re-prompt in `AGENT_CONTEXT.md`. |
| A4 | Return = copy is what the owner wants (Flameshot's Accept with no `-c`/`-p` behaves this way on 12.1, but upstream docs were inconsistent). | Ask the owner during review. It's one config key (`return`). |
| A5 | The NSSavePanel can be shown from a non-activating context without raising the shared window. | Spike. If not, save with `save-path-fixed` behavior and show a "Saved to …" toast with a Reveal action instead. |
| A6 | Hyper+X isn't used by another app on this Mac. | `grep alt-cmd-ctrl-shift config/aerospace/aerospace.toml` (only s/n/t today). |
| A7 | SF Symbols are close enough to Flameshot's Material icons. | Side-by-side check with the owner. |

## 8. Release

Relative sizes, no dates. Each phase ends with a build, `bin/run-tests.sh
screenshot`, and the owner trying it.

| Phase | Contents | Size |
|---|---|---|
| **0. Spikes** | A2, A3, A5 above. Permission flow + preflight row. | small |
| **1. Capture core** | Hotkey, CLI/socket, capture, overlays on all displays, dim, help card, selection + handles + keyboard nudge, size badge, Copy / Save / Exit / Esc chain, toast, `/paths` hook, `[screenshot]` config, `AGENT_CONTEXT.md` section. **The owner can already replace ⇧⌘4.** | medium |
| **2. Annotate** | Button ring (layout port + animation), all 11 drawing tools, undo/redo, wheel size + indicator, right-click color wheel, Shift snapping, object select / move / delete. | large |
| **3. Parity polish** | Side panel (size, color, grab color, hex, grid, layers), text config widget, circle counter tail + renumbering, pin window, magnifier, `full` / `screen` / `--raw` / `--print-geometry` / `--last-region` / `-s`, focus test case. | medium |
| **Later (not v1)** | Monitor picker (v14 style), selections that span displays, screenshot history window, upload targets (e.g. attach to a Jira issue via the existing Jira client), OCR ("copy text from region" with Vision), scrolling capture, window-snap selection (hover a window to select its frame). | — |

**Out of scope for v1:** Imgur/any upload, "open with" app launcher, tray
icon, Flameshot's launcher dialog, translations.

## 9. Definition of done (acceptance checks)

1. `./ws build` builds with no new warnings. `bin/run-tests.sh screenshot`
   passes.
2. Hyper+X from any app shows the frozen, dimmed screen and the help card in
   ≤ 400 ms (the log line proves it). The frontmost app does not change.
3. Drag → the ring appears in the default order of §7.2.2. A selection
   touching each screen edge in turn still shows every button. A full-screen
   selection puts the buttons inside.
4. Each tool letter selects its tool. Each tool draws as described in
   §7.2.3. Undo/redo walk through every action, including deletes, moves and
   color changes.
5. Cmd+C, then paste into TextEdit (and Webex) shows the selected area at
   Retina resolution with the annotations.
6. Cmd+S saves using the pattern. The file appears at the top of `/paths`.
7. Pin shows a floating image that AeroSpace doesn't tile. 0–9, wheel zoom,
   the right-click menu and double-click close all work.
8. Esc unwinds one layer at a time (rule 6). Cmd+Q never quits the daemon.
9. With Screen Recording revoked: a toast, Settings opens, no overlay is
   shown. `bin/preflight.sh` reports the warning.
10. `bin/ui-test-focus.py tools` passes, including the new screenshot case.
11. `AGENT_CONTEXT.md` documents the feature (section + code-map rows).
    `[shortcuts]` lists the keys.

## 10. Research notes and references

- **Local app studied:** `/Applications/flameshot.app`, `Flameshot v12.1.0
  (96c2c82e)`, Qt 5.15.5. The help card text, the button ring order and the
  size badge in §7.1 were observed by running `flameshot gui` on the owner's
  Mac. The owner's settings are in `~/.config/flameshot/flameshot.ini`. To
  compare side by side, run
  `/Applications/flameshot.app/Contents/MacOS/flameshot gui`. **It takes over
  every screen**, so only do it when the owner says it's OK.
- **Homebrew:** cask `flameshot` 14.0.0, disabled 2026-09-01 (fails
  Gatekeeper).
- **Upstream source (v14, `master`)**, the files the spec is ported from:
  - `src/utils/confighandler.cpp`: every option and default, every shortcut.
  - `src/widgets/capture/capturewidget.cpp`: overlay, help, Esc chain, wheel,
    undo, object selection.
  - `src/widgets/capture/buttonhandler.cpp`: ring placement.
  - `src/widgets/capture/capturetoolbutton.cpp`: button list, order,
    80 ms emerge animation.
  - `src/widgets/capture/selectionwidget.cpp`: handles (60% of button size,
    circles), 1 px border, cursors.
  - `src/utils/globalvalues.cpp`: `buttonBaseSize = lineSpacing × 2.2`.
  - `src/widgets/panel/sidepanelwidget.cpp`: side panel controls.
  - `src/tools/*`: per-tool behavior (`text/texttool.cpp`,
    `circlecount/circlecounttool.cpp`, `pixelate/pixelatetool.cpp`,
    `abstracttwopointtool.cpp`, `pin/pinwidget.cpp`).
  - Repo: https://github.com/flameshot-org/flameshot · Docs:
    https://flameshot.org/docs/guide/key-bindings/ · Releases:
    https://github.com/flameshot-org/flameshot/releases
- **Version differences that matter:** v13 = Qt 6 port, Imgur off by default.
  v14 = asks which monitor to capture, adds the arrow style selector and the
  counter number on the wheel, plus hex color input (all included above
  except the monitor picker). v15 RC = text tool fixes and the counter
  outline toggle (included as `counter-outline`).
