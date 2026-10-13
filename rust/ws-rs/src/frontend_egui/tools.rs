//! The Tools panels (prettyprint / filefast / health-checks / text-table) and the Setup
//! view — the Wave 5 egui panes.
//!
//! Both drive the same pure models the AppKit half uses, mirroring `files.rs`'s
//! shape: pull the model's snapshot each frame, draw it with egui widgets,
//! collect the user's actions as events, and hand them to the model.
//! [`crate::views::tools::ToolsSnapshot`] / [`crate::views::tools::ToolsEvent`]
//! drive the tool panels; [`crate::views::setup::SetupSnapshot`] /
//! [`crate::views::setup::SetupEvent`] drive the Setup view.

use crate::app::config::AppSettings;
use crate::ui::theme::{PopupColors, PopupTone};
use crate::views::setup::{setup_row_layouts, SetupEvent, SetupRowKind, SetupWindow};
use crate::views::tools::{TableTarget, ToolKind, ToolPanel, ToolsEvent};

use super::app::Pane;
use super::theme::c;

/// The tool panels in tab order.
const KINDS: [ToolKind; 4] = [ToolKind::PrettyPrint, ToolKind::Filefast, ToolKind::HealthChecks, ToolKind::TextTable];

/// The Tools panel host (`views/tools.rs`): one tab per [`ToolKind`], each
/// rendering that panel's [`ToolPanel`] snapshot. The AppKit half opens one
/// floating `PopupPanel` per kind; the egui frontend keeps them as tabs in the
/// shared window (functional-first, no pixel parity).
pub struct ToolsPane {
    /// The panel models (the host's `tools` map).
    panels: [ToolPanel; 4],
    /// The visible tab (`openTool`'s per-kind window).
    selected: ToolKind,
    /// The prettyprint editor buffer — the editable text lives in the frontend
    /// and the model only mirrors it via [`ToolsEvent::Edit`] (the same split
    /// `files.rs` uses for its filter field).
    pp_text: String,
    /// The filefast name + paste buffers (`name_field` / `paste` NSTextViews).
    ff_name: String,
    ff_paste: String,
    /// The health run was kicked off once for this pane (`show_macos` calls
    /// `run_health`).
    health_started: bool,
    /// The text-table input buffer (the model mirrors it via `Edit`).
    tt_text: String,
}

impl ToolsPane {
    /// Build the three panel models (`DaemonUi::open_tool` builds each lazily;
    /// the pure half is identical).
    pub fn build() -> Self {
        let settings = AppSettings::default();
        ToolsPane {
            panels: KINDS.map(|k| ToolPanel::new(k, &settings)),
            selected: ToolKind::PrettyPrint,
            pp_text: String::new(),
            ff_name: String::new(),
            ff_paste: String::new(),
            health_started: false,
            tt_text: String::new(),
        }
    }

    fn index(kind: ToolKind) -> usize {
        KINDS.iter().position(|k| *k == kind).unwrap_or(0)
    }

    /// Draw one frame and apply the events it generated.
    pub fn ui(&mut self, ui: &mut egui::Ui, colors: &PopupColors) {
        // --- tab bar (the AppKit per-kind windows' titles) ---
        ui.horizontal(|ui| {
            for kind in KINDS {
                let label = egui::RichText::new(kind.name()).color(c(colors.text));
                if ui.selectable_label(self.selected == kind, label).clicked() {
                    self.selected = kind;
                }
            }
        });
        ui.separator();

        match self.selected {
            ToolKind::PrettyPrint => self.prettyprint(ui, colors),
            ToolKind::Filefast => self.filefast(ui, colors),
            ToolKind::HealthChecks => self.health(ui, colors),
            ToolKind::TextTable => self.text_table(ui, colors),
        }

        // Apply each panel's background work (`ToolPanel::poll`, the AppKit
        // 30 ms drain); a no-op off macOS where the model spawns no threads.
        for panel in &mut self.panels {
            panel.poll();
        }
    }

