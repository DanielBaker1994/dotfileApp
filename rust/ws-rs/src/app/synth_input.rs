//! `do:input:*` — real keyboard / mouse events posted into the app's own
//! event queue, for test agents.
//!
//! `do:key:<spec>` only records the spec, and the other `do:` verbs call the
//! model directly, so neither exercises the event path (local key monitor
//! `route_key_event`, responder chain, terminal views, nvim). These verbs build
//! genuine `NSEvent`s and `NSApp.postEvent` them, so they are dequeued and
//! dispatched exactly like typing — with no global HID input (nothing reaches
//! other apps, no modifier can stick system-wide, clicks carry a real
//! `clickCount`).
//!
//! Grammar (socket line `do:input:…`):
//! - `key:<combo> [<combo>…]` — space-separated combos, e.g. `key:cmd+n`,
//!   `key:ctrl+b w`, `key:esc esc`. Names follow `bin/ks-input` (US layout).
//! - `text:<text>` — literal text, one key event per character.
//! - `click:<x>,<y>[,<count>][,right]` — points from the top-left of the key
//!   window's content view; `count` 2 is a real double-click.
//! - `activate` — activate the app; if no window is key, make the shared window key.
//!
//! Events go to the app's key window, so the app must be active (the verbs
//! never activate it on their own: `activate` is explicit). Delivery happens on
//! the main thread after the reply; `state.input` reports what was posted or
//! dropped and why. Limits: the class-level `NSEvent.modifierFlags` /
//! `mouseLocation` still read the real hardware, so code that polls them (not
//! the event) sees no modifier / the real cursor.

use std::sync::Mutex;

use serde_json::{json, Value};

/// One key press (down + up).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyStroke {
    pub code: u16,
    pub cmd: bool,
    pub ctrl: bool,
    pub opt: bool,
    pub shift: bool,
    /// `NSEvent.characters`.
    pub chars: String,
    /// `NSEvent.charactersIgnoringModifiers` (keeps Shift, like AppKit).
    pub chars_ignoring: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum InputCmd {
    Keys(Vec<KeyStroke>),
    /// Text with characters that have no US-layout key (é, 日, emoji): macOS
    /// delivers those as one text insertion (emoji picker, input methods), not
    /// as key presses, so the whole string goes to the first responder's
    /// `insertText:` in one piece (keeps the order).
    Insert(String),
    Click { x: f64, y: f64, count: u8, right: bool },
    Activate,
}

/// Key code used for text characters that have no key on the US layout
/// (é, 日, ✓): unassigned on ANSI keyboards, so no key handler matches it and
/// only `characters` matters — like the unicode events `ks-input type` sends.
// 0x34 is not safe: AppKit maps it to Enter for some characters (emoji were
// typed as newlines, night run 2026-10-20). 0x44 is unassigned in Events.h.
const UNMAPPED_CODE: u16 = 0x44;

/// (name, kVK code, unshifted char, shifted char).
const KEYS: &[(&str, u16, char, char)] = &[
    ("a", 0, 'a', 'A'), ("s", 1, 's', 'S'), ("d", 2, 'd', 'D'), ("f", 3, 'f', 'F'),
    ("h", 4, 'h', 'H'), ("g", 5, 'g', 'G'), ("z", 6, 'z', 'Z'), ("x", 7, 'x', 'X'),
    ("c", 8, 'c', 'C'), ("v", 9, 'v', 'V'), ("b", 11, 'b', 'B'), ("q", 12, 'q', 'Q'),
    ("w", 13, 'w', 'W'), ("e", 14, 'e', 'E'), ("r", 15, 'r', 'R'), ("y", 16, 'y', 'Y'),
    ("t", 17, 't', 'T'), ("1", 18, '1', '!'), ("2", 19, '2', '@'), ("3", 20, '3', '#'),
    ("4", 21, '4', '$'), ("6", 22, '6', '^'), ("5", 23, '5', '%'), ("=", 24, '=', '+'),
    ("9", 25, '9', '('), ("7", 26, '7', '&'), ("-", 27, '-', '_'), ("8", 28, '8', '*'),
    ("0", 29, '0', ')'), ("]", 30, ']', '}'), ("o", 31, 'o', 'O'), ("u", 32, 'u', 'U'),
    ("[", 33, '[', '{'), ("i", 34, 'i', 'I'), ("p", 35, 'p', 'P'), ("l", 37, 'l', 'L'),
    ("j", 38, 'j', 'J'), ("'", 39, '\'', '"'), ("k", 40, 'k', 'K'), (";", 41, ';', ':'),
    ("\\", 42, '\\', '|'), (",", 43, ',', '<'), ("/", 44, '/', '?'), ("n", 45, 'n', 'N'),
    ("m", 46, 'm', 'M'), (".", 47, '.', '>'), ("`", 50, '`', '~'),
];

