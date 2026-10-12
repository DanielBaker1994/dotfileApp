//! The Compare view — the egui port of the AppKit `CompareSurface`.
//!
//! It drives the same pure [`CompareWindowModel`] the AppKit half drives: pull
//! a [`CompareSnapshot`] each frame, draw the two-column diff rows (or, for a
//! folder session, the folder compare list) with egui widgets, collect the
//! user's actions as [`CompareEvent`]s, and hand them to
//! [`CompareWindowModel::handle`] — the same event contract the AppKit surface
//! reports. Each section is commented with the `compare_view.rs` /
//! `compare.rs` region it mirrors.

use crate::ui::theme::{PopupColors, PopupTone, Rgba};
use crate::views::compare::{
    folder_row_color, folder_size_label, pane_cell, CompareConfig, CompareFilter, CompareRecent,
    CompareRow, CompareSide, CompareWindowModel, FolderColor, FolderTreeRow, PaneCell, PaneModel,
};
use crate::views::compare_view::{CompareEvent, CompareSnapshot, FILTERS, TOOLS};

use super::app::Pane;
use super::theme::c;

pub struct ComparePane {
    pub model: CompareWindowModel,
    /// The start page's Left / Right path fields (`compare_view.rs`
    /// `search_box`); egui owns the buffers and the Compare button reads them.
    left: String,
    right: String,
}

impl ComparePane {
    /// Build the model like `DaemonUi`'s compare build (the pure half: the
    /// `[compare]` config section + the recent cache; no backend call yet).
    pub fn build() -> Self {
        let text = crate::app::config::read_config_text().unwrap_or_default();
        let section = crate::views::tools::section_vars(&text, "compare");
        let model = CompareWindowModel::new(
            CompareConfig::from_entries(section),
            CompareRecent::from_env(),
        );
        ComparePane { model, left: String::new(), right: String::new() }
    }

    /// Draw one frame and apply the events it generated. `colors` are the
    /// `[compare]` section tokens (from [`crate::ui::theme::section_colors`]).
    pub fn ui(&mut self, ui: &mut egui::Ui, colors: &PopupColors) {
        let snap = self.model.snapshot();
        let mut events: Vec<CompareEvent> = Vec::new();

        if snap.on_start {
            self.start_page(ui, colors, &mut events);
        } else if snap.folder {
            self.folder_list(ui, colors);
        } else {
            self.toolbar(ui, &snap, colors, &mut events);
            ui.separator();
            self.diff_rows(ui, &snap, colors);
        }
        // --- status line (`compare_view.rs` `draw`, bottom `snap.status`) ---
        ui.separator();
        ui.label(egui::RichText::new(snap.status.as_str()).color(c(colors.dim)).small());

        for ev in events {
            self.model.handle(ev);
        }
    }

