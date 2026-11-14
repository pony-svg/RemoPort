//! Devices page: saved PCs filtered by group/search and shown as cards or a list.

use crate::ui::app::RemoportApp;
use crate::ui::i18n::{resolution_label, t};
use crate::ui::icons::Icon;
use crate::ui::widgets::{self, EmptyAction};
use egui::{RichText, Ui};
use remoport_core::domain::{DeviceBookmark, SortOption, ViewMode};

pub(crate) fn show_filters(app: &mut RemoportApp, ui: &mut Ui) {
    let mut groups: Vec<String> = app
        .state
        .bookmarks
        .iter()
        .map(|pc| pc.group_name.clone())
        .collect();
    groups.sort_by_key(|g| g.to_lowercase());
    groups.dedup();
    let mut filter = app.group_filter.take();
    widgets::filter_chips(
        &app.theme,
        ui,
        "group_filter",
        t("Group"),
        &groups,
        &mut filter,
    );
    app.group_filter = filter;
}

pub(crate) fn show(app: &mut RemoportApp, ui: &mut Ui) {
    if app.state.bookmarks.is_empty() {
        widgets::show_empty_state(
            app,
            ui,
            Icon::Monitor,
            t("No PCs yet"),
            t("Add a PC by its name or address, or subscribe to your organization's workspace."),
            Some((t("Add a PC"), EmptyAction::AddPc)),
        );
        return;
    }

    let query = app.search_query.trim().to_lowercase();
    let mut pcs: Vec<DeviceBookmark> = app
        .state
        .bookmarks
        .iter()
        .filter(|b| {
            query.is_empty()
                || b.pc_name.to_lowercase().contains(&query)
                || b.friendly_name.to_lowercase().contains(&query)
        })
        .filter(|b| app.group_filter.as_ref().is_none_or(|g| *g == b.group_name))
        .cloned()
        .collect();
    if pcs.is_empty() {
        widgets::show_empty_state(
            app,
            ui,
            Icon::Search,
            t("No PCs match"),
            t("Try another name or address, or show all groups."),
            None,
        );
        return;
    }
    sort_pcs(app.state.sort_option, &mut pcs);

    // One section per group, in the order the groups first appear.
    let mut groups: Vec<(String, Vec<DeviceBookmark>)> = Vec::new();
    for pc in pcs {
        match groups.iter_mut().find(|(name, _)| *name == pc.group_name) {
            Some((_, list)) => list.push(pc),
            None => groups.push((pc.group_name.clone(), vec![pc])),
        }
    }
    for (group, pcs) in &groups {
        // The default group's name is stored in English.
        let shown = if group == "Saved PCs" {
            t("Saved PCs")
        } else {
            group
        };
        if widgets::section(app, ui, &format!("devices:{group}"), shown, pcs.len()) {
            match app.state.view_mode {
                ViewMode::Grid => widgets::card_row(ui, |ui| {
                    for pc in pcs {
                        widgets::show_pc_card(app, ui, pc);
                    }
                }),
                ViewMode::List => show_pc_list(app, ui, group, pcs),
            }
        }
        ui.add_space(18.0);
    }
}

fn sort_pcs(sort_option: SortOption, pcs: &mut [DeviceBookmark]) {
    match sort_option {
        SortOption::DeviceName => pcs.sort_by_key(|pc| pc.display_title().to_lowercase()),
        // Most recent first; PCs never connected to go last.
        SortOption::TimeLastConnected => {
            pcs.sort_by_key(|pc| std::cmp::Reverse(pc.last_connected));
        }
        SortOption::Manual => {}
    }
}

fn show_pc_list(app: &mut RemoportApp, ui: &mut Ui, group: &str, pcs: &[DeviceBookmark]) {
    let mut connect_target = None;
    let mut edit_target = None;
    let mut toggle_fav = None;

    egui::Grid::new(("device_list_table", group))
        .striped(true)
        .spacing([18.0, 10.0])
        .show(ui, |ui| {
            for header in [
                t("Name"),
                t("Name or address"),
                t("Resolution"),
                t("Last connected"),
                "",
            ] {
                ui.label(RichText::new(header).color(app.theme.text_secondary));
            }
            ui.end_row();

            for pc in pcs {
                ui.label(RichText::new(pc.display_title()).color(app.theme.text_primary));
                ui.label(&pc.pc_name);
                ui.label(resolution_label(&pc.resolution));
                ui.label(
                    pc.last_connected
                        .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
                        .unwrap_or_else(|| t("Never").to_string()),
                );
                ui.horizontal(|ui| {
                    if ui.add(app.theme.primary_button(t("Connect"))).clicked() {
                        connect_target = Some(pc.clone());
                    }
                    if ui.button(t("Edit")).clicked() {
                        edit_target = Some(pc.clone());
                    }
                    let star = if pc.is_favorite { "★" } else { "☆" };
                    if ui
                        .button(star)
                        .on_hover_text(if pc.is_favorite {
                            t("Remove from favorites")
                        } else {
                            t("Add to favorites")
                        })
                        .clicked()
                    {
                        toggle_fav = Some(pc.id);
                    }
                });
                ui.end_row();
            }
        });

    if let Some(pc) = connect_target {
        app.connect_to(&pc);
    }
    if let Some(pc) = edit_target {
        app.add_edit_dialog.open_edit(&pc);
    }
    if let Some(id) = toggle_fav {
        app.toggle_favorite(id);
    }
}
