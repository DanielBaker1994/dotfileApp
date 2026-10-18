//! The AppKit half of [`super::MultiPickerModel`]: the transient popover
//! (filter field, drawn list, drawn footer) and the closed-state chip.

use super::{MultiPickerModel, PickOption, GROUP_PREFIX, MORE_PREFIX};
use crate::ui::chrome::Rect;
use crate::ui::style::{self, ns, ButtonState, Weight};
use crate::ui::theme::{PopupColors, Rgba};
use crate::ui::view_base;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly, Message};
use objc2_app_kit::{
    NSAppearance, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSControlTextEditingDelegate, NSEvent, NSPopover,
    NSPopoverBehavior, NSPopoverDelegate, NSScrollView, NSTextField, NSTextFieldDelegate, NSTextView,
    NSTrackingArea, NSTrackingAreaOptions, NSView, NSViewController,
};
use objc2_foundation::{NSNotification, NSObjectProtocol, NSPoint, NSRect, NSRectEdge, NSSize, NSString};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

pub type Shared = Rc<RefCell<MultiPickerModel>>;
pub type Callback = Rc<dyn Fn()>;

const ROW_H: f64 = 24.0;
const FOOT_H: f64 = 34.0;

thread_local! {
    /// The open popover (one at a time, like the transient Swift picker).
    static OPEN: RefCell<Option<(Retained<NSPopover>, Retained<PickerList>)>> = const { RefCell::new(None) };
}

/// Close whatever picker popover is open.
pub fn close_open() {
    let p = OPEN.with(|o| o.borrow().as_ref().map(|(p, _)| p.clone()));
    if let Some(p) = p {
        p.close();
    }
}

/// The open popover's model changed underneath it (options arrived): refilter,
/// redraw, and grow / shrink the list to fit (4–14 rows).
pub fn refresh_open() {
    let open = OPEN.with(|o| o.borrow().as_ref().map(|(p, l)| (p.clone(), l.clone())));
    let Some((pop, list)) = open else { return };
    list.ivars().model.borrow_mut().refilter();
    let n = list.ivars().model.borrow().shown.len();
    let list_h = (n.clamp(4, 14) as f64) * ROW_H + 4.0;
    let Some(scroll) = list.ivars().scroll.borrow().clone() else { return };
    let old = scroll.frame().size.height;
    if (old - list_h).abs() > 0.5 {
        let size = pop.contentSize();
        let h = size.height - old + list_h;
        pop.setContentSize(NSSize::new(size.width, h));
        if let Some(content) = unsafe { scroll.superview() } {
            content.setFrameSize(NSSize::new(size.width, h));
            for sub in content.subviews().iter() {
                // The filter box rides the top edge (unflipped content view).
                let f = sub.frame();
                if f.origin.y > scroll.frame().origin.y + old - 1.0 {
                    sub.setFrameOrigin(NSPoint::new(f.origin.x, f.origin.y - old + list_h));
                }
            }
        }
        scroll.setFrameSize(NSSize::new(scroll.frame().size.width, list_h));
    }
    list.reload();
}

pub fn is_open() -> bool {
    OPEN.with(|o| o.borrow().is_some())
}

/// One drawn footer button.
struct FootButton {
    title: String,
    primary: bool,
    action: Callback,
}

pub struct ListIvars {
    model: Shared,
    colors: Cell<PopupColors>,
    on_change: Callback,
    on_close: RefCell<Option<Callback>>,
    field: RefCell<Option<Retained<NSTextField>>>,
    scroll: RefCell<Option<Retained<NSScrollView>>>,
    footer: RefCell<Option<Retained<PickerFooter>>>,
}

