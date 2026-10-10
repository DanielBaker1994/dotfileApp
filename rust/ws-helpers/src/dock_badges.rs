//! Rust port of `notify/helpers/dock_badges.swift`.
//!
//! stdout: one `<bundle id>\t<badge label>\n` line per Dock item exposing an
//! `AXURL` that resolves to a bundle identifier. Exit 1 when the Dock is not
//! running, 2 without Accessibility permission (stderr note, like Swift).

use std::ffi::c_void;
use std::os::raw::c_int;
use std::ptr::null_mut;

use objc2_app_kit::NSRunningApplication;
use objc2_foundation::{NSBundle, NSString, NSURL};

type CFTypeRef = *mut c_void;
type AXUIElementRef = CFTypeRef;

#[allow(non_snake_case, non_camel_case_types, dead_code)]
#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXUIElementCreateApplication(pid: c_int) -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(e: AXUIElementRef, attr: CFTypeRef, out: *mut CFTypeRef) -> c_int;
    fn AXIsProcessTrusted() -> u8;

    fn CFRelease(cf: CFTypeRef);
    fn CFRetain(cf: CFTypeRef) -> CFTypeRef;
    fn CFGetTypeID(cf: CFTypeRef) -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFArrayGetTypeID() -> usize;
    fn CFURLGetTypeID() -> usize;
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

unsafe fn bundle_id(v: CFTypeRef) -> Option<String> {
    if v.is_null() || CFGetTypeID(v) != CFURLGetTypeID() {
        return None;
    }
    let url = &*(v as *const NSURL);
    NSBundle::bundleWithURL(url)?.bundleIdentifier().map(|s| s.to_string())
}

fn badge_line(id: &str, label: &str) -> String {
    format!("{id}\t{label}\n")
}

fn main() {
    unsafe {
        let dock_bundle = NSString::from_str("com.apple.dock");
        let apps = NSRunningApplication::runningApplicationsWithBundleIdentifier(&dock_bundle);
        let Some(dock) = apps.iter().next() else {
            std::process::exit(1);
        };
        let app = AXUIElementCreateApplication(dock.processIdentifier());

        if AXIsProcessTrusted() == 0 {
            eprint!("dock_badges: no Accessibility permission\n");
            std::process::exit(2);
        }

        let mut out = String::new();
        for list in elems(app, "AXChildren") {
            for item in elems(list.0, "AXChildren") {
                let Some(url) = copy_attr(item.0, "AXURL") else { continue };
                let Some(id) = bundle_id(url.0) else { continue };
                let label = copy_attr(item.0, "AXStatusLabel")
                    .and_then(|v| cf_string(v.0))
                    .unwrap_or_default();
                out.push_str(&badge_line(&id, &label));
            }
        }
        print!("{out}");
    }
}

#[cfg(test)]
mod tests {
    use super::badge_line;

    #[test]
    fn line_is_tab_separated_with_trailing_newline() {
        assert_eq!(badge_line("com.apple.mail", "3"), "com.apple.mail\t3\n");
    }

    #[test]
    fn empty_label_keeps_the_empty_field() {
        assert_eq!(badge_line("com.apple.mail", ""), "com.apple.mail\t\n");
    }
}
