//! Rust handle for the SwiftTerm shim (Phase 0.7).
//!
//! `shim/WSShim.swift` wraps a SwiftTerm `TerminalView` in an `NSView`; it is
//! compiled and linked by `build.rs` against the pinned `libSwiftTerm.a`. When
//! the checkout/lib is absent the build script is a no-op and `swiftterm_present`
//! stays unset; the API below still COMPILES (so `ws-rs` builds either way) but
//! `create_terminal` must not be called at runtime without SwiftTerm linked.
#![cfg(target_os = "macos")]

mod present {
    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2::{extern_class, extern_methods, extern_protocol, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::NSView;
    use objc2_foundation::{NSArray, NSObjectProtocol, NSRect, NSString};

    extern_class!(
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSShim"]
        pub struct WSShim;
    );

    // The delegate hook the Swift side calls back into; implement it in Rust
    // with `define_class!` + `extern_conformance!` to observe terminal events.
    extern_protocol!(
        #[name = "WSShimDelegate"]
        pub unsafe trait WSShimDelegate: NSObjectProtocol {
            #[unsafe(method(shimEvent:value:))]
            unsafe fn shim_event(&self, kind: &NSString, value: &NSString);
        }
    );

    impl WSShim {
        extern_methods!(
            #[unsafe(method(makeTerminalWithFrame:))]
            pub fn make_terminal(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self>;

            #[unsafe(method(startWithExecutable:args:directory:))]
            pub fn start(
                &self,
                executable: &NSString,
                args: &NSArray<NSString>,
                directory: Option<&NSString>,
            );

            #[unsafe(method(sendKeys:))]
            pub fn send_keys(&self, text: &NSString);

            #[unsafe(method(setFontWithName:size:))]
            pub fn set_font(&self, name: &NSString, size: f64);

            #[unsafe(method(terminalView))]
            pub fn terminal_view(&self) -> Retained<NSView>;

            #[unsafe(method(terminalRunning))]
            pub fn terminal_running(&self) -> bool;

            #[unsafe(method(terminalExitCode))]
            pub fn terminal_exit_code(&self) -> i32;

            #[unsafe(method(setEventDelegate:))]
            pub fn set_event_delegate(
                &self,
                delegate: Option<&ProtocolObject<dyn WSShimDelegate>>,
            );
        );
    }

    // The ObjC runtime registers `WSShim` only if its object is kept by the
    // linker; objc2 looks the class up by name at runtime, so reference the
    // class symbol to pull the Swift object out of `libWSShim.a`. Only when the
    // shim actually linked (the symbol is otherwise undefined and would fail the
    // link).
    #[cfg(swiftterm_present)]
    unsafe extern "C" {
        #[link_name = "OBJC_CLASS_$_WSShim"]
        static WSShim_CLASS: u8;
    }

    fn force_link_shim() {
        #[cfg(swiftterm_present)]
        std::hint::black_box(core::ptr::addr_of!(WSShim_CLASS));
    }

    /// Build a shim view wrapping a SwiftTerm terminal. The returned `NSView`
    /// is the shim itself (`WSShim`); keep it for the lifetime of the panel.
    pub fn create_terminal(mtm: MainThreadMarker, frame: NSRect) -> Retained<NSView> {
        force_link_shim();
        WSShim::make_terminal(mtm, frame).into_super()
    }

    /// Send raw text (already in the terminal's key encoding) to the shell.
    pub fn send_text(view: &WSShim, text: &str) {
        let s = NSString::from_str(text);
        view.send_keys(&s);
    }

    /// Start the child process behind the terminal.
    pub fn start_process(
        view: &WSShim,
        executable: &str,
        args: &[String],
        directory: Option<&str>,
    ) {
        let exe = NSString::from_str(executable);
        let nsargs: Vec<Retained<NSString>> = args.iter().map(|a| NSString::from_str(a)).collect();
        let nsarray = NSArray::from_retained_slice(&nsargs);
        let dir = directory.map(NSString::from_str);
        view.start(&exe, &nsarray, dir.as_deref());
    }

    /// Set the terminal font (`[app] terminal-font` / `terminal-font-size`).
    pub fn set_font(view: &WSShim, name: &str, size: f64) {
        let n = NSString::from_str(name);
        view.set_font(&n, size);
    }
}

pub use present::*;
