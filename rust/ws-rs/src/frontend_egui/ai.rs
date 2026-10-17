//! The AI view — the egui port (Wave 5, card W5-E).
//!
//! Same shape as [`super::files`] (THE reference view): it drives the pure
//! [`crate::views::ai::AIWindowModel`] the AppKit
//! [`crate::views::ai_view::AIView`] drives. Pull an
//! [`AISnapshot`] each frame, draw it with egui widgets, collect the user's
//! actions as [`AIEvent`]s, and hand them to [`AIWindowModel::handle`] — the
//! exact event contract the AppKit surface reports. The Markdown "answer" is
//! rendered with `egui_commonmark` (the Linux stand-in for the AppKit output
//! `NSTextView` / web preview). Each section below is commented with the
//! `ai_view.rs` region it mirrors.
//!
//! Deliberately left to the frontend (the model's `snapshot`/`handle` do not
//! carry them): the rich "Copy for …" pasteboard write (only the Markdown copy
//! is wired), the rule sheet / context menu (`ai_new_rule` / `ai_rule_menu`),
//! the Outlook/Webex web preview (`show_preview` is AppKit `WKWebView`), and
//! the `NSUserDefaults` per-rule input restore (an AppKit prefs store).

use crate::ui::popup::KeyAction;
use crate::ui::sidebar::SidebarEvent;
use crate::ui::theme::{PopupColors, PopupTone, Rgba};
use crate::views::ai::{AIWindowModel, AITone, CopyPlan};
use crate::views::ai_view::{AIEvent, AISnapshot, OutRun, StepState};

use super::app::Pane;
use super::theme::c;
use super::widgets;

/// The AI pane. Holds the model plus the two pieces of view state `ai_view.rs`
/// keeps in `AIIvars`/`AIState`: the editable "Your text" buffer and the
/// Markdown render cache.
pub struct AiPane {
    pub model: AIWindowModel,
    /// The "Your text" input (`ai_view.rs` `input` NSTextView). The snapshot
    /// only carries [`AISnapshot::set_input`], so the pane owns the buffer — the
    /// same split [`super::files::FilesPane`] uses for its filter field.
    input: String,
    /// `egui_commonmark`'s parse cache for the rendered answer (the AppKit
    /// `NSTextView`'s attributed string, kept across frames).
    md: egui_commonmark::CommonMarkCache,
    /// The sidebar collapse-to-rail state (`SidebarView`'s rail; persisted
    /// as `sidebarRail.ai`, toggled by Ctrl+B B / `do:rail:ai`).
    rail: bool,
}

impl AiPane {
    /// Build the model like `DaemonUi::build_ai` (the pure half; the `WKWebView`
    /// preview and the `NSUserDefaults` restores are the AppKit-only half and
    /// are skipped here).
    pub fn build() -> Self {
        let cfg = crate::views::ai::AIConfig::load();
        let mut model = AIWindowModel::new(cfg);
        let dir = model.rules_dir();
        let _ = std::fs::create_dir_all(&dir);
        model.reload_rules(&dir);
        model.render_answer();
        let input = model.input.clone();
        AiPane { model, input, md: Default::default(), rail: crate::ui::sidebar::rail_default("ai") }
    }

