//! The live file-browser view — the AppKit half of Swift's
//! `PopupFileBrowser` + `FileListPane` (PopupWindow.swift). The model stays
//! in [`super::files::FileBrowser`]; the host pushes a [`FilesSnapshot`]
//! after every model change (`FileBrowser::sync_view`) and the view reports
//! clicks / typing back as [`FilesEvent`]s.
//!
//! Layout (Swift `layoutPanes`): the PLACES sidebar on the left; a 24 pt
//! toolbar row at y = 4 — parent ↑, the segmented Pin | Sort ⌄ | Asc/Desc,
//! the filter field; the list (56 %) | 6 pt gutter | preview (text, image,
//! folder listing or hint); an 18 pt status line at the bottom.

use super::files::Entry;
use crate::ui::chrome::Rect;
use crate::ui::sidebar::{SidebarEvent, SidebarModel};
use std::collections::BTreeSet;

/// What the preview pane shows (`PreviewContent` + the folder list).
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Preview {
    #[default]
    None,
    Hint(String),
    Text(String),
    Image(String),
    Folder(Vec<Entry>),
}

/// Everything the view draws, pushed by the model.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FilesSnapshot {
    pub rows: Vec<Entry>,
    pub selection: usize,
    pub marked: BTreeSet<usize>,
    pub sidebar: SidebarModel,
    pub pinned_on: bool,
    pub can_pin: bool,
    pub sort_short: String,
    pub descending: bool,
    pub status: String,
    pub placeholder: String,
    /// The filter text the model holds (applied to the field only when it
    /// differs, so typing isn't clobbered).
    pub query: String,
    /// What the idle field shows (`showCwdInFilter`: the cwd, or "" in a
    /// virtual list) — applied only while the field isn't being edited.
    pub field_text: String,
    pub preview: Preview,
    pub preview_key: String,
}

/// Clicks / typing reported to the owner.
#[derive(Clone, Debug, PartialEq)]
pub enum FilesEvent {
    Click { row: usize, cmd: bool, shift: bool, double: bool },
    RowMenu { row: usize, x: f64, y: f64 },
    /// Right-click on the empty part of the list (New Folder / Paste …).
    BlankMenu { x: f64, y: f64 },
    /// Files dropped on the list: onto folder row `row`, or (`None`) the
    /// whole listing. `copy` = Option held, `force_move` = Command held.
    Drop { row: Option<usize>, paths: Vec<String>, copy: bool, force_move: bool },
    PreviewOpen(usize),
    Parent,
    TogglePin,
    SortMenu { x: f64, y: f64 },
    ToggleOrder,
    Query(String),
    Enter,
    Move(i64),
    Escape,
    Sidebar(SidebarEvent),
    /// A drag that started in the list ended (the target may have moved the
    /// files away), so the listing needs a reload.
    DragEnded,
}

/// Toolbar hit ids.
pub const TB_PARENT: i32 = 1;
pub const TB_PIN: i32 = 2;
pub const TB_SORT: i32 = 3;
pub const TB_ORDER: i32 = 4;

pub const TOOLBAR_Y: f64 = 4.0;
pub const TOOLBAR_H: f64 = 24.0;
pub const STATUS_H: f64 = 18.0;
pub const SPLIT_W: f64 = 6.0;
pub const SIDEBAR_W: f64 = 240.0;

/// `layoutPanes()` geometry (toolbar widths come from text measurement on
/// the AppKit side; this is the pane split).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FilesLayout {
    pub sidebar: Rect,
    pub list: Rect,
    pub preview: Rect,
    pub status: Rect,
}

pub fn files_layout(width: f64, height: f64, sidebar_w: f64, split_fraction: f64) -> FilesLayout {
    let left = if sidebar_w > 0.0 { sidebar_w + 4.0 } else { 0.0 };
    let list_y = TOOLBAR_Y + TOOLBAR_H + 5.0 + 4.0;
    let split_x = (split_fraction * width).max(left + 200.0).min((width - 200.0).max(left + 200.0));
    let content_h = (height - list_y - STATUS_H).max(0.0);
    FilesLayout {
        sidebar: Rect::new(0.0, 0.0, sidebar_w, height),
        list: Rect::new(left, list_y, (split_x - left).max(0.0), content_h),
        preview: Rect::new(split_x + SPLIT_W, list_y, (width - split_x - SPLIT_W).max(0.0), content_h),
        status: Rect::new(8.0 + left, height - STATUS_H + 1.0, (width - 16.0 - left).max(0.0), STATUS_H - 3.0),
    }
}

#[cfg(target_os = "macos")]
pub use appkit::{FileListView, FilesView};

