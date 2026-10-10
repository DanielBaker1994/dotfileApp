# Backlog

Open work: bugs, features, and things to investigate. Newest first. Each
entry says what's wrong or wanted, how to reproduce it or where it lives, and
what a fix probably looks like.

---

## Build & tooling

### Every build recompiles the whole app — make it incremental

**Symptom.** A one-line change rebuilds everything and takes minutes.
`bin/build-app.sh`'s `compile()` (:192) runs one whole-module
`swiftc -O -swift-version 5 … "${SOURCES[@]}"` over every top-level
`*.swift` in `$SWIFT_SOURCES_GLOB` — no object files, no incremental state.
So editing `kitchen_sink.swift` (~553 KB), `PopupWindow.swift` (~11.7k
lines) or `CompareWindow.swift` (~147 KB) recompiles all of them plus every
other source. (SwiftTerm is already fine: `build_term_lib()` precompiles it
once into `.build/SwiftTerm/libSwiftTerm.a` and only rebuilds when its
sources or target change.)

**Investigate.**

1. **Per-file incremental `swiftc`.** Compile each source to
   `.build/obj/NAME.o` with `swiftc -c`, rebuild only the `.o` whose source
   is newer, then link the `.o` set once. Keep the Info.plist embedding
   (`-Xlinker -sectcreate … __info_plist`) on the link step. This is the
   smallest change that gets most of the win.
2. **SwiftPM.** A `Package.swift` target gives incremental builds plus a
   standard `swift build` / `swift run` / `swift test` surface for free.
   Bigger lift: the build currently depends on a single-invocation model, a
   static SwiftTerm lib, embedded plist and copied resources — all
   expressible in SwiftPM but real work to port.
3. **Split the mega-files** so an incremental build touches less and the
   compiler has smaller units (this also helps editor/agent context).
4. Look at `-enable-batch-mode` / build records and whether a ccache-style
   cache is worth it for Swift.

### An obvious build entry point

**Done.** `./ws build` is the one obvious entry point. `./ws help` lists every
command. The old shim scripts and a Makefile that once multiplied entry points
have been removed.

---

## Bugs

### Focus ring doesn't resize when the notes sidebar collapses to the rail

**Symptom.** With notes focused, the gray pane ring correctly outlines the
editor. Press `Ctrl+B` then `B` (sidebar ⇄ icon rail) and the editor widens,
but the ring keeps the old, pre-toggle width — it no longer hugs the pane.

**Repro.**

1. Open the notes view and click the editor so the focus ring is visible.
2. `Ctrl+B` `B` (or `Cmd+\`) to collapse the sidebar to the icon rail.
3. The editor grows; the ring stays where it was.

**Where.** `PaneNav.swift` — `refresh(_:)` (:231) positions the ring from
`cur.windowRect()`. It is re-run from a local `.keyDown` / `.leftMouseUp`
monitor and from window key/resize notifications (`track(_:)`, :189).
`PopupTabsBar.collapsed` (`PopupWindow.swift`, :1540) changes the sidebar
width and marks `superview.needsLayout`, but the window frame does not
change, so no `didResize` fires. The key monitor's `refreshSoon` runs on the
next async turn — before AppKit has re-laid out the panes — so
`windowRect()` is still the old size.

**Fix sketch.** Re-run `PaneNav.shared.refreshSoon(w)` *after* the layout
pass when the rail toggles: from `PopupTabsBar`'s `onCollapse`/`collapsed`
didSet, or in a `layout()` hook that fires once the sidebar width has been
applied. A generic "content-view subview frames changed" trigger would cover
other width changes too (drag-resize, sidebar width edits).

### Focused PDF window gets no border highlight

**Symptom.** Focusing a PDF window with the AeroSpace focus shortcut
(alt-h/j/k/l) gives it no highlight border, so it's impossible to see that
it's focused. Other windows get the border fine.

**Where.** The highlight is JankyBorders (`borders`), configured by
`config/borders/bordersrc` (symlinked to `~/.config/borders/bordersrc`) and
run as the brew service `borders`; `bin/preflight.sh` and
`jira/jira-doctor.sh` check it's running. It borders the focused window via
the accessibility API, so a window it can't read, or one it doesn't track as
focused, gets nothing.

**Investigate.** Why this one window is skipped:

- Does the PDF window expose a normal AX window (title bar / standard
  level)? JankyBorders can miss windows without a standard title bar, at a
  non-normal window level, or that are panels/sheets.
- Does AeroSpace's focus change post the AX focus event JankyBorders listens
  for? If AeroSpace focuses the window through its own IPC without the app
  becoming frontmost in the usual way, borders' active-window tracking may
  not update.
- Is the PDF opened in Preview, a Quick Look panel, or a kitchen-sink
  floating panel (`ProseWindow` / `QLPreviewPanel`)? A `.nonactivatingPanel`
  is invisible to AeroSpace and may be skipped by borders too.
- Check `bordersrc` options (`hidpi=on`, `style=round`) against that window
  type.

---

## Features & ideas

### Per-language code coloring in the markdown views

**Wanted.** Fenced code blocks render, but every language is colored the
same — Python, Swift, JSON all look alike. It should read differently per
language.

**Where.** Prose reading view + Export PDF go through pandoc
`--syntax-highlighting=\(highlight)` (`pylib/prose_pdf.py`, default `tango`,
`[notes] pdf-highlight`). pandoc/skylighting does emit per-language classes
(`pre.sourceCode.LANG`), and the dotfiles `friendly_document_styling.css`
styles code, so the shared look is most likely the one theme mapping the same
token categories to the same colors across languages — or the CSS overriding
the skylighting span colors with a single color.

**Investigate.** Confirm whether skylighting is actually tokenizing per
language (inspect the emitted HTML for `kw` / `st` / `co` spans per
language) and whether `pdf-css` overrides them. Then decide: a theme with
more distinct token colors, per-language CSS, or accept the single palette.

### Copy-to-clipboard buttons for code blocks in Prose / PDF

**Wanted.** A copy-to-clipboard affordance next to code items — e.g. the
`## Log` block with a fenced `log` sample should show a copy button to its
left, the way the dotfiles markdown generator already does it.

**Where the dotfiles do it.** `~/.dotfiles/markdown_generator/copy_button.js`
wraps every `<pre>` in a `div.codeblock` and inserts a `.copy-btn` to its
left; `copy_button.css` lays it out (flex, button on the left, 40px wide,
hover/copied states). The `friendly_document_styling.css` language icons
(`pre.sourceCode.LANG::before`) are the top-left glyphs already used by the
Prose view/PDF.

**Investigate.** `ProseRender` (`NotesProse.swift`) builds the reading view's
HTML and `prose_pdf.pandoc_args` (`pylib/prose_pdf.py`) builds the exported
document — neither injects copy buttons today.

- In the **reading view** (a `WKWebView`) a button with `navigator.clipboard`
  works, so port `copy_button.js`/`copy_button.css` into the Prose HTML the
  same way the dotfiles do. This is the part that can actually copy.
- For the **exported PDF**, weasyprint runs no JS, so a live button can't
  copy. Decide whether to (a) render the button as a static, clearly
  non-functional marker, (b) omit it from the PDF and keep it reading-view
  only, or (c) explore a PDF JavaScript action (unreliable across viewers) —
  likely (b) is right, with the button injected only for the screen render.
- Keep it config-driven: a `[notes]` key to turn the buttons on/off and to
  point at the dotfiles' CSS/JS if we want to reuse them verbatim.
