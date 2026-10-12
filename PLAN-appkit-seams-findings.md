# PLAN — AppKit seams: WP9 scout findings

Produced 2026-10-12 by the six WP9 scouts (S1–S6); updated after executing
WP1–WP8 of `PLAN-appkit-seams.md`. Read-only reconnaissance; line numbers are
approximate (the tree moves).

## State (final, after WP1–WP8)

- Executed 2026-10-12; all of WP1–WP8 landed on `main`: `76d5edc` bell,
  `5f6c936` open_path, `2350616` clipboard, `31e0a39` frontmost-app,
  `ee9c1cc` screens, `6cac346` settings, `4238929` file-dialog, `8a375a1`
  main-thread dispatch (baseline `b62c2cd`).
- Final: `grep -rn "openURL\|NSBeep" rust/ws-rs/src` → only `platform/`
  (doc comments + the bell backend). `cargo build -p ws-rs` = 20 warnings
  (baseline, none new); `cargo test -p ws-rs` = 727 passed, 0 failed,
  4 ignored.
- `objc2_app_kit`-touching files: 55 = 50 app/UI + 5 `platform/` backends
  (mod, clipboard, app, screens, dialog). The metric **includes the seam
  backends by design** and did not drop: each migrated leaf call was a small
  slice of its file's AppKit use, so the honest progress signal is the
  per-API greps, not this count. `cfg(target_os)` forks: 20 in `platform/`
  of 451 in the crate (the rest are pre-existing).

## S1 — Images (`NSImage` 81/13 files, `NSBitmapImageRep` 17/3, `iconForFile` 3)

| Candidate → sites | Verdict | Proposed plain signature | Size |
|---|---|---|---|
| Screenshot byte transforms: `screenshot.rs` `rotate_png`:3478, `encode_png`:3509, `reencode_png_as_jpeg`:3522, pin `decode_png`:4531 | **YES** — bytes↔bytes, headless-testable | `platform::image::{encode_png(&[u8] RGBA,w,h), rotate_png(&[u8],deg), png_to_jpeg(&[u8],q), decode_png(&[u8]) -> Option<RgbaBuffer>}` | S–M |
| PNG→TIFF for pasteboard: `screenshot.rs` `png_to_tiff`:6926 (used by `write_image_pasteboard`:6874) | **YES** | `png_to_tiff(&[u8]) -> Option<Vec<u8>>` | S |
| Save/copy pasted image: `notes_vim.rs` `save_image`:294, `notes.rs` `copy_image_file_to_pasteboard`:2489 (entry `host.rs:5494`) | **PARTIAL** — input is `NSImage` | `write_png(path,&RgbaBuffer)` / `to_png(&RgbaBuffer)`; NSImage→RGBA extraction stays edge-side | S |
| Decode-for-display: `style.rs` `image_file`:425, `file_popup.rs`:194, `notes.rs`:2159/2374, `files_view.rs`:832, `screenshot.rs`:4984 | **NO** — AppKit decodes a superset (HEIC/PDF/SVG, profiles, retina reps, lazy) | — | L |
| Pasteboard image (`notes_vim.rs`:259) | NO — WP2 clipboard owns bytes; byte seam exists | — | S |
| Draw/display `NSImage` (`style.rs` `draw_image`, chrome/card/toast), system images (`symbol_image`/`named_image`/`glyph_icon`, `menu_kit` swatch) | NO — view/drawing layer (§0) | — | — |
| `NSWorkspace::iconForFile`: `files_view.rs`:144, `host.rs` `app_icon`:3369, `switcher.rs`:1525 | NO — LaunchServices UTI/badge/custom-icon composition | at best later `platform::icons::icon_rgba(path,size)` | L |
| "Resize" (`notes.rs`:2012, VimImageOverlay, `file_popup.rs`:240) | bounds math + `NSImageScaling` at draw; a resize seam would have **zero call sites** | — | — |

