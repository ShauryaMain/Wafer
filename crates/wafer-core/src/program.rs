//! Programs a `Rom` can load: Crumb assembly (`.asm`), assembled on the
//! spot, or plain hex words (`.hex`).

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use crate::diag::{Diagnostic, FileId, SourceMap, Span};
use crate::gate::ROM_WIDTH;

#[derive(Clone, Debug)]
pub struct RomProgram {
    pub path: PathBuf,
    pub file: FileId,
    /// The ROM's contents from address 0.
    pub words: Arc<Vec<u64>>,
    /// For `.asm` files: the assembled program, with its symbols and where
    /// each word came from.
    pub assembled: Option<crumb::Program>,
}

/// Files a `Rom` can load.
pub fn is_program(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "asm" || ext == "hex")
}

/// Where `file: "name"` on a line of `chip_file` points: next to the chip.
pub fn resolve(chip_file: &Path, name: &str) -> PathBuf {
    normalize(&chip_file.parent().unwrap_or(Path::new("")).join(name))
}

/// Tidies `a/./b/../c` into `a/c` without touching the disk.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir if matches!(out.components().next_back(), Some(Component::Normal(_))) => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

pub fn load(sources: &SourceMap, file: FileId) -> Result<RomProgram, Vec<Diagnostic>> {
    let source = sources.get(file);
    let path = source.path.clone();
    let is_hex = path.extension().is_some_and(|ext| ext == "hex");
    if is_hex {
        let words = parse_hex(file, &source.text)?;
        return Ok(RomProgram { path, file, words: Arc::new(words), assembled: None });
    }
    match crumb::assemble(&source.text) {
        Ok(program) => {
            let words = program.words.iter().map(|&w| u64::from(w)).collect();
            Ok(RomProgram { path, file, words: Arc::new(words), assembled: Some(program) })
        }
        Err(errors) => Err(errors
            .into_iter()
            .map(|e| {
                let mut d = Diagnostic::error(Span::new(file, e.start, e.end), e.message);
                d.label = e.label;
                d.notes = e.notes;
                d
            })
            .collect()),
    }
}

/// Words in hex separated by spaces or new lines, like `wafer asm -o`
/// writes. `//` starts a comment.
fn parse_hex(file: FileId, text: &str) -> Result<Vec<u64>, Vec<Diagnostic>> {
    let (mut words, mut errors) = (Vec::new(), Vec::new());
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let code = line.split("//").next().unwrap_or("");
        let mut pos = 0;
        for word in code.split_whitespace() {
            let at = offset + pos + code[pos..].find(word).unwrap_or(0);
            pos = at - offset + word.len();
            let digits = word.strip_prefix("0x").or_else(|| word.strip_prefix("0X")).unwrap_or(word);
            match u64::from_str_radix(&digits.replace('_', ""), 16) {
                Ok(v) if v >> ROM_WIDTH == 0 => words.push(v),
                Ok(_) => errors.push(Diagnostic::error(
                    Span::new(file, at, at + word.len()),
                    format!("`{word}` doesn't fit in a {ROM_WIDTH}-bit word"),
                )),
                Err(_) => errors.push(
                    Diagnostic::error(Span::new(file, at, at + word.len()), format!("`{word}` isn't a hex number"))
                        .note("a .hex file is hex words separated by spaces, like `8005 6d00`"),
                ),
            }
        }
        offset += line.len();
    }
    if errors.is_empty() { Ok(words) } else { Err(errors) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_paths() {
        assert_eq!(normalize(Path::new("./chips/../chips/a.asm")), PathBuf::from("chips/a.asm"));
        assert_eq!(resolve(Path::new("chips/Cpu.chip"), "../games/pong.asm"), PathBuf::from("games/pong.asm"));
        assert_eq!(resolve(Path::new("Cpu.chip"), "pong.asm"), PathBuf::from("pong.asm"));
    }

    #[test]
    fn reads_hex_and_assembly() {
        let mut sources = SourceMap::default();
        let hex = sources.add("a.hex".into(), "// two words\n8005 0x6d00\n".into());
        assert_eq!(*load(&sources, hex).unwrap().words, vec![0x8005, 0x6d00]);
        let bad = sources.add("b.hex".into(), "12 zz 10000".into());
        let errors = load(&sources, bad).unwrap_err();
        assert_eq!(errors.len(), 2);
        assert_eq!(sources.slice(errors[0].span), "zz");
        let asm = sources.add("c.asm".into(), ": main 5 halt ;".into());
        let program = load(&sources, asm).unwrap();
        assert_eq!(program.words.len(), 3, "jump main, lit 5, halt");
        assert!(program.assembled.is_some());
        let broken = sources.add("d.asm".into(), ": main 5 hlat ;".into());
        let errors = load(&sources, broken).unwrap_err();
        assert_eq!(errors[0].label.as_deref(), Some("did you mean `halt`?"));
        assert_eq!(sources.slice(errors[0].span), "hlat");
    }
}
