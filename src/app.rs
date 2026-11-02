use crate::dialogs::{AddEditPcDialog, AddWorkspaceDialog};
use crate::fonts;
use crate::icons::{self, Icon};
use crate::logging;
use crate::model::{
    state_path, AppStateData, DeviceBookmark, NavTab, RemoteAppItem, ResolutionOption,
    SortOption, ViewMode,
};
use crate::session::ActiveSession;
use crate::theme::{Appearance, Theme};
use chrono::Utc;
use egui::popup::{popup_below_widget, PopupCloseBehavior};
use egui::{
    pos2, vec2, Align, Align2, Color32, Context, CursorIcon, FontId, Layout, Margin, Painter,
    Rect, Response, RichText, Rounding, Sense, Stroke, Ui,
};

/// Project pages linked from the rail, the header and Settings.
const REPO_URL: &str = "https://github.com/pony-svg/RemoPort";
const HELP_URL: &str = "https://github.com/pony-svg/RemoPort#readme";
const FEEDBACK_URL: &str = "https://github.com/pony-svg/RemoPort/issues";

const RAIL_WIDTH: f32 = 84.0;
const PC_CARD: egui::Vec2 = vec2(280.0, 204.0);
const APP_CARD: egui::Vec2 = vec2(190.0, 150.0);
const FAVORITE: Color32 = Color32::from_rgb(255, 184, 0);
const DANGER: Color32 = Color32::from_rgb(0xc4, 0x2b, 0x1c);

/// What the button of an empty view does.
#[derive(Clone, Copy)]
enum EmptyAction {
    AddPc,
    AddWorkspace,
}

pub struct RemoportApp {
    theme: Theme,
    pub state: AppStateData,
    search_query: String,
    /// Group shown on the Devices page; `None` shows them all.
    group_filter: Option<String>,
    /// Workspace shown on the Apps page; `None` shows them all.
    workspace_filter: Option<String>,
    add_edit_dialog: AddEditPcDialog,
    add_workspace_dialog: AddWorkspaceDialog,
    active_session: Option<ActiveSession>,
    quick_connect_host: String,
    /// A PC waiting for the user to confirm its deletion.
    pending_delete: Option<DeviceBookmark>,
    /// Whether egui has been given the current theme yet.
    theme_applied: bool,
    /// The state as last written to disk, to save only what changed.
    last_saved: String,
}

impl RemoportApp {
    /// Starts with the state saved by the previous run.
    pub fn load() -> Self {
        let state = AppStateData::load();
        logging::event(format_args!(
            "Remoport {} started: {} PCs and {} apps, {:?} appearance, state in {}",
            env!("CARGO_PKG_VERSION"),
            state.bookmarks.len(),
            state.remote_apps.len(),
            state.appearance,
            state_path().map_or_else(|| "memory only".to_string(), |p| p.display().to_string()),
        ));
        let last_saved = serde_json::to_string_pretty(&state).unwrap_or_default();
        Self {
            theme: Theme::for_appearance(state.appearance),
            state,
            search_query: String::new(),
            group_filter: None,
            workspace_filter: None,
            add_edit_dialog: AddEditPcDialog::default(),
            add_workspace_dialog: AddWorkspaceDialog::default(),
            active_session: None,
            quick_connect_host: String::new(),
            pending_delete: None,
            theme_applied: false,
            last_saved,
        }
    }

    pub fn update(&mut self, ctx: &Context) {
        if !self.theme_applied {
            self.theme.apply(ctx);
            self.theme_applied = true;
        }

        let mut saved_pc = None;
        self.add_edit_dialog.show(ctx, &self.theme, &mut saved_pc);
        if let Some(pc) = saved_pc {
            if let Some(pos) = self.state.bookmarks.iter().position(|b| b.id == pc.id) {
                self.state.bookmarks[pos] = pc;
            } else {
                self.state.bookmarks.push(pc);
            }
        }
        self.add_workspace_dialog.show(ctx, &self.theme);
        self.show_delete_confirmation(ctx);

        if let Some(ref mut session) = self.active_session {
            let mut disconnect = false;
            egui::CentralPanel::default()
                .frame(egui::Frame::none().fill(self.theme.bg_dark))
                .show(ctx, |ui| session.render(ui, &self.theme, &mut disconnect));
            if disconnect {
                // The session logs how it ended; nothing to add here.
                self.active_session = None;
            }
        } else {
            egui::SidePanel::left("rail")
                .exact_width(RAIL_WIDTH)
                .resizable(false)
                .show_separator_line(false)
                .frame(egui::Frame::none().fill(self.theme.sidebar_bg))
                .show(ctx, |ui| self.show_rail(ui));
            egui::CentralPanel::default()
                .frame(egui::Frame::none().fill(self.theme.bg_dark).inner_margin(Margin {
                    left: 32.0,
                    right: 28.0,
                    top: 22.0,
                    bottom: 0.0,
                }))
                .show(ctx, |ui| self.show_page(ui));
        }

        self.save_if_changed();
    }

    fn save_if_changed(&mut self) {
        let Ok(json) = serde_json::to_string_pretty(&self.state) else {
            return;
        };
        if json != self.last_saved {
            if let Err(err) = AppStateData::save(&json) {
                logging::event(format_args!("Could not save the state: {err}"));
            }
            self.last_saved = json;
        }
    }

    // ── Rail ────────────────────────────────────────────────────────────

