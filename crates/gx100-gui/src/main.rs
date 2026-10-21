//! `gx100-editor` — a graphical editor for the BOSS GX-100.
//!
//! Run with `--sim` to use an in-memory simulated pedal instead of USB MIDI.

mod app;
mod store;
mod worker;

fn main() -> eframe::Result {
    let sim = std::env::args().any(|a| a == "--sim");
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 720.0])
            .with_min_inner_size([700.0, 400.0]),
        ..Default::default()
    };
    eframe::run_native("GX-100 Editor", options, Box::new(move |cc| Ok(Box::new(app::EditorApp::new(cc, sim)))))
}
