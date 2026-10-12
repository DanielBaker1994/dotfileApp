//! The Confluence view — Wave 5 egui port (copies `files.rs`'s shape).
//!
//! It drives the same pure [`crate::views::confluence::ConfluenceModel`] the
//! AppKit [`crate::views::confluence_view::ConfluenceView`] drives: pull a
//! [`ConfSnapshot`] each frame, draw it with egui widgets, collect the user's
//! actions as [`ConfEvent`]s, and hand them to [`ConfluenceModel::handle`] — the
//! exact same event contract the AppKit surface reports. Each section below is
//! commented with the `confluence_view.rs` region it mirrors.
//!
//! The actions the model leaves to the frontend (the API search, favorites
//! fetch, chip popups and the clipboard) are emitted as [`ConfEvent`]s for a
//! later wiring pass; this pane does not run the Confluence API. The `WKWebView`
//! preview is replaced the functional-first Linux way: plain text plus an **Open
//! in browser** button, which writes the themed page to a temp file and opens it
//! with [`crate::platform::open_url`].

use std::path::{Path, PathBuf};

use crate::ui::popup::KeyAction;
use crate::ui::sidebar::SidebarEvent;
use crate::ui::theme::{PopupColors, PopupTone};
use crate::views::confluence::{ConfluenceConfig, ConfluenceModel, ConfluenceRow, StatusTone};
use crate::views::confluence_view::{ConfEvent, ConfSnapshot, MODES};

use super::app::Pane;
use super::theme::c;

pub struct ConfluencePane {
    pub model: ConfluenceModel,
    /// The search field's editable buffer (`ConfIvars.search` NSTextField).
    query: String,
}

impl ConfluencePane {
    /// Build the model like `DaemonUi::build_confluence` (the pure half; the
    /// search strip / table / splitter and the `WKWebView` are the AppKit-only
    /// half and are skipped here).
    pub fn build() -> Self {
        let text = crate::app::config::read_config_text().unwrap_or_default();
        let entries = crate::views::tools::section_vars(&text, "confluence");
        let mut model = ConfluenceModel::new(ConfluenceConfig::from_entries(entries));
        model.show_search_empty();
        ConfluencePane { model, query: String::new() }
    }