#[cfg(target_os = "macos")]
mod appkit {
    use super::*;
    use crate::ui::sidebar::SidebarView;
    use crate::ui::style::{self, ButtonState, Weight};
    use crate::ui::theme::{PopupColors, PopupTone};
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObject, ProtocolObject, Sel};
    use objc2::{define_class, msg_send, sel, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, Message};
    use objc2_app_kit::NSDraggingInfo;
    use objc2_app_kit::{
        NSAutoresizingMaskOptions, NSBezierPath, NSColor, NSControlTextEditingDelegate, NSEvent,
        NSDragOperation, NSDraggingContext, NSDraggingItem, NSDraggingSession, NSDraggingSource,
        NSEventModifierFlags, NSFocusRingType, NSFont, NSPasteboardWriting, NSGraphicsContext, NSImage, NSImageScaling,
        NSImageView, NSLineBreakMode, NSScrollView, NSTextAlignment, NSTextField,
        NSTextFieldDelegate, NSTextView, NSTrackingArea, NSTrackingAreaOptions, NSView, NSWorkspace,
    };
    use objc2_foundation::{
        NSArray, NSAttributedString, NSNotification, NSObjectProtocol, NSPoint, NSRect, NSSize,
        NSString, NSURL,
    };
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;

    fn as_any<T: Message + ?Sized>(obj: &T) -> &AnyObject {
        unsafe { &*(obj as *const T as *const AnyObject) }
    }

    fn ns(r: Rect) -> NSRect {
        style::ns(r)
    }

    thread_local! {
        /// `PopupFileBrowser.iconCache` — Finder icons per path.
        static ICONS: RefCell<HashMap<String, Retained<NSImage>>> = RefCell::new(HashMap::new());
    }

    pub fn file_icon(path: &str) -> Retained<NSImage> {
        if let Some(i) = ICONS.with(|c| c.borrow().get(path).cloned()) {
            return i;
        }
        let img = NSWorkspace::sharedWorkspace().iconForFile(&NSString::from_str(path));
        ICONS.with(|c| {
            let mut c = c.borrow_mut();
            if c.len() > 4000 {
                c.clear();
            }
            c.insert(path.to_string(), img.clone());
        });
        img
    }

    fn tracking(view: &NSView) -> Retained<NSTrackingArea> {
        let opts = NSTrackingAreaOptions::MouseMoved
            | NSTrackingAreaOptions::MouseEnteredAndExited
            | NSTrackingAreaOptions::ActiveAlways
            | NSTrackingAreaOptions::InVisibleRect;
        let t = unsafe {
            NSTrackingArea::initWithRect_options_owner_userInfo(
                NSTrackingArea::alloc(),
                view.bounds(),
                opts,
                Some(as_any(view)),
                None,
            )
        };
        view.addTrackingArea(&t);
        t
    }

    // ------------------------------------------------------------ list pane

    pub struct FileListIvars {
        rows: RefCell<Vec<Entry>>,
        selection: Cell<usize>,
        marked: RefCell<BTreeSet<usize>>,
        hover: Cell<Option<usize>>,
        colors: Cell<PopupColors>,
        zoom: Cell<f64>,
        on_event: RefCell<Option<Box<dyn Fn(FilesEvent)>>>,
        /// The preview-pane list reports `PreviewOpen` instead of clicks.
        preview: Cell<bool>,
        tracking: RefCell<Option<Retained<NSTrackingArea>>>,
        /// Mouse-down point (window coords) + row, armed until the drag
        /// threshold is crossed or the button goes up.
        drag_origin: Cell<Option<(NSPoint, usize)>>,
        /// A plain click on an already-marked row is held back until mouse-up
        /// so dragging a multi-selection does not collapse it first.
        pending_click: Cell<Option<(usize, bool)>>,
        /// The drop target while a drag hovers: -2 none, -1 the whole list,
        /// otherwise the folder row index.
        drop_row: Cell<i64>,
    }

    define_class!(
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSFileListView"]
        #[ivars = FileListIvars]
        /// `FileListPane`.
        pub struct FileListView;

        impl FileListView {
            #[unsafe(method(isFlipped))]
            fn is_flipped(&self) -> bool {
                true
            }

            /// `FileListPane` takes the keyboard (Ctrl+HJKL `files-list`).
            #[unsafe(method(acceptsFirstResponder))]
            fn afr(&self) -> bool {
                true
            }

            #[unsafe(method(mouseDownCanMoveWindow))]
            fn mdcmw(&self) -> bool {
                false
            }

            #[unsafe(method(acceptsFirstMouse:))]
            fn afm(&self, _e: Option<&NSEvent>) -> bool {
                true
            }

            #[unsafe(method(updateTrackingAreas))]
            fn uta(&self) {
                let _: () = unsafe { msg_send![super(self), updateTrackingAreas] };
                if let Some(t) = self.ivars().tracking.borrow_mut().take() {
                    self.removeTrackingArea(&t);
                }
                *self.ivars().tracking.borrow_mut() = Some(tracking(self));
            }

            #[unsafe(method(mouseMoved:))]
            fn mouse_moved(&self, e: &NSEvent) {
                let p = self.convertPoint_fromView(e.locationInWindow(), None);
                let i = (p.y / self.row_h()).floor();
                let n = self.ivars().rows.borrow().len();
                let h = (i >= 0.0 && (i as usize) < n).then_some(i as usize);
                if h != self.ivars().hover.get() {
                    self.ivars().hover.set(h);
                    self.setNeedsDisplay(true);
                }
            }

            #[unsafe(method(mouseExited:))]
            fn mouse_exited(&self, _e: &NSEvent) {
                self.ivars().hover.set(None);
                self.setNeedsDisplay(true);
            }

            #[unsafe(method(mouseDown:))]
            fn mouse_down(&self, e: &NSEvent) {
                let p = self.convertPoint_fromView(e.locationInWindow(), None);
                let i = (p.y / self.row_h()).floor();
                let n = self.ivars().rows.borrow().len();
                if i < 0.0 || i as usize >= n {
                    return;
                }
                let row = i as usize;
                let mods = e.modifierFlags();
                let double = e.clickCount() >= 2;
                if self.ivars().preview.get() {
                    self.ivars().selection.set(row);
                    self.setNeedsDisplay(true);
                    if double {
                        self.emit(FilesEvent::PreviewOpen(row));
                    }
                    return;
                }
                let plain = !mods.contains(NSEventModifierFlags::Command)
                    && !mods.contains(NSEventModifierFlags::Shift);
                self.ivars().drag_origin.set(Some((e.locationInWindow(), row)));
                let in_multi = {
                    let m = self.ivars().marked.borrow();
                    m.len() > 1 && m.contains(&row)
                };
                if plain && !double && in_multi {
                    self.ivars().pending_click.set(Some((row, double)));
                    return;
                }
                self.emit(FilesEvent::Click {
                    row,
                    cmd: mods.contains(NSEventModifierFlags::Command),
                    shift: mods.contains(NSEventModifierFlags::Shift),
                    double,
                });
            }

            #[unsafe(method(mouseUp:))]
            fn mouse_up(&self, _e: &NSEvent) {
                self.ivars().drag_origin.set(None);
                if let Some((row, double)) = self.ivars().pending_click.take() {
                    self.emit(FilesEvent::Click { row, cmd: false, shift: false, double });
                }
            }

            #[unsafe(method(mouseDragged:))]
            fn mouse_dragged(&self, e: &NSEvent) {
                let Some((origin, row)) = self.ivars().drag_origin.get() else { return };
                let now = e.locationInWindow();
                if (now.x - origin.x).hypot(now.y - origin.y) < 4.0 {
                    return;
                }
                self.ivars().drag_origin.set(None);
                // Dragging a marked row drags every marked row; any other row
                // drags just itself (Finder semantics).
                self.ivars().pending_click.set(None);
                self.begin_drag(e, row);
            }

            #[unsafe(method(rightMouseDown:))]
            fn right_mouse_down(&self, e: &NSEvent) {
                if self.ivars().preview.get() {
                    return;
                }
                let p = self.convertPoint_fromView(e.locationInWindow(), None);
                let i = (p.y / self.row_h()).floor();
                let n = self.ivars().rows.borrow().len();
                if i < 0.0 || i as usize >= n {
                    let s = NSEvent::mouseLocation();
                    self.emit(FilesEvent::BlankMenu { x: s.x, y: s.y });
                    return;
                }
                let s = NSEvent::mouseLocation();
                self.emit(FilesEvent::RowMenu { row: i as usize, x: s.x, y: s.y });
            }

            #[unsafe(method(drawRect:))]
            fn draw_rect(&self, dirty: NSRect) {
                self.draw(dirty);
                self.draw_drop_ring();
            }

            #[unsafe(method(draggingEntered:))]
            fn dragging_entered(&self, info: &ProtocolObject<dyn objc2_app_kit::NSDraggingInfo>) -> NSDragOperation {
                self.drag_update(info)
            }

            #[unsafe(method(draggingUpdated:))]
            fn dragging_updated(&self, info: &ProtocolObject<dyn objc2_app_kit::NSDraggingInfo>) -> NSDragOperation {
                self.drag_update(info)
            }

            #[unsafe(method(draggingExited:))]
            fn dragging_exited(&self, _info: Option<&ProtocolObject<dyn objc2_app_kit::NSDraggingInfo>>) {
                self.ivars().drop_row.set(-2);
                self.setNeedsDisplay(true);
            }

            #[unsafe(method(performDragOperation:))]
            fn perform_drag(&self, info: &ProtocolObject<dyn objc2_app_kit::NSDraggingInfo>) -> objc2::runtime::Bool {
                let target = self.drop_target(info);
                self.ivars().drop_row.set(-2);
                self.setNeedsDisplay(true);
                let paths = dragged_paths(info);
                if paths.is_empty() {
                    return objc2::runtime::Bool::NO;
                }
                let mods = NSEvent::modifierFlags_class();
                self.emit(FilesEvent::Drop {
                    row: target,
                    paths,
                    copy: mods.contains(NSEventModifierFlags::Option),
                    force_move: mods.contains(NSEventModifierFlags::Command),
                });
                objc2::runtime::Bool::YES
            }
        }

        unsafe impl NSObjectProtocol for FileListView {}

        unsafe impl NSDraggingSource for FileListView {
            #[unsafe(method(draggingSession:sourceOperationMaskForDraggingContext:))]
            fn drag_mask(&self, _s: &NSDraggingSession, ctx: NSDraggingContext) -> NSDragOperation {
                if ctx == NSDraggingContext::OutsideApplication {
                    NSDragOperation::Copy | NSDragOperation::Generic | NSDragOperation::Move
                } else {
                    NSDragOperation::Copy | NSDragOperation::Move
                }
            }

            #[unsafe(method(draggingSession:endedAtPoint:operation:))]
            fn drag_ended(&self, _s: &NSDraggingSession, _p: NSPoint, op: NSDragOperation) {
                if op.contains(NSDragOperation::Move) {
                    self.emit(FilesEvent::DragEnded);
                }
            }
        }
    );

    /// The file paths on a dragging pasteboard.
    fn dragged_paths(info: &ProtocolObject<dyn objc2_app_kit::NSDraggingInfo>) -> Vec<String> {
        let pb = info.draggingPasteboard();
        let ty = NSString::from_str("public.file-url");
        pb.pasteboardItems()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|it| it.stringForType(&ty))
                    .filter_map(|s| NSURL::URLWithString(&s))
                    .filter_map(|u| u.path().map(|p| p.to_string()))
                    .collect()
            })
            .unwrap_or_default()
    }

    impl FileListView {
        /// The folder row under the pointer (not `..`, not one of the dragged
        /// paths), else `None` = drop into the listing itself.
        fn drop_target(&self, info: &ProtocolObject<dyn objc2_app_kit::NSDraggingInfo>) -> Option<usize> {
            let p = self.convertPoint_fromView(info.draggingLocation(), None);
            let i = (p.y / self.row_h()).floor();
            let rows = self.ivars().rows.borrow();
            if i < 0.0 || i as usize >= rows.len() {
                return None;
            }
            let e = &rows[i as usize];
            if !e.is_dir || e.is_parent() {
                return None;
            }
            let dragged = dragged_paths(info);
            (!dragged.contains(&e.path)).then_some(i as usize)
        }

        fn drag_update(&self, info: &ProtocolObject<dyn objc2_app_kit::NSDraggingInfo>) -> NSDragOperation {
            let t = self.drop_target(info).map(|r| r as i64).unwrap_or(-1);
            if self.ivars().drop_row.get() != t {
                self.ivars().drop_row.set(t);
                self.setNeedsDisplay(true);
            }
            let allowed = info.draggingSourceOperationMask();
            let mods = NSEvent::modifierFlags_class();
            if mods.contains(NSEventModifierFlags::Option) && allowed.contains(NSDragOperation::Copy) {
                NSDragOperation::Copy
            } else if allowed.contains(NSDragOperation::Move) {
                NSDragOperation::Move
            } else {
                NSDragOperation::Copy
            }
        }

        /// The ring around the drop target (a folder row or the whole list).
        fn draw_drop_ring(&self) {
            let t = self.ivars().drop_row.get();
            if t == -2 {
                return;
            }
            let c = self.ivars().colors.get();
            let r = if t < 0 {
                let b = self.bounds();
                Rect::new(b.origin.x + 1.0, b.origin.y + 1.0, b.size.width - 2.0, b.size.height - 2.0)
            } else {
                self.row_rect(t as usize).inset_by(4.0, 1.0)
            };
            style::stroke_rounded(r, 4.0, 2.0, c.accent_on());
        }

        pub fn new(mtm: MainThreadMarker, colors: PopupColors, preview: bool) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(FileListIvars {
                rows: RefCell::new(Vec::new()),
                selection: Cell::new(0),
                marked: RefCell::new(BTreeSet::new()),
                hover: Cell::new(None),
                colors: Cell::new(colors),
                zoom: Cell::new(1.0),
                on_event: RefCell::new(None),
                preview: Cell::new(preview),
                tracking: RefCell::new(None),
                drag_origin: Cell::new(None),
                pending_click: Cell::new(None),
                drop_row: Cell::new(-2),
            });
            let v: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
            if !preview {
                let ty = unsafe { objc2_app_kit::NSPasteboardTypeFileURL };
                v.registerForDraggedTypes(&NSArray::from_slice(&[ty]));
            }
            v
        }

        /// Start a real file drag (`NSURL` writers) so the rows can be dropped
        /// into Finder, a terminal, mail, another app's window…
        fn begin_drag(&self, e: &NSEvent, row: usize) {
            let paths: Vec<String> = {
                let rows = self.ivars().rows.borrow();
                let marked = self.ivars().marked.borrow();
                let idx: Vec<usize> = if marked.contains(&row) {
                    marked.iter().copied().collect()
                } else {
                    vec![row]
                };
                idx.into_iter()
                    .filter_map(|i| rows.get(i))
                    .filter(|r| !r.is_parent())
                    .map(|r| r.path.clone())
                    .collect()
            };
            if paths.is_empty() {
                return;
            }
            let at = self.convertPoint_fromView(e.locationInWindow(), None);
            let items: Vec<Retained<NSDraggingItem>> = paths
                .iter()
                .enumerate()
                .map(|(n, p)| {
                    let url = NSURL::fileURLWithPath(&NSString::from_str(p));
                    let writer: Retained<ProtocolObject<dyn NSPasteboardWriting>> =
                        ProtocolObject::from_retained(url);
                    let item = NSDraggingItem::initWithPasteboardWriter(NSDraggingItem::alloc(), &writer);
                    let off = (n.min(5) as f64) * 4.0;
                    let frame = NSRect::new(
                        NSPoint::new(at.x - 16.0 + off, at.y - 16.0 + off),
                        NSSize::new(32.0, 32.0),
                    );
                    let icon = file_icon(p);
                    unsafe { item.setDraggingFrame_contents(frame, Some(as_any(&*icon))) };
                    item
                })
                .collect();
            let arr = NSArray::from_retained_slice(&items);
            let src = ProtocolObject::from_ref(self);
            self.beginDraggingSessionWithItems_event_source(&arr, e, src);
        }

        pub fn set_on_event(&self, cb: Box<dyn Fn(FilesEvent)>) {
            *self.ivars().on_event.borrow_mut() = Some(cb);
        }

        fn emit(&self, ev: FilesEvent) {
            let cb = self.ivars().on_event.borrow_mut().take();
            if let Some(cb) = cb {
                cb(ev);
                let mut slot = self.ivars().on_event.borrow_mut();
                if slot.is_none() {
                    *slot = Some(cb);
                }
            }
        }

        pub fn row_h(&self) -> f64 {
            (22.0 * self.ivars().zoom.get()).round()
        }

        /// `textZoom` (Cmd+± on the list).
        pub fn set_zoom(&self, z: f64) {
            self.ivars().zoom.set(z);
            self.setNeedsDisplay(true);
        }

        pub fn zoom(&self) -> f64 {
            self.ivars().zoom.get()
        }

        pub fn set_rows(&self, rows: &[Entry], selection: usize, marked: &BTreeSet<usize>) {
            let same = *self.ivars().rows.borrow() == rows;
            if !same {
                *self.ivars().rows.borrow_mut() = rows.to_vec();
                self.ivars().hover.set(None);
            }
            self.ivars().selection.set(selection);
            *self.ivars().marked.borrow_mut() = marked.clone();
            self.setNeedsDisplay(true);
        }

        pub fn row_count(&self) -> usize {
            self.ivars().rows.borrow().len()
        }

        pub fn row_rect(&self, i: usize) -> Rect {
            let h = self.row_h();
            Rect::new(0.0, i as f64 * h, self.bounds().size.width, h)
        }

        /// The visible rows' rects (for the vim search highlight overlay).
        pub fn vim_shown_rects(&self) -> Vec<(usize, Rect)> {
            let n = self.row_count();
            let rh = self.row_h();
            let vis = self.visibleRect();
            if n == 0 || rh <= 0.0 || vis.size.height <= 0.0 {
                return Vec::new();
            }
            let first = (vis.origin.y / rh).floor().max(0.0) as usize;
            let last = (((vis.origin.y + vis.size.height) / rh).floor() as usize).min(n - 1);
            if first > last {
                return Vec::new();
            }
            (first..=last).map(|i| (i, self.row_rect(i))).collect()
        }

        fn draw(&self, dirty: NSRect) {
            let c = self.ivars().colors.get();
            let z = self.ivars().zoom.get();
            let rh = self.row_h();
            let rows = self.ivars().rows.borrow();
            if rows.is_empty() {
                return;
            }
            let w = self.bounds().size.width;
            let first = (dirty.origin.y / rh).floor().max(0.0) as usize;
            let last = (((dirty.origin.y + dirty.size.height) / rh).floor() as usize).min(rows.len() - 1);
            let sel = self.ivars().selection.get();
            let marked = self.ivars().marked.borrow();
            let hover = self.ivars().hover.get();
            let radius = 4.0;
            let isz = (16.0 * z).round();
            let name_f = style::system_font(12.0 * z, Weight::Regular);
            let dir_f = style::system_font(11.0 * z, Weight::Bold);
            let trail_f = style::system_font(10.0 * z, Weight::Regular);
            for i in first..=last {
                let e = &rows[i];
                let r = self.row_rect(i);
                let pill = r.inset_by(4.0, 1.0);
                let on = i == sel || marked.contains(&i);
                if on {
                    style::fill_rounded(pill, radius, c.highlight);
                    if i == sel {
                        NSGraphicsContext::saveGraphicsState_class();
                        NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(ns(pill), radius, radius).addClip();
                        style::fill_rect(Rect::new(pill.x, pill.y, 3.0, pill.height), c.accent_on());
                        NSGraphicsContext::restoreGraphicsState_class();
                    }
                } else if hover == Some(i) {
                    style::fill_rounded(pill, radius, style::button_fill(ButtonState::Hover, &c));
                }
                let ir = Rect::new(r.x + 10.0, r.y, isz, rh).inset_by(1.0, (rh - isz) / 2.0);
                let icon = if e.is_parent() {
                    style::symbol_image("arrow.turn.left.up", 11.0, Weight::Semibold, c.dim)
                        .unwrap_or_else(|| file_icon(&e.path))
                } else {
                    file_icon(&e.path)
                };
                if e.is_parent() {
                    style::draw_symbol("arrow.turn.left.up", Rect::new(r.x + 10.0, r.y, isz, rh), c.dim, 11.0);
                } else {
                    style::draw_image(&icon, ir, 1.0);
                }
                let mut tx = r.x + 10.0 + isz + 6.0;
                let is_dir = e.is_dir && !e.is_parent();
                if is_dir {
                    tx += 4.0;
                }
                let trail_w = if e.trailing_text.is_empty() {
                    0.0
                } else {
                    style::text_size(&e.trailing_text, &trail_f).0
                };
                let avail = w - tx - 8.0 - if trail_w > 0.0 { trail_w + 16.0 + 4.0 } else { 0.0 };
                let name_color = if on { c.text } else { c.text.with_alpha(0.85) };
                let drawn = style::draw_text_left(&e.name, Rect::new(tx, r.y, avail.max(20.0), rh), &name_f, name_color);
                if is_dir {
                    let (_, gh) = style::text_size("/", &dir_f);
                    style::draw_text("/", tx + drawn + 2.0, r.mid_y() - gh / 2.0, &dir_f, c.tone(PopupTone::Accent2));
                }
                if trail_w > 0.0 {
                    let (_, th) = style::text_size(&e.trailing_text, &trail_f);
                    style::draw_text(&e.trailing_text, w - trail_w - 16.0, r.mid_y() - th / 2.0, &trail_f, c.dim);
                }
            }
        }
    }

    // ------------------------------------------------------------ container

    pub struct FilesIvars {
        snap: RefCell<FilesSnapshot>,
        colors: Cell<PopupColors>,
        sidebar: Retained<SidebarView>,
        search: Retained<NSTextField>,
        search_box: Retained<NSView>,
        list_scroll: Retained<NSScrollView>,
        list: Retained<FileListView>,
        preview_scroll: Retained<NSScrollView>,
        preview_text: Retained<NSTextView>,
        preview_image: Retained<NSImageView>,
        preview_hint: Retained<NSTextField>,
        preview_list_scroll: Retained<NSScrollView>,
        preview_list: Retained<FileListView>,
        on_event: RefCell<Option<std::rc::Rc<dyn Fn(FilesEvent)>>>,
        hover: Cell<Option<i32>>,
        pressed: Cell<Option<i32>>,
        tracking: RefCell<Option<Retained<NSTrackingArea>>>,
        shown_preview_key: RefCell<String>,
    }

    define_class!(
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSFilesView"]
        #[ivars = FilesIvars]
        pub struct FilesView;

        impl FilesView {

            #[unsafe(method(isFlipped))]
            fn is_flipped(&self) -> bool {
                true
            }

            #[unsafe(method(setFrameSize:))]
            fn set_frame_size(&self, size: NSSize) {
                let _: () = unsafe { msg_send![super(self), setFrameSize: size] };
                self.layout_panes();
            }

            #[unsafe(method(updateTrackingAreas))]
            fn uta(&self) {
                let _: () = unsafe { msg_send![super(self), updateTrackingAreas] };
                if let Some(t) = self.ivars().tracking.borrow_mut().take() {
                    self.removeTrackingArea(&t);
                }
                *self.ivars().tracking.borrow_mut() = Some(tracking(self));
            }

            #[unsafe(method(mouseMoved:))]
            fn mouse_moved(&self, e: &NSEvent) {
                let p = self.convertPoint_fromView(e.locationInWindow(), None);
                let h = self.toolbar_hit(p.x, p.y);
                if h != self.ivars().hover.get() {
                    self.ivars().hover.set(h);
                    self.setNeedsDisplay(true);
                }
            }

            #[unsafe(method(mouseExited:))]
            fn mouse_exited(&self, _e: &NSEvent) {
                self.ivars().hover.set(None);
                self.setNeedsDisplay(true);
            }

            #[unsafe(method(mouseDown:))]
            fn mouse_down(&self, e: &NSEvent) {
                let p = self.convertPoint_fromView(e.locationInWindow(), None);
                let h = self.toolbar_hit(p.x, p.y);
                self.ivars().pressed.set(h);
                self.setNeedsDisplay(true);
            }

            #[unsafe(method(mouseUp:))]
            fn mouse_up(&self, e: &NSEvent) {
                let p = self.convertPoint_fromView(e.locationInWindow(), None);
                let h = self.toolbar_hit(p.x, p.y);
                let was = self.ivars().pressed.replace(None);
                self.setNeedsDisplay(true);
                if h.is_none() || h != was {
                    return;
                }
                let rects = self.toolbar_rects();
                let ev = match h.unwrap() {
                    TB_PARENT => FilesEvent::Parent,
                    TB_PIN => FilesEvent::TogglePin,
                    TB_SORT => {
                        let r = rects.iter().find(|(id, _)| *id == TB_SORT).map(|(_, r)| *r).unwrap_or(Rect::ZERO);
                        FilesEvent::SortMenu { x: r.x, y: r.max_y() + 2.0 }
                    }
                    _ => FilesEvent::ToggleOrder,
                };
                self.emit(ev);
            }

            #[unsafe(method(drawRect:))]
            fn draw_rect(&self, _dirty: NSRect) {
                self.draw_toolbar();
                self.draw_status();
            }

            #[unsafe(method(controlTextDidChange:))]
            fn control_text_did_change(&self, _n: &NSNotification) {
                let q = self.ivars().search.stringValue().to_string();
                self.ivars().snap.borrow_mut().query = q.clone();
                self.emit(FilesEvent::Query(q));
            }

            #[unsafe(method(control:textView:doCommandBySelector:))]
            fn do_command(&self, _c: &AnyObject, _tv: &NSTextView, cmd: Sel) -> objc2::runtime::Bool {
                let ev = if cmd == sel!(insertNewline:) {
                    FilesEvent::Enter
                } else if cmd == sel!(moveDown:) {
                    FilesEvent::Move(1)
                } else if cmd == sel!(moveUp:) {
                    FilesEvent::Move(-1)
                } else if cmd == sel!(cancelOperation:) {
                    FilesEvent::Escape
                } else {
                    return objc2::runtime::Bool::NO;
                };
                self.emit(ev);
                objc2::runtime::Bool::YES
            }
        }

        unsafe impl NSObjectProtocol for FilesView {}
        unsafe impl NSControlTextEditingDelegate for FilesView {}
        unsafe impl NSTextFieldDelegate for FilesView {}
    );

    fn label(mtm: MainThreadMarker, size: f64, color: crate::ui::theme::Rgba) -> Retained<NSTextField> {
        let l = NSTextField::labelWithString(&NSString::from_str(""), mtm);
        l.setFont(Some(&NSFont::systemFontOfSize(size)));
        l.setTextColor(Some(&color.to_nscolor()));
        l
    }

    fn scroll(mtm: MainThreadMarker) -> Retained<NSScrollView> {
        let s = NSScrollView::new(mtm);
        s.setHasVerticalScroller(true);
        s.setAutohidesScrollers(true);
        s.setDrawsBackground(false);
        s
    }

    impl FilesView {
        /// `PopupFileBrowser.navPanes`: sidebar, filter, list, preview.
        pub fn nav_panes(&self) -> Vec<crate::app::live_panes::LivePane> {
            use crate::app::live_panes::LivePane;
            let iv = self.ivars();
            let mut out = vec![
                LivePane::new("files-sidebar", &iv.sidebar),
                LivePane::new("files-filter", &iv.search_box).focusing(&iv.search),
                LivePane::new("files-list", &iv.list_scroll).focusing(&iv.list),
            ];
            if !iv.preview_list_scroll.isHidden() {
                out.push(LivePane::new("files-preview", &iv.preview_list_scroll).focusing(&iv.preview_list));
            } else if !iv.preview_scroll.isHidden() {
                out.push(LivePane::new("files-preview", &iv.preview_scroll).focusing(&iv.preview_text));
            } else if !iv.preview_image.isHidden() && iv.preview_image.image().is_some() {
                out.push(LivePane::new("files-preview", &iv.preview_image));
            }
            out
        }
        pub fn new(mtm: MainThreadMarker, colors: PopupColors) -> Retained<Self> {
            let sidebar = SidebarView::new(mtm, colors);
            sidebar.set_collapse_key("files");
            let (search_box, search) = crate::ui::field::search_box(mtm, &colors, 11.0, "filter…", 4.0, TOOLBAR_H);
            let list_scroll = scroll(mtm);
            let list = FileListView::new(mtm, colors, false);
            list_scroll.setDocumentView(Some(&list));
            let preview_scroll = scroll(mtm);
            let preview_text = NSTextView::new(mtm);
            preview_text.setEditable(false);
            preview_text.setSelectable(true);
            preview_text.setDrawsBackground(false);
            preview_text.setTextContainerInset(NSSize::new(8.0, 8.0));
            preview_text.setFont(Some(&style::mono_font(12.0, Weight::Regular)));
            preview_text.setTextColor(Some(&colors.text.to_nscolor()));
            preview_text.setVerticallyResizable(true);
            preview_text.setHorizontallyResizable(false);
            preview_text.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
            if let Some(tc) = unsafe { preview_text.textContainer() } {
                tc.setWidthTracksTextView(true);
            }
            preview_scroll.setDocumentView(Some(&preview_text));
            let preview_image = NSImageView::new(mtm);
            preview_image.setImageScaling(NSImageScaling::ScaleProportionallyDown);
            let preview_hint = label(mtm, 12.0, colors.dim);
            preview_hint.setAlignment(NSTextAlignment::Center);
            let preview_list_scroll = scroll(mtm);
            let preview_list = FileListView::new(mtm, colors, true);
            preview_list_scroll.setDocumentView(Some(&preview_list));

            let this = Self::alloc(mtm).set_ivars(FilesIvars {
                snap: RefCell::new(FilesSnapshot::default()),
                colors: Cell::new(colors),
                sidebar,
                search,
                search_box,
                list_scroll,
                list,
                preview_scroll,
                preview_text,
                preview_image,
                preview_hint,
                preview_list_scroll,
                preview_list,
                on_event: RefCell::new(None),
                hover: Cell::new(None),
                pressed: Cell::new(None),
                tracking: RefCell::new(None),
                shown_preview_key: RefCell::new(String::new()),
            });
            let v: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
            let iv = v.ivars();
            v.addSubview(&iv.sidebar);
            v.addSubview(&iv.search_box);
            v.addSubview(&iv.list_scroll);
            v.addSubview(&iv.preview_scroll);
            v.addSubview(&iv.preview_image);
            v.addSubview(&iv.preview_hint);
            v.addSubview(&iv.preview_list_scroll);
            unsafe { iv.search.setDelegate(Some(ProtocolObject::from_ref(&*v))) };
            v.show_preview(&Preview::None);
            v
        }

        /// Install the event sink (the host's `files_event`).
        pub fn set_on_event(&self, cb: Box<dyn Fn(FilesEvent)>) {
            let rc: std::rc::Rc<dyn Fn(FilesEvent)> = std::rc::Rc::from(cb);
            *self.ivars().on_event.borrow_mut() = Some(rc.clone());
            let a = rc.clone();
            self.ivars().list.set_on_event(Box::new(move |e| a(e)));
            let b = rc.clone();
            self.ivars().preview_list.set_on_event(Box::new(move |e| b(e)));
            let s = rc;
            self.ivars().sidebar.set_on_event(Box::new(move |e| s(FilesEvent::Sidebar(e))));
        }

        fn emit(&self, ev: FilesEvent) {
            let cb = self.ivars().on_event.borrow().clone();
            if let Some(cb) = cb {
                cb(ev);
            }
        }

        pub fn search_field(&self) -> Retained<NSTextField> {
            self.ivars().search.clone()
        }

        pub fn list_view(&self) -> Retained<FileListView> {
            self.ivars().list.clone()
        }

        /// The file list's document view plus each visible row's rect (the vim
        /// search highlight draws the overlay into it).
        pub fn vim_shown_rows(&self) -> (Retained<NSView>, Vec<(usize, Rect)>) {
            let l = self.list_view();
            let rects = l.vim_shown_rects();
            (Retained::into_super(l), rects)
        }

        pub fn sidebar(&self) -> Retained<SidebarView> {
            self.ivars().sidebar.clone()
        }

        /// Push a fresh model snapshot.
        pub fn apply(&self, s: FilesSnapshot) {
            let iv = self.ivars();
            iv.sidebar.set_model(s.sidebar.clone());
            let cur = iv.search.stringValue().to_string();
            let editing = iv.search.currentEditor().is_some();
            let want = if editing || !s.query.is_empty() { &s.query } else { &s.field_text };
            if !(editing && s.query.is_empty()) && cur != *want {
                iv.search.setStringValue(&NSString::from_str(want));
            }
            let c = iv.colors.get();
            let ph = NSAttributedString::from_nsstring(&NSString::from_str(&s.placeholder));
            let _ = ph;
            set_placeholder(&iv.search, &s.placeholder, c);
            let rows_changed = iv.snap.borrow().rows != s.rows;
            iv.list.set_rows(&s.rows, s.selection, &s.marked);
            if *iv.shown_preview_key.borrow() != s.preview_key {
                *iv.shown_preview_key.borrow_mut() = s.preview_key.clone();
                self.show_preview(&s.preview);
            }
            *iv.snap.borrow_mut() = s;
            self.layout_panes();
            if rows_changed {
                iv.list_scroll.contentView().scrollToPoint(NSPoint::new(0.0, 0.0));
                iv.list_scroll.reflectScrolledClipView(&iv.list_scroll.contentView());
            }
            self.scroll_selection_visible();
            self.setNeedsDisplay(true);
        }

        fn sidebar_w(&self) -> f64 {
            self.ivars().sidebar.width(SIDEBAR_W)
        }

        pub fn layout_panes(&self) {
            let b = self.bounds();
            let iv = self.ivars();
            let lay = files_layout(b.size.width, b.size.height, self.sidebar_w(), 0.56);
            iv.sidebar.setFrame(ns(lay.sidebar));
            let rects = self.toolbar_rects();
            let order_max = rects.iter().find(|(id, _)| *id == TB_ORDER).map(|(_, r)| r.max_x()).unwrap_or(0.0);
            iv.search_box.setFrame(ns(Rect::new(
                order_max + 10.0,
                TOOLBAR_Y,
                (b.size.width - order_max - 10.0 - 8.0).max(60.0),
                TOOLBAR_H,
            )));
            iv.list_scroll.setFrame(ns(lay.list));
            let doc_h = (iv.list.row_count() as f64 * iv.list.row_h()).max(lay.list.height);
            iv.list.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(lay.list.width, doc_h)));
            for v in [&*iv.preview_scroll as &NSView, &iv.preview_image, &iv.preview_list_scroll] {
                v.setFrame(ns(lay.preview));
            }
            iv.preview_hint.setFrame(ns(Rect::new(
                lay.preview.x,
                lay.preview.y + 8.0,
                lay.preview.width,
                20.0,
            )));
            let pdoc = (iv.preview_list.row_count() as f64 * iv.preview_list.row_h()).max(lay.preview.height);
            iv.preview_list.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(lay.preview.width, pdoc)));
            iv.preview_text.setFrame(NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(lay.preview.width, iv.preview_text.frame().size.height.max(lay.preview.height)),
            ));
        }

        fn scroll_selection_visible(&self) {
            let iv = self.ivars();
            let sel = iv.snap.borrow().selection;
            if sel >= iv.list.row_count() {
                return;
            }
            let clip = iv.list_scroll.contentView();
            let vis = clip.bounds();
            if vis.size.height < iv.list.row_h() {
                return;
            }
            let r = iv.list.row_rect(sel);
            let mut target = vis.origin.y;
            if r.max_y() > vis.origin.y + vis.size.height {
                target = r.max_y() - vis.size.height;
            } else if r.y < vis.origin.y {
                target = r.y;
            }
            if target != vis.origin.y {
                clip.scrollToPoint(NSPoint::new(0.0, target));
                iv.list_scroll.reflectScrolledClipView(&clip);
            }
        }

        fn show_preview(&self, p: &Preview) {
            let iv = self.ivars();
            iv.preview_scroll.setHidden(true);
            iv.preview_image.setHidden(true);
            iv.preview_list_scroll.setHidden(true);
            iv.preview_hint.setHidden(true);
            match p {
                Preview::None => {}
                Preview::Hint(h) => {
                    iv.preview_hint.setStringValue(&NSString::from_str(h));
                    iv.preview_hint.setHidden(h.is_empty());
                }
                Preview::Text(t) => {
                    iv.preview_text.setString(&NSString::from_str(t));
                    iv.preview_text.setFont(Some(&style::mono_font(12.0, Weight::Regular)));
                    iv.preview_text.setTextColor(Some(&iv.colors.get().text.to_nscolor()));
                    iv.preview_text.scrollPoint(NSPoint::new(0.0, 0.0));
                    iv.preview_scroll.setHidden(false);
                }
                Preview::Image(path) => match style::image_file(path) {
                    Some(img) => {
                        iv.preview_image.setImage(Some(&img));
                        iv.preview_image.setHidden(false);
                    }
                    None => {
                        iv.preview_hint.setStringValue(&NSString::from_str("unable to preview"));
                        iv.preview_hint.setHidden(false);
                    }
                },
                Preview::Folder(rows) => {
                    iv.preview_list.set_rows(rows, usize::MAX, &BTreeSet::new());
                    iv.preview_list_scroll.setHidden(false);
                }
            }
        }

        /// Toolbar button rects (`layoutPanes`): parent, then the segmented
        /// Pin | Sort | Order group.
        pub fn toolbar_rects(&self) -> Vec<(i32, Rect)> {
            let snap = self.ivars().snap.borrow();
            let left = {
                let w = self.sidebar_w();
                if w > 0.0 { w + 4.0 } else { 0.0 }
            };
            let x0 = 8.0 + left;
            let f = style::button_font(10.5, ButtonState::On);
            let fit = |t: &str, sym: bool, chev: bool| -> f64 {
                let mut w = if t.is_empty() { 0.0 } else { style::text_size(t, &f).0 };
                if sym {
                    w += 12.0 + if t.is_empty() { 0.0 } else { 4.0 };
                }
                if chev {
                    w += 12.0;
                }
                (w + 18.0).ceil()
            };
            // No toolbar "up" button: the list's `..` row does that job.
            let pin_w = fit("Pin", true, false).max(fit("Pinned", true, false));
            let pin = Rect::new(x0, TOOLBAR_Y, pin_w, TOOLBAR_H);
            let sort_w = super::super::files::SortKey::ALL
                .iter()
                .map(|k| fit(k.short(), true, true))
                .fold(0.0, f64::max);
            let sort = Rect::new(pin.max_x(), TOOLBAR_Y, sort_w, TOOLBAR_H);
            let order_w = fit("Asc", true, false).max(fit("Desc", true, false));
            let order = Rect::new(sort.max_x(), TOOLBAR_Y, order_w, TOOLBAR_H);
            let _ = &snap;
            vec![(TB_PIN, pin), (TB_SORT, sort), (TB_ORDER, order)]
        }

        fn toolbar_hit(&self, x: f64, y: f64) -> Option<i32> {
            self.toolbar_rects().into_iter().find(|(_, r)| r.contains(x, y)).map(|(id, _)| id)
        }

        fn draw_toolbar(&self) {
            let c = self.ivars().colors.get();
            let snap = self.ivars().snap.borrow();
            let hover = self.ivars().hover.get();
            let pressed = self.ivars().pressed.get();
            let rects = self.toolbar_rects();
            for (id, r) in &rects {
                let is_on = *id == TB_PIN && snap.pinned_on;
                let st = if pressed == Some(*id) {
                    ButtonState::Pressed
                } else {
                    ButtonState::from_flags(is_on, hover == Some(*id), false)
                };
                let (title, sym, chev, seg) = match *id {
                    TB_PARENT => (String::new(), "arrow.up".to_string(), false, 0),
                    TB_PIN => (
                        if snap.pinned_on { "Pinned" } else { "Pin" }.to_string(),
                        if snap.pinned_on { "star.fill" } else { "star" }.to_string(),
                        false,
                        1,
                    ),
                    TB_SORT => (snap.sort_short.clone(), "line.3.horizontal.decrease".to_string(), true, 2),
                    _ => (
                        if snap.descending { "Desc" } else { "Asc" }.to_string(),
                        if snap.descending { "arrow.down" } else { "arrow.up" }.to_string(),
                        false,
                        3,
                    ),
                };
                draw_theme_button(*r, st, &c, &title, &sym, chev, seg);
            }
        }

        fn draw_status(&self) {
            let c = self.ivars().colors.get();
            let snap = self.ivars().snap.borrow();
            let b = self.bounds();
            let lay = files_layout(b.size.width, b.size.height, self.sidebar_w(), 0.56);
            let f = style::system_font(10.5, Weight::Regular);
            style::draw_text_left(&snap.status, lay.status, &f, c.dim);
        }
    }

    /// `ThemeButton.draw` — alone (seg 0) or a segment of a joined group
    /// (1 first, 2 middle, 3 last).
    pub fn draw_theme_button(r: Rect, st: ButtonState, c: &PopupColors, title: &str, sym: &str, chev: bool, seg: u8) {
        if seg == 0 {
            style::draw_button(r, st, c, 4.0, false);
        } else {
            let rad = 8.0;
            let first = seg == 1;
            let last = seg == 3;
            let ext = Rect::new(
                if first { r.x } else { r.x - rad },
                r.y + 0.5,
                r.width + if first { 0.0 } else { rad } + if last { 0.0 } else { rad },
                r.height - 1.0,
            );
            NSGraphicsContext::saveGraphicsState_class();
            NSBezierPath::clipRect(ns(r));
            style::fill_rounded(ext, rad, style::button_fill(st, c));
            NSGraphicsContext::restoreGraphicsState_class();
            if !first {
                style::fill_rect(Rect::new(r.x, r.y + 5.0, 1.0, r.height - 10.0), c.text.with_alpha(0.14));
            }
        }
        let color = style::button_text(st, c);
        let size = 10.5;
        if title.is_empty() && !chev {
            style::draw_symbol(sym, r, color, size + 0.5);
            return;
        }
        let f = style::button_font(size, st);
        let tw = if title.is_empty() { 0.0 } else { style::text_size(title, &f).0 };
        let mut content = tw;
        if !sym.is_empty() {
            content += 12.0 + if title.is_empty() { 0.0 } else { 4.0 };
        }
        if chev {
            content += 12.0;
        }
        let mut x = (r.mid_x() - content / 2.0).round();
        if !sym.is_empty() {
            style::draw_symbol(sym, Rect::new(x, r.y, 12.0, r.height), color, size);
            x += 12.0 + if title.is_empty() { 0.0 } else { 4.0 };
        }
        if !title.is_empty() {
            let (_, th) = style::text_size(title, &f);
            style::draw_text(title, x, r.mid_y() - th / 2.0, &f, color);
            x += tw;
        }
        if chev {
            style::chevron(Rect::new(x + 3.0, r.mid_y() - 4.0, 8.0, 8.0), color);
        }
    }

    fn set_placeholder(field: &NSTextField, text: &str, c: PopupColors) {
        use objc2_app_kit::{NSFontAttributeName, NSForegroundColorAttributeName};
        use objc2_foundation::{NSAttributedStringKey, NSDictionary};
        let font = NSFont::systemFontOfSize(11.0);
        let color = c.dim.to_nscolor();
        let keys: [&NSAttributedStringKey; 2] = unsafe { [NSFontAttributeName, NSForegroundColorAttributeName] };
        let objs: [&AnyObject; 2] = [as_any(&*font), as_any(&*color)];
        let attrs = NSDictionary::from_slices(&keys, &objs);
        let s = unsafe {
            NSAttributedString::initWithString_attributes(
                NSAttributedString::alloc(),
                &NSString::from_str(text),
                Some(&attrs),
            )
        };
        unsafe {
            let cell: Option<Retained<AnyObject>> = msg_send![field, cell];
            if let Some(cell) = cell {
                let _: () = msg_send![&*cell, setPlaceholderAttributedString: &*s];
            }
        }
        let _ = (NSLineBreakMode::ByTruncatingTail, NSColor::clearColor());
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_splits_list_and_preview() {
        let l = files_layout(1600.0, 1000.0, 240.0, 0.56);
        assert_eq!(l.list.x, 244.0);
        assert!((l.list.max_x() - 896.0).abs() < 1e-9);
        assert!((l.preview.x - (896.0 + SPLIT_W)).abs() < 1e-9);
        assert_eq!(l.status.y, 1000.0 - STATUS_H + 1.0);
        assert!(l.list.height > 0.0);
    }

    #[test]
    fn narrow_window_keeps_200pt_panes() {
        let l = files_layout(500.0, 400.0, 0.0, 0.56);
        assert!(l.list.width >= 200.0);
    }
}
