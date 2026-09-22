# Bug: Window Shakes When Dragged After Opening from Menu Bar

## Root Cause

When a window is opened from the menu bar and the user immediately starts dragging it, multiple systems fight over the window position:

### Primary Cause: `takeFocus()` Retry Loop Conflicts with Drag

**File:** `PopupWindow.swift:5621-5641`

When `show()` → `presentList()` → `takeFocus()` is called:

1. `takeFocus()` calls `NSApp.activate(ignoringOtherApps: true)` 
2. This fires `NSApplication.didBecomeActiveNotification`
3. The notification handler (line 5654-5670) calls `panel.makeKeyAndOrderFront(nil)` — which **repositions the window**
4. Meanwhile, the `takeFocus()` retry loop fires up to 10 times at 150ms intervals, each time calling `makeKeyAndOrderFront` again
5. If the user starts dragging during this 1.5s window, the native drag and the repeated `makeKeyAndOrderFront` calls fight over the window origin

### Secondary Cause: Two Drag Systems on Titled Windows

**File:** `PopupWindow.swift:4417-4433`

Note/list windows have a real (hidden) `.titled` titlebar for the AX close button. When `config.enableDrag` is true:

- The chrome header view calls `window?.performDrag(with: event)` (line 3416) — the native macOS drag
- But if something causes `mouseDragged` to fire with `draggingWindow = true`, it falls back to manual `setFrameOrigin` (line 3449)
- The hidden titlebar's own drag tracking can conflict with `performDrag`

## Affected Windows

All windows opened via menu bar toggles that have `enableDrag = true`:
- Notes (`cfg.enableDrag = cmd.drag` — defaults true)
- Jira list (`cfg.enableDrag = cmd.drag`)
- Health checks / output windows
- Prettyprint window (`cfg.enableDrag = true`)
- Detail window (`cfg.enableDrag = true`)

## Reproduction Steps

1. Click a window toggle in the menu bar (e.g., "Toggle Notes")
2. Immediately click and drag the window before it finishes animating into place
3. Window shakes/jitters as `takeFocus()` retries and `didBecomeActiveNotification` handler fight the drag

## Fix Plan

### Fix 1: Cancel takeFocus() retry loop when a drag starts

In `PopupWindow.swift`, track when a drag is in progress and cancel the `takeFocus()` retry:

```swift
// Add a property
private var isDragging = false

// In takeFocus(), check before retrying:
if !panel.isKeyWindow, focusRetries < 10, !isDragging {
    focusRetries += 1
    DispatchQueue.main.asyncAfter(deadline: .now() + 0.15) { [weak self] in
        self?.takeFocus()
    }
}

// In chrome view mouseDown, set isDragging = true before performDrag
// In mouseUp, set isDragging = false
```

### Fix 2: Suppress didBecomeActiveNotification handler during active drag

In the `didBecomeActiveNotification` handler (line 5654), check if a drag is in progress before calling `makeKeyAndOrderFront`.

### Fix 3: Reduce takeFocus() retry count and interval

The 10 retries at 150ms = 1.5s of potential interference. Reduce to 3 retries at 100ms.

### Fix 4: Debounce makeKeyAndOrderFront in the activation handler

Add a small debounce (50ms) to the `didBecomeActiveNotification` handler so rapid activations don't cause multiple repositionings.

## Test Plan (Regression)

See `TESTING_PLAN.md` in this directory.