    /// Draw one frame and apply the events it generated. `colors` are the
    /// `[ai]` section tokens (from [`crate::ui::theme::section_colors`]).
    pub fn ui(&mut self, ui: &mut egui::Ui, colors: &PopupColors) {
        let snap = self.model.snapshot();
        let mut events: Vec<AIEvent> = Vec::new();

        // The model's input sync (`AIWindow.apply`: `set_input` after a rule
        // switch / restore) — `None` leaves the field alone.
        if let Some(t) = snap.set_input.clone() {
            self.input = t;
        }

        // --- RULES sidebar | right column (`ai_view.rs` `layout_all` /
        // `AIWindow.navPanes`: sidebar, input, answer) ---
        ui.horizontal_top(|ui| {
            if self.rail {
                ui.allocate_ui_with_layout(
                    egui::vec2(widgets::RAIL_W, ui.available_height()),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        if widgets::rail_button(ui, true, colors) {
                            self.set_rail(false);
                        }
                    },
                );
            } else {
                ui.allocate_ui_with_layout(
                    egui::vec2(widgets::SIDEBAR_W, ui.available_height()),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        self.rules_sidebar(ui, &snap, colors, &mut events);
                        if widgets::rail_button(ui, false, colors) {
                            self.set_rail(true);
                        }
                    },
                );
            }
            ui.separator();
            ui.vertical(|ui| {
                // --- AICommandLine (`ai_view.rs` `draw_command`) ---
                self.command_line(ui, &snap, colors, &mut events);
                // --- AIPipelineStrip (`ai_view.rs` `draw_strip`) ---
                self.pipeline_strip(ui, &snap, colors);
                if !snap.warning.is_empty() {
                    ui.label(
                        egui::RichText::new(snap.warning.as_str())
                            .color(c(colors.tone(PopupTone::Warning)))
                            .small(),
                    );
                }
                ui.separator();
                // --- "Your text" | "Changes/Answer" split (`ai_view.rs`
                // `layoutPanes` / `draw_split`) ---
                ui.columns(2, |cols| {
                    self.input_pane(&mut cols[0], &snap, colors, &mut events);
                    self.output_pane(&mut cols[1], &snap, colors);
                });
                ui.separator();
                // --- footer (Run ⌃↩ · status · tokens · Copy for …) ---
                self.footer(ui, &snap, colors, &mut events);
            });
        });

        for ev in events {
            self.apply(ui.ctx(), ev);
        }
    }

    /// The left RULES column (`ai_view.rs` `SidebarView` fed by `apply`).
    fn rules_sidebar(
        &self,
        ui: &mut egui::Ui,
        snap: &AISnapshot,
        colors: &PopupColors,
        events: &mut Vec<AIEvent>,
    ) {
        let m = &snap.sidebar;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            if !m.section_title.is_empty() {
                widgets::section_label(ui, &m.section_title, colors);
            }
            for (i, row) in m.rows.iter().enumerate() {
                let label = egui::RichText::new(row.title.as_str()).color(c(colors.text));
                let mut resp = ui.selectable_label(m.selected == Some(i), label);
                if !row.tip.is_empty() {
                    resp = resp.on_hover_text(row.tip.clone());
                }
                if resp.clicked() {
                    events.push(AIEvent::Sidebar(SidebarEvent::Select(i)));
                }
            }
            if m.add_button && ui.button("+").on_hover_text("New rule").clicked() {
                events.push(AIEvent::Sidebar(SidebarEvent::Add));
            }
            if let Some((text, tone)) = &m.status {
                ui.label(
                    egui::RichText::new(text.as_str())
                        .color(c(colors.tone(*tone)))
                        .small(),
                );
            }
        });
    }

    /// The `$ fm respond …` command line (`ai_view.rs` `draw_command`). Clicking
    /// the command copies it with the input (`host::ai_event` `CopyCommand`).
    fn command_line(
        &self,
        ui: &mut egui::Ui,
        snap: &AISnapshot,
        colors: &PopupColors,
        events: &mut Vec<AIEvent>,
    ) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("$").color(c(colors.dim)).monospace());
            if snap.command.is_empty() {
                ui.label(egui::RichText::new("no rule").color(c(colors.dim)).monospace());
            } else if ui
                .button(egui::RichText::new(snap.command.as_str()).monospace())
                .on_hover_text("Copy this command with your text")
                .clicked()
            {
                events.push(AIEvent::CopyCommand);
            }
        });
    }

    /// The chained-rule chips (`ai_view.rs` `draw_strip`), each toned by its
    /// [`StepState`].
    fn pipeline_strip(&self, ui: &mut egui::Ui, snap: &AISnapshot, colors: &PopupColors) {
        if snap.steps.is_empty() {
            return;
        }
        ui.horizontal(|ui| {
            for (i, (name, state)) in snap.steps.iter().enumerate() {
                if i > 0 {
                    ui.label(egui::RichText::new("\u{203a}").color(c(colors.dim)));
                }
                ui.label(
                    egui::RichText::new(name.as_str())
                        .color(c(colors.tone(step_tone(*state))))
                        .small(),
                );
            }
        });
    }

    /// The editable input (`ai_view.rs` `input` NSTextView + `textDidChange`).
    fn input_pane(
        &mut self,
        ui: &mut egui::Ui,
        snap: &AISnapshot,
        colors: &PopupColors,
        events: &mut Vec<AIEvent>,
    ) {
        ui.label(egui::RichText::new("Your text").color(c(colors.dim)).small());
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let resp = ui.add(
                egui::TextEdit::multiline(&mut self.input)
                    .hint_text(snap.placeholder.as_str())
                    .desired_width(f32::INFINITY),
            );
            if resp.changed() {
                events.push(AIEvent::InputChanged(self.input.clone()));
            }
        });
    }

    /// The right pane: the answer, rendered from the output runs. Diff and
    /// Markdown draw the native text (`ai_view.rs` `set_output`); the
    /// Outlook/Webex "as it will paste" web preview is AppKit-only, so the
    /// skeleton shows a hint instead.
    fn output_pane(&mut self, ui: &mut egui::Ui, snap: &AISnapshot, colors: &PopupColors) {
        ui.label(egui::RichText::new(snap.right_title.as_str()).color(c(colors.dim)).small());
        match output_kind(&snap.output) {
            OutputKind::Preview => {
                ui.label(
                    egui::RichText::new(
                        "The rich paste preview is AppKit-only; switch to Diff or Markdown to read the answer.",
                    )
                    .color(c(colors.dim))
                    .small(),
                );
            }
            OutputKind::Empty => {
                if snap.running {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(
                            egui::RichText::new("Waiting for the model\u{2026}")
                                .color(c(colors.dim)),
                        );
                    });
                } else {
                    ui.label(
                        egui::RichText::new(snap.placeholder.as_str()).color(c(colors.dim)),
                    );
                }
            }
            OutputKind::Plain => {
                // The Markdown answer, rendered by `egui_commonmark`
                // (`ai_view.rs` `set_output`'s single plain run).
                if let Some(runs) = &snap.output {
                    let md = runs_text(runs);
                    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                        egui_commonmark::CommonMarkViewer::new().show(ui, &mut self.md, &md);
                    });
                }
            }
            OutputKind::Diff => {
                // `ai_view.rs` `set_output`: -1 deleted (danger), +1 inserted
                // (success), 0 unchanged.
                if let Some(runs) = &snap.output {
                    egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                        for (kind, text) in runs {
                            let rt = egui::RichText::new(text.as_str()).monospace();
                            let rt = if *kind < 0 {
                                rt.color(c(colors.tone(PopupTone::Danger))).strikethrough()
                            } else if *kind > 0 {
                                rt.color(c(colors.text))
                                    .background_color(c(colors.tone(PopupTone::Success)))
                            } else {
                                rt.color(c(colors.text))
                            };
                            ui.label(rt);
                        }
                    });
                }
            }
        }
    }

    /// Run / Copy / status / tokens (the `ai_view.rs` footer strip).
    fn footer(
        &self,
        ui: &mut egui::Ui,
        snap: &AISnapshot,
        colors: &PopupColors,
        events: &mut Vec<AIEvent>,
    ) {
        ui.horizontal(|ui| {
            let run = if snap.running { "Stop" } else { "Run" };
            if widgets::capsule_accent(ui, run, colors).clicked() {
                events.push(AIEvent::Run);
            }
            if widgets::capsule(ui, snap.copy_title.as_str(), colors).clicked() {
                events.push(AIEvent::Copy);
            }
            ui.separator();
            ui.label(
                egui::RichText::new(snap.status.as_str())
                    .color(c(tone_color(colors, snap.tone)))
                    .small(),
            );
            if !snap.tokens.is_empty() {
                let col = if snap.tokens_warn {
                    colors.tone(PopupTone::Warning)
                } else {
                    colors.dim
                };
                ui.label(egui::RichText::new(snap.tokens.as_str()).color(c(col)).small());
            }
        });
    }

    /// Apply one event. Copy / CopyCommand are the frontend's pasteboard jobs
    /// (`host::ai_event`); everything else is the model's `handle`.
    fn apply(&mut self, ctx: &egui::Context, ev: AIEvent) {
        match &ev {
            AIEvent::Copy => match self.model.copy_plan(true) {
                CopyPlan::Markdown(md) => {
                    ctx.copy_text(md);
                    self.model.set_status("Copied the Markdown", AITone::Success);
                }
                // No rich clipboard off macOS; fall back to the plain answer.
                CopyPlan::Rich => {
                    ctx.copy_text(self.model.run.answer.clone());
                    self.model.set_status("Copied the answer", AITone::Success);
                }
                CopyPlan::Nothing => {}
            },
            AIEvent::CopyCommand => {
                if let Some(r) = self.model.rule() {
                    ctx.copy_text(r.runnable(&self.input));
                    self.model.set_status("Copied the command", AITone::Success);
                }
            }
            _ => {}
        }
        self.model.handle(ev);
    }
}

