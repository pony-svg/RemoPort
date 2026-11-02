use egui::{FontData, FontDefinitions, FontFamily};
use std::path::Path;

/// Font family for titles and labels that need more weight than body text.
pub const MEDIUM: &str = "medium";

/// Noto Sans, where the system has it (most Linux desktops do).
const REGULAR_FILES: &[&str] = &[
    "/usr/share/fonts/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/google-noto/NotoSans-Regular.ttf",
];
const MEDIUM_FILES: &[&str] = &[
    "/usr/share/fonts/noto/NotoSans-Medium.ttf",
    "/usr/share/fonts/truetype/noto/NotoSans-Medium.ttf",
    "/usr/share/fonts/google-noto/NotoSans-Medium.ttf",
];

fn first_readable(paths: &[&str]) -> Option<Vec<u8>> {
    paths
        .iter()
        .map(Path::new)
        .find_map(|path| std::fs::read(path).ok())
}

/// Puts the system's Noto Sans in front of egui's bundled fonts, which stay
/// as the fallback for symbols, and defines the [`MEDIUM`] family. Without
/// Noto Sans everything uses egui's own fonts.
pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let fallback = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();

    if let Some(bytes) = first_readable(REGULAR_FILES) {
        fonts
            .font_data
            .insert("noto-sans".to_owned(), FontData::from_owned(bytes));
        if let Some(family) = fonts.families.get_mut(&FontFamily::Proportional) {
            family.insert(0, "noto-sans".to_owned());
        }
    }

    let mut medium = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or(fallback);
    if let Some(bytes) = first_readable(MEDIUM_FILES) {
        fonts
            .font_data
            .insert("noto-sans-medium".to_owned(), FontData::from_owned(bytes));
        medium.insert(0, "noto-sans-medium".to_owned());
    }
    fonts
        .families
        .insert(FontFamily::Name(MEDIUM.into()), medium);

    ctx.set_fonts(fonts);
}

/// A [`MEDIUM`] font of the given size.
pub fn medium(size: f32) -> egui::FontId {
    egui::FontId::new(size, FontFamily::Name(MEDIUM.into()))
}
