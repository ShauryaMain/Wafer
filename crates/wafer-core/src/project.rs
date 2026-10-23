//! A project is a folder of `.chip` files that can all use each other.

use std::io;
use std::path::{Path, PathBuf};

use crate::ast;
use crate::diag::{Diagnostic, FileId, SourceMap};
use crate::parser::parse;

pub struct Project {
    pub sources: SourceMap,
    pub files: Vec<(FileId, ast::File)>,
    /// Syntax errors from parsing.
    pub diagnostics: Vec<Diagnostic>,
}

impl Project {
    /// Loads every `.chip` file under `path`, or just `path` if it's a file.
    pub fn load(path: &Path) -> io::Result<Project> {
        let mut paths = Vec::new();
        if path.is_dir() {
            collect_chip_files(path, &mut paths)?;
            paths.sort();
        } else {
            paths.push(path.to_path_buf());
        }
        let mut sources = Vec::new();
        for path in paths {
            let text = std::fs::read_to_string(&path)?;
            sources.push((path, text));
        }
        Ok(Project::from_sources(sources))
    }

    pub fn from_sources(sources: Vec<(PathBuf, String)>) -> Project {
        let mut project =
            Project { sources: SourceMap::default(), files: Vec::new(), diagnostics: Vec::new() };
        for (path, text) in sources {
            let id = project.sources.add(path, text);
            let file = parse(id, &project.sources.get(id).text, &mut project.diagnostics);
            project.files.push((id, file));
        }
        project
    }

    pub fn chips(&self) -> impl Iterator<Item = &ast::ChipDef> {
        self.files.iter().flat_map(|(_, file)| &file.chips)
    }

    pub fn tests(&self) -> impl Iterator<Item = &ast::TestDef> {
        self.files.iter().flat_map(|(_, file)| &file.tests)
    }
}

/// Recurses into subfolders, skipping hidden ones and build output.
fn collect_chip_files(dir: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.starts_with('.') || name == "target" || name == "node_modules" {
            continue;
        }
        if path.is_dir() {
            collect_chip_files(&path, out)?;
        } else if path.extension().is_some_and(|ext| ext == "chip") {
            out.push(path);
        }
    }
    Ok(())
}
