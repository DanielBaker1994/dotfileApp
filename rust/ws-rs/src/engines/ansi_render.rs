//! Port of `AnsiRender.swift` — the ANSI parser, the grid/cell model and the
//! ghostty/xterm256 theme tables.
//!
//! The Swift type delegates `AnsiGrid.parse` to `pylib/ansi.py`; this is the
//! pure-Rust mirror of that python reference (the accepted port of the Swift
//! suite), so the parser + theme logic is testable without the helper. The
//! CoreText glyph-drawing half uses `objc2-core-text` / `objc2-core-graphics`.

use core::ffi::c_void;
use core::ptr::{self, NonNull};
use std::collections::HashMap;

use objc2::runtime::AnyObject;
use objc2::AnyThread;
use objc2_core_graphics::{
    kCGColorSpaceDisplayP3, kCGColorSpaceSRGB, CGBitmapContextCreate, CGBitmapContextCreateImage,
    CGAffineTransformIdentity, CGColor, CGColorSpace, CGContext, CGImage, CGImageAlphaInfo,
    CGGlyph,
};
use objc2_core_text::{
    kCTFontAttributeName, kCTForegroundColorAttributeName, CTFont, CTFontOrientation,
    CTFontSymbolicTraits, CTLine,
};
use objc2_foundation::{NSMutableAttributedString, NSPoint, NSRange, NSRect, NSSize, NSString};

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRetain(cf: *const c_void) -> *const c_void;
    fn CFRelease(cf: *const c_void);
}

