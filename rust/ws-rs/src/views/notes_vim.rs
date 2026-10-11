//! The notes pane's embedded `nvim` (mirrors the vim pane in `PopupWindow.swift`).
//!
//! Surface: a SwiftTerm terminal view (`swiftterm_shim`) running
//! `nvim --listen <sock> -u <asset>/vim/init.lua --cmd … <file>`; the `init.lua`
//! config owns autosave, the inline-image sidecar (`<sock>.images.json`) and the
//! snippet picker. This module owns the process lifecycle, the socket/args/env
//! contract and the RPC glue (`engines::nvim_rpc`).
//!
//! See `bin/ui-test-vim.sh` for the end-to-end contract:
//! * `--listen $HOME/.cache/kitchen-sink/nvim-notes-<pid>.sock` (so
//!   `nvim --server … --remote-expr` and `NvimRpc` can drive the same editor);
//! * `g:ws_sock` points at the app's command socket (`$TMPDIR/ws-notes.sock`)
//!   so `<leader>sf` / `<leader>sg` can ask the host for the find popup;
//! * `g:ws_img_file` / `g:ws_img_rows` / `g:ws_cell_h` feed the inline-image
//!   sidecar the notes surface draws over the terminal.
//!
//! ## Environment (important)
//!
//! The shim's `WSShim.start(executable:args:directory:)` calls SwiftTerm's
//! `startProcess(executable:args:currentDirectory:)` with **no** environment
//! parameter, so the child gets SwiftTerm's fixed minimal env
//! (`Terminal.getEnvironmentVariables`: `TERM=xterm-256color`,
//! `COLORTERM=truecolor`, `LANG=en_US.UTF-8`, plus `HOME`/`USER`/`LOGNAME`/
//! `DISPLAY`/`LC_TYPE` copied from the host). **`PATH` is deliberately not
//! inherited** (it is commented out in SwiftTerm). [`launch_env`] is the env
//! the Swift app passes directly and the env a future shim with an `environment:`
//! parameter should use; until then [`env_cmds`] recovers the one variable that
//! matters (`PATH`) by injecting `let $PATH='…'` through `--cmd`, which nvim
//! applies to itself and its children (so `executable('pbcopy')`, lazy.nvim's
//! `git`, … work). Mutating the host process env around `start_process` would
//! not help: SwiftTerm only whitelists the variables above.

use std::sync::Mutex;
use std::time::Duration;

#[cfg(target_os = "macos")]
use objc2::rc::Retained;
#[cfg(target_os = "macos")]
use objc2::MainThreadMarker;
#[cfg(target_os = "macos")]
use objc2_app_kit::NSView;
#[cfg(target_os = "macos")]
use objc2_foundation::NSRect;

use crate::engines::nvim_rpc::{vim_string, NvimRpc};

/// RPC deadline for interactive `eval` / `command` / `remote` calls. Short so a
/// wedged editor never blocks the main thread for long (Swift's `NvimRPC`
/// uses the same order of magnitude).
pub const RPC_TIMEOUT: Duration = Duration::from_secs(1);

/// `[notes] image-rows` default (Swift `CommandSpec.imageRows`).
pub const DEFAULT_IMAGE_ROWS: i64 = 10;

/// Fallback cell height (points) for `g:ws_cell_h`; the notes surface computes
/// the real value from the terminal font (`PopupWindow.cellSize`).
pub const DEFAULT_CELL_H: i64 = 16;

/// Directories Swift's `vimEnvironment` guarantees on `PATH`, appended in order
/// when missing.
pub const NEEDED_PATH_DIRS: [&str; 6] = [
    "/opt/homebrew/bin",
    "/usr/local/bin",
    "/usr/bin",
    "/bin",
    "/usr/sbin",
    "/sbin",
];

/// `$HOME/.cache/kitchen-sink/nvim-notes-<pid>.sock` (Swift
/// `PopupController.vimSocket`).
pub fn socket_path(pid: i32) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    format!("{home}/.cache/kitchen-sink/nvim-notes-{pid}.sock")
}

/// `<sock>` minus `.sock` plus `.images.json` (Swift `vimImageFile`).
pub fn image_file(sock: &str) -> String {
    let base = sock.strip_suffix(".sock").unwrap_or(sock);
    format!("{base}.images.json")
}

/// The app's command socket (`$TMPDIR/ws-notes.sock`, `Paths::notes_socket_path`)
/// resolved from the environment — the value `g:ws_sock` must carry.
pub fn notes_socket_path() -> String {
    let tmp = crate::app::paths::popup_tmp_dir(std::env::var("TMPDIR").ok().as_deref());
    format!("{tmp}{}", crate::app::paths::NOTES_SOCKET_NAME)
}

// ---------------------------------------------------------------------------
// Launch arguments (pure)
// ---------------------------------------------------------------------------

