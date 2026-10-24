//! The code panel: the open chip's `.chip` file, editable, with syntax
//! colors and problems underlined. It saves shortly after you stop typing,
//! and follows along when the canvas (or anyone else) changes the file.
//!
//! Selecting a box on the canvas highlights its line here, and moving the
//! cursor onto a line selects its box: one line, one box.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::text::{LayoutJob, TextFormat};
use egui::{FontId, RichText, Stroke};
use wafer_core::ast::StmtValue;

use crate::canvas::Canvas;
use crate::theme;
use crate::workspace::Workspace;

/// How long after the last keystroke the file is saved.
pub const SAVE_DELAY: Duration = Duration::from_millis(350);

#[derive(Default)]
pub struct CodePanel {
    path: Option<PathBuf>,
    /// What's in the editor.
    buffer: String,
    /// The file's text as of the last load or save.
    base: String,
    /// When the buffer was last edited, if it hasn't been saved since.
    edited: Option<Instant>,
    /// The file changed on disk while there were unsaved edits.
    conflict: bool,
    seen_revision: u64,
    last_cursor: Option<usize>,
}

impl CodePanel {
    /// Text currently in the editor (for tests).
    pub fn text(&self) -> &str {
        &self.buffer
    }

    pub fn has_conflict(&self) -> bool {
        self.conflict
    }

    /// Saves now if there are unsaved edits.
    pub fn flush(&mut self, ws: &mut Workspace) {
        if let (Some(path), Some(_), false) = (&self.path, self.edited, self.conflict) {
            let path = path.clone();
            ws.write_text(&path, &self.buffer);
            self.base = self.buffer.clone();
            self.edited = None;
            self.seen_revision = ws.revision;
        }
    }

    /// Keeps the editor in step with the file on disk.
    fn sync(&mut self, ws: &mut Workspace, chip: &str) {
        let path = ws.chip_file(chip);
        if path != self.path {
            self.flush(ws);
            self.path = path.clone();
            self.buffer = path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
            self.base = self.buffer.clone();
            self.edited = None;
            self.conflict = false;
            self.seen_revision = ws.revision;
            self.last_cursor = None;
            return;
        }
        if ws.revision == self.seen_revision {
            return;
        }
        self.seen_revision = ws.revision;
        let Some(disk) = self.path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()) else { return };
        if disk == self.base {
            return;
        }
        if self.edited.is_some() {
            self.conflict = true;
        } else {
            self.buffer = disk.clone();
        }
        self.base = disk;
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, ws: &mut Workspace, canvas: &mut Canvas) {
        let chip = canvas.chip.clone();
        self.sync(ws, &chip);
        let Some(path) = self.path.clone() else {
            ui.label(RichText::new("No chip open.").color(theme::MUTED));
            return;
        };

        ui.horizontal(|ui| {
            let name = path.strip_prefix(&ws.dir).unwrap_or(&path).display().to_string();
            ui.label(RichText::new(name).monospace().color(theme::MUTED));
            if self.edited.is_some() && !self.conflict {
                ui.label(RichText::new("● saving…").color(theme::BUS).small());
            }
        });
        if self.conflict {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new("⚠ This file changed outside the app while you were typing.").color(theme::WARNING),
                );
                if ui.button("Use the file's version").clicked() {
                    self.buffer = self.base.clone();
                    self.edited = None;
                    self.conflict = false;
                }
                if ui.button("Keep mine").clicked() {
                    self.conflict = false;
                    self.edited = Some(Instant::now() - SAVE_DELAY);
                }
            });
        }

        // Line highlights and underlines only line up while the editor
        // matches what was last checked.
        let in_sync = self.buffer == self.base && ws.stale.is_none();
        let spans = if in_sync { node_spans(ws, &chip) } else { Vec::new() };
        let selected = match canvas.selection.iter().collect::<Vec<_>>().as_slice() {
            [key] => spans.iter().find(|(k, _, _)| k == *key).map(|&(_, start, _)| line_range(&self.buffer, start)),
            _ => None,
        };
        let problems: Vec<(usize, usize, bool)> = if self.buffer == self.base {
            ws.problems.iter().filter(|p| p.file == path).map(|p| (p.start, p.end.max(p.start + 1), p.error)).collect()
        } else {
            Vec::new()
        };

        let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, _wrap: f32| {
            let mut job = highlight(text.as_str(), selected.clone(), &problems);
            job.wrap.max_width = f32::INFINITY;
            ui.fonts_mut(|f| f.layout_job(job))
        };
        let output = egui::ScrollArea::both()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::TextEdit::multiline(&mut self.buffer)
                    .code_editor()
                    .desired_width(f32::INFINITY)
                    .desired_rows(30)
                    .lock_focus(true)
                    .layouter(&mut layouter)
                    .show(ui)
            })
            .inner;

        if output.response.changed() {
            self.edited = Some(Instant::now());
        }
        let save_now =
            output.response.has_focus() && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::S));
        if let Some(when) = self.edited {
            if save_now || when.elapsed() >= SAVE_DELAY {
                self.flush(ws);
            } else {
                ui.ctx().request_repaint_after(SAVE_DELAY);
            }
        }

        // Moving the cursor onto a line selects that line's box.
        let cursor = output.cursor_range.map(|r| char_to_byte(&self.buffer, r.primary.index.0));
        if output.response.has_focus()
            && cursor != self.last_cursor
            && in_sync
            && let Some(at) = cursor
        {
            let line = line_range(&self.buffer, at);
            let on_line: Vec<&(String, usize, usize)> = spans.iter().filter(|(_, s, _)| line.contains(s)).collect();
            let best = on_line.iter().find(|(_, s, e)| (*s..=*e).contains(&at)).or(on_line.first());
            if let Some((key, _, _)) = best {
                canvas.selection = [key.clone()].into();
                canvas.selected_wire = None;
            }
        }
        self.last_cursor = cursor;
    }
}

