//! Port of `ScreenshotAnnotations.swift` (the AppKit-free model half).
//!
//! Pure Foundation/std only. The CoreGraphics/CoreText/CoreImage drawing
//! pipeline (`ShotCanvas`, `ShotRenderer`, `ShotPixels::init?(CGImage)`,
//! `ShotText::font/lines/metrics`) is intentionally not ported — it needs
//! AppKit/CG and lands with the objc2 drawing pass. Everything geometric and
//! stateful (tools, ring layout, snap, pixelate math, file naming, CLI args,
//! persisted state) is here, mirroring the Swift semantics and the python
//! `pylib/shot_model.py` the Swift facades delegate to.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub fn round_half_away(x: f64) -> f64 {
    if x >= 0.0 {
        (x + 0.5).floor()
    } else {
        (x - 0.5).ceil()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const ZERO: Point = Point { x: 0.0, y: 0.0 };
    pub fn new(x: f64, y: f64) -> Self {
        Point { x, y }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Size {
    pub w: f64,
    pub h: f64,
}

impl Size {
    pub fn new(w: f64, h: f64) -> Self {
        Size { w, h }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub const ZERO: Rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 0.0,
        h: 0.0,
    };

    pub fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Rect { x, y, w, h }
    }
    pub fn min_x(&self) -> f64 {
        self.x
    }
    pub fn min_y(&self) -> f64 {
        self.y
    }
    pub fn max_x(&self) -> f64 {
        self.x + self.w
    }
    pub fn max_y(&self) -> f64 {
        self.y + self.h
    }
    pub fn mid_x(&self) -> f64 {
        self.x + self.w / 2.0
    }
    pub fn mid_y(&self) -> f64 {
        self.y + self.h / 2.0
    }
    pub fn is_empty(&self) -> bool {
        self.w <= 0.0 || self.h <= 0.0
    }

    pub fn inset(&self, dx: f64, dy: f64) -> Rect {
        Rect::new(self.x + dx, self.y + dy, self.w - 2.0 * dx, self.h - 2.0 * dy)
    }

    pub fn union(&self, o: &Rect) -> Rect {
        let x0 = self.min_x().min(o.min_x());
        let y0 = self.min_y().min(o.min_y());
        let x1 = self.max_x().max(o.max_x());
        let y1 = self.max_y().max(o.max_y());
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }

    pub fn intersection(&self, o: &Rect) -> Option<Rect> {
        let x0 = self.min_x().max(o.min_x());
        let y0 = self.min_y().max(o.min_y());
        let x1 = self.max_x().min(o.max_x());
        let y1 = self.max_y().min(o.max_y());
        if x1 <= x0 || y1 <= y0 {
            None
        } else {
            Some(Rect::new(x0, y0, x1 - x0, y1 - y0))
        }
    }

    pub fn intersects(&self, o: &Rect) -> bool {
        self.intersection(o).is_some()
    }

    // CGRectContainsPoint is half-open on the max edge.
    pub fn contains_point(&self, p: Point) -> bool {
        !self.is_empty()
            && p.x >= self.min_x()
            && p.x < self.max_x()
            && p.y >= self.min_y()
            && p.y < self.max_y()
    }

    // CGRectContainsRect is inclusive on both edges.
    pub fn contains_rect(&self, r: &Rect) -> bool {
        r.min_x() >= self.min_x()
            && r.max_x() <= self.max_x()
            && r.min_y() >= self.min_y()
            && r.max_y() <= self.max_y()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShotToolKind {
    Draw,
    Mode,
    Action,
    Info,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShotTool {
    Pencil,
    Line,
    Arrow,
    Selection,
    Rectangle,
    Circle,
    Marker,
    Text,
    Counter,
    Pixelate,
    Invert,
    Size,
    Move,
    Undo,
    Redo,
    Copy,
    Save,
    Accept,
    Exit,
    Pin,
    Recent,
    CopyText,
    SizeUp,
    SizeDown,
}

impl ShotTool {
    pub const ALL: [ShotTool; 24] = [
        ShotTool::Pencil,
        ShotTool::Line,
        ShotTool::Arrow,
        ShotTool::Selection,
        ShotTool::Rectangle,
        ShotTool::Circle,
        ShotTool::Marker,
        ShotTool::Text,
        ShotTool::Counter,
        ShotTool::Pixelate,
        ShotTool::Invert,
        ShotTool::Size,
        ShotTool::Move,
        ShotTool::Undo,
        ShotTool::Redo,
        ShotTool::Copy,
        ShotTool::Save,
        ShotTool::Accept,
        ShotTool::Exit,
        ShotTool::Pin,
        ShotTool::Recent,
        ShotTool::CopyText,
        ShotTool::SizeUp,
        ShotTool::SizeDown,
    ];

    pub const DEFAULT_BUTTONS: &'static str = "pencil, line, arrow, selection, rectangle, circle, marker, text, counter, pixelate, invert, move, undo, redo, copy, copy-text, save, exit, pin, recent";

    pub fn raw_value(&self) -> &'static str {
        match self {
            ShotTool::Pencil => "pencil",
            ShotTool::Line => "line",
            ShotTool::Arrow => "arrow",
            ShotTool::Selection => "selection",
            ShotTool::Rectangle => "rectangle",
            ShotTool::Circle => "circle",
            ShotTool::Marker => "marker",
            ShotTool::Text => "text",
            ShotTool::Counter => "counter",
            ShotTool::Pixelate => "pixelate",
            ShotTool::Invert => "invert",
            ShotTool::Size => "size",
            ShotTool::Move => "move",
            ShotTool::Undo => "undo",
            ShotTool::Redo => "redo",
            ShotTool::Copy => "copy",
            ShotTool::Save => "save",
            ShotTool::Accept => "accept",
            ShotTool::Exit => "exit",
            ShotTool::Pin => "pin",
            ShotTool::Recent => "recent",
            ShotTool::CopyText => "copy-text",
            ShotTool::SizeUp => "size-increase",
            ShotTool::SizeDown => "size-decrease",
        }
    }

    pub fn from_raw(s: &str) -> Option<ShotTool> {
        ShotTool::ALL.iter().copied().find(|t| t.raw_value() == s)
    }

    pub fn kind(&self) -> ShotToolKind {
        match self {
            ShotTool::Pencil
            | ShotTool::Line
            | ShotTool::Arrow
            | ShotTool::Selection
            | ShotTool::Rectangle
            | ShotTool::Circle
            | ShotTool::Marker
            | ShotTool::Text
            | ShotTool::Counter
            | ShotTool::Pixelate
            | ShotTool::Invert => ShotToolKind::Draw,
            ShotTool::Move => ShotToolKind::Mode,
            ShotTool::Size => ShotToolKind::Info,
            _ => ShotToolKind::Action,
        }
    }

    pub fn is_drawing(&self) -> bool {
        self.kind() == ShotToolKind::Draw
    }

    pub fn finishes(&self) -> bool {
        matches!(
            self,
            ShotTool::Copy
                | ShotTool::Save
                | ShotTool::Accept
                | ShotTool::Exit
                | ShotTool::Pin
                | ShotTool::CopyText
        )
    }

    pub fn letter(&self) -> Option<char> {
        match self {
            ShotTool::Pencil => Some('p'),
            ShotTool::Line => Some('d'),
            ShotTool::Arrow => Some('a'),
            ShotTool::Selection => Some('s'),
            ShotTool::Rectangle => Some('r'),
            ShotTool::Circle => Some('c'),
            ShotTool::Marker => Some('m'),
            ShotTool::Text => Some('t'),
            ShotTool::Pixelate => Some('b'),
            ShotTool::Invert => Some('i'),
            _ => None,
        }
    }

    pub fn for_letter(c: char) -> Option<ShotTool> {
        let c = c.to_ascii_lowercase();
        ShotTool::ALL.iter().copied().find(|t| t.letter() == Some(c))
    }

    pub fn symbol(&self) -> &'static str {
        match self {
            ShotTool::Pencil => "pencil",
            ShotTool::Line => "line.diagonal",
            ShotTool::Arrow => "arrow.down.left",
            ShotTool::Selection => "square",
            ShotTool::Rectangle => "square.fill",
            ShotTool::Circle => "circle",
            ShotTool::Marker => "highlighter",
            ShotTool::Text => "textformat",
            ShotTool::Counter => "1.circle",
            ShotTool::Pixelate => "square.grid.3x3.fill",
            ShotTool::Invert => "circle.lefthalf.filled",
            ShotTool::Size => "",
            ShotTool::Move => "arrow.up.and.down.and.arrow.left.and.right",
            ShotTool::Undo => "arrow.uturn.backward",
            ShotTool::Redo => "arrow.uturn.forward",
            ShotTool::Copy => "doc.on.doc",
            ShotTool::Save => "square.and.arrow.down",
            ShotTool::Accept => "checkmark",
            ShotTool::Exit => "xmark",
            ShotTool::Pin => "pin.fill",
            ShotTool::Recent => "clock",
            ShotTool::CopyText => "text.viewfinder",
            ShotTool::SizeUp => "plus",
            ShotTool::SizeDown => "minus",
        }
    }

    pub fn tooltip(&self) -> &'static str {
        match self {
            ShotTool::Pencil => "Set the Pencil as the paint tool (P)",
            ShotTool::Line => "Set the Line as the paint tool (D)",
            ShotTool::Arrow => "Set the Arrow as the paint tool (A)",
            ShotTool::Selection => "Set Selection as the paint tool (S)",
            ShotTool::Rectangle => "Set the Rectangle as the paint tool (R)",
            ShotTool::Circle => "Set the Circle as the paint tool (C)",
            ShotTool::Marker => "Set the Marker as the paint tool (M)",
            ShotTool::Text => "Add text to your capture (T)",
            ShotTool::Counter => "Add an autoincrementing counter bubble",
            ShotTool::Pixelate => "Set Pixelate as the paint tool (B)",
            ShotTool::Invert => "Set Inverter as the paint tool (I)",
            ShotTool::Size => "Selection size",
            ShotTool::Move => "Move the selection area (\u{2318}M)",
            ShotTool::Undo => "Undo the last modification (\u{2318}Z)",
            ShotTool::Redo => "Redo the next modification (\u{21e7}\u{2318}Z)",
            ShotTool::Copy => "Copy selection to clipboard (\u{2318}C)",
            ShotTool::Save => "Save screenshot to a file (\u{2318}S)",
            ShotTool::Accept => "Accept the capture (Return)",
            ShotTool::Exit => "Leave the capture screen (\u{2318}Q)",
            ShotTool::Pin => "Pin image on the desktop",
            ShotTool::Recent => "Recent screenshots",
            ShotTool::CopyText => "Copy the text in the selection (\u{21e7}\u{2318}C)",
            ShotTool::SizeUp => "Increase tool size",
            ShotTool::SizeDown => "Decrease tool size",
        }
    }

    pub fn default_size(&self) -> i64 {
        match self {
            ShotTool::Text => 8,
            ShotTool::Marker => 5,
            ShotTool::Pixelate => 2,
            ShotTool::Counter => 1,
            ShotTool::Rectangle => 1,
            _ => 3,
        }
    }

    pub fn size_range(&self) -> (i64, i64) {
        if *self == ShotTool::Rectangle {
            (0, 100)
        } else {
            (1, 100)
        }
    }

    /// Empty spec -> the default ring; `badge` inserts the size bubble after
    /// the last drawing tool (or drops it). Unknown + duplicate names dropped.
    pub fn ring(spec: &str, badge: bool) -> Vec<ShotTool> {
        let mut out: Vec<ShotTool> = Vec::new();
        for part in spec.split(',') {
            let n = part.trim().to_lowercase();
            if let Some(t) = ShotTool::from_raw(&n) {
                if !out.contains(&t) {
                    out.push(t);
                }
            }
        }
        if out.is_empty() {
            return ShotTool::ring(ShotTool::DEFAULT_BUTTONS, badge);
        }
        if !badge {
            out.retain(|t| *t != ShotTool::Size);
        } else if !out.contains(&ShotTool::Size) {
            let mut at = 0usize;
            for (i, t) in out.iter().enumerate() {
                if t.is_drawing() {
                    at = i + 1;
                }
            }
            out.insert(at, ShotTool::Size);
        }
        out
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShotColor {
    pub r: f64,
    pub g: f64,
    pub b: f64,
    pub a: f64,
}

impl ShotColor {
    pub const WHITE: ShotColor = ShotColor {
        r: 1.0,
        g: 1.0,
        b: 1.0,
        a: 1.0,
    };
    pub const BLACK: ShotColor = ShotColor {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    };

    pub fn new(r: f64, g: f64, b: f64) -> Self {
        ShotColor { r, g, b, a: 1.0 }
    }
    pub fn new_a(r: f64, g: f64, b: f64, a: f64) -> Self {
        ShotColor { r, g, b, a }
    }

    pub fn from_hex(hex: &str) -> Option<ShotColor> {
        let mut s = hex.trim();
        if let Some(rest) = s.strip_prefix('#') {
            s = rest;
        }
        if s.len() != 6 && s.len() != 8 {
            return None;
        }
        let v = u32::from_str_radix(s, 16).ok()?;
        let a = if s.len() == 8 {
            ((v >> 24) & 0xff) as f64 / 255.0
        } else {
            1.0
        };
        Some(ShotColor::new_a(
            ((v >> 16) & 0xff) as f64 / 255.0,
            ((v >> 8) & 0xff) as f64 / 255.0,
            (v & 0xff) as f64 / 255.0,
            a,
        ))
    }

    pub fn hex(&self) -> String {
        fn c(x: f64) -> i64 {
            round_half_away(x.max(0.0).min(1.0) * 255.0) as i64
        }
        format!("#{:02x}{:02x}{:02x}", c(self.r), c(self.g), c(self.b))
    }

    pub fn with_alpha(&self, alpha: f64) -> ShotColor {
        ShotColor::new_a(self.r, self.g, self.b, alpha)
    }

    pub fn luminance(&self) -> f64 {
        0.299 * self.r + 0.587 * self.g + 0.114 * self.b
    }

    pub fn is_dark(&self) -> bool {
        self.luminance() < 0.5
    }

    pub fn mixed(&self, o: &ShotColor, t: f64) -> ShotColor {
        ShotColor::new_a(
            self.r + (o.r - self.r) * t,
            self.g + (o.g - self.g) * t,
            self.b + (o.b - self.b) * t,
            self.a,
        )
    }

    pub fn from_hsv(h: f64, s: f64, v: f64) -> ShotColor {
        let i = ((h * 6.0).floor() as i64).rem_euclid(6);
        let f = h * 6.0 - (h * 6.0).floor();
        let p = v * (1.0 - s);
        let q = v * (1.0 - f * s);
        let t = v * (1.0 - (1.0 - f) * s);
        match i {
            0 => ShotColor::new(v, t, p),
            1 => ShotColor::new(q, v, p),
            2 => ShotColor::new(p, v, t),
            3 => ShotColor::new(p, q, v),
            4 => ShotColor::new(t, p, v),
            _ => ShotColor::new(v, p, q),
        }
    }

    pub fn hsv(&self) -> (f64, f64, f64) {
        let mx = self.r.max(self.g).max(self.b);
        let mn = self.r.min(self.g).min(self.b);
        let d = mx - mn;
        let mut h = 0.0;
        if d > 0.0 {
            if mx == self.r {
                h = ((self.g - self.b) / d) % 6.0;
            } else if mx == self.g {
                h = (self.b - self.r) / d + 2.0;
            } else {
                h = (self.r - self.g) / d + 4.0;
            }
            h /= 6.0;
            if h < 0.0 {
                h += 1.0;
            }
        }
        (h, if mx == 0.0 { 0.0 } else { d / mx }, mx)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShotTextStyle {
    #[serde(default)]
    pub family: String,
    #[serde(default)]
    pub bold: bool,
    #[serde(default)]
    pub italic: bool,
    #[serde(default)]
    pub underline: bool,
    #[serde(default)]
    pub strike: bool,
    #[serde(default)]
    pub align: i64,
}

impl Default for ShotTextStyle {
    fn default() -> Self {
        ShotTextStyle {
            family: String::new(),
            bold: false,
            italic: false,
            underline: false,
            strike: false,
            align: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ShotObject {
    pub tool: ShotTool,
    pub points: Vec<Point>,
    pub color: ShotColor,
    pub size: i64,
    pub text: String,
    pub style: ShotTextStyle,
    pub number_offset: i64,
    pub number: i64,
    pub open_arrow: bool,
    pub reversed: bool,
    pub outline: bool,
    pub secure: bool,
}

impl ShotObject {
    pub fn new(tool: ShotTool, points: Vec<Point>, color: ShotColor, size: i64) -> Self {
        ShotObject {
            tool,
            points,
            color,
            size,
            text: String::new(),
            style: ShotTextStyle::default(),
            number_offset: 0,
            number: 1,
            open_arrow: false,
            reversed: false,
            outline: true,
            secure: true,
        }
    }

    pub fn start(&self) -> Point {
        self.points.first().copied().unwrap_or(Point::ZERO)
    }
    pub fn end(&self) -> Point {
        self.points.last().copied().unwrap_or(Point::ZERO)
    }
    pub fn rect(&self) -> Rect {
        ShotGeom::rect(self.start(), self.end())
    }

    pub fn stroke_width(&self) -> f64 {
        match self.tool {
            ShotTool::Marker => (self.size * 2 + 2) as f64,
            _ => (self.size.max(1)) as f64,
        }
    }
    pub fn font_size(&self) -> f64 {
        self.size as f64 + 8.0
    }
    pub fn counter_radius(&self) -> f64 {
        self.size as f64 + 12.0
    }

    pub fn moved(&self, dx: f64, dy: f64) -> ShotObject {
        let mut o = self.clone();
        o.points = self
            .points
            .iter()
            .map(|p| Point::new(p.x + dx, p.y + dy))
            .collect();
        o
    }

    pub fn bbox(&self) -> Rect {
        match self.tool {
            ShotTool::Text => {
                let s = ShotText::box_size(self);
                Rect::new(self.start().x, self.start().y, s.w, s.h)
            }
            ShotTool::Counter => {
                let r = self.counter_radius();
                let mut b = Rect::new(
                    self.start().x - r,
                    self.start().y - r,
                    2.0 * r,
                    2.0 * r,
                );
                if self.points.len() > 1 {
                    b = b.union(&Rect::new(self.end().x, self.end().y, 0.0, 0.0));
                }
                b
            }
            ShotTool::Pencil | ShotTool::Line | ShotTool::Arrow | ShotTool::Marker => {
                let mut b = ShotGeom::bounds(&self.points);
                let pad = self.stroke_width() / 2.0
                    + if self.tool == ShotTool::Arrow {
                        ShotArrow::head_length(self.size) / 2.0
                    } else {
                        0.0
                    };
                b = b.inset(-pad, -pad);
                b
            }
            ShotTool::Selection | ShotTool::Circle => {
                let s = self.stroke_width() / 2.0;
                self.rect().inset(-s, -s)
            }
            _ => self.rect(),
        }
    }

    pub fn hit(&self, p: Point, slop: f64) -> bool {
        let w = self.stroke_width() / 2.0 + slop;
        match self.tool {
            ShotTool::Pencil | ShotTool::Marker => {
                if self.points.len() == 1 {
                    return ShotGeom::dist(p, self.start()) <= w;
                }
                for i in 1..self.points.len() {
                    if ShotGeom::seg_dist(p, self.points[i - 1], self.points[i]) <= w {
                        return true;
                    }
                }
                false
            }
            ShotTool::Line | ShotTool::Arrow => {
                if ShotGeom::seg_dist(p, self.start(), self.end()) <= w {
                    return true;
                }
                self.tool == ShotTool::Arrow
                    && ShotGeom::dist(p, if self.reversed { self.start() } else { self.end() })
                        <= ShotArrow::head_length(self.size)
            }
            ShotTool::Selection => {
                let r = self.rect();
                r.inset(-w, -w).contains_point(p) && !r.inset(w, w).contains_point(p)
            }
            ShotTool::Circle => {
                let r = self.rect();
                if r.w <= 0.0 || r.h <= 0.0 {
                    return ShotGeom::dist(p, Point::new(r.x, r.y)) <= w;
                }
                let dx = (p.x - r.mid_x()) / (r.w / 2.0);
                let dy = (p.y - r.mid_y()) / (r.h / 2.0);
                let d = (dx * dx + dy * dy).sqrt();
                let tol = w / (1.0f64).max(r.w.min(r.h) / 2.0);
                (d - 1.0).abs() <= tol
            }
            ShotTool::Counter => {
                if ShotGeom::dist(p, self.start()) <= self.counter_radius() + slop {
                    return true;
                }
                self.points.len() > 1
                    && ShotGeom::seg_dist(p, self.start(), self.end()) <= self.counter_radius() / 2.0 + slop
            }
            _ => self.bbox().inset(-slop, -slop).contains_point(p),
        }
    }
}

pub struct ShotDocument {
    pub objects: Vec<ShotObject>,
    pub selected: Option<usize>,
    pub undo_limit: usize,
    undo_stack: Vec<(Vec<ShotObject>, Option<String>)>,
    redo_stack: Vec<Vec<ShotObject>>,
    last_coalesce: Option<String>,
}

impl ShotDocument {
    pub fn new(undo_limit: usize) -> Self {
        ShotDocument {
            objects: Vec::new(),
            selected: None,
            undo_limit: undo_limit.max(1),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            last_coalesce: None,
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }
    pub fn undo_depth(&self) -> usize {
        self.undo_stack.len()
    }

    pub fn commit<F: FnOnce(&mut Vec<ShotObject>)>(&mut self, coalesce: Option<&str>, change: F) {
        let take = coalesce.is_none() || coalesce != self.last_coalesce.as_deref();
        if take {
            self.undo_stack
                .push((self.objects.clone(), coalesce.map(|s| s.to_string())));
            if self.undo_stack.len() > self.undo_limit {
                let excess = self.undo_stack.len() - self.undo_limit;
                self.undo_stack.drain(0..excess);
            }
        }
        self.last_coalesce = coalesce.map(|s| s.to_string());
        change(&mut self.objects);
        ShotDocument::renumber(&mut self.objects);
        self.redo_stack.clear();
        if let Some(s) = self.selected {
            if s >= self.objects.len() {
                self.selected = None;
            }
        }
    }

    pub fn add(&mut self, o: ShotObject) -> usize {
        self.commit(None, |v| v.push(o));
        self.objects.len() - 1
    }

    pub fn remove(&mut self, i: usize) {
        if i < self.objects.len() {
            self.commit(None, |v| {
                v.remove(i);
            });
            self.selected = None;
        }
    }

    pub fn update<F: FnOnce(&mut ShotObject)>(&mut self, i: usize, coalesce: Option<&str>, f: F) {
        if i < self.objects.len() {
            self.commit(coalesce, |v| f(&mut v[i]));
        }
    }

    pub fn reorder(&mut self, from: usize, to: usize) {
        if from < self.objects.len() && to < self.objects.len() && from != to {
            self.commit(None, |v| {
                let o = v.remove(from);
                v.insert(to, o);
            });
            self.selected = Some(to);
        }
    }

    pub fn undo(&mut self) -> bool {
        match self.undo_stack.pop() {
            Some((prev, _)) => {
                self.redo_stack.push(self.objects.clone());
                self.objects = prev;
                self.last_coalesce = None;
                self.selected = None;
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self) -> bool {
        match self.redo_stack.pop() {
            Some(next) => {
                self.undo_stack.push((self.objects.clone(), None));
                self.objects = next;
                self.last_coalesce = None;
                self.selected = None;
                true
            }
            None => false,
        }
    }

    pub fn update_live<F: FnOnce(&mut ShotObject)>(&mut self, i: usize, f: F) {
        if i < self.objects.len() {
            f(&mut self.objects[i]);
        }
    }

    pub fn reset(&mut self) {
        self.objects = Vec::new();
        self.undo_stack = Vec::new();
        self.redo_stack = Vec::new();
        self.selected = None;
        self.last_coalesce = None;
    }

    pub fn hit(&self, p: Point) -> Option<usize> {
        self.objects
            .iter()
            .enumerate()
            .rev()
            .find(|(_, o)| o.hit(p, 4.0))
            .map(|(i, _)| i)
    }

    pub fn next_counter_number(&self, offset: i64) -> i64 {
        let last = self
            .objects
            .iter()
            .rev()
            .find(|o| o.tool == ShotTool::Counter)
            .map(|o| o.number)
            .unwrap_or(0);
        999.min(1.max(last + 1 + offset))
    }

    pub fn renumber(objs: &mut [ShotObject]) {
        let mut n = 0i64;
        for o in objs.iter_mut() {
            if o.tool == ShotTool::Counter {
                n = 999.min(1.max(n + 1 + o.number_offset));
                o.number = n;
            }
        }
    }
}

pub struct ShotGeom;

impl ShotGeom {
    pub fn rect(a: Point, b: Point) -> Rect {
        Rect::new(
            a.x.min(b.x),
            a.y.min(b.y),
            (b.x - a.x).abs(),
            (b.y - a.y).abs(),
        )
    }

    pub fn bounds(pts: &[Point]) -> Rect {
        match pts.first() {
            None => Rect::ZERO,
            Some(f) => {
                let mut r = Rect::new(f.x, f.y, 0.0, 0.0);
                for p in pts.iter().skip(1) {
                    r = r.union(&Rect::new(p.x, p.y, 0.0, 0.0));
                }
                r
            }
        }
    }

    pub fn dist(a: Point, b: Point) -> f64 {
        (a.x - b.x).hypot(a.y - b.y)
    }

    pub fn seg_dist(p: Point, a: Point, b: Point) -> f64 {
        let dx = b.x - a.x;
        let dy = b.y - a.y;
        let l2 = dx * dx + dy * dy;
        if l2 <= 0.0 {
            return ShotGeom::dist(p, a);
        }
        let t = (0.0f64)
            .max(1.0f64.min(((p.x - a.x) * dx + (p.y - a.y) * dy) / l2));
        ShotGeom::dist(p, Point::new(a.x + t * dx, a.y + t * dy))
    }
}

pub struct ShotSnap;

impl ShotSnap {
    pub fn angle(from: Point, to: Point) -> Point {
        let dx = to.x - from.x;
        let dy = to.y - from.y;
        let len = dx.hypot(dy);
        if len <= 0.0 {
            return to;
        }
        let step = std::f64::consts::PI / 4.0;
        let ang = round_half_away((dy.atan2(dx)) / step) * step;
        let mut x = from.x + ang.cos() * len;
        let mut y = from.y + ang.sin() * len;
        if (x - from.x).abs() < 1e-9 {
            x = from.x;
        }
        if (y - from.y).abs() < 1e-9 {
            y = from.y;
        }
        Point::new(x, y)
    }

    pub fn square(from: Point, to: Point) -> Point {
        let dx = to.x - from.x;
        let dy = to.y - from.y;
        let s = dx.abs().max(dy.abs());
        Point::new(
            from.x + if dx < 0.0 { -s } else { s },
            from.y + if dy < 0.0 { -s } else { s },
        )
    }

    pub fn snap(tool: ShotTool, a: Point, b: Point) -> Point {
        match tool {
            ShotTool::Line | ShotTool::Arrow | ShotTool::Marker => ShotSnap::angle(a, b),
            ShotTool::Selection
            | ShotTool::Rectangle
            | ShotTool::Circle
            | ShotTool::Pixelate
            | ShotTool::Invert => ShotSnap::square(a, b),
            _ => b,
        }
    }
}

pub struct RingLayout {
    pub frames: Vec<Rect>,
    pub inside: bool,
}

pub struct ButtonRing;

impl ButtonRing {
    pub fn default_button_size(line_height: f64) -> f64 {
        round_half_away(line_height * 2.2)
    }

    fn place(
        frames: &mut [Option<Rect>],
        idx: &mut usize,
        count: usize,
        base: f64,
        pts: &[Point],
    ) {
        for p in pts {
            if *idx >= count {
                break;
            }
            frames[*idx] = Some(Rect::new(p.x, p.y, base, base));
            *idx += 1;
        }
    }

    fn horizontal(c: Point, n: usize, left_to_right: bool, ext: f64, sep: f64, base: f64) -> Vec<Point> {
        let mut shift = if n % 2 == 0 {
            ext * ((n / 2) as f64) - sep / 2.0
        } else {
            ext * (((n - 1) / 2) as f64) + base / 2.0
        };
        if !left_to_right {
            shift -= base;
        }
        let mut x = if left_to_right { c.x - shift } else { c.x + shift };
        let mut out = Vec::with_capacity(n);
        while out.len() < n {
            out.push(Point::new(x, c.y));
            x += if left_to_right { ext } else { -ext };
        }
        out
    }

    fn vertical(c: Point, n: usize, up_to_down: bool, ext: f64, sep: f64, base: f64) -> Vec<Point> {
        let mut shift = if n % 2 == 0 {
            ext * ((n / 2) as f64) - sep / 2.0
        } else {
            ext * (((n - 1) / 2) as f64) + base / 2.0
        };
        if !up_to_down {
            shift -= base;
        }
        let mut y = if up_to_down { c.y - shift } else { c.y + shift };
        let mut out = Vec::with_capacity(n);
        while out.len() < n {
            out.push(Point::new(c.x, y));
            y += if up_to_down { ext } else { -ext };
        }
        out
    }

    pub fn layout(selection: Rect, screen: Rect, count: usize, button: f64) -> RingLayout {
        let mut frames = vec![None; count];
        if count == 0 {
            return RingLayout {
                frames: Vec::new(),
                inside: false,
            };
        }
        let base = button;
        let sep = (base / 4.0).floor();
        let ext = base + sep;
        let mut sel = selection.intersection(&screen).unwrap_or_else(|| {
            Rect::new(selection.min_x(), selection.min_y(), 0.0, 0.0)
        });
        let mut idx = 0usize;
        let mut inside = false;

        if sel.w < base {
            sel.x -= ((base - sel.w) / 2.0).floor();
            sel.w = base;
        }
        if sel.h < base {
            sel.y -= ((base - sel.h) / 2.0).floor();
            sel.h = base;
        }
        sel.x = screen.min_x().max(sel.min_x().min(screen.max_x() - sel.w));
        sel.y = screen.min_y().max(sel.min_y().min(screen.max_y() - sel.h));

        let mut guard_loops = 0;
        while idx < count && guard_loops < 64 {
            guard_loops += 1;
            let e = sep * 2.0 + base;

            let on_screen = |a: Point, b: Point| -> bool {
                let s = screen.inset(-0.5, -0.5);
                s.contains_point(a) && s.contains_point(b)
            };

            let b_right = !on_screen(
                Point::new(sel.max_x() + e, sel.max_y()),
                Point::new(sel.max_x() + e, sel.min_y()),
            );
            let b_left = !on_screen(
                Point::new(sel.min_x() - e, sel.max_y()),
                Point::new(sel.min_x() - e, sel.min_y()),
            );
            let b_bottom = !on_screen(
                Point::new(sel.min_x(), sel.max_y() + e),
                Point::new(sel.max_x(), sel.max_y() + e),
            );
            let b_top = !on_screen(
                Point::new(sel.min_x(), sel.min_y() - e),
                Point::new(sel.max_x(), sel.min_y() - e),
            );
            let one_horizontal = b_right != b_left;
            let both_horizontal = b_right && b_left;

            if b_left && both_horizontal && b_bottom && b_top {
                let mut area = sel
                    .intersection(&screen)
                    .unwrap_or(Rect::new(0.0, 0.0, 0.0, 0.0));
                if (area.w / ext) as i64 == 0 {
                    area = screen;
                }
                let per_row = (area.w / ext) as i64;
                if per_row <= 0 {
                    break;
                }
                let mut c = Point::new(area.mid_x(), area.max_y() - ext);
                while idx < count {
                    let n = per_row.min(count as i64 - idx as i64).max(0) as usize;
                    ButtonRing::place(
                        &mut frames,
                        &mut idx,
                        count,
                        base,
                        &ButtonRing::horizontal(c, n, true, ext, sep, base),
                    );
                    c.y -= ext;
                }
                inside = true;
                break;
            }

            let per_row = ((sel.w + sep) / ext) as i64;
            let per_col = ((sel.h + sep) / ext) as i64;
            let extra = (count as i64 - idx as i64) - (per_row + per_col) * 2;
            let mut corners = 4i64.min(extra);
            let max_extra = if one_horizontal {
                1
            } else if both_horizontal {
                0
            } else {
                2
            };
            let corners_top = 0i64.max(corners.min(max_extra));
            corners -= corners_top;
            let corners_bottom = 0i64.max(corners.min(max_extra));

            if !b_bottom {
                let n = 0i64
                    .max((per_row + corners_bottom).min(count as i64 - idx as i64))
                    as usize;
                let mut c = Point::new(sel.mid_x(), sel.max_y() + sep);
                if n as i64 > per_row {
                    if b_left {
                        c.x += (ext / 2.0).floor();
                    } else if b_right {
                        c.x -= (ext / 2.0).floor();
                    }
                }
                ButtonRing::place(
                    &mut frames,
                    &mut idx,
                    count,
                    base,
                    &ButtonRing::horizontal(c, n, true, ext, sep, base),
                );
            }
            if !b_right && idx < count {
                let n = 0i64.max(per_col.min(count as i64 - idx as i64)) as usize;
                ButtonRing::place(
                    &mut frames,
                    &mut idx,
                    count,
                    base,
                    &ButtonRing::vertical(
                        Point::new(sel.max_x() + sep, sel.mid_y()),
                        n,
                        false,
                        ext,
                        sep,
                        base,
                    ),
                );
            }
            if !b_top && idx < count {
                let n = 0i64
                    .max((per_row + corners_top).min(count as i64 - idx as i64))
                    as usize;
                let mut c = Point::new(sel.mid_x(), sel.min_y() - ext);
                if n as i64 == per_row + 1 {
                    if b_left {
                        c.x += (ext / 2.0).floor();
                    } else if b_right {
                        c.x -= (ext / 2.0).floor();
                    }
                }
                ButtonRing::place(
                    &mut frames,
                    &mut idx,
                    count,
                    base,
                    &ButtonRing::horizontal(c, n, false, ext, sep, base),
                );
            }
            if !b_left && idx < count {
                let n = 0i64.max(per_col.min(count as i64 - idx as i64)) as usize;
                ButtonRing::place(
                    &mut frames,
                    &mut idx,
                    count,
                    base,
                    &ButtonRing::vertical(
                        Point::new(sel.min_x() - ext, sel.mid_y()),
                        n,
                        true,
                        ext,
                        sep,
                        base,
                    ),
                );
            }
            if idx < count {
                let grown = Rect::new(sel.x - ext, sel.y - ext, sel.w + 2.0 * ext, sel.h + 2.0 * ext);
                match grown.intersection(&screen) {
                    Some(s) => sel = s,
                    None => break,
                }
            }
        }

        RingLayout {
            frames: frames
                .into_iter()
                .map(|f| f.unwrap_or(Rect::ZERO))
                .collect(),
            inside,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ShotPixels {
    pub width: i64,
    pub height: i64,
    pub data: Vec<u8>,
}

impl ShotPixels {
    pub fn new(width: i64, height: i64, data: Vec<u8>) -> Self {
        ShotPixels {
            width,
            height,
            data,
        }
    }

    pub fn color(&self, x: i64, y: i64) -> ShotColor {
        let cx = x.max(0).min(self.width - 1);
        let cy = y.max(0).min(self.height - 1);
        let i = ((cy * self.width + cx) * 4) as usize;
        ShotColor::new(
            self.data[i] as f64 / 255.0,
            self.data[i + 1] as f64 / 255.0,
            self.data[i + 2] as f64 / 255.0,
        )
    }
}

pub struct ShotPixelate;

impl ShotPixelate {
    pub fn grid(r: Rect, size: i64) -> (usize, usize) {
        let f = 0.5 / (size.max(1) + 1) as f64;
        let cols = 1.max(round_half_away(r.w * f) as i64) as usize;
        let rows = 1.max(round_half_away(r.h * f) as i64) as usize;
        (cols, rows)
    }

    fn span(i: i64, n: i64, a: i64, b: i64) -> Vec<i64> {
        let lo = a + ((i as f64 / n as f64) * (b - a) as f64) as i64;
        let hi = a + (((i + 1) as f64 / n as f64) * (b - a) as f64) as i64 - 1;
        let hi = hi.max(lo);
        (lo..=hi).collect()
    }

    fn stepped(r: &[i64]) -> Vec<i64> {
        let st = 1.max(r.len() / 8) as usize;
        r.iter().step_by(st).copied().collect()
    }

    fn mean(
        pts: &[(i64, i64)],
        pixels: &ShotPixels,
        x0: i64,
        y0: i64,
        x1: i64,
        y1: i64,
    ) -> Option<ShotColor> {
        let mut r = 0.0;
        let mut g = 0.0;
        let mut b = 0.0;
        let mut n = 0.0;
        for &(x, y) in pts {
            let in_x = x >= 0 && x < pixels.width;
            let in_y = y >= 0 && y < pixels.height;
            if in_x && in_y && !(x >= x0 && x < x1 && y >= y0 && y < y1) {
                let c = pixels.color(x, y);
                r += c.r;
                g += c.g;
                b += c.b;
                n += 1.0;
            }
        }
        if n > 0.0 {
            Some(ShotColor::new(r / n, g / n, b / n))
        } else {
            None
        }
    }

    fn smooth(a: &[Option<ShotColor>]) -> Vec<Option<ShotColor>> {
        let mut out = Vec::with_capacity(a.len());
        for k in 0..a.len() {
            let mut near: Vec<ShotColor> = Vec::new();
            for i in [k as i64 - 1, k as i64, k as i64 + 1] {
                if i >= 0 && (i as usize) < a.len() {
                    if let Some(c) = a[i as usize] {
                        near.push(c);
                    }
                }
            }
            if near.is_empty() {
                out.push(None);
                continue;
            }
            let n = near.len() as f64;
            out.push(Some(ShotColor::new(
                near.iter().map(|c| c.r).sum::<f64>() / n,
                near.iter().map(|c| c.g).sum::<f64>() / n,
                near.iter().map(|c| c.b).sum::<f64>() / n,
            )));
        }
        out
    }

    fn avg(cs: &[(ShotColor, f64)]) -> Option<ShotColor> {
        let w: f64 = cs.iter().map(|(_, t)| t).sum();
        if w <= 0.0 {
            return None;
        }
        Some(ShotColor::new(
            cs.iter().map(|(c, t)| c.r * t).sum::<f64>() / w,
            cs.iter().map(|(c, t)| c.g * t).sum::<f64>() / w,
            cs.iter().map(|(c, t)| c.b * t).sum::<f64>() / w,
        ))
    }

    pub fn secure_blocks(pixels: &ShotPixels, px: Rect, size: i64, scale: f64) -> Vec<Vec<ShotColor>> {
        let s = scale.max(1.0);
        let (cols, rows) = ShotPixelate::grid(
            Rect::new(0.0, 0.0, px.w / s, px.h / s),
            size,
        );
        let x0 = px.min_x().floor() as i64;
        let y0 = px.min_y().floor() as i64;
        let x1 = px.max_x().ceil() as i64;
        let y1 = px.max_y().ceil() as i64;
        let band = [1i64, 2, 3, 4];

        let mut top: Vec<Option<ShotColor>> = Vec::new();
        let mut bottom: Vec<Option<ShotColor>> = Vec::new();
        for i in 0..cols {
            let xs = ShotPixelate::stepped(&ShotPixelate::span(i as i64, cols as i64, x0, x1));
            let mut top_pts = Vec::new();
            let mut bottom_pts = Vec::new();
            for x in &xs {
                for k in band {
                    top_pts.push((*x, y0 - k));
                    bottom_pts.push((*x, y1 - 1 + k));
                }
            }
            top.push(ShotPixelate::mean(&top_pts, pixels, x0, y0, x1, y1));
            bottom.push(ShotPixelate::mean(&bottom_pts, pixels, x0, y0, x1, y1));
        }
        let mut left: Vec<Option<ShotColor>> = Vec::new();
        let mut right: Vec<Option<ShotColor>> = Vec::new();
        for j in 0..rows {
            let ys = ShotPixelate::stepped(&ShotPixelate::span(j as i64, rows as i64, y0, y1));
            let mut left_pts = Vec::new();
            let mut right_pts = Vec::new();
            for y in &ys {
                for k in band {
                    left_pts.push((x0 - k, *y));
                    right_pts.push((x1 - 1 + k, *y));
                }
            }
            left.push(ShotPixelate::mean(&left_pts, pixels, x0, y0, x1, y1));
            right.push(ShotPixelate::mean(&right_pts, pixels, x0, y0, x1, y1));
        }
        let top = ShotPixelate::smooth(&top);
        let bottom = ShotPixelate::smooth(&bottom);
        let left = ShotPixelate::smooth(&left);
        let right = ShotPixelate::smooth(&right);

        let gray = ShotColor::new(0.5, 0.5, 0.5);
        let mut seed: u32 =
            ((x0.wrapping_mul(73_856_093)) as u32 ^ (y0.wrapping_mul(19_349_663)) as u32) | 1;
        let mut noise = || {
            seed ^= seed.wrapping_shl(13);
            seed ^= seed >> 17;
            seed ^= seed.wrapping_shl(5);
            ((seed % 1000) as f64 / 1000.0 - 0.5) * 0.05
        };

        let mut out: Vec<Vec<ShotColor>> = Vec::with_capacity(rows);
        for j in 0..rows {
            let mut row = Vec::with_capacity(cols);
            let v = if rows == 1 {
                0.5
            } else {
                j as f64 / (rows - 1) as f64
            };
            for i in 0..cols {
                let u = if cols == 1 {
                    0.5
                } else {
                    i as f64 / (cols - 1) as f64
                };
                let mut parts: Vec<(ShotColor, f64)> = Vec::new();
                if let Some(c) = top[i] {
                    parts.push((c, 1.0 - v + 0.001));
                }
                if let Some(c) = bottom[i] {
                    parts.push((c, v + 0.001));
                }
                if let Some(c) = left[j] {
                    parts.push((c, 1.0 - u + 0.001));
                }
                if let Some(c) = right[j] {
                    parts.push((c, u + 0.001));
                }
                let c = ShotPixelate::avg(&parts).unwrap_or(gray);
                let n = noise();
                row.push(ShotColor::new(
                    (0.0f64).max(1.0f64.min(c.r + n)),
                    (0.0f64).max(1.0f64.min(c.g + n)),
                    (0.0f64).max(1.0f64.min(c.b + n)),
                ));
            }
            out.push(row);
        }
        out
    }
}

pub struct ShotArrow;

impl ShotArrow {
    pub fn head_length(size: i64) -> f64 {
        (3 * size + 10) as f64
    }
    pub fn head_width(size: i64) -> f64 {
        (2 * size + 6) as f64
    }
    pub fn head(from: Point, to: Point, size: i64) -> (Point, Point, Point, Point) {
        let len = 1.0f64.max(ShotGeom::dist(from, to));
        let ux = (to.x - from.x) / len;
        let uy = (to.y - from.y) / len;
        let hl = ShotArrow::head_length(size).min(len);
        let hw = ShotArrow::head_width(size) / 2.0;
        let base = Point::new(to.x - ux * hl, to.y - uy * hl);
        let l = Point::new(base.x - uy * hw, base.y + ux * hw);
        let r = Point::new(base.x + uy * hw, base.y - ux * hw);
        (to, l, r, base)
    }
}

/// Text metrics need CoreText; the only formula here is a documented
/// approximation used by `ShotObject::bbox` for `.text` objects. The exact
/// `ShotText.font/lines/metrics` pipeline is not ported.
pub struct ShotText;

impl ShotText {
    pub const PADDING: f64 = 5.0;

    pub fn box_size(o: &ShotObject) -> Size {
        let line_height = (o.font_size() * 1.2).ceil();
        let lines: Vec<&str> = o.text.split('\n').collect();
        let mut max_w = line_height / 2.0;
        for l in &lines {
            let w = l.chars().count() as f64 * o.font_size() * 0.5;
            if w > max_w {
                max_w = w;
            }
        }
        Size::new(
            max_w.ceil() + ShotText::PADDING * 2.0,
            line_height * lines.len().max(1) as f64 + ShotText::PADDING * 2.0,
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShotDate {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

impl ShotDate {
    pub fn from_unix_local(secs: i64) -> ShotDate {
        unsafe {
            let t = secs as libc::time_t;
            let mut tm: libc::tm = std::mem::zeroed();
            libc::localtime_r(&t, &mut tm);
            ShotDate {
                year: tm.tm_year as i64 + 1900,
                month: (tm.tm_mon + 1) as u32,
                day: tm.tm_mday as u32,
                hour: tm.tm_hour as u32,
                minute: tm.tm_min as u32,
                second: tm.tm_sec as u32,
            }
        }
    }

    pub fn strftime(&self, fmt: &str) -> String {
        let chars: Vec<char> = fmt.chars().collect();
        let mut out = String::new();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            if c == '%' && i + 1 < chars.len() {
                match chars[i + 1] {
                    '%' => out.push('%'),
                    'F' => out.push_str(&format!(
                        "{:04}-{:02}-{:02}",
                        self.year, self.month, self.day
                    )),
                    'Y' => out.push_str(&format!("{:04}", self.year)),
                    'y' => out.push_str(&format!("{:02}", self.year.rem_euclid(100))),
                    'm' => out.push_str(&format!("{:02}", self.month)),
                    'd' => out.push_str(&format!("{:02}", self.day)),
                    'H' => out.push_str(&format!("{:02}", self.hour)),
                    'M' => out.push_str(&format!("{:02}", self.minute)),
                    'S' => out.push_str(&format!("{:02}", self.second)),
                    other => {
                        out.push('%');
                        out.push(other);
                    }
                }
                i += 2;
            } else {
                out.push(c);
                i += 1;
            }
        }
        out
    }
}

pub struct ShotFiles;

impl ShotFiles {
    pub fn expand(pattern: &str) -> String {
        ShotFiles::expand_at(pattern, ShotFiles::unix_now())
    }

    pub fn expand_at(pattern: &str, secs: i64) -> String {
        ShotFiles::expand_date(pattern, ShotDate::from_unix_local(secs))
    }

    pub fn expand_date(pattern: &str, date: ShotDate) -> String {
        let pattern = if pattern.is_empty() {
            "%F_%H-%M"
        } else {
            pattern
        };
        let replaced = pattern.replace("%F", "%Y-%m-%d");
        let expanded = date.strftime(&replaced);
        let s = if expanded.is_empty() {
            "screenshot".to_string()
        } else {
            expanded
        };
        s.replace('/', "-")
    }

    pub fn unique_path(dir: &str, name: &str, ext: &str) -> String {
        ShotFiles::unique_path_with(dir, name, ext, &|p: &str| {
            std::path::Path::new(p).exists()
        })
    }

    pub fn unique_path_with<F: Fn(&str) -> bool>(
        dir: &str,
        name: &str,
        ext: &str,
        exists: &F,
    ) -> String {
        let d = if dir.ends_with('/') {
            &dir[..dir.len() - 1]
        } else {
            dir
        };
        let e = if ext.is_empty() {
            String::new()
        } else {
            format!(".{}", ext)
        };
        let mut p = format!("{}/{}{}", d, name, e);
        let mut n = 2;
        while exists(&p) && n < 10_000 {
            p = format!("{}/{} {}{}", d, name, n, e);
            n += 1;
        }
        p
    }

    pub fn target(path: &str, pattern: &str, format: &str) -> String {
        ShotFiles::target_with(
            path,
            pattern,
            format,
            ShotFiles::unix_now(),
            &|p: &str| std::path::Path::new(p).is_dir(),
            &|p: &str| std::path::Path::new(p).exists(),
        )
    }

    pub fn target_with<D: Fn(&str) -> bool, E: Fn(&str) -> bool>(
        path: &str,
        pattern: &str,
        format: &str,
        secs: i64,
        is_dir: &D,
        exists: &E,
    ) -> String {
        let p = ShotFiles::expand_tilde(path);
        if is_dir(&p) || p.ends_with('/') {
            return ShotFiles::unique_path_with(
                &p,
                &ShotFiles::expand_at(pattern, secs),
                format,
                exists,
            );
        }
        let base = p.rsplit('/').next().unwrap_or("");
        let ext = if !base.contains('.') || (base.starts_with('.') && base.matches('.').count() == 1)
        {
            ""
        } else {
            base.rsplit('.').next().unwrap_or("")
        };
        if ext.is_empty() {
            format!("{}.{}", p, format)
        } else {
            p
        }
    }

    fn expand_tilde(p: &str) -> String {
        if p == "~" {
            std::env::var("HOME").unwrap_or_else(|_| p.to_string())
        } else if let Some(rest) = p.strip_prefix("~/") {
            match std::env::var("HOME") {
                Ok(home) => format!("{}/{}", home, rest),
                Err(_) => p.to_string(),
            }
        } else {
            p.to_string()
        }
    }

    fn unix_now() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShotMode {
    Gui,
    Text,
    Full,
    Screen,
}

impl ShotMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            ShotMode::Gui => "gui",
            ShotMode::Text => "text",
            ShotMode::Full => "full",
            ShotMode::Screen => "screen",
        }
    }
    pub fn parse(s: &str) -> Option<ShotMode> {
        match s {
            "gui" => Some(ShotMode::Gui),
            "text" => Some(ShotMode::Text),
            "full" => Some(ShotMode::Full),
            "screen" => Some(ShotMode::Screen),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ShotArgs {
    pub mode: ShotMode,
    pub path: Option<String>,
    pub clipboard: bool,
    pub delay_ms: i64,
    pub region: Option<String>,
    pub last_region: bool,
    pub accept_on_select: bool,
    pub pin: bool,
    pub raw: bool,
    pub print_geometry: bool,
    pub screen_number: Option<i64>,
}

impl Default for ShotArgs {
    fn default() -> Self {
        ShotArgs {
            mode: ShotMode::Gui,
            path: None,
            clipboard: false,
            delay_ms: 0,
            region: None,
            last_region: false,
            accept_on_select: false,
            pin: false,
            raw: false,
            print_geometry: false,
            screen_number: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShotArgsProblem {
    pub message: String,
}

impl ShotArgs {
    pub fn is_overlay(&self) -> bool {
        matches!(self.mode, ShotMode::Gui | ShotMode::Text)
    }
    pub fn wants_reply(&self) -> bool {
        self.raw || self.print_geometry
    }

    fn scan_number(b: &[u8], mut i: usize) -> Option<(f64, usize)> {
        let start = i;
        if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
            i += 1;
        }
        let mut int_digits = 0;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
            int_digits += 1;
        }
        let mut frac_digits = 0;
        if i < b.len() && b[i] == b'.' {
            i += 1;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
                frac_digits += 1;
            }
        }
        if int_digits == 0 && frac_digits == 0 {
            return None;
        }
        let tok = std::str::from_utf8(&b[start..i]).ok()?;
        tok.parse::<f64>().ok().map(|v| (v, i))
    }

    pub fn parse_region(s: &str) -> Option<Rect> {
        let t = s.trim();
        let b = t.as_bytes();
        let (w, mut i) = ShotArgs::scan_number(b, 0)?;
        if i >= b.len() || b[i] != b'x' {
            return None;
        }
        let (h, j) = ShotArgs::scan_number(b, i + 1)?;
        i = j;
        let mut x = 0.0;
        let mut y = 0.0;
        if i < b.len() {
            if b[i] != b'+' {
                return None;
            }
            let (xx, j) = ShotArgs::scan_number(b, i + 1)?;
            if j >= b.len() || b[j] != b'+' {
                return None;
            }
            let (yy, k) = ShotArgs::scan_number(b, j + 1)?;
            x = xx;
            y = yy;
            i = k;
        }
        if i != b.len() {
            return None;
        }
        if w <= 0.0 || h <= 0.0 {
            return None;
        }
        Some(Rect::new(x, y, w, h))
    }

    pub fn parse<S: AsRef<str>>(words: &[S]) -> Result<ShotArgs, ShotArgsProblem> {
        let words: Vec<&str> = words.iter().map(|w| w.as_ref()).collect();
        let mut a = ShotArgs::default();
        let mut i = 0usize;
        if let Some(first) = words.first() {
            if let Some(m) = ShotMode::parse(first) {
                a.mode = m;
                i = 1;
            }
        }
        let err = |m: String| ShotArgsProblem { message: m };
        let is_digits = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());

        while i < words.len() {
            let w = words[i];
            match w {
                "-p" | "--path" => {
                    if i + 1 >= words.len() {
                        return Err(err(format!("{} needs a path", w)));
                    }
                    i += 1;
                    a.path = Some(words[i].to_string());
                }
                "-c" | "--clipboard" => a.clipboard = true,
                "-d" | "--delay" => {
                    if i + 1 >= words.len() || !is_digits(words[i + 1]) {
                        return Err(err(format!("{} needs milliseconds", w)));
                    }
                    i += 1;
                    a.delay_ms = words[i].parse().unwrap_or(0);
                }
                "--region" => {
                    if i + 1 >= words.len() {
                        return Err(err("--region WxH+X+Y | screenN".to_string()));
                    }
                    let v = words[i + 1];
                    if ShotArgs::parse_region(v).is_none() && !v.starts_with("screen") {
                        return Err(err("--region WxH+X+Y | screenN".to_string()));
                    }
                    i += 1;
                    a.region = Some(v.to_string());
                }
                "--last-region" => a.last_region = true,
                "-s" | "--accept-on-select" => a.accept_on_select = true,
                "--pin" => a.pin = true,
                "-r" | "--raw" => a.raw = true,
                "-g" | "--print-geometry" => a.print_geometry = true,
                "-n" | "--number" => {
                    if i + 1 >= words.len() || !is_digits(words[i + 1]) {
                        return Err(err(format!("{} needs a screen number", w)));
                    }
                    i += 1;
                    a.screen_number = words[i].parse().ok();
                }
                _ => return Err(err(format!("unknown option {}", w))),
            }
            i += 1;
        }
        Ok(a)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShotRegion {
    pub display: u32,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

fn default_grid_size() -> i64 {
    10
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShotState {
    #[serde(default)]
    pub sizes: HashMap<String, i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default)]
    pub style: ShotTextStyle,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "lastRegion")]
    pub last_region: Option<ShotRegion>,
    #[serde(default = "default_grid_size", rename = "gridSize")]
    pub grid_size: i64,
    #[serde(default)]
    pub grid: bool,
}

impl Default for ShotState {
    fn default() -> Self {
        ShotState {
            sizes: HashMap::new(),
            color: None,
            style: ShotTextStyle::default(),
            last_region: None,
            grid_size: 10,
            grid: false,
        }
    }
}

impl ShotState {
    pub fn size(&self, tool: ShotTool) -> i64 {
        self.sizes
            .get(tool.raw_value())
            .copied()
            .unwrap_or_else(|| tool.default_size())
    }

    pub fn load(path: &str) -> ShotState {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<ShotState>(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &str) {
        if let Some(dir) = std::path::Path::new(path).parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(s) = serde_json::to_string(self) {
            let _ = std::fs::write(path, s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ShotTool::*;

    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() <= 0.01
    }

    fn no_overlap(frames: &[Rect]) -> bool {
        for i in 0..frames.len() {
            for j in (i + 1)..frames.len() {
                if frames[i].inset(0.5, 0.5).intersects(&frames[j]) {
                    return false;
                }
            }
        }
        true
    }

    fn counter_objs(d: &ShotDocument) -> Vec<i64> {
        d.objects
            .iter()
            .filter(|o| o.tool == ShotTool::Counter)
            .map(|o| o.number)
            .collect()
    }

    #[test]
    fn ring_defaults_and_tools() {
        let r = ShotTool::ring("", true);
        assert_eq!(r.len(), 21, "default ring = 20 buttons + the size badge");
        assert_eq!(r.iter().position(|t| *t == Size), Some(11), "badge after last drawing tool");
        let expected = vec![
            Pencil,
            Line,
            Arrow,
            Selection,
            Rectangle,
            Circle,
            Marker,
            Text,
            Counter,
            Pixelate,
            Invert,
        ];
        assert_eq!(&r[..11], &expected[..]);
        assert!(!ShotTool::ring("", false).contains(&Size));
        assert!(!r.contains(&Accept));
        assert!(!r.contains(&SizeUp));
        assert_eq!(
            ShotTool::ring("copy, nonsense, copy, exit", false),
            vec![Copy, Exit],
            "unknown + duplicate names dropped"
        );
        assert!(ShotTool::ring("", false).contains(&CopyText));
        assert!(CopyText.finishes());
        assert_eq!(CopyText.kind(), ShotToolKind::Action);
        assert_eq!(ShotTool::for_letter('P'), Some(Pencil));
        assert_eq!(ShotTool::for_letter('z'), None);
    }

    #[test]
    fn ring_layout_centered() {
        let screen = Rect::new(0.0, 0.0, 1440.0, 900.0);
        let b = 34.0;
        let sel = Rect::new(500.0, 300.0, 400.0, 250.0);
        let count = 21;
        let l = ButtonRing::layout(sel, screen, count, b);
        assert_eq!(l.frames.len(), count);
        assert!(!l.inside);
        assert!(l.frames.iter().all(|f| screen.contains_rect(f)));
        assert!(l.frames.iter().all(|f| !f.intersects(&sel)));
        assert!(no_overlap(&l.frames));
        let per_row = ((sel.w + (b / 4.0).floor()) / (b + (b / 4.0).floor())) as usize;
        assert!(l.frames[..per_row].iter().all(|f| f.min_y() >= sel.max_y()));
        let row: Vec<Rect> = l.frames[..per_row].to_vec();
        assert!(row.windows(2).all(|w| w[0].min_x() < w[1].min_x()));
        let right = l.frames[per_row];
        assert!(right.min_x() >= sel.max_x());
        assert!(l.frames[per_row + 1].min_y() < right.min_y());
    }

    #[test]
    fn ring_layout_edges() {
        let screen = Rect::new(0.0, 0.0, 1440.0, 900.0);
        let b = 34.0;
        let count = 21;
        let edges = [
            ("left", Rect::new(0.0, 300.0, 300.0, 200.0)),
            ("right", Rect::new(1140.0, 300.0, 300.0, 200.0)),
            ("top", Rect::new(500.0, 0.0, 300.0, 200.0)),
            ("bottom", Rect::new(500.0, 700.0, 300.0, 200.0)),
            ("top-left corner", Rect::new(0.0, 0.0, 200.0, 150.0)),
            ("bottom-right corner", Rect::new(1240.0, 750.0, 200.0, 150.0)),
        ];
        for (name, s) in edges {
            let e = ButtonRing::layout(s, screen, count, b);
            assert_eq!(e.frames.len(), count, "{}", name);
            assert!(e.frames.iter().all(|f| f.w == b), "{}: every button placed", name);
            assert!(e.frames.iter().all(|f| screen.contains_rect(f)), "{}: on screen", name);
            assert!(no_overlap(&e.frames), "{}: no overlap", name);
        }
    }

    #[test]
    fn ring_layout_full_tiny_empty() {
        let screen = Rect::new(0.0, 0.0, 1440.0, 900.0);
        let b = 34.0;
        let count = 21;

        let full = ButtonRing::layout(screen, screen, count, b);
        assert!(full.inside);
        assert!(full.frames.iter().all(|f| screen.contains_rect(f) && f.w == b));
        assert!(no_overlap(&full.frames));
        assert!(full.frames[0].max_y() >= screen.max_y() - b - 10.0);

        for (name, sel) in [
            ("tiny", Rect::new(700.0, 400.0, 4.0, 4.0)),
            ("tiny corner", Rect::new(0.0, 0.0, 6.0, 6.0)),
        ] {
            let t = ButtonRing::layout(sel, screen, count, b);
            assert!(
                t.frames.iter().all(|f| screen.contains_rect(f) && f.w == b),
                "{}",
                name
            );
            assert!(no_overlap(&t.frames), "{}", name);
        }

        assert!(ButtonRing::layout(Rect::new(500.0, 300.0, 400.0, 250.0), screen, 0, b)
            .frames
            .is_empty());
        assert_eq!(ButtonRing::default_button_size(15.5), 34.0);
    }

    fn counter(x: f64) -> ShotObject {
        ShotObject::new(ShotTool::Counter, vec![Point::new(x, 10.0)], ShotColor::BLACK, 1)
    }

    #[test]
    fn document_counters_and_undo() {
        let mut d = ShotDocument::new(100);
        d.add(counter(10.0));
        d.add(counter(100.0));
        d.add(counter(200.0));
        assert_eq!(counter_objs(&d), vec![1, 2, 3]);
        d.remove(1);
        assert_eq!(counter_objs(&d), vec![1, 2], "delete renumbers the later ones");
        d.undo();
        assert_eq!(counter_objs(&d), vec![1, 2, 3], "undo brings it back, renumbered");
        d.redo();
        assert_eq!(counter_objs(&d), vec![1, 2], "redo deletes again");
        d.undo();
        d.add(ShotObject::new(
            ShotTool::Line,
            vec![Point::ZERO, Point::new(5.0, 5.0)],
            ShotColor::BLACK,
            3,
        ));
        d.undo();
        assert_eq!(d.objects.len(), 3, "undo of an add");
        assert_eq!(d.next_counter_number(0), 4);
        assert_eq!(d.next_counter_number(6), 10);

        let mut jump = counter(300.0);
        jump.number_offset = 6;
        d.add(jump);
        assert_eq!(d.objects.last().unwrap().number, 10, "bubble after a wheel jump");
        d.remove(0);
        assert_eq!(counter_objs(&d), vec![1, 2, 9], "renumber keeps the jump");
        assert_eq!(ShotDocument::new(5).next_counter_number(0), 1);
    }

    #[test]
    fn document_undo_limit_redo_coalesce_hit() {
        let mut lim = ShotDocument::new(3);
        for i in 0..5 {
            lim.add(counter(i as f64 * 50.0));
        }
        assert_eq!(lim.undo_depth(), 3, "undo stack capped at undo-limit");
        while lim.undo() {}
        assert_eq!(lim.objects.len(), 2, "only undo-limit steps back");
        assert!(lim.can_redo(), "redo after undo");
        lim.add(counter(1.0));
        assert!(!lim.can_redo(), "a new change clears redo");

        let mut m = ShotDocument::new(100);
        m.add(ShotObject::new(
            ShotTool::Selection,
            vec![Point::new(10.0, 10.0), Point::new(50.0, 50.0)],
            ShotColor::BLACK,
            3,
        ));
        m.update(0, None, |o| *o = o.moved(5.0, 5.0));
        m.update(0, None, |o| o.color = ShotColor::WHITE);
        for _ in 0..4 {
            m.update(0, Some("size0"), |o| o.size += 1);
        }
        assert_eq!(m.objects[0].size, 7, "size changed");
        m.undo();
        assert_eq!(m.objects[0].size, 3, "wheel notches undo as one step");
        m.undo();
        assert_eq!(m.objects[0].color, ShotColor::BLACK, "color change undone");
        m.undo();
        assert_eq!(m.objects[0].start(), Point::new(10.0, 10.0), "move undone");

        let mut h = ShotDocument::new(100);
        h.add(ShotObject::new(
            ShotTool::Selection,
            vec![Point::new(100.0, 100.0), Point::new(200.0, 200.0)],
            ShotColor::BLACK,
            3,
        ));
        h.add(ShotObject::new(
            ShotTool::Line,
            vec![Point::new(0.0, 0.0), Point::new(50.0, 0.0)],
            ShotColor::BLACK,
            3,
        ));
        assert_eq!(h.hit(Point::new(100.0, 150.0)), Some(0), "outline rect edge hit");
        assert_eq!(h.hit(Point::new(150.0, 150.0)), None, "outline rect middle misses");
        assert_eq!(h.hit(Point::new(25.0, 2.0)), Some(1), "line hit near it");
        h.reorder(1, 0);
        assert_eq!(h.objects[0].tool, ShotTool::Line, "reorder (Layers)");
    }

    #[test]
    fn snap_tests() {
        let o = Point::ZERO;
        let a = ShotSnap::angle(o, Point::new(100.0, 7.0));
        assert!(near(a.y, 0.0) && near(a.x, 100.0f64.hypot(7.0)));
        let b = ShotSnap::angle(o, Point::new(50.0, 47.0));
        assert!(near(b.x, b.y), "near-diagonal snaps to 45°");
        let c = ShotSnap::angle(o, Point::new(-3.0, -80.0));
        assert!(near(c.x, 0.0) && c.y < 0.0, "near-vertical snaps to 90°");
        for k in 0..8 {
            let ang = k as f64 * std::f64::consts::PI / 4.0 + 0.1;
            let s = ShotSnap::angle(o, Point::new(ang.cos() * 50.0, ang.sin() * 50.0));
            let got = s.y.atan2(s.x);
            let want = k as f64 * std::f64::consts::PI / 4.0;
            assert!(((got - want).rem_euclid(2.0 * std::f64::consts::PI)).min((want - got).rem_euclid(2.0 * std::f64::consts::PI)) < 1e-6);
        }
        let sq = ShotSnap::square(Point::new(10.0, 10.0), Point::new(40.0, -5.0));
        assert_eq!(sq, Point::new(40.0, -20.0));
        assert_eq!(
            ShotSnap::snap(ShotTool::Pencil, o, Point::new(3.0, 4.0)),
            Point::new(3.0, 4.0),
            "pencil never snaps"
        );
    }

    fn make_pixels<FI: Fn(i64, i64) -> [u8; 3], FO: Fn(i64, i64) -> [u8; 3]>(
        w: i64,
        h: i64,
        interior: Rect,
        inner: FI,
        outer: FO,
    ) -> ShotPixels {
        let mut data = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let c = if interior.contains_point(Point::new(x as f64 + 0.5, y as f64 + 0.5)) {
                    inner(x, y)
                } else {
                    outer(x, y)
                };
                let i = ((y * w + x) * 4) as usize;
                data[i] = c[0];
                data[i + 1] = c[1];
                data[i + 2] = c[2];
                data[i + 3] = 255;
            }
        }
        ShotPixels::new(w, h, data)
    }

    #[test]
    fn pixelate_tests() {
        let interior = Rect::new(30.0, 30.0, 40.0, 40.0);
        let px = make_pixels(
            100,
            100,
            interior,
            |_, _| [0, 255, 0],
            |x, y| [120u8.saturating_add(x as u8), 60u8.saturating_add((y / 2) as u8), 60],
        );
        for size in [1i64, 2, 5, 20] {
            let blocks = ShotPixelate::secure_blocks(&px, interior, size, 1.0);
            let (cols, rows) = ShotPixelate::grid(interior, size);
            assert_eq!(blocks.len(), rows, "size {}: rows", size);
            assert!(blocks.iter().all(|r| r.len() == cols), "size {}: cols", size);
            let leaked = blocks
                .iter()
                .flatten()
                .any(|c| c.g > 0.6 && c.r < 0.3);
            assert!(!leaked, "size {}: no hidden interior color", size);
        }
        let edge = make_pixels(
            60,
            60,
            Rect::new(0.0, 0.0, 30.0, 30.0),
            |_, _| [0, 255, 0],
            |_, _| [200, 40, 40],
        );
        let eb = ShotPixelate::secure_blocks(&edge, Rect::new(0.0, 0.0, 30.0, 30.0), 2, 1.0);
        assert!(!eb.iter().flatten().any(|c| c.g > 0.6), "corner: no leak");
        let retina = ShotPixelate::secure_blocks(&px, interior, 2, 2.0);
        let pts = ShotPixelate::grid(Rect::new(0.0, 0.0, 20.0, 20.0), 2);
        assert_eq!(retina.len(), pts.1, "2x display: grid in points (rows)");
        assert_eq!(retina[0].len(), pts.0, "2x display: grid in points (cols)");
        let grid = ShotPixelate::grid(Rect::new(0.0, 0.0, 300.0, 120.0), 2);
        assert_eq!(grid, (50, 20), "block resolution = rect × 0.5 / (size + 1)");
    }

    #[test]
    fn color_tests() {
        assert_eq!(ShotColor::from_hex("#740096").unwrap().hex(), "#740096");
        assert!(ShotColor::from_hex("#740096").unwrap().is_dark());
        assert!(!ShotColor::from_hex("#ffff00").unwrap().is_dark());
        assert!(ShotColor::from_hex("zz").is_none());
        assert!(ShotColor::from_hex("#12345").is_none());
        let c = ShotColor::from_hex("#3366cc").unwrap();
        let (h, s, v) = c.hsv();
        assert_eq!(ShotColor::from_hsv(h, s, v).hex(), "#3366cc");
    }

    #[test]
    fn files_tests() {
        let d = ShotDate {
            year: 2026,
            month: 10,
            day: 2,
            hour: 14,
            minute: 5,
            second: 0,
        };
        assert_eq!(
            ShotFiles::expand_date("%F_%H-%M", d),
            "2026-10-02_14-05",
            "Flameshot's default pattern"
        );
        assert_eq!(ShotFiles::expand_date("shot %Y/%m", d), "shot 2026-10", "no '/' in a name");

        let taken = |p: &str| p == "/tmp/x/a.png" || p == "/tmp/x/a 2.png";
        assert_eq!(
            ShotFiles::unique_path_with("/tmp/x", "a", "png", &taken),
            "/tmp/x/a 3.png",
            "clash → ' 3'"
        );
        let nope = |_: &str| false;
        assert_eq!(
            ShotFiles::unique_path_with("/tmp/x/", "b", "png", &nope),
            "/tmp/x/b.png",
            "free name kept"
        );

        let yep_dir = |_: &str| true;
        let secs = local_epoch(2026, 10, 2, 14, 5, 0);
        assert_eq!(
            ShotFiles::target_with("/tmp/x", "%F", "png", secs, &yep_dir, &nope),
            "/tmp/x/2026-10-02.png",
            "-p DIR → the pattern inside it"
        );
        assert_eq!(
            ShotFiles::target_with("/tmp/y/shot", "%F", "jpg", secs, &nope, &nope),
            "/tmp/y/shot.jpg",
            "-p FILE gets the format's extension"
        );
        assert_eq!(
            ShotFiles::target_with("/tmp/y/s.png", "%F", "png", secs, &nope, &nope),
            "/tmp/y/s.png",
            "-p FILE.png kept"
        );
    }

    #[test]
    fn args_tests() {
        assert_eq!(ShotArgs::parse::<&str>(&[]).unwrap(), ShotArgs::default());

        let a = ShotArgs::parse(&[
            "gui",
            "-p",
            "/tmp",
            "-c",
            "-d",
            "500",
            "--region",
            "300x200+10+20",
            "-s",
            "--pin",
            "-r",
            "-g",
        ])
        .unwrap();
        assert_eq!(a.mode, ShotMode::Gui);
        assert_eq!(a.path.as_deref(), Some("/tmp"));
        assert!(a.clipboard);
        assert_eq!(a.delay_ms, 500);
        assert_eq!(a.region.as_deref(), Some("300x200+10+20"));
        assert!(a.accept_on_select && a.pin && a.raw && a.print_geometry);
        assert!(a.wants_reply());

        let a = ShotArgs::parse(&["screen", "-n", "1", "-c"]).unwrap();
        assert_eq!((a.mode, a.screen_number, a.clipboard, a.wants_reply()), (ShotMode::Screen, Some(1), true, false));
        let a = ShotArgs::parse(&["full", "--region", "screen0"]).unwrap();
        assert_eq!((a.mode, a.region.as_deref()), (ShotMode::Full, Some("screen0")));
        let a = ShotArgs::parse(&["text", "-r"]).unwrap();
        assert_eq!((a.mode, a.is_overlay(), a.raw), (ShotMode::Text, true, true));

        for words in [
            vec!["-d", "x"],
            vec!["--bogus"],
            vec!["--region", "12"],
            vec!["-p"],
        ] {
            assert!(ShotArgs::parse(&words).is_err(), "{:?}", words);
        }

        assert_eq!(
            ShotArgs::parse_region("300x200+10+20"),
            Some(Rect::new(10.0, 20.0, 300.0, 200.0))
        );
        assert_eq!(
            ShotArgs::parse_region("300x200"),
            Some(Rect::new(0.0, 0.0, 300.0, 200.0))
        );
        assert_eq!(ShotArgs::parse_region("0x200+1+1"), None);
    }

    fn local_epoch(year: i64, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> i64 {
        unsafe {
            let mut tm: libc::tm = std::mem::zeroed();
            tm.tm_year = (year - 1900) as i32;
            tm.tm_mon = (month as i32) - 1;
            tm.tm_mday = day as i32;
            tm.tm_hour = hour as i32;
            tm.tm_min = minute as i32;
            tm.tm_sec = second as i32;
            tm.tm_isdst = -1;
            libc::mktime(&mut tm)
        }
    }

    #[test]
    fn state_tests() {
        let root = std::env::temp_dir().join(format!("shot-state-{}", std::process::id()));
        let path = root.join("s").join("state.json");
        let path_str = path.to_string_lossy().to_string();

        let mut st = ShotState::default();
        st.sizes.insert("pencil".to_string(), 7);
        st.last_region = Some(ShotRegion {
            display: 1,
            x: 0.0,
            y: 0.0,
            w: 10.0,
            h: 10.0,
        });
        st.save(&path_str);

        let back = ShotState::load(&path_str);
        assert_eq!(back.sizes.get("pencil"), Some(&7));
        assert_eq!(back.size(ShotTool::Pencil), 7);
        assert_eq!(back.size(ShotTool::Text), 8, "tool defaults");
        assert_eq!(back.size(ShotTool::Marker), 5);
        assert_eq!(back.size(ShotTool::Counter), 1);
        assert_eq!(back.size(ShotTool::Circle), 3);
        assert_eq!(back.grid_size, 10);
        assert!(!back.grid);

        let json = serde_json::to_string(&back).unwrap();
        assert!(!json.contains("\"color\""), "color omitted when None");
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["lastRegion"]["w"], 10.0);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn text_box_grows() {
        let mut t = ShotObject::new(ShotTool::Text, vec![Point::ZERO], ShotColor::BLACK, 8);
        t.text = "a".to_string();
        let small = ShotText::box_size(&t);
        t.text = "a much longer line\nand a second".to_string();
        let big = ShotText::box_size(&t);
        assert!(big.w > small.w && big.h > small.h, "text box grows");
    }
}
