//! Saved canvas positions. `Adder.chip` keeps its positions in
//! `Adder.layout.json` next to it:
//!
//! ```json
//! {
//!   "HalfAdder": {
//!     "in:a": [40, 120],
//!     "xor1": [200, 100]
//!   }
//! }
//! ```
//!
//! Each chip maps node keys (`in:a`, `out:sum`, or a box's name) to the
//! box's top-left corner. Keys are sorted so diffs stay small.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LayoutFile {
    pub chips: BTreeMap<String, BTreeMap<String, (f32, f32)>>,
}

impl LayoutFile {
    pub fn path_for(chip_file: &Path) -> PathBuf {
        chip_file.with_extension("layout.json")
    }

    /// Reads a layout file. A missing file is just an empty layout.
    pub fn load(path: &Path) -> Result<LayoutFile, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => LayoutFile::parse(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(LayoutFile::default()),
            Err(e) => Err(e.to_string()),
        }
    }

    pub fn parse(text: &str) -> Result<LayoutFile, String> {
        let raw: BTreeMap<String, BTreeMap<String, [f64; 2]>> =
            serde_json::from_str(text).map_err(|e| format!("this layout file isn't valid: {e}"))?;
        let chips = raw
            .into_iter()
            .map(|(chip, nodes)| (chip, nodes.into_iter().map(|(k, [x, y])| (k, (x as f32, y as f32))).collect()))
            .collect();
        Ok(LayoutFile { chips })
    }

    /// One node per line, coordinates rounded to whole units.
    pub fn to_json(&self) -> String {
        let quote = |s: &str| serde_json::to_string(s).expect("strings always serialize");
        let chips: Vec<String> = self
            .chips
            .iter()
            .filter(|(_, nodes)| !nodes.is_empty())
            .map(|(chip, nodes)| {
                let lines: Vec<String> = nodes
                    .iter()
                    .map(|(key, (x, y))| format!("    {}: [{}, {}]", quote(key), x.round(), y.round()))
                    .collect();
                format!("  {}: {{\n{}\n  }}", quote(chip), lines.join(",\n"))
            })
            .collect();
        if chips.is_empty() { "{}\n".into() } else { format!("{{\n{}\n}}\n", chips.join(",\n")) }
    }

    pub fn positions(&self, chip: &str) -> Option<HashMap<String, (f32, f32)>> {
        self.chips.get(chip).map(|nodes| nodes.iter().map(|(k, &p)| (k.clone(), p)).collect())
    }

    pub fn set_positions(&mut self, chip: &str, positions: impl IntoIterator<Item = (String, (f32, f32))>) {
        let nodes = self.chips.entry(chip.to_string()).or_default();
        for (key, (x, y)) in positions {
            nodes.insert(key, (x.round(), y.round()));
        }
    }

    pub fn rename_key(&mut self, chip: &str, old: &str, new: &str) {
        if let Some(nodes) = self.chips.get_mut(chip)
            && let Some(position) = nodes.remove(old)
        {
            nodes.insert(new.to_string(), position);
        }
    }

    pub fn remove_key(&mut self, chip: &str, key: &str) {
        if let Some(nodes) = self.chips.get_mut(chip) {
            nodes.remove(key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_json() {
        let mut layout = LayoutFile::default();
        layout.set_positions("HalfAdder", [("in:a".to_string(), (40.4, 120.0)), ("xor1".to_string(), (200.0, 99.6))]);
        let json = layout.to_json();
        assert_eq!(json, "{\n  \"HalfAdder\": {\n    \"in:a\": [40, 120],\n    \"xor1\": [200, 100]\n  }\n}\n");
        assert_eq!(LayoutFile::parse(&json).unwrap(), layout);
    }

    #[test]
    fn missing_file_is_empty() {
        assert_eq!(LayoutFile::load(Path::new("/nonexistent/x.layout.json")).unwrap(), LayoutFile::default());
    }
}
