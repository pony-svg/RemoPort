mod app;
mod dialogs;
mod fonts;
mod icons;
mod model;
mod session;
mod theme;

use app::RemoportApp;
use eframe::egui;
use std::env;
use std::path::PathBuf;

/// Application ID: names the desktop entry, icon and metainfo, and lets
/// Wayland compositors match the window to them.
const APP_ID: &str = "app.remoport.Remoport";

struct AppRunner {
    app: RemoportApp,
    screenshot_path: Option<PathBuf>,
    frame_count: u32,
    screenshot_taken: bool,
}

impl AppRunner {
    pub fn new(screenshot_path: Option<PathBuf>) -> Self {
        Self {
            app: RemoportApp::load(),
            screenshot_path,
            frame_count: 0,
            screenshot_taken: false,
        }
    }
}

impl eframe::App for AppRunner {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Draw the application
        self.app.update(ctx);

        if let Some(ref target_path) = self.screenshot_path {
            self.frame_count += 1;

            // Keep repainting until we reach 120 frames
            ctx.request_repaint();

            // At frame 120, request screenshot
            if self.frame_count == 120 {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot);
            }

            // Inspect raw events for the screenshot reply
            for event in &ctx.input(|i| i.raw.events.clone()) {
                if let egui::Event::Screenshot { image, .. } = event {
                    let [w, h] = image.size;
                    let mut rgba_bytes = Vec::with_capacity(w * h * 4);
                    for px in &image.pixels {
                        rgba_bytes.push(px.r());
                        rgba_bytes.push(px.g());
                        rgba_bytes.push(px.b());
                        rgba_bytes.push(px.a());
                    }

                    if let Some(parent) = target_path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }

                    if let Err(err) = image::save_buffer(
                        target_path,
                        &rgba_bytes,
                        w as u32,
                        h as u32,
                        image::ExtendedColorType::Rgba8,
                    ) {
                        eprintln!("Failed to save screenshot: {}", err);
                    } else {
                        println!("Screenshot successfully saved to {:?}", target_path);
                    }

                    self.screenshot_taken = true;
                    std::process::exit(0);
                }
            }

            // Fallback safety timeout if no screenshot event arrives after 140 frames
            if self.frame_count >= 140 && !self.screenshot_taken {
                eprintln!("Timeout waiting for screenshot event at frame {}", self.frame_count);
                std::process::exit(0);
            }
        }
    }
}

fn main() -> eframe::Result<()> {
    let args: Vec<String> = env::args().collect();
    let mut screenshot_path = None;

    let mut i = 1;
    while i < args.len() {
        if args[i] == "--screenshot" && i + 1 < args.len() {
            screenshot_path = Some(PathBuf::from(&args[i + 1]));
            i += 2;
        } else {
            i += 1;
        }
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Remoport")
            .with_app_id(APP_ID)
            .with_inner_size([1200.0, 760.0])
            .with_min_inner_size([860.0, 540.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Remoport",
        options,
        Box::new(|cc| {
            fonts::install(&cc.egui_ctx);
            Ok(Box::new(AppRunner::new(screenshot_path)))
        }),
    )
}
