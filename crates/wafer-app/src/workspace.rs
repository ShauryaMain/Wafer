//! The project on disk: loading, live reload when files change, edits from
//! the canvas, and undo/redo.
//!
//! The `.chip` files are the source of truth. Every canvas edit is written
//! to disk straight away and the project is re-read, so the app always
//! shows exactly what the files say, whether the change came from the
//! canvas, from Claude, or from a text editor.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use wafer_core::edit::{self, Edit, PortDir, is_valid_name};
use wafer_core::layout_file::LayoutFile;
use wafer_core::{Design, Project};

pub struct Problem {
    pub error: bool,
    /// The file and byte range it points at.
    pub file: PathBuf,
    pub start: usize,
    pub end: usize,
    pub location: String,
    pub message: String,
    /// The full message with the source line, as `wafer check` prints it.
    pub detail: String,
}

pub struct Workspace {
    pub dir: PathBuf,
    pub project: Project,
    pub design: Design,
    pub problems: Vec<Problem>,
    /// Set when a file has a syntax error. The app then keeps showing the
    /// last version that read cleanly, and canvas editing pauses.
    pub stale: Option<String>,
    /// The latest message for the status bar, and whether it's an error.
    pub status: Option<(String, bool)>,
    /// Goes up whenever the chips' text changes, so views know to rebuild.
    pub generation: u64,
    /// Goes up whenever saved positions change.
    pub layout_generation: u64,
    /// Goes up on every reload, even when nothing parsed differently.
    pub revision: u64,
    layouts: HashMap<PathBuf, LayoutFile>,
    seen: Vec<(PathBuf, SystemTime, u64)>,
    text_hash: u64,
    last_poll: Instant,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
}

/// File contents from before a change. `None` means the file didn't exist.
struct Snapshot {
    files: Vec<(PathBuf, Option<String>)>,
}

impl Snapshot {
    fn take(paths: &[PathBuf]) -> Snapshot {
        Snapshot { files: paths.iter().map(|p| (p.clone(), std::fs::read_to_string(p).ok())).collect() }
    }

    fn paths(&self) -> Vec<PathBuf> {
        self.files.iter().map(|(p, _)| p.clone()).collect()
    }

    fn restore(&self) {
        for (path, content) in &self.files {
            let _ = match content {
                Some(text) => std::fs::write(path, text),
                None => std::fs::remove_file(path),
            };
        }
    }
}

impl Workspace {
    pub fn open(dir: PathBuf) -> Workspace {
        let empty = Project::from_sources(Vec::new());
        let design = Design::check(&empty);
        let mut ws = Workspace {
            dir,
            project: empty,
            design,
            problems: Vec::new(),
            stale: None,
            status: None,
            generation: 0,
            layout_generation: 0,
            revision: 0,
            layouts: HashMap::new(),
            seen: Vec::new(),
            text_hash: 0,
            last_poll: Instant::now(),
            undo: Vec::new(),
            redo: Vec::new(),
        };
        ws.reload();
        ws
    }

    /// Re-reads every file. If one has a syntax error, the last clean
    /// version stays on screen.
    pub fn reload(&mut self) {
        self.revision += 1;
        self.seen = scan(&self.dir);
        let project = match Project::load(&self.dir) {
            Ok(project) => project,
            Err(e) => {
                self.stale = Some(format!("can't read {}: {e}", self.dir.display()));
                return;
            }
        };
        let design = Design::check(&project);
        // Show paths relative to the project folder.
        let prefix = format!("{}/", self.dir.display());
        let short = |s: String| s.replace(&prefix, "");
        self.problems = project
            .diagnostics
            .iter()
            .chain(&design.diagnostics)
            .map(|d| Problem {
                error: d.is_error(),
                file: project.sources.get(d.span.file).path.clone(),
                start: d.span.start,
                end: d.span.end,
                location: short(project.sources.location(d.span)),
                message: d.message.clone(),
                detail: short(d.render(&project.sources, false)),
            })
            .collect();

        let syntax_error = project.diagnostics.iter().find(|d| d.is_error());
        self.stale = syntax_error.map(|d| format!("{} has a syntax error", short(project.sources.location(d.span))));
        if self.stale.is_some() && self.generation > 0 {
            return;
        }

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for id in project.files.iter().map(|(id, _)| id).chain(&project.programs) {
            project.sources.get(*id).text.hash(&mut hasher);
        }
        let text_hash = hasher.finish();
        if text_hash != self.text_hash || self.generation == 0 {
            self.text_hash = text_hash;
            self.generation += 1;
        }
        self.project = project;
        self.design = design;

        let mut layouts = HashMap::new();
        for (id, _) in &self.project.files {
            let path = LayoutFile::path_for(&self.project.sources.get(*id).path);
            if let Ok(layout) = LayoutFile::load(&path) {
                layouts.insert(path, layout);
            }
        }
        if layouts != self.layouts {
            self.layouts = layouts;
            self.layout_generation += 1;
        }
    }

