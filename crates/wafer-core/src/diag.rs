//! Source files, spans and friendly error messages.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

pub type FileId = usize;

/// A byte range in one source file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Span {
    pub file: FileId,
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(file: FileId, start: usize, end: usize) -> Span {
        Span { file, start, end }
    }

    /// The smallest span covering both.
    pub fn to(self, other: Span) -> Span {
        Span { file: self.file, start: self.start.min(other.start), end: self.end.max(other.end) }
    }
}

pub struct SourceFile {
    pub path: PathBuf,
    pub text: String,
    line_starts: Vec<usize>,
}

impl SourceFile {
    pub fn new(path: PathBuf, text: String) -> SourceFile {
        let line_starts = std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).collect();
        SourceFile { path, text, line_starts }
    }

    /// 1-based line and column of a byte offset.
    pub fn line_col(&self, offset: usize) -> (usize, usize) {
        let line = match self.line_starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        let start = self.line_starts[line];
        let col = self.text[start..offset.min(self.text.len())].chars().count() + 1;
        (line + 1, col)
    }

    /// The text of a 1-based line, without its newline.
    pub fn line_text(&self, line: usize) -> &str {
        let start = self.line_starts[line - 1];
        let end = self.line_starts.get(line).map_or(self.text.len(), |&next| next - 1);
        self.text[start..end].trim_end_matches('\r')
    }
}

#[derive(Default)]
pub struct SourceMap {
    files: Vec<SourceFile>,
}

impl SourceMap {
    pub fn add(&mut self, path: PathBuf, text: String) -> FileId {
        self.files.push(SourceFile::new(path, text));
        self.files.len() - 1
    }

    pub fn get(&self, id: FileId) -> &SourceFile {
        &self.files[id]
    }

    pub fn slice(&self, span: Span) -> &str {
        &self.files[span.file].text[span.start..span.end]
    }

    pub fn line(&self, span: Span) -> usize {
        self.files[span.file].line_col(span.start).0
    }

    /// `path:line`, for pointing at a test row or a declaration.
    pub fn location(&self, span: Span) -> String {
        format!("{}:{}", display_path(&self.files[span.file].path), self.line(span))
    }
}

/// Paths print relative to the folder `wafer` was run from.
pub fn display_path(path: &Path) -> String {
    path.strip_prefix(".").unwrap_or(path).display().to_string()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
    pub span: Span,
    /// Short text printed under the `^^^` marker.
    pub label: Option<String>,
    pub notes: Vec<String>,
}

impl Diagnostic {
    pub fn error(span: Span, message: impl Into<String>) -> Diagnostic {
        Diagnostic { severity: Severity::Error, message: message.into(), span, label: None, notes: Vec::new() }
    }

    pub fn warning(span: Span, message: impl Into<String>) -> Diagnostic {
        Diagnostic { severity: Severity::Warning, ..Diagnostic::error(span, message) }
    }

    pub fn label(mut self, label: impl Into<String>) -> Diagnostic {
        self.label = Some(label.into());
        self
    }

    pub fn note(mut self, note: impl Into<String>) -> Diagnostic {
        self.notes.push(note.into());
        self
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }

    /// Prints the diagnostic like the Rust compiler does, with the offending
    /// line and a `^^^` marker under the problem.
    pub fn render(&self, sources: &SourceMap, color: bool) -> String {
        let paint = |code: &str, s: &str| {
            if color { format!("\x1b[{code}m{s}\x1b[0m") } else { s.to_string() }
        };
        let (kind, code) = match self.severity {
            Severity::Error => ("error", "1;31"),
            Severity::Warning => ("warning", "1;33"),
        };
        let file = sources.get(self.span.file);
        let (line, col) = file.line_col(self.span.start);
        let text = file.line_text(line);
        let number = line.to_string();
        let pad = " ".repeat(number.len());
        let bar = paint("1;34", "|");

        // Underline the span, clamped to the first line it touches.
        let line_start = file.line_starts[line - 1];
        let from = (self.span.start - line_start).min(text.len());
        let to = (self.span.end.saturating_sub(line_start)).clamp(from, text.len());
        let marks = "^".repeat(text.get(from..to).map_or(1, |s| s.chars().count().max(1)));
        let label = self.label.as_deref().map(|l| format!(" {l}")).unwrap_or_default();

        let mut out = String::new();
        let _ = writeln!(out, "{}: {}", paint(code, kind), paint("1", &self.message));
        let _ = writeln!(out, "{pad}{} {}:{line}:{col}", paint("1;34", "-->"), display_path(&file.path));
        let _ = writeln!(out, "{pad} {bar}");
        let _ = writeln!(out, "{} {bar} {text}", paint("1;34", &number));
        let _ = writeln!(out, "{pad} {bar} {}{}", " ".repeat(col - 1), paint(code, &format!("{marks}{label}")));
        for note in &self.notes {
            let _ = writeln!(out, "{pad} {} note: {note}", paint("1;34", "="));
        }
        out
    }
}

/// The closest candidate to a misspelled name, if any is close enough.
pub fn suggest<'a>(name: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let limit = (name.chars().count() / 3).max(1);
    candidates
        .into_iter()
        .filter(|c| *c != name)
        .map(|c| (edit_distance(&name.to_lowercase(), &c.to_lowercase()), c))
        .filter(|&(d, _)| d <= limit)
        .min_by_key(|&(d, _)| d)
        .map(|(_, c)| c)
}

/// Edits needed to turn `a` into `b`, where swapping two neighboring
/// letters (`Xro` → `Xor`) counts as one edit.
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for j in 0..=b.len() {
        d[0][j] = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j - 1] + cost).min(d[i - 1][j] + 1).min(d[i][j - 1] + 1);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[a.len()][b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggests_close_names() {
        assert_eq!(suggest("Xro", ["Xor", "And", "Nand"]), Some("Xor"));
        assert_eq!(suggest("sm", ["sum", "carry"]), Some("sum"));
        assert_eq!(suggest("banana", ["Xor", "And"]), None);
    }

    #[test]
    fn renders_line_and_marker() {
        let mut sources = SourceMap::default();
        let text = "chip A {\n  x = Xro(a, b)\n}\n";
        let file = sources.add("A.chip".into(), text.into());
        let start = text.find("Xro").unwrap();
        let d = Diagnostic::error(Span::new(file, start, start + 3), "unknown chip `Xro`").label("did you mean `Xor`?");
        let out = d.render(&sources, false);
        assert!(out.contains("A.chip:2:7"), "{out}");
        assert!(out.contains("      ^^^ did you mean `Xor`?"), "{out}");
    }
}