Notes: new dep would be the pure-Rust `image` crate (not present today).
`png_to_tiff` exists only to feed `NSPasteboardTypeTIFF` — it could move into
the clipboard seam instead of an image seam. Byte pipeline is already plain
Rust (`ShotImage.png: Vec<u8>`), so the natural boundary is `&[u8]`/RGBA; the
only AppKit-ish inputs are `NSImage` (paste) and `CGImage` (rendered shots).

## S2 — Colors/fonts as data (`NSColor` 66/23, `NSFont` 144/26)

| Item → sites | Verdict | Signature | Size |
|---|---|---|---|
| `Rgba`, `HexColor` (`theme.rs`:19,95; 29 files) | Already plain — no seam needed | keep | — |
| `Rgba::to_nscolor` (`theme.rs`:89) — 155 exprs / 33 files | Partial — conversion is already one fn, but sites live inside AppKit draw/build code (§0) | `platform::colors::as_nscolor(&Rgba)` | S |
| `PopupColors`/`ThemePreset`/`HeaderStyle` (`theme.rs`:150-828; 188 uses / 39 files) | Already plain | keep | — |
| Theme global state (`theme.rs`:345-377, installed `host.rs:~656`) | Partial — global, not passed | pass `&PopupColors` into view ctors | M |
| `NSFont::systemFontOfSize` (+bold) — 54 direct + ~100 via `style::system_font`:164 | **YES** | `FontSpec { family: Option<String>, size: f64, weight: Weight, mono: bool }` + `platform::fonts::resolve(&FontSpec)` | M |
| `monospacedSystemFontOfSize` (10 sites) / `fontWithName` (5: notes prose/vim, tools, terminal) | YES | `FontSpec::mono(...)`, `FontSpec::named(...)`, `is_fixed_pitch` | S |
| `Weight` enum (`ui/style.rs`:146, inside cfg-macOS `mod appkit`) | Un-gate; move to the seam | — | S |
| Font metrics: `style::text_size`/`truncate_to`:186-242, `jira_view.rs`:1019, `compare.rs`:5201, `ansi_render.rs`:671, `host_vim.rs`:74, … | Partial | `platform::text::{measure,line_height}` | L |
| Attr-dict builders ×4: `style::attrs`:178, `vim_keys.rs`:877, `ai.rs`:1796, `compare.rs`:5182 | YES (consolidate) | one `attrs(&FontSpec,&Rgba)` | S |
| `NSFontManager.convertFont_toHaveTrait` (`ansi_text.rs`:96) | Partial | `FontSpec::bold()` | S |
| SwiftTerm `set_colors(bg,fg,palette)` (`terminal.rs`:469, `notes.rs`:2993/3061/3887) | YES | shim takes `(&Rgba,&Rgba,&[Rgba;16])` | S |
| Dynamic system colors: `clearColor` ×19, `secondaryLabelColor`/`systemGreen`/`systemRed` (`jira_setup.rs`:323,438-440), `blackColor` ×2 | Partial — only ~6 truly dynamic | explicit tokens + `SystemColor::Clear` at the edge | S |

Key findings: **the pure palette already exists** (`Rgba`/`PopupColors`/
30 presets, born in the Swift port); only `to_nscolor` crosses into AppKit,
and most call sites already pass `Rgba` because `ui/style.rs`'s `mod appkit`
half (line ~117) converts inside `fill_rect`/`draw_text`/`attrs`/
`system_font`. The unmapped half is **fonts**: no `FontSpec`, `Weight` is
cfg-compiled out, fonts are built transiently (no `Retained<NSFont>` fields).
Two font systems must both be served by one `FontSpec`: AppKit `NSFont` and
CoreText `CTFont` (`engines/ansi_render.rs`, `screenshot.rs:3099`
`ShotTextStyle` — already a proto-FontSpec). `theme_state()` clones on every
`section_colors` read — a renderer would want the theme passed in.

## S3 — Input events (`NSEvent` 245/27)