    /// The start page (`compare_view.rs` `draw` `on_start`: the Left / Right
    /// path boxes and the Compare ⏎ | New Text Compare capsule). The RECENT
    /// list and the session sidebar are the macOS half's (they need the
    /// `CompareSnap` chrome state); the egui start page keeps the two path
    /// fields and the two open actions.
    fn start_page(
        &mut self,
        ui: &mut egui::Ui,
        colors: &PopupColors,
        events: &mut Vec<CompareEvent>,
    ) {
        ui.heading("Compare");
        for (label, buf, hint) in [
            ("Left", &mut self.left, "Left: a file or folder path"),
            ("Right", &mut self.right, "Right: a file or folder path"),
        ] {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(label).color(c(colors.dim)));
                ui.add(egui::TextEdit::singleline(buf).hint_text(hint));
            });
        }
        ui.horizontal(|ui| {
            if ui.button("Compare  ⏎").clicked() {
                events.push(CompareEvent::Compare {
                    left: self.left.clone(),
                    right: self.right.clone(),
                });
            }
            if ui.button("New Text Compare").clicked() {
                events.push(CompareEvent::NewText);
            }
        });
    }

    /// The text toolbar (`compare_view.rs` `draw` toolbar: the filter segments
    /// and the summary, then the tool buttons).
    fn toolbar(
        &self,
        ui: &mut egui::Ui,
        snap: &CompareSnapshot,
        colors: &PopupColors,
        events: &mut Vec<CompareEvent>,
    ) {
        ui.horizontal(|ui| {
            let cur = CompareFilter::ALL.iter().position(|f| *f == snap.filter).unwrap_or(0);
            for (i, label) in FILTERS.iter().enumerate() {
                if ui.selectable_label(cur == i, *label).clicked() {
                    events.push(CompareEvent::Filter(i));
                }
            }
            ui.separator();
            let color = if snap.pane.identical { c(colors.tone(PopupTone::Success)) } else { c(colors.text) };
            ui.label(egui::RichText::new(snap.summary.as_str()).color(color));
            // The four pure tool actions `handle` dispatches (`TOOLS[0..4]`);
            // importance / find are the macOS half's.
            const GLYPHS: [&str; 4] = ["▲", "▼", "⇄", "⟳"];
            for (i, glyph) in GLYPHS.iter().enumerate() {
                if ui.button(*glyph).on_hover_text(TOOLS[i].1).clicked() {
                    events.push(CompareEvent::Tool(i));
                }
            }
        });
    }

    /// The two-column diff body (`compare.rs` `ComparePaneView.render` /
    /// `draw_row`): the left / right path headers, then one row per display
    /// row with both sides' line number + text.
    fn diff_rows(&self, ui: &mut egui::Ui, snap: &CompareSnapshot, colors: &PopupColors) {
        let pane = &snap.pane;
        // Path headers (`CompareHeaderLabel` per side).
        ui.horizontal(|ui| {
            let lc = if pane.focus == CompareSide::Left { colors.text } else { colors.dim };
            let rc = if pane.focus == CompareSide::Right { colors.text } else { colors.dim };
            ui.monospace(egui::RichText::new(pane.left_header.as_str()).color(c(lc)));
            ui.separator();
            ui.monospace(egui::RichText::new(pane.right_header.as_str()).color(c(rc)));
        });
        ui.separator();
        if pane.display_count() == 0 {
            let msg = if pane.rows.is_empty() { "(no differences)" } else { "(filtered)" };
            ui.label(egui::RichText::new(msg).color(c(colors.dim)));
            return;
        }
        let num_width = format!("{}", pane.max_lines()).len();
        egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
            for d in 0..pane.display_count() {
                let m = pane.model_row(d);
                let Some(row) = pane.rows.get(m) else { continue };
                let fill = row_fill(row, d == pane.cursor, pane.ignore_unimportant, colors);
                egui::Frame::NONE.fill(fill).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.monospace(side_cell(pane, CompareSide::Left, row, num_width));
                        ui.separator();
                        ui.monospace(side_cell(pane, CompareSide::Right, row, num_width));
                    });
                });
            }
        });
    }

    /// The folder compare list (`compare.rs` `FolderTreeView.render`). The
    /// W3-F [`CompareSnapshot`] carries no folder rows (its diff pane is empty
    /// for a folder session), so this reads the model's bound `folder_page`
    /// session directly — cached node reads only, no helper call.
    fn folder_list(&self, ui: &mut egui::Ui, colors: &PopupColors) {
        let Some(fs) = self.model.folder_page.session.as_ref() else {
            ui.label(egui::RichText::new("(no folder session)").color(c(colors.dim)));
            return;
        };
        let rows: Vec<FolderTreeRow> = match fs.tree.as_ref() {
            Some(tree) => fs
                .rows
                .iter()
                .filter_map(|r| {
                    let n = tree.node(r.id)?;
                    Some(FolderTreeRow {
                        id: n.id,
                        depth: r.depth,
                        name: n.name.clone(),
                        rel: n.rel.clone(),
                        is_dir: n.is_dir(),
                        kind_mismatch: n.kind_mismatch(),
                        status: n.status,
                        newer: n.newer,
                        left: n.left.clone(),
                        right: n.right.clone(),
                    })
                })
                .collect(),
            None => Vec::new(),
        };
        ui.label(
            egui::RichText::new(format!("{} items", rows.len())).color(c(colors.dim)).small(),
        );
        if rows.is_empty() {
            ui.label(egui::RichText::new("(empty)").color(c(colors.dim)));
            return;
        }
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for (i, row) in rows.iter().enumerate() {
                let color = folder_token(colors, folder_row_color(row.status, row.newer, row.is_dir, fs.focus));
                let indent = " ".repeat(row.depth * 2);
                let size = row
                    .left
                    .as_ref()
                    .filter(|l| !l.is_dir)
                    .map(|l| format!("  {}", folder_size_label(l.size)))
                    .unwrap_or_default();
                let label = format!("{indent}{} {}{size}", row.glyph(), row.display_name(fs.view.flatten));
                let _ = ui.selectable_label(
                    i == fs.cursor,
                    egui::RichText::new(label).color(c(color)).monospace(),
                );
            }
        });
    }
}