/// Mirror of Swift `PopupController.vimArgs(for:socket:file:)`, minus the theme
/// colour `let`s (the caller passes those through `extra_cmds`).
///
/// `sock` is the nvim `--listen` socket ([`socket_path`]); `notes_sock` is the
/// app command socket ([`notes_socket_path`]) that `g:ws_sock` must point at.
/// Each `extra_cmds` entry becomes its own `--cmd` (e.g. `let $PATH='…'` from
/// [`env_cmds`]); `--cmd` runs before `-u`, which is what `init.lua` expects.
pub fn launch_args(
    asset_dir: &str,
    file: Option<&str>,
    sock: &str,
    notes_sock: &str,
    image_file: &str,
    image_rows: i64,
    cell_h: i64,
    extra_cmds: &[String],
) -> Vec<String> {
    let mut a: Vec<String> = vec![
        "--listen".into(),
        sock.into(),
        "-u".into(),
        format!("{asset_dir}/vim/init.lua"),
        "--cmd".into(),
        format!("let g:ws_sock={}", vim_string(notes_sock)),
        "--cmd".into(),
        format!("let g:ws_img_file={}", vim_string(image_file)),
        "--cmd".into(),
        format!("let g:ws_img_rows={}", image_rows.max(1)),
        "--cmd".into(),
        format!("let g:ws_cell_h={}", cell_h.max(1)),
    ];
    for cmd in extra_cmds {
        a.push("--cmd".into());
        a.push(cmd.clone());
    }
    if let Some(file) = file {
        a.push(file.to_string());
    }
    a
}

// ---------------------------------------------------------------------------
// Environment (pure)
// ---------------------------------------------------------------------------

/// Mirror of Swift `PopupWindow.vimEnvironment()`: prepend the homebrew/system
/// dirs to `PATH`, force a colour-capable terminal, default `LANG`, and strip
/// the inherited nvim server variables so a nested `nvim` never reuses the
/// parent's socket. Sorted by key for a deterministic order.
pub fn launch_env() -> Vec<(String, String)> {
    launch_env_from(std::env::vars().collect())
}

/// [`launch_env`] over an explicit environment (test seam).
pub fn launch_env_from(mut env: Vec<(String, String)>) -> Vec<(String, String)> {
    fn get(env: &[(String, String)], key: &str) -> Option<String> {
        env.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
    }
    fn set(env: &mut Vec<(String, String)>, key: &str, value: String) {
        match env.iter_mut().find(|(k, _)| k == key) {
            Some((_, v)) => *v = value,
            None => env.push((key.to_string(), value)),
        }
    }

    let mut path: Vec<String> = get(&env, "PATH")
        .unwrap_or_default()
        .split(':')
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();
    for dir in NEEDED_PATH_DIRS {
        if !path.iter().any(|p| p == dir) {
            path.push(dir.to_string());
        }
    }
    set(&mut env, "PATH", path.join(":"));
    set(&mut env, "TERM", "xterm-256color".to_string());
    set(&mut env, "COLORTERM", "truecolor".to_string());
    if get(&env, "LANG").unwrap_or_default().is_empty() {
        set(&mut env, "LANG", "en_US.UTF-8".to_string());
    }
    env.retain(|(k, _)| k != "NVIM" && k != "NVIM_LISTEN_ADDRESS");
    env.sort_by(|a, b| a.0.cmp(&b.0));
    env
}