/// Where each box is written in the file: its key and byte range.
fn node_spans(ws: &Workspace, chip: &str) -> Vec<(String, usize, usize)> {
    let Some(def) = ws.project.chips().find(|c| c.name.name == chip) else { return Vec::new() };
    let mut spans = Vec::new();
    for decl in &def.inputs {
        spans.push((format!("in:{}", decl.name.name), decl.span.start, decl.span.end));
    }
    for decl in &def.outputs {
        spans.push((format!("out:{}", decl.name.name), decl.span.start, decl.span.end));
    }
    for stmt in &def.stmts {
        if let StmtValue::Call(_) = stmt.value {
            spans.push((stmt.target.name.clone(), stmt.span.start, stmt.span.end));
        }
    }
    spans
}

fn line_range(text: &str, at: usize) -> std::ops::Range<usize> {
    let at = at.min(text.len());
    let start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let end = text[at..].find('\n').map_or(text.len(), |i| at + i + 1);
    start..end
}

fn char_to_byte(text: &str, chars: usize) -> usize {
    text.char_indices().nth(chars).map_or(text.len(), |(i, _)| i)
}

/// Colors `.chip` source: keywords green, chip names bright, numbers like
/// buses, comments dim. Problems get a wavy-ish underline, and the selected
/// box's line gets a soft background.
fn highlight(text: &str, selected: Option<std::ops::Range<usize>>, problems: &[(usize, usize, bool)]) -> LayoutJob {
    let mut job = LayoutJob::default();
    let font = FontId::monospace(13.0);
    let bytes = text.as_bytes();
    let mut i = 0;
    let mut previous_word = "";
    while i < bytes.len() {
        let start = i;
        let c = bytes[i];
        let mut color = theme::TEXT;
        if c == b'/' && bytes.get(i + 1) == Some(&b'/') {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            color = theme::MUTED;
        } else if c == b'\n' {
            i += 1;
        } else if c == b' ' || c == b'\t' || c == b'\r' {
            while i < bytes.len() && matches!(bytes[i], b' ' | b'\t' | b'\r') {
                i += 1;
            }
        } else if c.is_ascii_digit() {
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            color = theme::BUS;
        } else if c.is_ascii_alphabetic() || c == b'_' {
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let word = &text[start..i];
            let line_start = text[..start].rfind('\n').map_or(0, |n| n + 1);
            let first_on_line = text[line_start..start].trim().is_empty();
            let next = text[i..].trim_start_matches([' ', '\t']).chars().next();
            color = if matches!(word, "chip" | "test") && first_on_line
                || matches!(word, "in" | "out" | "wire")
                    && first_on_line
                    && next.is_some_and(|n| n.is_ascii_alphabetic())
            {
                theme::ONE
            } else if next == Some('(') || matches!(previous_word, "chip" | "test") {
                theme::SELECTED
            } else {
                theme::TEXT
            };
            previous_word = word;
            push(&mut job, &text[start..i], &font, color, start, &selected, problems);
            continue;
        } else {
            i += text[i..].chars().next().map_or(1, char::len_utf8);
            color = theme::MUTED;
        }
        push(&mut job, &text[start..i], &font, color, start, &selected, problems);
    }
    job
}

fn push(
    job: &mut LayoutJob,
    piece: &str,
    font: &FontId,
    color: egui::Color32,
    start: usize,
    selected: &Option<std::ops::Range<usize>>,
    problems: &[(usize, usize, bool)],
) {
    let end = start + piece.len();
    let background = match selected {
        Some(line) if line.contains(&start) => theme::SELECTED.gamma_multiply(0.12),
        _ => egui::Color32::TRANSPARENT,
    };
    let underline = problems
        .iter()
        .find(|(s, e, _)| start < *e && *s < end && !piece.trim().is_empty())
        .map_or(Stroke::NONE, |(_, _, error)| Stroke::new(1.5, if *error { theme::UNKNOWN } else { theme::WARNING }));
    job.append(piece, 0.0, TextFormat { font_id: font.clone(), color, background, underline, ..Default::default() });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlighting_covers_every_byte() {
        let text = "// hi\nchip A {\n  in  a[4]\n  y = Not(a) // x\n}\n";
        let job = highlight(text, Some(line_range(text, 20)), &[(30, 33, true)]);
        assert_eq!(job.text, text);
        let colors: Vec<(String, egui::Color32)> = job
            .sections
            .iter()
            .map(|s| (text[s.byte_range.start.0..s.byte_range.end.0].to_string(), s.format.color))
            .collect();
        assert!(colors.contains(&("chip".into(), theme::ONE)));
        assert!(colors.contains(&("A".into(), theme::SELECTED)));
        assert!(colors.contains(&("Not".into(), theme::SELECTED)));
        assert!(colors.contains(&("4".into(), theme::BUS)));
        assert!(colors.contains(&("// x".into(), theme::MUTED)));
    }
}
