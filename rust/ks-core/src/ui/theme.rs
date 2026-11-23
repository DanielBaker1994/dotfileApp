//! Theme / color model, ported from `PopupWindow.swift` (`PopupColors`,
//! `PopupPalette`, `PopupTone`, `ButtonStyle` color helpers, `HeaderStyle`,
//! `PopupThemeDefaults`) and `kitchen_sink.swift` (`THEME` / `BAR` /
//! `GROUP_BG` / `TEXT` / `DIM`, `hexColor` / `hexString`, `ThemePreset`).
//!
//! The token model is plain `f64` RGBA so it stays testable without AppKit;
//! only [`Rgba::to_nscolor`] crosses into `objc2_app_kit::NSColor`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};

#[cfg(target_os = "macos")]
use objc2::rc::Retained;
#[cfg(target_os = "macos")]
use objc2_app_kit::NSColor;

/// An sRGB color, components in `0.0...1.0` (matches `NSColor` in `.sRGB`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgba {
    pub r: f64,
    pub g: f64,
    pub b: f64,
    pub a: f64,
}

impl Rgba {
    pub const BLACK: Rgba = Rgba::new(0.0, 0.0, 0.0, 1.0);
    pub const WHITE: Rgba = Rgba::new(1.0, 1.0, 1.0, 1.0);

    pub const fn new(r: f64, g: f64, b: f64, a: f64) -> Self {
        Rgba { r, g, b, a }
    }

    pub fn from_u8(r: u8, g: u8, b: u8, a: u8) -> Self {
        Rgba::new(
            r as f64 / 255.0,
            g as f64 / 255.0,
            b as f64 / 255.0,
            a as f64 / 255.0,
        )
    }

    pub fn with_alpha(self, a: f64) -> Self {
        Rgba { a, ..self }
    }

    /// `ButtonStyle.opaque` / `NSColor.usingColorSpace(.sRGB).withAlphaComponent(1)`.
    pub fn opaque(self) -> Self {
        self.with_alpha(1.0)
    }

    /// `NSColor.blended(withFraction:of:)` — per-channel linear interpolation.
    pub fn blended(self, fraction: f64, of: Rgba) -> Self {
        let f = fraction;
        Rgba::new(
            self.r * (1.0 - f) + of.r * f,
            self.g * (1.0 - f) + of.g * f,
            self.b * (1.0 - f) + of.b * f,
            self.a * (1.0 - f) + of.a * f,
        )
    }