/// `--cmd` entries that recover the env the shim cannot pass. Today that is just
/// `PATH`; the colour variables are already fixed by SwiftTerm.
pub fn env_cmds() -> Vec<String> {
    match launch_env().iter().find(|(k, _)| k == "PATH") {
        Some((_, path)) if !path.is_empty() => vec![format!("let $PATH={}", vim_string(path))],
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Ex commands / key decoding (pure, mirror PopupWindow's vim helpers)
// ---------------------------------------------------------------------------

/// `PopupWindow.vimOpen`'s ex string.
pub fn open_ex(path: &str) -> String {
    let lit = vim_string(path);
    format!("silent! wall | stopinsert | execute 'edit ' .. fnameescape({lit}) | redraw!")
}

/// `PopupWindow.vimRemote`'s `<…>` → raw PTY byte translation.
pub fn decode_keys(keys: &str) -> String {
    keys.replace("<C-\\><C-N>", "\u{1c}\u{0e}")
        .replace("<CR>", "\r")
        .replace("<Esc>", "\u{1b}")
}

/// The process-exit edge `poll_exit` reports: running before, gone now.
pub fn exit_transition(was_running: bool, running: bool) -> bool {
    was_running && !running
}

// ---------------------------------------------------------------------------
// Clipboard paste / image save (mirror PopupWindow's vimPaste + saveImage)
// ---------------------------------------------------------------------------

/// `PopupTextView.imageExts` — the extensions a dropped/pasted file URL may
/// have for [`save_pasteboard_image`] to try opening it as an image.
pub const IMAGE_EXTS: [&str; 8] = ["png", "jpg", "jpeg", "gif", "heic", "webp", "tif", "tiff"];

/// `PopupWindow.vimPaste`'s RPC expression: one single-quoted Vim string per
/// line, joined with `,`, fed to `nvim_paste`.
///
/// `text.split('\n')` mirrors Swift's `components(separatedBy: "\n")` (which,
/// like Rust's `split`, yields one empty component for an empty string); each
/// line drops every `\r` first (CRLF clipboards) and is then quoted by
/// [`vim_string`]. The separator argument is the two characters `\` `n` inside
/// double quotes, i.e. `"\\n"`.
pub fn paste_expr(text: &str) -> String {
    let lines: Vec<String> = text
        .split('\n')
        .map(|line| vim_string(&line.replace('\r', "")))
        .collect();
    format!(
        "nvim_paste(join([{}], \"\\n\"), v:true, -1)",
        lines.join(",")
    )
}

/// `saveImage`'s file name: `img-<epoch seconds>.png`.
pub fn image_name(epoch_secs: u64) -> String {
    format!("img-{epoch_secs}.png")
}

/// `saveImage`'s directory: the note's parent plus `/assets`, or `None` when
/// the note has no parent directory (a bare file name).
pub fn image_dir(note_path: &str) -> Option<String> {
    let parent = std::path::Path::new(note_path).parent()?;
    if parent.as_os_str().is_empty() {
        return None;
    }
    Some(format!("{}/assets", parent.to_string_lossy()))
}

/// `PopupTextView.image(from:)` + `saveImage`: an image off the general
/// pasteboard, re-encoded to PNG next to `note_path`, returning the
/// markdown-relative `assets/<name>` link. `None` when there is no usable image
/// or the write fails. Never panics.
///
/// Source order mirrors Swift: a PNG/TIFF pasteboard *data* first, then a
/// `NSPasteboardTypeFileURL` whose extension is in [`IMAGE_EXTS`]. Only images
/// wider than 1pt count (a 1×1 placeholder is ignored).
#[cfg(target_os = "macos")]
pub fn save_pasteboard_image(note_path: &str) -> Option<String> {
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::AnyThread;
    use objc2_app_kit::{
        NSBitmapImageFileType, NSBitmapImageRep, NSBitmapImageRepPropertyKey, NSImage,
        NSPasteboard, NSPasteboardTypeFileURL, NSPasteboardTypePNG, NSPasteboardTypeTIFF,
    };
    use objc2_foundation::{NSDictionary, NSString, NSURL};

    /// The pasteboard image, if any (Swift `PopupTextView.image(from:)`).
    fn pasteboard_image(pb: &NSPasteboard) -> Option<Retained<NSImage>> {
        let types = [
            unsafe { NSPasteboardTypePNG },
            unsafe { NSPasteboardTypeTIFF },
        ];
        for ty in types {
            if let Some(data) = pb.dataForType(ty) {
                if let Some(img) = NSImage::initWithData(NSImage::alloc(), &data) {
                    return Some(img);
                }
            }
        }
        let url_str = pb.stringForType(unsafe { NSPasteboardTypeFileURL })?;
        let url = NSURL::URLWithString(&url_str)?;
        let path = url.path()?.to_string();
        let ext = std::path::Path::new(&path)
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())?;
        if !IMAGE_EXTS.contains(&ext.as_str()) {
            return None;
        }
        NSImage::initWithContentsOfFile(NSImage::alloc(), &NSString::from_str(&path))
    }

    let pb = NSPasteboard::generalPasteboard();
    let img = pasteboard_image(&pb)?;
    if img.size().width <= 1.0 {
        return None;
    }

    let dir = image_dir(note_path)?;
    std::fs::create_dir_all(&dir).ok()?;
    let epoch_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    let name = image_name(epoch_secs);
    let tiff = img.TIFFRepresentation()?;
    let rep = NSBitmapImageRep::initWithData(NSBitmapImageRep::alloc(), &tiff)?;
    let props: Retained<NSDictionary<NSBitmapImageRepPropertyKey, AnyObject>> = NSDictionary::new();
    let data =
        unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &props) }?;
    std::fs::write(format!("{dir}/{name}"), data.to_vec()).ok()?;
    Some(format!("assets/{name}"))
}

/// No pasteboard off macOS. Never panics.
#[cfg(not(target_os = "macos"))]
pub fn save_pasteboard_image(_note_path: &str) -> Option<String> {
    None
}

// ---------------------------------------------------------------------------
// Launch configuration
// ---------------------------------------------------------------------------

/// Everything `build` needs to start the editor, resolved once and overridable
/// by the caller (tests / alternate sockets).
#[derive(Clone, Debug, PartialEq)]
pub struct VimLaunch {
    /// nvim `--listen` socket ([`socket_path`]).
    pub sock: String,
    /// App command socket for `g:ws_sock` ([`notes_socket_path`]).
    pub notes_sock: String,
    /// Inline-image sidecar ([`image_file`]).
    pub image_file: String,
    /// `g:ws_img_rows`.
    pub image_rows: i64,
    /// `g:ws_cell_h`.
    pub cell_h: i64,
    /// Extra `--cmd`s (defaults to [`env_cmds`], i.e. the `PATH` recovery).
    pub extra_cmds: Vec<String>,
    /// Working directory; `None` = the note's directory, else `$HOME`.
    pub cwd: Option<String>,
}

