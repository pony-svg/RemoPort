//! Application shell: left navigation rail, header bar, search box, welcome view,
//! and dispatch to the active page.

use crate::ui::app::{RemoportApp, FEEDBACK_URL, HELP_URL};
use crate::ui::fonts;
use crate::ui::i18n::t;
use crate::ui::icons::{self, Icon};
use crate::ui::pages;
use crate::ui::widgets;
use egui::popup::{popup_below_widget, PopupCloseBehavior};
use egui::{
    pos2, vec2, Align, Align2, Color32, CursorIcon, FontId, Layout, Margin, Rect, Response,
    RichText, Rounding, Sense, Stroke, Ui,
};
use remoport_core::domain::{NavTab, SortOption, ViewMode};
use remoport_core::logging;

pub(super) fn show_rail(app: &mut RemoportApp, ui: &mut Ui) {
    let rail = ui.max_rect();
    ui.vertical_centered(|ui| {
        ui.add_space(12.0);
        for (tab, icon, label) in [
            (NavTab::Favorites, Icon::Star, t("Favorites")),
            (NavTab::Devices, Icon::Monitor, t("Devices")),
            (NavTab::Apps, Icon::Window, t("Apps")),
        ] {
            if rail_item(app, ui, icon, label, app.state.active_tab == tab).clicked() {
                open_page(app, tab);
            }
        }

        ui.add_space(6.0);
        let y = ui.cursor().min.y;
        ui.painter().line_segment(
            [pos2(rail.left() + 20.0, y), pos2(rail.right() - 20.0, y)],
            Stroke::new(1.0_f32, app.theme.divider),
        );
        ui.add_space(8.0);

        let add = rail_item(app, ui, Icon::Plus, t("Add"), false);
        let menu = ui.make_persistent_id("add_menu");
        if add.clicked() {
            logging::event("Add menu opened");
            ui.memory_mut(|m| m.toggle_popup(menu));
        }
        popup_below_widget(ui, menu, &add, PopupCloseBehavior::CloseOnClick, |ui| {
            ui.set_min_width(230.0);
            add_menu_items(app, ui);
        });
    });

    ui.with_layout(Layout::bottom_up(Align::Center), |ui| {
        ui.add_space(12.0);
        if rail_item(app, ui, Icon::Feedback, t("Feedback"), false)
            .on_hover_text(t("Report a problem or suggest an idea"))
            .clicked()
        {
            logging::event("Opened the feedback page");
            ui.ctx().open_url(egui::OpenUrl::new_tab(FEEDBACK_URL));
        }
        let settings = app.state.active_tab == NavTab::Settings;
        if rail_item(app, ui, Icon::Gear, t("Settings"), settings).clicked() {
            open_page(app, NavTab::Settings);
        }
    });
}

fn rail_item(app: &RemoportApp, ui: &mut Ui, icon: Icon, label: &str, selected: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(68.0, 60.0), Sense::click());
    let painter = ui.painter();
    if selected {
        painter.rect_filled(rect, Rounding::same(8.0), app.theme.nav_selected);
        painter.rect_filled(
            Rect::from_min_size(
                pos2(rect.left() - 6.0, rect.center().y - 11.0),
                vec2(3.0, 22.0),
            ),
            Rounding::same(2.0),
            app.theme.accent,
        );
    } else if response.hovered() {
        painter.rect_filled(rect, Rounding::same(8.0), app.theme.nav_hover);
    }
    let (icon_color, text_color) = if selected {
        (app.theme.accent, app.theme.text_primary)
    } else {
        (app.theme.text_secondary, app.theme.text_secondary)
    };
    icons::paint(
        painter,
        Rect::from_center_size(pos2(rect.center().x, rect.top() + 22.0), vec2(22.0, 22.0)),
        icon,
        icon_color,
    );
    painter.text(
        pos2(rect.center().x, rect.bottom() - 13.0),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(11.0),
        text_color,
    );
    response.on_hover_cursor(CursorIcon::PointingHand)
}

fn open_page(app: &mut RemoportApp, tab: NavTab) {
    if app.state.active_tab != tab {
        logging::event(format_args!("Page {tab:?}"));
        app.state.active_tab = tab;
    }
}

fn add_menu_items(app: &mut RemoportApp, ui: &mut Ui) {
    if ui.button(t("Add a PC…")).clicked() {
        app.add_edit_dialog.open_add(app.state.default_resolution);
    }
    if ui.button(t("Subscribe to a workspace…")).clicked() {
        app.add_workspace_dialog.open();
    }
    if ui.button(t("Sign in to an organization…")).clicked() {
        logging::event("Add menu: Sign in to an organization");
        app.add_workspace_dialog.open();
    }
}