define_class!(
    /// The option list (drawn rows) and the filter field's delegate.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "WSMultiPickerList"]
    #[ivars = ListIvars]
    pub struct PickerList;

    impl PickerList {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool { true }

        #[unsafe(method(acceptsFirstMouse:))]
        fn afm(&self, _e: Option<&NSEvent>) -> bool { true }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, e: &NSEvent) {
            let p = self.convertPoint_fromView(e.locationInWindow(), None);
            let row = (p.y / ROW_H).floor();
            if row >= 0.0 {
                self.toggle(row as usize);
            }
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            self.draw();
        }

        #[unsafe(method(controlTextDidChange:))]
        fn control_text_did_change(&self, _n: &NSNotification) {
            let q = self.ivars().field.borrow().as_ref().map(|f| f.stringValue().to_string()).unwrap_or_default();
            {
                let mut m = self.ivars().model.borrow_mut();
                m.query = q;
                m.highlight = 0;
                m.refilter();
            }
            self.reload();
        }

        #[unsafe(method(control:textView:doCommandBySelector:))]
        fn do_command(&self, _c: &AnyObject, _tv: &NSTextView, cmd: Sel) -> objc2::runtime::Bool {
            // Ctrl+N / Ctrl+P arrive as moveDown: / moveUp: (Emacs bindings).
            if cmd == sel!(moveDown:) || cmd == sel!(insertTab:) {
                self.ivars().model.borrow_mut().move_highlight(1);
            } else if cmd == sel!(moveUp:) || cmd == sel!(insertBacktab:) {
                self.ivars().model.borrow_mut().move_highlight(-1);
            } else if cmd == sel!(insertNewline:) {
                let r = self.ivars().model.borrow().highlight;
                self.toggle(r);
                return objc2::runtime::Bool::YES;
            } else if cmd == sel!(cancelOperation:) {
                let empty = self.ivars().model.borrow().query.is_empty();
                if empty {
                    close_open();
                } else if let Some(f) = self.ivars().field.borrow().as_ref() {
                    f.setStringValue(&NSString::from_str(""));
                    let mut m = self.ivars().model.borrow_mut();
                    m.query.clear();
                    m.refilter();
                }
                self.reload();
                return objc2::runtime::Bool::YES;
            } else {
                return objc2::runtime::Bool::NO;
            }
            self.reload();
            self.scroll_highlight_visible();
            objc2::runtime::Bool::YES
        }
    }

    unsafe impl NSObjectProtocol for PickerList {}
    unsafe impl NSControlTextEditingDelegate for PickerList {}
    unsafe impl NSTextFieldDelegate for PickerList {}

    unsafe impl NSPopoverDelegate for PickerList {
        #[unsafe(method(popoverDidClose:))]
        fn popover_did_close(&self, _n: &NSNotification) {
            OPEN.with(|o| o.borrow_mut().take());
            let cb = self.ivars().on_close.borrow_mut().take();
            if let Some(cb) = cb {
                cb();
            }
        }
    }
);

impl PickerList {
    fn toggle(&self, row: usize) {
        let changed = self.ivars().model.borrow_mut().toggle(row);
        self.reload();
        if changed {
            (self.ivars().on_change)();
        }
    }

    fn reload(&self) {
        let n = self.ivars().model.borrow().shown.len();
        let w = self.ivars().scroll.borrow().as_ref().map(|s| s.contentSize().width).unwrap_or(self.bounds().size.width);
        self.setFrameSize(NSSize::new(w, (n as f64 * ROW_H).max(ROW_H)));
        self.setNeedsDisplay(true);
        if let Some(f) = self.ivars().footer.borrow().as_ref() {
            f.setNeedsDisplay(true);
        }
    }

    fn scroll_highlight_visible(&self) {
        let h = self.ivars().model.borrow().highlight;
        let w = self.bounds().size.width;
        self.scrollRectToVisible(ns(Rect::new(0.0, h as f64 * ROW_H, w, ROW_H)));
    }