impl VimLaunch {
    /// The defaults for this process: the per-pid nvim socket, the app command
    /// socket, the sidecar, `[notes]` image rows (default 10), the fallback cell
    /// height, the `PATH` recovery command.
    pub fn for_pid(pid: i32) -> Self {
        let sock = socket_path(pid);
        VimLaunch {
            image_file: image_file(&sock),
            notes_sock: notes_socket_path(),
            sock,
            image_rows: DEFAULT_IMAGE_ROWS,
            cell_h: DEFAULT_CELL_H,
            extra_cmds: env_cmds(),
            cwd: None,
        }
    }

    /// [`launch_args`] for this configuration.
    pub fn args(&self, asset_dir: &str, file: Option<&str>) -> Vec<String> {
        launch_args(
            asset_dir,
            file,
            &self.sock,
            &self.notes_sock,
            &self.image_file,
            self.image_rows,
            self.cell_h,
            &self.extra_cmds,
        )
    }
}

// ---------------------------------------------------------------------------
// The pane
// ---------------------------------------------------------------------------

/// The notes pane's embedded editor: the SwiftTerm shim view running `nvim`,
/// plus the `NvimRpc` client used to drive it without stealing focus.
pub struct VimPane {
    vim_bin: String,
    asset_dir: String,
    file: Option<String>,
    launch: VimLaunch,
    font: Option<(String, f64)>,
    #[cfg(target_os = "macos")]
    view: Option<Retained<NSView>>,
    /// Lazily opened, reused across calls; `NvimRpc` reconnects internally.
    rpc: Mutex<Option<NvimRpc>>,
    was_running: bool,
    shutting_down: bool,
}

impl VimPane {
    /// `vim_bin` is `[notes] vim-bin` (e.g. `"nvim"`), `asset_dir` the resolved
    /// app resource dir, `file` the note to open (or `None`).
    pub fn new(
        vim_bin: impl Into<String>,
        asset_dir: impl Into<String>,
        file: Option<String>,
    ) -> Self {
        VimPane {
            vim_bin: vim_bin.into(),
            asset_dir: asset_dir.into(),
            file,
            launch: VimLaunch::for_pid(std::process::id() as i32),
            font: None,
            #[cfg(target_os = "macos")]
            view: None,
            rpc: Mutex::new(None),
            was_running: false,
            shutting_down: false,
        }
    }

    /// Replace the launch configuration (sockets, sidecar, extra `--cmd`s, cwd).
    pub fn set_launch(&mut self, launch: VimLaunch) {
        self.launch = launch;
    }

    pub fn launch(&self) -> &VimLaunch {
        &self.launch
    }

    pub fn file(&self) -> Option<&str> {
        self.file.as_deref()
    }

    pub fn is_shutting_down(&self) -> bool {
        self.shutting_down
    }

    /// `PopupWindow.vimFont`-style font; applied immediately when built, else
    /// stored for `build`.
    pub fn set_font(&mut self, name: &str, size: f64) {
        self.font = Some((name.to_string(), size));
        #[cfg(target_os = "macos")]
        if let Some(shim) = self.shim() {
            swiftterm_shim::set_font(&shim, name, size);
        }
        #[cfg(not(target_os = "macos"))]
        let _ = (name, size);
    }

    /// Create the SwiftTerm view and start `nvim` behind it. Returns the view
    /// (the shim itself) for embedding, or `None` when SwiftTerm is unavailable
    /// or the view cannot be built. Never panics.
    #[cfg(target_os = "macos")]
    pub fn build(
        &mut self,
        mtm: MainThreadMarker,
        frame: NSRect,
    ) -> Option<Retained<NSView>> {
        if let Some(view) = &self.view {
            return Some(view.clone());
        }
        let view = swiftterm_shim::create_terminal(mtm, frame);
        let shim = match view.clone().downcast::<swiftterm_shim::WSShim>() {
            Ok(shim) => shim,
            Err(_) => return None,
        };
        if let Some((name, size)) = &self.font {
            swiftterm_shim::set_font(&shim, name, *size);
        }
        let args = self.launch.args(&self.asset_dir, self.file.as_deref());
        let cwd = self.launch.cwd.clone().or_else(|| self.note_dir());
        swiftterm_shim::start_process(&shim, &self.vim_bin, &args, cwd.as_deref());
        self.view = Some(view.clone());
        // The shim's `processTerminated` callback is the exit edge; until it
        // fires the pane counts as running (`terminalRunning` is unreliable:
        // SwiftTerm assigns its process asynchronously).
        self.was_running = true;
        Some(view)
    }

    #[cfg(not(target_os = "macos"))]
    pub fn build(&mut self, _mtm: MainThreadMarker, _frame: ()) -> Option<()> {
        None
    }

