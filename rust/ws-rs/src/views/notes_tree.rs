//! The notes window's pinned-folder tree model (the "Pinned + tree" section of
//! the left rail). A root is a directory from `[notes] paths`; its children are
//! read one level at a time, only for folders the user has expanded, so a huge
//! directory never blocks the UI. The widget is `ui::tree_view`.

use std::collections::BTreeSet;
use std::path::Path;

/// One visible row of the tree (roots are depth 0).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TreeRow {
    pub path: String,
    pub name: String,
    pub depth: usize,
    pub is_dir: bool,
    pub expanded: bool,
    pub is_root: bool,
}

/// `[notes] paths` split into pinned directory roots and pinned files.
/// Both keep config order; entries that do not exist are dropped.
pub fn split_roots(raw: &str, is_dir: &dyn Fn(&str) -> bool, exists: &dyn Fn(&str) -> bool) -> (Vec<String>, Vec<String>) {
    let mut roots: Vec<String> = Vec::new();
    let mut files: Vec<String> = Vec::new();
    for p in super::notes::existing_note_paths(raw, exists) {
        let p = p.trim_end_matches('/').to_string();
        if is_dir(&p) {
            if !roots.contains(&p) {
                roots.push(p);
            }
        } else {
            files.push(p);
        }
    }
    (roots, files)
}

/// Entries directly inside `dir` as `(name, is_dir)`: folders first, then
/// files, each case-insensitively sorted. Dotfiles only when `show_hidden`.
pub fn list_dir(dir: &Path, show_hidden: bool) -> Vec<(String, bool)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(String, bool)> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            if !show_hidden && name.starts_with('.') {
                return None;
            }
            // `metadata` follows symlinks, so a linked folder shows as a folder.
            let is_dir = std::fs::metadata(e.path()).map(|m| m.is_dir()).unwrap_or(false);
            Some((name, is_dir))
        })
        .collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
    out
}

#[derive(Clone, Debug, Default)]
pub struct NotesTree {
    pub roots: Vec<String>,
    /// Paths (roots and folders) currently expanded.
    pub expanded: BTreeSet<String>,
    pub show_hidden: bool,
    /// The row highlighted as "the open file" (or a selected folder).
    pub selected: Option<String>,
}

impl NotesTree {
    pub fn new(roots: Vec<String>, expanded: BTreeSet<String>, show_hidden: bool) -> Self {
        NotesTree { roots, expanded, show_hidden, selected: None }
    }

    pub fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }

    /// The root `path` lives under (longest match), if any.
    pub fn root_of(&self, path: &str) -> Option<&str> {
        self.roots
            .iter()
            .filter(|r| path == r.as_str() || path.starts_with(&format!("{r}/")))
            .max_by_key(|r| r.len())
            .map(String::as_str)
    }

    /// Flatten the expanded roots into rows (reads only expanded folders).
    pub fn rows(&self) -> Vec<TreeRow> {
        let mut out = Vec::new();
        for r in &self.roots {
            let name = Path::new(r).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| r.clone());
            let open = self.expanded.contains(r);
            out.push(TreeRow { path: r.clone(), name, depth: 0, is_dir: true, expanded: open, is_root: true });
            if open {
                self.push_children(r, 1, &mut out);
            }
        }
        out
    }

    fn push_children(&self, dir: &str, depth: usize, out: &mut Vec<TreeRow>) {
        for (name, is_dir) in list_dir(Path::new(dir), self.show_hidden) {
            let path = format!("{dir}/{name}");
            let open = is_dir && self.expanded.contains(&path);
            out.push(TreeRow { path: path.clone(), name, depth, is_dir, expanded: open, is_root: false });
            if open {
                self.push_children(&path, depth + 1, out);
            }
        }
    }

    /// Flip a folder open/closed; returns the new state.
    pub fn toggle(&mut self, path: &str) -> bool {
        if self.expanded.remove(path) {
            false
        } else {
            self.expanded.insert(path.to_string());
            true
        }
    }

    /// Select `path` and expand every ancestor between its root and it.
    /// Returns false (selection cleared) when it is not under a pinned root,
    /// so the tree never claims a position the file does not have.
    pub fn reveal(&mut self, path: &str) -> bool {
        let Some(root) = self.root_of(path).map(str::to_string) else {
            self.selected = None;
            return false;
        };
        let mut dir = Path::new(path).parent();
        while let Some(d) = dir {
            let s = d.to_string_lossy();
            if s.len() < root.len() {
                break;
            }
            self.expanded.insert(s.into_owned());
            dir = d.parent();
        }
        self.expanded.insert(root);
        self.selected = Some(path.to_string());
        true
    }

    /// Re-resolve the pinned roots from `[notes] paths` (a rescan), keeping the
    /// expanded set and the selection. A folder that did not exist when the
    /// view was built joins the roots now, and opens, so its notes list without
    /// a click. Rows are always read live from disk by [`NotesTree::rows`], so
    /// the caller only has to repaint after this. Returns true when the root
    /// set changed (the caller re-splits the rail then).
    pub fn rescan(
        &mut self,
        raw: &str,
        is_dir: &dyn Fn(&str) -> bool,
        exists: &dyn Fn(&str) -> bool,
    ) -> bool {
        let (roots, _) = split_roots(raw, is_dir, exists);
        if roots == self.roots {
            return false;
        }
        for r in &roots {
            if !self.roots.contains(r) {
                self.expanded.insert(r.clone());
            }
        }
        self.roots = roots;
        true
    }

    /// Row index of `path` in `rows` (for scrolling it into view).
    pub fn row_of(rows: &[TreeRow], path: &str) -> Option<usize> {
        rows.iter().position(|r| r.path == path)
    }
}