pub(super) fn show_page(app: &mut RemoportApp, ui: &mut Ui) {
    let nothing_yet = app.state.bookmarks.is_empty() && app.state.remote_apps.is_empty();
    if nothing_yet && matches!(app.state.active_tab, NavTab::Favorites | NavTab::Devices) {
        show_welcome(app, ui);
        return;
    }

    show_header(app, ui);
    ui.add_space(14.0);
    match app.state.active_tab {
        NavTab::Devices => {
            pages::devices::show_filters(app, ui);
            ui.add_space(10.0);
        }
        NavTab::Apps => {
            pages::apps::show_filters(app, ui);
            ui.add_space(10.0);
        }
        NavTab::Favorites | NavTab::Settings => {}
    }

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            match app.state.active_tab {
                NavTab::Favorites => pages::favorites::show(app, ui),
                NavTab::Devices => pages::devices::show(app, ui),
                NavTab::Apps => pages::apps::show(app, ui),
                NavTab::Settings => pages::settings::show(app, ui),
            }
            ui.add_space(24.0);
        });
}

fn show_header(app: &mut RemoportApp, ui: &mut Ui) {
    let (title, subtitle) = match app.state.active_tab {
        NavTab::Favorites => (
            t("Favorites"),
            t("The PCs and apps you starred").to_string(),
        ),
        NavTab::Devices => (
            t("Devices"),
            app.group_filter
                .clone()
                .unwrap_or_else(|| t("Saved on this computer").to_string()),
        ),
        NavTab::Apps => (
            t("Apps"),
            app.workspace_filter
                .clone()
                .unwrap_or_else(|| t("Published by your workspaces").to_string()),
        ),
        NavTab::Settings => (
            t("Settings"),
            format!("Remoport {}", env!("CARGO_PKG_VERSION")),
        ),
    };

    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(
                RichText::new(title)
                    .font(fonts::medium(26.0))
                    .color(app.theme.text_primary),
            );
            ui.label(
                RichText::new(subtitle)
                    .size(12.5)
                    .color(app.theme.text_secondary),
            );
        });

        ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
            account_button(app, ui);
            ui.add_space(2.0);
            if widgets::icon_button(&app.theme, ui, Icon::Help, t("Help")).clicked() {
                logging::event("Opened help");
                ui.ctx().open_url(egui::OpenUrl::new_tab(HELP_URL));
            }
            if app.state.active_tab == NavTab::Devices {
                let more = widgets::icon_button(&app.theme, ui, Icon::More, t("View and sort"));
                let menu = ui.make_persistent_id("view_menu");
                if more.clicked() {
                    ui.memory_mut(|m| m.toggle_popup(menu));
                }
                popup_below_widget(ui, menu, &more, PopupCloseBehavior::CloseOnClick, |ui| {
                    ui.set_min_width(190.0);
                    view_menu_items(app, ui);
                });
            }
            if matches!(app.state.active_tab, NavTab::Devices | NavTab::Apps) {
                ui.add_space(8.0);
                search_box(app, ui);
            }
        });
    });
}

fn view_menu_items(app: &mut RemoportApp, ui: &mut Ui) {
    let (view, sort) = (app.state.view_mode, app.state.sort_option);
    view_menu_choices(app, ui);
    if app.state.view_mode != view {
        logging::event(format_args!("View: {:?}", app.state.view_mode));
    }
    if app.state.sort_option != sort {
        logging::event(format_args!("Sort by: {:?}", app.state.sort_option));
    }
}

fn view_menu_choices(app: &mut RemoportApp, ui: &mut Ui) {
    ui.label(RichText::new(t("View")).strong());
    ui.selectable_value(&mut app.state.view_mode, ViewMode::Grid, t("Cards"));
    ui.selectable_value(&mut app.state.view_mode, ViewMode::List, t("List"));
    ui.separator();
    ui.label(RichText::new(t("Sort by")).strong());
    ui.selectable_value(
        &mut app.state.sort_option,
        SortOption::DeviceName,
        t("Name"),
    );
    ui.selectable_value(
        &mut app.state.sort_option,
        SortOption::TimeLastConnected,
        t("Last connected"),
    );
    ui.selectable_value(
        &mut app.state.sort_option,
        SortOption::Manual,
        t("Order added"),
    );
}

