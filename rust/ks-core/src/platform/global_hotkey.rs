//! System-wide hotkeys owned by this process (Carbon `RegisterEventHotKey`).
//!
//! The quick agent registers Hyper+S / Hyper+X itself, so a press goes from
//! the window server straight into its run loop: no hotkey daemon, no shell,
//! no client process, no socket write. `RegisterEventHotKey` needs no
//! Accessibility grant. Callbacks run on the main thread.
//!
//! Specs are `+`-joined: modifiers `hyper` (= cmd+ctrl+opt+shift), `cmd`,
//! `ctrl`, `opt`/`alt`, `shift`, then one key: a letter, digit, punctuation
//! (`/ , . ; ' [ ] \ - = ``), `space`, `return`, `tab`, `esc`, `delete`,
//! `f1`…`f12`. Example: `hyper+s`, `cmd+shift+/`.

/// Carbon modifier bits (`cmdKey`, `shiftKey`, `optionKey`, `controlKey`).
pub const CMD: u32 = 0x0100;
pub const SHIFT: u32 = 0x0200;
pub const OPT: u32 = 0x0800;
pub const CTRL: u32 = 0x1000;

/// `(virtual key code, Carbon modifiers)` for a spec like `hyper+s`.
pub fn parse_spec(spec: &str) -> Result<(u32, u32), String> {
    let spec = spec.trim().to_lowercase();
    if spec.is_empty() {
        return Err("empty hotkey".into());
    }
    // A trailing `+` key ("cmd++") is the plus/equals key.
    let (mods_part, key) = match spec.strip_suffix("++") {
        Some(rest) => (rest.to_string(), "=".to_string()),
        None => match spec.rsplit_once('+') {
            Some((m, k)) => (m.to_string(), k.to_string()),
            None => (String::new(), spec.clone()),
        },
    };
    let mut mods = 0;
    for m in mods_part.split('+').filter(|m| !m.is_empty()) {
        mods |= match m {
            "hyper" => CMD | CTRL | OPT | SHIFT,
            "cmd" | "command" => CMD,
            "ctrl" | "control" => CTRL,
            "opt" | "option" | "alt" => OPT,
            "shift" => SHIFT,
            other => return Err(format!("unknown modifier '{other}' in '{spec}'")),
        };
    }
    let code = key_code(&key).ok_or_else(|| format!("unknown key '{key}' in '{spec}'"))?;
    if mods == 0 {
        return Err(format!("'{spec}' has no modifier (a bare key would be taken from every app)"));
    }
    Ok((code, mods))
}

/// ANSI virtual key code for one key name.
fn key_code(k: &str) -> Option<u32> {
    let table: &[(&str, u32)] = &[
        ("a", 0), ("s", 1), ("d", 2), ("f", 3), ("h", 4), ("g", 5), ("z", 6), ("x", 7),
        ("c", 8), ("v", 9), ("b", 11), ("q", 12), ("w", 13), ("e", 14), ("r", 15),
        ("y", 16), ("t", 17), ("1", 18), ("2", 19), ("3", 20), ("4", 21), ("6", 22),
        ("5", 23), ("=", 24), ("9", 25), ("7", 26), ("-", 27), ("8", 28), ("0", 29),
        ("]", 30), ("o", 31), ("u", 32), ("[", 33), ("i", 34), ("p", 35), ("return", 36),
        ("l", 37), ("j", 38), ("'", 39), ("k", 40), (";", 41), ("\\", 42), (",", 43),
        ("/", 44), ("slash", 44), ("n", 45), ("m", 46), (".", 47), ("tab", 48),
        ("space", 49), ("`", 50), ("delete", 51), ("esc", 53), ("escape", 53),
        ("f1", 122), ("f2", 120), ("f3", 99), ("f4", 118), ("f5", 96), ("f6", 97),
        ("f7", 98), ("f8", 100), ("f9", 101), ("f10", 109), ("f11", 103), ("f12", 111),
    ];
    table.iter().find(|(n, _)| *n == k).map(|(_, c)| *c)
}

#[cfg(target_os = "macos")]
pub use carbon::{register, unregister_all};

#[cfg(target_os = "macos")]
mod carbon {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::ffi::c_void;

    #[repr(C)]
    struct EventTypeSpec {
        event_class: u32,
        event_kind: u32,
    }

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct EventHotKeyID {
        signature: u32,
        id: u32,
    }

    type Handler = extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> i32;

    #[link(name = "Carbon", kind = "framework")]
    extern "C" {
        fn GetEventDispatcherTarget() -> *mut c_void;
        fn InstallEventHandler(
            target: *mut c_void,
            handler: Handler,
            num_types: u32,
            list: *const EventTypeSpec,
            user_data: *mut c_void,
            out_ref: *mut *mut c_void,
        ) -> i32;
        fn RegisterEventHotKey(
            code: u32,
            modifiers: u32,
            id: EventHotKeyID,
            target: *mut c_void,
            options: u32,
            out_ref: *mut *mut c_void,
        ) -> i32;
        fn UnregisterEventHotKey(r: *mut c_void) -> i32;
        fn GetEventParameter(
            event: *mut c_void,
            name: u32,
            desired_type: u32,
            actual_type: *mut u32,
            size: usize,
            actual_size: *mut usize,
            data: *mut c_void,
        ) -> i32;
    }

