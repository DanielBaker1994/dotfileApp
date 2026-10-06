---
name: kitchen-sink
description: A keyboard-summoned card dressed in your terminal theme — one window, many tools, layered like a tmux status bar.
colors:
  base: "#24273A"
  mantle: "#1C1E2D"
  crust: "#151722"
  header: "#181926"
  surface0: "#33374B"
  surface1: "#3F4358"
  highlight: "#3F4A5A"
  text: "#CAD3F5"
  dim: "#939AB7"
  accent: "#C6A0F6"
  accent2: "#8AADF4"
  success: "#A6DA95"
  warning: "#EED49F"
  danger: "#ED8796"
  info: "#8BD5CA"
  outline: "#7D6AA1"
  browser-pane: "#4F5A6E8C"
  terminal-pane: "#4F5A6EC7"
typography:
  editor:
    fontFamily: "SF Mono, Menlo, monospace"
    fontSize: "13pt"
    fontWeight: 400
  toast:
    fontFamily: "SF Pro Text, system-ui"
    fontSize: "12.5pt"
    fontWeight: 500
  input:
    fontFamily: "SF Pro Text, system-ui"
    fontSize: "12pt"
    fontWeight: 400
  row:
    fontFamily: "SF Pro Text, system-ui"
    fontSize: "11pt"
    fontWeight: 400
  button:
    fontFamily: "SF Pro Text, system-ui"
    fontSize: "10.5pt"
    fontWeight: 500
  button-on:
    fontFamily: "SF Pro Text, system-ui"
    fontSize: "10.5pt"
    fontWeight: 600
  meta:
    fontFamily: "SF Pro Text, system-ui"
    fontSize: "9pt"
    fontWeight: 400
rounded:
  button: "4px"
  focus-ring: "5px"
  field: "6px"
  terminal: "8px"
  card: "9px"
  picker: "10px"
  overlay: "12px"
  pill: "999px"
spacing:
  hairline: "1px"
  edge: "3px"
  row-inset: "4px"
  padding: "8px"
  nav-icon: "26px"
  header: "30px"
  row: "30px"
components:
  card:
    backgroundColor: "{colors.base}"
    textColor: "{colors.text}"
    rounded: "{rounded.card}"
  header:
    backgroundColor: "{colors.header}"
    textColor: "{colors.dim}"
    height: "{spacing.header}"
  tab-strip:
    backgroundColor: "{colors.mantle}"
    textColor: "{colors.dim}"
  tab-active:
    backgroundColor: "{colors.accent}"
    textColor: "{colors.crust}"
    typography: "{typography.button-on}"
    rounded: "{rounded.button}"
  button-ghost:
    backgroundColor: "{colors.surface0}"
    textColor: "{colors.dim}"
    typography: "{typography.button}"
    rounded: "{rounded.button}"
  button-on:
    backgroundColor: "{colors.surface1}"
    textColor: "{colors.accent}"
    typography: "{typography.button-on}"
    rounded: "{rounded.button}"
  input-well:
    backgroundColor: "{colors.mantle}"
    textColor: "{colors.text}"
    typography: "{typography.input}"
    rounded: "{rounded.field}"
  row:
    textColor: "{colors.text}"
    typography: "{typography.row}"
    height: "{spacing.row}"
  row-cursor:
    backgroundColor: "{colors.highlight}"
    textColor: "{colors.text}"
    rounded: "{rounded.button}"
    height: "{spacing.row}"
  toast:
    backgroundColor: "{colors.crust}"
    textColor: "{colors.text}"
    typography: "{typography.toast}"
    rounded: "{rounded.pill}"
---

# Design System: kitchen-sink

## Overview

**Creative North Star: "The Terminal Card"**