    /// Draw one frame and apply the events it generated. `colors` are the
    /// `[confluence]` section tokens (from [`crate::ui::theme::section_colors`]).
    pub fn ui(&mut self, ui: &mut egui::Ui, colors: &PopupColors) {
        let snap = self.model.snapshot();
        let mut events: Vec<ConfEvent> = Vec::new();

        // --- sidebar | strip/list/preview split (`confluence_view.rs` `layout_all`) ---
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(200.0, ui.available_height()),
                egui::Layout::top_down(egui::Align::Min),
                |ui| self.sidebar(ui, &snap, colors, &mut events),
            );
            ui.separator();
            ui.vertical(|ui| {
                self.search_strip(ui, &snap, colors, &mut events);
                ui.separator();
                self.results_and_preview(ui, &snap, colors, &mut events);
                ui.separator();
                self.footer(ui, &snap, colors, &mut events);
            });
        });

        for ev in events {
            self.model.handle(ev);
        }
    }

    /// The left scope rail (`confluence_view.rs` `SidebarView`; Search /
    /// Favorites rows).
    fn sidebar(
        &self,
        ui: &mut egui::Ui,
        snap: &ConfSnapshot,
        colors: &PopupColors,
        events: &mut Vec<ConfEvent>,
    ) {
        let m = &snap.sidebar;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            if !m.section_title.is_empty() {
                ui.label(
                    egui::RichText::new(m.section_title.to_uppercase())
                        .color(c(colors.dim))
                        .small(),
                );
            }
            for (i, row) in m.rows.iter().enumerate() {
                let label = egui::RichText::new(row.title.as_str()).color(c(colors.text));
                if ui.selectable_label(m.selected == Some(i), label).clicked() {
                    events.push(ConfEvent::Sidebar(SidebarEvent::Select(i)));
                }
            }
        });
    }

    /// The search strip (`confluence_view.rs` `controls` / `draw` top row):
    /// mode segments, the title-only toggle, the search field + Search button,
    /// then the criteria chips.
    fn search_strip(
        &mut self,
        ui: &mut egui::Ui,
        snap: &ConfSnapshot,
        colors: &PopupColors,
        events: &mut Vec<ConfEvent>,
    ) {
        // --- ConfSegmented mode chips (`MODES`) ---
        ui.horizontal(|ui| {
            for (i, title) in MODES.iter().enumerate() {
                let label = egui::RichText::new(*title).color(c(colors.text));
                if ui.selectable_label(snap.mode == i, label).clicked() {
                    events.push(ConfEvent::Mode(i));
                }
            }
            // --- ConfToggle "Title only" ---
            let mut title_only = snap.title_only;
            if ui.checkbox(&mut title_only, "Title only").changed() {
                events.push(ConfEvent::TitleOnly(title_only));
            }
            // --- search field + Search button (`ConfIvars.search`) ---
            // Mirror the AppKit `apply`: the field follows `snap.query` except
            // while it is being edited (typing must not be clobbered).
            let id = ui.make_persistent_id("confluence-search");
            let focused = ui.memory(|m| m.has_focus(id));
            if !focused && self.query != snap.query {
                self.query = snap.query.clone();
            }
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .hint_text("Search Confluence    \"exact phrase\"    prefix*")
                    .id(id),
            );
            if resp.changed() {
                events.push(ConfEvent::Query(self.query.clone()));
            }
            if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                events.push(ConfEvent::Enter);
            }
            if ui.button("Search").clicked() {
                events.push(ConfEvent::Search);
            }
        });
        // --- criteria chips (`JiraMultiPicker` Spaces/Contributors + choices) ---
        ui.horizontal(|ui| {
            let chips = [
                (0usize, snap.spaces.as_str()),
                (1, snap.people.as_str()),
                (2, snap.type_label.as_str()),
                (3, snap.mod_label.as_str()),
                (4, snap.sort_label.as_str()),
            ];
            for (which, text) in chips {
                if ui.button(text).clicked() {
                    events.push(ConfEvent::Chip { which, x: 0.0, y: 0.0 });
                }
            }
        });
    }

    /// The results list (left) | preview (right) split
    /// (`confluence_view.rs` `regions` table / preview).
    fn results_and_preview(
        &self,
        ui: &mut egui::Ui,
        snap: &ConfSnapshot,
        colors: &PopupColors,
        events: &mut Vec<ConfEvent>,
    ) {
        ui.columns(2, |cols| {
            // --- results list (`ConfResultsView` / `RESULT_H` rows) ---
            let rows = snap.rows.clone();
            let selection = snap.selection;
            cols[0].push_id("confluence-results", |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    if rows.is_empty() {
                        if !snap.empty.is_empty() {
                            ui.label(egui::RichText::new(snap.empty.as_str()).color(c(colors.dim)));
                        }
                        return;
                    }
                    for (i, r) in rows.iter().enumerate() {
                        // Star gutter (`ConfResultsView.mouseDown` x < STAR_GUTTER).
                        let star = if r.favorite { "★" } else { "☆" };
                        ui.horizontal_top(|ui| {
                            let star_color = if r.favorite {
                                c(colors.tone(PopupTone::Warning))
                            } else {
                                c(colors.dim)
                            };
                            if ui
                                .selectable_label(false, egui::RichText::new(star).color(star_color))
                                .on_hover_text("Favorite")
                                .clicked()
                            {
                                events.push(ConfEvent::Star(i));
                            }
                            let mut title = if r.title.is_empty() {
                                "(untitled)".to_string()
                            } else {
                                r.title.clone()
                            };
                            if !r.type_label().is_empty() {
                                title = format!("{}  {}", r.type_label().to_uppercase(), title);
                            }
                            let title_color = if r.missing { c(colors.dim) } else { c(colors.text) };
                            let resp = ui
                                .vertical(|ui| {
                                    let tr = ui.selectable_label(
                                        selection == Some(i),
                                        egui::RichText::new(&title).color(title_color),
                                    );
                                    ui.label(
                                        egui::RichText::new(r.meta()).color(c(colors.dim)).small(),
                                    );
                                    if !r.excerpt.is_empty() {
                                        ui.label(
                                            egui::RichText::new(r.excerpt.as_str())
                                                .color(c(colors.text)),
                                        );
                                    }
                                    tr
                                })
                                .inner;
                            if resp.clicked() {
                                events.push(ConfEvent::Select { row: i, double: resp.double_clicked() });
                            }
                        });
                        ui.separator();
                    }
                });
            });

            // --- preview (`confluence_view.rs` preview region) ---
            let row = selection.and_then(|i| rows.get(i)).cloned();
            cols[1].push_id("confluence-preview", |ui| {
                self.preview(ui, snap, row.as_ref(), colors, events);
            });
        });
    }

    /// The preview pane: plain text (title · meta · excerpt) plus the frontend's
    /// **Open in browser** (the `WKWebView` replacement).
    fn preview(
        &self,
        ui: &mut egui::Ui,
        snap: &ConfSnapshot,
        row: Option<&ConfluenceRow>,
        colors: &PopupColors,
        events: &mut Vec<ConfEvent>,
    ) {
        let p = &snap.preview;
        if !p.hint.is_empty() {
            ui.label(egui::RichText::new(p.hint.as_str()).color(c(colors.dim)));
        }
        if p.setup && ui.button("Set Up Confluence…").clicked() {
            events.push(ConfEvent::Setup);
        }
        let Some(row) = row else { return };

        ui.horizontal(|ui| {
            ui.heading(if row.title.is_empty() { "(untitled)".to_string() } else { row.title.clone() });
            let star = if row.favorite { "★" } else { "☆" };
            if ui.selectable_label(row.favorite, star).on_hover_text("Favorite").clicked() {
                events.push(ConfEvent::PreviewStar);
            }
        });
        let meta = row.meta();
        if !meta.is_empty() {
            ui.label(egui::RichText::new(meta).color(c(colors.dim)).small());
        }
        ui.separator();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.label(egui::RichText::new(row.excerpt.as_str()).color(c(colors.text)));
        });
        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("Open in browser").clicked() {
                self.open_in_browser(row);
            }
            if ui.add_enabled(!row.url.is_empty(), egui::Button::new("Copy Link")).clicked() {
                events.push(ConfEvent::CopyLink);
            }
        });
    }

    /// The footer (`confluence_view.rs` footer): status · Load more · Copy CQL |
    /// Copy curl.
    fn footer(
        &self,
        ui: &mut egui::Ui,
        snap: &ConfSnapshot,
        colors: &PopupColors,
        events: &mut Vec<ConfEvent>,
    ) {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(snap.status.as_str())
                    .color(tone_color(snap.tone, colors))
                    .small(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add_enabled(snap.has_curl, egui::Button::new("Copy curl")).clicked() {
                    events.push(ConfEvent::CopyCurl);
                }
                if ui.add_enabled(snap.has_cql, egui::Button::new("Copy CQL")).clicked() {
                    events.push(ConfEvent::CopyCql);
                }
                if ui.add_enabled(snap.can_more, egui::Button::new("Load more")).clicked() {
                    events.push(ConfEvent::LoadMore);
                }
            });
        });
    }

    /// The frontend's "Open in browser": render the row as the themed page and
    /// open it from a temp file (there is no `WKWebView` here).
    fn open_in_browser(&self, row: &ConfluenceRow) {
        let colors = crate::ui::theme::section_colors("confluence");
        let body = if row.excerpt.is_empty() {
            String::new()
        } else {
            format!("<p>{}</p>", escape_html(&row.excerpt))
        };
        let html = crate::views::confluence::placeholder_html(&colors, &row.title, &body);
        let path = preview_path(&row.id);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if std::fs::write(&path, html).is_ok() {
            let _ = crate::platform::open_url(&file_url(&path));
        }
    }

    /// Enter on the list opens the selected row (the frontend's `ConfEvent::Open`).
    fn open_selected_in_browser(&mut self) {
        if let Some(row) = self.model.rows.get(self.model.selection).cloned() {
            self.open_in_browser(&row);
        }
    }
}

