mod constants;
mod events;
mod format;
mod state;
mod ui;

use state::CsvFastViewApp;

pub fn run() -> eframe::Result<()> {
    let options = eframe::NativeOptions::default();
    eframe::run_native(
        "CSV Fast View",
        options,
        Box::new(|_cc| Ok(Box::<CsvFastViewApp>::default())),
    )
}
