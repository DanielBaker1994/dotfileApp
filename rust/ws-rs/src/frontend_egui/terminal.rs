//! The egui terminal panel — the non-macOS port of the SwiftTerm-backed
//! `/terminal` tool panel (`views/terminal.rs`).
//!
//! `egui_term` (the alacritty_terminal-backed egui widget) tracks egui 0.31, not
//! the 0.32 this frontend uses, so this drives [`alacritty_terminal`] directly:
//! a [`Term`] + [`EventLoop`] run the PTY and parse it on a background thread,
//! and this widget renders the grid with egui text runs each frame. It is
//! functional-first (monospace text, the theme's ANSI palette, resize, mouse
//! scrollback) — no selection, no hyperlinks, no bold/italic glyph variants.
//!
//! [`TerminalView`] is reusable (W6-B's nvim pane): `new(program, args, cwd,
//! env)` spawns the process in a PTY and `ui` draws it and routes keys.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use alacritty_terminal::event::{Notify, OnResize, VoidListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, Msg, Notifier};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::tty;
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor, Rgb};

use crate::ui::theme::Rgba;
use crate::views::terminal::DEFAULT_FONT_SIZE;

/// A terminal size in cells, as [`Dimensions`] (`Term::new` / `Term::resize`).
struct TermSize {
    columns: usize,
    screen_lines: usize,
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.screen_lines
    }

    fn screen_lines(&self) -> usize {
        self.screen_lines
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

/// The terminal's colour palette, resolved once from the `[theme]` terminal
/// colours (`section_colors("terminal")` + `ansi_palette` for the 16 base
/// colours, `xterm256` for the indexed ramp) — the same source the AppKit panel
/// feeds SwiftTerm. Dynamic OSC colour changes are not merged in (minimal).
#[derive(Clone, Copy)]
struct TermPalette {
    fg: Rgb,
    bg: Rgb,
    /// Indexed colours: `0..16` = theme ANSI, `16..256` = xterm cube/grayscale.
    colors: [Rgb; 256],
}

impl TermPalette {
    fn from_theme() -> Self {
        let pc = crate::ui::theme::section_colors("terminal");
        let fg = rgba_to_rgb(pc.text);
        let bg = rgba_to_rgb(pc.background);
        let mut colors = [Rgb { r: 0, g: 0, b: 0 }; 256];
        for (i, c) in crate::engines::ansi_render::xterm256().iter().enumerate() {
            colors[i] = Rgb { r: c.r, g: c.g, b: c.b };
        }
        // The 16 base colours track the AppKit panel's `ansiPalette` exactly.
        for (i, c) in crate::ui::theme::ansi_palette(&pc).iter().enumerate() {
            colors[i] = rgba_to_rgb(*c);
        }
        TermPalette { fg, bg, colors }
    }

    /// Resolve a cell colour to `Rgb`. `Color::Named(Background)` maps to
    /// `None` (the caller falls back to the terminal background).
    fn resolve(&self, color: Color) -> Option<Rgb> {
        match color {
            Color::Named(n) => self.resolve_named(n),
            Color::Spec(rgb) => Some(rgb),
            Color::Indexed(i) => Some(self.colors[i as usize]),
        }
    }

    fn resolve_named(&self, n: NamedColor) -> Option<Rgb> {
        use NamedColor as N;
        match n {
            N::Black | N::Red | N::Green | N::Yellow | N::Blue | N::Magenta | N::Cyan | N::White
            | N::BrightBlack | N::BrightRed | N::BrightGreen | N::BrightYellow
            | N::BrightBlue | N::BrightMagenta | N::BrightCyan | N::BrightWhite => {
                Some(self.colors[n as usize])
            }
            N::Foreground | N::BrightForeground | N::DimForeground | N::Cursor => Some(self.fg),
            N::Background => None,
            N::DimBlack | N::DimRed | N::DimGreen | N::DimYellow | N::DimBlue | N::DimMagenta
            | N::DimCyan | N::DimWhite => Some(self.colors[n as usize - 259]),
        }
    }

    /// `(fg, bg)` for a cell, applying inverse.
    fn cell_colors(&self, cell: &Cell) -> (Rgb, Option<Rgb>) {
        let mut fg = self.resolve(cell.fg).unwrap_or(self.fg);
        let mut bg = self.resolve(cell.bg);
        if cell.flags.contains(Flags::INVERSE) {
            let b = bg.unwrap_or(self.bg);
            bg = Some(fg);
            fg = b;
        }
        (fg, bg)
    }
}

fn rgba_to_rgb(rgba: Rgba) -> Rgb {
    Rgb {
        r: (rgba.r * 255.0).round() as u8,
        g: (rgba.g * 255.0).round() as u8,
        b: (rgba.b * 255.0).round() as u8,
    }
}

fn rgb32(rgb: Rgb) -> egui::Color32 {
    egui::Color32::from_rgb(rgb.r, rgb.g, rgb.b)
}

/// The control byte for `Ctrl` + a letter (1..26), or `None` for other keys.
fn ctrl_byte(key: egui::Key) -> Option<u8> {
    use egui::Key as K;
    Some(match key {
        K::A => 0x01,
        K::B => 0x02,
        K::C => 0x03,
        K::D => 0x04,
        K::E => 0x05,
        K::F => 0x06,
        K::G => 0x07,
        K::H => 0x08,
        K::I => 0x09,
        K::J => 0x0a,
        K::K => 0x0b,
        K::L => 0x0c,
        K::M => 0x0d,
        K::N => 0x0e,
        K::O => 0x0f,
        K::P => 0x10,
        K::Q => 0x11,
        K::R => 0x12,
        K::S => 0x13,
        K::T => 0x14,
        K::U => 0x15,
        K::V => 0x16,
        K::W => 0x17,
        K::X => 0x18,
        K::Y => 0x19,
        K::Z => 0x1a,
        _ => return None,
    })
}

/// Map a non-printable egui key press to the PTY byte sequence it should send.
///
/// Printable characters arrive as `egui::Event::Text` (not a `Key` event) and
/// are handled separately, so this returns `None` for letters/digits/symbols.
/// `Ctrl` + a letter maps to the control byte; the special keys map to their
/// xterm escape sequences. Pure and unit-tested.
fn key_to_bytes(key: egui::Key, mods: egui::Modifiers) -> Option<Vec<u8>> {
    use egui::Key as K;

    if mods.ctrl && !mods.alt && !mods.mac_cmd {
        if let Some(b) = ctrl_byte(key) {
            return Some(vec![b]);
        }
    }

    let bytes: &[u8] = match key {
        K::Enter => b"\r",
        K::Backspace => b"\x7f",
        K::Tab => b"\t",
        K::Escape => b"\x1b",
        K::ArrowUp => b"\x1b[A",
        K::ArrowDown => b"\x1b[B",
        K::ArrowRight => b"\x1b[C",
        K::ArrowLeft => b"\x1b[D",
        K::Home => b"\x1b[H",
        K::End => b"\x1b[F",
        K::Delete => b"\x1b[3~",
        K::Insert => b"\x1b[2~",
        K::PageUp => b"\x1b[5~",
        K::PageDown => b"\x1b[6~",
        _ => return None,
    };
    Some(bytes.to_vec())
}

/// An embedded terminal: a shell (or any program) running in a PTY, rendered as
/// a monospace grid. Reusable (W6-B reuses it for the nvim pane).
pub(super) struct TerminalView {
    term: Option<Arc<FairMutex<Term<VoidListener>>>>,
    notifier: Option<Notifier>,
    error: Option<String>,
    palette: TermPalette,
    font_size: f32,
    cols: usize,
    rows: usize,
    /// Un-applied mouse-wheel scroll (points), flushed to lines once a full row
    /// accumulates.
    scroll_pending: f32,
}

impl TerminalView {
    /// Spawn `program` (with `args`) in a PTY, in `cwd`, with the extra `env`
    /// vars layered over the inherited environment. Never fails the caller: a
    /// spawn error is recorded and shown in [`Self::ui`].
    pub(super) fn new(
        program: String,
        args: Vec<String>,
        cwd: Option<PathBuf>,
        env: HashMap<String, String>,
    ) -> Self {
        let cols = 80usize;
        let rows = 24usize;
        let window_size = WindowSize {
            num_lines: rows as u16,
            num_cols: cols as u16,
            cell_width: 8,
            cell_height: 16,
        };
        let palette = TermPalette::from_theme();
        let (term, notifier, error) = match spawn(program, args, cwd, env, window_size, cols, rows) {
            Ok((term, notifier)) => (Some(term), Some(notifier), None),
            Err(e) => (None, None, Some(e)),
        };
        TerminalView {
            term,
            notifier,
            error,
            palette,
            font_size: DEFAULT_FONT_SIZE as f32,
            cols,
            rows,
            scroll_pending: 0.0,
        }
    }

    /// Set the cell font size (config `terminal-font-size`); applied next frame.
    pub(super) fn set_font_size(&mut self, size: f32) {
        self.font_size = size.max(6.0);
    }

    /// Draw the terminal, route keys, and apply scrollback / resize for one frame.
    pub(super) fn ui(&mut self, ui: &mut egui::Ui) {
        let rect = ui.available_rect_before_wrap();
        let resp = ui.allocate_rect(rect, egui::Sense::click());
        if resp.clicked() {
            resp.request_focus();
        }

        let font_id = egui::FontId::monospace(self.font_size);
        let (cell_w, row_h) = ui.fonts(|f| (f.glyph_width(&font_id, 'M'), f.row_height(&font_id)));
        if cell_w <= 0.0 || row_h <= 0.0 {
            return;
        }

        let cols = ((resp.rect.width() / cell_w).floor() as usize).max(2);
        let rows = ((resp.rect.height() / row_h).floor() as usize).max(1);
        if cols != self.cols || rows != self.rows {
            self.resize(cols, rows, cell_w, row_h);
        }

        // The app's `handle_keys` skips while any widget has focus, so with the
        // terminal focused these are the only consumer of the key events.
        if resp.has_focus() {
            for ev in ui.input(|i| i.events.clone()) {
                match ev {
                    egui::Event::Text(text) => self.write(text.into_bytes()),
                    egui::Event::Key { key, pressed: true, modifiers, .. } => {
                        if let Some(bytes) = key_to_bytes(key, modifiers) {
                            self.write(bytes);
                        }
                    }
                    _ => {}
                }
            }
            let scroll = ui.input(|i| i.raw_scroll_delta.y);
            if scroll != 0.0 {
                self.scroll_pending += scroll;
            }
        }

        self.draw(ui, resp.rect, cell_w, row_h);

        // Flush accumulated scroll into history lines (outside the draw's read lock).
        if self.scroll_pending.abs() >= row_h {
            let lines = (self.scroll_pending / row_h).round() as i32;
            self.scroll_pending = 0.0;
            if let Some(term) = &self.term {
                term.lock_unfair().scroll_display(Scroll::Delta(lines));
            }
        }
    }

    fn write(&self, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        if let Some(n) = &self.notifier {
            n.notify(bytes);
        }
    }

    fn resize(&mut self, cols: usize, rows: usize, cell_w: f32, row_h: f32) {
        self.cols = cols;
        self.rows = rows;
        if let Some(term) = &self.term {
            term.lock_unfair().resize(TermSize { columns: cols, screen_lines: rows });
        }
        let window_size = WindowSize {
            num_lines: rows as u16,
            num_cols: cols as u16,
            cell_width: cell_w.round() as u16,
            cell_height: row_h.round() as u16,
        };
        if let Some(n) = &mut self.notifier {
            n.on_resize(window_size);
        }
    }

    fn draw(&mut self, ui: &egui::Ui, rect: egui::Rect, cell_w: f32, row_h: f32) {
        let palette = self.palette;
        let font_id = egui::FontId::monospace(self.font_size);
        let painter = ui.painter();
        let origin = rect.min;
        let bg32 = rgb32(palette.bg);
        painter.rect_filled(rect, 0.0, bg32);

        if let Some(error) = &self.error {
            painter.text(
                origin + egui::vec2(8.0, 8.0),
                egui::Align2::LEFT_TOP,
                error,
                font_id,
                rgb32(palette.fg),
            );
            return;
        }

        let Some(term) = &self.term else { return };
        let content = term.lock_unfair();
        let rc = content.renderable_content();
        let top_line = -(rc.display_offset as i32);
        let (cols, rows) = (self.cols, self.rows);

        for indexed in rc.display_iter {
            let row = (indexed.point.line.0 - top_line) as usize;
            let col = indexed.point.column.0;
            let cell = indexed.cell;
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER)
                || cell.flags.contains(Flags::LEADING_WIDE_CHAR_SPACER)
            {
                continue;
            }

            let (fg, bg) = palette.cell_colors(cell);
            let pos = origin + egui::vec2(col as f32 * cell_w, row as f32 * row_h);
            let cell_rect = egui::Rect::from_min_size(pos, egui::vec2(cell_w, row_h));
            if let Some(b) = bg {
                if b != palette.bg {
                    painter.rect_filled(cell_rect, 0.0, rgb32(b));
                }
            }

            let ch = cell.c;
            if ch == ' ' || cell.flags.contains(Flags::HIDDEN) {
                continue;
            }
            let mut color = rgb32(fg);
            if cell.flags.contains(Flags::DIM) {
                color = color.gamma_multiply(0.5);
            }
            painter.text(pos, egui::Align2::LEFT_TOP, ch.to_string(), font_id.clone(), color);
            if cell.flags.contains(Flags::UNDERLINE) {
                painter.line_segment(
                    [
                        pos + egui::vec2(0.0, row_h - 1.0),
                        pos + egui::vec2(cell_w, row_h - 1.0),
                    ],
                    egui::Stroke::new(1.0, color),
                );
            }
            if cell.flags.contains(Flags::STRIKEOUT) {
                let mid = pos + egui::vec2(0.0, row_h * 0.5);
                painter.line_segment(
                    [mid, mid + egui::vec2(cell_w, 0.0)],
                    egui::Stroke::new(1.0, color),
                );
            }
        }

        // Block cursor: fill the cell with the text colour, redraw the char in
        // the background colour.
        if rc.cursor.shape != CursorShape::Hidden {
            let p = rc.cursor.point;
            let row = (p.line.0 - top_line) as usize;
            let col = p.column.0;
            if row < rows && col < cols {
                let pos = origin + egui::vec2(col as f32 * cell_w, row as f32 * row_h);
                let cell_rect = egui::Rect::from_min_size(pos, egui::vec2(cell_w, row_h));
                painter.rect_filled(cell_rect, 0.0, rgb32(palette.fg));
                let ch = content.grid()[p].c;
                if ch != ' ' {
                    painter.text(pos, egui::Align2::LEFT_TOP, ch.to_string(), font_id.clone(), bg32);
                }
            }
        }
    }
}