A plain key type **exists but is split four ways**: `ui::popup::KeyInput`
(`ui/popup.rs:~143`) `{key_code, chars, cmd/ctrl/opt/shift, esc_streak}` with
pure `route_key` and the only `NSEvent→plain` adapter `key_input_from_event`
(~730); `engines/text_edit_keys::route(key_code, Modifiers, Focus)`;
`views::notes::Mods`; `views::ai::KeyInputLike` (its doc admits `KeyInput`
could be used directly). `KeyInput` lives in `ui/` yet pure engines import it
— dependency inversion; ideal home is `platform::input` with a widened
`KeyEvent { key_code, chars, chars_with_modifiers, modifiers, is_repeat }`
(`esc_streak` is caller state). ~200 of 245 hits are view-layer mouse/tracking
(§0 non-goal). In-scope: ~15 key sites, 12 static `mouseLocation()` calls,
`doubleClickInterval` (`host.rs:~2937`).

| File | Hits | Raw NSEvent reads? | Verdict |
|---|---|---|---|
| `app/host.rs` | 24 | YES — ~4113, ~4151, picker ~4187, sheet ~4199, timestamps ~4212/~4240, Ctrl+B prefix ~4226-36, notes-ts ~4339, ~4475 (~4490-95), ~8404 (~8408-56) | migrate (router internals) — S/M |
| `views/screenshot.rs` | 51 | YES — duplicate adapters ~1194/~1213, keyDown ~5101, scroll-cmd ~3804 | partial — delete dups, adopt KeyEvent |
| `views/file_popup.rs` | 15 | YES ~431-478 | keys yes; `sendEvent` drag pump no |
| `views/compare.rs` | 11 | YES ~5591-5601 | yes (builds text_edit_keys::Modifiers by hand) |
| `ui/sidebar.rs` | 11 | YES ~584-615 | yes — `nav_key`:96 is already pure |
| `ui/tree_view.rs` | 12 | YES ~346-393 | yes — same shape |
| `ui/popup.rs` | 12 | only the adapter | IS the seam — add `install_key_monitor(Fn(&KeyEvent)->bool)` |
| `ui/jira_setup.rs` | 6 | YES ~384-418 (own monitor) | yes |
| `views/files.rs` | 3 | YES ~1010-12 (QL delegate) | yes |
| `host/daemon_ui/host_vim.rs` | 1 | YES ~227 `characters()` | yes (`chars_with_modifiers`) |
| `ui/shortcuts.rs` | 3 | plain args already | adopt KeyEvent |
| `app/menu.rs` | 7 | reverse (Modifiers→NSEventModifierFlags) | leave (NSMenu layer) |
| `engines/text_edit_keys.rs` | 1 | comment only | unify `Modifiers` |
| mouse/view layer (14 files: files_view, jira_view, confluence_view, compare_view, terminal, card, notes, ai_view, prose_switch, list_panel, chrome, action_picker, note_find, switcher) | 90 | mouse fields only | no (§0) — except ~12 `mouseLocation()` statics |

Top candidates: (1) host.rs router internals — make `key_input_from_event`
the only conversion, pass `KeyEvent` down; blocker: widen the struct with
timestamp/repeat/modified chars first. (2) sidebar/tree_view `handle_nav_key`
wrappers over existing pure `nav_key`. (3) duplicate adapters
(screenshot/compare/file_popup/jira_setup). Surprises: **3 local keyDown
monitors + 1 global** (`popup::install_local_monitor` from `host.rs:3153` and
`views/tools.rs:1069`, raw ones at `screenshot.rs:5579`/`jira_setup.rs:418`,
global LeftMouseDown `host.rs:3183`); terminal keys bypass routing entirely
(SwiftTerm view is first responder); `screenshot.rs` has a string→`KeyInput`
front-end (`do:key:` specs, `shot_key_event`:1089).

## S4 — Attributed text / drawing (`NSAttributedString` 50/11, `NSBezierPath` 49/14)

