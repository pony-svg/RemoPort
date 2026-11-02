use crate::dialogs::{AddEditPcDialog, AddWorkspaceDialog};
use crate::model::{
    AppStateData, DeviceBookmark, NavTab, RemoteAppItem, ResolutionOption, SortOption, ViewMode,
};
use crate::session::ActiveSession;
use crate::theme::{Appearance, Theme};
use chrono::Utc;
use egui::{
    vec2, Align, Align2, Color32, Context, FontId, Layout, Pos2, Rect, RichText, Rounding,
    Stroke, Ui,
};

/// Project pages linked from the sidebar and Settings.
const REPO_URL: &str = "https://github.com/pony-svg/RemoPort";
const HELP_URL: &str = "https://github.com/pony-svg/RemoPort#readme";
const FEEDBACK_URL: &str = "https://github.com/pony-svg/RemoPort/issues";

/// What the button of an empty view does.
#[derive(Clone, Copy)]
enum EmptyAction {
    AddPc,
    AddWorkspace,
}

pub struct RemoportApp {
    pub theme: Theme,
    pub state: AppStateData,
    pub active_tab: NavTab,
    pub search_query: String,
    pub add_edit_dialog: AddEditPcDialog,
    pub add_workspace_dialog: AddWorkspaceDialog,
    pub active_session: Option<ActiveSession>,
    pub quick_connect_host: String,
    /// Resolution a newly added PC starts with (Settings → New PCs).
    pub default_resolution: ResolutionOption,
    /// Whether egui has been given the current theme yet.
    theme_applied: bool,
}

impl Default for RemoportApp {
    fn default() -> Self {
        Self {
            theme: Theme::default(),
            state: AppStateData::default(),
            active_tab: NavTab::Devices,
            search_query: String::new(),
            add_edit_dialog: AddEditPcDialog::default(),
            add_workspace_dialog: AddWorkspaceDialog::default(),
            active_session: None,
            quick_connect_host: String::new(),
            default_resolution: ResolutionOption::Native,
            theme_applied: false,
        }
    }
}

impl RemoportApp {
    pub fn update(&mut self, ctx: &Context) {
        if !self.theme_applied {
            self.theme.apply(ctx);
            self.theme_applied = true;
        }

        // Handle saving from dialog
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

        if let Some(ref mut session) = self.active_session {
            let mut disconnect = false;
            egui::CentralPanel::default()
                .frame(egui::Frame::none().fill(self.theme.bg_dark))
                .show(ctx, |ui| session.render(ui, &mut disconnect));
            if disconnect {
                self.active_session = None;
            }
            return;
        }

        // Connection center. The sidebar gets a panel of its own so it spans
        // the window's full height: its bottom items stay at the bottom, and
        // the content area is not cut to the height of a row.
        egui::SidePanel::left("sidebar")
            .exact_width(220.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::none().fill(self.theme.sidebar_bg))
            .show(ctx, |ui| self.show_sidebar(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(self.theme.bg_dark))
            .show(ctx, |ui| self.show_main_content(ui));
    }

    fn show_sidebar(&mut self, ui: &mut Ui) {
        let rect = ui.max_rect();
        ui.add_space(14.0);

        // User Profile Header
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            // Avatar circle (36x36)
            let (avatar_rect, _resp) =
                ui.allocate_exact_size(vec2(36.0, 36.0), egui::Sense::hover());
            ui.painter()
                .circle_filled(avatar_rect.center(), 18.0, self.theme.accent);
            ui.painter().text(
                avatar_rect.center(),
                Align2::CENTER_CENTER,
                "👤",
                FontId::proportional(18.0),
                Color32::WHITE,
            );

            ui.vertical(|ui| {
                ui.label(
                    RichText::new(&self.state.user_account.display_name)
                        .font(FontId::proportional(12.5))
                        .strong()
                        .color(self.theme.text_primary),
                );
                ui.label(
                    RichText::new(&self.state.user_account.email)
                        .font(FontId::proportional(10.5))
                        .color(self.theme.text_secondary),
                );
            });
        });

