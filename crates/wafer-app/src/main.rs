//! Wafer: a "Blender for computer chips". Opens a folder of `.chip` files
//! (default: `chips/` if it exists, otherwise the current folder).

use std::path::{Path, PathBuf};

use wafer_app::app::WaferApp;

fn main() -> eframe::Result {
    let dir = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| if Path::new("chips").is_dir() { PathBuf::from("chips") } else { PathBuf::from(".") });
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Wafer")
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([900.0, 560.0]),
        ..Default::default()
    };
    eframe::run_native("Wafer", options, Box::new(move |cc| Ok(Box::new(WaferApp::new(cc, dir)))))
}
