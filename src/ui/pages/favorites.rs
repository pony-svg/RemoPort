//! Favorites page: quick one-off connect plus starred PCs and apps.

use crate::ui::app::RemoportApp;
use crate::ui::i18n::t;
use crate::ui::icons::Icon;
use crate::ui::widgets;
use egui::{RichText, Ui};
use remoport_core::domain::{DeviceBookmark, RemoteAppItem};

pub(crate) fn show(app: &mut RemoportApp, ui: &mut Ui) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Quick connect").color(app.theme.text_secondary));
        widgets::quick_connect(app, ui, 260.0);
    });
    ui.add_space(18.0);

    let pcs: Vec<DeviceBookmark> = app
        .state
        .bookmarks
        .iter()
        .filter(|b| b.is_favorite)
        .cloned()
        .collect();
    let apps: Vec<RemoteAppItem> = app
        .state
        .remote_apps
        .iter()
        .filter(|a| a.is_favorite)
        .cloned()
        .collect();

    if pcs.is_empty() && apps.is_empty() {
        widgets::show_empty_state(
            app,
            ui,
            Icon::Star,
            t("No favorites yet"),
            t("Open the ··· menu of a PC or an app and add it to your favorites."),
            None,
        );
        return;
    }
    if !pcs.is_empty() && widgets::section(app, ui, "favorites:pcs", "PCs", pcs.len()) {
        widgets::card_row(ui, |ui| {
            for pc in &pcs {
                widgets::show_pc_card(app, ui, pc);
            }
        });
        ui.add_space(18.0);
    }
    if !apps.is_empty() && widgets::section(app, ui, "favorites:apps", t("Apps"), apps.len()) {
        widgets::card_row(ui, |ui| {
            for item in &apps {
                widgets::show_app_card(app, ui, item);
            }
        });
    }
}