fn account_button(app: &mut RemoportApp, ui: &mut Ui) {
    let (rect, response) = ui.allocate_exact_size(vec2(34.0, 34.0), Sense::click());
    let painter = ui.painter();
    painter.circle_filled(rect.center(), 15.0, app.theme.accent);
    icons::paint(painter, rect.shrink(9.0), Icon::Person, Color32::WHITE);
    let response = response
        .on_hover_text(t("Account"))
        .on_hover_cursor(CursorIcon::PointingHand);
    let menu = ui.make_persistent_id("account_menu");
    if response.clicked() {
        logging::event("Account menu opened");
        ui.memory_mut(|m| m.toggle_popup(menu));
    }
    popup_below_widget(
        ui,
        menu,
        &response,
        PopupCloseBehavior::CloseOnClick,
        |ui| {
            ui.set_min_width(240.0);
            ui.label(
                RichText::new(&app.state.user_account.display_name)
                    .font(fonts::medium(14.0))
                    .color(app.theme.text_primary),
            );
            ui.label(RichText::new(&app.state.user_account.email).color(app.theme.text_secondary));
            ui.separator();
            if ui.button(t("Sign in to an organization…")).clicked() {
                logging::event("Account menu: Sign in to an organization");
                app.add_workspace_dialog.open();
            }
        },
    );
}

fn search_box(app: &mut RemoportApp, ui: &mut Ui) {
    egui::Frame::none()
        .fill(app.theme.card_bg)
        .stroke(app.theme.card_stroke())
        .rounding(Rounding::same(6.0))
        .inner_margin(Margin::symmetric(8.0, 5.0))
        .show(ui, |ui| {
            // Explicitly left to right: the header around it runs right to left.
            ui.allocate_ui_with_layout(
                vec2(220.0, 22.0),
                Layout::left_to_right(Align::Center),
                |ui| {
                    let (icon, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                    icons::paint(ui.painter(), icon, Icon::Search, app.theme.text_secondary);
                    ui.add(
                        egui::TextEdit::singleline(&mut app.search_query)
                            .frame(false)
                            .hint_text(t("Search"))
                            .desired_width(f32::INFINITY),
                    );
                },
            );
        });
}

fn show_welcome(app: &mut RemoportApp, ui: &mut Ui) {
    let height = ui.available_height();
    ui.vertical_centered(|ui| {
        ui.add_space((height * 0.18).max(30.0));
        let (logo, _) = ui.allocate_exact_size(vec2(88.0, 88.0), Sense::hover());
        icons::paint_logo(ui.painter(), logo);
        ui.add_space(18.0);
        ui.label(
            RichText::new(t("Welcome to Remoport"))
                .font(fonts::medium(26.0))
                .color(app.theme.text_primary),
        );
        ui.add_space(4.0);
        ui.label(
            RichText::new(t("Your remote computers and apps, in one place."))
                .size(13.5)
                .color(app.theme.text_secondary),
        );
        ui.add_space(26.0);

        if ui
            .add_sized([280.0, 38.0], app.theme.primary_button(t("Add a PC")))
            .clicked()
        {
            app.add_edit_dialog.open_add(app.state.default_resolution);
        }

        ui.add_space(6.0);
        let (rule, _) = ui.allocate_exact_size(vec2(280.0, 22.0), Sense::hover());
        let painter = ui.painter();
        let y = rule.center().y;
        let stroke = Stroke::new(1.0_f32, app.theme.card_border);
        painter.line_segment(
            [pos2(rule.left(), y), pos2(rule.center().x - 16.0, y)],
            stroke,
        );
        painter.line_segment(
            [pos2(rule.center().x + 16.0, y), pos2(rule.right(), y)],
            stroke,
        );
        painter.text(
            rule.center(),
            Align2::CENTER_CENTER,
            t("or"),
            FontId::proportional(12.0),
            app.theme.text_secondary,
        );
        ui.add_space(6.0);

        let subscribe = egui::Button::new(
            RichText::new(t("Subscribe to a workspace")).color(app.theme.text_primary),
        )
        .fill(app.theme.card_bg)
        .stroke(app.theme.card_stroke());
        if ui.add_sized([280.0, 38.0], subscribe).clicked() {
            app.add_workspace_dialog.open();
        }

        ui.add_space(30.0);
        ui.label(
            RichText::new(t("Just once, without saving it:"))
                .size(12.5)
                .color(app.theme.text_secondary),
        );
        ui.add_space(4.0);
        ui.allocate_ui_with_layout(
            vec2(280.0, 32.0),
            Layout::left_to_right(Align::Center),
            |ui| widgets::quick_connect(app, ui, 190.0),
        );
    });
}