/// (names, kVK code, characters) for keys without a printable glyph.
const SPECIAL: &[(&[&str], u16, char)] = &[
    (&["return", "enter"], 36, '\r'),
    (&["tab"], 48, '\t'),
    (&["space"], 49, ' '),
    (&["delete", "backspace"], 51, '\u{7f}'),
    (&["esc", "escape"], 53, '\u{1b}'),
    (&["forwarddelete"], 117, '\u{f728}'),
    (&["left"], 123, '\u{f702}'),
    (&["right"], 124, '\u{f703}'),
    (&["down"], 125, '\u{f701}'),
    (&["up"], 126, '\u{f700}'),
    (&["home"], 115, '\u{f729}'),
    (&["end"], 119, '\u{f72b}'),
    (&["pageup"], 116, '\u{f72c}'),
    (&["pagedown"], 121, '\u{f72d}'),
    (&["f1"], 122, '\u{f704}'),
    (&["f2"], 120, '\u{f705}'),
    (&["f3"], 99, '\u{f706}'),
    (&["f4"], 118, '\u{f707}'),
    (&["f5"], 96, '\u{f708}'),
    (&["f6"], 97, '\u{f709}'),
    (&["f7"], 98, '\u{f70a}'),
    (&["f8"], 100, '\u{f70b}'),
    (&["f9"], 101, '\u{f70c}'),
    (&["f10"], 109, '\u{f70d}'),
    (&["f11"], 103, '\u{f70e}'),
    (&["f12"], 111, '\u{f70f}'),
];

/// The characters a Ctrl chord produces (Ctrl+A..Z = 0x01..0x1A, Ctrl+[ = Esc …).
fn ctrl_char(c: char) -> Option<char> {
    match c {
        'a'..='z' => char::from_u32(c as u32 - 'a' as u32 + 1),
        '[' => Some('\u{1b}'),
        '\\' => Some('\u{1c}'),
        ']' => Some('\u{1d}'),
        _ => None,
    }
}

/// One combo: `cmd+shift+n`, `ctrl+b`, `esc`, `f5`.
pub fn parse_combo(combo: &str) -> Result<KeyStroke, String> {
    let lower = combo.to_lowercase();
    if lower.is_empty() {
        return Err("empty key combo".into());
    }
    let (mut cmd, mut ctrl, mut opt, mut shift) = (false, false, false, false);
    let mut key: Option<(u16, char, char)> = None;
    for part in lower.split('+') {
        match part {
            "cmd" | "command" | "meta" => cmd = true,
            "ctrl" | "control" => ctrl = true,
            "alt" | "opt" | "option" => opt = true,
            "shift" => shift = true,
            "" => return Err(format!("empty part in combo '{combo}'")),
            name => {
                if key.is_some() {
                    return Err(format!("two keys in combo '{combo}'"));
                }
                if let Some(&(_, code, lo, hi)) = KEYS.iter().find(|k| k.0 == name) {
                    key = Some((code, lo, hi));
                } else if let Some(&(_, code, ch)) =
                    SPECIAL.iter().find(|s| s.0.contains(&name))
                {
                    key = Some((code, ch, ch));
                } else {
                    return Err(format!("unknown key '{name}' in combo '{combo}'"));
                }
            }
        }
    }
    let Some((code, lo, hi)) = key else {
        return Err(format!("no key in combo '{combo}'"));
    };
    let ignoring = if shift { hi } else { lo };
    let chars = if ctrl { ctrl_char(lo).unwrap_or(ignoring) } else { ignoring };
    Ok(KeyStroke {
        code,
        cmd,
        ctrl,
        opt,
        shift,
        chars: chars.to_string(),
        chars_ignoring: ignoring.to_string(),
    })
}