    /// `startVimIfNeeded()` after `:q` / a crash: restart the editor in the
    /// **same** terminal view, like Swift's `TerminalAutoRestart`. Dropping the
    /// old SwiftTerm view and building a new one kills the new child at once
    /// (the old view's deinit tears down its PTY after the new `forkpty`, and
    /// the new child dies by signal with no exit code). Clears the stale
    /// `--listen` socket first. Returns whether the process is running again.
    #[cfg(target_os = "macos")]
    pub fn restart(&mut self) -> bool {
        let Some(shim) = self.shim() else {
            return false;
        };
        if self.shutting_down {
            return false;
        }
        let _ = std::fs::remove_file(&self.launch.sock);
        if let Some(dir) = std::path::Path::new(&self.launch.sock).parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(mut rpc) = self.rpc.lock() {
            *rpc = None;
        }
        let args = self.launch.args(&self.asset_dir, self.file.as_deref());
        let cwd = self.launch.cwd.clone().or_else(|| self.note_dir());
        swiftterm_shim::start_process(&shim, &self.vim_bin, &args, cwd.as_deref());
        self.was_running = true;
        shim.terminal_running()
    }

    #[cfg(not(target_os = "macos"))]
    pub fn restart(&mut self) -> bool {
        false
    }

    /// The embedded view once built.
    #[cfg(target_os = "macos")]
    pub fn view(&self) -> Option<Retained<NSView>> {
        self.view.clone()
    }

    #[cfg(not(target_os = "macos"))]
    pub fn view(&self) -> Option<()> {
        None
    }

    /// Make the inner terminal view the first responder of its window (a no-op
    /// without a window, e.g. before the pane is attached).
    #[cfg(target_os = "macos")]
    pub fn focus(&self) {
        let Some(shim) = self.shim() else { return };
        let tv = shim.terminal_view();
        if let Some(window) = tv.window() {
            window.makeFirstResponder(Some(&tv));
        }
    }

    /// Whether the inner terminal view is its window's first responder (the
    /// `:q` relaunch's reclaim check).
    #[cfg(target_os = "macos")]
    pub fn has_focus(&self) -> bool {
        let Some(shim) = self.shim() else { return false };
        let tv = shim.terminal_view();
        let Some(window) = tv.window() else { return false };
        let Some(fr) = window.firstResponder() else { return false };
        let a = objc2::rc::Retained::as_ptr(&fr) as *const objc2::runtime::AnyObject;
        let b = &*tv as *const objc2_app_kit::NSView as *const objc2::runtime::AnyObject;
        a == b
    }

    #[cfg(not(target_os = "macos"))]
    pub fn has_focus(&self) -> bool {
        false
    }

    /// Send raw PTY keys/text to the editor (shim `sendKeys:`). No `<…>` key
    /// notation is decoded here — use [`Self::remote`] for that.
    #[cfg(target_os = "macos")]
    pub fn send_keys(&self, keys: &str) {
        if let Some(shim) = self.shim() {
            swiftterm_shim::send_text(&shim, keys);
        }
    }

    /// Send `<…>`-notated keys like Swift's `vimRemote`: first try the RPC
    /// (`nvim_input`, which understands the notation), else decode to raw bytes
    /// and write them to the PTY.
    pub fn remote(&self, keys: &str) {
        if let Some(guard) = self.ensure_rpc() {
            if let Some(rpc) = guard.as_ref() {
                if rpc.send_keys(keys, RPC_TIMEOUT).is_ok() {
                    return;
                }
            }
        }
        #[cfg(target_os = "macos")]
        if let Some(shim) = self.shim() {
            swiftterm_shim::send_text(&shim, &decode_keys(keys));
        }
        #[cfg(not(target_os = "macos"))]
        let _ = keys;
    }

    /// `--remote-expr`: evaluate `expr` in the editor, `None` when it is not
    /// reachable (missing socket / RPC error). Never blocks longer than
    /// [`RPC_TIMEOUT`].
    pub fn eval(&self, expr: &str) -> Option<String> {
        let guard = self.ensure_rpc()?;
        guard.as_ref()?.vim_eval(expr, RPC_TIMEOUT).ok()
    }

    /// `PopupWindow.vimCommand`: run ex via `execute()`, else send it as keys.
    pub fn command(&self, ex: &str) {
        if self
            .eval(&format!("execute({})", vim_string(ex)))
            .is_some()
        {
            return;
        }
        self.remote(&format!("<C-\\><C-N>:{ex} | echo ''<CR>"));
    }

    /// `PopupWindow.vimPaste`: paste `text` via `nvim_paste` (so nvim keeps its
    /// own registers/undo), else fall back to the PTY with bracketed-paste
    /// markers — Swift's exact order. A no-op for empty text.
    pub fn paste(&self, text: &str) {
        if text.is_empty() {
            return;
        }
        if self.eval(&paste_expr(text)).is_some() {
            return;
        }
        #[cfg(target_os = "macos")]
        self.send_keys(&format!("\u{1b}[200~{text}\u{1b}[201~"));
    }

    /// Open `path`, flushing the current buffer and leaving insert mode first.
    /// Updates [`Self::file`]. Mirrors `PopupWindow.vimOpen`.
    pub fn open(&mut self, path: &str) {
        let ex = open_ex(path);
        let ok = self
            .eval(&format!("execute({})", vim_string(&ex)))
            .is_some();
        if !ok {
            let esc = path.replace(' ', "\\ ");
            self.remote(&format!("<C-\\><C-N>:silent! wall | edit {esc}<CR>"));
        }
        self.file = Some(path.to_string());
    }