    fn show_rail(&mut self, ui: &mut Ui) {
        let rail = ui.max_rect();
        ui.vertical_centered(|ui| {
            ui.add_space(12.0);
            for (tab, icon, label) in [
                (NavTab::Favorites, Icon::Star, "Favorites"),
                (NavTab::Devices, Icon::Monitor, "Devices"),
                (NavTab::Apps, Icon::Window, "Apps"),
            ] {
                if self
                    .rail_item(ui, icon, label, self.state.active_tab == tab)
                    .clicked()
                {
                    self.open_page(tab);
                }
            }

            ui.add_space(6.0);
            let y = ui.cursor().min.y;
            ui.painter().line_segment(
                [pos2(rail.left() + 20.0, y), pos2(rail.right() - 20.0, y)],
                Stroke::new(1.0_f32, self.theme.divider),
            );
            ui.add_space(8.0);

            let add = self.rail_item(ui, Icon::Plus, "Add", false);
            let menu = ui.make_persistent_id("add_menu");
            if add.clicked() {
                logging::event("Add menu opened");
                ui.memory_mut(|m| m.toggle_popup(menu));
            }
            popup_below_widget(ui, menu, &add, PopupCloseBehavior::CloseOnClick, |ui| {
                ui.set_min_width(230.0);
                self.add_menu_items(ui);
            });
        });

        ui.with_layout(Layout::bottom_up(Align::Center), |ui| {
            ui.add_space(12.0);
            if self
                .rail_item(ui, Icon::Feedback, "Feedback", false)
                .on_hover_text("Report a problem or suggest an idea")
                .clicked()
            {
                logging::event("Opened the feedback page");
                ui.ctx().open_url(egui::OpenUrl::new_tab(FEEDBACK_URL));
            }
            let settings = self.state.active_tab == NavTab::Settings;
            if self.rail_item(ui, Icon::Gear, "Settings", settings).clicked() {
                self.open_page(NavTab::Settings);
            }
        });
    }

    fn rail_item(&self, ui: &mut Ui, icon: Icon, label: &str, selected: bool) -> Response {
        let (rect, response) = ui.allocate_exact_size(vec2(68.0, 60.0), Sense::click());
        let painter = ui.painter();
        if selected {
            painter.rect_filled(rect, Rounding::same(8.0), self.theme.nav_selected);
            painter.rect_filled(
                Rect::from_min_size(pos2(rect.left() - 6.0, rect.center().y - 11.0), vec2(3.0, 22.0)),
                Rounding::same(2.0),
                self.theme.accent,
            );
        } else if response.hovered() {
            painter.rect_filled(rect, Rounding::same(8.0), self.theme.nav_hover);
        }
        let (icon_color, text_color) = if selected {
            (self.theme.accent, self.theme.text_primary)
        } else {
            (self.theme.text_secondary, self.theme.text_secondary)
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

    fn open_page(&mut self, tab: NavTab) {
        if self.state.active_tab != tab {
            logging::event(format_args!("Page {tab:?}"));
            self.state.active_tab = tab;
        }
    }

    fn add_menu_items(&mut self, ui: &mut Ui) {
        if ui.button("Add a PC…").clicked() {
            self.add_edit_dialog.open_add(self.state.default_resolution);
        }
        if ui.button("Subscribe to a workspace…").clicked() {
            self.add_workspace_dialog.open();
        }
        if ui.button("Sign in to an organization…").clicked() {
            logging::event("Add menu: Sign in to an organization");
            self.add_workspace_dialog.open();
        }
    }

    // ── Page ────────────────────────────────────────────────────────────

    fn show_page(&mut self, ui: &mut Ui) {
        let nothing_yet = self.state.bookmarks.is_empty() && self.state.remote_apps.is_empty();
        if nothing_yet && matches!(self.state.active_tab, NavTab::Favorites | NavTab::Devices) {
            self.show_welcome(ui);
            return;
        }

        self.show_header(ui);
        ui.add_space(14.0);
        match self.state.active_tab {
            NavTab::Devices => {
                self.show_device_filters(ui);
                ui.add_space(10.0);
            }
            NavTab::Apps => {
                self.show_app_filters(ui);
                ui.add_space(10.0);
            }
            NavTab::Favorites | NavTab::Settings => {}
        }

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                match self.state.active_tab {
                    NavTab::Favorites => self.show_favorites(ui),
                    NavTab::Devices => self.show_devices(ui),
                    NavTab::Apps => self.show_apps(ui),
                    NavTab::Settings => self.show_settings(ui),
                }
                ui.add_space(24.0);
            });
    }