/// `StatusTone` → the footer text colour (`confluence_view.rs` `tone_rgba`).
fn tone_color(tone: Option<StatusTone>, colors: &PopupColors) -> egui::Color32 {
    match tone {
        Some(StatusTone::Success) => c(colors.tone(PopupTone::Success)),
        Some(StatusTone::Warning) => c(colors.tone(PopupTone::Warning)),
        Some(StatusTone::Danger) => c(colors.tone(PopupTone::Danger)),
        _ => c(colors.dim),
    }
}

/// The temp file a row's preview is written to (under the system temp dir).
fn preview_path(id: &str) -> PathBuf {
    std::env::temp_dir()
        .join("kitchen-sink")
        .join("confluence")
        .join(preview_file_name(id))
}

/// `<id>.html`, with path-hostile characters replaced so a page id can never
/// escape the preview directory.
fn preview_file_name(id: &str) -> String {
    let mut s: String = id
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') { ch } else { '_' })
        .collect();
    if s.is_empty() {
        s.push_str("page");
    }
    s.push_str(".html");
    s
}

/// The `file://` URL [`crate::platform::open_url`] takes for an absolute path.
fn file_url(path: &Path) -> String {
    format!("file://{}", path.display())
}

/// Escape the excerpt before it is embedded in the preview page's body.
fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

impl Pane for ConfluencePane {
    fn ui(&mut self, ui: &mut egui::Ui, colors: &PopupColors) {
        ConfluencePane::ui(self, ui, colors);
    }

    /// The key actions the Confluence model consumes (the same ones
    /// `route_key_event` routed to it before the registry refactor): Escape,
    /// list up/down, and Enter (which opens the preview in the browser here).
    fn handle_key(&mut self, action: KeyAction) {
        match action {
            KeyAction::Escape(_) => self.model.handle(ConfEvent::Escape),
            KeyAction::ListMove(delta) => self.model.handle(ConfEvent::Move(delta as i64)),
            KeyAction::ListAccept => self.open_selected_in_browser(),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_file_name_is_sanitized() {
        assert_eq!(preview_file_name("12345"), "12345.html");
        assert_eq!(preview_file_name("A/B:c d"), "A_B_c_d.html");
        assert_eq!(preview_file_name(""), "page.html");
        // Traversal can never leave the preview directory (no separators).
        assert_eq!(preview_file_name("../../etc/passwd"), ".._.._etc_passwd.html");
    }

    #[test]
    fn file_url_prefixes_absolute_paths() {
        assert_eq!(file_url(Path::new("/tmp/ks/a b.html")), "file:///tmp/ks/a b.html");
    }

    #[test]
    fn escape_html_neutralizes_markup() {
        assert_eq!(escape_html("<b>&\"x\"</b>"), "&lt;b&gt;&amp;&quot;x&quot;&lt;/b&gt;");
        assert_eq!(escape_html("it's"), "it&#39;s");
        assert_eq!(escape_html("plain"), "plain");
    }
}