- **No glyph/outline paths exist**: every `NSBezierPath` is rect/oval/
  polyline fills & strokes (icons: chevron/plus/cross/star, badges, clips).
  Pure geometry is extractable; painting stays AppKit. Woven cases
  (`vim_keys.rs`:917, `compare.rs`:5814, jira check/star) are woven in the
  *enclosing view*, not the path.
- `engines/ai_format.rs`:223-291 HTML→RTF is the cleanest candidate
  (`html_to_rtf(&str) -> Option<Vec<u8>>`) but non-macOS falls to `None`.
- `ui/ansi_text.rs`:15-77 `parse()` is already pure + tested; only the
  `attributed()` conversion is AppKit.
- Attr-dict builders (`style.rs`:178 + 3 clones) belong to the S2 span-type
  work; `ui/style.rs` already splits pure (1-118) vs `mod appkit` (117+).
- Precedent: `panes/pane_nav.rs`:391-402/553-568 has pure ring helpers with
  unit tests.
- Clips (9 sites: tree_view:480, sidebar:898, files_view:405/944,
  screenshot:5048, compare_view:611, jira_view:1203/1311) own
  save/restore-graphics-state pairing — that is the drawing-layer decision
  §0 rules out.
- Dead code: `files_view.rs`:731 builds an attributed string then discards it.

## S5 — WKWebView (≈48/7 files; §0 non-goal — future wave)

Per-file: `notes_prose.rs` `ProsePane` (4 user scripts, 3 msg handlers
`wsImage/wsJump/wsSearch`, eval/eval_number, zoom, `loadFileURL` cached page);
`notes.rs` `ProseWindow` (bare web view, exposes `LivePane "editor"`);
`confluence.rs` `ConfluencePreview` (`wsconf://` scheme handler + `"ws"`
handler → `HitPayload`); `ai.rs` `AIPreview` (`"ws"` link handler, no eval);
`confluence_view.rs` holds it as opaque `NSView`; `host_vim.rs` downcasts any
`NSView`→`WKWebView` for scroll/find; `panes/vim_keys.rs` pure model.

~20 call sites fit a trait; embedding (NSView) does not:
`trait WebPane { load_html, load_file, eval, eval_number, set_zoom,
set_on_message(WebMsg), set_hidden }` with `WebMsg { Link, Hit, Search, Jump,
Image }`. First moves: (1) `host_vim.rs` web keys need a pane *handle*
(`find_live` walks the view tree — add a marker/registration);
(2) `notes_prose` (blocker: `MainThreadBound` render hand-off needs a Send
handle); (3) unify confluence/ai `"ws"` handler (blocker: confluence's
`wsconf://` scheme handler has no plain-Rust equivalent — AI only first).

## S6 — System integration (NSStatusItem, `NSApplication` 34/8, `NSMenu` 62/4, hotkey, AX)

| Capability → where | Cross-platform candidate | Verdict |
|---|---|---|
| Status item (`app/menu.rs` ~547 `install_status_item`) | `tray-icon` | S, but see below |
| Status/main menu render (`menu.rs` ~447-646; plain `MenuSpec`) | `muda` (non-macOS) | M — **specs already plain + tested** |
| In-window menu re-pop (`pop_status_menu`:514) | muda `Menu::popup` | S |
| Closure pop-up/context menus (`ui/menu_kit.rs` `MenuEntry`; ~20 callers) | muda popup | M — hover previews/headers/event-based context popup have no muda equivalent |
| App lifecycle (`main.rs` setActivationPolicy/run/terminate; `host.rs` ~2660/~3786) | none real | leave — macOS-only semantics |
| Live focus/window queries (`host.rs` ~2795/2817/2829, chrome.rs ~1087, card.rs ~433, menu_kit ~291) | none | leave (windowing layer) |
| Responder-chain actions (`host.rs` ~8967 `sendAction_to_from`) | none | leave |
| Global hotkeys | `global-hotkey` exists — **wrong fit**: the app registers zero; AeroSpace TOML + `bin/kitchen_sink.sh` own the keys | leave |
| AeroSpace IPC (`app/hotkey.rs`, 460 lines, tested; `switcher.rs` ~1039) | per-WM adapters (swaymsg/hyprctl/komorebi) behind the injectable `AeroIpc` | M — feature-level, not an AppKit symbol |
| AX helpers (`rust/ws-helpers` dock badges / Webex unread) | `accessibility` crate exists; features are macOS-app-specific | leave (process boundary already is the seam) |
| TCC probes (`screenshot.rs` ~688-728) | ashpd (Linux) | leave |