/// Space-separated combos: `ctrl+b w`, `esc esc`.
pub fn parse_sequence(spec: &str) -> Result<Vec<KeyStroke>, String> {
    let strokes: Result<Vec<_>, _> = spec.split_whitespace().map(parse_combo).collect();
    let strokes = strokes?;
    if strokes.is_empty() {
        return Err("input:key:<combo> [<combo>…]".into());
    }
    Ok(strokes)
}

/// Literal text: printable US-layout characters use their real key (with
/// Shift where needed), `\n` is Return, `\t` Tab; anything else is a
/// characters-only event on [`UNMAPPED_CODE`].
pub fn text_strokes(text: &str) -> Vec<KeyStroke> {
    text.chars()
        .map(|c| {
            let s = c.to_string();
            let plain = |code, shift| KeyStroke {
                code,
                cmd: false,
                ctrl: false,
                opt: false,
                shift,
                chars: s.clone(),
                chars_ignoring: s.clone(),
            };
            match c {
                '\n' | '\r' => plain(36, false).with_chars('\r'),
                '\t' => plain(48, false),
                ' ' => plain(49, false),
                _ => {
                    if let Some(k) = KEYS.iter().find(|k| k.2 == c) {
                        plain(k.1, false)
                    } else if let Some(k) = KEYS.iter().find(|k| k.3 == c) {
                        plain(k.1, true)
                    } else {
                        plain(UNMAPPED_CODE, false)
                    }
                }
            }
        })
        .collect()
}

impl KeyStroke {
    fn with_chars(mut self, c: char) -> Self {
        self.chars = c.to_string();
        self.chars_ignoring = c.to_string();
        self
    }
}

/// `do:input:<rest>` → a command for the main thread, or the error reply text.
pub fn parse(rest: &str) -> Result<InputCmd, String> {
    if rest == "activate" {
        return Ok(InputCmd::Activate);
    }
    if let Some(spec) = rest.strip_prefix("key:") {
        return parse_sequence(spec).map(InputCmd::Keys);
    }
    if let Some(text) = rest.strip_prefix("text:") {
        if text.is_empty() {
            return Err("input:text:<text>".into());
        }
        let strokes = text_strokes(text);
        if strokes.iter().any(|k| k.code == UNMAPPED_CODE) {
            return Ok(InputCmd::Insert(text.to_string()));
        }
        return Ok(InputCmd::Keys(strokes));
    }
    if let Some(args) = rest.strip_prefix("click:") {
        let usage = || "input:click:<x>,<y>[,<count>][,right]".to_string();
        let parts: Vec<&str> = args.split(',').map(str::trim).collect();
        if parts.len() < 2 || parts.len() > 4 {
            return Err(usage());
        }
        let x: f64 = parts[0].parse().map_err(|_| usage())?;
        let y: f64 = parts[1].parse().map_err(|_| usage())?;
        if !x.is_finite() || !y.is_finite() || x < 0.0 || y < 0.0 {
            return Err(usage());
        }
        let mut count = 1u8;
        let mut right = false;
        for p in &parts[2..] {
            if *p == "right" {
                right = true;
            } else {
                count = p.parse().ok().filter(|n| (1..=3).contains(n)).ok_or_else(usage)?;
            }
        }
        return Ok(InputCmd::Click { x, y, count, right });
    }
    Err("input:<key:|text:|click:|activate>".into())
}

/// Delivery counters, written on the main thread, read by `state.input`.
#[derive(Default)]
struct Stats {
    posted: u64,
    dropped: u64,
    last_error: String,
}

