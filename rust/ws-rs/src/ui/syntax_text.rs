//! Lightweight syntax highlighting for the prettyprint editor: JSON and XML
//! tokenizers (pure, unit-tested) plus the AppKit attributed-string builder
//! that maps token classes onto theme colors — the syntax counterpart of
//! `ui/ansi_text.rs`'s ANSI runs.
//!
//! The panel's own `prettyFormat` already decides the language (first non-space
//! char `{`/`[` = JSON, `<` = XML); [`detect`] mirrors that pick and anything
//! else stays plain.

use crate::ui::theme::Rgba;

/// The languages the prettyprint panel colors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    Json,
    Xml,
}

/// A token class; [`token_color`] maps each onto a theme color.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Token {
    /// A JSON object key.
    Key,
    /// A JSON string value / XML attribute value.
    Str,
    /// A number literal.
    Number,
    /// `true` / `false` / `null`.
    Literal,
    /// Structural punctuation (`{}[],:` and `<`, `>`, `=`, `/`).
    Punct,
    /// An XML element name.
    Tag,
    /// An XML attribute name.
    Attr,
    /// An XML comment / declaration / CDATA section.
    Comment,
}

/// One styled span as a **byte** range into the source (converted to UTF-16
/// offsets by the AppKit builder).
#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub token: Token,
}

/// Pick the language from the first non-space character: `{`/`[` = JSON,
/// `<` = XML, anything else = none (leave the text plain). Mirrors
/// `ToolPanel::format_text`'s dispatch.
pub fn detect(text: &str) -> Option<Lang> {
    match text.trim_start().chars().next() {
        Some('{') | Some('[') => Some(Lang::Json),
        Some('<') => Some(Lang::Xml),
        _ => None,
    }
}

/// Tokenize `text` as `lang`. Spans are ordered, non-overlapping and within
/// byte bounds; whitespace and unrecognized bytes carry no span (the builder's
/// base color shows through).
pub fn tokenize(text: &str, lang: Lang) -> Vec<Span> {
    match lang {
        Lang::Json => json(text),
        Lang::Xml => xml(text),
    }
}

fn json(text: &str) -> Vec<Span> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < b.len() {
        match b[i] {
            b'"' => {
                let start = i;
                i += 1;
                while i < b.len() {
                    match b[i] {
                        b'\\' => i += 2,
                        b'"' => {
                            i += 1;
                            break;
                        }
                        _ => i += 1,
                    }
                }
                i = i.min(b.len());
                // A string is a key when the next non-space byte is `:`.
                let mut j = i;
                while j < b.len() && matches!(b[j], b' ' | b'\t' | b'\n' | b'\r') {
                    j += 1;
                }
                let token = if j < b.len() && b[j] == b':' {
                    Token::Key
                } else {
                    Token::Str
                };
                out.push(Span { start, end: i, token });
            }
            b'-' | b'0'..=b'9' => {
                let start = i;
                i += 1;
                while i < b.len() && matches!(b[i], b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-') {
                    i += 1;
                }
                out.push(Span { start, end: i, token: Token::Number });
            }
            b't' | b'f' | b'n' => {
                let start = i;
                while i < b.len() && b[i].is_ascii_alphabetic() {
                    i += 1;
                }
                out.push(Span { start, end: i, token: Token::Literal });
            }
            b'{' | b'}' | b'[' | b']' | b',' | b':' => {
                out.push(Span { start: i, end: i + 1, token: Token::Punct });
                i += 1;
            }
            _ => i += 1,
        }
    }
    out
}

fn is_xml_ws(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r')
}

