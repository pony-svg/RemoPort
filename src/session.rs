use crate::model::DeviceBookmark;
use egui::{Align2, Color32, FontId, Rounding, Stroke, Ui};

/// FreeRDP clients to try, newest first: FreeRDP 3 renamed its binaries.
const FREERDP_CLIENTS: &[&str] = &["sdl-freerdp3", "xfreerdp3", "wlfreerdp", "xfreerdp"];

pub struct ActiveSession {
    pub bookmark: DeviceBookmark,
    pub open_app: Option<String>,
    pub notification_msg: Option<String>,
}

impl ActiveSession {
    pub fn new(bookmark: DeviceBookmark) -> Self {
        Self {
            bookmark,
            open_app: None,
            notification_msg: None,
        }
    }

    pub fn render(&mut self, ui: &mut Ui, on_disconnect: &mut bool) {
        // Session Top Bar
        ui.horizontal(|ui| {
            ui.visuals_mut().override_text_color = Some(Color32::from_rgb(220, 220, 220));
            ui.label(format!("🖥 {}", self.bookmark.display_title()));
            ui.separator();
            ui.label(format!("Host: {}", self.bookmark.pc_name));
            ui.separator();
            ui.label("Preview — not connected");

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("❌ Close").clicked() {
                    *on_disconnect = true;
                }

                if ui.button("🖥 Open in FreeRDP").clicked() {
                    let host = self.bookmark.pc_name.clone();
                    std::thread::spawn(move || {
                        let target = format!("/v:{host}");
                        for client in FREERDP_CLIENTS {
                            if std::process::Command::new(client).arg(&target).spawn().is_ok() {
                                break;
                            }
                        }
                    });
                    self.notification_msg = Some("Opening the session in FreeRDP…".to_string());
                }
            });
        });

        ui.separator();

        if let Some(ref msg) = self.notification_msg {
            ui.colored_label(Color32::from_rgb(100, 200, 255), format!("ℹ {}", msg));
        }

        // Remote screen area: a placeholder until the RDP engine is connected.
        let available_size = ui.available_size();
        let (rect, _response) = ui.allocate_exact_size(available_size, egui::Sense::click());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, Rounding::ZERO, Color32::from_rgb(18, 22, 28));
        painter.rect_stroke(
            rect.shrink(24.0),
            Rounding::same(12.0),
            Stroke::new(1.0_f32, Color32::from_rgb(48, 54, 62)),
        );

        let target = match self.open_app {
            Some(ref app) => format!("{app} on {}", self.bookmark.pc_name),
            None => self.bookmark.pc_name.clone(),
        };
        painter.text(
            rect.center() - egui::vec2(0.0, 18.0),
            Align2::CENTER_CENTER,
            format!("The remote screen of {target} will appear here"),
            FontId::proportional(16.0),
            Color32::from_rgb(220, 220, 220),
        );
        painter.text(
            rect.center() + egui::vec2(0.0, 12.0),
            Align2::CENTER_CENTER,
            "Remoport's own RDP engine is not connected yet. Use \"Open in FreeRDP\" meanwhile.",
            FontId::proportional(12.5),
            Color32::from_rgb(150, 156, 164),
        );
    }
}