    /// `PopupWindow.vimFlush` — `silent! wall`.
    pub fn flush(&self) {
        self.command("silent! wall");
    }

    /// Whether the editor process is alive (shim `terminalRunning`).
    #[cfg(target_os = "macos")]
    pub fn is_running(&self) -> bool {
        self.shim().map(|s| s.terminal_running()).unwrap_or(false)
    }

    #[cfg(not(target_os = "macos"))]
    pub fn is_running(&self) -> bool {
        false
    }

    /// The editor's exit code (shim `terminalExitCode`); `-1` while running or
    /// unknown.
    #[cfg(target_os = "macos")]
    pub fn exit_code(&self) -> i32 {
        self.shim().map(|s| s.terminal_exit_code()).unwrap_or(-1)
    }

    #[cfg(not(target_os = "macos"))]
    pub fn exit_code(&self) -> i32 {
        -1
    }

    /// Flush and quit the editor (`:wall` then `:qa!`), best-effort. After this
    /// [`Self::poll_exit`] stays quiet so the caller does not relaunch.
    pub fn shutdown(&mut self) {
        self.shutting_down = true;
        self.flush();
        if self.eval("execute('silent! wall')").is_none() {
            self.remote("<C-\\><C-N>:silent! wall<CR>");
        }
        self.remote("<C-\\><C-N>:qa!<CR>");
    }

    /// True once, when the process exited since the last call. Does **not**
    /// relaunch — the caller owns that policy (mirrors Swift's
    /// `TerminalAutoRestart`, which relaunches `:q` but not `shutdownVim`).
    /// Whether the child has terminated: the shim's `processTerminated`
    /// callback stores an exit code (`-1` until then). This is the reliable
    /// edge — `terminalRunning` reads through SwiftTerm's process property,
    /// which is assigned asynchronously after `startProcess`.
    #[cfg(target_os = "macos")]
    fn terminated(&self) -> bool {
        self.shim()
            .map(|s| s.terminal_exit_code() >= 0)
            .unwrap_or(false)
    }

    #[cfg(not(target_os = "macos"))]
    fn terminated(&self) -> bool {
        false
    }

    pub fn poll_exit(&mut self) -> bool {
        let running = !self.terminated();
        let exited = exit_transition(self.was_running, running);
        self.was_running = running;
        if self.shutting_down {
            return false;
        }
        exited
    }