    const K_EVENT_CLASS_KEYBOARD: u32 = u32::from_be_bytes(*b"keyb");
    const K_EVENT_HOT_KEY_PRESSED: u32 = 5;
    const K_EVENT_PARAM_DIRECT_OBJECT: u32 = u32::from_be_bytes(*b"----");
    const TYPE_EVENT_HOT_KEY_ID: u32 = u32::from_be_bytes(*b"hkid");
    const SIGNATURE: u32 = u32::from_be_bytes(*b"kshk");

    struct Entry {
        hotkey: *mut c_void,
        callback: Box<dyn Fn()>,
    }

    thread_local! {
        static INSTALLED: RefCell<bool> = const { RefCell::new(false) };
        static KEYS: RefCell<HashMap<u32, Entry>> = RefCell::new(HashMap::new());
        static NEXT_ID: RefCell<u32> = const { RefCell::new(1) };
    }

    extern "C" fn on_hotkey(_call: *mut c_void, event: *mut c_void, _user: *mut c_void) -> i32 {
        let mut hk = EventHotKeyID::default();
        let status = unsafe {
            GetEventParameter(
                event,
                K_EVENT_PARAM_DIRECT_OBJECT,
                TYPE_EVENT_HOT_KEY_ID,
                std::ptr::null_mut(),
                std::mem::size_of::<EventHotKeyID>(),
                std::ptr::null_mut(),
                &mut hk as *mut EventHotKeyID as *mut c_void,
            )
        };
        if status != 0 || hk.signature != SIGNATURE {
            return -9874; // eventNotHandledErr
        }
        // Run the callback outside the KEYS borrow (it may register keys).
        let cb: Option<*const dyn Fn()> =
            KEYS.with(|k| k.borrow().get(&hk.id).map(|e| &*e.callback as *const dyn Fn()));
        match cb {
            Some(cb) => {
                unsafe { (*cb)() };
                0
            }
            None => -9874,
        }
    }

    /// Register `spec` (see the module docs) system-wide; `callback` runs on
    /// the main thread at each press. Main thread only. `Err` carries the
    /// parse error or the Carbon status (-9878 = another app holds the key).
    pub fn register(spec: &str, callback: Box<dyn Fn()>) -> Result<(), String> {
        let (code, mods) = super::parse_spec(spec)?;
        let target = unsafe { GetEventDispatcherTarget() };
        let installed = INSTALLED.with(|i| *i.borrow());
        if !installed {
            let types = [EventTypeSpec { event_class: K_EVENT_CLASS_KEYBOARD, event_kind: K_EVENT_HOT_KEY_PRESSED }];
            let mut handler_ref = std::ptr::null_mut();
            let st = unsafe {
                InstallEventHandler(target, on_hotkey, 1, types.as_ptr(), std::ptr::null_mut(), &mut handler_ref)
            };
            if st != 0 {
                return Err(format!("InstallEventHandler failed ({st})"));
            }
            INSTALLED.with(|i| *i.borrow_mut() = true);
        }
        let id = NEXT_ID.with(|n| {
            let v = *n.borrow();
            *n.borrow_mut() = v + 1;
            v
        });
        let mut hotkey = std::ptr::null_mut();
        let st = unsafe {
            RegisterEventHotKey(code, mods, EventHotKeyID { signature: SIGNATURE, id }, target, 0, &mut hotkey)
        };
        if st != 0 {
            return Err(format!("RegisterEventHotKey '{spec}' failed ({st}{})", if st == -9878 { ": taken by another app" } else { "" }));
        }
        KEYS.with(|k| k.borrow_mut().insert(id, Entry { hotkey, callback }));
        Ok(())
    }

    /// Drop every hotkey this process registered (config reload).
    pub fn unregister_all() {
        KEYS.with(|k| {
            for (_, e) in k.borrow_mut().drain() {
                unsafe { UnregisterEventHotKey(e.hotkey) };
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_parse() {
        assert_eq!(parse_spec("hyper+s"), Ok((1, CMD | CTRL | OPT | SHIFT)));
        assert_eq!(parse_spec("Hyper+X"), Ok((7, CMD | CTRL | OPT | SHIFT)));
        assert_eq!(parse_spec("hyper+/"), Ok((44, CMD | CTRL | OPT | SHIFT)));
        assert_eq!(parse_spec("cmd+shift+f5"), Ok((96, CMD | SHIFT)));
        assert_eq!(parse_spec("alt++"), Ok((24, OPT)));
    }

    #[test]
    fn bad_specs_are_refused() {
        assert!(parse_spec("").is_err());
        assert!(parse_spec("s").is_err(), "a bare key is refused");
        assert!(parse_spec("hyper+nope").is_err());
        assert!(parse_spec("meta+s").is_err());
    }
}