/// Which of the four output renderings a snapshot asks for (`ai_view.rs`
/// `apply`: `show_web` when `output.is_none()`; otherwise the native text pane,
/// empty until the first answer).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OutputKind {
    /// `output.is_none()` → the Outlook/Webex web preview.
    Preview,
    /// No runs yet and not running.
    Empty,
    /// Plain Markdown → `egui_commonmark`.
    Plain,
    /// A diff (any non-zero kind) → the coloured run list.
    Diff,
}

fn output_kind(output: &Option<Vec<OutRun>>) -> OutputKind {
    match output {
        None => OutputKind::Preview,
        Some(runs) if runs.is_empty() => OutputKind::Empty,
        Some(runs) if runs.iter().all(|(kind, _)| *kind == 0) => OutputKind::Plain,
        Some(_) => OutputKind::Diff,
    }
}

/// Concatenate the runs' text (the plain-answer / diff source string).
fn runs_text(runs: &[OutRun]) -> String {
    runs.iter().map(|(_, text)| text.as_str()).collect()
}

/// A pipeline chip's tone (`ai_view.rs` `AIPipelineStrip.State`).
fn step_tone(state: StepState) -> PopupTone {
    match state {
        StepState::Waiting => PopupTone::Dim,
        StepState::Running => PopupTone::Accent,
        StepState::Done => PopupTone::Success,
        StepState::Warn => PopupTone::Warning,
        StepState::Failed => PopupTone::Danger,
    }
}