    fn draw(&self) {
        let c = self.ivars().colors.get();
        let m = self.ivars().model.borrow();
        let w = self.bounds().size.width;
        let font = style::system_font(13.0, Weight::Regular);
        let bold = style::system_font(13.0, Weight::Semibold);
        let small = style::system_font(11.0, Weight::Regular);
        let head = style::system_font(10.5, Weight::Semibold);
        if m.shown.is_empty() {
            let t = if m.options.is_empty() { format!("(no {}s cached)", m.noun) } else { "No matches".to_string() };
            style::draw_text_left(&t, Rect::new(10.0, 0.0, w - 20.0, ROW_H), &small, c.dim);
            return;
        }
        for (i, o) in m.shown.iter().enumerate() {
            let band = Rect::new(0.0, i as f64 * ROW_H, w, ROW_H);
            if i == m.highlight {
                style::fill_rounded(Rect::new(2.0, band.y + 1.0, w - 4.0, ROW_H - 2.0), 5.0, c.highlight);
            }
            let header = o.id.starts_with(GROUP_PREFIX);
            let more = o.id.starts_with(MORE_PREFIX);
            if more {
                let tw = style::draw_text_left(&o.title, Rect::new(30.0, band.y, w * 0.5, ROW_H), &font, style::button_accent(&c));
                style::draw_text_left(&o.detail, Rect::new(30.0 + tw + 8.0, band.y, w - tw - 46.0, ROW_H), &small, c.dim);
                continue;
            }
            let on = m.is_on(o);
            let sym = if on { "checkmark.square.fill" } else { "square" };
            let tint = if on { style::button_accent(&c) } else { c.dim.with_alpha(if header { 0.45 } else { 0.6 }) };
            style::draw_symbol(sym, Rect::new(6.0, band.y + 4.0, 16.0, 16.0), tint, 13.0);
            if header {
                let tw = style::draw_text_left(&o.title.to_uppercase(), Rect::new(28.0, band.y + 2.0, w * 0.6, ROW_H), &head, c.dim);
                style::draw_text_left(&o.detail, Rect::new(28.0 + tw + 8.0, band.y + 2.0, w - tw - 44.0, ROW_H), &small, c.dim);
                continue;
            }
            let f = if o.id == super::ALL_ID { &bold } else { &font };
            let tw = style::draw_text_left(&o.title, Rect::new(28.0, band.y, (w - 40.0) * 0.6, ROW_H), f, c.text);
            if !o.detail.is_empty() {
                style::draw_text_left(&o.detail, Rect::new(28.0 + tw + 8.0, band.y, (w - 44.0 - tw).max(0.0), ROW_H), &small, c.dim);
            }
        }
    }
}

pub struct FooterIvars {
    model: Shared,
    colors: Cell<PopupColors>,
    buttons: RefCell<Vec<FootButton>>,
}

define_class!(
    /// "N selected · M shown" + the extra buttons, Clear and Done.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "WSMultiPickerFooter"]
    #[ivars = FooterIvars]
    pub struct PickerFooter;

    impl PickerFooter {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool { true }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, e: &NSEvent) {
            let p = self.convertPoint_fromView(e.locationInWindow(), None);
            let hit = self.button_rects().into_iter().position(|r| r.contains(p.x, p.y));
            if let Some(i) = hit {
                let a = self.ivars().buttons.borrow().get(i).map(|b| b.action.clone());
                if let Some(a) = a {
                    a();
                }
            }
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            let c = self.ivars().colors.get();
            let small = style::system_font(11.0, Weight::Regular);
            let rects = self.button_rects();
            let left = rects.first().map(|r| r.x).unwrap_or(self.bounds().size.width) - 8.0;
            let count = self.ivars().model.borrow().count_text();
            style::draw_text_left(&count, Rect::new(10.0, 0.0, (left - 10.0).max(0.0), FOOT_H), &small, c.dim);
            let f = style::system_font(12.0, Weight::Medium);
            for (b, r) in self.ivars().buttons.borrow().iter().zip(rects) {
                if b.primary {
                    style::capsule_primary(r, &c, false, false);
                    style::draw_text_centered(&b.title, r, &f, c.on_accent());
                } else {
                    style::draw_button(r, ButtonState::Idle, &c, 6.0, false);
                    style::draw_text_centered(&b.title, r, &f, style::button_text(ButtonState::Idle, &c));
                }
            }
        }
    }
);

