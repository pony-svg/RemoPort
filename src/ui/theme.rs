use egui::{Color32, Context, RichText, Stroke};
use remoport_core::domain::Appearance;

pub struct Theme {
    pub appearance: Appearance,
    pub bg_dark: Color32,
    pub sidebar_bg: Color32,
    pub card_bg: Color32,
    pub card_border: Color32,
    pub card_hover: Color32,
    /// Ends of the diagonal shade behind a PC card's illustration.
    pub thumb_from: Color32,
    pub thumb_to: Color32,
    pub nav_selected: Color32,
    pub nav_hover: Color32,
    pub accent: Color32,
    pub text_primary: Color32,
    pub text_secondary: Color32,
    pub divider: Color32,
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

impl Theme {
    pub fn dark() -> Self {
        Self {
            appearance: Appearance::Dark,
            bg_dark: Color32::from_rgb(0x20, 0x20, 0x20),
            sidebar_bg: Color32::from_rgb(0x18, 0x18, 0x18),
            card_bg: Color32::from_rgb(0x2d, 0x2d, 0x2d),
            card_border: Color32::from_rgb(0x3a, 0x3a, 0x3a),
            card_hover: Color32::from_rgb(0x38, 0x38, 0x38),
            thumb_from: Color32::from_rgb(0x0f, 0x4f, 0x4a),
            thumb_to: Color32::from_rgb(0x1b, 0x2a, 0x3d),
            nav_selected: Color32::from_rgb(0x32, 0x32, 0x32),
            nav_hover: Color32::from_rgb(0x26, 0x26, 0x26),
            // Remoport accent (provisional): a teal of its own.
            accent: Color32::from_rgb(0x1f, 0xa8, 0x9b),
            text_primary: Color32::from_rgb(0xf3, 0xf3, 0xf3),
            text_secondary: Color32::from_rgb(0xaa, 0xaa, 0xaa),
            divider: Color32::from_rgb(0x2e, 0x2e, 0x2e),
        }
    }

    pub fn light() -> Self {
        Self {
            appearance: Appearance::Light,
            bg_dark: Color32::from_rgb(0xfa, 0xfa, 0xfb),
            sidebar_bg: Color32::from_rgb(0xf0, 0xf1, 0xf3),
            card_bg: Color32::from_rgb(0xff, 0xff, 0xff),
            card_border: Color32::from_rgb(0xe1, 0xe4, 0xe8),
            card_hover: Color32::from_rgb(0xf5, 0xf7, 0xf8),
            thumb_from: Color32::from_rgb(0x7f, 0xd6, 0xcb),
            thumb_to: Color32::from_rgb(0x9c, 0xb8, 0xe0),
            nav_selected: Color32::from_rgb(0xff, 0xff, 0xff),
            nav_hover: Color32::from_rgb(0xe6, 0xe8, 0xeb),
            // The same teal, darker, so it keeps its contrast on white.
            accent: Color32::from_rgb(0x12, 0x84, 0x7a),
            text_primary: Color32::from_rgb(0x1d, 0x1f, 0x22),
            text_secondary: Color32::from_rgb(0x5f, 0x66, 0x6d),
            divider: Color32::from_rgb(0xdd, 0xe0, 0xe4),
        }
    }

    pub fn for_appearance(appearance: Appearance) -> Self {
        match appearance {
            Appearance::Dark => Self::dark(),
            Appearance::Light => Self::light(),
        }
    }

    /// Applies the palette to egui's own widgets: selections, links, menus
    /// and dialogs pick up the accent instead of egui's default blue.
    pub fn apply(&self, ctx: &Context) {
        let mut visuals = match self.appearance {
            Appearance::Dark => egui::Visuals::dark(),
            Appearance::Light => egui::Visuals::light(),
        };
        visuals.selection.bg_fill = self.accent;
        visuals.selection.stroke = Stroke::new(1.0_f32, Color32::WHITE);
        visuals.hyperlink_color = self.accent;
        visuals.window_rounding = egui::Rounding::same(10.0);
        visuals.menu_rounding = egui::Rounding::same(8.0);
        ctx.set_visuals(visuals);
    }

    pub fn card_stroke(&self) -> Stroke {
        Stroke::new(1.0_f32, self.card_border)
    }

    /// The filled accent button for the main action of a view or dialog.
    pub fn primary_button(&self, text: &str) -> egui::Button<'static> {
        egui::Button::new(RichText::new(text.to_owned()).color(Color32::WHITE)).fill(self.accent)
    }
}
