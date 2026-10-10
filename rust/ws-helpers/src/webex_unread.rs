//! Rust port of `notify/helpers/webex_unread.swift`.
//!
//! argv[1] = bundle id (default `Cisco-Systems.Spark`).
//! stdout: `count\t<AXValue of the messaging hub badge>\n` followed by one
//! `space\t<title>\n` per unread space row. Exit 2 without Accessibility
//! permission, 3 when Webex (or its main window) is not available.

use std::ffi::c_void;
use std::os::raw::c_int;
use std::ptr::null_mut;

use objc2_app_kit::NSRunningApplication;
use objc2_foundation::{NSNumber, NSString};

type CFTypeRef = *mut c_void;
type AXUIElementRef = CFTypeRef;

#[allow(non_snake_case, non_camel_case_types, dead_code)]
#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXUIElementCreateApplication(pid: c_int) -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(e: AXUIElementRef, attr: CFTypeRef, out: *mut CFTypeRef) -> c_int;
    fn AXUIElementSetMessagingTimeout(e: AXUIElementRef, timeout: f32) -> c_int;
    fn AXIsProcessTrusted() -> u8;

    fn CFRelease(cf: CFTypeRef);
    fn CFRetain(cf: CFTypeRef) -> CFTypeRef;
    fn CFGetTypeID(cf: CFTypeRef) -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFArrayGetTypeID() -> usize;
    fn CFNumberGetTypeID() -> usize;
    fn CFArrayGetCount(a: CFTypeRef) -> isize;
    fn CFArrayGetValueAtIndex(a: CFTypeRef, i: isize) -> *const c_void;
}

struct Cf(CFTypeRef);
impl Drop for Cf {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) };
        }
    }
}

unsafe fn copy_attr(e: AXUIElementRef, name: &str) -> Option<Cf> {
    let ns = NSString::from_str(name);
    let key = &*ns as *const NSString as *const c_void as *mut c_void;
    let mut out: CFTypeRef = null_mut();
    let ok = AXUIElementCopyAttributeValue(e, key, &mut out) == 0 && !out.is_null();
    if ok {
        Some(Cf(out))
    } else {
        None
    }
}

unsafe fn elems(e: AXUIElementRef, name: &str) -> Vec<Cf> {
    let Some(v) = copy_attr(e, name) else { return Vec::new() };
    if CFGetTypeID(v.0) != CFArrayGetTypeID() {
        return Vec::new();
    }
    let n = CFArrayGetCount(v.0);
    let mut out = Vec::with_capacity(n.max(0) as usize);
    for i in 0..n {
        let p = CFArrayGetValueAtIndex(v.0, i);
        if p.is_null() {
            continue;
        }
        out.push(Cf(CFRetain(p as CFTypeRef)));
    }
    out
}

unsafe fn cf_string(v: CFTypeRef) -> Option<String> {
    if v.is_null() || CFGetTypeID(v) != CFStringGetTypeID() {
        return None;
    }
    Some((&*(v as *const NSString)).to_string())
}

// Swift: (v as? String) ?? (v as? NSNumber)?.stringValue ?? ""
unsafe fn text(e: AXUIElementRef, name: &str) -> String {
    let Some(v) = copy_attr(e, name) else { return String::new() };
    if let Some(s) = cf_string(v.0) {
        return s;
    }
    if CFGetTypeID(v.0) == CFNumberGetTypeID() {
        return (&*(v.0 as *const NSNumber)).stringValue().to_string();
    }
    String::new()
}

unsafe fn find(id: &str, root: AXUIElementRef, depth: usize) -> Option<Cf> {
    let mut level: Vec<Cf> = vec![Cf(CFRetain(root))];
    for _ in 0..depth {
        let mut next: Vec<Cf> = Vec::new();
        for e in level {
            if text(e.0, "AXIdentifier") == id {
                return Some(e);
            }
            if text(e.0, "AXRole") != "AXMenuBar" {
                next.extend(elems(e.0, "AXChildren"));
            }
        }
        level = next;
    }
    None
}

fn fail(msg: &str, code: i32) -> ! {
    eprint!("webex_unread: {msg}\n");
    std::process::exit(code);
}

fn count_line(value: &str) -> String {
    format!("count\t{value}\n")
}

fn space_line(title: &str) -> String {
    format!("space\t{title}\n")
}

// Swift: !title.isEmpty, title != "Recommended messages", desc.contains(title + ", ")
fn qualifies(title: &str, desc: &str) -> bool {
    !title.is_empty() && title != "Recommended messages" && desc.contains(&(title.to_string() + ", "))
}

fn main() {
    let bundle = std::env::args().nth(1).unwrap_or_else(|| "Cisco-Systems.Spark".to_string());

    unsafe {
        if AXIsProcessTrusted() == 0 {
            fail("no Accessibility permission", 2);
        }

        let bid = NSString::from_str(&bundle);
        let apps = NSRunningApplication::runningApplicationsWithBundleIdentifier(&bid);
        let Some(webex) = apps.iter().next() else {
            fail("Webex is not running", 3);
        };

        let app = AXUIElementCreateApplication(webex.processIdentifier());
        AXUIElementSetMessagingTimeout(app, 2.0);

        let window = elems(app, "AXWindows")
            .into_iter()
            .find(|w| text(w.0, "AXIdentifier") == "main_window");
        let Some(window) = window else {
            fail("no Webex window", 3);
        };

        let Some(hub) = find("WTMessagingHubButton", window.0, 8) else {
            fail("no Webex window", 3);
        };

        let badge = elems(hub.0, "AXChildren")
            .into_iter()
            .find(|c| text(c.0, "AXRole") == "AXValueIndicator");

        let mut out = String::new();
        out.push_str(&count_line(&badge.map(|b| text(b.0, "AXValue")).unwrap_or_else(|| "0".to_string())));

        if let Some(list) = find("spaces_list", window.0, 8) {
            for row in elems(list.0, "AXChildren") {
                if text(row.0, "AXRole") != "AXRow" {
                    continue;
                }
                let Some(cell) = elems(row.0, "AXChildren").into_iter().next() else { continue };
                let Some(brick) = elems(cell.0, "AXChildren").into_iter().next() else { continue };
                if text(brick.0, "AXIdentifier") != "RegularSpaceBrickletCellView" {
                    continue;
                }
                let title = elems(brick.0, "AXChildren")
                    .into_iter()
                    .next()
                    .map(|c| text(c.0, "AXDescription"))
                    .unwrap_or_default();
                let desc = text(cell.0, "AXDescription");
                if qualifies(&title, &desc) {
                    out.push_str(&space_line(&title));
                }
            }
        }

        print!("{out}");
    }
}

#[cfg(test)]
mod tests {
    use super::{count_line, qualifies, space_line};

    #[test]
    fn count_line_format() {
        assert_eq!(count_line("5"), "count\t5\n");
        assert_eq!(count_line(""), "count\t\n");
    }

    #[test]
    fn space_line_format() {
        assert_eq!(space_line("Team Chat"), "space\tTeam Chat\n");
    }

    #[test]
    fn qualifies_matches_swift_predicate() {
        assert!(qualifies("Team Chat", "Team Chat, 2 unread"));
        assert!(!qualifies("", "anything, "));
        assert!(!qualifies("Recommended messages", "Recommended messages, x"));
        assert!(!qualifies("Team Chat", "no suffix here"));
        assert!(!qualifies("Team Chat", "Team Chat without comma"));
    }
}
