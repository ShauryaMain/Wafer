//! A project is a folder of `.chip` files that can all use each other, plus
//! the programs (`.asm` and `.hex` files) their `Rom`s load.

use std::io;
use std::path::{Path, PathBuf};

use crate::ast::{self, ExprKind, StmtValue};
use crate::diag::{Diagnostic, FileId, SourceMap};
use crate::parser::parse;
use crate::program::{self, is_program};

pub struct Project {
    pub sources: SourceMap,
    pub files: Vec<(FileId, ast::File)>,
    /// Program files, which aren't parsed as chips.
    pub programs: Vec<FileId>,
    /// Syntax errors from parsing.
    pub diagnostics: Vec<Diagnostic>,
}

impl Project {
    /// Loads every `.chip`, `.asm` and `.hex` file under `path`, or just
    /// `path` if it's a file. Programs a `Rom` names from somewhere else
    /// are read too.
    pub fn load(path: &Path) -> io::Result<Project> {
        let mut paths = Vec::new();
        if path.is_dir() {
            collect_files(path, &mut paths)?;
            paths.sort();
        } else {
            paths.push(path.to_path_buf());
        }
        let mut sources = Vec::new();
        for path in paths {
            let text = std::fs::read_to_string(&path)?;
            sources.push((path, text));
        }
        let mut project = Project::from_sources(sources);
        for path in project.rom_files() {
            if project.program(&path).is_none()
                && let Ok(text) = std::fs::read_to_string(&path)
            {
                project.add_program(path, text);
            }
        }
        Ok(project)
    }

    /// Builds a project from file contents. Paths ending in `.asm` or
    /// `.hex` are programs; everything else is parsed as chips.
    pub fn from_sources(sources: Vec<(PathBuf, String)>) -> Project {
        let mut project =
            Project { sources: SourceMap::default(), files: Vec::new(), programs: Vec::new(), diagnostics: Vec::new() };
        for (path, text) in sources {
            if is_program(&path) {
                project.add_program(path, text);
                continue;
            }
            let id = project.sources.add(path, text);
            let file = parse(id, &project.sources.get(id).text, &mut project.diagnostics);
            project.files.push((id, file));
        }
        project
    }

    fn add_program(&mut self, path: PathBuf, text: String) {
        let id = self.sources.add(path, text);
        self.programs.push(id);
    }

    /// The loaded program at `path`, if there is one.
    pub fn program(&self, path: &Path) -> Option<FileId> {
        let want = program::normalize(path);
        self.programs.iter().copied().find(|&id| program::normalize(&self.sources.get(id).path) == want)
    }

    /// Every program a `Rom` line names, resolved next to its chip file.
    pub fn rom_files(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for (id, file) in &self.files {
            let chip_path = &self.sources.get(*id).path;
            for stmt in file.chips.iter().flat_map(|c| &c.stmts) {
                let StmtValue::Call(call) = &stmt.value else { continue };
                for arg in &call.args {
                    if let ExprKind::Str(name) = &arg.value.kind {
                        let path = program::resolve(chip_path, name);
                        if !out.contains(&path) {
                            out.push(path);
                        }
                    }
                }
            }
        }
        out
    }

    pub fn chips(&self) -> impl Iterator<Item = &ast::ChipDef> {
        self.files.iter().flat_map(|(_, file)| &file.chips)
    }

    pub fn tests(&self) -> impl Iterator<Item = &ast::TestDef> {
        self.files.iter().flat_map(|(_, file)| &file.tests)
    }
}

/// Recurses into subfolders, skipping hidden ones and build output.
fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.starts_with('.') || name == "target" || name == "node_modules" {
            continue;
        }
        if path.is_dir() {
            collect_files(&path, out)?;
        } else if path.extension().is_some_and(|ext| ext == "chip") || is_program(&path) {
            out.push(path);
        }
    }
    Ok(())
}