impl PickerFooter {
    fn button_rects(&self) -> Vec<Rect> {
        let f = style::system_font(12.0, Weight::Medium);
        let mut x = self.bounds().size.width - 10.0;
        let mut out: Vec<Rect> = self
            .ivars()
            .buttons
            .borrow()
            .iter()
            .rev()
            .map(|b| {
                let w = style::text_size(&b.title, &f).0 + 22.0;
                x -= w;
                let r = Rect::new(x, (FOOT_H - 24.0) / 2.0, w, 24.0);
                x -= 6.0;
                r
            })
            .collect();
        out.reverse();
        out
    }
}

/// Open the picker under `rect` of `anchor` (`togglePopover`). `extras` are
/// Swift's `extraButtons` (left of Clear / Done); `on_change` runs after
/// every toggle / Clear, `on_close` when the popover goes away.
pub fn show(
    anchor: &NSView,
    rect: Rect,
    model: Shared,
    colors: PopupColors,
    extras: Vec<(String, Callback)>,
    on_change: Callback,
    on_close: Option<Callback>,
) {
    close_open();
    let Some(mtm) = MainThreadMarker::new() else { return };
    {
        let mut m = model.borrow_mut();
        m.query.clear();
        m.expanded.clear();
        m.highlight = 0;
        m.refilter();
    }
    let n = model.borrow().shown.len();
    let w = if extras.is_empty() { 340.0f64 } else { 440.0 }.max(rect.width);
    let list_h = (n.clamp(4, 14) as f64) * ROW_H + 4.0;
    let h = 10.0 + 28.0 + 6.0 + list_h + 6.0 + FOOT_H;
    let content = NSView::initWithFrame(NSView::alloc(mtm), NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(w, h)));
    // NSView is unflipped: lay out from the bottom.
    let noun = model.borrow().noun.clone();
    let (field_box, field) = crate::ui::field::search_box(
        mtm,
        &colors,
        13.0,
        &format!("Filter {noun}s — ↑↓ move · Return toggles · Esc closes"),
        6.0,
        28.0,
    );
    field_box.setFrame(NSRect::new(NSPoint::new(10.0, h - 10.0 - 28.0), NSSize::new(w - 20.0, 28.0)));
    content.addSubview(&field_box);
    let scroll = NSScrollView::new(mtm);
    scroll.setFrame(NSRect::new(NSPoint::new(10.0, FOOT_H + 6.0), NSSize::new(w - 20.0, list_h)));
    scroll.setHasVerticalScroller(true);
    scroll.setAutohidesScrollers(true);
    scroll.setDrawsBackground(false);
    let list = PickerList::alloc(mtm).set_ivars(ListIvars {
        model: model.clone(),
        colors: Cell::new(colors),
        on_change: on_change.clone(),
        on_close: RefCell::new(on_close),
        field: RefCell::new(Some(field.clone())),
        scroll: RefCell::new(Some(scroll.clone())),
        footer: RefCell::new(None),
    });
    let list: Retained<PickerList> = unsafe { msg_send![super(list), initWithFrame: NSRect::ZERO] };
    scroll.setDocumentView(Some(&list));
    content.addSubview(&scroll);
    let mut buttons: Vec<FootButton> = extras.into_iter().map(|(t, a)| FootButton { title: t, primary: false, action: a }).collect();
    let lw = objc2::rc::Weak::from_retained(&list);
    let m2 = model.clone();
    let oc = on_change.clone();
    buttons.push(FootButton {
        title: "Clear".into(),
        primary: false,
        action: Rc::new(move || {
            m2.borrow_mut().clear();
            if let Some(l) = lw.load() {
                l.reload();
            }
            oc();
        }),
    });
    buttons.push(FootButton { title: "Done".into(), primary: true, action: Rc::new(close_open) });
    let footer = PickerFooter::alloc(mtm).set_ivars(FooterIvars {
        model: model.clone(),
        colors: Cell::new(colors),
        buttons: RefCell::new(buttons),
    });
    let footer: Retained<PickerFooter> = unsafe { msg_send![super(footer), initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(w, FOOT_H))] };
    content.addSubview(&footer);
    *list.ivars().footer.borrow_mut() = Some(footer);
    unsafe { field.setDelegate(Some(ProtocolObject::from_ref(&*list))) };
    list.reload();

    let vc = NSViewController::new(mtm);
    vc.setView(&content);
    let pop = NSPopover::new(mtm);
    let name = if colors.is_light() { unsafe { NSAppearanceNameAqua } } else { unsafe { NSAppearanceNameDarkAqua } };
    pop.setAppearance(NSAppearance::appearanceNamed(name).as_deref());
    pop.setBehavior(NSPopoverBehavior::Transient);
    pop.setContentViewController(Some(&vc));
    pop.setContentSize(NSSize::new(w, h));
    pop.setDelegate(Some(ProtocolObject::from_ref(&*list)));
    let edge = if anchor.isFlipped() { NSRectEdge::MaxY } else { NSRectEdge::MinY };
    pop.showRelativeToRect_ofView_preferredEdge(ns(rect), anchor, edge);
    OPEN.with(|o| *o.borrow_mut() = Some((pop, list.clone())));
    if let Some(win) = field.window() {
        win.makeKeyWindow();
        win.makeFirstResponder(Some(&field));
    }
}