kitchen-sink is a floating card that wears your terminal theme.
Surfaces are stacked like a tmux or Catppuccin port, deepest first: the
header strip is the darkest band (crust), toolbars, tab strips and input
wells sit one step in (mantle), the card is the base, and raised controls
float just above it (surface). Nothing uses a system colour. Every pixel
comes from the active theme preset, so switching from Macchiato to Tokyo
Night or Paper recolours every view, the embedded shell's ANSI palette and
the nvim pane together.

The look is dense and quiet. Text is small (10.5–13 pt), rows are 30 pt,
and chrome is thin hairlines rather than boxes. Colour carries meaning, not
decoration. The one accent marks what is *selected or active*: the active
tab, the cursor row's edge, an "on" toggle, a focus ring. The five palette
hues (accent2, success, warning, danger, info) mark *state*: links and fuzzy
matches, Jira statuses, poll health, the ✕ hover. The card floats over a
blurred desktop, so the system is translucent at the edges and solid where
you read.

**Key Characteristics:**
- Depth by tone, not shadow: crust < mantle < base < surface.
- One accent marks selection; palette hues mark status.
- Every colour is derived from the theme, with contrast lifted in code
  (`readable`, `accentOn`, `onAccent`) instead of hand-picked per theme.
- Small, dense, system-font UI; monospace only where text is edited.
- Thin hairlines (text at 8–12% opacity) separate regions; boxes are rare.

## Colors

The default is Catppuccin Macchiato (`commands.toml [theme]`); 29 built-in
presets plus `[themes]` entries swap the whole set live. The values in the
frontmatter are the default theme. `mantle`, `crust`, `surface0`,
`surface1` and `outline` are **derived** from base, text and accent in
`extension PopupColors`. Never hard-code them.

### Primary
- **Lavender Signal** (accent): the active tab's solid pill, the 3 pt
  cursor edge on selected rows, "on" button text and tint, focus rings,
  header-style accents. `accentOn` lifts it toward text until it reaches
  2.2:1 on the card.

### Secondary
- **Periwinkle Link** (accent2): keys, links, fuzzy-match highlights, folder
  marks, and the second stop in the Stripe and Aurora header gradients.

### Tertiary
- **Status hues**: Sage Success, Wheat Warning, Coral Danger, Seafoam Info.
  These cover Jira statuses and priorities, tab freshness badges (green
  fresh, yellow stale, red stale and failed), the ✕ hover, the toast's
  check glyph and the terminal's ANSI palette. They are always passed
  through `readable()` (3:1 on the card) before drawing.

### Neutral
- **Macchiato Base** (base): the card fill under the blur (`tintAlpha`
  0.78).
- **Mantle Well** (mantle): tab strips, toolbars, table headers, recessed
  text inputs.
- **Crust Band** (crust / header): the drag header, status line, toast
  pill, and the text drawn on the accent pill.
- **Surface 0 / 1**: base blended 9% / 16% toward text. Raised ghost buttons
  and hover fills.
- **Highlight Slate** (highlight): the selected-row pill and the text
  selection colour (lifted to 1.7:1 against the card if needed).
- **Moonlit Text / Dusk Dim**: primary and secondary text. Idle button
  labels are dim; hover brightens them to text at 92%.
- **Accent Outline** (outline): the card's border, the accent sunk 45% into
  the base at 85% opacity, so every theme frames itself in its own hue.
- **Browser / Terminal panes**: the file browser and terminal drawer fills
  (translucent slate, `[theme] browser` / `terminal`).

### Named Rules
**The Theme-Only Rule.** No `NSColor.systemX`, `controlAccentColor` or
default selection colours. Draw with `PopupColors` tokens or a `tone(_:)`,
because the machine's accent colour once made selections unreadable.

**The One Accent Rule.** The accent means "this is the selected or active
one". Status never borrows it; selection never borrows a status hue.

**The Contrast-in-Code Rule.** A new themed colour goes through
`readable`, `accentOn` or `onAccent`. Text drawn on a tint goes through
`ensure(fg, on: over(tint, alpha, on: base))`, which measures it against
the opaque colour it actually sits on: 4.5:1 for text, 3:1 for marks. It
must survive all 29 presets, including Paper and Solarized Light.