/// Reinterpret one opaque CF reference as another (toll-free bridged) CF
/// type; keeps the underlying `objc2-core-foundation` type names out of this
/// file, which cannot depend on that crate directly.
fn ffi_cast<T, U>(r: &T) -> &U {
    unsafe { &*(r as *const T as *const U) }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnsiRGB {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl AnsiRGB {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        AnsiRGB { r, g, b }
    }

    pub fn from_hex(hex: &str) -> Option<AnsiRGB> {
        let mut s = hex.trim();
        if let Some(rest) = s.strip_prefix('#') {
            s = rest;
        }
        if s.len() != 6 {
            return None;
        }
        let v = u32::from_str_radix(s, 16).ok()?;
        Some(AnsiRGB::new(
            ((v >> 16) & 0xff) as u8,
            ((v >> 8) & 0xff) as u8,
            (v & 0xff) as u8,
        ))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnsiColor {
    None,
    Index(i64),
    Rgb(AnsiRGB),
}

impl Default for AnsiColor {
    fn default() -> Self {
        AnsiColor::None
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct AnsiStyle {
    pub fg: AnsiColor,
    pub bg: AnsiColor,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
    pub strike: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnsiCell {
    pub text: String,
    pub style: AnsiStyle,
    pub width: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AnsiGrid {
    pub rows: Vec<Vec<AnsiCell>>,
}

impl AnsiGrid {
    pub fn columns(&self) -> usize {
        self.rows.iter().map(|r| r.len()).max().unwrap_or(0)
    }

    pub fn parse(text: &str) -> AnsiGrid {
        let mut g = AnsiGrid::default();
        let mut row: Vec<AnsiCell> = Vec::new();
        let mut col: usize = 0;
        let mut style = AnsiStyle::default();
        let chars = graphemes(text);
        let n = chars.len();
        let mut i = 0usize;

        while i < n {
            let c = chars[i].as_str();

            if c == "\u{1b}" {
                i += 1;
                if i >= n {
                    break;
                }
                let nxt = chars[i].chars().next().unwrap();
                match nxt {
                    '[' => {
                        let mut params = String::new();
                        i += 1;
                        while i < n {
                            let f = chars[i].chars().next().unwrap() as u32;
                            if (0x40..=0x7e).contains(&f) {
                                break;
                            }
                            params.push_str(&chars[i]);
                            i += 1;
                        }
                        if i < n && chars[i] == "m" {
                            apply_sgr(&params, &mut style);
                        }
                        i += 1;
                    }
                    ']' | 'P' | '_' | '^' => {
                        i += 1;
                        while i < n {
                            if chars[i] == "\u{7}" {
                                i += 1;
                                break;
                            }
                            if chars[i] == "\u{1b}" && i + 1 < n && chars[i + 1] == "\\" {
                                i += 2;
                                break;
                            }
                            i += 1;
                        }
                    }
                    '(' | ')' | '*' | '+' => {
                        i += 2;
                    }
                    _ => {
                        i += 1;
                    }
                }
                continue;
            }

            if c == "\n" || c == "\r\n" {
                g.rows.push(std::mem::take(&mut row));
                col = 0;
            } else if c == "\r" {
                col = 0;
            } else if c == "\t" {
                let stop = (col / 8 + 1) * 8;
                while col < stop {
                    put(&mut row, &mut col, " ", 1, &style);
                }
            } else {
                let a = c.chars().next().unwrap() as u32;
                if !(a < 0x20 || a == 0x7f) {
                    let w = cell_width(c);
                    put(&mut row, &mut col, c, w, &style);
                }
            }
            i += 1;
        }
        if !row.is_empty() {
            g.rows.push(row);
        }

        while let Some(last) = g.rows.last() {
            let blank = last.iter().all(|c| {
                (c.text == " " || c.text.is_empty())
                    && c.style.bg == AnsiColor::None
                    && !c.style.inverse
            });
            if blank {
                g.rows.pop();
            } else {
                break;
            }
        }
        g
    }
}

fn put(row: &mut Vec<AnsiCell>, col: &mut usize, text: &str, w: i64, style: &AnsiStyle) {
    while row.len() < *col {
        row.push(AnsiCell {
            text: " ".to_string(),
            style: *style,
            width: 1,
        });
    }
    let cell = AnsiCell {
        text: text.to_string(),
        style: *style,
        width: w,
    };
    if *col < row.len() {
        row[*col] = cell;
    } else {
        row.push(cell);
    }
    *col += 1;
    if w == 2 {
        let rest = AnsiCell {
            text: String::new(),
            style: *style,
            width: 0,
        };
        if *col < row.len() {
            row[*col] = rest;
        } else {
            row.push(rest);
        }
        *col += 1;
    }
}

/// Grapheme clustering as `pylib/ansi.py::_graphemes`: a combining mark, a
/// VS16 or anything after a ZWJ rides in the previous cluster.
fn graphemes(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for ch in text.chars() {
        if let Some(last) = out.last_mut() {
            if last.ends_with('\u{200d}') || is_combining(ch) || ch == '\u{fe0f}' {
                last.push(ch);
                continue;
            }
        }
        out.push(ch.to_string());
    }
    out
}

fn is_combining(c: char) -> bool {
    matches!(c as u32,
        0x0300..=0x036F | 0x0483..=0x0489 | 0x0591..=0x05BD | 0x05BF |
        0x05C1..=0x05C2 | 0x05C4..=0x05C5 | 0x05C7 | 0x0610..=0x061A |
        0x064B..=0x065F | 0x0670 | 0x06D6..=0x06DC | 0x06DF..=0x06E4 |
        0x06E7..=0x06E8 | 0x06EA..=0x06ED | 0x0711 | 0x0730..=0x074A |
        0x07A6..=0x07B0 | 0x07EB..=0x07F3 | 0x0816..=0x0819 |
        0x081B..=0x0823 | 0x0825..=0x0827 | 0x0829..=0x082D |
        0x0859..=0x085B | 0x08D4..=0x08E1 | 0x08E3..=0x0903 |
        0x093A..=0x093C | 0x093E..=0x094F | 0x0951..=0x0957 |
        0x0962..=0x0963 | 0x0981..=0x0983 | 0x09BC | 0x09BE..=0x09C4 |
        0x09C7..=0x09C8 | 0x09CB..=0x09CD | 0x09D7 | 0x09E2..=0x09E3 |
        0x0A01..=0x0A03 | 0x0A3C | 0x0A3E..=0x0A42 | 0x0A47..=0x0A48 |
        0x0A4B..=0x0A4D | 0x0A51 | 0x0A70..=0x0A71 | 0x0A75 |
        0x0A81..=0x0A83 | 0x0ABC | 0x0ABE..=0x0AC5 | 0x0AC7..=0x0AC9 |
        0x0ACB..=0x0ACD | 0x0AE2..=0x0AE3 | 0x0B01..=0x0B03 | 0x0B3C |
        0x0B3E..=0x0B44 | 0x0B47..=0x0B48 | 0x0B4B..=0x0B4D |
        0x0B56..=0x0B57 | 0x0B62..=0x0B63 | 0x0B82 | 0x0BBE..=0x0BC2 |
        0x0BC6..=0x0BC8 | 0x0BCA..=0x0BCD | 0x0BD7 | 0x0C00..=0x0C04 |
        0x0C3E..=0x0C44 | 0x0C46..=0x0C48 | 0x0C4A..=0x0C4D |
        0x0C55..=0x0C56 | 0x0C62..=0x0C63 | 0x0C81..=0x0C83 | 0x0CBC |
        0x0CBE..=0x0CC4 | 0x0CC6..=0x0CC8 | 0x0CCA..=0x0CCD |
        0x0CD5..=0x0CD6 | 0x0CE2..=0x0CE3 | 0x0D01..=0x0D03 |
        0x0D3E..=0x0D44 | 0x0D46..=0x0D48 | 0x0D4A..=0x0D4D | 0x0D57 |
        0x0D62..=0x0D63 | 0x0D82..=0x0D83 | 0x0DCA | 0x0DCF..=0x0DD4 |
        0x0DD6 | 0x0DD8..=0x0DDF | 0x0DF2..=0x0DF3 | 0x0E31 |
        0x0E34..=0x0E3A | 0x0E47..=0x0E4E | 0x0EB1 | 0x0EB4..=0x0EB9 |
        0x0EBB..=0x0EBC | 0x0EC8..=0x0ECD | 0x0F18..=0x0F19 | 0x0F35 |
        0x0F37 | 0x0F39 | 0x0F3E..=0x0F3F | 0x0F71..=0x0F84 |
        0x0F86..=0x0F87 | 0x0F8D..=0x0F97 | 0x0F99..=0x0FBC | 0x0FC6 |
        0x102B..=0x103E | 0x1056..=0x1059 | 0x105E..=0x1060 |
        0x1062..=0x1064 | 0x1067..=0x106D | 0x1071..=0x1074 |
        0x1082..=0x108D | 0x108F | 0x109A..=0x109D | 0x135D..=0x135F |
        0x1712..=0x1714 | 0x1732..=0x1734 | 0x1752..=0x1753 |
        0x1772..=0x1773 | 0x17B4..=0x17D3 | 0x17DD | 0x180B..=0x180D |
        0x1885..=0x1886 | 0x18A9 | 0x1920..=0x192B | 0x1930..=0x193B |
        0x1A17..=0x1A1B | 0x1A55..=0x1A5E | 0x1A60..=0x1A7C | 0x1A7F |
        0x1AB0..=0x1ABE | 0x1B00..=0x1B04 | 0x1B34..=0x1B44 |
        0x1B6B..=0x1B73 | 0x1B80..=0x1B82 | 0x1BA1..=0x1BAD |
        0x1BE6..=0x1BF3 | 0x1C24..=0x1C37 | 0x1CD0..=0x1CD2 |
        0x1CD4..=0x1CE8 | 0x1CED | 0x1CF2..=0x1CF4 | 0x1CF8..=0x1CF9 |
        0x1DC0..=0x1DFF | 0x20D0..=0x20F0 | 0x2CEF..=0x2CF1 |
        0x2D7F | 0x2DE0..=0x2DFF | 0x302A..=0x302F | 0x3099..=0x309A |
        0xA66F..=0xA672 | 0xA674..=0xA67D | 0xA69E..=0xA69F |
        0xA6F0..=0xA6F1 | 0xA802 | 0xA806 | 0xA80B | 0xA823..=0xA827 |
        0xA880..=0xA881 | 0xA8B4..=0xA8C5 | 0xA8E0..=0xA8F1 |
        0xA926..=0xA92D | 0xA947..=0xA953 | 0xA980..=0xA983 |
        0xA9B3..=0xA9C0 | 0xA9E5 | 0xAA29..=0xAA36 | 0xAA43 |
        0xAA4C..=0xAA4D | 0xAA7B..=0xAA7D | 0xAAB0 | 0xAAB2..=0xAAB4 |
        0xAAB7..=0xAAB8 | 0xAABE..=0xAABF | 0xAAC1 | 0xAAEB..=0xAAEF |
        0xAAF5..=0xAAF6 | 0xABE3..=0xABEA | 0xABEC..=0xABED |
        0xFB1E | 0xFE20..=0xFE2F | 0x101FD | 0x102E0 | 0x10376..=0x1037A |
        0x10A01..=0x10A03 | 0x10A05..=0x10A06 | 0x10A0C..=0x10A0F |
        0x10A38..=0x10A3A | 0x10A3F | 0x10AE5..=0x10AE6 |
        0x11000..=0x11002 | 0x11038..=0x11046 | 0x1107F..=0x11082 |
        0x110B0..=0x110BA | 0x11100..=0x11102 | 0x11127..=0x11134 |
        0x11173 | 0x11180..=0x11182 | 0x111B3..=0x111C0 |
        0x111CA..=0x111CC | 0x1122C..=0x11237 | 0x1123E |
        0x112DF..=0x112EA | 0x11300..=0x11303 | 0x1133C |
        0x1133E..=0x11344 | 0x11347..=0x11348 | 0x1134B..=0x1134D |
        0x11357 | 0x11362..=0x11363 | 0x11366..=0x1136C |
        0x11370..=0x11374 | 0x114B0..=0x114C3 | 0x115AF..=0x115B5 |
        0x115B8..=0x115C0 | 0x115DC..=0x115DD | 0x11630..=0x11640 |
        0x116AB..=0x116B7 | 0x1171D..=0x1172B | 0x11C2F..=0x11C36 |
        0x11C38..=0x11C3F | 0x11C92..=0x11CA7 | 0x11CA9..=0x11CB6 |
        0x16AF0..=0x16AF4 | 0x16B30..=0x16B36 | 0x16F51..=0x16F7E |
        0x16F8F..=0x16F92 | 0x1BC9D..=0x1BC9E | 0x1D165..=0x1D169 |
        0x1D16D..=0x1D172 | 0x1D17B..=0x1D182 | 0x1D185..=0x1D18B |
        0x1D1AA..=0x1D1AD | 0x1D242..=0x1D244 | 0x1DA00..=0x1DA36 |
        0x1DA3B..=0x1DA6C | 0x1DA75 | 0x1DA84 | 0x1DA9B..=0x1DA9F |
        0x1DAA1..=0x1DAAF | 0x1E000..=0x1E006 | 0x1E008..=0x1E018 |
        0x1E01B..=0x1E021 | 0x1E023..=0x1E024 | 0x1E026..=0x1E02A |
        0x1E8D0..=0x1E8D6 | 0x1E944..=0x1E94A | 0xE0100..=0xE01EF
    )
}

pub fn cell_width(c: &str) -> i64 {
    if c.chars().any(|ch| ch as u32 == 0xfe0f) {
        return 2;
    }
    if c.is_empty() {
        return 1;
    }
    let first = c.chars().next().unwrap() as u32;
    if (0x1f300..=0x1faff).contains(&first) {
        return 2;
    }
    const WIDE: [(u32, u32); 12] = [
        (0x1100, 0x115f),
        (0x2e80, 0x303e),
        (0x3041, 0x33ff),
        (0x3400, 0x4dbf),
        (0x4e00, 0x9fff),
        (0xa000, 0xa4cf),
        (0xac00, 0xd7a3),
        (0xf900, 0xfaff),
        (0xfe30, 0xfe4f),
        (0xff00, 0xff60),
        (0xffe0, 0xffe6),
        (0x20000, 0x3fffd),
    ];
    for (lo, hi) in WIDE {
        if lo <= first && first <= hi {
            return 2;
        }
    }
    1
}

pub fn apply_sgr(params: &str, s: &mut AnsiStyle) {
    let mut codes: Vec<i64> = params
        .split(|ch| ch == ';' || ch == ':')
        .map(|x| x.trim().parse::<i64>().unwrap_or(0))
        .collect();
    if codes.is_empty() {
        codes.push(0);
    }
    let n = codes.len();
    let mut i = 0usize;
    while i < n {
        match codes[i] {
            0 => {
                s.fg = AnsiColor::None;
                s.bg = AnsiColor::None;
                s.bold = false;
                s.dim = false;
                s.italic = false;
                s.underline = false;
                s.inverse = false;
                s.strike = false;
            }
            1 => s.bold = true,
            2 => s.dim = true,
            3 => s.italic = true,
            4 => s.underline = true,
            7 => s.inverse = true,
            9 => s.strike = true,
            21 => s.underline = true,
            22 => {
                s.bold = false;
                s.dim = false;
            }
            23 => s.italic = false,
            24 => s.underline = false,
            27 => s.inverse = false,
            29 => s.strike = false,
            30..=37 => s.fg = AnsiColor::Index(codes[i] - 30),
            38 => {
                if let Some(x) = extended(&codes, &mut i) {
                    s.fg = x;
                }
            }
            39 => s.fg = AnsiColor::None,
            40..=47 => s.bg = AnsiColor::Index(codes[i] - 40),
            48 => {
                if let Some(x) = extended(&codes, &mut i) {
                    s.bg = x;
                }
            }
            49 => s.bg = AnsiColor::None,
            58 => {
                let _ = extended(&codes, &mut i);
            }
            90..=97 => s.fg = AnsiColor::Index(codes[i] - 90 + 8),
            100..=107 => s.bg = AnsiColor::Index(codes[i] - 100 + 8),
            _ => {}
        }
        i += 1;
    }
}

fn extended(codes: &[i64], i: &mut usize) -> Option<AnsiColor> {
    let n = codes.len();
    if *i + 1 >= n {
        return None;
    }
    if codes[*i + 1] == 5 && *i + 2 < n {
        let v = codes[*i + 2].clamp(0, 255);
        *i += 2;
        return Some(AnsiColor::Index(v));
    }
    if codes[*i + 1] == 2 && *i + 4 < n {
        let r = codes[*i + 2].clamp(0, 255) as u8;
        let g = codes[*i + 3].clamp(0, 255) as u8;
        let b = codes[*i + 4].clamp(0, 255) as u8;
        *i += 4;
        return Some(AnsiColor::Rgb(AnsiRGB::new(r, g, b)));
    }
    None
}

#[derive(Clone, Debug, PartialEq)]
pub struct AnsiTheme {
    pub foreground: AnsiRGB,
    pub background: AnsiRGB,
    pub palette: Vec<AnsiRGB>,
    pub font_name: String,
    pub font_size: f64,
    pub bold_is_bright: bool,
    pub display_p3: bool,
}

impl Default for AnsiTheme {
    fn default() -> Self {
        AnsiTheme {
            foreground: AnsiRGB::new(0xc0, 0xca, 0xf5),
            background: AnsiRGB::new(0x1a, 0x1b, 0x26),
            palette: xterm256(),
            font_name: "Menlo".to_string(),
            font_size: 13.0,
            bold_is_bright: false,
            display_p3: false,
        }
    }
}

impl AnsiTheme {
    pub fn ghostty(text: &str) -> AnsiTheme {
        let mut t = AnsiTheme::default();
        let mut font_set = false;
        for line in text.split('\n') {
            let Some(at) = line.find(" = ") else { continue };
            let k = line[..at].trim();
            let v = line[at + 3..].trim();
            match k {
                "background" => {
                    if let Some(c) = AnsiRGB::from_hex(v) {
                        t.background = c;
                    }
                }
                "foreground" => {
                    if let Some(c) = AnsiRGB::from_hex(v) {
                        t.foreground = c;
                    }
                }
                "font-family" => {
                    if !font_set && !v.is_empty() {
                        t.font_name = v.to_string();
                        font_set = true;
                    }
                }
                "font-size" => {
                    if let Ok(s) = v.parse::<f64>() {
                        if s > 0.0 {
                            t.font_size = s;
                        }
                    }
                }
                "bold-is-bright" => t.bold_is_bright = v == "true",
                "window-colorspace" => t.display_p3 = v == "display-p3",
                "palette" => {
                    if let Some((ns, cs)) = v.split_once('=') {
                        if let Ok(nn) = ns.parse::<i64>() {
                            if let Some(c) = AnsiRGB::from_hex(cs) {
                                if (0..256).contains(&nn) {
                                    t.palette[nn as usize] = c;
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        t
    }

    pub fn colors(&self, s: &AnsiStyle) -> (AnsiRGB, Option<AnsiRGB>) {
        let res = |c: AnsiColor, bright: bool| -> Option<AnsiRGB> {
            match c {
                AnsiColor::None => None,
                AnsiColor::Index(n) => {
                    let idx = if bright && n < 8 { n + 8 } else { n };
                    Some(self.palette[idx as usize])
                }
                AnsiColor::Rgb(v) => Some(v),
            }
        };
        let mut fg = res(s.fg, self.bold_is_bright && s.bold).unwrap_or(self.foreground);
        let mut bg = res(s.bg, false);
        if s.inverse {
            let f = fg;
            fg = bg.unwrap_or(self.background);
            bg = Some(f);
        }
        (fg, bg)
    }
}

pub fn xterm256() -> Vec<AnsiRGB> {
    let mut p: Vec<AnsiRGB> = vec![
        AnsiRGB::new(0x00, 0x00, 0x00),
        AnsiRGB::new(0xcd, 0x00, 0x00),
        AnsiRGB::new(0x00, 0xcd, 0x00),
        AnsiRGB::new(0xcd, 0xcd, 0x00),
        AnsiRGB::new(0x00, 0x00, 0xee),
        AnsiRGB::new(0xcd, 0x00, 0xcd),
        AnsiRGB::new(0x00, 0xcd, 0xcd),
        AnsiRGB::new(0xe5, 0xe5, 0xe5),
        AnsiRGB::new(0x7f, 0x7f, 0x7f),
        AnsiRGB::new(0xff, 0x00, 0x00),
        AnsiRGB::new(0x00, 0xff, 0x00),
        AnsiRGB::new(0xff, 0xff, 0x00),
        AnsiRGB::new(0x5c, 0x5c, 0xff),
        AnsiRGB::new(0xff, 0x00, 0xff),
        AnsiRGB::new(0x00, 0xff, 0xff),
        AnsiRGB::new(0xff, 0xff, 0xff),
    ];
    let steps: [u8; 6] = [0, 95, 135, 175, 215, 255];
    for &r in &steps {
        for &g in &steps {
            for &b in &steps {
                p.push(AnsiRGB::new(r, g, b));
            }
        }
    }
    for i in 0..24u16 {
        let v = (8 + i * 10) as u8;
        p.push(AnsiRGB::new(v, v, v));
    }
    p
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnsiMetrics {
    pub cell_width: f64,
    pub line_height: f64,
    pub descent: f64,
}

/// An owned `CTFont` (retained; released on drop). Held as a raw pointer
/// because `CFRetained` lives in a crate this module cannot name.
pub struct AnsiCoreFont {
    ptr: NonNull<CTFont>,
}

impl AnsiCoreFont {
    pub fn as_ref(&self) -> &CTFont {
        unsafe { self.ptr.as_ref() }
    }

    pub fn clone_ref(&self) -> AnsiCoreFont {
        let _ = unsafe { CFRetain(self.ptr.as_ptr() as *const c_void) };
        AnsiCoreFont { ptr: self.ptr }
    }
}

impl Drop for AnsiCoreFont {
    fn drop(&mut self) {
        unsafe { CFRelease(self.ptr.as_ptr() as *const c_void) };
    }
}

/// An owned `CGImage` (retained; released on drop).
pub struct AnsiCoreImage {
    ptr: NonNull<CGImage>,
}

impl AnsiCoreImage {
    pub fn as_ref(&self) -> &CGImage {
        unsafe { self.ptr.as_ref() }
    }

    pub fn width(&self) -> usize {
        CGImage::width(Some(self.as_ref()))
    }

    pub fn height(&self) -> usize {
        CGImage::height(Some(self.as_ref()))
    }
}

impl Drop for AnsiCoreImage {
    fn drop(&mut self) {
        unsafe { CFRelease(self.ptr.as_ptr() as *const c_void) };
    }
}

/// Take ownership of a `CFRetained<T>` without naming its type: leak the
/// wrapper (its +1 retain is kept) and rebuild the handle over the raw pointer.
fn take_font<F: std::ops::Deref<Target = CTFont>>(owner: F) -> AnsiCoreFont {
    let ptr = NonNull::from(&*owner);
    std::mem::forget(owner);
    AnsiCoreFont { ptr }
}

fn take_image<F: std::ops::Deref<Target = CGImage>>(owner: F) -> AnsiCoreImage {
    let ptr = NonNull::from(&*owner);
    std::mem::forget(owner);
    AnsiCoreImage { ptr }
}

fn font_named(name: &str, size: f64) -> AnsiCoreFont {
    let ns = NSString::from_str(name);
    let cf = ffi_cast(&*ns);
    take_font(unsafe { CTFont::with_name(cf, size, ptr::null()) })
}

pub struct AnsiRender;

impl AnsiRender {
    pub fn font(t: &AnsiTheme) -> AnsiCoreFont {
        let f = font_named(&t.font_name, t.font_size);
        let mono = unsafe { f.as_ref().symbolic_traits() }
            .contains(CTFontSymbolicTraits::TraitMonoSpace);
        let family = unsafe { f.as_ref().family_name() }.to_string();
        if mono || family == t.font_name {
            f
        } else {
            font_named("Menlo", t.font_size)
        }
    }

    pub fn metrics(f: &AnsiCoreFont) -> AnsiMetrics {
        let font = f.as_ref();
        let mut ch: u16 = 0x4D;
        let mut glyph: CGGlyph = 0;
        unsafe { font.glyphs_for_characters(NonNull::from(&mut ch), NonNull::from(&mut glyph), 1) };
        let mut adv = NSSize::ZERO;
        unsafe {
            font.advances_for_glyphs(
                CTFontOrientation::Horizontal,
                NonNull::from(&mut glyph),
                &mut adv,
                1,
            )
        };
        let descent = unsafe { font.descent() };
        AnsiMetrics {
            cell_width: adv.width,
            line_height: (unsafe { font.ascent() } + descent + unsafe { font.leading() }).ceil(),
            descent: descent.ceil(),
        }
    }

    pub fn size(grid: &AnsiGrid, m: &AnsiMetrics, padding: f64) -> (f64, f64) {
        (
            ((grid.columns() as f64) * m.cell_width + padding * 2.0).ceil(),
            (grid.rows.len() as f64) * m.line_height + padding * 2.0,
        )
    }

    pub fn scale(width: f64, height: f64, wanted: f64, max_pixels: f64) -> f64 {
        if width.max(height) * wanted > max_pixels {
            1.0
        } else {
            wanted
        }
    }

    /// `CGRect.integral`: floor the origin, grow the size to the next integer.
    pub fn integral(x: f64, y: f64, w: f64, h: f64) -> (f64, f64, f64, f64) {
        let x0 = x.floor();
        let y0 = y.floor();
        let x1 = (x + w).ceil();
        let y1 = (y + h).ceil();
        (x0, y0, x1 - x0, y1 - y0)
    }

    /// Swift pins a glyph only when the cell holds one Unicode scalar; the
    /// per-font glyph lookup is checked separately at draw time.
    pub fn single_scalar(text: &str) -> bool {
        text.chars().count() == 1
    }

    pub fn image(
        grid: &AnsiGrid,
        t: &AnsiTheme,
        padding: f64,
        wanted: f64,
    ) -> Option<AnsiCoreImage> {
        let base = AnsiRender::font(t);
        let m = AnsiRender::metrics(&base);
        let (w, h) = AnsiRender::size(grid, &m, padding);
        if w <= 0.0 || h <= 0.0 {
            return None;
        }
        let sc = AnsiRender::scale(w, h, wanted, 32000.0);
        let space_name = unsafe {
            if t.display_p3 {
                kCGColorSpaceDisplayP3
            } else {
                kCGColorSpaceSRGB
            }
        };
        let space_owner = CGColorSpace::with_name(Some(space_name));
        let space = space_owner.as_deref()?;
        let ctx_owner = unsafe {
            CGBitmapContextCreate(
                ptr::null_mut(),
                (w * sc) as usize,
                (h * sc) as usize,
                8,
                0,
                Some(space),
                CGImageAlphaInfo::PremultipliedLast.0,
            )
        };
        let ctx = ctx_owner.as_deref()?;
        CGContext::scale_ctm(Some(ctx), sc, sc);

        let set_fill = |ctx: &CGContext, c: AnsiRGB, a: f64| {
            let comps = [c.r as f64 / 255.0, c.g as f64 / 255.0, c.b as f64 / 255.0, a];
            let color = unsafe { CGColor::new(Some(space), comps.as_ptr()) };
            if let Some(color) = color.as_deref() {
                CGContext::set_fill_color_with_color(Some(ctx), Some(color));
            }
        };
        set_fill(ctx, t.background, 1.0);
        CGContext::fill_rect(Some(ctx), NSRect::new(NSPoint::ZERO, NSSize::new(w, h)));
        CGContext::set_should_smooth_fonts(Some(ctx), false);
        unsafe { CGContext::set_text_matrix(Some(ctx), CGAffineTransformIdentity) };

        let base_ref = base.as_ref();
        let ul_pos = unsafe { base_ref.underline_position() };
        let ul_thick = unsafe { base_ref.underline_thickness() }.max(1.0);
        let x_height = unsafe { base_ref.x_height() };

        let mut variants: HashMap<u8, AnsiCoreFont> = HashMap::new();

        for (r, row) in grid.rows.iter().enumerate() {
            let top = h - padding - (r as f64 + 1.0) * m.line_height;
            let baseline = top + m.descent;

            for (c, cell) in row.iter().enumerate() {
                if let Some(bg) = t.colors(&cell.style).1 {
                    set_fill(ctx, bg, 1.0);
                    let (x, y, ww, hh) = AnsiRender::integral(
                        padding + c as f64 * m.cell_width,
                        top,
                        m.cell_width,
                        m.line_height,
                    );
                    CGContext::fill_rect(
                        Some(ctx),
                        NSRect::new(NSPoint::new(x, y), NSSize::new(ww, hh)),
                    );
                }
            }

            for (c, cell) in row.iter().enumerate() {
                if cell.text.is_empty() {
                    continue;
                }
                let x = padding + c as f64 * m.cell_width;
                let fg = t.colors(&cell.style).0;
                let alpha = if cell.style.dim { 0.5 } else { 1.0 };
                let ww = (cell.width.max(1) as f64) * m.cell_width;
                if cell.style.underline {
                    set_fill(ctx, fg, alpha);
                    CGContext::fill_rect(
                        Some(ctx),
                        NSRect::new(
                            NSPoint::new(x, baseline + ul_pos - ul_thick / 2.0),
                            NSSize::new(ww, ul_thick),
                        ),
                    );
                }
                if cell.style.strike {
                    set_fill(ctx, fg, alpha);
                    CGContext::fill_rect(
                        Some(ctx),
                        NSRect::new(
                            NSPoint::new(x, baseline + x_height / 2.0),
                            NSSize::new(ww, ul_thick),
                        ),
                    );
                }
                if cell.text == " " {
                    continue;
                }
                let key = (if cell.style.bold { 1u8 } else { 0 })
                    | (if cell.style.italic { 2u8 } else { 0 });
                let font: &AnsiCoreFont = if key == 0 {
                    &base
                } else {
                    let mut traits = CTFontSymbolicTraits::empty();
                    if cell.style.bold {
                        traits.insert(CTFontSymbolicTraits::TraitBold);
                    }
                    if cell.style.italic {
                        traits.insert(CTFontSymbolicTraits::TraitItalic);
                    }
                    variants.entry(key).or_insert_with(|| {
                        match unsafe {
                            base_ref.copy_with_symbolic_traits(0.0, ptr::null(), traits, traits)
                        } {
                            Some(owner) => take_font(owner),
                            None => base.clone_ref(),
                        }
                    })
                };
                let utf16: Vec<u16> = cell.text.encode_utf16().collect();
                let mut glyphs: Vec<CGGlyph> = vec![0; utf16.len()];
                if AnsiRender::single_scalar(&cell.text)
                    && unsafe {
                        font.as_ref().glyphs_for_characters(
                            NonNull::from(&utf16[0]),
                            NonNull::from(&mut glyphs[0]),
                            utf16.len() as isize,
                        )
                    }
                {
                    set_fill(ctx, fg, alpha);
                    CGContext::set_text_position(Some(ctx), 0.0, 0.0);
                    let mut pos = NSPoint::new(x, baseline);
                    unsafe {
                        font.as_ref().draw_glyphs(
                            NonNull::from(&glyphs[0]),
                            NonNull::from(&mut pos),
                            1,
                            ctx,
                        )
                    };
                } else {
                    let ns = NSString::from_str(&cell.text);
                    let attr = NSMutableAttributedString::initWithString(
                        NSMutableAttributedString::alloc(),
                        &ns,
                    );
                    let len = utf16.len();
                    let font_obj: &AnyObject = ffi_cast(font.as_ref());
                    unsafe {
                        attr.addAttribute_value_range(
                            ffi_cast::<_, NSString>(kCTFontAttributeName),
                            font_obj,
                            NSRange::new(0, len),
                        )
                    };
                    let comps = [
                        fg.r as f64 / 255.0,
                        fg.g as f64 / 255.0,
                        fg.b as f64 / 255.0,
                        alpha,
                    ];
                    let color = unsafe { CGColor::new(Some(space), comps.as_ptr()) };
                    if let Some(color) = color.as_deref() {
                        let color_obj: &AnyObject = ffi_cast(color);
                        unsafe {
                            attr.addAttribute_value_range(
                                ffi_cast::<_, NSString>(kCTForegroundColorAttributeName),
                                color_obj,
                                NSRange::new(0, len),
                            )
                        };
                    }
                    let line = unsafe {
                        let cf = ffi_cast(&*attr);
                        CTLine::with_attributed_string(cf)
                    };
                    let lw = unsafe {
                        line.typographic_bounds(ptr::null_mut(), ptr::null_mut(), ptr::null_mut())
                    };
                    CGContext::set_text_position(
                        Some(ctx),
                        x + (ww - lw as f64).max(0.0) / 2.0,
                        baseline,
                    );
                    unsafe { line.draw(ctx) };
                }
            }
        }

        let image = CGBitmapContextCreateImage(Some(ctx))?;
        Some(take_image(image))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ESC: char = '\u{1b}';

    fn sgr(params: &str) -> AnsiStyle {
        let mut s = AnsiStyle::default();
        apply_sgr(params, &mut s);
        s
    }

    fn sgr_from(params: &str, from: &AnsiStyle) -> AnsiStyle {
        let mut s = *from;
        apply_sgr(params, &mut s);
        s
    }

    fn row_text(row: &[AnsiCell]) -> String {
        row.iter().map(|c| c.text.as_str()).collect()
    }

    fn all_on() -> AnsiStyle {
        sgr("1;2;3;4;7;9")
    }

    #[test]
    fn sgr_extended_colors() {
        assert_eq!(sgr("38;5;12").fg, AnsiColor::Index(12), "256-color fg");
        assert_eq!(sgr("48;5;8").bg, AnsiColor::Index(8), "256-color bg");
        assert_eq!(
            sgr("38;2;10;20;30").fg,
            AnsiColor::Rgb(AnsiRGB::new(10, 20, 30)),
            "truecolor fg"
        );
        assert_eq!(
            sgr("38:2:10:20:30").fg,
            AnsiColor::Rgb(AnsiRGB::new(10, 20, 30)),
            "colon-separated truecolor"
        );
    }

    #[test]
    fn sgr_basic_and_bright() {
        assert_eq!(sgr("31").fg, AnsiColor::Index(1));
        assert_eq!(sgr("94").fg, AnsiColor::Index(12), "basic + bright fg");
        assert_eq!(sgr("41").bg, AnsiColor::Index(1));
        assert_eq!(sgr("103").bg, AnsiColor::Index(11), "basic + bright bg");
    }

    #[test]
    fn sgr_attributes() {
        let on = all_on();
        assert!(
            on.bold && on.dim && on.italic && on.underline && on.inverse && on.strike,
            "attributes on"
        );
        assert_eq!(sgr_from("0", &on), AnsiStyle::default(), "0 resets");
        assert_eq!(sgr_from("", &on), AnsiStyle::default(), "empty = reset");
        let off = sgr_from("22;23;24;27;29", &all_on());
        assert!(
            !off.bold && !off.dim && !off.italic && !off.underline && !off.inverse && !off.strike,
            "attributes off"
        );
        assert_eq!(sgr_from("39", &sgr("31")).fg, AnsiColor::None, "39 = default fg");
        let mixed = sgr("1;38;5;200;4");
        assert_eq!(mixed.fg, AnsiColor::Index(200));
        assert!(mixed.underline, "params after an extended color");
        assert_eq!(sgr("38;5").fg, AnsiColor::None, "truncated extended color ignored");
    }

    #[test]
    fn grid_styled_rows() {
        let text = format!(
            "{ESC}[0m{ESC}[38;5;7mab{ESC}[0m\r\n\r\ncd{ESC}[1me\r\n"
        );
        let g = AnsiGrid::parse(&text);
        assert_eq!(g.rows.len(), 3, "rows");
        assert_eq!(row_text(&g.rows[0]), "ab");
        assert_eq!(g.rows[0][0].style.fg, AnsiColor::Index(7), "styled row");
        assert!(g.rows[1].is_empty(), "empty row kept");
        assert!(
            g.rows[2][2].style.bold && !g.rows[2][1].style.bold,
            "style starts mid-row"
        );
        assert_eq!(g.columns(), 3, "columns = widest row");
    }

    #[test]
    fn grid_blank_row_trimming() {
        assert_eq!(AnsiGrid::parse("x\n   \n\n").rows.len(), 1, "trailing blank rows trimmed");
        let kept = AnsiGrid::parse(&format!("x\n{ESC}[41m  {ESC}[0m\n"));
        assert_eq!(kept.rows.len(), 2, "a blank row with a background is content");
    }

    #[test]
    fn grid_cr_tab_and_escapes() {
        assert_eq!(
            row_text(&AnsiGrid::parse("hello\rJ\n").rows[0]),
            "Jello",
            "lone CR overwrites from column 0"
        );
        assert_eq!(row_text(&AnsiGrid::parse("a\tb").rows[0]), "a       b", "tab to column 8");
        let osc = AnsiGrid::parse(&format!(
            "{ESC}]8;;https://x.y\u{07}link{ESC}]8;;{ESC}\\ done"
        ));
        assert_eq!(row_text(&osc.rows[0]), "link done", "OSC 8 links dropped");
        let csi = AnsiGrid::parse(&format!("a{ESC}[2Kb{ESC}[?25lc{ESC}(Bd"));
        assert_eq!(row_text(&csi.rows[0]), "abcd", "other CSI / charset escapes dropped");
        assert_eq!(
            row_text(&AnsiGrid::parse("a\u{07}b\u{08}c").rows[0]),
            "abc",
            "control characters dropped"
        );
    }

    #[test]
    fn widths() {
        assert_eq!(cell_width("a"), 1);
        assert_eq!(cell_width("─"), 1, "narrow");
        assert_eq!(cell_width("中"), 2);
        assert_eq!(cell_width("한"), 2, "CJK wide");
        assert_eq!(cell_width("😀"), 2, "emoji wide");
        assert_eq!(cell_width("❤\u{fe0f}"), 2, "VS16 makes it wide");
        assert_eq!(cell_width("\u{f121}"), 1, "nerd font icon (private use) narrow");
    }

    #[test]
    fn wide_cells_occupy_two_columns() {
        let g = AnsiGrid::parse("中x");
        assert_eq!(g.rows[0].len(), 3);
        assert_eq!(g.rows[0][1].text, "");
        assert_eq!(g.rows[0][2].text, "x", "wide cell + its right half");
        assert_eq!(
            AnsiGrid::parse("e\u{301}x").rows[0].len(),
            2,
            "combining mark rides in its grapheme"
        );
    }

    fn ghostty() -> AnsiTheme {
        AnsiTheme::ghostty(
            "\nfont-family = JetBrainsMono Nerd Font\n\
             font-family = Fallback Font\n\
             font-size = 15\n\
             background = #1a1b26\n\
             foreground = #c0caf5\n\
             palette = 4=#7aa2f7\n\
             palette = 200=#123456\n\
             bold-is-bright = true\n\
             window-colorspace = display-p3\n\
             junk line\n",
        )
    }

    #[test]
    fn theme_xterm256() {
        let x = xterm256();
        assert_eq!(x.len(), 256, "256 colors");
        assert_eq!(x[16], AnsiRGB::new(0, 0, 0));
        assert_eq!(x[21], AnsiRGB::new(0, 0, 255));
        assert_eq!(x[196], AnsiRGB::new(255, 0, 0), "cube");
        assert_eq!(x[232], AnsiRGB::new(8, 8, 8));
        assert_eq!(x[255], AnsiRGB::new(238, 238, 238), "grays");
    }

    #[test]
    fn theme_ghostty() {
        let t = ghostty();
        assert_eq!(t.font_name, "JetBrainsMono Nerd Font", "first font-family wins");
        assert_eq!(t.font_size, 15.0, "font-size");
        assert_eq!(t.background, AnsiRGB::new(0x1a, 0x1b, 0x26));
        assert_eq!(t.foreground, AnsiRGB::new(0xc0, 0xca, 0xf5), "fg / bg");
        assert_eq!(t.palette[4], AnsiRGB::new(0x7a, 0xa2, 0xf7));
        assert_eq!(t.palette[200], AnsiRGB::new(0x12, 0x34, 0x56), "palette");
        assert!(t.bold_is_bright && t.display_p3, "bold-is-bright + colorspace");
    }

    #[test]
    fn theme_colors() {
        let t = ghostty();
        let s = AnsiStyle::default();
        let (fg, bg) = t.colors(&s);
        assert_eq!(fg, t.foreground);
        assert_eq!(bg, None, "defaults");
        let mut s = AnsiStyle::default();
        s.fg = AnsiColor::Index(1);
        s.bold = true;
        assert_eq!(t.colors(&s).0, t.palette[9], "bold-is-bright");
        let mut inv = AnsiStyle::default();
        inv.inverse = true;
        let (fg, bg) = t.colors(&inv);
        assert_eq!(fg, t.background);
        assert_eq!(bg, Some(t.foreground), "inverse swaps");
    }

    #[test]
    fn geometry_helpers() {
        let m = AnsiMetrics {
            cell_width: 8.0,
            line_height: 16.0,
            descent: 3.0,
        };
        let g = AnsiGrid::parse("ab\ncde\n");
        let (w, h) = AnsiRender::size(&g, &m, 10.0);
        assert_eq!(h, 2.0 * m.line_height + 20.0, "height = rows × line + padding");
        assert_eq!(w, (3.0 * m.cell_width + 20.0).ceil(), "width = columns × cell + padding");
        assert_eq!(AnsiRender::scale(800.0, 15000.0, 2.0, 32000.0), 2.0, "2× when it fits");
        assert_eq!(AnsiRender::scale(800.0, 17000.0, 2.0, 32000.0), 1.0, "1× when huge");
    }

    #[test]
    fn glyph_selection() {
        assert!(AnsiRender::single_scalar("a"));
        assert!(AnsiRender::single_scalar("中"));
        assert!(AnsiRender::single_scalar("😀"), "one non-BMP scalar");
        assert!(
            !AnsiRender::single_scalar("❤\u{fe0f}"),
            "heart + VS16 is two scalars, so CTLineDraw"
        );
        assert!(!AnsiRender::single_scalar("ab"));
        assert!(
            !AnsiRender::single_scalar("e\u{301}"),
            "a combining mark means two scalars, so CTLineDraw, not a pinned glyph"
        );
    }

    #[test]
    fn integral_rect() {
        assert_eq!(AnsiRender::integral(1.2, 2.8, 3.4, 0.5), (1.0, 2.0, 4.0, 2.0));
        assert_eq!(AnsiRender::integral(0.0, 0.0, 8.0, 16.0), (0.0, 0.0, 8.0, 16.0));
        assert_eq!(AnsiRender::integral(4.1, 7.9, 8.0, 16.0), (4.0, 7.0, 9.0, 17.0));
    }
}
