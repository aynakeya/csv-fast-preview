mod constants;
mod events;
mod fonts;
mod format;
mod state;
mod ui;

use state::CsvFastViewApp;
use std::path::PathBuf;

pub fn run(initial_path: Option<PathBuf>) -> eframe::Result<()> {
    let options = eframe::NativeOptions::default();
    eframe::run_native(
        "CSV Fast View",
        options,
        Box::new(|cc| {
            fonts::install_cjk_fallback(&cc.egui_ctx);
            let mut app = CsvFastViewApp::default();
            if let Some(path) = initial_path {
                app.open_path(path);
            }
            Ok(Box::new(app))
        }),
    )
}