## Typography

**Body Font:** SF Pro (the system font) for all chrome, lists and labels.
**Mono Font:** SF Mono by default, or the user's `font` per section, for
the notes editor, Compare panes, previews and AI output. The terminal
drawer uses `terminal-font` (a Nerd Font for glyphs).

**Character:** quiet system UI with a monospace work surface. The chrome
gets out of the way so the text you edit stands out.

### Hierarchy
- **Editor** (400, 13 pt, mono): the notes, Compare and AI panes. Each is
  user-sized (`font-size`, Cmd+Opt+±).
- **Toast** (500, 12.5 pt): the bottom-centre confirmation pill.
- **Input** (400, 12 pt): filter bars and search fields.
- **Row** (400, 11 pt): list, table and file rows.
- **Button / Tab** (500, 10.5 pt; 600 when on): chips, tabs and header
  segments. Weight, not size, marks the active one.
- **Meta** (400, 9 pt, dim): header item counts and last-write times.

### Named Rules
**The Zoom Rule.** Every size multiplies by `zoom` (Cmd+±). Never place a
fixed point size in chrome without `* zoom`.

**The Weight-Marks-State Rule.** Active means semibold plus accent, never
a bigger size. Sizes stay on the scale above.

## Layout

A card of a fixed, shared frame (`PopupWindow.baseFrame`). Every view of
the shared window uses the same rect, so switching views never resizes the
window; only the notes drawers grow it. From top to bottom: a 30 pt drag
header (✕, kitchen-sink icon, view-switcher icons at 26 pt with 2 pt gaps,
a centred title, and right-hand buttons), then a mantle tab strip or
toolbar, then the content. Content is lists of 30 pt rows inset 4 × 1 pt,
master–detail splits (Jira Config, AI, Compare), or a filter box over a
list (the palette, /paths). Window padding is 8 pt. Density is high on
purpose: Jira tabs hold 5k–20k rows.

Tool panels (/screenshot, /paths, /filefast) are separate borderless cards
placed on the mouse's screen, top at 20%. Overlays inside a window (the
Cmd+/ shortcuts card, the Cmd+K action picker, sheets) are centred cards
above a dimmed backdrop.

## Elevation & Depth

Inside the card the system is **tonal**: depth is the crust → mantle → base
→ surface ladder, and inputs are *recessed* (mantle well with a hairline)
while buttons are *raised* (a ghost fill of the text colour). Real shadows
appear only on things that float above the card.

### Shadow Vocabulary
- **Window** (the system window shadow, `hasShadow`): the card itself over
  the desktop.
- **Toast** (`0 -2 8 rgba(0,0,0,0.25)`): the confirmation pill.
- **Action picker** (`radius 14, opacity 0.35`): the Cmd+K list.
- **Overlay card** (`radius 18, opacity 0.40`): the shortcuts card and
  in-window overlays.

### Named Rules
**The Recessed-Field Rule.** Text inputs sink (mantle with a hairline) and
buttons rise (a ghost fill). A field never looks like a button. Focus swaps
the hairline for the accent.

## Shapes

Gently rounded and never pill-shaped, except for the toast. The card has a
9 pt corner, and `_cornerRadius` overrides keep macOS 26's 16 pt frame from
showing around it. Chips, tabs, row pills and buttons use 4 pt. Fields and
image wells use 6 pt, the terminal drawer 8 pt, and floating overlays
10–12 pt, so bigger floats get rounder corners. The cursor mark is a 3 pt
accent edge clipped inside the row pill's left curve. Lines are 1 pt
hairlines; the only 2 pt lines are the active-chip indicator and the Accent
Edge header.

## Components

### Buttons
- **Shape:** squared-off chips (4 pt).
- **Ghost (idle):** text at 6% fill, dim label, 500 weight. Hover 11%,
  pressed 16%, label brightens. No stroke.