pub struct ChipIvars {
    model: Shared,
    colors: Cell<PopupColors>,
    on_change: RefCell<Option<Callback>>,
    hover: Cell<bool>,
    tracking: RefCell<Option<Retained<NSTrackingArea>>>,
}

define_class!(
    /// The closed picker: an input-styled box of pills + a chevron. Click,
    /// Space, Return or ↓ opens the popover (`JiraMultiPicker.keyDown`).
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "WSMultiPickerChip"]
    #[ivars = ChipIvars]
    pub struct PickerChip;

    impl PickerChip {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool { true }

        #[unsafe(method(acceptsFirstResponder))]
        fn afr(&self) -> bool { true }

        #[unsafe(method(becomeFirstResponder))]
        fn become_fr(&self) -> bool {
            self.setNeedsDisplay(true);
            true
        }

        #[unsafe(method(resignFirstResponder))]
        fn resign_fr(&self) -> bool {
            self.setNeedsDisplay(true);
            true
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, _e: &NSEvent) {
            self.open();
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, e: &NSEvent) {
            if matches!(e.keyCode(), 49 | 36 | 125) {
                self.open();
            } else {
                let _: () = unsafe { msg_send![super(self), keyDown: e] };
            }
        }

        #[unsafe(method(updateTrackingAreas))]
        fn uta(&self) {
            let _: () = unsafe { msg_send![super(self), updateTrackingAreas] };
            view_base::reset_tracking(self, &self.ivars().tracking, NSTrackingAreaOptions::MouseEnteredAndExited | NSTrackingAreaOptions::ActiveAlways | NSTrackingAreaOptions::InVisibleRect);
        }

        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, _e: &NSEvent) {
            self.ivars().hover.set(true);
            self.setNeedsDisplay(true);
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _e: &NSEvent) {
            view_base::set_hover(self, &self.ivars().hover, false);
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            self.draw();
        }
    }
);

