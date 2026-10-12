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

/// Non-macOS: no main run loop exists in this workspace yet, so `f` is pushed
/// onto a process-wide FIFO and runs only when the embedder's main loop calls
/// [`drain_main_queue`] (the counterpart of `NSApplication`'s run loop on
/// macOS). This is the least-wrong trivially-correct fallback: running `f`
/// inline would break the deferral these call sites rely on (they schedule
/// exactly to escape the current callback) and spawning a thread would run it
/// off the main thread. Uncompiled on macOS — see the commit body.
#[cfg(not(target_os = "macos"))]
pub fn run_on_main(f: impl FnOnce() + Send + 'static) {
    if let Ok(mut q) = QUEUE.lock() {
        q.push_back(Box::new(f));
    }
}

#[cfg(not(target_os = "macos"))]
static QUEUE: std::sync::Mutex<std::collections::VecDeque<Box<dyn FnOnce() + Send>>> =
    std::sync::Mutex::new(std::collections::VecDeque::new());

/// Run every job queued by [`run_on_main`] so far, in FIFO order. Non-macOS
/// embedders must call this from their main loop; jobs queued while draining
/// run in the same pass. A panic in one job propagates to the caller after the
/// job is dequeued.
#[cfg(not(target_os = "macos"))]
pub fn drain_main_queue() {
    loop {
        let job = match QUEUE.lock() {
            Ok(mut q) => q.pop_front(),
            Err(_) => None,
        };
        match job {
            Some(job) => job(),
            None => break,
        }
    }
}