/// The row background (`ComparePaneView.drawRow`'s per-cell fills collapsed to
/// one row tint): the cursor highlight wins, else the important / unimportant
/// diff tint, else no fill.
fn row_fill(row: &CompareRow, cursor: bool, ignore_unimportant: bool, colors: &PopupColors) -> egui::Color32 {
    if cursor {
        return c(colors.highlight.with_alpha(0.55));
    }
    let tint = |cell| match cell {
        PaneCell::DiffImportant => Some(colors.tone(PopupTone::Danger).with_alpha(0.15)),
        PaneCell::DiffUnimportant => Some(colors.tone(PopupTone::Info).with_alpha(0.13)),
        _ => None,
    };
    let cell = pane_cell(row, CompareSide::Left, ignore_unimportant);
    let tint = tint(cell).or_else(|| tint(pane_cell(row, CompareSide::Right, ignore_unimportant)));
    tint.map(c).unwrap_or(egui::Color32::TRANSPARENT)
}

/// One side's row text (`ComparePaneView.drawRow`): the 1-based line number
/// (right-aligned to `num_width`) and the line's text, or blanks for a side
/// with no line. Pure so the row formatting is unit-tested.
fn side_cell(pane: &PaneModel, side: CompareSide, row: &CompareRow, num_width: usize) -> String {
    let line = row.line(side);
    if line < 0 {
        return " ".repeat(num_width + 1);
    }
    let num = format!("{:>width$}", line + 1, width = num_width);
    format!("{num} {}", pane.line_text(side, line).unwrap_or(""))
}

/// The theme token for a folder row (mirror of `compare.rs`
/// `macos::folder_color`).
fn folder_token(colors: &PopupColors, token: FolderColor) -> Rgba {
    match token {
        FolderColor::Text => colors.text,
        FolderColor::Dim => colors.dim,
        FolderColor::Danger => colors.tone(PopupTone::Danger),
        FolderColor::Info => colors.tone(PopupTone::Info),
        FolderColor::Accent2 => colors.tone(PopupTone::Accent2),
        FolderColor::Warning => colors.tone(PopupTone::Warning),
    }
}

impl Pane for ComparePane {
    fn ui(&mut self, ui: &mut egui::Ui, colors: &PopupColors) {
        ComparePane::ui(self, ui, colors);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::compare::RowKind;

    fn pane_of(left: &[&str], right: &[&str], rows: Vec<CompareRow>) -> PaneModel {
        let mut p = PaneModel::empty();
        p.left_lines = left.iter().map(|s| s.to_string()).collect();
        p.right_lines = right.iter().map(|s| s.to_string()).collect();
        p.rows = rows;
        p
    }

    fn row(l: i32, r: i32, kind: RowKind) -> CompareRow {
        CompareRow { l, r, kind, important: kind != RowKind::Same }
    }

    #[test]
    fn side_cell_formats_line_number_and_text() {
        let pane = pane_of(&["a", "b"], &["a", "c"], vec![row(0, 0, RowKind::Same)]);
        let r = pane.rows[0];
        assert_eq!(side_cell(&pane, CompareSide::Left, &r, 2), " 1 a");
        assert_eq!(side_cell(&pane, CompareSide::Right, &r, 2), " 1 a");
    }

    #[test]
    fn side_cell_blanks_a_missing_line() {
        let pane = pane_of(&[], &["x"], vec![row(-1, 0, RowKind::RightOnly)]);
        let r = pane.rows[0];
        assert_eq!(side_cell(&pane, CompareSide::Left, &r, 2), "   ");
        assert_eq!(side_cell(&pane, CompareSide::Right, &r, 2), " 1 x");
    }
}
