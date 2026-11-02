use egui::{Color32, Stroke};

pub struct Theme {
    pub bg_dark: Color32,
    pub sidebar_bg: Color32,
    pub card_bg: Color32,
    pub card_border: Color32,
    pub card_hover: Color32,
    pub accent: Color32,
    pub text_primary: Color32,
    pub text_secondary: Color32,
    pub divider: Color32,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            bg_dark: Color32::from_rgb(0x20, 0x20, 0x20),
            sidebar_bg: Color32::from_rgb(0x18, 0x18, 0x18),
            card_bg: Color32::from_rgb(0x2d, 0x2d, 0x2d),
            card_border: Color32::from_rgb(0x3a, 0x3a, 0x3a),
            card_hover: Color32::from_rgb(0x38, 0x38, 0x38),
            // Remoport accent (provisional): a teal of its own.
            accent: Color32::from_rgb(0x1f, 0xa8, 0x9b),
            text_primary: Color32::from_rgb(0xf3, 0xf3, 0xf3),
            text_secondary: Color32::from_rgb(0xaa, 0xaa, 0xaa),
            divider: Color32::from_rgb(0x2e, 0x2e, 0x2e),
        }
    }
}

impl Theme {
    pub fn card_stroke(&self) -> Stroke {
        Stroke::new(1.0_f32, self.card_border)
    }

    pub fn card_rounding(&self) -> egui::Rounding {
        egui::Rounding::same(8.0)
    }
}
