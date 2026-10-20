# W2-compare-trace: Compare folder keys ignored

Method: palette -> Compare, typed /private/tmp/kssb/home/cmp-a and cmp-b into Left/Right fields (real clicks + type), Return -> folder session (4 rows). Then `key down down`, `key j j`.

## Evidence
- Keys DO reach the handler and in_text is FALSE (hypothesis wrong):
  `DBG compare key code=125 in_text=false fr=Some("NSKVONotifying_WSSlotHostWindow")` (x2 for Down, code=38 x2 for j).
  While typing in the start page: `in_text=true fr=NSTextView`; after Return starts the session first responder becomes the window itself, in_text=false.
- state after Down Down and j j: folder.cursor stays 0, selected 0, sub false.
- So the key is consumed by handle_key's wrong branch, not stood down.

## Root cause
rust/ws-rs/src/views/compare.rs:4072 (`handle_key`):
`if self.current().is_some_and(|s| s.folder.is_some()) { return self.folder_key(key); }`
A bound folder session is MOVED into `folder_page` (bind_selected_folder, compare.rs:3436-3441: `s.folder.take()`, `folder_bound = Some(id)`), so `current().folder` is None while bound. The check is false, handle_key falls through to the TEXT-diff key table (display_count 0, move_cursor no-op, j not mapped) and folder_key is never called. `current_is_folder()` (compare.rs:3288) already handles this correctly (`folder.is_some() || folder_bound == Some(id)`) but handle_key does not use it. do:compare:folder-open works because test_do goes to folder_open() directly.

## Fix
compare.rs:4072: `if self.current_is_folder() { return self.folder_key(key); }`
(host.rs:5470-5490 routing is fine; first responder = window is acceptable, no makeFirstResponder needed.)

## File diff (W2-2), not re-driven (Return cannot open the diff until the fix)
Same text-path table has no `j` and `n` only with ctrl (code 45 && ctrl), so plain j/n are unbound there; and for a diff opened from a folder the same current().folder check misroutes. Cmd+W (code 13) is not in handle_key at all; check where Cmd+W is routed (host.rs close-session path) separately. Click/Esc/Tab experiments skipped: fr/in_text were already shown to be non-blocking.