    fn show_header(&mut self, ui: &mut Ui) {
        let (title, subtitle) = match self.state.active_tab {
            NavTab::Favorites => ("Favorites", "The PCs and apps you starred".to_string()),
            NavTab::Devices => (
                "Devices",
                self.group_filter
                    .clone()
                    .unwrap_or_else(|| "Saved on this computer".to_string()),
            ),
            NavTab::Apps => (
                "Apps",
                self.workspace_filter
                    .clone()
                    .unwrap_or_else(|| "Published by your workspaces".to_string()),
            ),
            NavTab::Settings => ("Settings", format!("Remoport {}", env!("CARGO_PKG_VERSION"))),
        };

        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(
                    RichText::new(title)
                        .font(fonts::medium(26.0))
                        .color(self.theme.text_primary),
                );
                ui.label(
                    RichText::new(subtitle)
                        .size(12.5)
                        .color(self.theme.text_secondary),
                );
            });

            ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                self.account_button(ui);
                ui.add_space(2.0);
                if self.icon_button(ui, Icon::Help, "Help").clicked() {
                    logging::event("Opened help");
                    ui.ctx().open_url(egui::OpenUrl::new_tab(HELP_URL));
                }
                if self.state.active_tab == NavTab::Devices {
                    let more = self.icon_button(ui, Icon::More, "View and sort");
                    let menu = ui.make_persistent_id("view_menu");
                    if more.clicked() {
                        ui.memory_mut(|m| m.toggle_popup(menu));
                    }
                    popup_below_widget(ui, menu, &more, PopupCloseBehavior::CloseOnClick, |ui| {
                        ui.set_min_width(190.0);
                        self.view_menu_items(ui);
                    });
                }
                if matches!(self.state.active_tab, NavTab::Devices | NavTab::Apps) {
                    ui.add_space(8.0);
                    self.search_box(ui);
                }
            });
        });
    }

    fn view_menu_items(&mut self, ui: &mut Ui) {
        let (view, sort) = (self.state.view_mode, self.state.sort_option);
        self.view_menu_choices(ui);
        if self.state.view_mode != view {
            logging::event(format_args!("View: {:?}", self.state.view_mode));
        }
        if self.state.sort_option != sort {
            logging::event(format_args!("Sort by: {:?}", self.state.sort_option));
        }
    }

    fn view_menu_choices(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("View").strong());
        ui.selectable_value(&mut self.state.view_mode, ViewMode::Grid, "Cards");
        ui.selectable_value(&mut self.state.view_mode, ViewMode::List, "List");
        ui.separator();
        ui.label(RichText::new("Sort by").strong());
        ui.selectable_value(&mut self.state.sort_option, SortOption::DeviceName, "Name");
        ui.selectable_value(
            &mut self.state.sort_option,
            SortOption::TimeLastConnected,
            "Last connected",
        );
        ui.selectable_value(&mut self.state.sort_option, SortOption::Manual, "Order added");
    }

    fn account_button(&mut self, ui: &mut Ui) {
        let (rect, response) = ui.allocate_exact_size(vec2(34.0, 34.0), Sense::click());
        let painter = ui.painter();
        painter.circle_filled(rect.center(), 15.0, self.theme.accent);
        icons::paint(painter, rect.shrink(9.0), Icon::Person, Color32::WHITE);
        let response = response
            .on_hover_text("Account")
            .on_hover_cursor(CursorIcon::PointingHand);
        let menu = ui.make_persistent_id("account_menu");
        if response.clicked() {
            logging::event("Account menu opened");
            ui.memory_mut(|m| m.toggle_popup(menu));
        }
        popup_below_widget(ui, menu, &response, PopupCloseBehavior::CloseOnClick, |ui| {
            ui.set_min_width(240.0);
            ui.label(
                RichText::new(&self.state.user_account.display_name)
                    .font(fonts::medium(14.0))
                    .color(self.theme.text_primary),
            );
            ui.label(
                RichText::new(&self.state.user_account.email).color(self.theme.text_secondary),
            );
            ui.separator();
            if ui.button("Sign in to an organization…").clicked() {
                logging::event("Account menu: Sign in to an organization");
                self.add_workspace_dialog.open();
            }
        });
    }

    fn icon_button(&self, ui: &mut Ui, icon: Icon, tooltip: &str) -> Response {
        let (rect, response) = ui.allocate_exact_size(vec2(34.0, 34.0), Sense::click());
        let hovered = response.hovered();
        if hovered {
            ui.painter()
                .rect_filled(rect, Rounding::same(6.0), self.theme.nav_hover);
        }
        let color = if hovered {
            self.theme.text_primary
        } else {
            self.theme.text_secondary
        };
        icons::paint(ui.painter(), rect.shrink(8.0), icon, color);
        response
            .on_hover_text(tooltip)
            .on_hover_cursor(CursorIcon::PointingHand)
    }

    fn search_box(&mut self, ui: &mut Ui) {
        egui::Frame::none()
            .fill(self.theme.card_bg)
            .stroke(self.theme.card_stroke())
            .rounding(Rounding::same(6.0))
            .inner_margin(Margin::symmetric(8.0, 5.0))
            .show(ui, |ui| {
                // Explicitly left to right: the header around it runs right to left.
                ui.allocate_ui_with_layout(vec2(220.0, 22.0), Layout::left_to_right(Align::Center), |ui| {
                    let (icon, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                    icons::paint(ui.painter(), icon, Icon::Search, self.theme.text_secondary);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.search_query)
                            .frame(false)
                            .hint_text("Search")
                            .desired_width(f32::INFINITY),
                    );
                });
            });
    }

    // ── Filters ─────────────────────────────────────────────────────────

    /// A rounded filter pill; filled when selected, with a chevron when it
    /// opens a list.
    fn chip(&self, ui: &mut Ui, text: &str, selected: bool, dropdown: bool) -> Response {
        let text_color = if selected {
            self.theme.bg_dark
        } else {
            self.theme.text_primary
        };
        let galley = ui
            .painter()
            .layout_no_wrap(text.to_owned(), FontId::proportional(13.0), text_color);
        let width = galley.size().x + 26.0 + if dropdown { 18.0 } else { 0.0 };
        let (rect, response) = ui.allocate_exact_size(vec2(width, 30.0), Sense::click());
        let painter = ui.painter();
        let (fill, stroke) = if selected {
            (self.theme.text_primary, Stroke::NONE)
        } else if response.hovered() {
            (self.theme.card_hover, self.theme.card_stroke())
        } else {
            (self.theme.card_bg, self.theme.card_stroke())
        };
        painter.rect(rect, Rounding::same(15.0), fill, stroke);
        let text_pos = pos2(rect.left() + 13.0, rect.center().y - galley.size().y / 2.0);
        painter.galley(text_pos, galley, text_color);
        if dropdown {
            icons::paint(
                painter,
                Rect::from_center_size(pos2(rect.right() - 17.0, rect.center().y), vec2(14.0, 14.0)),
                Icon::ChevronDown,
                text_color,
            );
        }
        response.on_hover_cursor(CursorIcon::PointingHand)
    }

    /// "All" plus a drop-down chip choosing one of `options`.
    fn filter_chips(
        &self,
        ui: &mut Ui,
        id: &str,
        label: &str,
        options: &[String],
        filter: &mut Option<String>,
    ) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if self.chip(ui, "All", filter.is_none(), false).clicked() && filter.is_some() {
                logging::event(format_args!("Filter: all ({label})"));
                *filter = None;
            }
            let text = filter.clone().unwrap_or_else(|| label.to_string());
            let chip = self.chip(ui, &text, filter.is_some(), true);
            let menu = ui.make_persistent_id(id);
            if chip.clicked() {
                ui.memory_mut(|m| m.toggle_popup(menu));
            }
            popup_below_widget(ui, menu, &chip, PopupCloseBehavior::CloseOnClick, |ui| {
                ui.set_min_width(180.0);
                if options.is_empty() {
                    ui.label(RichText::new("Nothing to filter by yet").color(self.theme.text_secondary));
                }
                for option in options {
                    if ui
                        .selectable_label(filter.as_deref() == Some(option.as_str()), option)
                        .clicked()
                    {
                        logging::event(format_args!("Filter: {label} \"{option}\""));
                        *filter = Some(option.clone());
                    }
                }
            });
        });
    }

    fn show_device_filters(&mut self, ui: &mut Ui) {
        let mut groups: Vec<String> = self
            .state
            .bookmarks
            .iter()
            .map(|pc| pc.group_name.clone())
            .collect();
        groups.sort_by_key(|g| g.to_lowercase());
        groups.dedup();
        let mut filter = self.group_filter.take();
        self.filter_chips(ui, "group_filter", "Group", &groups, &mut filter);
        self.group_filter = filter;
    }

    fn show_app_filters(&mut self, ui: &mut Ui) {
        let mut workspaces: Vec<String> = self
            .state
            .remote_apps
            .iter()
            .map(|app| app.workspace_name.clone())
            .collect();
        workspaces.sort_by_key(|w| w.to_lowercase());
        workspaces.dedup();
        let mut filter = self.workspace_filter.take();
        self.filter_chips(ui, "workspace_filter", "Workspace", &workspaces, &mut filter);
        self.workspace_filter = filter;
    }

    // ── Sections ────────────────────────────────────────────────────────

    /// A foldable section title; returns whether the section is open.
    fn section(&mut self, ui: &mut Ui, key: &str, title: &str, count: usize) -> bool {
        let open = !self.state.collapsed_groups.iter().any(|g| g == key);
        let (rect, response) =
            ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
        let painter = ui.painter();
        let chevron = if open {
            Icon::ChevronDown
        } else {
            Icon::ChevronRight
        };
        icons::paint(
            painter,
            Rect::from_center_size(pos2(rect.left() + 8.0, rect.center().y), vec2(16.0, 16.0)),
            chevron,
            self.theme.text_secondary,
        );
        let title = painter.layout_no_wrap(title.to_owned(), fonts::medium(14.0), self.theme.text_primary);
        let title_width = title.size().x;
        painter.galley(
            pos2(rect.left() + 24.0, rect.center().y - title.size().y / 2.0),
            title,
            self.theme.text_primary,
        );
        painter.text(
            pos2(rect.left() + 32.0 + title_width, rect.center().y),
            Align2::LEFT_CENTER,
            count.to_string(),
            FontId::proportional(12.0),
            self.theme.text_secondary,
        );
        if response
            .on_hover_cursor(CursorIcon::PointingHand)
            .clicked()
        {
            if open {
                logging::event(format_args!("Folded {key}"));
                self.state.collapsed_groups.push(key.to_string());
            } else {
                logging::event(format_args!("Unfolded {key}"));
                self.state.collapsed_groups.retain(|g| g != key);
            }
        }
        ui.add_space(6.0);
        open
    }

    /// Lays cards out left to right, wrapping, with their tops aligned.
    fn card_row(ui: &mut Ui, add_cards: impl FnOnce(&mut Ui)) {
        ui.with_layout(Layout::left_to_right(Align::TOP).with_main_wrap(true), |ui| {
            ui.spacing_mut().item_spacing = vec2(16.0, 16.0);
            add_cards(ui);
        });
    }

    fn show_empty_state(
        &mut self,
        ui: &mut Ui,
        icon: Icon,
        title: &str,
        description: &str,
        action: Option<(&str, EmptyAction)>,
    ) {
        ui.vertical_centered(|ui| {
            ui.add_space(70.0);
            let (rect, _) = ui.allocate_exact_size(vec2(48.0, 48.0), Sense::hover());
            icons::paint(ui.painter(), rect, icon, self.theme.text_secondary);
            ui.add_space(12.0);
            ui.label(
                RichText::new(title)
                    .font(fonts::medium(18.0))
                    .color(self.theme.text_primary),
            );
            ui.add_space(4.0);
            ui.label(
                RichText::new(description)
                    .size(13.0)
                    .color(self.theme.text_secondary),
            );
            if let Some((text, action)) = action {
                ui.add_space(16.0);
                if ui
                    .add_sized([220.0, 34.0], self.theme.primary_button(text))
                    .clicked()
                {
                    match action {
                        EmptyAction::AddPc => {
                            self.add_edit_dialog.open_add(self.state.default_resolution);
                        }
                        EmptyAction::AddWorkspace => self.add_workspace_dialog.open(),
                    }
                }
            }
        });
    }

    // ── Welcome ─────────────────────────────────────────────────────────

    fn show_welcome(&mut self, ui: &mut Ui) {
        let height = ui.available_height();
        ui.vertical_centered(|ui| {
            ui.add_space((height * 0.18).max(30.0));
            let (logo, _) = ui.allocate_exact_size(vec2(88.0, 88.0), Sense::hover());
            icons::paint_logo(ui.painter(), logo);
            ui.add_space(18.0);
            ui.label(
                RichText::new("Welcome to Remoport")
                    .font(fonts::medium(26.0))
                    .color(self.theme.text_primary),
            );
            ui.add_space(4.0);
            ui.label(
                RichText::new("Your remote computers and apps, in one place.")
                    .size(13.5)
                    .color(self.theme.text_secondary),
            );
            ui.add_space(26.0);

            if ui
                .add_sized([280.0, 38.0], self.theme.primary_button("Add a PC"))
                .clicked()
            {
                self.add_edit_dialog.open_add(self.state.default_resolution);
            }

            ui.add_space(6.0);
            let (rule, _) = ui.allocate_exact_size(vec2(280.0, 22.0), Sense::hover());
            let painter = ui.painter();
            let y = rule.center().y;
            let stroke = Stroke::new(1.0_f32, self.theme.card_border);
            painter.line_segment([pos2(rule.left(), y), pos2(rule.center().x - 16.0, y)], stroke);
            painter.line_segment([pos2(rule.center().x + 16.0, y), pos2(rule.right(), y)], stroke);
            painter.text(
                rule.center(),
                Align2::CENTER_CENTER,
                "or",
                FontId::proportional(12.0),
                self.theme.text_secondary,
            );
            ui.add_space(6.0);

            let subscribe = egui::Button::new(
                RichText::new("Subscribe to a workspace").color(self.theme.text_primary),
            )
            .fill(self.theme.card_bg)
            .stroke(self.theme.card_stroke());
            if ui.add_sized([280.0, 38.0], subscribe).clicked() {
                self.add_workspace_dialog.open();
            }

            ui.add_space(30.0);
            ui.label(
                RichText::new("Just once, without saving it:")
                    .size(12.5)
                    .color(self.theme.text_secondary),
            );
            ui.add_space(4.0);
            ui.allocate_ui_with_layout(
                vec2(280.0, 32.0),
                Layout::left_to_right(Align::Center),
                |ui| self.quick_connect(ui, 190.0),
            );
        });
    }

    /// A one-off connection by name or address, without saving the PC.
    fn quick_connect(&mut self, ui: &mut Ui, width: f32) {
        let field = ui.add(
            egui::TextEdit::singleline(&mut self.quick_connect_host)
                .hint_text("Name or address of the PC")
                .desired_width(width),
        );
        let entered = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let clicked = ui.add(self.theme.primary_button("Connect")).clicked();
        let host = self.quick_connect_host.trim().to_string();
        if (entered || clicked) && !host.is_empty() {
            logging::event(format_args!("Quick connect to {host}"));
            let pc = DeviceBookmark::new(host.clone(), host, String::new());
            self.active_session = Some(ActiveSession::new(pc));
        }
    }

    // ── Favorites ───────────────────────────────────────────────────────

    fn show_favorites(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Quick connect").color(self.theme.text_secondary));
            self.quick_connect(ui, 260.0);
        });
        ui.add_space(18.0);

        let pcs: Vec<DeviceBookmark> = self
            .state
            .bookmarks
            .iter()
            .filter(|b| b.is_favorite)
            .cloned()
            .collect();
        let apps: Vec<RemoteAppItem> = self
            .state
            .remote_apps
            .iter()
            .filter(|a| a.is_favorite)
            .cloned()
            .collect();

        if pcs.is_empty() && apps.is_empty() {
            self.show_empty_state(
                ui,
                Icon::Star,
                "No favorites yet",
                "Open the ··· menu of a PC or an app and add it to your favorites.",
                None,
            );
            return;
        }
        if !pcs.is_empty() && self.section(ui, "favorites:pcs", "PCs", pcs.len()) {
            Self::card_row(ui, |ui| {
                for pc in &pcs {
                    self.show_pc_card(ui, pc);
                }
            });
            ui.add_space(18.0);
        }
        if !apps.is_empty() && self.section(ui, "favorites:apps", "Apps", apps.len()) {
            Self::card_row(ui, |ui| {
                for app in &apps {
                    self.show_app_card(ui, app);
                }
            });
        }
    }

    // ── Devices ─────────────────────────────────────────────────────────

    fn sort_pcs(&self, pcs: &mut [DeviceBookmark]) {
        match self.state.sort_option {
            SortOption::DeviceName => pcs.sort_by_key(|pc| pc.display_title().to_lowercase()),
            // Most recent first; PCs never connected to go last.
            SortOption::TimeLastConnected => {
                pcs.sort_by_key(|pc| std::cmp::Reverse(pc.last_connected));
            }
            SortOption::Manual => {}
        }
    }

    fn show_devices(&mut self, ui: &mut Ui) {
        if self.state.bookmarks.is_empty() {
            self.show_empty_state(
                ui,
                Icon::Monitor,
                "No PCs yet",
                "Add a PC by its name or address, or subscribe to your organization's workspace.",
                Some(("Add a PC", EmptyAction::AddPc)),
            );
            return;
        }

        let query = self.search_query.trim().to_lowercase();
        let mut pcs: Vec<DeviceBookmark> = self
            .state
            .bookmarks
            .iter()
            .filter(|b| {
                query.is_empty()
                    || b.pc_name.to_lowercase().contains(&query)
                    || b.friendly_name.to_lowercase().contains(&query)
            })
            .filter(|b| self.group_filter.as_ref().is_none_or(|g| *g == b.group_name))
            .cloned()
            .collect();
        if pcs.is_empty() {
            self.show_empty_state(
                ui,
                Icon::Search,
                "No PCs match",
                "Try another name or address, or show all groups.",
                None,
            );
            return;
        }
        self.sort_pcs(&mut pcs);

        // One section per group, in the order the groups first appear.
        let mut groups: Vec<(String, Vec<DeviceBookmark>)> = Vec::new();
        for pc in pcs {
            match groups.iter_mut().find(|(name, _)| *name == pc.group_name) {
                Some((_, list)) => list.push(pc),
                None => groups.push((pc.group_name.clone(), vec![pc])),
            }
        }
        for (group, pcs) in &groups {
            if self.section(ui, &format!("devices:{group}"), group, pcs.len()) {
                match self.state.view_mode {
                    ViewMode::Grid => Self::card_row(ui, |ui| {
                        for pc in pcs {
                            self.show_pc_card(ui, pc);
                        }
                    }),
                    ViewMode::List => self.show_pc_list(ui, group, pcs),
                }
            }
            ui.add_space(18.0);
        }
    }

    fn paint_thumbnail(&self, painter: &Painter, thumb: Rect) {
        painter.add(icons::gradient_rect(thumb, self.theme.thumb_from, self.theme.thumb_to));
        let clipped = painter.with_clip_rect(thumb);
        clipped.circle_filled(
            pos2(thumb.right() - 40.0, thumb.top() + 26.0),
            72.0,
            // Premultiplied on purpose: from_rgba_unmultiplied premultiplies in
            // linear space, which turns a faint white into a bright one.
            Color32::from_rgba_premultiplied(26, 26, 26, 26),
        );
        clipped.circle_filled(
            pos2(thumb.left() + 54.0, thumb.bottom() + 6.0),
            86.0,
            Color32::from_rgba_premultiplied(16, 16, 16, 16),
        );
        icons::paint(
            painter,
            Rect::from_center_size(thumb.center(), vec2(44.0, 44.0)),
            Icon::Monitor,
            Color32::WHITE,
        );
        // Round the shaded corners off with the card's own color.
        painter.rect_stroke(
            thumb.expand(3.0),
            Rounding::same(11.0),
            Stroke::new(6.0_f32, self.theme.card_bg),
        );
    }

    fn show_pc_card(&mut self, ui: &mut Ui, pc: &DeviceBookmark) {
        let (rect, response) = ui.allocate_exact_size(PC_CARD, Sense::click());
        let painter = ui.painter_at(rect.expand(1.0));
        let border = if response.hovered() {
            self.theme.accent
        } else {
            self.theme.card_border
        };
        painter.rect(rect, Rounding::same(10.0), self.theme.card_bg, Stroke::new(1.0_f32, border));

        let thumb = Rect::from_min_size(rect.min + vec2(6.0, 6.0), vec2(rect.width() - 12.0, 130.0));
        self.paint_thumbnail(&painter, thumb);
        if pc.is_favorite {
            let badge = Rect::from_center_size(thumb.right_top() + vec2(-18.0, 18.0), vec2(18.0, 18.0));
            painter.circle_filled(badge.center(), 13.0, self.theme.card_bg);
            icons::paint(&painter, badge, Icon::StarFilled, FAVORITE);
        }

        painter.text(
            pos2(rect.left() + 14.0, thumb.bottom() + 22.0),
            Align2::LEFT_CENTER,
            ellipsize(pc.display_title(), 26),
            fonts::medium(14.0),
            self.theme.text_primary,
        );
        painter.text(
            pos2(rect.left() + 14.0, thumb.bottom() + 44.0),
            Align2::LEFT_CENTER,
            ellipsize(&pc.pc_name, 32),
            FontId::proportional(12.0),
            self.theme.text_secondary,
        );

        let more_rect = Rect::from_center_size(pos2(rect.right() - 24.0, thumb.bottom() + 33.0), vec2(30.0, 30.0));
        let more = ui
            .interact(more_rect, response.id.with("more"), Sense::click())
            .on_hover_text("More actions")
            .on_hover_cursor(CursorIcon::PointingHand);
        if more.hovered() {
            painter.rect_filled(more_rect, Rounding::same(6.0), self.theme.nav_hover);
        }
        icons::paint(&painter, more_rect.shrink(7.0), Icon::More, self.theme.text_secondary);

        let menu = response.id.with("menu");
        if more.clicked() {
            logging::event(format_args!("Menu of \"{}\"", pc.display_title()));
            ui.memory_mut(|m| m.toggle_popup(menu));
        } else if response.clicked() {
            self.connect_to(pc);
        }
        popup_below_widget(ui, menu, &more, PopupCloseBehavior::CloseOnClick, |ui| {
            self.pc_menu_items(ui, pc);
        });
        response
            .on_hover_cursor(CursorIcon::PointingHand)
            .context_menu(|ui| self.pc_menu_items(ui, pc));
    }

    fn pc_menu_items(&mut self, ui: &mut Ui, pc: &DeviceBookmark) {
        ui.set_min_width(200.0);
        if ui.button("Connect").clicked() {
            self.connect_to(pc);
            ui.close_menu();
        }
        if ui.button("Edit…").clicked() {
            self.add_edit_dialog.open_edit(pc);
            ui.close_menu();
        }
        let favorite = if pc.is_favorite {
            "Remove from favorites"
        } else {
            "Add to favorites"
        };
        if ui.button(favorite).clicked() {
            self.toggle_favorite(pc.id);
            ui.close_menu();
        }
        ui.separator();
        if ui
            .button(RichText::new("Delete…").color(DANGER))
            .clicked()
        {
            logging::event(format_args!(
                "Delete requested for \"{}\" ({})",
                pc.display_title(),
                pc.pc_name
            ));
            self.pending_delete = Some(pc.clone());
            ui.close_menu();
        }
    }

    fn show_delete_confirmation(&mut self, ctx: &Context) {
        let Some(pc) = self.pending_delete.clone() else {
            return;
        };
        let mut decision = None;
        egui::Window::new("Delete this PC?")
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.label(format!(
                    "\"{}\" ({}) will be removed from Remoport. The PC itself is not affected.",
                    pc.display_title(),
                    pc.pc_name
                ));
                ui.add_space(12.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let delete = egui::Button::new(RichText::new("Delete").color(Color32::WHITE)).fill(DANGER);
                    if ui.add(delete).clicked() {
                        decision = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        decision = Some(false);
                    }
                });
            });
        match decision {
            Some(true) => {
                logging::event(format_args!("Deleted \"{}\" ({})", pc.display_title(), pc.pc_name));
                self.state.bookmarks.retain(|b| b.id != pc.id);
                self.pending_delete = None;
            }
            Some(false) => {
                logging::event(format_args!("Kept \"{}\": delete cancelled", pc.display_title()));
                self.pending_delete = None;
            }
            None => {}
        }
    }

    fn show_pc_list(&mut self, ui: &mut Ui, group: &str, pcs: &[DeviceBookmark]) {
        let mut connect_target = None;
        let mut edit_target = None;
        let mut toggle_fav = None;

        egui::Grid::new(("device_list_table", group))
            .striped(true)
            .spacing([18.0, 10.0])
            .show(ui, |ui| {
                for header in ["Name", "Name or address", "Resolution", "Last connected", ""] {
                    ui.label(RichText::new(header).color(self.theme.text_secondary));
                }
                ui.end_row();

                for pc in pcs {
                    ui.label(RichText::new(pc.display_title()).color(self.theme.text_primary));
                    ui.label(&pc.pc_name);
                    ui.label(pc.resolution.display_label());
                    ui.label(
                        pc.last_connected
                            .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
                            .unwrap_or_else(|| "Never".to_string()),
                    );
                    ui.horizontal(|ui| {
                        if ui.add(self.theme.primary_button("Connect")).clicked() {
                            connect_target = Some(pc.clone());
                        }
                        if ui.button("Edit").clicked() {
                            edit_target = Some(pc.clone());
                        }
                        let star = if pc.is_favorite { "★" } else { "☆" };
                        if ui
                            .button(star)
                            .on_hover_text(if pc.is_favorite {
                                "Remove from favorites"
                            } else {
                                "Add to favorites"
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
            self.connect_to(&pc);
        }
        if let Some(pc) = edit_target {
            self.add_edit_dialog.open_edit(&pc);
        }
        if let Some(id) = toggle_fav {
            self.toggle_favorite(id);
        }
    }

    fn toggle_favorite(&mut self, id: uuid::Uuid) {
        if let Some(pc) = self.state.bookmarks.iter_mut().find(|b| b.id == id) {
            pc.is_favorite = !pc.is_favorite;
            let change = if pc.is_favorite { "Added to" } else { "Removed from" };
            logging::event(format_args!("{change} favorites: \"{}\"", pc.display_title()));
        }
    }

    fn connect_to(&mut self, pc: &DeviceBookmark) {
        logging::event(format_args!("Connect to \"{}\" ({})", pc.display_title(), pc.pc_name));
        let mut updated_pc = pc.clone();
        updated_pc.last_connected = Some(Utc::now());
        if let Some(pos) = self.state.bookmarks.iter().position(|b| b.id == pc.id) {
            self.state.bookmarks[pos] = updated_pc.clone();
        }
        self.active_session = Some(ActiveSession::new(updated_pc));
    }

    // ── Apps ────────────────────────────────────────────────────────────

    fn show_apps(&mut self, ui: &mut Ui) {
        if self.state.remote_apps.is_empty() {
            self.show_empty_state(
                ui,
                Icon::Window,
                "No apps yet",
                "Subscribe to your organization's workspace to open the apps it publishes as windows of their own.",
                Some(("Subscribe to a workspace", EmptyAction::AddWorkspace)),
            );
            return;
        }

        let query = self.search_query.trim().to_lowercase();
        let mut apps: Vec<RemoteAppItem> = self
            .state
            .remote_apps
            .iter()
            .filter(|a| query.is_empty() || a.name.to_lowercase().contains(&query))
            .filter(|a| self.workspace_filter.as_ref().is_none_or(|w| *w == a.workspace_name))
            .cloned()
            .collect();
        if apps.is_empty() {
            self.show_empty_state(
                ui,
                Icon::Search,
                "No apps match",
                "Try another name, or show all workspaces.",
                None,
            );
            return;
        }
        apps.sort_by_key(|app| app.name.to_lowercase());

        let mut workspaces: Vec<(String, Vec<RemoteAppItem>)> = Vec::new();
        for app in apps {
            match workspaces.iter_mut().find(|(name, _)| *name == app.workspace_name) {
                Some((_, list)) => list.push(app),
                None => workspaces.push((app.workspace_name.clone(), vec![app])),
            }
        }
        for (workspace, apps) in &workspaces {
            if self.section(ui, &format!("apps:{workspace}"), workspace, apps.len()) {
                Self::card_row(ui, |ui| {
                    for app in apps {
                        self.show_app_card(ui, app);
                    }
                });
            }
            ui.add_space(18.0);
        }
    }

    fn show_app_card(&mut self, ui: &mut Ui, app: &RemoteAppItem) {
        let (rect, response) = ui.allocate_exact_size(APP_CARD, Sense::click());
        let painter = ui.painter_at(rect.expand(1.0));
        let border = if response.hovered() {
            self.theme.accent
        } else {
            self.theme.card_border
        };
        painter.rect(rect, Rounding::same(10.0), self.theme.card_bg, Stroke::new(1.0_f32, border));

        let tile = Rect::from_center_size(pos2(rect.center().x, rect.top() + 44.0), vec2(52.0, 52.0));
        painter.rect_filled(tile, Rounding::same(12.0), self.theme.accent);
        match app.icon_name.as_str() {
            "terminal" => {
                painter.text(
                    tile.center(),
                    Align2::CENTER_CENTER,
                    ">_",
                    FontId::monospace(18.0),
                    Color32::WHITE,
                );
            }
            _ => icons::paint(&painter, tile.shrink(12.0), Icon::Window, Color32::WHITE),
        }
        if app.is_favorite {
            icons::paint(
                &painter,
                Rect::from_center_size(rect.right_top() + vec2(-18.0, 18.0), vec2(16.0, 16.0)),
                Icon::StarFilled,
                FAVORITE,
            );
        }

        painter.text(
            pos2(rect.center().x, rect.top() + 94.0),
            Align2::CENTER_CENTER,
            ellipsize(&app.name, 22),
            fonts::medium(13.0),
            self.theme.text_primary,
        );
        painter.text(
            pos2(rect.center().x, rect.top() + 116.0),
            Align2::CENTER_CENTER,
            ellipsize(&app.workspace_name, 26),
            FontId::proportional(11.5),
            self.theme.text_secondary,
        );

        let id = app.id;
        let favorite = if app.is_favorite {
            "Remove from favorites"
        } else {
            "Add to favorites"
        };
        let response = response.on_hover_cursor(CursorIcon::PointingHand);
        if response.clicked() {
            self.open_app(app);
        }
        response.context_menu(|ui| {
            if ui.button("Open").clicked() {
                self.open_app(app);
                ui.close_menu();
            }
            if ui.button(favorite).clicked() {
                if let Some(app) = self.state.remote_apps.iter_mut().find(|a| a.id == id) {
                    app.is_favorite = !app.is_favorite;
                    logging::event(format_args!("{favorite}: app \"{}\"", app.name));
                }
                ui.close_menu();
            }
        });
    }

    fn open_app(&mut self, app: &RemoteAppItem) {
        logging::event(format_args!("Open app \"{}\" from \"{}\"", app.name, app.workspace_name));
        let mut session = ActiveSession::new(DeviceBookmark::new(
            app.workspace_name.clone(),
            app.name.clone(),
            String::new(),
        ));
        session.open_app = Some(app.name.clone());
        self.active_session = Some(session);
    }

    // ── Settings ────────────────────────────────────────────────────────

    fn settings_heading(&self, ui: &mut Ui, text: &str) {
        ui.label(
            RichText::new(text)
                .font(fonts::medium(16.0))
                .color(self.theme.text_primary),
        );
        ui.add_space(8.0);
    }

    fn show_settings(&mut self, ui: &mut Ui) {
        self.settings_heading(ui, "Appearance");
        let mut appearance = self.theme.appearance;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            for (option, label) in [(Appearance::Light, "Light"), (Appearance::Dark, "Dark")] {
                if self.chip(ui, label, appearance == option, false).clicked() {
                    appearance = option;
                }
            }
        });
        if appearance != self.theme.appearance {
            logging::event(format_args!("Appearance: {appearance:?}"));
            self.state.appearance = appearance;
            self.theme = Theme::for_appearance(appearance);
            self.theme_applied = false;
        }

        ui.add_space(26.0);
        self.settings_heading(ui, "New PCs");
        let resolution = self.state.default_resolution;
        egui::ComboBox::from_label(RichText::new("Resolution for PCs you add").color(self.theme.text_secondary))
            .selected_text(self.state.default_resolution.display_label())
            .show_ui(ui, |ui| {
                for option in [
                    ResolutionOption::Native,
                    ResolutionOption::Fhd1080p,
                    ResolutionOption::Qhd1440p,
                    ResolutionOption::Uhd4k,
                ] {
                    ui.selectable_value(
                        &mut self.state.default_resolution,
                        option,
                        option.display_label(),
                    );
                }
            });
        if self.state.default_resolution != resolution {
            logging::event(format_args!(
                "Resolution for new PCs: {}",
                self.state.default_resolution.display_label()
            ));
        }

        ui.add_space(26.0);
        self.settings_heading(ui, "Your data");
        let location = state_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "nowhere (no home folder)".to_string());
        ui.label(
            RichText::new(format!("Your PCs and settings are saved in {location}"))
                .color(self.theme.text_secondary),
        );
        let log = logging::log_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "standard error only".to_string());
        ui.label(
            RichText::new(format!("What you do in Remoport is logged to {log}"))
                .color(self.theme.text_secondary),
        );

        ui.add_space(26.0);
        self.settings_heading(ui, "About Remoport");
        for line in [
            format!("Remoport {}", env!("CARGO_PKG_VERSION")),
            "Your remote computers and apps, from Linux.".to_string(),
            "Free software under the GNU GPL, version 3 or later.".to_string(),
            "Connections use the RDP engine of RustConn, by Anton Isaiev.".to_string(),
        ] {
            ui.label(RichText::new(line).color(self.theme.text_secondary));
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.hyperlink_to("Source code", REPO_URL).clicked() {
                logging::event("Opened the source code page");
            }
            ui.label(RichText::new("·").color(self.theme.text_secondary));
            if ui.hyperlink_to("Report a problem", FEEDBACK_URL).clicked() {
                logging::event("Opened the feedback page");
            }
        });
    }
}

/// Shortens `text` to `max` characters, ending with "…" when cut.
fn ellipsize(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let cut: String = text.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

impl Drop for RemoportApp {
    fn drop(&mut self) {
        logging::event("Remoport closed");
    }
}
