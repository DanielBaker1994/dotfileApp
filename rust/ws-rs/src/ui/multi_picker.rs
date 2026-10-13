//! `JiraMultiPicker` (JiraSearch.swift): a searchable multi-select popover —
//! type to filter, ↑↓ / Ctrl+N / Ctrl+P move, Return toggles, Esc clears the
//! filter then closes, Clear / Done, a footer count "N selected · M shown
//! (ORed)". Options may be grouped under headers ("N of M"), the unused tail
//! folds behind "Show N unused", an optional All row, and picks the options
//! no longer list stay as "not in the directory cache".
//!
//! [`MultiPickerModel`] is the pure half (tested); the AppKit popover and
//! the closed-state [`appkit::PickerChip`] are macOS only.

use std::collections::{BTreeSet, HashMap};

pub const ALL_ID: &str = "\u{0}all";
pub const GROUP_PREFIX: &str = "\u{0}group:";
pub const MORE_PREFIX: &str = "\u{0}more:";
/// `JiraMultiPicker.topN`.
pub const TOP_N: usize = 50;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PickOption {
    pub id: String,
    pub title: String,
    pub detail: String,
    pub group: String,
    pub unused: bool,
}

impl PickOption {
    pub fn new(id: &str, title: &str, detail: &str) -> Self {
        PickOption { id: id.into(), title: title.into(), detail: detail.into(), ..Default::default() }
    }

    /// The All row, a group header or a fold row (not a pickable value).
    pub fn is_synthetic(&self) -> bool {
        self.id == ALL_ID || self.id.starts_with(GROUP_PREFIX) || self.id.starts_with(MORE_PREFIX)
    }
}

/// `foldTail`: past `TOP_N` (+5 slack) the tail is marked unused.
pub fn fold_tail(opts: Vec<PickOption>) -> Vec<PickOption> {
    if opts.len() <= TOP_N + 5 {
        return opts;
    }
    opts.into_iter().enumerate().map(|(i, o)| PickOption { unused: i >= TOP_N, ..o }).collect()
}

#[derive(Clone, Debug, Default)]
pub struct MultiPickerModel {
    pub options: Vec<PickOption>,
    pub selected: Vec<String>,
    pub is_all: bool,
    pub all_title: Option<String>,
    pub noun: String,
    pub placeholder: String,
    pub group_order: Vec<String>,
    pub group_detail: HashMap<String, String>,
    /// The fold row's title; `{n}` is the hidden count (`foldTitle`).
    pub fold_title: Option<String>,
    pub expanded: BTreeSet<String>,
    pub query: String,
    pub shown: Vec<PickOption>,
    pub highlight: usize,
}

impl MultiPickerModel {
    pub fn new(noun: &str, all_title: Option<&str>) -> Self {
        MultiPickerModel {
            noun: noun.into(),
            all_title: all_title.map(str::to_string),
            placeholder: "Choose…".into(),
            ..Default::default()
        }
    }

    pub fn set_options(&mut self, opts: Vec<PickOption>) {
        self.options = opts;
        let keep = self.selected.clone();
        let all = self.is_all;
        self.set(keep, all);
    }

    /// `set(_:all:)` — picks missing from the options stay listed.
    pub fn set(&mut self, ids: Vec<String>, all: bool) {
        self.selected = if all { Vec::new() } else { ids };
        self.is_all = all && self.all_title.is_some();
        let missing: Vec<String> =
            self.selected.iter().filter(|id| !self.options.iter().any(|o| &o.id == *id)).cloned().collect();
        for id in missing {
            self.options.push(PickOption::new(&id, &id, "not in the directory cache"));
        }
        self.refilter();
    }

    pub fn title_of(&self, id: &str) -> String {
        self.options.iter().find(|o| o.id == id).map(|o| o.title.clone()).unwrap_or_else(|| id.to_string())
    }

    /// What the closed chip shows: the All title, else the picked titles.
    pub fn display_items(&self) -> Vec<String> {
        if self.is_all {
            return vec![self.all_title.clone().unwrap_or_default()];
        }
        self.selected.iter().map(|id| self.title_of(id)).collect()
    }

    pub fn tooltip(&self) -> String {
        if self.is_all {
            return self.all_title.clone().unwrap_or_default();
        }
        if self.selected.is_empty() {
            return format!("Choose {}s (type to filter)", self.noun);
        }
        self.display_items().join(", ")
    }

    fn all_row(&self) -> Option<PickOption> {
        self.all_title.as_ref().map(|a| PickOption::new(ALL_ID, a, ""))
    }