Surprises: zero global hotkeys in Rust (AeroSpace + bash own them);
AeroSpace's `%{monitor-appkit-nsscreen-screens-id}` bakes AppKit screen
semantics into hotkey queries; the focus bridge (`ws-aerospace-focus`) has no
reader; `ws-helpers` links full `objc2-app-kit` solely for
`NSRunningApplication` pid lookups (shrinkable to core-foundation).

## Recommended next wave (ranked by value × safety)

1. **`platform::image` byte transforms** (S1) — S–M, headless-testable,
   removes real CoreGraphics glue: `rotate_png`, `reencode_png_as_jpeg`,
   `png_to_tiff`, `encode_png`; boundary `&[u8]`/RGBA; new pure-Rust dep.
   Manual step needed: one paste/pasteboard parity check (TIFF consumers).
2. **`platform::fonts` + `FontSpec`** (S2) — M; un-gate `Weight`, move it,
   resolve at the edge; serve both `NSFont` and `CTFont` users. Biggest
   structural enabler for any future renderer; do fonts before colors.
3. **`platform::input` + widened `KeyEvent`** (S3) — M; move `KeyInput` out
   of `ui/`, kill the duplicate adapters, make `key_input_from_event` the
   only converter, then clean `host.rs`'s raw reads. Coordinates with the 4
   key-monitor sites (they must move together).
4. **`WebPane`** (S5) — start with `host_vim` web keys (needs pane
   registration), then `notes_prose`; confluence blocked on `wsconf://`.
5. **Menu renderer behind the existing specs** (S6) — specs are ready;
   blocked on the muda gaps (hover previews, headers) + tray coupling.

**Stay out of scope** (per plan §0): NSImage display/decode paths, the
drawing layer (`NSBezierPath`/clips/attr-dict rendering), `NSApplication`
lifecycle, status item/main menu, AX helpers, TCC.

## Leftovers after executing WP1–WP8 (candidates for a follow-up wave)

- WP5 screens follow-up: `views/file_popup.rs` (2 sites need the live
  `window.screen()` — the plan's explicit leave case), `views/note_find.rs`
  + `views/tools.rs` (same shapes as the migrated sites; were outside the
  WP's file scope), `ui/shared_window.rs` (already pure index math).
- WP8 dispatch follow-up: the after-delay shape (7 sites: chrome 1, toast 1,
  host 4, screenshot 1) is a ready `platform::after(Duration, F)` candidate,
  deliberately not migrated; `MainThreadBound` constructions (4 sites) stay
  at call sites; 2 run-loop `NSTimer`s (host `drain:`, ai `pollStream:`)
  are audit-only. A pre-existing sync `ControllerInner::run_on_main`
  (host.rs, channel+timeout) is distinct from the new async seam.
- WP6 follow-up: `ui/sheet.rs` (NSAlert) — at most extract the pure confirm
  dialog into `platform::confirm(title, msg, buttons) -> usize`.
- WP1/WP4: `switcher.rs`/`host.rs` icon lookups (`NSImage`) stay — S1.
- WP2: file-url pasteboards, `ai_format` HTML/RTF, image pasteboards left
  (separate seams / later).
- WP7: all 22 `NSUserDefaults` hits migrated, nothing left.
- Non-macOS branches added (arboard, rfd, JSON prefs, FIFO dispatch) are
  cfg'd out on macOS and therefore compile-unverified here; they were kept
  trivially correct per plan rule 4.
