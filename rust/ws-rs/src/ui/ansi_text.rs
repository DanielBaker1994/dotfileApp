//! ANSI SGR text → styled runs (the output windows' colored text, Swift
//! `AnsiText`): 30–37 / 90–97 foregrounds from the theme's ANSI palette,
//! 1 bold, 2 dim, 0 / 39 / 22 resets; every other escape is dropped.

/// One styled run: (text, palette index or None for the default, bold, dim).
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub text: String,
    pub fg: Option<usize>,
    pub bold: bool,
    pub dim: bool,
}

/// Split `s` into runs.
pub fn parse(s: &str) -> Vec<Run> {
    let mut out: Vec<Run> = Vec::new();
    let (mut fg, mut bold, mut dim) = (None::<usize>, false, false);
    let mut cur = String::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    let flush = |cur: &mut String, out: &mut Vec<Run>, fg, bold, dim| {
        if !cur.is_empty() {
            out.push(Run { text: std::mem::take(cur), fg, bold, dim });
        }
    };
    while i < chars.len() {
        let c = chars[i];
        if c == '\u{1b}' {
            if i + 1 < chars.len() && chars[i + 1] == '[' {
                let mut j = i + 2;
                while j < chars.len() && !(chars[j].is_ascii_alphabetic()) {
                    j += 1;
                }
                if j < chars.len() && chars[j] == 'm' {
                    flush(&mut cur, &mut out, fg, bold, dim);
                    let params: String = chars[i + 2..j].iter().collect();
                    let codes: Vec<u32> = if params.is_empty() {
                        vec![0]
                    } else {
                        params.split(';').filter_map(|p| p.parse().ok()).collect()
                    };
                    for code in codes {
                        match code {
                            0 => {
                                fg = None;
                                bold = false;
                                dim = false;
                            }
                            1 => bold = true,
                            2 => dim = true,
                            22 => {
                                bold = false;
                                dim = false;
                            }
                            30..=37 => fg = Some((code - 30) as usize),
                            90..=97 => fg = Some((code - 90 + 8) as usize),
                            39 => fg = None,
                            _ => {}
                        }
                    }
                }
                i = j + 1;
                continue;
            }
            i += 1;
            continue;
        }
        if c == '\r' {
            i += 1;
            continue;
        }
        cur.push(c);
        i += 1;
    }
    flush(&mut cur, &mut out, fg, bold, dim);
    out
}

#[cfg(target_os = "macos")]
pub use appkit::attributed;

#[cfg(target_os = "macos")]
mod appkit {
    use super::parse;
    use crate::ui::theme::{ansi_palette, PopupColors};
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::AnyThread;
    use objc2_app_kit::{NSFont, NSFontAttributeName, NSFontManager, NSFontTraitMask, NSForegroundColorAttributeName};
    use objc2_foundation::{NSAttributedString, NSAttributedStringKey, NSDictionary, NSMutableAttributedString, NSString};

    /// Build the attributed text for `s` in `font` with the theme's palette.
    pub fn attributed(s: &str, font: &NSFont, c: &PopupColors) -> Retained<NSMutableAttributedString> {
        let pal = ansi_palette(c);
        let mtm = objc2::MainThreadMarker::new().expect("main thread");
        let bold_font = NSFontManager::sharedFontManager(mtm).convertFont_toHaveTrait(font, NSFontTraitMask::BoldFontMask);
        let out = NSMutableAttributedString::new();
        for run in parse(s) {
            let mut color = run.fg.and_then(|i| pal.get(i).copied()).unwrap_or(c.text);
            if run.dim {
                color = color.with_alpha(0.6);
            }
            let f: &NSFont = if run.bold { &bold_font } else { font };
            let col = color.to_nscolor();
            let keys: [&NSAttributedStringKey; 2] = unsafe { [NSFontAttributeName, NSForegroundColorAttributeName] };
            let objs: [&AnyObject; 2] = unsafe {
                [
                    &*(f as *const NSFont as *const AnyObject),
                    &*(&*col as *const objc2_app_kit::NSColor as *const AnyObject),
                ]
            };
            let attrs = NSDictionary::from_slices(&keys, &objs);
            let piece = unsafe {
                NSAttributedString::initWithString_attributes(NSAttributedString::alloc(), &NSString::from_str(&run.text), Some(&attrs))
            };
            out.appendAttributedString(&piece);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_and_resets() {
        let r = parse("\u{1b}[1;36m== deps ==\u{1b}[0m\n  \u{1b}[32mPASS\u{1b}[0m curl");
        assert_eq!(r[0], Run { text: "== deps ==".into(), fg: Some(6), bold: true, dim: false });
        assert_eq!(r[1].text, "\n  ");
        assert_eq!(r[1].fg, None);
        assert_eq!(r[2], Run { text: "PASS".into(), fg: Some(2), bold: false, dim: false });
        assert_eq!(r[3].text, " curl");
    }

    #[test]
    fn strips_other_escapes() {
        let r = parse("a\u{1b}[2Kb\rc");
        assert_eq!(r.iter().map(|x| x.text.clone()).collect::<String>(), "abc");
    }
}