static STATS: Mutex<Stats> = Mutex::new(Stats { posted: 0, dropped: 0, last_error: String::new() });

pub fn state() -> Value {
    let s = STATS.lock().unwrap();
    json!({ "posted": s.posted, "dropped": s.dropped, "lastError": s.last_error })
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn record(posted: u64, dropped: u64, err: Option<String>) {
    let mut s = STATS.lock().unwrap();
    s.posted += posted;
    s.dropped += dropped;
    if let Some(e) = err {
        s.last_error = e;
    } else if posted > 0 {
        s.last_error.clear();
    }
}

#[cfg(target_os = "macos")]
pub mod appkit {
    use super::{record, InputCmd, KeyStroke};
    use objc2::rc::Retained;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSEvent, NSEventModifierFlags as F, NSEventType, NSWindow};
    use objc2_foundation::{NSPoint, NSProcessInfo, NSString};

    fn now() -> f64 {
        NSProcessInfo::processInfo().systemUptime()
    }

    fn flags(k: &KeyStroke) -> F {
        let mut f = F::empty();
        if k.cmd {
            f |= F::Command;
        }
        if k.ctrl {
            f |= F::Control;
        }
        if k.opt {
            f |= F::Option;
        }
        if k.shift {
            f |= F::Shift;
        }
        if k.chars.chars().next().is_some_and(|c| ('\u{f700}'..='\u{f8ff}').contains(&c)) {
            f |= F::Function;
        }
        f
    }

    fn key_event(ty: NSEventType, k: &KeyStroke, win: isize) -> Option<Retained<NSEvent>> {
        let chars = NSString::from_str(&k.chars);
        let ign = NSString::from_str(&k.chars_ignoring);
        NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
            ty, NSPoint::new(0.0, 0.0), flags(k), now(), win, None, &chars, &ign, false, k.code,
        )
    }

    fn mouse_event(ty: NSEventType, at: NSPoint, count: u8, win: isize) -> Option<Retained<NSEvent>> {
        NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
            ty, at, F::empty(), now(), win, None, 0, count as isize, 1.0,
        )
    }

    /// Run one command on the main thread. `shared` is the app's shared host
    /// window (made key by `activate`).
    pub fn deliver(mtm: MainThreadMarker, cmd: &InputCmd, shared: Option<&NSWindow>) {
        let app = NSApplication::sharedApplication(mtm);
        if let InputCmd::Activate = cmd {
            #[allow(deprecated)]
            app.activateIgnoringOtherApps(true);
            // Keep whichever window is key (compare, a panel …); fall back to the
            // shared window only when none is.
            if app.keyWindow().is_none() {
                if let Some(w) = shared {
                    w.makeKeyAndOrderFront(None);
                }
            }
            record(0, 0, None);
            return;
        }
        let Some(win) = app.keyWindow() else {
            let n = match cmd {
                InputCmd::Keys(k) => k.len() as u64,
                _ => 1,
            };
            record(0, n, Some("no key window (app not active: send do:input:activate)".into()));
            return;
        };
        let num = win.windowNumber();
        match cmd {
            InputCmd::Keys(strokes) => {
                let mut posted = 0;
                for k in strokes {
                    for ty in [NSEventType::KeyDown, NSEventType::KeyUp] {
                        if let Some(ev) = key_event(ty, k, num) {
                            app.postEvent_atStart(&ev, false);
                        }
                    }
                    posted += 1;
                }
                record(posted, 0, None);
            }
            InputCmd::Insert(text) => {
                let Some(r) = win.firstResponder() else {
                    record(0, 1, Some("key window has no first responder".into()));
                    return;
                };
                let sel = objc2::sel!(insertText:);
                if !objc2_foundation::NSObjectProtocol::respondsToSelector(&*r, sel) {
                    record(0, 1, Some("first responder does not accept text".into()));
                    return;
                }
                let ns = NSString::from_str(text);
                let _: () = unsafe { objc2::msg_send![&*r, insertText: &*ns] };
                record(text.chars().count() as u64, 0, None);
            }
            InputCmd::Click { x, y, count, right } => {
                let Some(view) = win.contentView() else {
                    record(0, 1, Some("key window has no content view".into()));
                    return;
                };
                let h = view.frame().size.height;
                let local = NSPoint::new(*x, if view.isFlipped() { *y } else { h - *y });
                let at = view.convertPoint_toView(local, None);
                let (down, up) = if *right {
                    (NSEventType::RightMouseDown, NSEventType::RightMouseUp)
                } else {
                    (NSEventType::LeftMouseDown, NSEventType::LeftMouseUp)
                };
                // A double-click is press 1 then press 2, as the hardware sends it.
                for n in 1..=*count {
                    for ty in [down, up] {
                        if let Some(ev) = mouse_event(ty, at, n, num) {
                            app.postEvent_atStart(&ev, false);
                        }
                    }
                }
                record(1, 0, None);
            }
            InputCmd::Activate => unreachable!(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combos_map_to_us_key_codes_and_chars() {
        let k = parse_combo("cmd+n").unwrap();
        assert_eq!((k.code, k.cmd, k.chars.as_str()), (45, true, "n"));
        let k = parse_combo("ctrl+b").unwrap();
        assert_eq!((k.code, k.ctrl, k.chars.as_str(), k.chars_ignoring.as_str()), (11, true, "\u{2}", "b"));
        let k = parse_combo("shift+/").unwrap();
        assert_eq!((k.code, k.chars.as_str()), (44, "?"));
        let k = parse_combo("Esc").unwrap();
        assert_eq!((k.code, k.chars.as_str()), (53, "\u{1b}"));
        let k = parse_combo("down").unwrap();
        assert_eq!(k.code, 125);
        assert_eq!(parse_combo("cmd+=").unwrap().code, 24);
    }

    #[test]
    fn bad_combos_are_errors() {
        assert!(parse_combo("").is_err());
        assert!(parse_combo("cmd+").is_err());
        assert!(parse_combo("cmd").is_err());
        assert!(parse_combo("a+b").is_err());
        assert!(parse_combo("hyper+x").is_err());
    }

    #[test]
    fn sequences_split_on_spaces() {
        let s = parse_sequence("ctrl+b w").unwrap();
        assert_eq!(s.len(), 2);
        assert_eq!(s[1].chars, "w");
        assert!(parse_sequence("   ").is_err());
    }

    #[test]
    fn text_uses_real_keys_and_shift() {
        let t = text_strokes("Go!\né");
        assert_eq!((t[0].code, t[0].shift, t[0].chars.as_str()), (5, true, "G"));
        assert_eq!((t[1].code, t[1].shift), (31, false));
        assert_eq!((t[2].code, t[2].shift, t[2].chars.as_str()), (18, true, "!"));
        assert_eq!((t[3].code, t[3].chars.as_str()), (36, "\r"));
        assert_eq!((t[4].code, t[4].chars.as_str()), (UNMAPPED_CODE, "é"));
    }

    #[test]
    fn parse_verbs() {
        assert_eq!(parse("activate"), Ok(InputCmd::Activate));
        assert!(matches!(parse("key:cmd+k"), Ok(InputCmd::Keys(k)) if k.len() == 1));
        assert!(matches!(parse("text:hi"), Ok(InputCmd::Keys(k)) if k.len() == 2));
        assert_eq!(parse("text:tube 🧪 ok"), Ok(InputCmd::Insert("tube 🧪 ok".into())));
        assert_eq!(parse("text:café"), Ok(InputCmd::Insert("café".into())));
        assert_eq!(parse("click:10,20"), Ok(InputCmd::Click { x: 10.0, y: 20.0, count: 1, right: false }));
        assert_eq!(parse("click:10,20,2"), Ok(InputCmd::Click { x: 10.0, y: 20.0, count: 2, right: false }));
        assert_eq!(parse("click:1,2,right"), Ok(InputCmd::Click { x: 1.0, y: 2.0, count: 1, right: true }));
        for bad in ["", "key:", "text:", "click:", "click:1", "click:-1,2", "click:1,2,9", "click:a,b", "wiggle"] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }
}
