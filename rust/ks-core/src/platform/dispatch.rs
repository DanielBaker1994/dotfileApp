//! Main-thread dispatch seam. `run_on_main` replaces the direct
//! `dispatch2::DispatchQueue::main().exec_async(…)` hops worker threads were
//! making back to the AppKit main thread at the call sites (WP8 audit).
//!
//! Semantics: `f` runs asynchronously on the main thread — FIFO with the main
//! queue like any other `exec_async`, never before `run_on_main` returns, and
//! never at all when nothing services the main queue. The replaced call sites
//! all passed a `Send + FnOnce() + 'static` closure, so their bodies are
//! unchanged; only the dispatch call moved behind this seam.
//!
//! Out of scope (audited, not migrated): the `Queue::main().after(…)` delay
//! sites, `MainThreadBound` construction (it stays at the call sites), and the
//! two repeating run-loop `NSTimer` drains (`DaemonUi.drain:`,
//! `AIHandler.pollStream:`).

/// Run `f` on the main thread, asynchronously
/// (was `dispatch2::DispatchQueue::main().exec_async(f)` at every call site).
#[cfg(target_os = "macos")]
pub fn run_on_main(f: impl FnOnce() + Send + 'static) {
    // The replaced call, moved verbatim: one dispatch2 main-queue enqueue.
    dispatch2::DispatchQueue::main().exec_async(f);
}

