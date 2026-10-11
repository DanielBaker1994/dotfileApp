# FEATURES.md — wanted features, written for a future agent

Each entry says what the owner wants, why, and where the code is. Pick one up,
build it on `main`, verify it in the live app (`rule.md`, `AGENTS.md`), then
move it to "Done" with the commit hash.

## 1. Hyper+X header: one pill with three stops

**Owner request (Oct 11 2026).** Hyper+X should show, at the top of the
screenshot overlay, the capture modes **Copy Text** and **Screenshot** plus
**Recent screenshots**. Recent should be part of the same pill, tacked on at the
right as a smaller item with the same clock icon it has today. It signals "related
to capture, but not a capture mode itself". All three must be reachable by
tapping/clicking or with `Tab`, so everything is one keystroke away after Hyper+X.

Mockup (interactive, light/dark): https://claude.ai/artifact/4JiXjDYiftksyjkueeUxZq
(owner-private link; the HTML source is not committed, the spec below is the
source of truth).

**Spec**

- Layout: `[ Copy Text | Screenshot | hairline | (clock) ]`. The chip is a 26 pt
  circle with a 14 pt glyph, inside the pill's right padding, behind a 1 pt
  divider (white at 22 %). The pill grows by about 40 pt. Today the recents
  button is a separate 28 pt disc 12 pt to the right of the pill.
- Three focus stops. `Tab` advances Copy Text → Screenshot → Recent → wrap;
  `Shift+Tab` goes back. Clicking a stop is the same as reaching it with Tab.
- Reaching Recent opens the recents grid (`toggle_recent`); leaving it (Tab, Esc,
  or picking a cell) closes the grid and returns to the previous mode. `⌘R` stays
  as the direct shortcut.
- The overlay opens on the mode from `[screenshot] start-mode` in
  `commands.toml` (the owner has `start-mode = "text"`).
- Active colours: Copy Text = `shotTextBlue` `#0A84FF`, Screenshot = `ui-color`,
  Recent active = white chip with dark glyph.
- Config over code: sizes and the chip's visibility belong in `[screenshot]`.

**Where**

- Painting and layout: `draw_mode_chrome` in
  `rust/ws-rs/src/views/screenshot.rs` (the `overlay` module). Hit testing:
  `ShotSession::mouse_down` / `mouse_up` with `chrome_rects` / `ChromePress`.
- Today `Tab` goes through `ShotSession::handle_key` → `toggle_text_mode`
  (two states only). Add a third state (a `ShotStop` enum) instead of overloading
  `text_mode`.
- Swift reference: `ShotModePill`, `ShotRecentButton` in
  `ScreenshotOverlay.swift` (`git archive 608d07d`).

## 2. Screenshot overlay: parity with the Swift tool (Flameshot-style)

The Swift screenshot tool is a Flameshot-style region editor. The Rust overlay
paints its chrome into one CoreGraphics context instead of using AppKit
subviews, so these Swift pieces are still missing or only partly there. Verify
each against the Swift behaviour before calling it done.

Done on Oct 11 2026 (live-verified): mode pill + recents button drawn and
clickable; pointer tracking (loupe, pill follow the mouse); mouse-driven finish
(drag-select in Copy Text mode used to do nothing; `mouse_dragged` dropped the
drag state); Copy Text writes OCR text to the clipboard; copy/save/text toasts
now show.

Still open (not yet audited live unless noted):

- **Ring buttons have no glyphs.** `draw_button_ring` paints plain discs; the
  Swift `ShotButton` draws SF Symbols, a hover state and a tooltip per tool.
  This is the most visible gap: you cannot tell which disc is Copy, Save, Pin…
- **Tool tab + side panel.** `ShotToolTab` (left-edge "Tool Settings" tab) and
  `ShotSidePanel` (colour, size, text style, grid, history…) are model-only.
- **`ShotSizeIndicator`** (the live size badge in the corner), **`ShotHSVWheel`**
  (custom colour), **`ShotShortcutsCard`** (`⌘/`) visual.
- **Text objects.** `ShotTextField` (typing in place with an `NSTextView`); the
  Rust overlay only paints the draft.
- **Save card.** `ShotSaveCard` paints, but its path field is not editable.
- **Cursor updates** (`updateCursor`: crosshair / resize / move cursors) and
  making the overlay under the pointer key on `mouseMoved`.
- **Per-tool rendering check:** pencil, line, arrow, selection, rectangle,
  circle, marker, text, counter, pixelate, invert, move, undo / redo — draw one
  of each and compare to the Swift output (`samples/parity`).
- **Pin window extras** and the `copy-text` ring button path.

## Done

- Hyper+X overlay: mode pill + recents button drawn, click targets, pointer
  tracking, Copy Text clipboard — Oct 11 2026 (see the git log for the commit
  "Screenshot overlay: mode pill + recents button …").
