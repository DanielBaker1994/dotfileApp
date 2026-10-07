# Finding: the focused PDF/reading window had no JankyBorders highlight

**Symptom.** Focusing a PDF (reading) window with the AeroSpace focus keys
(`alt-h/j/k/l`) left it with no border highlight; every other window got one.

**Status.** Fixed in `NotesProse.swift` (`ProseWindow` is now a plain
`NSWindow` at `.normal` level, not a floating `NSPanel`).

---

## TL;DR

JankyBorders 1.9.0 does **not** use the accessibility API by default. It
resolves the front process and reads SkyLight window *tags*, then borders only
windows tagged `DOCUMENT` (or `FLOATING` + `MODAL`) — see `window_suitable()`
in JankyBorders `src/windows.c`. **Any window at a non-normal level, and every
`NSPanel`, is tagged `FLOATING` and skipped.** The pop-out reading window
(`ProseWindow`) was an `NSPanel` with `isFloatingPanel = true` /
`level = .floating`, so the window server tagged it floating and JankyBorders
ignored it even though AeroSpace could focus it.

The fix is to present it as a normal document window. There is **no bordersrc
option** that can include a floating panel.

## The actual mechanism (read from source, not assumed)

JankyBorders `windows_determine_and_focus_active_window()` resolves the focused
window id, then `windows_window_create()` runs it through:

```c
static inline bool window_suitable(CFTypeRef iterator) {
  uint64_t tags = SLSWindowIteratorGetTags(iterator);
  uint64_t attributes = SLSWindowIteratorGetAttributes(iterator);
  uint32_t parent_wid = SLSWindowIteratorGetParentID(iterator);
  if ((parent_wid == 0)
       && ((attributes & 0x2) || (tags & 0x400000000000000))
       && !(tags & WINDOW_TAG_ATTACHED)
       && !(tags & WINDOW_TAG_IGNORES_CYCLE)
       && ((tags & WINDOW_TAG_DOCUMENT) || ((tags & WINDOW_TAG_FLOATING)
                                            && (tags & WINDOW_TAG_MODAL)))) {
    return true;
  }
  return false;
}
```

- `WINDOW_TAG_DOCUMENT = 1<<0`, `WINDOW_TAG_FLOATING = 1<<1`, `MODAL = 1<<31`.
- So: a normal app window (document) is bordered; a floating panel is **not**
  (it would have to be both floating *and* modal).

`ax_focus` (man page) is **not** a workaround: `ax_get_front_window()` only
changes how the focused window id is found. `windows_window_create()` still
applies `window_suitable`, so a floating panel is still rejected.

### Empirical confirmation

Using the same private SkyLight calls (`SLSWindowQueryWindows` /
`SLSWindowIteratorGetTags`) and the same predicate:

| window                                              | level | doc | floating | `window_suitable` |
|-----------------------------------------------------|-------|-----|----------|-------------------|
| plain `NSWindow`, `.titled`                          | 0     | yes | no       | **true**          |
| `NSWindow`, `.titled`, **level `.floating`**         | 3     | no  | yes      | false             |
| `NSPanel`, `.titled`, `isFloatingPanel`, `.floating` | 3     | no  | yes      | false             |
| `NSPanel`, `.titled`, **level `.normal`**            | 0     | yes | no       | **false** (`attrs=0x1`) |
| `NSWindow`, `.titled`, `.normal` (fixed ProseWindow) | 0     | yes | no       | **true**          |

The deciding factor is the **window level** (`NSWindowController`/`NSPanel`
class also matters: an `NSPanel` never gets the `attrs` bit JankyBorders
needs, even at level 0). Real app windows in the live session (Ghostty,
Firefox, TextEdit, QuickTime, …) all showed `doc=true, suitable=true`;
JankyBorders' own overlay windows showed `floating=true, suitable=false`.

## The four hypotheses, judged

1. **No standard title bar / non-normal level / panel** — **cause.** The reading
   window was a floating `NSPanel` (non-normal level), so it was tagged
   `FLOATING` and skipped.
2. **AeroSpace focuses without the app becoming frontmost, so borders'
   active-window tracking misses it** — **not the cause.** JankyBorders listens
   to SkyLight front-app / reorder / title events, which AeroSpace's focus does
   raise, and normal windows follow correctly.
3. **PDF shown in Preview / Quick Look / a kitchen-sink floating panel** — the
   kitchen-sink floating panel (`ProseWindow`) is the real one. A PDF opened in
   Preview is a normal document window and *is* bordered. A system
   `QLPreviewPanel` (Quick Look) is a floating panel and is still skipped — see
   "Not fixed here" below.
4. **bordersrc options** — **no option helps.** `whitelist`/`blacklist` filter
   by app but `window_suitable` runs regardless; `ax_focus` doesn't bypass it.

Note: a PDF opened as a **note tab** is *not* affected — the notes/list window
is a plain `NSWindow` (`PopupPlainWindow`) at `.normal` level, so it is tagged
document and bordered.

## The fix

`NotesProse.swift`, `ProseWindow`:

- `final class ProseWindow: NSPanel` → `: NSWindow`
- removed `isFloatingPanel = true` and `hidesOnDeactivate = false` (NSPanel-only)
- `level = .floating` → `level = .normal`

The window still has `.titled, .closable, .resizable, .fullSizeContentView` with
a hidden/transparent titlebar, so it keeps AeroSpace's close-button heuristic
(and now JankyBorders' document tag). Under AeroSpace the app's own
`on-window-detected` rule already tiles kitchen-sink windows, so nothing about
placement changes.

## Not fixed here

A system `QLPreviewPanel` (Quick Look, Space on a PDF in the file browser) is
also a floating panel and will not get a border. It is system-owned, so the app
cannot change its tags; fixing it would mean previewing PDFs in an app-owned
document window instead of Quick Look.

## Verifying manually

1. Build: `./ws build` (or `bin/build-app.sh`).
2. Open a note, press `⌘⇧P` (Prose) then `⌘⇧O` / the ⤢ chip to pop out the
   reading window.
3. `alt-h/j/k/l` to it: it should now carry the active border.