    pub fn refilter(&mut self) {
        let q: Vec<String> = self.query.to_lowercase().split_whitespace().map(str::to_string).collect();
        let mut list: Vec<PickOption>;
        if q.is_empty() && self.options.iter().any(|o| !o.group.is_empty()) {
            list = self.grouped_rows();
            if let Some(a) = self.all_row() {
                list.insert(0, a);
            }
        } else if q.is_empty() {
            list = self.selected.iter().filter_map(|id| self.options.iter().find(|o| &o.id == id).cloned()).collect();
            list.extend(self.options.iter().filter(|o| !self.selected.contains(&o.id)).cloned());
            if let Some(a) = self.all_row() {
                list.insert(0, a);
            }
        } else {
            list = self
                .options
                .iter()
                .filter(|o| {
                    let hay = format!("{} {} {} {}", o.title, o.detail, o.id, o.group).to_lowercase();
                    q.iter().all(|w| hay.contains(w.as_str()))
                })
                .cloned()
                .collect();
        }
        self.shown = list;
        self.highlight = self.highlight.min(self.shown.len().saturating_sub(1));
    }

    fn grouped_rows(&self) -> Vec<PickOption> {
        let mut order: Vec<String> =
            self.group_order.iter().filter(|g| self.options.iter().any(|o| &o.group == *g)).cloned().collect();
        for o in &self.options {
            if !order.contains(&o.group) {
                order.push(o.group.clone());
            }
        }
        let mut out = Vec::new();
        for g in order {
            let mine: Vec<&PickOption> = self.options.iter().filter(|o| o.group == g).collect();
            if !g.is_empty() {
                let used = mine.iter().filter(|o| !o.unused).count();
                let detail = self.group_detail.get(&g).cloned().unwrap_or_else(|| {
                    if used == mine.len() {
                        mine.len().to_string()
                    } else {
                        format!("{used} of {}", mine.len())
                    }
                });
                out.push(PickOption { id: format!("{GROUP_PREFIX}{g}"), title: g.clone(), detail, ..Default::default() });
            }
            let open = self.expanded.contains(&g);
            out.extend(mine.iter().filter(|o| open || !o.unused || self.selected.contains(&o.id)).map(|o| (*o).clone()));
            let hidden: Vec<&&PickOption> = mine.iter().filter(|o| o.unused && !self.selected.contains(&o.id)).collect();
            if !open && !hidden.is_empty() {
                let mut names = hidden.iter().take(3).map(|o| o.title.clone()).collect::<Vec<_>>().join(", ");
                if hidden.len() > 3 {
                    names.push('…');
                }
                let title = self
                    .fold_title
                    .as_deref()
                    .map(|t| t.replace("{n}", &hidden.len().to_string()))
                    .unwrap_or_else(|| format!("Show {} unused", hidden.len()));
                out.push(PickOption { id: format!("{MORE_PREFIX}{g}"), title, detail: names, ..Default::default() });
            }
        }
        out
    }

    fn group_ids(&self, g: &str) -> Vec<String> {
        self.options.iter().filter(|o| o.group == g).map(|o| o.id.clone()).collect()
    }

    pub fn is_on(&self, o: &PickOption) -> bool {
        if let Some(g) = o.id.strip_prefix(GROUP_PREFIX) {
            let ids = self.group_ids(g);
            return !ids.is_empty() && ids.iter().all(|i| self.selected.contains(i));
        }
        if o.id == ALL_ID {
            return self.is_all;
        }
        self.selected.contains(&o.id)
    }

    /// Toggle `shown[row]`; `true` when the selection changed (a fold row
    /// only expands its group).
    pub fn toggle(&mut self, row: usize) -> bool {
        let Some(o) = self.shown.get(row).cloned() else { return false };
        if let Some(g) = o.id.strip_prefix(MORE_PREFIX) {
            self.expanded.insert(g.to_string());
            self.refilter();
            self.highlight = row.min(self.shown.len().saturating_sub(1));
            return false;
        }
        if let Some(g) = o.id.strip_prefix(GROUP_PREFIX) {
            let ids = self.group_ids(g);
            if self.is_on(&o) {
                self.selected.retain(|s| !ids.contains(s));
            } else {
                for id in &ids {
                    if !self.selected.contains(id) {
                        self.selected.push(id.clone());
                    }
                }
            }
            if !ids.is_empty() {
                self.is_all = false;
            }
            self.refilter();
            self.highlight = row;
            return true;
        }
        if o.id == ALL_ID {
            self.is_all = !self.is_all;
            if self.is_all {
                self.selected.clear();
            }
        } else if let Some(i) = self.selected.iter().position(|s| *s == o.id) {
            self.selected.remove(i);
        } else {
            self.selected.push(o.id.clone());
            self.is_all = false;
        }
        self.highlight = row;
        true
    }

