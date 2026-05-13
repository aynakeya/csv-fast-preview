mod constants;
mod events;
mod fonts;
mod format;
mod state;
mod ui;

use state::CsvFastViewApp;

pub fn run() -> eframe::Result<()> {
    let options = eframe::NativeOptions::default();
    eframe::run_native(
        "CSV Fast View",
        options,
        Box::new(|cc| {
            fonts::install_cjk_fallback(&cc.egui_ctx);
            Ok(Box::<CsvFastViewApp>::default())
        }),
    )
}