/// Persisted expanded-set encoding (one path per line).
pub fn encode_expanded(set: &BTreeSet<String>) -> String {
    set.iter().cloned().collect::<Vec<_>>().join("\n")
}

pub fn decode_expanded(s: &str) -> BTreeSet<String> {
    s.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect()
}

const DEFAULTS_KEY: &str = "notesTree.expanded";

/// Expanded folders restored at launch (UI state, kept in the app's settings
/// beside the rail-collapsed flag rather than rewriting `commands.toml`).
pub fn load_expanded() -> BTreeSet<String> {
    crate::platform::get_string(DEFAULTS_KEY)
        .map(|s| decode_expanded(&s))
        .unwrap_or_default()
}

pub fn save_expanded(set: &BTreeSet<String>) {
    crate::platform::set_string(DEFAULTS_KEY, &encode_expanded(set));
}

/// The tree never takes more than this share of the rail; the Open list keeps the rest.
pub const TREE_MAX_FRACTION: f64 = 0.4;

const COLLAPSED_KEY: &str = "notesTree.collapsed";

/// Whether the whole tree card is collapsed to its header (persisted).
pub fn load_tree_collapsed() -> bool {
    crate::platform::get_bool(COLLAPSED_KEY)
}

pub fn save_tree_collapsed(on: bool) {
    crate::platform::set_bool(COLLAPSED_KEY, on);
}

/// Heights `(tree, open_list)` for the rail column. The tree is only as tall
/// as its `wanted` content, capped at `TREE_MAX_FRACTION` (it scrolls beyond
/// that); the Open list gets everything else.
pub fn rail_split(height: f64, show_tree: bool, wanted: f64) -> (f64, f64) {
    if !show_tree {
        return (0.0, height);
    }
    let t = wanted.min((height * TREE_MAX_FRACTION).round()).clamp(0.0, height).round();
    (t, height - t)
}

#[cfg(test)]
mod tests {
    #[test]
    fn rail_split_gives_tree_the_top_share() {
        assert_eq!(super::rail_split(600.0, false, 200.0), (0.0, 600.0));
        // A short tree takes only what it needs...
        assert_eq!(super::rail_split(600.0, true, 120.0), (120.0, 480.0));
        // ...and a long one is capped so the Open list stays visible.
        assert_eq!(super::rail_split(600.0, true, 5000.0), (240.0, 360.0));
    }

    use super::*;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("ws-notes-tree-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("work/2026")).unwrap();
        std::fs::create_dir_all(d.join("assets")).unwrap();
        std::fs::create_dir_all(d.join(".git")).unwrap();
        std::fs::write(d.join("helloworld.md"), "x").unwrap();
        std::fs::write(d.join("Default.md"), "x").unwrap();
        std::fs::write(d.join(".hidden"), "x").unwrap();
        std::fs::write(d.join("work/q3-plan.md"), "x").unwrap();
        std::fs::write(d.join("work/2026/kickoff.md"), "x").unwrap();
        d
    }