fn xml(text: &str) -> Vec<Span> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    let push = |out: &mut Vec<Span>, start: usize, end: usize, token: Token| {
        if end > start {
            out.push(Span { start, end, token });
        }
    };
    while i < b.len() {
        if b[i] != b'<' {
            i += 1;
            continue;
        }
        // Comment / CDATA / processing instruction / doctype: one Comment span.
        let closed = |from: usize, marker: &str, back: usize| -> Option<usize> {
            text[from..].find(marker).map(|p| from + p + back)
        };
        if text[i..].starts_with("<!--") {
            let end = closed(i + 4, "-->", 3).unwrap_or(b.len());
            push(&mut out, i, end, Token::Comment);
            i = end;
            continue;
        }
        if text[i..].starts_with("<![CDATA[") {
            let end = closed(i + 9, "]]>", 3).unwrap_or(b.len());
            push(&mut out, i, end, Token::Comment);
            i = end;
            continue;
        }
        if text[i..].starts_with("<?") {
            let end = closed(i + 2, "?>", 2).unwrap_or(b.len());
            push(&mut out, i, end, Token::Comment);
            i = end;
            continue;
        }
        if text[i..].starts_with("<!") {
            let end = text[i + 2..].find('>').map(|p| i + 2 + p + 1).unwrap_or(b.len());
            push(&mut out, i, end, Token::Comment);
            i = end;
            continue;
        }
        // An element: '<' [ '/' ] name ( attr [ '=' value ] )* [ '/' ] '>'.
        push(&mut out, i, i + 1, Token::Punct);
        i += 1;
        if i < b.len() && b[i] == b'/' {
            push(&mut out, i, i + 1, Token::Punct);
            i += 1;
        }
        let name_start = i;
        while i < b.len() && !is_xml_ws(b[i]) && b[i] != b'>' && b[i] != b'/' && b[i] != b'=' {
            i += 1;
        }
        push(&mut out, name_start, i, Token::Tag);
        loop {
            while i < b.len() && is_xml_ws(b[i]) {
                i += 1;
            }
            if i >= b.len() {
                break;
            }
            match b[i] {
                b'>' => {
                    push(&mut out, i, i + 1, Token::Punct);
                    i += 1;
                    break;
                }
                b'/' => {
                    push(&mut out, i, i + 1, Token::Punct);
                    i += 1;
                    if i < b.len() && b[i] == b'>' {
                        push(&mut out, i, i + 1, Token::Punct);
                        i += 1;
                    }
                    break;
                }
                _ => {}
            }
            let attr_start = i;
            while i < b.len() && !is_xml_ws(b[i]) && b[i] != b'=' && b[i] != b'>' && b[i] != b'/' {
                i += 1;
            }
            push(&mut out, attr_start, i, Token::Attr);
            while i < b.len() && is_xml_ws(b[i]) {
                i += 1;
            }
            if i < b.len() && b[i] == b'=' {
                push(&mut out, i, i + 1, Token::Punct);
                i += 1;
                while i < b.len() && is_xml_ws(b[i]) {
                    i += 1;
                }
                if i < b.len() && (b[i] == b'"' || b[i] == b'\'') {
                    let quote = b[i];
                    let val_start = i;
                    i += 1;
                    while i < b.len() && b[i] != quote {
                        i += 1;
                    }
                    if i < b.len() {
                        i += 1;
                    }
                    push(&mut out, val_start, i, Token::Str);
                }
            }
        }
    }
    out
}

/// The theme color for a token class.
pub fn token_color(c: &crate::ui::theme::PopupColors, t: Token) -> Rgba {
    match t {
        Token::Key | Token::Tag => c.palette.accent2,
        Token::Str => c.palette.success,
        Token::Number | Token::Attr => c.palette.warning,
        Token::Literal => c.palette.info,
        Token::Punct | Token::Comment => c.dim,
    }
}

/// [`tokenize`]'s spans converted to UTF-16 `(location, length, token)` — the
/// units `NSAttributedString` uses. Pure, so the multibyte conversion (JSON
/// string values with emoji, say) is unit-tested rather than only exercised
/// on-screen.
pub fn utf16_spans(text: &str, lang: Lang) -> Vec<(usize, usize, Token)> {
    let mut out = Vec::new();
    let mut byte = 0usize;
    let mut unit = 0usize;
    for span in tokenize(text, lang) {
        if span.start < byte || span.end > text.len() {
            continue;
        }
        unit += text[byte..span.start].encode_utf16().count();
        let len = text[span.start..span.end].encode_utf16().count();
        byte = span.end;
        if len > 0 {
            out.push((unit, len, span.token));
        }
        unit += len;
    }
    out
}

#[cfg(target_os = "macos")]
pub use appkit::attributed;

#[cfg(target_os = "macos")]
mod appkit {
    use super::{detect, token_color, utf16_spans};
    use crate::ui::style;
    use crate::ui::theme::PopupColors;
    use crate::ui::view_base::as_any;
    use objc2::rc::Retained;
    use objc2::AnyThread;
    use objc2_app_kit::{NSFont, NSForegroundColorAttributeName};
    use objc2_foundation::{NSMutableAttributedString, NSRange, NSString};

