//! `ruruby-ui app.rb`: runs a ruruby UI script in a window (egui).

use ruruby_egui::UiApp;

fn main() -> eframe::Result {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: ruruby-ui app.rb  (the script makes a component with new, or defines `component App`)");
        std::process::exit(2);
    };
    let app = match UiApp::load(&path) {
        Ok(app) => app,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    let (title, w, h) = app.window().unwrap_or_else(|| (path.clone(), 480.0, 360.0));
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default().with_inner_size([w, h]).with_title(title.clone()),
        ..Default::default()
    };
    eframe::run_native(&title, options, Box::new(|_cc| Ok(Box::new(app))))
}
