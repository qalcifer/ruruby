//! `ruruby-ui app.rb [--eval CODE]... [--screenshot out.png]`: runs a
//! ruruby UI script in a window (egui). With `--screenshot`, it renders a
//! few frames, runs the `--eval` code (e.g. `$__ui_root.find(:plus).emit(:click)`),
//! saves the window as a PNG and exits.

use eframe::egui;
use ruruby_egui::UiApp;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

struct Shot {
    app: UiApp,
    evals: Vec<String>,
    path: Option<String>,
    frame: u32,
    done: Arc<AtomicBool>,
}

impl eframe::App for Shot {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default_margins().show(ui, |ui| self.app.show(ui));
        let Some(path) = self.path.clone() else { return };
        self.frame += 1;
        let ctx = ui.ctx().clone();
        if self.frame == 3 {
            for code in std::mem::take(&mut self.evals) {
                if let Err(e) = self.app.interp.eval(&code) {
                    self.app.error = Some(format!("--eval: {e}"));
                }
            }
            self.app.sync();
        }
        if self.frame == 8 {
            let done = self.done.clone();
            ctx.request_screenshot(move |image| {
                let image = ruruby_egui::trim_bottom(&image, 48);
                match ruruby_egui::write_png(&path, &image) {
                    Ok(()) => eprintln!("saved {path} ({}x{})", image.size[0], image.size[1]),
                    Err(e) => eprintln!("{path}: {e}"),
                }
                done.store(true, Ordering::SeqCst);
            });
        }
        if self.done.load(Ordering::SeqCst) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ctx.request_repaint();
    }
}

fn main() -> eframe::Result {
    let mut args = std::env::args().skip(1);
    let (mut file, mut evals, mut path) = (None, vec![], None);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--eval" => evals.extend(args.next()),
            "--screenshot" => path = args.next(),
            _ => file = Some(a),
        }
    }
    let Some(file) = file else {
        eprintln!("usage: ruruby-ui app.rb [--eval CODE]... [--screenshot out.png]");
        std::process::exit(2);
    };
    let app = match UiApp::load(&file) {
        Ok(app) => app,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    let (title, w, h) = app.window().unwrap_or_else(|| (file.clone(), 480.0, 360.0));
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([w, h]).with_title(title.clone()),
        ..Default::default()
    };
    let shot = Shot { app, evals, path, frame: 0, done: Arc::new(AtomicBool::new(false)) };
    eframe::run_native(&title, options, Box::new(|_cc| Ok(Box::new(shot))))
}