    /// The prettyprint body (`show_macos`'s editor, status line and the two
    /// header buttons; auto-format is an explicit button here).
    fn prettyprint(&mut self, ui: &mut egui::Ui, colors: &PopupColors) {
        let i = Self::index(ToolKind::PrettyPrint);
        let snap = self.panels[i].snapshot();

        // Follow `snap.text` unless the editor owns the keyboard (the rule
        // `files.rs` uses for its filter field).
        let id = ui.make_persistent_id("tools-pp");
        if !ui.memory(|m| m.has_focus(id)) && self.pp_text != snap.text {
            self.pp_text = snap.text.clone();
        }
        let mut events: Vec<ToolsEvent> = Vec::new();
        let resp = ui.add_sized(
            [ui.available_width(), (ui.available_height() - 40.0).max(80.0)],
            egui::TextEdit::multiline(&mut self.pp_text)
                .id(id)
                .code_editor()
                .hint_text("paste JSON or XML below — auto-formats"),
        );
        if resp.changed() {
            events.push(ToolsEvent::Edit(self.pp_text.clone()));
        }

        // --- button row (`pp_handler`: copy contents / save file) ---
        ui.horizontal(|ui| {
            // `prettyFormat` is the model's pure helper; the AppKit debounce
            // runs it on a background thread, the frontend runs it inline.
            if ui.button("format").clicked() {
                if let Ok(Some(formatted)) = ToolPanel::format_text(&self.pp_text) {
                    self.pp_text = formatted;
                    events.push(ToolsEvent::Edit(self.pp_text.clone()));
                }
            }
            if ui.button("save file").clicked() {
                events.push(ToolsEvent::Save);
            }
            if ui.button("copy contents").clicked() {
                let _ = crate::platform::set_text(&self.pp_text);
            }
            if let Some((msg, is_err)) = &snap.status {
                let tone = if *is_err { colors.tone(PopupTone::Danger) } else { colors.dim };
                ui.label(egui::RichText::new(msg).color(c(tone)).small());
            }
        });

        for ev in events {
            self.panels[i].handle(ev);
        }
    }

    /// The filefast body (`show_macos`'s name box + paste box, Return saves)
    /// and its indicator (`FileFastPasteView.refreshIndicator`).
    fn filefast(&mut self, ui: &mut egui::Ui, colors: &PopupColors) {
        let i = Self::index(ToolKind::Filefast);
        let snap = self.panels[i].snapshot();
        let mut events: Vec<ToolsEvent> = Vec::new();

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("name").color(c(colors.dim)));
            ui.add(egui::TextEdit::singleline(&mut self.ff_name).hint_text("file name"));
        });
        let hint_tone = if self.ff_paste.is_empty() { colors.dim } else { colors.tone(PopupTone::Success) };
        ui.label(egui::RichText::new(filefast_hint(&self.ff_paste)).color(c(hint_tone)).small());
        ui.add_sized(
            [ui.available_width(), (ui.available_height() - 60.0).max(60.0)],
            egui::TextEdit::multiline(&mut self.ff_paste).hint_text("paste output"),
        );
        ui.horizontal(|ui| {
            if ui.button("save").clicked() {
                events.push(ToolsEvent::FilefastSave);
            }
            if let Some((msg, is_err)) = &snap.status {
                let tone = if *is_err { colors.tone(PopupTone::Danger) } else { colors.dim };
                ui.label(egui::RichText::new(msg).color(c(tone)).small());
            }
        });

        for ev in events {
            self.panels[i].handle(ev);
        }
    }

    /// The health-checks body (`run_health` + `poll_macos`'s streaming buffer):
    /// kick the run once, then render it.
    fn health(&mut self, ui: &mut egui::Ui, colors: &PopupColors) {
        let i = Self::index(ToolKind::HealthChecks);
        let snap = self.panels[i].snapshot();
        if !self.health_started {
            self.health_started = true;
            self.panels[i].handle(ToolsEvent::HealthRun);
        }
        ui.horizontal(|ui| {
            if ui.button("re-run").clicked() {
                self.panels[i].handle(ToolsEvent::HealthRun);
            }
            if ui.button("copy output").clicked() {
                let _ = crate::platform::set_text(&snap.text);
            }
            if snap.running {
                ui.label(egui::RichText::new("running…").color(c(colors.dim)).small());
            }
        });
        let text = if snap.running && snap.text.is_empty() {
            "running health-checks…".to_string()
        } else {
            snap.text.clone()
        };
        egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
            ui.monospace(text);
        });
    }
}