    pub fn clear(&mut self) {
        self.selected.clear();
        self.is_all = false;
        self.refilter();
    }

    pub fn move_highlight(&mut self, d: i64) {
        if self.shown.is_empty() {
            return;
        }
        self.highlight = (self.highlight as i64 + d).clamp(0, self.shown.len() as i64 - 1) as usize;
    }

    /// `updateCount`.
    pub fn count_text(&self) -> String {
        if self.is_all {
            return self.all_title.clone().unwrap_or_default();
        }
        let shown = self.shown.iter().filter(|o| !o.is_synthetic()).count();
        let ored = if self.selected.len() > 1 { " (ORed)" } else { "" };
        format!("{} selected · {shown} shown{ored}", self.selected.len())
    }
}

#[cfg(target_os = "macos")]
pub mod appkit;

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(names: &[&str]) -> Vec<PickOption> {
        names.iter().map(|n| PickOption::new(n, n, "")).collect()
    }

    #[test]
    fn picked_first_then_filter_and_toggle() {
        let mut m = MultiPickerModel::new("user", None);
        m.set_options(opts(&["ann", "bob", "cy"]));
        m.set(vec!["cy".into()], false);
        let ids: Vec<&str> = m.shown.iter().map(|o| o.id.as_str()).collect();
        assert_eq!(ids, vec!["cy", "ann", "bob"]);
        m.query = "b".into();
        m.refilter();
        assert_eq!(m.shown.len(), 1);
        assert!(m.toggle(0));
        assert_eq!(m.selected, vec!["cy", "bob"]);
        assert_eq!(m.count_text(), "2 selected · 1 shown (ORed)");
        m.clear();
        assert!(m.selected.is_empty());
    }

    #[test]
    fn all_row_and_stale_picks() {
        let mut m = MultiPickerModel::new("project", Some("All projects"));
        m.set_options(opts(&["A", "B"]));
        assert_eq!(m.shown[0].id, ALL_ID);
        assert!(m.toggle(0));
        assert!(m.is_all && m.selected.is_empty());
        assert_eq!(m.display_items(), vec!["All projects"]);
        assert!(m.toggle(1));
        assert!(!m.is_all);
        m.set(vec!["GONE".into()], false);
        let gone = m.options.iter().find(|o| o.id == "GONE").unwrap();
        assert_eq!(gone.detail, "not in the directory cache");
    }

    #[test]
    fn groups_fold_and_group_toggle() {
        let mut m = MultiPickerModel::new("status", None);
        let mut o = vec![
            PickOption { id: "Open".into(), title: "Open".into(), group: "To Do".into(), ..Default::default() },
            PickOption { id: "Doing".into(), title: "Doing".into(), group: "In Progress".into(), ..Default::default() },
            PickOption { id: "Old".into(), title: "Old".into(), group: "In Progress".into(), unused: true, ..Default::default() },
        ];
        m.group_order = vec!["To Do".into(), "In Progress".into()];
        m.set_options(std::mem::take(&mut o));
        let ids: Vec<&str> = m.shown.iter().map(|o| o.id.as_str()).collect();
        assert_eq!(ids, vec!["\u{0}group:To Do", "Open", "\u{0}group:In Progress", "Doing", "\u{0}more:In Progress"]);
        assert_eq!(m.shown[2].detail, "1 of 2");
        // The fold row expands without changing the picks.
        assert!(!m.toggle(4));
        assert!(m.shown.iter().any(|o| o.id == "Old"));
        // A header picks every value of its group.
        assert!(m.toggle(2));
        assert_eq!(m.selected, vec!["Doing", "Old"]);
        assert!(m.is_on(&m.shown[2].clone()));
    }

    #[test]
    fn fold_tail_marks_past_top_n() {
        let many: Vec<PickOption> = (0..60).map(|i| PickOption::new(&i.to_string(), &i.to_string(), "")).collect();
        let f = fold_tail(many);
        assert!(!f[49].unused && f[50].unused);
        assert_eq!(fold_tail(opts(&["a", "b"])).iter().filter(|o| o.unused).count(), 0);
    }

    #[test]
    fn nul_prefixed_values_still_count_as_shown() {
        let mut m = MultiPickerModel::new("assignee", None);
        m.set_options(vec![PickOption::new("ann", "ann", ""), PickOption::new("\u{0}none", "(unassigned)", "")]);
        assert_eq!(m.count_text(), "0 selected · 2 shown");
    }
}
