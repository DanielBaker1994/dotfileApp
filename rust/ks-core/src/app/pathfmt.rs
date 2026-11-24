//! Small path / config helpers shared by the views (moved out of the removed
//! notes view): `~` spelling for display and a terminal selection as a path.

/// `~` → `$HOME`, `~/x` → `$HOME/x`; anything else (including `~user`) unchanged.
pub fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix('~') {
        if rest.is_empty() || rest.starts_with('/') {
            let home = std::env::var("HOME").unwrap_or_default();
            return format!("{home}{rest}");
        }
    }
    path.to_string()
}

/// `/Users/me/x` → `~/x` for display.
pub fn tilde_path(path: &str, home: &str) -> String {
    if !home.is_empty() && path.starts_with(&format!("{home}/")) {
        format!("~{}", &path[home.len()..])
    } else {
        path.to_string()
    }
}

/// The drawer terminal's selected text as an existing absolute path: one
/// line, quotes / a trailing `:line[:col]` stripped, `~` expanded, relative
/// paths taken from `home` (where the drawer shell starts).
pub fn terminal_path(sel: &str, home: &str, exists: &dyn Fn(&str) -> bool) -> Option<String> {
    let line = sel.lines().map(str::trim).find(|l| !l.is_empty())?;
    let t = line.trim_matches(|c| matches!(c, '"' | '\'' | '`'));
    let t = t.replace("\\ ", " ");
    let abs = |t: &str| -> String {
        if let Some(rest) = t.strip_prefix("~/") {
            format!("{home}/{rest}")
        } else if t == "~" {
            home.to_string()
        } else if t.starts_with('/') {
            t.to_string()
        } else {
            format!("{home}/{}", t.trim_start_matches("./"))
        }
    };
    let p = abs(&t);
    if exists(&p) {
        return Some(p);
    }
    // `file.rs:12` / `file.rs:12:5` (grep / compiler output).
    let mut base = t.as_str();
    for _ in 0..2 {
        match base.rsplit_once(':') {
            Some((head, tail)) if !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()) => base = head,
            _ => break,
        }
    }
    let p = abs(base);
    exists(&p).then_some(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tilde_spelling() {
        assert_eq!(tilde_path("/h/a/b", "/h"), "~/a/b");
        assert_eq!(tilde_path("/hx/a", "/h"), "/hx/a");
        assert_eq!(tilde_path("/h/a", ""), "/h/a");
    }

    #[test]
    fn terminal_selection_resolves_to_a_path() {
        let ex = |p: &str| ["/h/docs/a.md", "/h/src/main.rs", "/tmp/x y.txt", "/h"].contains(&p);
        assert_eq!(terminal_path("~/docs/a.md\n", "/h", &ex).as_deref(), Some("/h/docs/a.md"));
        assert_eq!(terminal_path("docs/a.md", "/h", &ex).as_deref(), Some("/h/docs/a.md"));
        assert_eq!(terminal_path("'/tmp/x y.txt'", "/h", &ex).as_deref(), Some("/tmp/x y.txt"));
        assert_eq!(terminal_path("/tmp/x\\ y.txt", "/h", &ex).as_deref(), Some("/tmp/x y.txt"));
        assert_eq!(terminal_path("src/main.rs:12:5", "/h", &ex).as_deref(), Some("/h/src/main.rs"));
        assert_eq!(terminal_path("no such thing", "/h", &ex), None);
        assert_eq!(terminal_path("   ", "/h", &ex), None);
    }
}