- **On:** accent-tinted fill (22% dark, 18% light), accent label at 4:1,
  600 weight. On-hover deepens to 30% / 25%.
- **AppKit forms:** `ThemedPushButton` (`role` .primary / .danger) and
  `ThemedPopUpButton`, never stock bezels.

### Tabs
- **Strip:** mantle band under the header.
- **Active:** a SOLID accent pill with `onAccent` text (crust when it
  contrasts, else black or white). It is the loudest element on screen.
- **Badges:** a 7 pt freshness dot plus age text inside the pill.

### Inputs / Fields
- **Style:** a recessed mantle well (60% dark, 55% light) with a hairline
  (text at 12%), 6 pt corners.
- **Focus:** the hairline becomes the accent. The file browser's active
  part gets a 2 pt accent `partRing`.
- **Selection:** pinned to the theme highlight with a contrast-picked
  foreground.

### Rows (list, file list, table, switcher)
- **Cursor:** a highlight pill (4 pt) with a 3 pt accent edge on the left.
- **Multi-select:** the same pill without the edge on marked rows.
- **Hover:** a ghost hover fill.
- **Tables:** hairline row separators, mantle header, `jiraCellTone` cells.

### Header (signature)
- The crust band with ✕, the kitchen-sink app icon, view-switcher icons
  (the current one on an accent chip), dim 9 pt meta, and a centred title.
- **Header Style** (`[app] header-style`), app-wide, is built only from
  header, accent and accent2: Flat (hairline), Accent Edge (2 pt accent
  rule), Accent Stripe (3 pt accent → accent2 top ribbon), Tinted (20%
  accent wash), Glow (accent light falling from the top), Aurora (accent →
  accent2 → header, left to right).

### Toast
- A crust pill (94%) with a hairline (text at 10%), a soft shadow, a
  success SF Symbol and 12.5 pt medium text. Bottom centre, fades and rises
  in 0.18 s, leaves after 0.25 s, holds 1.4 s.

### Overlay Cards (shortcuts, questions)
- **Shortcuts card** (`ShortcutsOverlay`, Cmd+/): the same card in every view,
  popup and card window alike. Key caps in a mantle well with a hairline,
  the descriptions in a single column, group titles in the accent.
- **Confirm card** (`CardWindowController.confirm` / `prompt`): a question
  asked inside the window, never a system alert. 12 pt corners over a 30%
  black backdrop. The default button is the accent-filled one and carries
  the focus ring. A risky choice is a danger button, and Cancel is the
  default for anything that can trash or overwrite. An optional recessed
  input (`JiraInputBox`) sits under the text. A click outside does nothing.

### Theme Swatch (signature)
- Each Theme ▸ preset is drawn as a 58 × 20 miniature window: a crust
  header, the solid accent tab, a text line, a selected row with its edge,
  and four palette dots. Hovering a preset previews it live.

## Do's and Don'ts

### Do:
- **Do** draw with `PopupColors` tokens and `tone(_:)`, and push live
  changes through `PopupThemeable` / `pushColors()`.
- **Do** layer surfaces crust < mantle < base < surface0/1, and keep inputs
  recessed and buttons raised.
- **Do** mark the cursor row with the highlight pill plus a 3 pt accent
  edge, and the active tab with the solid accent pill.
- **Do** scale every chrome size by `zoom`.
- **Do** check a new colour against light presets (Paper, Latte, Solarized
  Light) as well as dark ones.

### Don't:
- **Don't** use system colours, the macOS accent colour, or AppKit default
  selection and bezel styles.
- **Don't** use the accent for status, or a status hue for selection.
- **Don't** add shadows to elements inside the card. Depth there is tonal.
- **Don't** resize the shared window when switching views.
- **Don't** ask with an `NSAlert` / `jiraFormSheet` in a card window. Use the
  confirm card, because a system sheet ignores the theme.
- **Don't** use capsule chips (the toast is the only pill) or corners above
  4 pt on in-card controls.