    /// Checks for changed files a couple of times a second. Returns true if
    /// it reloaded.
    pub fn poll(&mut self) -> bool {
        if self.last_poll.elapsed() < Duration::from_millis(400) {
            return false;
        }
        self.last_poll = Instant::now();
        if scan(&self.dir) == self.seen {
            return false;
        }
        self.reload();
        true
    }

    pub fn chip_file(&self, chip: &str) -> Option<PathBuf> {
        let info = self.design.chips.get(chip)?;
        Some(self.project.sources.get(info.span.file).path.clone())
    }

    /// Saved positions for a chip, if it has been arranged by hand.
    pub fn positions(&self, chip: &str) -> Option<HashMap<String, (f32, f32)>> {
        let path = LayoutFile::path_for(&self.chip_file(chip)?);
        self.layouts.get(&path)?.positions(chip)
    }

    /// Chips in build order: gates first, then the chips made from them.
    pub fn chip_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.design.chips.keys().cloned().collect();
        names.sort_by_key(|n| (self.design.depth(n), n.clone()));
        names
    }

    fn report<T>(&mut self, result: Result<T, String>) -> Option<T> {
        match result {
            Ok(value) => {
                self.status = None;
                Some(value)
            }
            Err(message) => {
                self.status = Some((message, true));
                None
            }
        }
    }

    /// Applies text edits to a chip (and saved positions, `place`) as one
    /// undoable step. Returns false and sets the status message on failure.
    pub fn edit(&mut self, chip: &str, edits: &[Edit], place: &[(String, (f32, f32))]) -> bool {
        let result = self.try_edit(chip, edits, place);
        self.report(result).is_some()
    }

    fn try_edit(&mut self, chip: &str, edits: &[Edit], place: &[(String, (f32, f32))]) -> Result<(), String> {
        if let Some(stale) = &self.stale {
            return Err(format!("{stale}; fix it before editing on the canvas"));
        }
        let chip_path = self.chip_file(chip).ok_or_else(|| format!("there's no chip named `{chip}`"))?;
        let layout_path = LayoutFile::path_for(&chip_path);
        let snapshot = Snapshot::take(&[chip_path.clone(), layout_path.clone()]);
        let mut layout = self.layouts.get(&layout_path).cloned().unwrap_or_default();

        for e in edits {
            let result = match edit::apply(&self.project, &self.design, chip, e) {
                Ok(result) => result,
                Err(message) => {
                    snapshot.restore();
                    self.reload();
                    return Err(message);
                }
            };
            std::fs::write(&chip_path, &result.text).map_err(|e| e.to_string())?;
            match e {
                Edit::RemoveInstance { name } => layout.remove_key(chip, name),
                Edit::RemovePort { dir, name } => layout.remove_key(chip, &port_key(*dir, name)),
                Edit::Rename { old, new } => {
                    layout.rename_key(chip, old, new);
                    for dir in [PortDir::In, PortDir::Out] {
                        layout.rename_key(chip, &port_key(dir, old), &port_key(dir, new));
                    }
                }
                _ => {}
            }
            for (old, new) in &result.renamed {
                layout.rename_key(chip, old, new);
            }
            self.reload();
            if self.stale.is_some() {
                snapshot.restore();
                self.reload();
                return Err("that change would have broken the file, so it was undone".into());
            }
        }
        layout.set_positions(chip, place.iter().cloned());
        self.write_layout(&layout_path, &layout)?;
        self.reload();
        self.undo.push(snapshot);
        self.redo.clear();
        Ok(())
    }

    fn write_layout(&self, path: &Path, layout: &LayoutFile) -> Result<(), String> {
        let empty = layout.chips.values().all(|nodes| nodes.is_empty());
        // (A missing file and an empty layout mean the same thing.)
        if empty && !path.exists() {
            return Ok(());
        }
        std::fs::write(path, layout.to_json()).map_err(|e| e.to_string())
    }

    /// Saves new positions for boxes as one undoable step.
    pub fn move_nodes(&mut self, chip: &str, positions: Vec<(String, (f32, f32))>) {
        let result = (|| {
            let chip_path = self.chip_file(chip).ok_or("that chip is gone")?;
            let layout_path = LayoutFile::path_for(&chip_path);
            let snapshot = Snapshot::take(std::slice::from_ref(&layout_path));
            let mut layout = self.layouts.get(&layout_path).cloned().unwrap_or_default();
            layout.set_positions(chip, positions);
            self.write_layout(&layout_path, &layout)?;
            self.undo.push(snapshot);
            self.redo.clear();
            self.reload();
            Ok::<(), String>(())
        })();
        self.report(result);
    }

    /// Saves text typed in the code panel as one undoable step.
    pub fn write_text(&mut self, path: &Path, text: &str) {
        let snapshot = Snapshot::take(&[path.to_path_buf()]);
        let result = std::fs::write(path, text).map_err(|e| e.to_string());
        if self.report(result).is_some() {
            self.undo.push(snapshot);
            self.redo.clear();
            self.reload();
        }
    }

    /// "Make Chip": moves boxes of `chip` into a new chip `name` in its own
    /// file, as one undoable step. `place` is where the new box goes in the
    /// parent and `inner` keeps the moved boxes' positions inside. Returns
    /// the new box's name.
    pub fn extract(
        &mut self,
        chip: &str,
        boxes: &[String],
        name: &str,
        place: (f32, f32),
        inner: Vec<(String, (f32, f32))>,
    ) -> Option<String> {
        let result = (|| {
            if let Some(stale) = &self.stale {
                return Err(format!("{stale}; fix it first"));
            }
            let out = edit::extract_chip(&self.project, &self.design, chip, boxes, name)?;
            let parent_path = self.project.sources.get(out.parent_file).path.clone();
            let new_path = parent_path.with_file_name(format!("{name}.chip"));
            if new_path.exists() {
                return Err(format!("{} already exists", new_path.display()));
            }
            let parent_layout = LayoutFile::path_for(&parent_path);
            let new_layout = LayoutFile::path_for(&new_path);
            let snapshot =
                Snapshot::take(&[parent_path.clone(), new_path.clone(), parent_layout.clone(), new_layout.clone()]);

            std::fs::write(&new_path, &out.new_text).map_err(|e| e.to_string())?;
            std::fs::write(&parent_path, &out.parent_text).map_err(|e| e.to_string())?;
            let mut layout = self.layouts.get(&parent_layout).cloned().unwrap_or_default();
            for b in boxes {
                layout.remove_key(chip, b);
            }
            layout.set_positions(chip, [(out.instance.clone(), place)]);
            self.write_layout(&parent_layout, &layout)?;
            let mut inside = LayoutFile::default();
            inside.set_positions(name, inner);
            self.write_layout(&new_layout, &inside)?;

            self.reload();
            if self.stale.is_some() {
                snapshot.restore();
                self.reload();
                return Err("making that chip would have broken a file, so it was undone".into());
            }
            self.undo.push(snapshot);
            self.redo.clear();
            Ok(out.instance)
        })();
        self.report(result)
    }

    /// Creates `Name.chip` with an empty chip in it.
    pub fn new_chip(&mut self, name: &str) -> bool {
        let result = (|| {
            if !is_valid_name(name) {
                return Err(format!("`{name}` isn't a valid chip name (use letters, digits and _)"));
            }
            if self.design.chips.contains_key(name) || wafer_core::GateKind::from_name(name).is_some() {
                return Err(format!("there's already a chip named `{name}`"));
            }
            let path = self.dir.join(format!("{name}.chip"));
            if path.exists() {
                return Err(format!("{} already exists", path.display()));
            }
            let snapshot = Snapshot::take(std::slice::from_ref(&path));
            std::fs::write(&path, format!("chip {name} {{\n}}\n")).map_err(|e| e.to_string())?;
            self.undo.push(snapshot);
            self.redo.clear();
            self.reload();
            Ok(())
        })();
        self.report(result).is_some()
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) {
        if let Some(snapshot) = self.undo.pop() {
            self.redo.push(Snapshot::take(&snapshot.paths()));
            snapshot.restore();
            self.reload();
            self.status = Some(("Undone".into(), false));
        }
    }

    pub fn redo(&mut self) {
        if let Some(snapshot) = self.redo.pop() {
            self.undo.push(Snapshot::take(&snapshot.paths()));
            snapshot.restore();
            self.reload();
            self.status = Some(("Redone".into(), false));
        }
    }
}

pub fn port_key(dir: PortDir, name: &str) -> String {
    match dir {
        PortDir::In => format!("in:{name}"),
        PortDir::Out => format!("out:{name}"),
    }
}

/// Modification times of every file the app reads, to notice changes.
fn scan(dir: &Path) -> Vec<(PathBuf, SystemTime, u64)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with('.') || name == "target" || name == "node_modules" {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else if (name.ends_with(".chip")
                || name.ends_with(".layout.json")
                || name.ends_with(".asm")
                || name.ends_with(".hex"))
                && let Ok(meta) = entry.metadata()
            {
                out.push((path, meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), meta.len()));
            }
        }
    }
    out.sort();
    out
}
