//! Modal file/folder pickers (was direct `NSOpenPanel` calls).
//!
//! macOS keeps the app's existing `NSOpenPanel` implementation verbatim:
//! `runModal` blocks the calling thread until the user answers, exactly like
//! the two call sites this replaces. `NSOpenPanel` is main-thread-only, so
//! the macOS backend takes a `MainThreadMarker` itself and reports "nothing"
//! (`None`) off the main thread (the compare `E::Browse` call site used to
//! `unwrap()` it; the other already no-opped). Non-macOS targets use `rfd`'s
//! XDG desktop-portal backend instead.

/// What the picker accepts, mirroring the old panel configuration
/// (`canChooseFiles` / `canChooseDirectories` / `allowsMultipleSelection`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PickOpts {
    /// `canChooseFiles`: files may be selected.
    pub files: bool,
    /// `canChooseDirectories`: folders may be selected.
    pub dirs: bool,
    /// `allowsMultipleSelection`: the panel accepts more than one entry.
    /// No current caller passes `true`; [`pick_path`] returns the first
    /// selected path in that case.
    pub multiple: bool,
}

/// Show a modal file/folder picker and block until the user answers.
///
/// Returns the first chosen path, or `None` when the user cancels, when the
/// selection is empty, and when called off the main thread (macOS).
#[cfg(target_os = "macos")]
pub fn pick_path(opts: &PickOpts) -> Option<std::path::PathBuf> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSModalResponseOK, NSOpenPanel};
    let mtm = MainThreadMarker::new()?;
    let panel = NSOpenPanel::openPanel(mtm);
    panel.setCanChooseFiles(opts.files);
    panel.setCanChooseDirectories(opts.dirs);
    panel.setAllowsMultipleSelection(opts.multiple);
    if panel.runModal() != NSModalResponseOK {
        return None;
    }
    let url = panel.URLs().firstObject()?;
    url.path().map(|p| p.to_string().into())
}

/// Show a modal file/folder picker via `rfd` (XDG desktop portal). Same
/// blocking "first chosen path" contract as the macOS backend.
#[cfg(not(target_os = "macos"))]
pub fn pick_path(opts: &PickOpts) -> Option<std::path::PathBuf> {
    // `rfd` has no combined file-or-folder picker: folders-only uses the
    // folder picker, everything else the file picker (files-and-folders
    // degrades to files).
    let d = rfd::FileDialog::new();
    let folder = opts.dirs && !opts.files;
    match (folder, opts.multiple) {
        (true, true) => d.pick_folders().and_then(|v| v.into_iter().next()),
        (true, false) => d.pick_folder(),
        (false, true) => d.pick_files().and_then(|v| v.into_iter().next()),
        (false, false) => d.pick_file(),
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    /// The picker is main-thread-only on macOS (like `NSOpenPanel`); tests run
    /// off the main thread, so this must report `None` without opening a
    /// panel. Guards the seam's documented off-main contract.
    #[test]
    fn off_main_thread_returns_none() {
        assert!(
            objc2::MainThreadMarker::new().is_none(),
            "test must not run on the main thread"
        );
        let opts = PickOpts {
            files: true,
            dirs: true,
            multiple: false,
        };
        assert!(pick_path(&opts).is_none());
    }
}
