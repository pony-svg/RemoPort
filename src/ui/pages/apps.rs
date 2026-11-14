//! Apps page: published remote applications filtered by workspace and search.

use crate::ui::app::RemoportApp;
use crate::ui::i18n::t;
use crate::ui::icons::Icon;
use crate::ui::widgets::{self, EmptyAction};
use egui::Ui;
use remoport_core::domain::RemoteAppItem;

pub(crate) fn show_filters(app: &mut RemoportApp, ui: &mut Ui) {
    let mut workspaces: Vec<String> = app
        .state
        .remote_apps
        .iter()
        .map(|item| item.workspace_name.clone())
        .collect();
    workspaces.sort_by_key(|w| w.to_lowercase());
    workspaces.dedup();
    let mut filter = app.workspace_filter.take();
    widgets::filter_chips(
        &app.theme,
        ui,
        "workspace_filter",
        t("Workspace"),
        &workspaces,
        &mut filter,
    );
    app.workspace_filter = filter;
}

pub(crate) fn show(app: &mut RemoportApp, ui: &mut Ui) {
    if app.state.remote_apps.is_empty() {
        widgets::show_empty_state(
            app,
            ui,
            Icon::Window,
            t("No apps yet"),
            t("Subscribe to your organization's workspace to open the apps it publishes as windows of their own."),
            Some((t("Subscribe to a workspace"), EmptyAction::AddWorkspace)),
        );
        return;
    }

    let query = app.search_query.trim().to_lowercase();
    let mut apps: Vec<RemoteAppItem> = app
        .state
        .remote_apps
        .iter()
        .filter(|a| query.is_empty() || a.name.to_lowercase().contains(&query))
        .filter(|a| {
            app.workspace_filter
                .as_ref()
                .is_none_or(|w| *w == a.workspace_name)
        })
        .cloned()
        .collect();
    if apps.is_empty() {
        widgets::show_empty_state(
            app,
            ui,
            Icon::Search,
            t("No apps match"),
            t("Try another name, or show all workspaces."),
            None,
        );
        return;
    }
    apps.sort_by_key(|app| app.name.to_lowercase());

    let mut workspaces: Vec<(String, Vec<RemoteAppItem>)> = Vec::new();
    for item in apps {
        match workspaces
            .iter_mut()
            .find(|(name, _)| *name == item.workspace_name)
        {
            Some((_, list)) => list.push(item),
            None => workspaces.push((item.workspace_name.clone(), vec![item])),
        }
    }
    for (workspace, items) in &workspaces {
        if widgets::section(
            app,
            ui,
            &format!("apps:{workspace}"),
            workspace,
            items.len(),
        ) {
            widgets::card_row(ui, |ui| {
                for item in items {
                    widgets::show_app_card(app, ui, item);
                }
            });
        }
        ui.add_space(18.0);
    }
}