    /// `ButtonStyle.luminance` / `NSColor.relativeLuminance` (WCAG).
    pub fn luminance(self) -> f64 {
        let s = self.opaque();
        fn lin(v: f64) -> f64 {
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * lin(s.r) + 0.7152 * lin(s.g) + 0.0722 * lin(s.b)
    }

    pub fn relative_luminance(self) -> f64 {
        self.luminance()
    }

    /// `ButtonStyle.contrast`.
    pub fn contrast(self, other: Rgba) -> f64 {
        let la = self.luminance();
        let lb = other.luminance();
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    /// `NSColor(srgbRed:green:blue:alpha:)`.
    #[cfg(target_os = "macos")]
    pub fn to_nscolor(self) -> Retained<NSColor> {
        NSColor::colorWithSRGBRed_green_blue_alpha(self.r, self.g, self.b, self.a)
    }
}

/// `hexColor(_:)` (parse) / `hexString(_:)` (format) from `kitchen_sink.swift`.
pub struct HexColor;

impl HexColor {
    /// `hexColor`: optional `0x` / `#` prefix, exactly 6 or 8 hex digits,
    /// alpha (8-digit) clamped to a minimum of `0.08`.
    pub fn parse(s: &str) -> Option<Rgba> {
        if s.is_empty() {
            return None;
        }
        let hex = s.strip_prefix("0x").unwrap_or(s);
        let hex = hex.strip_prefix('#').unwrap_or(hex);
        if hex.len() != 6 && hex.len() != 8 {
            return None;
        }
        if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let v = u32::from_str_radix(hex, 16).ok()?;
        let has_alpha = hex.len() == 8;
        let a = if has_alpha {
            ((v >> 24) & 0xFF) as f64 / 255.0
        } else {
            1.0
        };
        Some(Rgba::new(
            ((v >> 16) & 0xFF) as f64 / 255.0,
            ((v >> 8) & 0xFF) as f64 / 255.0,
            (v & 0xFF) as f64 / 255.0,
            a.max(0.08),
        ))
    }

    /// `hexString`: `AARRGGBB` when alpha < 255, else `RRGGBB` (uppercase).
    pub fn format(c: &Rgba) -> String {
        let r = (c.r * 255.0).round().clamp(0.0, 255.0) as u8;
        let g = (c.g * 255.0).round().clamp(0.0, 255.0) as u8;
        let b = (c.b * 255.0).round().clamp(0.0, 255.0) as u8;
        let a = (c.a * 255.0).round().clamp(0.0, 255.0) as u8;
        if a < 255 {
            format!("{a:02X}{r:02X}{g:02X}{b:02X}")
        } else {
            format!("{r:02X}{g:02X}{b:02X}")
        }
    }

    /// `paletteString`: always `RRGGBB` (alpha dropped).
    pub fn format_rgb(c: &Rgba) -> String {
        let r = (c.r * 255.0).round().clamp(0.0, 255.0) as u8;
        let g = (c.g * 255.0).round().clamp(0.0, 255.0) as u8;
        let b = (c.b * 255.0).round().clamp(0.0, 255.0) as u8;
        format!("{r:02X}{g:02X}{b:02X}")
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PopupPalette {
    pub accent2: Rgba,
    pub success: Rgba,
    pub warning: Rgba,
    pub danger: Rgba,
    pub info: Rgba,
}

impl Default for PopupPalette {
    fn default() -> Self {
        PopupPalette {
            accent2: Rgba::from_u8(138, 173, 244, 255),
            success: Rgba::from_u8(166, 218, 149, 255),
            warning: Rgba::from_u8(238, 212, 159, 255),
            danger: Rgba::from_u8(237, 135, 150, 255),
            info: Rgba::from_u8(145, 215, 227, 255),
        }
    }
}

impl PopupPalette {
    pub fn all(&self) -> [Rgba; 5] {
        [
            self.accent2,
            self.success,
            self.warning,
            self.danger,
            self.info,
        ]
    }

    pub fn from_slice(colors: &[Rgba]) -> Option<PopupPalette> {
        if colors.len() != 5 {
            return None;
        }
        Some(PopupPalette {
            accent2: colors[0],
            success: colors[1],
            warning: colors[2],
            danger: colors[3],
            info: colors[4],
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PopupTone {
    Text,
    Dim,
    Accent,
    Accent2,
    Success,
    Warning,
    Danger,
    Info,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PopupColors {
    pub background: Rgba,
    pub border: Rgba,
    pub text: Rgba,
    pub dim: Rgba,
    pub highlight: Rgba,
    pub accent: Rgba,
    pub palette: PopupPalette,
}

impl Default for PopupColors {
    fn default() -> Self {
        PopupColors {
            background: Rgba::from_u8(36, 39, 58, 255),
            border: Rgba::from_u8(159, 200, 232, 255),
            text: Rgba::from_u8(202, 211, 245, 255),
            dim: Rgba::from_u8(147, 154, 183, 255),
            highlight: Rgba::from_u8(63, 74, 90, 255),
            accent: Rgba::from_u8(85, 104, 130, 255),
            palette: PopupPalette::default(),
        }
    }
}

impl PopupColors {
    pub fn is_light(&self) -> bool {
        self.background.luminance() > 0.45
    }

    /// `ButtonStyle.opaque(background)`.
    pub fn base(&self) -> Rgba {
        self.background.opaque()
    }

    fn deeper(&self, f: f64) -> Rgba {
        if self.is_light() {
            self.base().blended(f * 0.35, self.text.opaque())
        } else {
            self.base().blended(f, Rgba::BLACK)
        }
    }

    pub fn mantle(&self) -> Rgba {
        self.deeper(0.22)
    }

    pub fn crust(&self) -> Rgba {
        self.deeper(0.42)
    }

    pub fn surface0(&self) -> Rgba {
        self.base().blended(0.09, self.text.opaque())
    }

    pub fn surface1(&self) -> Rgba {
        self.base().blended(0.16, self.text.opaque())
    }

    /// `ButtonStyle.accent(self)`.
    pub fn accent_on(&self) -> Rgba {
        let card = self.background.opaque();
        let mut a = self.accent.opaque();
        let mut step = 0;
        while a.contrast(card) < 2.2 && step < 6 {
            a = a.blended(0.2, self.text.opaque());
            step += 1;
        }
        a
    }

    pub fn on_accent(&self) -> Rgba {
        let a = self.accent_on();
        if self.crust().contrast(a) >= 4.5 {
            self.crust()
        } else if Rgba::WHITE.contrast(a) >= Rgba::BLACK.contrast(a) {
            Rgba::WHITE
        } else {
            Rgba::BLACK
        }
    }

    /// `readable(_:min:)` — default ratio 3, six 0.2 steps toward `text`.
    pub fn readable(&self, c: Rgba, min: f64) -> Rgba {
        let mut out = c.opaque();
        let mut step = 0;
        while out.contrast(self.base()) < min && step < 6 {
            out = out.blended(0.2, self.text.opaque());
            step += 1;
        }
        out
    }

    pub fn tone(&self, t: PopupTone) -> Rgba {
        match t {
            PopupTone::Text => self.text,
            PopupTone::Dim => self.dim,
            PopupTone::Accent => self.accent_on(),
            PopupTone::Accent2 => self.readable(self.palette.accent2, 3.0),
            PopupTone::Success => self.readable(self.palette.success, 3.0),
            PopupTone::Warning => self.readable(self.palette.warning, 3.0),
            PopupTone::Danger => self.readable(self.palette.danger, 3.0),
            PopupTone::Info => self.readable(self.palette.info, 3.0),
        }
    }

    pub fn over(&self, top: Rgba, alpha: f64, bottom: Rgba) -> Rgba {
        bottom.opaque().blended(alpha, top.opaque())
    }

    /// `ensure(_:on:_:)` — default ratio 4.5, twelve 0.15 steps toward the pole.
    pub fn ensure(&self, fg: Rgba, bg: Rgba, ratio: f64) -> Rgba {
        let b = bg.opaque();
        let pole = if Rgba::BLACK.contrast(b) >= Rgba::WHITE.contrast(b) {
            Rgba::BLACK
        } else {
            Rgba::WHITE
        };
        let mut out = fg.opaque();
        let mut step = 0;
        while out.contrast(b) < ratio && step < 12 {
            out = out.blended(0.15, pole);
            step += 1;
        }
        out
    }

    pub fn hairline(&self) -> Rgba {
        self.text.with_alpha(if self.is_light() { 0.12 } else { 0.08 })
    }

    pub fn outline(&self) -> Rgba {
        self.accent_on().blended(0.45, self.base()).with_alpha(0.85)
    }
}

/// `PopupThemeDefaults` — the window colors from the loaded `[theme]`
/// (Swift `windowColors()` over `THEME`), else the built-in defaults.
pub struct PopupThemeDefaults;

impl PopupThemeDefaults {
    pub fn colors() -> PopupColors {
        theme_state().map(|t| t.globals.window_colors()).unwrap_or_default()
    }
}

/// The loaded theme: `[theme]` globals + each section's color keys.
#[derive(Clone, Debug, Default)]
pub struct LoadedTheme {
    pub globals: ThemeGlobals,
    pub sections: HashMap<String, HashMap<String, String>>,
}

static THEME_STATE: std::sync::RwLock<Option<LoadedTheme>> = std::sync::RwLock::new(None);

fn theme_state() -> Option<LoadedTheme> {
    THEME_STATE.read().ok().and_then(|g| g.clone())
}

/// Install the theme parsed from commands.toml (`parseTheme()`); `sections`
/// maps each section to its raw entries (only the color keys are read).
pub fn install_theme(theme: &HashMap<String, String>, sections: HashMap<String, HashMap<String, String>>) {
    let map: HashMap<String, Rgba> = theme
        .iter()
        .filter_map(|(k, v)| HexColor::parse(v.trim()).map(|c| (k.to_lowercase(), c)))
        .collect();
    let globals = ThemeGlobals::from_overrides(&map);
    if let Ok(mut g) = THEME_STATE.write() {
        *g = Some(LoadedTheme { globals, sections });
    }
}

fn hsb(c: Rgba) -> (f64, f64, f64) {
    let (r, g, b) = (c.r, c.g, c.b);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / d) % 6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    let h = if h < 0.0 { h + 360.0 } else { h };
    let s = if max == 0.0 { 0.0 } else { d / max };
    (h, s, max)
}

fn from_hsb(h: f64, s: f64, v: f64) -> Rgba {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match (h / 60.0) as i64 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    Rgba::new(r + m, g + m, b + m, 1.0)
}

/// `PopupWindow.ansiPalette(_:)` — the 16 ANSI colors derived from the theme
/// (blue / magenta picked by hue from accent / accent2 / info, black / white
/// from the surfaces, brights lifted 15 %).
pub fn ansi_palette(c: &PopupColors) -> Vec<Rgba> {
    let p = c.palette;
    let dist = |a: f64, b: f64| {
        let d = (a - b).abs();
        d.min(360.0 - d)
    };
    let pick = |target: f64, from: &[Rgba], base: Rgba| -> Rgba {
        let best = from
            .iter()
            .copied()
            .min_by(|x, y| dist(hsb(x.opaque()).0, target).partial_cmp(&dist(hsb(y.opaque()).0, target)).unwrap())
            .unwrap_or(base);
        if dist(hsb(best.opaque()).0, target) <= 50.0 {
            return best;
        }
        let (_, sat, bri) = hsb(base.opaque());
        from_hsb(target, sat.max(0.35), bri)
    };
    let blue = pick(220.0, &[c.accent, p.accent2, p.info], c.accent);
    let mag_from: Vec<Rgba> = [c.accent, p.accent2].into_iter().filter(|x| *x != blue).collect();
    let magenta = pick(300.0, &mag_from, p.accent2);
    let light = c.is_light();
    let black = if light { c.dim.blended(0.35, c.text) } else { c.surface1() };
    let white = if light { c.surface1() } else { c.dim };
    let normal = [black, p.danger, p.success, p.warning, blue, magenta, p.info, white];
    let mut out: Vec<Rgba> = normal.iter().map(|x| x.opaque()).collect();
    for (i, x) in normal.iter().enumerate() {
        let b = if i == 0 {
            if light { c.dim } else { c.highlight.blended(0.25, c.text) }
        } else if i == 7 {
            c.text
        } else {
            x.opaque().blended(0.15, if light { Rgba::new(0.0, 0.0, 0.0, 1.0) } else { Rgba::new(1.0, 1.0, 1.0, 1.0) })
        };
        out.push(b.opaque());
    }
    out
}

/// `parsePalette` — "9D79D6, 81B29A, …" (accent2, success, warning, danger, info).
pub fn parse_palette(s: &str) -> Option<PopupPalette> {
    let cols: Vec<Rgba> = s.split(',').filter_map(|p| HexColor::parse(p.trim())).collect();
    PopupPalette::from_slice(&cols)
}

/// `cardColors(_:)` / `windowColors(cmd)` — a section's
/// `background-color` / `text-color` / `dim-color` / `highlight-color` /
/// `accent-color` / `palette` over the `[theme]` globals.
pub fn section_colors(section: &str) -> PopupColors {
    let Some(t) = theme_state() else { return PopupColors::default() };
    let g = &t.globals;
    let Some(sec) = t.sections.get(section) else { return g.window_colors() };
    let v = |k: &str| sec.get(k).and_then(|x| HexColor::parse(x.trim()));
    let mut c = PopupColors {
        background: v("background-color").map(|b| b.with_alpha(1.0)).unwrap_or(g.bar),
        border: g.border,
        text: v("text-color").unwrap_or(g.text),
        dim: v("dim-color").unwrap_or(g.dim),
        highlight: v("highlight-color").unwrap_or(g.group_bg),
        accent: v("accent-color").unwrap_or(g.accent),
        palette: sec.get("palette").and_then(|p| parse_palette(p)).unwrap_or(g.palette),
    };
    if v("accent-color").is_some() || !g.has_border {
        c.border = c.outline();
    }
    c
}

/// A section's `header-color` (else `[theme] header`, else blue-silver).
pub fn section_header_color(section: &str) -> Rgba {
    let t = theme_state().unwrap_or_default();
    t.sections
        .get(section)
        .and_then(|s| s.get("header-color"))
        .and_then(|x| HexColor::parse(x.trim()))
        .unwrap_or_else(|| t.globals.header_color())
}

/// A section's `browser-background` (the file-browser / list fill).
pub fn section_browser_background(section: &str) -> Option<Rgba> {
    let t = theme_state()?;
    t.sections
        .get(section)
        .and_then(|s| s.get("browser-background"))
        .and_then(|x| HexColor::parse(x.trim()))
        .or(t.globals.browser)
}

/// The `[theme]` globals (`THEME` / `BAR` / `GROUP_BG` / `TEXT` / `DIM` …).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ThemeGlobals {
    pub bar: Rgba,
    pub group_bg: Rgba,
    pub text: Rgba,
    pub dim: Rgba,
    pub border: Rgba,
    pub accent: Rgba,
    pub palette: PopupPalette,
    pub header: Option<Rgba>,
    pub browser: Option<Rgba>,
    pub terminal: Option<Rgba>,
    has_border: bool,
}

impl Default for ThemeGlobals {
    fn default() -> Self {
        ThemeGlobals {
            bar: Rgba::from_u8(0x24, 0x27, 0x3A, 255),
            group_bg: Rgba::from_u8(0x3F, 0x4A, 0x5A, 255),
            text: Rgba::from_u8(0xCA, 0xD3, 0xF5, 255),
            dim: Rgba::from_u8(0x93, 0x9A, 0xB7, 255),
            border: Rgba::from_u8(0xC6, 0xA0, 0xF6, 255),
            accent: Rgba::from_u8(85, 104, 130, 255),
            palette: PopupPalette::default(),
            header: None,
            browser: None,
            terminal: None,
            has_border: false,
        }
    }
}

impl ThemeGlobals {
    /// Mirror of `parseTheme()` + the `let BAR = …` derivations.
    pub fn from_overrides(map: &HashMap<String, Rgba>) -> Self {
        let mut g = ThemeGlobals::default();
        let get = |k: &str| map.get(&k.to_lowercase()).copied();
        if let Some(v) = get("background") {
            g.bar = v;
        }
        if let Some(v) = get("highlight") {
            g.group_bg = v;
        }
        if let Some(v) = get("text") {
            g.text = v;
        }
        if let Some(v) = get("dim") {
            g.dim = v;
        }
        if let Some(v) = get("border") {
            g.border = v;
            g.has_border = true;
        }
        if let Some(v) = get("accent") {
            g.accent = v;
        }
        g.header = get("header");
        g.browser = get("browser");
        g.terminal = get("terminal");
        let d = PopupPalette::default();
        g.palette = PopupPalette {
            accent2: get("accent2").unwrap_or(d.accent2),
            success: get("success").unwrap_or(d.success),
            warning: get("warning").unwrap_or(d.warning),
            danger: get("danger").unwrap_or(d.danger),
            info: get("info").unwrap_or(d.info),
        };
        g
    }

    /// `windowColors(nil)` — no per-window overrides.
    pub fn window_colors(&self) -> PopupColors {
        let mut c = PopupColors {
            background: self.bar,
            border: self.border,
            text: self.text,
            dim: self.dim,
            highlight: self.group_bg,
            accent: self.accent,
            palette: self.palette,
        };
        if !self.has_border {
            c.border = c.outline();
        }
        c
    }

    /// `headerBlueSilver` = `THEME["header"] ?? 0.27, 0.31, 0.36`.
    pub fn header_color(&self) -> Rgba {
        self.header
            .unwrap_or_else(|| Rgba::new(0.27, 0.31, 0.36, 1.0))
    }
}

/// `HeaderStyle` from `PopupWindow.swift`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeaderStyle {
    Quiet,
    Flat,
    Edge,
    Stripe,
    Tinted,
    Glow,
    Aurora,
}

impl HeaderStyle {
    pub const ALL: [HeaderStyle; 7] = [
        HeaderStyle::Quiet,
        HeaderStyle::Flat,
        HeaderStyle::Edge,
        HeaderStyle::Stripe,
        HeaderStyle::Tinted,
        HeaderStyle::Glow,
        HeaderStyle::Aurora,
    ];

    pub fn raw_value(self) -> &'static str {
        match self {
            HeaderStyle::Quiet => "quiet",
            HeaderStyle::Flat => "flat",
            HeaderStyle::Edge => "edge",
            HeaderStyle::Stripe => "stripe",
            HeaderStyle::Tinted => "tinted",
            HeaderStyle::Glow => "glow",
            HeaderStyle::Aurora => "aurora",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            HeaderStyle::Quiet => "Quiet",
            HeaderStyle::Flat => "Flat",
            HeaderStyle::Edge => "Accent Edge",
            HeaderStyle::Stripe => "Accent Stripe",
            HeaderStyle::Tinted => "Tinted",
            HeaderStyle::Glow => "Glow",
            HeaderStyle::Aurora => "Aurora",
        }
    }

    pub fn from_raw(value: &str) -> Option<HeaderStyle> {
        HeaderStyle::ALL.into_iter().find(|s| s.raw_value() == value)
    }
}

impl Default for HeaderStyle {
    fn default() -> Self {
        HeaderStyle::Flat
    }
}

static CURRENT_HEADER_STYLE: AtomicU8 = AtomicU8::new(1); // .flat

/// The `HeaderStyle.current` global.
pub fn current_header_style() -> HeaderStyle {
    let i = CURRENT_HEADER_STYLE.load(Ordering::Relaxed) as usize;
    HeaderStyle::ALL.get(i).copied().unwrap_or_default()
}

pub fn set_current_header_style(style: HeaderStyle) {
    let i = HeaderStyle::ALL
        .iter()
        .position(|s| *s == style)
        .unwrap_or(1);
    CURRENT_HEADER_STYLE.store(i as u8, Ordering::Relaxed);
}

/// `ThemePreset` from `kitchen_sink.swift`.
#[derive(Clone, Debug, PartialEq)]
pub struct ThemePreset {
    pub name: String,
    pub background: Rgba,
    pub browser: Rgba,
    pub terminal: Rgba,
    pub header: Rgba,
    pub text: Rgba,
    pub dim: Rgba,
    pub highlight: Rgba,
    pub accent: Rgba,
    pub palette: PopupPalette,
}

/// `ThemePreset.Tone` (`mid = 0`, `dark = 1`, `light = 2`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tone {
    Mid,
    Dark,
    Light,
}

impl Tone {
    pub fn header_title(self) -> &'static str {
        match self {
            Tone::Mid => "Mid Tones",
            Tone::Dark => "Dark",
            Tone::Light => "Light",
        }
    }
}

impl ThemePreset {
    pub fn is_light(&self) -> bool {
        self.background.relative_luminance() > 0.45
    }

    pub fn tone(&self) -> Tone {
        if self.is_light() {
            Tone::Light
        } else if self.background.relative_luminance() >= 0.017 {
            Tone::Mid
        } else {
            Tone::Dark
        }
    }

    /// `defaultPalette(light:)`.
    pub fn default_palette(light: bool) -> PopupPalette {
        if light {
            let cs = ["1E66F5", "40A02B", "DF8E1D", "D20F39", "179299"]
                .map(|h| HexColor::parse(h).unwrap());
            PopupPalette::from_slice(&cs).unwrap()
        } else {
            PopupPalette::default()
        }
    }

    /// `parse(name:_:)` — 7, 8 or 13 comma-separated hex colors.
    pub fn parse(name: &str, value: &str) -> Option<ThemePreset> {
        if name.is_empty() {
            return None;
        }
        let parts: Vec<&str> = value.split(',').map(|s| s.trim()).collect();
        if !matches!(parts.len(), 7 | 8 | 13) {
            return None;
        }
        let colors: Option<Vec<Rgba>> = parts.iter().map(|s| HexColor::parse(s)).collect();
        let colors = colors?;
        let accent = if colors.len() >= 8 {
            colors[7]
        } else {
            colors[4]
        };
        let palette = if colors.len() == 13 {
            PopupPalette::from_slice(&colors[8..13])?
        } else {
            ThemePreset::default_palette(colors[0].relative_luminance() > 0.45)
        };
        Some(ThemePreset {
            name: name.to_string(),
            background: colors[0],
            browser: colors[1],
            terminal: colors[2],
            header: colors[3],
            text: colors[4],
            dim: colors[5],
            highlight: colors[6],
            accent,
            palette,
        })
    }

    /// `ThemePreset.builtIn` plus the hue-forward set — 44 presets, each 13 colors.
    pub fn built_in() -> Vec<ThemePreset> {
        PRESET_DATA
            .iter()
            .map(|(name, value)| {
                ThemePreset::parse(name, value)
                    .unwrap_or_else(|| panic!("invalid built-in preset: {name}"))
            })
            .collect()
    }

    /// `ThemePreset.all()` ignoring the config `[themes]` overlay.
    pub fn all() -> Vec<ThemePreset> {
        Self::built_in()
    }

    /// `ThemePreset.all()` with `[themes]` entries merged (replace by name,
    /// else append).
    pub fn all_from(overrides: &[(String, String)]) -> Vec<ThemePreset> {
        let mut out = Self::built_in();
        for (name, value) in overrides {
            let Some(p) = ThemePreset::parse(name, value) else {
                continue;
            };
            if let Some(i) = out.iter().position(|x| x.name == *name) {
                out[i] = p;
            } else {
                out.push(p);
            }
        }
        out
    }
}

/// `presets.indices.sorted` — by tone, then background luminance descending.
pub fn ordered_by_tone(presets: &[ThemePreset]) -> Vec<ThemePreset> {
    let mut out = presets.to_vec();
    out.sort_by(|a, b| {
        a.tone().cmp(&b.tone()).then_with(|| {
            b.background
                .relative_luminance()
                .partial_cmp(&a.background.relative_luminance())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    });
    out
}

/// The theme menu's tone sections, in order, with their titles.
pub fn grouped_by_tone(presets: &[ThemePreset]) -> Vec<(Tone, &'static str, Vec<ThemePreset>)> {
    let ordered = ordered_by_tone(presets);
    let mut groups: Vec<(Tone, &'static str, Vec<ThemePreset>)> = Vec::new();
    for p in ordered {
        let tone = p.tone();
        if groups.last().map(|g| g.0) != Some(tone) {
            groups.push((tone, tone.header_title(), Vec::new()));
        }
        groups.last_mut().unwrap().2.push(p);
    }
    groups
}

const PRESET_DATA: &[(&str, &str)] = &[
    ("Tokyo Night", "1A1B26, 16161E, 13141C, 111219, C0CAF5, 9AA5CE, 283457, 7AA2F7, BB9AF7, 9ECE6A, E0AF68, F7768E, 7DCFFF"),
    ("Tokyo Night Storm", "24283B, 1F2335, 1B1E2D, 1A1D2B, C0CAF5, 9AA5CE, 2E3C64, 7AA2F7, BB9AF7, 9ECE6A, E0AF68, F7768E, 7DCFFF"),
    ("Ink & Brass", "1A1D24, 171A20, 13161C, 13161C, E7E2D7, 928C80, 2F3440, C9A45C, 7C9CB5, 8DB07A, D8A657, D0705F, 7FA3BF"),
    ("Catppuccin Mocha", "1E1E2E, 181825, 11111B, 11111B, CDD6F4, A6ADC8, 45475A, CBA6F7, 89B4FA, A6E3A1, F9E2AF, F38BA8, 94E2D5"),
    ("Catppuccin Macchiato", "24273A, 1E2030, 181926, 181926, CAD3F5, A5ADCB, 494D64, C6A0F6, 8AADF4, A6DA95, EED49F, ED8796, 8BD5CA"),
    ("Dracula", "282A36, 21222C, 191A21, 191A21, F8F8F2, A4AACC, 44475A, BD93F9, FF79C6, 50FA7B, F1FA8C, FF5555, 8BE9FD"),
    ("Nord", "2E3440, 3B4252, 272C36, 242933, ECEFF4, A3ACBD, 4C566A, 88C0D0, 81A1C1, A3BE8C, EBCB8B, BF616A, 8FBCBB"),
    ("Gruvbox Dark", "282828, 1D2021, 1D2021, 1B1B1B, EBDBB2, A89984, 504945, FABD2F, 83A598, B8BB26, FE8019, FB4934, 8EC07C"),
    ("One Dark", "282C34, 21252B, 1E2127, 1B1E23, ABB2BF, 7F848E, 3E4451, 61AFEF, C678DD, 98C379, E5C07B, E06C75, 56B6C2"),
    ("Rosé Pine", "191724, 1F1D2E, 16141F, 12101A, E0DEF4, 908CAA, 403D52, EBBCBA, C4A7E7, 9CCFD8, F6C177, EB6F92, 31748F"),
    ("Solarized Dark", "002B36, 073642, 00212B, 001E26, 93A1A1, 657B83, 0A4A5A, 268BD2, 6C71C4, 859900, B58900, DC322F, 2AA198"),
    ("Graphite", "1E1E1E, 252525, 181818, 151515, E5E5E5, 9A9A9A, 3A3A3A, 0A84FF, BF5AF2, 30D158, FFD60A, FF453A, 64D2FF"),
    ("Catppuccin Frappé", "303446, 292C3C, 232634, 232634, C6D0F5, A5ADCE, 51576D, CA9EE6, 8CAAEE, A6D189, E5C890, E78284, 81C8BE"),
    ("Tokyo Night Moon", "222436, 1E2030, 191B29, 171927, C8D3F5, 9AA5CE, 2D3F76, 82AAFF, C099FF, C3E88D, FFC777, FF757F, 86E1FC"),
    ("Rosé Pine Moon", "232136, 2A273F, 1D1B2E, 19172A, E0DEF4, 908CAA, 44415A, EA9A97, C4A7E7, 9CCFD8, F6C177, EB6F92, 3E8FB0"),
    ("Everforest Dark", "2D353B, 272E33, 232A2E, 1E2326, D3C6AA, 9DA9A0, 475258, A7C080, D699B6, 83C092, DBBC7F, E67E80, 7FBBB3"),
    ("Palenight", "292D3E, 232635, 1E2130, 1B1E2B, A6ACCD, 8087A2, 444267, C792EA, 82AAFF, C3E88D, FFCB6B, F07178, 89DDFF"),
    ("GitHub Dark Dimmed", "22272E, 1C2128, 1A1E24, 161B22, ADBAC7, 8B98A5, 373E47, 539BF5, DCBDFB, 57AB5A, C69026, E5534B, 96D0FF"),
    ("Ayu Mirage", "1F2430, 1C212B, 171B24, 141820, CCCAC2, 8A9199, 33415E, FFCC66, DFBFFF, D5FF80, FFD173, F28779, 5CCFE6"),
    ("Monokai Pro", "2D2A2E, 221F22, 19181A, 171517, FCFCFA, 939293, 403E41, FFD866, AB9DF2, A9DC76, FC9867, FF6188, 78DCE8"),
    ("Synthwave '84", "262335, 241B2F, 1E1A29, 171520, F0EFF5, 9D98C4, 463465, FF7EDB, 36F9F6, 72F1B8, FEDE5D, FE4450, 03EDF9"),
    ("Kanagawa Wave", "1F1F28, 1A1A22, 16161D, 131318, DCD7BA, C8C093, 2D4F67, 7E9CD8, 957FB8, 98BB6C, E6C384, E46876, 7FB4CA"),
    ("Nightfox", "192330, 131A24, 111720, 0F141C, CDCECF, AEAFB0, 2B3B51, 719CD6, 9D79D6, 81B29A, DBC074, C94F6D, 63CDCF"),
    ("Poimandres", "1B1E28, 171922, 13151D, 111219, E4F0FB, A6ACCD, 303340, 5DE4C7, FCC5E9, 5FB3A1, FFFAC2, D0679D, 89DDFF"),
    ("Night Owl", "011627, 01111D, 010E17, 000C14, D6DEEB, 8BA1B7, 1D3B53, 82AAFF, C792EA, ADDB67, ECC48D, EF5350, 7FDBCA"),
    ("Kanagawa Dragon", "181616, 12120F, 0D0C0C, 0B0A0A, C5C9C5, A6A69C, 2D4F67, 8BA4B0, A292A3, 87A987, C4B28A, C4746E, 8EA4A2"),
    ("Catppuccin Latte", "EFF1F5, E6E9EF, DCE0E8, DCE0E8, 4C4F69, 6C6F85, BCC0CC, 8839EF, 1E66F5, 40A02B, DF8E1D, D20F39, 179299"),
    ("Tokyo Night Day", "E1E2E7, D5D6DB, D0D5E3, C8CCD9, 3760BF, 6172B0, B7C1E3, 2E7DE9, 9854F1, 587539, 8C6C3E, F52A65, 007197"),
    ("Solarized Light", "FDF6E3, EEE8D5, EEE8D5, E4DDC8, 586E75, 839496, DDD6C1, 268BD2, D33682, 859900, B58900, DC322F, 2AA198"),
    ("Paper", "F5F5F5, EDEDED, FFFFFF, E3E3E3, 1D1D1F, 6E6E73, D1D1D6, 007AFF, AF52DE, 248A3D, B25000, D70015, 0071A4"),
    // Hue-forward presets: the classics above are nearly all slate-blue near
    // black or near white, so these spread hue and lightness instead.
    ("Oxblood", "2A1215, 220E11, 1C0B0D, 1A0A0C, F2DEDC, B8999A, 4E2228, E06C5A, E8A0BF, 9BC48A, E8C07A, FF5F56, 7FC8C8"),
    ("Deep Forest", "12261B, 0E1F16, 0B1A12, 0A1710, DCEBDD, 98B39E, 24472F, 7FD38F, C6A0F6, A6E3A1, E5C07B, F07A7A, 6CCFCB"),
    ("Aubergine", "2B1630, 241228, 1E0F21, 1B0D1E, EEDDF2, AE94B5, 4C2856, D88CF2, FF8FB8, 9AD69A, F2CB7E, FF6F86, 82D6E8"),
    ("Espresso", "2B2018, 241A13, 1D150F, 1A130E, EFE1D1, B39C86, 4D3A2B, E0A458, D99CB8, A8C47F, F0C674, E0705A, 8CC0C0"),
    ("Abyss Teal", "0C2E33, 09262A, 072023, 061C1F, D6F0EE, 8FB8B5, 1A5058, 4FD6C8, 9FB2FF, 8CD98C, F2D27A, F27A7A, 6BC6F2"),
    ("Pure Black", "000000, 000000, 000000, 0A0A0A, E6E6E6, 8C8C8C, 262626, 0A84FF, BF5AF2, 30D158, FFD60A, FF453A, 64D2FF"),
    ("Slate Dusk", "3B4A6B, 34425F, 2E3A54, 2A3550, EEF1F8, B4BDD3, 55688F, FFD27A, F2A1C6, A7E3A0, FFD27A, FF8A8A, 9FD8FF"),
    ("Sage", "4A5A4C, 425144, 3B483D, 374339, EEF2EA, B9C4B4, 66796A, F2C57C, E0A6D0, B6E3A8, F2C57C, F28B82, 9AD3D6"),
    ("Clay", "6B4A3C, 604235, 553A2F, 4F362B, FBF0EA, DCC3B6, 87604E, FFD08A, F7B3D2, C2E3A2, FFD08A, FF9B8A, A9DDE6"),
    ("Parchment", "EAD9B5, E0CDA6, F2E6CC, D6C299, 3B2F1E, 7A6A50, D4BF92, A0522D, 7B3FA0, 4F7A28, A86F00, B22222, 2A7A8C"),
    ("Mint", "DDF2E6, D0EADB, ECF8F1, C3E2D0, 1F3A2C, 5E7B6B, BFE0CC, 0F8F5F, 7A4FCF, 2E8B3E, B07800, C8323C, 1A7FA0"),
    ("Blush", "F7E1E4, F0D4D8, FCEEF0, E9C9CE, 4A2329, 8A6066, E8BFC5, C2185B, 6A3FC0, 2E7D32, B26A00, C62828, 00838F"),
    ("Sky", "DCEBFA, CFE2F6, ECF4FD, C2D8F0, 1B2E45, 5B708A, BCD3EE, 1565C0, 8E44AD, 2E7D32, B26A00, C62828, 00838F"),
    ("Lavender", "E9E2F7, DED5F2, F4F0FB, D3C8EC, 2E2445, 6C6185, CFC2EC, 6A3FC0, C2185B, 2E7D32, B26A00, C62828, 00838F"),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Rgba {
        HexColor::parse(s).unwrap()
    }

    #[test]
    fn hex_parse_prefixes_and_case() {
        assert_eq!(HexColor::parse("1A2B3C").unwrap(), hex("1a2b3c"));
        assert_eq!(HexColor::parse("#1A2B3C").unwrap(), hex("0x1A2B3C"));
        assert_eq!(hex("1A2B3C").r, 0x1A as f64 / 255.0);
        assert_eq!(hex("1A2B3C").g, 0x2B as f64 / 255.0);
        assert_eq!(hex("1A2B3C").b, 0x3C as f64 / 255.0);
        assert_eq!(hex("1A2B3C").a, 1.0);
    }

    #[test]
    fn hex_format_round_trips() {
        for s in [
            "000000", "FFFFFF", "1A2B3C", "FF0000", "00FF00", "0000FF",
        ] {
            assert_eq!(HexColor::format(&hex(s)), s);
        }
        // 8-digit is AARRGGBB.
        assert_eq!(HexColor::format(&hex("801A2B3C")), "801A2B3C");
        assert_eq!(HexColor::format(&hex("FF1A2B3C")), "1A2B3C");
        // lowercase input formats uppercase.
        assert_eq!(HexColor::format(&hex("abcdef")), "ABCDEF");
    }

    #[test]
    fn hex_parse_rejects_bad_input() {
        assert!(HexColor::parse("").is_none());
        assert!(HexColor::parse("12345").is_none());
        assert!(HexColor::parse("1234567").is_none());
        assert!(HexColor::parse("1A2B3C4D5E").is_none());
        assert!(HexColor::parse("GGGGGG").is_none());
        assert!(HexColor::parse("#12345").is_none());
    }

    #[test]
    fn hex_alpha_clamped_to_min() {
        let c = hex("001A2B3C");
        assert_eq!(c.a, 0.08);
        assert_eq!(HexColor::format(&c), "141A2B3C");
    }

    #[test]
    fn hex_format_rgb_drops_alpha() {
        assert_eq!(HexColor::format_rgb(&hex("801A2B3C")), "1A2B3C");
    }

    #[test]
    fn palette_string_matches() {
        let s = PopupPalette::default()
            .all()
            .map(|c| HexColor::format_rgb(&c))
            .join(", ");
        assert_eq!(s, "8AADF4, A6DA95, EED49F, ED8796, 91D7E3");
    }

    #[test]
    fn popup_colors_defaults() {
        let c = PopupColors::default();
        assert_eq!(HexColor::format_rgb(&c.background), "24273A");
        assert_eq!(HexColor::format_rgb(&c.border), "9FC8E8");
        assert_eq!(HexColor::format_rgb(&c.text), "CAD3F5");
        assert_eq!(HexColor::format_rgb(&c.dim), "939AB7");
        assert_eq!(HexColor::format_rgb(&c.highlight), "3F4A5A");
        assert_eq!(HexColor::format_rgb(&c.accent), "556882");
        assert!(!c.is_light());
    }

    #[test]
    fn theme_globals_defaults() {
        let g = ThemeGlobals::default();
        assert_eq!(HexColor::format_rgb(&g.bar), "24273A");
        assert_eq!(HexColor::format_rgb(&g.group_bg), "3F4A5A");
        assert_eq!(HexColor::format_rgb(&g.text), "CAD3F5");
        assert_eq!(HexColor::format_rgb(&g.dim), "939AB7");
        assert_eq!(HexColor::format_rgb(&g.border), "C6A0F6");
        assert_eq!(HexColor::format_rgb(&g.accent), "556882");
        assert_eq!(HexColor::format_rgb(&g.header_color()), "454F5C");

        // No explicit border -> window border becomes the accent outline.
        let c = g.window_colors();
        assert_eq!(c.border, c.outline());

        let mut map = HashMap::new();
        map.insert("border".to_string(), hex("123456"));
        let g2 = ThemeGlobals::from_overrides(&map);
        assert_eq!(HexColor::format_rgb(&g2.window_colors().border), "123456");
    }

    #[test]
    fn derived_tokens_dark() {
        let c = PopupColors::default();
        assert!(!c.is_light());
        assert_eq!(c.base(), c.background.opaque());
        // dark: deeper blends toward black.
        assert_eq!(c.mantle(), c.base().blended(0.22, Rgba::BLACK));
        assert_eq!(c.crust(), c.base().blended(0.42, Rgba::BLACK));
        assert_eq!(c.surface0(), c.base().blended(0.09, c.text.opaque()));
        assert_eq!(c.surface1(), c.base().blended(0.16, c.text.opaque()));
        assert_eq!(c.hairline().a, 0.08);
        assert_eq!(c.outline().a, 0.85);
        assert_eq!(c.outline().r, c.accent_on().blended(0.45, c.base()).r);
    }

    #[test]
    fn derived_tokens_light() {
        let c = PopupColors {
            background: hex("EFF1F5"),
            text: hex("4C4F69"),
            ..PopupColors::default()
        };
        assert!(c.is_light());
        // light: deeper blends toward the opaque text color by f*0.35.
        assert_eq!(c.mantle(), c.base().blended(0.22 * 0.35, c.text.opaque()));
        assert_eq!(c.crust(), c.base().blended(0.42 * 0.35, c.text.opaque()));
        assert_eq!(c.hairline().a, 0.12);
    }

    #[test]
    fn tone_matches_swift() {
        let c = PopupColors::default();
        assert_eq!(c.tone(PopupTone::Text), c.text);
        assert_eq!(c.tone(PopupTone::Dim), c.dim);
        assert_eq!(c.tone(PopupTone::Accent), c.accent_on());
        assert_eq!(c.tone(PopupTone::Accent2), c.readable(c.palette.accent2, 3.0));
        assert_eq!(
            c.tone(PopupTone::Success),
            c.readable(c.palette.success, 3.0)
        );
        assert_eq!(
            c.tone(PopupTone::Warning),
            c.readable(c.palette.warning, 3.0)
        );
        assert_eq!(c.tone(PopupTone::Danger), c.readable(c.palette.danger, 3.0));
        assert_eq!(c.tone(PopupTone::Info), c.readable(c.palette.info, 3.0));
    }

    #[test]
    fn on_accent_is_white_or_black_or_crust() {
        let c = PopupColors::default();
        let oa = c.on_accent();
        assert!(oa == c.crust() || oa == Rgba::WHITE || oa == Rgba::BLACK);
    }

    #[test]
    fn header_style_labels_and_raw() {
        let expected = [
            ("quiet", "Quiet"),
            ("flat", "Flat"),
            ("edge", "Accent Edge"),
            ("stripe", "Accent Stripe"),
            ("tinted", "Tinted"),
            ("glow", "Glow"),
            ("aurora", "Aurora"),
        ];
        assert_eq!(HeaderStyle::ALL.len(), 7);
        for (i, (raw, label)) in expected.iter().enumerate() {
            assert_eq!(HeaderStyle::ALL[i].raw_value(), *raw);
            assert_eq!(HeaderStyle::ALL[i].label(), *label);
            assert_eq!(HeaderStyle::from_raw(raw), Some(HeaderStyle::ALL[i]));
        }
        assert_eq!(HeaderStyle::default(), HeaderStyle::Flat);
        assert_eq!(current_header_style(), HeaderStyle::Flat);
        set_current_header_style(HeaderStyle::Aurora);
        assert_eq!(current_header_style(), HeaderStyle::Aurora);
        set_current_header_style(HeaderStyle::Flat);
    }

    #[test]
    fn presets_count_and_nightfox_values() {
        let all = ThemePreset::built_in();
        assert_eq!(all.len(), 44);

        let nf = all.iter().find(|p| p.name == "Nightfox").unwrap();
        assert_eq!(HexColor::format_rgb(&nf.background), "192330");
        assert_eq!(HexColor::format_rgb(&nf.browser), "131A24");
        assert_eq!(HexColor::format_rgb(&nf.terminal), "111720");
        assert_eq!(HexColor::format_rgb(&nf.header), "0F141C");
        assert_eq!(HexColor::format_rgb(&nf.text), "CDCECF");
        assert_eq!(HexColor::format_rgb(&nf.dim), "AEAFB0");
        assert_eq!(HexColor::format_rgb(&nf.highlight), "2B3B51");
        assert_eq!(HexColor::format_rgb(&nf.accent), "719CD6");
        assert_eq!(HexColor::format_rgb(&nf.palette.accent2), "9D79D6");
        assert_eq!(HexColor::format_rgb(&nf.palette.success), "81B29A");
        assert_eq!(HexColor::format_rgb(&nf.palette.warning), "DBC074");
        assert_eq!(HexColor::format_rgb(&nf.palette.danger), "C94F6D");
        assert_eq!(HexColor::format_rgb(&nf.palette.info), "63CDCF");
        assert_eq!(nf.tone(), Tone::Dark);
    }

    #[test]
    fn presets_grouped_and_ordered() {
        let all = ThemePreset::built_in();
        let groups = grouped_by_tone(&all);
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].0, Tone::Mid);
        assert_eq!(groups[0].1, "Mid Tones");
        assert_eq!(groups[1].0, Tone::Dark);
        assert_eq!(groups[1].1, "Dark");
        assert_eq!(groups[2].0, Tone::Light);
        assert_eq!(groups[2].1, "Light");
        assert!(groups.iter().all(|g| !g.2.is_empty()));

        let ordered = ordered_by_tone(&all);
        assert_eq!(ordered.len(), all.len());
        for w in ordered.windows(2) {
            let (a, b) = (&w[0], &w[1]);
            assert!(a.tone() <= b.tone());
            if a.tone() == b.tone() {
                assert!(a.background.relative_luminance() >= b.background.relative_luminance());
            }
        }
        // flat concatenation of groups equals the ordered list
        let flat: Vec<ThemePreset> = groups.into_iter().flat_map(|g| g.2).collect();
        assert_eq!(flat, ordered);

        let light = ordered.iter().find(|p| p.name == "Catppuccin Latte").unwrap();
        assert_eq!(light.tone(), Tone::Light);
    }

    #[test]
    fn preset_parse_7_8_13() {
        // 7 colors: accent defaults to text.
        let p = ThemePreset::parse("t7", "111111,222222,333333,444444,555555,666666,777777").unwrap();
        assert_eq!(HexColor::format_rgb(&p.accent), "555555");
        // 8 colors: explicit accent.
        let p = ThemePreset::parse(
            "t8",
            "111111,222222,333333,444444,555555,666666,777777,888888",
        )
        .unwrap();
        assert_eq!(HexColor::format_rgb(&p.accent), "888888");
        // light 7-color: default palette is the Latte palette.
        let p = ThemePreset::parse("tl", "FFFFFF,EEEEEE,DDDDDD,CCCCCC,000000,111111,222222").unwrap();
        assert_eq!(HexColor::format_rgb(&p.palette.accent2), "1E66F5");
        // bad counts / bad hex.
        assert!(ThemePreset::parse("x", "111111,222222").is_none());
        assert!(ThemePreset::parse("x", "111111,222222,333333,444444,555555,666666,777777,888888,999999,AAAAAA,BBBBBB,CCCCCC,DDDDDD,EEEEEE").is_none());
        assert!(ThemePreset::parse("x", "GGGGGG,222222,333333,444444,555555,666666,777777").is_none());
        assert!(ThemePreset::parse("", "111111,222222,333333,444444,555555,666666,777777").is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn nscolor_conversion() {
        let c = Rgba::from_u8(0x80, 0x40, 0x20, 0xFF);
        let ns = c.to_nscolor();
        assert!((ns.redComponent() - 0x80 as f64 / 255.0).abs() < 1e-6);
        assert!((ns.greenComponent() - 0x40 as f64 / 255.0).abs() < 1e-6);
        assert!((ns.blueComponent() - 0x20 as f64 / 255.0).abs() < 1e-6);
    }
}