impl ToolsPane {
    /// The text-table body: input, the two copy targets, the preview and the
    /// status line (`show_macos`'s split; the model debounces and runs).
    fn text_table(&mut self, ui: &mut egui::Ui, colors: &PopupColors) {
        let i = Self::index(ToolKind::TextTable);
        let snap = self.panels[i].snapshot();
        let mut events: Vec<ToolsEvent> = Vec::new();

        ui.horizontal(|ui| {
            if ui.button("from clipboard").clicked() {
                if let Some(clip) = crate::platform::get_text() {
                    self.tt_text = clip;
                    events.push(ToolsEvent::Edit(self.tt_text.clone()));
                }
            }
            if ui.button("Copy table for Outlook").clicked() {
                events.push(ToolsEvent::TableCopy(TableTarget::Outlook));
            }
            if ui.button("Copy Markdown file").clicked() {
                events.push(ToolsEvent::TableCopy(TableTarget::MarkdownFile));
            }
        });
        let half = ((ui.available_height() - 30.0) / 2.0).max(60.0);
        let resp = ui.add_sized(
            [ui.available_width(), half],
            egui::TextEdit::multiline(&mut self.tt_text)
                .code_editor()
                .hint_text("rows from OCR or paste — first row = header"),
        );
        if resp.changed() {
            events.push(ToolsEvent::Edit(self.tt_text.clone()));
        }
        egui::ScrollArea::both()
            .id_salt("tools-tt-preview")
            .max_height(half)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.monospace(&snap.preview);
            });
        if let Some((msg, is_err)) = &snap.status {
            let tone = if *is_err { colors.tone(PopupTone::Danger) } else { colors.dim };
            ui.label(egui::RichText::new(msg).color(c(tone)).small());
        }

        for ev in events {
            self.panels[i].handle(ev);
        }
    }
}

impl Pane for ToolsPane {
    fn ui(&mut self, ui: &mut egui::Ui, colors: &PopupColors) {
        ToolsPane::ui(self, ui, colors);
    }
}

/// `FileFastPasteView.refreshIndicator` — the paste box's status hint.
fn filefast_hint(pasted: &str) -> String {
    if pasted.is_empty() {
        return "paste output".to_string();
    }
    let lines = pasted.split('\n').count();
    format!(
        "************************  {lines} line{}, {} chars pasted",
        if lines == 1 { "" } else { "s" },
        pasted.chars().count()
    )
}

/// The Setup view (`views/setup.rs` `SetupWindow`): the preflight check list,
/// its summary line and the Check Again / Done buttons. The model is
/// host-driven — its `on_recheck`/`on_stack`/`on_fix` callbacks run the work —
/// so this pane renders the snapshot and reports the user's actions as
/// [`SetupEvent`]s (the host supplies the callbacks).
pub struct SetupPane {
    pub model: SetupWindow,
}

impl SetupPane {
    pub fn build() -> Self {
        SetupPane { model: SetupWindow::new() }
    }

    /// Draw one frame and apply the events it generated.
    pub fn ui(&mut self, ui: &mut egui::Ui, colors: &PopupColors) {
        let snap = self.model.snapshot();
        let mut events: Vec<SetupEvent> = Vec::new();

        // --- summary line (`say(_:_:)` from `summarize`) ---
        if let Some(sum) = &snap.summary {
            ui.label(egui::RichText::new(&sum.message).color(c(colors.tone(sum.tone))));
            ui.separator();
        }

        // --- checks list (`rebuildRows()`'s groups + rows, pure geometry) ---
        let (rows, _total) = setup_row_layouts(&snap.checks, (ui.available_width() as f64).max(1.0));
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for row in &rows {
                match &row.kind {
                    SetupRowKind::Group(_) => {
                        ui.label(egui::RichText::new(&row.title).color(c(colors.dim)).small());
                    }
                    SetupRowKind::Check(i) => {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(&row.glyph).color(c(colors.tone(row.glyph_tone))));
                            ui.label(egui::RichText::new(&row.title).color(c(colors.text)));
                            if let Some(label) = &row.fix_title {
                                if ui.button(label).clicked() {
                                    events.push(SetupEvent::Fix(*i));
                                }
                            }
                        });
                    }
                    SetupRowKind::FixHint(_) => {
                        ui.label(egui::RichText::new(&row.detail).color(c(colors.dim)).small());
                    }
                }
            }
        });
        ui.separator();

        // --- buttons (`updateButtons`) + the streamed log ---
        ui.horizontal(|ui| {
            if ui.add_enabled(!snap.busy, egui::Button::new("Check Again")).clicked() {
                events.push(SetupEvent::Recheck);
            }
            if ui.button("Done").clicked() {
                events.push(SetupEvent::Done);
            }
        });
        if !snap.log.is_empty() {
            egui::ScrollArea::vertical().max_height(120.0).auto_shrink([false, false]).show(ui, |ui| {
                ui.monospace(&snap.log);
            });
        }

        for ev in events {
            self.model.handle(ev);
        }
    }
}

impl Pane for SetupPane {
    fn ui(&mut self, ui: &mut egui::Ui, colors: &PopupColors) {
        SetupPane::ui(self, ui, colors);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filefast_hint_reports_lines_and_chars() {
        assert_eq!(filefast_hint(""), "paste output");
        assert_eq!(
            filefast_hint("a"),
            "************************  1 line, 1 chars pasted"
        );
        assert_eq!(
            filefast_hint("a\nb"),
            "************************  2 lines, 3 chars pasted"
        );
    }
}
