use egui::{Color32, Context, RichText, Stroke};

/// Light or dark look, chosen in Settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appearance {
    Dark,
    Light,
}

pub struct Theme {
    pub appearance: Appearance,
    pub bg_dark: Color32,
    pub sidebar_bg: Color32,
    pub card_bg: Color32,
    pub card_border: Color32,
    pub card_hover: Color32,
    pub thumb_bg: Color32,
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
            thumb_bg: Color32::from_rgb(0x16, 0x1c, 0x24),
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
            bg_dark: Color32::from_rgb(0xf6, 0xf7, 0xf8),
            sidebar_bg: Color32::from_rgb(0xeb, 0xed, 0xef),
            card_bg: Color32::from_rgb(0xff, 0xff, 0xff),
            card_border: Color32::from_rgb(0xd9, 0xdc, 0xe0),
            card_hover: Color32::from_rgb(0xf1, 0xf3, 0xf5),
            thumb_bg: Color32::from_rgb(0xe4, 0xe9, 0xee),
            nav_selected: Color32::from_rgb(0xdc, 0xdf, 0xe3),
            nav_hover: Color32::from_rgb(0xe3, 0xe6, 0xe9),
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

    pub fn card_rounding(&self) -> egui::Rounding {
        egui::Rounding::same(8.0)
    }

    /// The filled accent button for the main action of a view or dialog.
    pub fn primary_button(&self, text: &str) -> egui::Button<'static> {
        egui::Button::new(RichText::new(text.to_owned()).color(Color32::WHITE)).fill(self.accent)
    }
}