impl PickerChip {
    pub fn new(mtm: MainThreadMarker, model: Shared, colors: PopupColors) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ChipIvars {
            model,
            colors: Cell::new(colors),
            on_change: RefCell::new(None),
            hover: Cell::new(false),
            tracking: RefCell::new(None),
        });
        let v: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
        v.refresh();
        v
    }

    pub fn model(&self) -> Shared {
        self.ivars().model.clone()
    }

    pub fn set_on_change(&self, cb: Callback) {
        *self.ivars().on_change.borrow_mut() = Some(cb);
    }

    pub fn set_colors(&self, c: PopupColors) {
        self.ivars().colors.set(c);
        self.setNeedsDisplay(true);
    }

    /// Redraw + retooltip after the model changed from outside.
    pub fn refresh(&self) {
        let tip = self.ivars().model.borrow().tooltip();
        self.setToolTip(Some(&NSString::from_str(&tip)));
        self.setNeedsDisplay(true);
    }

    pub fn open(&self) {
        let me = objc2::rc::Weak::from_retained(&self.retain());
        let cb = self.ivars().on_change.borrow().clone();
        let on_change: Callback = Rc::new(move || {
            if let Some(v) = me.load() {
                v.refresh();
            }
            if let Some(cb) = &cb {
                cb();
            }
        });
        let me2 = objc2::rc::Weak::from_retained(&self.retain());
        let on_close: Callback = Rc::new(move || {
            if let Some(v) = me2.load() {
                if let Some(w) = v.window() {
                    w.makeFirstResponder(Some(&v));
                }
                v.setNeedsDisplay(true);
            }
        });
        let b = self.bounds();
        show(self, crate::ui::style::rect_of(b), self.ivars().model.clone(), self.ivars().colors.get(), Vec::new(), on_change, Some(on_close));
        self.setNeedsDisplay(true);
    }

    fn draw(&self) {
        let c = self.ivars().colors.get();
        let b = crate::ui::style::rect_of(self.bounds());
        let focused = self.window().and_then(|w| w.firstResponder()).is_some_and(|r| {
            let me: &AnyObject = self.as_ref();
            std::ptr::eq(&*r as *const _ as *const AnyObject, me as *const AnyObject)
        }) || is_open();
        style::fill_rounded(b.inset_by(0.5, 0.5), 6.0, style::input_fill(&c));
        let stroke = if focused { style::button_accent(&c) } else if self.ivars().hover.get() { c.dim.with_alpha(0.6) } else { style::input_stroke(&c) };
        style::stroke_rounded(b.inset_by(0.5, 0.5), 6.0, if focused { 1.5 } else { 1.0 }, stroke);
        let chev = Rect::new(b.width - 22.0, 0.0, 16.0, b.height);
        style::chevron(chev.inset_by(4.0, (b.height - 8.0) / 2.0), c.dim);
        let limit = chev.x - 2.0;
        let m = self.ivars().model.borrow();
        let items = m.display_items();
        let font = style::system_font(12.5, Weight::Regular);
        if items.is_empty() {
            let p = if m.options.is_empty() { format!("(no {}s cached)", m.noun) } else { m.placeholder.clone() };
            style::draw_text_left(&p, Rect::new(9.0, 0.0, (limit - 9.0).max(0.0), b.height), &font, c.dim.with_alpha(0.8));
            return;
        }
        let pill_font = style::system_font(11.5, Weight::Medium);
        let pill_h = b.height - 8.0;
        let width = |t: &str| (style::text_size(t, &pill_font).0 + 14.0).ceil();
        let mut x = 4.0;
        for (i, t) in items.iter().enumerate() {
            let rest = items.len() - i - 1;
            let more = if rest > 0 { width(&format!("+{rest}")) + 4.0 } else { 0.0 };
            let mut w = width(t).min(200.0);
            if x + w + more > limit {
                if i == 0 {
                    w = (limit - x - more).max(24.0);
                } else {
                    let t = format!("+{}", items.len() - i);
                    pill(&t, x, width(&t), pill_h, b.height, &pill_font, &c, true);
                    return;
                }
            }
            pill(t, x, w, pill_h, b.height, &pill_font, &c, false);
            x += w + 4.0;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn pill(t: &str, x: f64, w: f64, h: f64, bh: f64, f: &objc2_app_kit::NSFont, c: &PopupColors, dim: bool) {
    let r = Rect::new(x, (bh - h) / 2.0, w, h);
    style::fill_rounded(r, 4.0, c.text.with_alpha(if dim { 0.07 } else { 0.13 }));
    let col: Rgba = if dim { c.dim } else { c.text };
    style::draw_text_left(t, Rect::new(r.x + 7.0, r.y, r.width - 14.0, r.height), f, col);
}

/// Shorthand for building options from plain values.
pub fn options(vals: &[String]) -> Vec<PickOption> {
    vals.iter().map(|v| PickOption::new(v, v, "")).collect()
}