impl Drop for TerminalView {
    fn drop(&mut self) {
        if let Some(n) = &self.notifier {
            let _ = n.0.send(Msg::Shutdown);
        }
    }
}

/// Build the `Term` + `EventLoop`, spawn the PTY reader thread, and return the
/// shared terminal handle and its input notifier.
fn spawn(
    program: String,
    args: Vec<String>,
    cwd: Option<PathBuf>,
    env: HashMap<String, String>,
    window_size: WindowSize,
    cols: usize,
    rows: usize,
) -> Result<(Arc<FairMutex<Term<VoidListener>>>, Notifier), String> {
    let pty_config = tty::Options {
        shell: Some(tty::Shell::new(program, args)),
        working_directory: cwd,
        env,
        ..tty::Options::default()
    };
    let size = TermSize { columns: cols, screen_lines: rows };
    let pty = tty::new(&pty_config, window_size, 0).map_err(|e| e.to_string())?;
    let term = Term::new(Config::default(), &size, VoidListener);
    let term = Arc::new(FairMutex::new(term));
    let event_loop = EventLoop::new(term.clone(), VoidListener, pty, true, false)
        .map_err(|e| e.to_string())?;
    let notifier = Notifier(event_loop.channel());
    event_loop.spawn();
    Ok((term, notifier))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_mods() -> egui::Modifiers {
        egui::Modifiers::default()
    }

    #[test]
    fn maps_special_keys_to_pty_bytes() {
        use egui::Key as K;
        assert_eq!(key_to_bytes(K::Enter, no_mods()).unwrap(), b"\r");
        assert_eq!(key_to_bytes(K::Backspace, no_mods()).unwrap(), b"\x7f");
        assert_eq!(key_to_bytes(K::Tab, no_mods()).unwrap(), b"\t");
        assert_eq!(key_to_bytes(K::Escape, no_mods()).unwrap(), b"\x1b");
        assert_eq!(key_to_bytes(K::ArrowUp, no_mods()).unwrap(), b"\x1b[A");
        assert_eq!(key_to_bytes(K::ArrowDown, no_mods()).unwrap(), b"\x1b[B");
        assert_eq!(key_to_bytes(K::ArrowRight, no_mods()).unwrap(), b"\x1b[C");
        assert_eq!(key_to_bytes(K::ArrowLeft, no_mods()).unwrap(), b"\x1b[D");
        assert_eq!(key_to_bytes(K::Home, no_mods()).unwrap(), b"\x1b[H");
        assert_eq!(key_to_bytes(K::End, no_mods()).unwrap(), b"\x1b[F");
        assert_eq!(key_to_bytes(K::Delete, no_mods()).unwrap(), b"\x1b[3~");
        assert_eq!(key_to_bytes(K::PageUp, no_mods()).unwrap(), b"\x1b[5~");
        assert_eq!(key_to_bytes(K::PageDown, no_mods()).unwrap(), b"\x1b[6~");
    }

    #[test]
    fn maps_ctrl_letters_to_control_bytes() {
        use egui::Key as K;
        let ctrl = egui::Modifiers { ctrl: true, ..egui::Modifiers::default() };
        assert_eq!(key_to_bytes(K::C, ctrl).unwrap(), vec![0x03]);
        assert_eq!(key_to_bytes(K::D, ctrl).unwrap(), vec![0x04]);
        assert_eq!(key_to_bytes(K::L, ctrl).unwrap(), vec![0x0c]);
        assert_eq!(key_to_bytes(K::Z, ctrl).unwrap(), vec![0x1a]);
        // Command/Ctrl+Alt are not treated as terminal Ctrl.
        let cmd = egui::Modifiers { mac_cmd: true, ctrl: true, ..egui::Modifiers::default() };
        assert_eq!(key_to_bytes(K::C, cmd), None);
    }

    #[test]
    fn printable_keys_are_left_to_event_text() {
        use egui::Key as K;
        assert_eq!(key_to_bytes(K::A, no_mods()), None);
        assert_eq!(key_to_bytes(K::Num0, no_mods()), None);
        assert_eq!(key_to_bytes(K::Space, no_mods()), None);
        let shifted = egui::Modifiers { shift: true, ..egui::Modifiers::default() };
        assert_eq!(key_to_bytes(K::A, shifted), None);
    }

    #[test]
    fn palette_resolves_named_spec_and_indexed_colors() {
        let p = TermPalette::from_theme();
        assert_eq!(p.resolve(Color::Named(NamedColor::Red)).unwrap(), p.colors[1]);
        assert_eq!(
            p.resolve(Color::Spec(Rgb { r: 10, g: 20, b: 30 })),
            Some(Rgb { r: 10, g: 20, b: 30 })
        );
        assert_eq!(p.resolve(Color::Indexed(196)).unwrap(), p.colors[196]);
        assert_eq!(p.resolve(Color::Named(NamedColor::Background)), None);
        assert_eq!(p.resolve(Color::Named(NamedColor::Foreground)).unwrap(), p.fg);
        assert_eq!(p.resolve(Color::Named(NamedColor::DimRed)).unwrap(), p.colors[1]);
    }

    #[test]
    fn inverse_swaps_foreground_and_background() {
        let p = TermPalette::from_theme();
        let mut cell = Cell {
            c: 'x',
            fg: Color::Named(NamedColor::Green),
            bg: Color::Named(NamedColor::Background),
            flags: Flags::empty(),
            extra: None,
        };
        let (fg, bg) = p.cell_colors(&cell);
        assert_eq!(fg, p.colors[2]);
        assert_eq!(bg, None);

        cell.flags.insert(Flags::INVERSE);
        let (fg, bg) = p.cell_colors(&cell);
        assert_eq!(fg, p.bg);
        assert_eq!(bg, Some(p.colors[2]));
    }
}