        ui.add_space(16.0);
        ui.painter().line_segment(
            [
                Pos2::new(rect.min.x + 12.0, ui.cursor().min.y),
                Pos2::new(rect.max.x - 12.0, ui.cursor().min.y),
            ],
            Stroke::new(1.0_f32, self.theme.divider),
        );
        ui.add_space(12.0);

        // Navigation Items
        self.sidebar_nav_button(ui, NavTab::Home, "⭐  Home");
        self.sidebar_nav_button(ui, NavTab::Devices, "🖥  Devices");
        self.sidebar_nav_button(ui, NavTab::Apps, "🗖  Apps");

        // Bottom Sidebar Items
        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            ui.add_space(12.0);
            self.sidebar_nav_button(ui, NavTab::Settings, "⚙  Settings");
            ui.add_space(4.0);
            // Indented to line up with the navigation buttons above.
            for (label, url) in [("💬  Feedback", FEEDBACK_URL), ("❓  Help", HELP_URL)] {
                ui.horizontal(|ui| {
                    ui.add_space(18.0);
                    if ui.selectable_label(false, label).on_hover_text(url).clicked() {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                    }
                });
            }
        });
    }

    fn sidebar_nav_button(&mut self, ui: &mut Ui, tab: NavTab, label: &str) {
        let is_selected = self.active_tab == tab;
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            let btn_width = 200.0;
            let (rect, response) =
                ui.allocate_exact_size(vec2(btn_width, 34.0), egui::Sense::click());

            if response.clicked() {
                self.active_tab = tab;
            }

            let bg_color = if is_selected {
                self.theme.nav_selected
            } else if response.hovered() {
                self.theme.nav_hover
            } else {
                Color32::TRANSPARENT
            };

            ui.painter().rect_filled(rect, Rounding::same(6.0), bg_color);

            if is_selected {
                // Accent selection indicator pill on left edge
                let indicator = Rect::from_min_size(
                    Pos2::new(rect.min.x + 2.0, rect.min.y + 8.0),
                    vec2(3.0, 18.0),
                );
                ui.painter()
                    .rect_filled(indicator, Rounding::same(2.0), self.theme.accent);
            }

            let text_color = if is_selected {
                self.theme.text_primary
            } else {
                self.theme.text_secondary
            };

            ui.painter().text(
                Pos2::new(rect.min.x + 16.0, rect.center().y),
                Align2::LEFT_CENTER,
                label,
                FontId::proportional(13.0),
                text_color,
            );
        });
        ui.add_space(2.0);
    }

    fn show_main_content(&mut self, ui: &mut Ui) {
        ui.vertical(|ui| {
            self.show_toolbar(ui);
            ui.separator();

            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_space(14.0);
                match self.active_tab {
                    NavTab::Home => self.show_home_view(ui),
                    NavTab::Devices => self.show_devices_view(ui),
                    NavTab::Apps => self.show_apps_view(ui),
                    NavTab::Settings => self.show_settings_view(ui),
                }
                ui.add_space(14.0);
            });
        });
    }

    fn show_toolbar(&mut self, ui: &mut Ui) {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_space(12.0);

            let tab_title = match self.active_tab {
                NavTab::Home => "Home",
                NavTab::Devices => "Devices",
                NavTab::Apps => "Apps",
                NavTab::Settings => "Settings",
            };

            ui.label(
                RichText::new(tab_title)
                    .font(FontId::proportional(19.0))
                    .strong()
                    .color(self.theme.text_primary),
            );

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(16.0);

                ui.menu_button("···", |ui| self.more_menu(ui))
                    .response
                    .on_hover_text("More options");

                // Grid and list apply to the PC list only.
                if self.active_tab == NavTab::Devices {
                    ui.selectable_value(&mut self.state.view_mode, ViewMode::List, "☰")
                        .on_hover_text("List");
                    ui.selectable_value(&mut self.state.view_mode, ViewMode::Grid, "▣")
                        .on_hover_text("Grid");
                }

                ui.menu_button("➕ Add", |ui| self.add_menu(ui));

                if matches!(self.active_tab, NavTab::Devices | NavTab::Apps) {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.search_query)
                            .hint_text("🔍 Search")
                            .desired_width(180.0),
                    );
                }
            });
        });
        ui.add_space(4.0);
    }

    fn add_menu(&mut self, ui: &mut Ui) {
        if ui.button("🖥  Add a PC…").clicked() {
            self.add_edit_dialog.open_add(self.default_resolution);
            ui.close_menu();
        }
        if ui.button("🏢  Subscribe to a workspace…").clicked() {
            self.add_workspace_dialog.open();
            ui.close_menu();
        }
        if ui.button("👤  Sign in to an organization…").clicked() {
            self.add_workspace_dialog.open();
            ui.close_menu();
        }
    }

    fn more_menu(&mut self, ui: &mut Ui) {
        ui.add_enabled(false, egui::Button::new("🔄  Refresh workspaces"))
            .on_disabled_hover_text("Workspaces are not available yet");
        ui.separator();
        ui.label(RichText::new("Sort PCs by").strong());
        for (option, label) in [
            (SortOption::DeviceName, "Name"),
            (SortOption::TimeLastConnected, "Last connected"),
            (SortOption::Manual, "Order added"),
        ] {
            if ui
                .selectable_label(self.state.sort_option == option, label)
                .clicked()
            {
                self.state.sort_option = option;
                ui.close_menu();
            }
        }
    }

    /// Lays cards out left to right, wrapping, with their tops aligned.
    fn card_row(ui: &mut Ui, add_cards: impl FnOnce(&mut Ui)) {
        ui.with_layout(
            Layout::left_to_right(Align::TOP).with_main_wrap(true),
            add_cards,
        );
    }

    fn show_section_header(&self, ui: &mut Ui, text: &str) {
        ui.label(
            RichText::new(text)
                .font(FontId::proportional(14.0))
                .strong()
                .color(self.theme.text_secondary),
        );
        ui.add_space(8.0);
    }

    fn show_home_view(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            ui.vertical(|ui| {
                // Quick Connect Bar
                ui.horizontal(|ui| {
                    ui.label("⚡ Quick connect:");
                    let field = ui.add(
                        egui::TextEdit::singleline(&mut self.quick_connect_host)
                            .hint_text("Name or address of the PC"),
                    );
                    let entered =
                        field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let clicked = ui.add(self.theme.primary_button("Connect")).clicked();
                    let host = self.quick_connect_host.trim().to_string();
                    if (entered || clicked) && !host.is_empty() {
                        let pc = DeviceBookmark::new(host.clone(), host, String::new());
                        self.active_session = Some(ActiveSession::new(pc));
                    }
                });

                ui.add_space(16.0);
                ui.label(
                    RichText::new("Favorites")
                        .font(FontId::proportional(16.0))
                        .strong()
                        .color(self.theme.text_primary),
                );
                ui.add_space(8.0);

                let favorite_pcs: Vec<DeviceBookmark> = self
                    .state
                    .bookmarks
                    .iter()
                    .filter(|b| b.is_favorite)
                    .cloned()
                    .collect();

                let favorite_apps: Vec<RemoteAppItem> = self
                    .state
                    .remote_apps
                    .iter()
                    .filter(|a| a.is_favorite)
                    .cloned()
                    .collect();

                if favorite_pcs.is_empty() && favorite_apps.is_empty() {
                    self.show_empty_state(
                        ui,
                        "⭐",
                        "No favorites yet",
                        "Star a PC or an app to keep it here, one click away.",
                        None,
                    );
                } else {
                    Self::card_row(ui, |ui| {
                        for pc in favorite_pcs {
                            self.show_pc_card(ui, &pc);
                        }
                        for app in favorite_apps {
                            self.show_app_card(ui, &app);
                        }
                    });
                }
            });
        });
    }

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

    fn show_devices_view(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            ui.vertical(|ui| {
                let query = self.search_query.trim().to_lowercase();
                let mut filtered: Vec<DeviceBookmark> = self
                    .state
                    .bookmarks
                    .iter()
                    .filter(|b| {
                        query.is_empty()
                            || b.pc_name.to_lowercase().contains(&query)
                            || b.friendly_name.to_lowercase().contains(&query)
                    })
                    .cloned()
                    .collect();
                self.sort_pcs(&mut filtered);

                if filtered.is_empty() && !query.is_empty() {
                    self.show_empty_state(
                        ui,
                        "🔍",
                        "No PCs match your search",
                        "Try another name or address.",
                        None,
                    );
                    return;
                }
                if filtered.is_empty() {
                    self.show_empty_state(
                        ui,
                        "💻",
                        "No PCs yet",
                        "Add a PC by its name or address, or subscribe to your organization's workspace to see the ones it publishes for you.",
                        Some(("Add a PC…", EmptyAction::AddPc)),
                    );
                    return;
                }

                // One section per group, in the order the groups first appear.
                let mut groups: Vec<(String, Vec<DeviceBookmark>)> = Vec::new();
                for pc in filtered {
                    match groups.iter_mut().find(|(name, _)| *name == pc.group_name) {
                        Some((_, pcs)) => pcs.push(pc),
                        None => groups.push((pc.group_name.clone(), vec![pc])),
                    }
                }
                for (group, pcs) in &groups {
                    self.show_section_header(ui, &format!("{group} ({})", pcs.len()));
                    match self.state.view_mode {
                        ViewMode::Grid => Self::card_row(ui, |ui| {
                            for pc in pcs {
                                self.show_pc_card(ui, pc);
                            }
                        }),
                        ViewMode::List => self.show_pc_list(ui, group, pcs),
                    }
                    ui.add_space(12.0);
                }
            });
        });
    }

    fn show_apps_view(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            ui.vertical(|ui| {
                let query = self.search_query.trim().to_lowercase();
                let mut filtered: Vec<RemoteAppItem> = self
                    .state
                    .remote_apps
                    .iter()
                    .filter(|a| query.is_empty() || a.name.to_lowercase().contains(&query))
                    .cloned()
                    .collect();
                filtered.sort_by_key(|app| app.name.to_lowercase());

                if filtered.is_empty() && !query.is_empty() {
                    self.show_empty_state(
                        ui,
                        "🔍",
                        "No apps match your search",
                        "Try another name.",
                        None,
                    );
                    return;
                }
                if filtered.is_empty() {
                    self.show_empty_state(
                        ui,
                        "🗖",
                        "No apps yet",
                        "Subscribe to your organization's workspace to open the apps it publishes as windows of their own.",
                        Some(("Subscribe to a workspace…", EmptyAction::AddWorkspace)),
                    );
                    return;
                }

                // One section per workspace.
                let mut workspaces: Vec<(String, Vec<RemoteAppItem>)> = Vec::new();
                for app in filtered {
                    match workspaces
                        .iter_mut()
                        .find(|(name, _)| *name == app.workspace_name)
                    {
                        Some((_, apps)) => apps.push(app),
                        None => workspaces.push((app.workspace_name.clone(), vec![app])),
                    }
                }
                for (workspace, apps) in &workspaces {
                    self.show_section_header(ui, workspace);
                    Self::card_row(ui, |ui| {
                        for app in apps {
                            self.show_app_card(ui, app);
                        }
                    });
                    ui.add_space(12.0);
                }
            });
        });
    }

    fn show_settings_view(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.add_space(20.0);
            ui.vertical(|ui| {
                let heading = |text: &str| RichText::new(text).strong().font(FontId::proportional(15.0));

                ui.label(heading("Appearance"));
                ui.add_space(6.0);
                let mut appearance = self.theme.appearance;
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut appearance, Appearance::Dark, "Dark");
                    ui.selectable_value(&mut appearance, Appearance::Light, "Light");
                });
                if appearance != self.theme.appearance {
                    self.theme = Theme::for_appearance(appearance);
                    self.theme_applied = false;
                }

                ui.add_space(18.0);
                ui.label(heading("New PCs"));
                ui.add_space(6.0);
                egui::ComboBox::from_label("Resolution for PCs you add")
                    .selected_text(self.default_resolution.display_label())
                    .show_ui(ui, |ui| {
                        for option in [
                            ResolutionOption::Native,
                            ResolutionOption::Fhd1080p,
                            ResolutionOption::Qhd1440p,
                            ResolutionOption::Uhd4k,
                        ] {
                            ui.selectable_value(
                                &mut self.default_resolution,
                                option,
                                option.display_label(),
                            );
                        }
                    });

                ui.add_space(18.0);
                ui.label(heading("About Remoport"));
                ui.add_space(6.0);
                ui.label(format!("Remoport {}", env!("CARGO_PKG_VERSION")));
                ui.label("Your remote computers and apps, from Linux.");
                ui.label("Free software under the GNU GPL, version 3 or later.");
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.hyperlink_to("Source code", REPO_URL);
                    ui.label("·");
                    ui.hyperlink_to("Report a problem", FEEDBACK_URL);
                });
            });
        });
    }

    fn show_empty_state(
        &mut self,
        ui: &mut Ui,
        emoji: &str,
        title: &str,
        description: &str,
        action: Option<(&str, EmptyAction)>,
    ) {
        ui.vertical_centered(|ui| {
            ui.add_space(60.0);
            ui.label(RichText::new(emoji).font(FontId::proportional(54.0)));
            ui.add_space(10.0);
            ui.label(
                RichText::new(title)
                    .font(FontId::proportional(18.0))
                    .strong()
                    .color(self.theme.text_primary),
            );
            ui.add_space(6.0);
            ui.label(
                RichText::new(description)
                    .font(FontId::proportional(12.5))
                    .color(self.theme.text_secondary),
            );

            if let Some((text, action)) = action {
                ui.add_space(14.0);
                if ui.add(self.theme.primary_button(text)).clicked() {
                    match action {
                        EmptyAction::AddPc => self.add_edit_dialog.open_add(self.default_resolution),
                        EmptyAction::AddWorkspace => self.add_workspace_dialog.open(),
                    }
                }
            }
        });
    }

    fn toggle_favorite(&mut self, id: uuid::Uuid) {
        if let Some(pc) = self.state.bookmarks.iter_mut().find(|b| b.id == id) {
            pc.is_favorite = !pc.is_favorite;
        }
    }

    fn connect_to(&mut self, pc: &DeviceBookmark) {
        let mut updated_pc = pc.clone();
        updated_pc.last_connected = Some(Utc::now());
        if let Some(pos) = self.state.bookmarks.iter().position(|b| b.id == pc.id) {
            self.state.bookmarks[pos] = updated_pc.clone();
        }
        self.active_session = Some(ActiveSession::new(updated_pc));
    }

    fn show_pc_card(&mut self, ui: &mut Ui, pc: &DeviceBookmark) {
        let card_size = vec2(230.0, 175.0);
        let (rect, response) = ui.allocate_exact_size(card_size, egui::Sense::click());

        let is_hovered = response.hovered();
        let bg = if is_hovered {
            self.theme.card_hover
        } else {
            self.theme.card_bg
        };

        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, self.theme.card_rounding(), bg);
        painter.rect_stroke(rect, self.theme.card_rounding(), self.theme.card_stroke());

        // Thumbnail Display Box inside Card (16:9 ratio)
        let thumb_rect = Rect::from_min_size(
            Pos2::new(rect.min.x + 10.0, rect.min.y + 10.0),
            vec2(210.0, 100.0),
        );
        painter.rect_filled(thumb_rect, Rounding::same(6.0), self.theme.thumb_bg);

        // Screen illustration inside thumbnail
        painter.text(
            thumb_rect.center(),
            Align2::CENTER_CENTER,
            "🖥",
            FontId::proportional(36.0),
            self.theme.accent,
        );

        // Title and Hostname
        painter.text(
            Pos2::new(rect.min.x + 12.0, rect.min.y + 122.0),
            Align2::LEFT_CENTER,
            pc.display_title(),
            FontId::proportional(13.0),
            self.theme.text_primary,
        );

        painter.text(
            Pos2::new(rect.min.x + 12.0, rect.min.y + 140.0),
            Align2::LEFT_CENTER,
            &pc.pc_name,
            FontId::proportional(11.0),
            self.theme.text_secondary,
        );

        // Favorite star: a button of its own, on top of the card.
        let star_rect =
            Rect::from_center_size(Pos2::new(rect.max.x - 20.0, rect.min.y + 122.0), vec2(26.0, 26.0));
        let star = ui
            .interact(star_rect, response.id.with("favorite"), egui::Sense::click())
            .on_hover_text(if pc.is_favorite {
                "Remove from favorites"
            } else {
                "Add to favorites"
            });
        let star_icon = if pc.is_favorite { "★" } else { "☆" };
        let star_color = if pc.is_favorite {
            Color32::from_rgb(255, 196, 0)
        } else if star.hovered() {
            self.theme.text_primary
        } else {
            self.theme.text_secondary
        };
        painter.text(
            star_rect.center(),
            Align2::CENTER_CENTER,
            star_icon,
            FontId::proportional(16.0),
            star_color,
        );

        if star.clicked() {
            self.toggle_favorite(pc.id);
        } else if response.clicked() {
            self.connect_to(pc);
        }

        // Secondary right click to edit
        response.context_menu(|ui| {
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
            if ui.button("Delete").clicked() {
                self.state.bookmarks.retain(|b| b.id != pc.id);
                ui.close_menu();
            }
        });

        ui.add_space(8.0);
    }

    fn show_pc_list(&mut self, ui: &mut Ui, group: &str, pcs: &[DeviceBookmark]) {
        let mut connect_target = None;
        let mut edit_target = None;
        let mut toggle_fav = None;

        egui::Grid::new(("device_list_table", group))
            .striped(true)
            .spacing([16.0, 8.0])
            .show(ui, |ui| {
                ui.label(RichText::new("Name").strong());
                ui.label(RichText::new("Name or address").strong());
                ui.label(RichText::new("Resolution").strong());
                ui.label(RichText::new("Last connected").strong());
                ui.label(RichText::new("Actions").strong());
                ui.end_row();

                for pc in pcs {
                    ui.label(pc.display_title());
                    ui.label(&pc.pc_name);
                    ui.label(pc.resolution.display_label());
                    let last_conn_str = pc
                        .last_connected
                        .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
                        .unwrap_or_else(|| "Never".to_string());
                    ui.label(last_conn_str);

                    ui.horizontal(|ui| {
                        if ui.add(self.theme.primary_button("Connect")).clicked() {
                            connect_target = Some(pc.clone());
                        }
                        if ui.button("Edit").clicked() {
                            edit_target = Some(pc.clone());
                        }
                        let star = if pc.is_favorite { "★" } else { "☆" };
                        if ui.button(star).clicked() {
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

    fn show_app_card(&mut self, ui: &mut Ui, app: &RemoteAppItem) {
        let card_size = vec2(170.0, 110.0);
        let (rect, response) = ui.allocate_exact_size(card_size, egui::Sense::click());

        let is_hovered = response.hovered();
        let bg = if is_hovered {
            self.theme.card_hover
        } else {
            self.theme.card_bg
        };

        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, self.theme.card_rounding(), bg);
        painter.rect_stroke(rect, self.theme.card_rounding(), self.theme.card_stroke());

        // App Icon
        let icon_emoji = match app.icon_name.as_str() {
            "terminal" => ">_",
            "folder" => "📁",
            "browser" => "🌐",
            _ => "🗖",
        };

        painter.text(
            Pos2::new(rect.center().x, rect.min.y + 35.0),
            Align2::CENTER_CENTER,
            icon_emoji,
            FontId::proportional(28.0),
            self.theme.accent,
        );

        painter.text(
            Pos2::new(rect.center().x, rect.min.y + 72.0),
            Align2::CENTER_CENTER,
            &app.name,
            FontId::proportional(12.5),
            self.theme.text_primary,
        );

        painter.text(
            Pos2::new(rect.center().x, rect.min.y + 88.0),
            Align2::CENTER_CENTER,
            "Published app",
            FontId::proportional(10.0),
            self.theme.text_secondary,
        );

        if response.clicked() {
            let mut session = ActiveSession::new(DeviceBookmark::new(
                "apps.example.com".to_string(),
                app.name.clone(),
                "Published Apps".to_string(),
            ));
            session.open_app = Some(app.name.clone());
            self.active_session = Some(session);
        }

        ui.add_space(8.0);
    }
}