/// The status line's tone (`AISnapshot.tone`: 0 dim / 1 success / 2 warning /
/// 3 danger).
fn tone_color(colors: &PopupColors, tone: u8) -> Rgba {
    match tone {
        1 => colors.tone(PopupTone::Success),
        2 => colors.tone(PopupTone::Warning),
        3 => colors.tone(PopupTone::Danger),
        _ => colors.dim,
    }
}

impl Pane for AiPane {
    fn ui(&mut self, ui: &mut egui::Ui, colors: &PopupColors) {
        AiPane::ui(self, ui, colors);
    }

    fn has_rail(&self) -> bool {
        true
    }

    fn rail(&self) -> bool {
        self.rail
    }

    /// Flip the sidebar rail; the pane owns its `sidebarRail.<view>`
    /// persistence, like the AppKit `SidebarView`.
    fn set_rail(&mut self, on: bool) {
        self.rail = on;
        crate::ui::sidebar::set_rail_default("ai", on);
    }

    /// Esc while a run is in flight cancels it (`AIWindow.handleKey`'s `Stop`);
    /// otherwise let the host's Esc handling win. Run/copy have buttons.
    fn handle_key(&mut self, action: KeyAction) {
        if let KeyAction::Escape(_) = action {
            if self.model.is_running() {
                self.model.cancel_run();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_kind_classifies_the_four_shapes() {
        assert_eq!(output_kind(&None), OutputKind::Preview);
        assert_eq!(output_kind(&Some(Vec::new())), OutputKind::Empty);
        assert_eq!(output_kind(&Some(vec![(0, "hi".into())])), OutputKind::Plain);
        assert_eq!(
            output_kind(&Some(vec![(-1, "a".into()), (0, "b".into()), (1, "c".into())])),
            OutputKind::Diff
        );
    }

    #[test]
    fn runs_text_joins_in_order() {
        let runs: Vec<OutRun> = vec![(0, "one ".into()), (-1, "two ".into()), (1, "three".into())];
        assert_eq!(runs_text(&runs), "one two three");
    }

    #[test]
    fn step_tone_maps_every_state() {
        assert_eq!(step_tone(StepState::Waiting), PopupTone::Dim);
        assert_eq!(step_tone(StepState::Running), PopupTone::Accent);
        assert_eq!(step_tone(StepState::Done), PopupTone::Success);
        assert_eq!(step_tone(StepState::Warn), PopupTone::Warning);
        assert_eq!(step_tone(StepState::Failed), PopupTone::Danger);
    }

    #[test]
    fn tone_color_selects_by_code() {
        let colors = PopupColors::default();
        assert_eq!(tone_color(&colors, 0), colors.dim);
        assert_eq!(tone_color(&colors, 3), colors.tone(PopupTone::Danger));
    }
}