    #[test]
    fn dirs_first_case_insensitive_and_hidden_filtered() {
        let d = scratch("sort");
        let names: Vec<_> = list_dir(&d, false).into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, ["assets", "work", "Default.md", "helloworld.md"]);
        let all: Vec<_> = list_dir(&d, true).into_iter().map(|(n, _)| n).collect();
        assert!(all.contains(&".git".to_string()) && all.contains(&".hidden".to_string()));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn rows_read_only_expanded_folders() {
        let d = scratch("rows");
        let root = d.to_string_lossy().into_owned();
        let mut t = NotesTree::new(vec![root.clone()], BTreeSet::new(), false);
        assert_eq!(t.rows().len(), 1);
        t.toggle(&root);
        let rows = t.rows();
        assert_eq!(rows.len(), 5);
        assert!(rows[0].is_root && rows[0].expanded);
        assert!(rows[1..].iter().all(|r| r.depth == 1));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn reveal_expands_ancestors_and_selects() {
        let d = scratch("reveal");
        let root = d.to_string_lossy().into_owned();
        let mut t = NotesTree::new(vec![root.clone()], BTreeSet::new(), false);
        let file = format!("{root}/work/2026/kickoff.md");
        assert!(t.reveal(&file));
        assert!(t.expanded.contains(&root));
        assert!(t.expanded.contains(&format!("{root}/work")));
        assert!(t.expanded.contains(&format!("{root}/work/2026")));
        let rows = t.rows();
        let i = NotesTree::row_of(&rows, &file).expect("kickoff visible");
        assert_eq!(rows[i].depth, 3);
        assert_eq!(t.selected.as_deref(), Some(file.as_str()));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn outside_file_clears_selection() {
        let mut t = NotesTree::new(vec!["/a/notes".into()], BTreeSet::new(), false);
        t.selected = Some("/a/notes/x.md".into());
        assert!(!t.reveal("/b/other.md"));
        assert!(t.selected.is_none());
        // A sibling that merely shares a prefix is not inside the root.
        assert!(!t.reveal("/a/notes-old/x.md"));
    }

    #[test]
    fn split_roots_separates_dirs_from_files() {
        let (roots, files) = split_roots(
            "/n/notes, /n/a.md, /n/notes/, /missing",
            &|p| p == "/n/notes",
            &|p| p != "/missing",
        );
        assert_eq!(roots, ["/n/notes"]);
        assert_eq!(files, ["/n/a.md"]);
    }

    #[test]
    fn expanded_set_round_trips() {
        let s: BTreeSet<String> = ["/a".to_string(), "/b/c".to_string()].into();
        assert_eq!(decode_expanded(&encode_expanded(&s)), s);
    }

    /// The Notes sidebar rescan (bug: `.md` files created on disk never showed
    /// up): a rescan re-resolves `[notes] paths`, so a folder that did not
    /// exist when the view was built becomes a root and lists its notes.
    #[test]
    fn rescan_pins_and_lists_a_folder_created_after_build() {
        let d = scratch("rescan-root");
        let later = d.join("later");
        let raw = later.to_string_lossy().into_owned();
        let is_dir = |p: &str| Path::new(p).is_dir();
        let exists = |p: &str| Path::new(p).exists();
        // Built while the folder is still missing: no root, no tree.
        let (roots, _) = split_roots(&raw, &is_dir, &exists);
        let mut t = NotesTree::new(roots, BTreeSet::new(), false);
        assert!(t.is_empty());

        std::fs::create_dir_all(&later).unwrap();
        std::fs::write(later.join("on-disk.md"), "x").unwrap();
        assert!(t.rescan(&raw, &is_dir, &exists));
        assert_eq!(t.roots, [raw.clone()]);
        assert!(t.expanded.contains(&raw), "a freshly pinned root opens");
        let names: Vec<String> = t.rows().into_iter().filter(|r| !r.is_root).map(|r| r.name).collect();
        assert_eq!(names, ["on-disk.md"]);
        // Nothing new on the next rescan.
        assert!(!t.rescan(&raw, &is_dir, &exists));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A note written into an already-expanded root appears after a rescan,
    /// even though the root set did not change.
    #[test]
    fn rescan_lists_a_md_file_written_into_an_expanded_root() {
        let d = scratch("rescan-file");
        let root = d.to_string_lossy().into_owned();
        let mut t = NotesTree::new(vec![root.clone()], [root.clone()].into(), false);
        assert_eq!(t.rows().len(), 5);
        std::fs::write(d.join("note-new.md"), "x").unwrap();
        let is_dir = |p: &str| Path::new(p).is_dir();
        let exists = |p: &str| Path::new(p).exists();
        assert!(!t.rescan(&root, &is_dir, &exists));
        assert!(t.rows().iter().any(|r| r.name == "note-new.md"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