    /// The editor text as an attributed string: base font/color everywhere,
    /// then the theme color over each token span.
    pub fn attributed(
        text: &str,
        font: &NSFont,
        c: &PopupColors,
    ) -> Retained<NSMutableAttributedString> {
        let out = NSMutableAttributedString::initWithString(
            NSMutableAttributedString::alloc(),
            &NSString::from_str(text),
        );
        let base = style::attrs(font, c.text);
        let full = NSRange::new(0, out.length());
        unsafe { out.addAttributes_range(&base, full) };
        let Some(lang) = detect(text) else {
            return out;
        };
        for (loc, len, token) in utf16_spans(text, lang) {
            let color = token_color(c, token).to_nscolor();
            unsafe {
                out.addAttribute_value_range(
                    NSForegroundColorAttributeName,
                    as_any(&*color),
                    NSRange::new(loc, len),
                );
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str, lang: Lang) -> Vec<(String, Token)> {
        tokenize(text, lang)
            .into_iter()
            .map(|s| (text[s.start..s.end].to_string(), s.token))
            .collect()
    }

    #[test]
    fn detect_mirrors_format_text() {
        assert_eq!(detect("{\"a\":1}"), Some(Lang::Json));
        assert_eq!(detect("  \n[1,2]"), Some(Lang::Json));
        assert_eq!(detect("  <a/>"), Some(Lang::Xml));
        assert_eq!(detect("hello"), None);
        assert_eq!(detect("   "), None);
    }

    #[test]
    fn json_keys_values_and_literals() {
        let k = kinds("{\"name\": \"Ada\", \"n\": 42, \"ok\": true, \"x\": null}", Lang::Json);
        assert!(k.contains(&("\"name\"".into(), Token::Key)));
        assert!(k.contains(&("\"Ada\"".into(), Token::Str)));
        assert!(k.contains(&("42".into(), Token::Number)));
        assert!(k.contains(&("true".into(), Token::Literal)));
        assert!(k.contains(&("null".into(), Token::Literal)));
        assert!(k.contains(&("{".into(), Token::Punct)));
        assert!(k.contains(&(":".into(), Token::Punct)));
    }

    #[test]
    fn json_escaped_quote_does_not_end_the_string() {
        let k = kinds("{\"a\": \"x\\\"y\"}", Lang::Json);
        assert!(k.contains(&("\"x\\\"y\"".into(), Token::Str)), "{k:?}");
    }

    #[test]
    fn xml_tags_attrs_values_and_comments() {
        let k = kinds("<?xml version=\"1.0\"?>\n<note id='7'>hi</note><!-- c -->", Lang::Xml);
        assert!(k.iter().any(|(s, t)| s == "<" && *t == Token::Punct));
        assert!(k.contains(&("note".into(), Token::Tag)));
        assert!(k.contains(&("id".into(), Token::Attr)));
        assert!(k.contains(&("'7'".into(), Token::Str)));
        assert!(k.iter().any(|(s, t)| s.starts_with("<?") && *t == Token::Comment));
        assert!(k.contains(&("<!-- c -->".into(), Token::Comment)));
    }

    #[test]
    fn spans_are_ordered_and_in_bounds() {
        for (text, lang) in [("{\"a\": [1, 2.5e3]}", Lang::Json), ("<a b=\"c\"/>", Lang::Xml)] {
            let spans = tokenize(text, lang);
            let mut last = 0;
            for s in &spans {
                assert!(s.start >= last && s.end <= text.len() && s.end > s.start, "{s:?}");
                last = s.end;
            }
        }
    }

    #[test]
    fn token_colors_come_from_the_theme() {
        let c = crate::ui::theme::PopupColors::default();
        assert_eq!(token_color(&c, Token::Str), c.palette.success);
        assert_eq!(token_color(&c, Token::Punct), c.dim);
        assert_ne!(token_color(&c, Token::Key), token_color(&c, Token::Str));
    }

    #[test]
    fn utf16_spans_account_for_multibyte_values() {
        // "😀" is one char but two UTF-16 units; the number after it must land
        // at the right UTF-16 offset, not its byte offset.
        let text = "{\"k\": \"😀\", \"n\": 5}";
        let spans = utf16_spans(text, Lang::Json);
        let n = spans.iter().find(|(_, _, t)| *t == Token::Number).expect("number span");
        // UTF-16 index of the "5".
        let want: usize = text[..text.find('5').unwrap()].encode_utf16().count();
        assert_eq!(n.0, want);
        assert_eq!(n.1, 1);
        // The key/string spans stay inside the string.
        let s = spans.iter().find(|(_, _, t)| *t == Token::Str).expect("string span");
        let got: String = String::from_utf16(&text.encode_utf16().collect::<Vec<_>>()[s.0..s.0 + s.1]).unwrap();
        assert_eq!(got, "\"😀\"");
    }

    /// The AppKit builder actually paints the token colors onto the attributed
    /// string (the piece the pure tests cannot see). macOS-only, like the
    /// theme's `nscolor_conversion` test.
    #[cfg(target_os = "macos")]
    #[test]
    fn attributed_applies_token_colors() {
        use objc2::rc::Retained;
        use objc2_app_kit::{NSColor, NSFont, NSFontWeightRegular, NSForegroundColorAttributeName};
        use objc2_foundation::NSRange;

        let c = crate::ui::theme::PopupColors::default();
        let font = unsafe { NSFont::monospacedSystemFontOfSize_weight(13.0, NSFontWeightRegular) };
        let attr = attributed("{\"a\": 1}", &font, &c);
        let color_at = |i: usize| -> Retained<NSColor> {
            let mut eff = NSRange::new(0, 0);
            let attrs = unsafe { attr.attributesAtIndex_effectiveRange(i, &mut eff) };
            let col = attrs
                .objectForKey(unsafe { NSForegroundColorAttributeName })
                .expect("foreground color");
            col.downcast::<NSColor>().ok().expect("an NSColor")
        };
        let eq = |a: &NSColor, b: &NSColor| {
            (a.redComponent() - b.redComponent()).abs() < 1e-6
                && (a.greenComponent() - b.greenComponent()).abs() < 1e-6
                && (a.blueComponent() - b.blueComponent()).abs() < 1e-6
        };
        // `{`(0) `"a"`(1..4) `:`(4) ` `(5) `1`(6) `}`(7).
        assert!(eq(&color_at(1), &token_color(&c, Token::Key).to_nscolor()));
        assert!(eq(&color_at(6), &token_color(&c, Token::Number).to_nscolor()));
        assert!(eq(&color_at(0), &token_color(&c, Token::Punct).to_nscolor()));
    }
}