    /// The `NvimRpc` client, opened on first use and reused. `NvimRpc`
    /// reconnects internally, and a missing socket fails fast, so this is safe
    /// to call before the editor is up.
    fn ensure_rpc(&self) -> Option<std::sync::MutexGuard<'_, Option<NvimRpc>>> {
        let mut guard = self.rpc.lock().ok()?;
        if guard.is_none() {
            *guard = Some(NvimRpc::new(self.launch.sock.clone()));
        }
        Some(guard)
    }

    /// The shim handle behind [`Self::view`] (it IS the returned `NSView`).
    #[cfg(target_os = "macos")]
    fn shim(&self) -> Option<Retained<swiftterm_shim::WSShim>> {
        self.view.as_ref()?.clone().downcast::<swiftterm_shim::WSShim>().ok()
    }

    /// The directory to start nvim in: the note's directory, else `$HOME`.
    fn note_dir(&self) -> Option<String> {
        if let Some(file) = &self.file {
            if let Some(parent) = std::path::Path::new(file).parent() {
                if !parent.as_os_str().is_empty() {
                    return Some(parent.to_string_lossy().into_owned());
                }
            }
        }
        std::env::var("HOME").ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_names_follow_swift() {
        let s = socket_path(4242);
        assert!(s.ends_with("/.cache/kitchen-sink/nvim-notes-4242.sock"), "{s}");
        assert_eq!(image_file(&s), format!("{}.images.json", s.trim_end_matches(".sock")));
    }

    #[test]
    fn launch_args_matches_swift_order() {
        let args = launch_args(
            "/asset",
            Some("/notes/a.md"),
            "/nv.sock",
            "/tmp/ws-notes.sock",
            "/nv.images.json",
            10,
            16,
            &[],
        );
        assert_eq!(
            args,
            vec![
                "--listen",
                "/nv.sock",
                "-u",
                "/asset/vim/init.lua",
                "--cmd",
                "let g:ws_sock='/tmp/ws-notes.sock'",
                "--cmd",
                "let g:ws_img_file='/nv.images.json'",
                "--cmd",
                "let g:ws_img_rows=10",
                "--cmd",
                "let g:ws_cell_h=16",
                "/notes/a.md",
            ]
        );
    }

    #[test]
    fn launch_args_clamps_rows_and_cell_and_omits_missing_file() {
        let args = launch_args("/a", None, "/s", "/n", "/i", 0, 0, &[]);
        assert!(args.contains(&"let g:ws_img_rows=1".to_string()));
        assert!(args.contains(&"let g:ws_cell_h=1".to_string()));
        assert_eq!(args.last().unwrap(), "let g:ws_cell_h=1");
        // no file appended
        assert!(!args.iter().any(|a| a.ends_with(".md")));
    }

    #[test]
    fn launch_args_appends_extra_cmds_before_file() {
        let extra = vec!["let $PATH='/x:/y'".to_string(), "let g:ws_fg='#fff'".to_string()];
        let args = launch_args("/a", Some("/n.md"), "/s", "/ns", "/i", 10, 16, &extra);
        assert_eq!(
            &args[args.len() - 5..],
            &[
                "--cmd",
                "let $PATH='/x:/y'",
                "--cmd",
                "let g:ws_fg='#fff'",
                "/n.md",
            ]
        );
    }

    #[test]
    fn launch_args_escapes_quotes_in_sockets() {
        let args = launch_args("/a", None, "/s", "/it's.sock", "/i", 10, 16, &[]);
        assert!(args.contains(&"let g:ws_sock='/it''s.sock'".to_string()));
    }

    #[test]
    fn vim_launch_for_pid_wires_sockets_and_env() {
        let launch = VimLaunch::for_pid(777);
        assert!(launch.sock.ends_with("nvim-notes-777.sock"));
        assert_eq!(launch.image_file, image_file(&launch.sock));
        assert!(launch.notes_sock.ends_with("ws-notes.sock"));
        assert_eq!(launch.image_rows, DEFAULT_IMAGE_ROWS);
        assert_eq!(launch.cell_h, DEFAULT_CELL_H);
        // PATH recovery is included by default
        assert!(launch.extra_cmds.iter().any(|c| c.starts_with("let $PATH=")));

        let args = launch.args("/asset", Some("/n.md"));
        assert_eq!(args[0], "--listen");
        assert_eq!(args[1], launch.sock);
        assert_eq!(args.last().unwrap(), "/n.md");
    }

    #[test]
    fn launch_env_prepends_path_and_sets_term() {
        let env = launch_env_from(vec![
            ("PATH".to_string(), "/usr/bin".to_string()),
            ("NVIM".to_string(), "/tmp/x.sock".to_string()),
            ("NVIM_LISTEN_ADDRESS".to_string(), "/tmp/y.sock".to_string()),
            ("LANG".to_string(), String::new()),
            ("HOME".to_string(), "/Users/x".to_string()),
        ]);
        let get = |k: &str| env.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone());
        assert_eq!(
            get("PATH").unwrap(),
            "/usr/bin:/opt/homebrew/bin:/usr/local/bin:/bin:/usr/sbin:/sbin"
        );
        assert_eq!(get("TERM").unwrap(), "xterm-256color");
        assert_eq!(get("COLORTERM").unwrap(), "truecolor");
        assert_eq!(get("LANG").unwrap(), "en_US.UTF-8");
        assert_eq!(get("NVIM"), None);
        assert_eq!(get("NVIM_LISTEN_ADDRESS"), None);
        assert_eq!(get("HOME").unwrap(), "/Users/x");
        // deterministic (sorted) order
        let keys: Vec<&String> = env.iter().map(|(k, _)| k).collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted);
    }

    #[test]
    fn launch_env_keeps_existing_lang_and_dedups_path() {
        let env = launch_env_from(vec![
            ("LANG".to_string(), "de_DE.UTF-8".to_string()),
            (
                "PATH".to_string(),
                "/opt/homebrew/bin:/custom/bin".to_string(),
            ),
        ]);
        let get = |k: &str| env.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone());
        assert_eq!(get("LANG").unwrap(), "de_DE.UTF-8");
        assert_eq!(
            get("PATH").unwrap(),
            "/opt/homebrew/bin:/custom/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
        );
    }

    #[test]
    fn env_cmds_sets_path_only() {
        let cmds = env_cmds();
        assert_eq!(cmds.len(), 1);
        assert!(cmds[0].starts_with("let $PATH='"));
        assert!(cmds[0].ends_with('\''));
    }

    #[test]
    fn ex_helpers_match_swift() {
        assert_eq!(open_ex("/a/b c.md"), "silent! wall | stopinsert | execute 'edit ' .. fnameescape('/a/b c.md') | redraw!");
        assert_eq!(open_ex("it's.md"), "silent! wall | stopinsert | execute 'edit ' .. fnameescape('it''s.md') | redraw!");
    }

    #[test]
    fn decode_keys_translates_notation() {
        assert_eq!(
            decode_keys("<C-\\><C-N>:w<CR>"),
            "\u{1c}\u{0e}:w\r"
        );
        assert_eq!(decode_keys("x<Esc>"), "x\u{1b}");
        assert_eq!(decode_keys("plain"), "plain");
    }

    #[test]
    fn exit_transition_is_an_edge() {
        assert!(!exit_transition(false, false));
        assert!(!exit_transition(false, true));
        assert!(!exit_transition(true, true));
        assert!(exit_transition(true, false));
    }

    #[test]
    fn paste_expr_matches_swift_for_single_and_multi_line() {
        assert_eq!(
            paste_expr("hello"),
            "nvim_paste(join(['hello'], \"\\n\"), v:true, -1)"
        );
        assert_eq!(
            paste_expr("a\nb\nc"),
            "nvim_paste(join(['a','b','c'], \"\\n\"), v:true, -1)"
        );
    }

    #[test]
    fn paste_expr_escapes_quotes_and_leaves_backslashes() {
        assert_eq!(
            paste_expr("it's a\\b"),
            "nvim_paste(join(['it''s a\\b'], \"\\n\"), v:true, -1)"
        );
    }

    #[test]
    fn paste_expr_strips_carriage_returns() {
        assert_eq!(
            paste_expr("a\r\nb\r"),
            "nvim_paste(join(['a','b'], \"\\n\"), v:true, -1)"
        );
        assert_eq!(
            paste_expr("only\r"),
            "nvim_paste(join(['only'], \"\\n\"), v:true, -1)"
        );
    }

    #[test]
    fn paste_expr_of_empty_string_has_one_empty_line() {
        assert_eq!(
            paste_expr(""),
            "nvim_paste(join([''], \"\\n\"), v:true, -1)"
        );
    }

    #[test]
    fn image_names_and_dirs_match_save_image() {
        assert_eq!(image_name(1699999999), "img-1699999999.png");
        assert_eq!(image_name(0), "img-0.png");
        assert_eq!(
            image_dir("/notes/foo.md").as_deref(),
            Some("/notes/assets")
        );
        assert_eq!(image_dir("foo.md"), None);
    }

    #[test]
    fn image_exts_matches_swift() {
        assert_eq!(IMAGE_EXTS.len(), 8);
        for ext in ["png", "jpg", "jpeg", "gif", "heic", "webp", "tif", "tiff"] {
            assert!(IMAGE_EXTS.contains(&ext), "missing {ext}");
        }
    }

    #[test]
    fn unbuilt_pane_is_inert() {
        let mut pane = VimPane::new("nvim", "/asset", Some("/n.md".to_string()));
        assert_eq!(pane.file(), Some("/n.md"));
        assert!(!pane.is_running());
        assert_eq!(pane.exit_code(), -1);
        assert!(!pane.poll_exit());
        assert!(pane.eval("1+1").is_none());
        pane.flush();
        pane.command("silent! wall");
        pane.remote("<C-\\><C-N>:qa!<CR>");
        pane.shutdown();
        assert!(pane.is_shutting_down());
        assert!(!pane.poll_exit());
    }

    // ---- live, gated on a real nvim (optional, cleaned up) ------------------

    struct ChildGuard(Option<std::process::Child>);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            if let Some(c) = self.0.as_mut() {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
    }

    fn find_nvim() -> Option<String> {
        use std::os::unix::fs::PermissionsExt;
        let mut candidates = vec![
            "/opt/homebrew/bin/nvim".to_string(),
            "/usr/local/bin/nvim".to_string(),
        ];
        if let Ok(path) = std::env::var("PATH") {
            for dir in path.split(':') {
                if !dir.is_empty() {
                    candidates.push(format!("{dir}/nvim"));
                }
            }
        }
        candidates.into_iter().find(|c| {
            std::fs::metadata(c)
                .map(|m| m.is_file() && (m.permissions().mode() & 0o111) != 0)
                .unwrap_or(false)
        })
    }

    fn start_nvim(nvim: &str, sock: &str) -> Option<ChildGuard> {
        use std::process::{Command, Stdio};
        let child = Command::new(nvim)
            .args(["--headless", "--clean", "--listen", sock])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let guard = ChildGuard(Some(child));
        for _ in 0..300 {
            if std::path::Path::new(sock).exists() {
                return Some(guard);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        None
    }

    #[test]
    fn live_open_round_trip() {
        let Some(nvim) = find_nvim() else {
            eprintln!("skipping live notes_vim test: no nvim executable");
            return;
        };
        let dir = std::env::temp_dir().join(format!("ws-rs-notesvim-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let sock = dir.join("n.sock");
        let sock = sock.to_string_lossy().to_string();
        let _ = std::fs::remove_file(&sock);

        let Some(_child) = start_nvim(&nvim, &sock) else {
            eprintln!("skipping live notes_vim test: server never started");
            return;
        };

        let mut launch = VimLaunch::for_pid(0);
        launch.sock = sock.clone();
        launch.image_file = image_file(&sock);
        launch.extra_cmds = Vec::new();

        let mut pane = VimPane::new(nvim, dir.to_string_lossy().into_owned(), None);
        pane.set_launch(launch);

        assert_eq!(pane.eval("1+1").as_deref(), Some("2"));

        let note = dir.join("note.md");
        let _ = std::fs::write(&note, "hello\n");
        let note_s = note.to_string_lossy().to_string();
        pane.open(&note_s);
        assert_eq!(pane.file(), Some(note_s.as_str()));
        assert_eq!(pane.eval("expand('%:t')").as_deref(), Some("note.md"));

        pane.command("silent! wall");
        pane.shutdown();

        drop(_child);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
