use crate::dialogs::{AddEditPcDialog, AddWorkspaceDialog};
use crate::model::{
    AppStateData, DeviceBookmark, NavTab, RemoteAppItem, SortOption, ViewMode,
};
use crate::session::ActiveSession;
use crate::theme::Theme;
use chrono::Utc;
use egui::{
    vec2, Align, Align2, Color32, Context, FontId, Layout, Pos2, Rect, Rounding, Stroke, Ui,
};

pub struct RemoportApp {
    pub theme: Theme,
    pub state: AppStateData,
    pub active_tab: NavTab,
    pub search_query: String,
    pub add_menu_open: bool,
    pub more_menu_open: bool,
    pub add_edit_dialog: AddEditPcDialog,
    pub add_workspace_dialog: AddWorkspaceDialog,
    pub active_session: Option<ActiveSession>,
    pub quick_connect_host: String,
}

impl Default for RemoportApp {
    fn default() -> Self {
        Self {
            theme: Theme::default(),
            state: AppStateData::default(),
            active_tab: NavTab::Devices,
            search_query: "".to_string(),
            add_menu_open: false,
            more_menu_open: false,
            add_edit_dialog: AddEditPcDialog::default(),
            add_workspace_dialog: AddWorkspaceDialog::default(),
            active_session: None,
            quick_connect_host: "".to_string(),
        }
    }
}

impl RemoportApp {
    pub fn update(&mut self, ctx: &Context) {
        // Handle saving from dialog
        let mut saved_pc = None;
        self.add_edit_dialog.show(ctx, &mut saved_pc);
        if let Some(pc) = saved_pc {
            if let Some(pos) = self.state.bookmarks.iter().position(|b| b.id == pc.id) {
                self.state.bookmarks[pos] = pc;
            } else {
                self.state.bookmarks.push(pc);
            }
        }

        self.add_workspace_dialog.show(ctx);

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(self.theme.bg_dark))
            .show(ctx, |ui| {
                if let Some(ref mut session) = self.active_session {
                    let mut disconnect = false;
                    session.render(ui, &mut disconnect);
                    if disconnect {
                        self.active_session = None;
                    }
                    return;
                }

                // Connection center
                ui.horizontal(|ui| {
                    self.show_sidebar(ui);
                    self.show_main_content(ui);
                });
            });
    }

    fn show_sidebar(&mut self, ui: &mut Ui) {
        let sidebar_width = 220.0;
        let available_height = ui.available_height();

        ui.allocate_ui_with_layout(
            vec2(sidebar_width, available_height),
            Layout::top_down(Align::Min),
            |ui| {
                let rect = ui.available_rect_before_wrap();
                ui.painter().rect_filled(rect, Rounding::ZERO, self.theme.sidebar_bg);

                ui.add_space(14.0);

                // User Profile Header
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    // Avatar circle (36x36)
                    let (avatar_rect, _resp) = ui.allocate_exact_size(vec2(36.0, 36.0), egui::Sense::hover());
                    ui.painter().circle_filled(
                        avatar_rect.center(),
                        18.0,
                        self.theme.accent,
                    );
                    ui.painter().text(
                        avatar_rect.center(),
                        Align2::CENTER_CENTER,
                        "👤",
                        FontId::proportional(18.0),
                        Color32::WHITE,
                    );

                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new(&self.state.user_account.display_name)
                                .font(FontId::proportional(12.5))
                                .strong()
                                .color(self.theme.text_primary),
                        );
                        ui.label(
                            egui::RichText::new(&self.state.user_account.email)
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
                self.sidebar_nav_button(ui, NavTab::Apps, "🗔  Apps");

                // Bottom Sidebar Items
                ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                    ui.add_space(12.0);
                    self.sidebar_nav_button(ui, NavTab::Settings, "⚙  Settings");
                    ui.add_space(4.0);
                    let _ = ui.selectable_label(false, "💬  Feedback");
                    let _ = ui.selectable_label(false, "❓  Help");
                });
            },
        );
    }

    fn sidebar_nav_button(&mut self, ui: &mut Ui, tab: NavTab, label: &str) {
        let is_selected = self.active_tab == tab;
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            let btn_width = 200.0;
            let (rect, response) = ui.allocate_exact_size(vec2(btn_width, 34.0), egui::Sense::click());

            if response.clicked() {
                self.active_tab = tab;
            }

            let bg_color = if is_selected {
                Color32::from_rgb(0x32, 0x32, 0x32)
            } else if response.hovered() {
                Color32::from_rgb(0x26, 0x26, 0x26)
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
                ui.painter().rect_filled(indicator, Rounding::same(2.0), self.theme.accent);
            }

            let text_color = if is_selected {
                Color32::WHITE
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
                egui::RichText::new(tab_title)
                    .font(FontId::proportional(19.0))
                    .strong()
                    .color(self.theme.text_primary),
            );

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(16.0);

                // More Actions ("...") Menu
                if ui.button("···").clicked() {
                    self.more_menu_open = !self.more_menu_open;
                }

                // View Mode Toggle (Grid vs List)
                let view_label = match self.state.view_mode {
                    ViewMode::Grid => "▦ Grid",
                    ViewMode::List => "☰ List",
                };
                if ui.button(view_label).clicked() {
                    self.state.view_mode = match self.state.view_mode {
                        ViewMode::Grid => ViewMode::List,
                        ViewMode::List => ViewMode::Grid,
                    };
                }

                // Add button with menu
                if ui.button("➕ Add").clicked() {
                    self.add_menu_open = !self.add_menu_open;
                }

                // Search field
                ui.add(
                    egui::TextEdit::singleline(&mut self.search_query)
                        .hint_text("🔍 Search")
                        .desired_width(180.0),
                );
            });
        });

        // Add dropdown popup if open
        if self.add_menu_open {
            ui.horizontal(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_space(80.0);
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        if ui.button("🖥  Add a PC…").clicked() {
                            self.add_edit_dialog.open_add();
                            self.add_menu_open = false;
                        }
                        if ui.button("🏢  Subscribe to a workspace…").clicked() {
                            self.add_workspace_dialog.open();
                            self.add_menu_open = false;
                        }
                        if ui.button("👤  Sign in to an organization…").clicked() {
                            self.add_workspace_dialog.open();
                            self.add_menu_open = false;
                        }
                    });
                });
            });
        }

        // More actions popup if open
        if self.more_menu_open {
            ui.horizontal(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_space(16.0);
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        if ui.button("🔄 Refresh workspaces").clicked() {
                            self.more_menu_open = false;
                        }
                        ui.separator();
                        ui.label(egui::RichText::new("Sort by:").strong());
                        if ui.selectable_label(self.state.sort_option == SortOption::DeviceName, "Device Name").clicked() {
                            self.state.sort_option = SortOption::DeviceName;
                            self.more_menu_open = false;
                        }
                        if ui.selectable_label(self.state.sort_option == SortOption::TimeLastConnected, "Time Last Connected").clicked() {
                            self.state.sort_option = SortOption::TimeLastConnected;
                            self.more_menu_open = false;
                        }
                        if ui.selectable_label(self.state.sort_option == SortOption::Manual, "Manual").clicked() {
                            self.state.sort_option = SortOption::Manual;
                            self.more_menu_open = false;
                        }
                    });
                });
            });
        }

        ui.add_space(4.0);
    }

    fn show_home_view(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            ui.vertical(|ui| {
                // Quick Connect Bar
                ui.horizontal(|ui| {
                    ui.label("⚡ Quick Connect:");
                    ui.add(egui::TextEdit::singleline(&mut self.quick_connect_host).hint_text("Enter hostname or IP"));
                    if ui.button("Connect").clicked() && !self.quick_connect_host.trim().is_empty() {
                        let pc = DeviceBookmark::new(
                            self.quick_connect_host.trim().to_string(),
                            self.quick_connect_host.trim().to_string(),
                            "Saved PCs".to_string(),
                        );
                        self.active_session = Some(ActiveSession::new(pc));
                    }
                });

                ui.add_space(16.0);
                ui.label(
                    egui::RichText::new("Favorites")
                        .font(FontId::proportional(16.0))
                        .strong()
                        .color(self.theme.text_primary),
                );
                ui.add_space(8.0);

                let favorite_pcs: Vec<DeviceBookmark> = self.state.bookmarks
                    .iter()
                    .filter(|b| b.is_favorite)
                    .cloned()
                    .collect();

                let favorite_apps: Vec<RemoteAppItem> = self.state.remote_apps
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
                    ui.horizontal_wrapped(|ui| {
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

    fn show_devices_view(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            ui.vertical(|ui| {
                let filtered: Vec<DeviceBookmark> = self.state.bookmarks
                    .iter()
                    .filter(|b| {
                        if self.search_query.trim().is_empty() {
                            true
                        } else {
                            let q = self.search_query.to_lowercase();
                            b.pc_name.to_lowercase().contains(&q)
                                || b.friendly_name.to_lowercase().contains(&q)
                        }
                    })
                    .cloned()
                    .collect();

                if filtered.is_empty() {
                    self.show_empty_state(
                        ui,
                        "💻",
                        "No PCs yet",
                        "Add a PC by its name or address, or subscribe to your organization's workspace to see the ones it publishes for you.",
                        Some("Add a PC…"),
                    );
                } else {
                    // Group by group_name
                    ui.label(
                        egui::RichText::new(format!("Saved PCs ({})", filtered.len()))
                            .font(FontId::proportional(14.0))
                            .strong()
                            .color(self.theme.text_secondary),
                    );
                    ui.add_space(8.0);

                    match self.state.view_mode {
                        ViewMode::Grid => {
                            ui.horizontal_wrapped(|ui| {
                                for pc in &filtered {
                                    self.show_pc_card(ui, pc);
                                }
                            });
                        }
                        ViewMode::List => {
                            self.show_pc_list(ui, &filtered);
                        }
                    }
                }
            });
        });
    }

    fn show_apps_view(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            ui.vertical(|ui| {
                let filtered: Vec<RemoteAppItem> = self.state.remote_apps
                    .iter()
                    .filter(|a| {
                        if self.search_query.trim().is_empty() {
                            true
                        } else {
                            a.name.to_lowercase().contains(&self.search_query.to_lowercase())
                        }
                    })
                    .cloned()
                    .collect();

                if filtered.is_empty() {
                    self.show_empty_state(
                        ui,
                        "🗔",
                        "No apps yet",
                        "Subscribe to your organization's workspace to open the apps it publishes as windows of their own.",
                        Some("Subscribe to a workspace…"),
                    );
                } else {
                    ui.label(
                        egui::RichText::new("Published apps")
                            .font(FontId::proportional(14.0))
                            .strong()
                            .color(self.theme.text_secondary),
                    );
                    ui.add_space(8.0);

                    ui.horizontal_wrapped(|ui| {
                        for app in &filtered {
                            self.show_app_card(ui, app);
                        }
                    });
                }
            });
        });
    }

    fn show_settings_view(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.add_space(20.0);
            ui.vertical(|ui| {
                ui.label(egui::RichText::new("General Preferences").strong().font(FontId::proportional(15.0)));
                ui.add_space(8.0);

                ui.label("Appearance:");
                ui.horizontal(|ui| {
                    let _ = ui.selectable_label(true, "Dark mode");
                    let _ = ui.selectable_label(false, "Light mode");
                    let _ = ui.selectable_label(false, "System setting");
                });

                ui.add_space(16.0);
                ui.label(egui::RichText::new("Display & Resolutions").strong().font(FontId::proportional(15.0)));
                ui.label("Default single-monitor remote session resolution:");
                ui.label("• 1920 x 1080 (Full HD)");
                ui.label("• Dynamic resolution sync: Enabled");

                ui.add_space(16.0);
                ui.label(egui::RichText::new("About Remoport").strong().font(FontId::proportional(15.0)));
                ui.label(format!("Remoport {}", env!("CARGO_PKG_VERSION")));
                ui.label("Your remote computers and apps, from Linux.");
                ui.label("Free software under the GNU GPL, version 3 or later.");
            });
        });
    }

    fn show_empty_state(&mut self, ui: &mut Ui, emoji: &str, title: &str, description: &str, action_btn: Option<&str>) {
        ui.vertical_centered(|ui| {
            ui.add_space(60.0);
            ui.label(egui::RichText::new(emoji).font(FontId::proportional(54.0)));
            ui.add_space(10.0);
            ui.label(
                egui::RichText::new(title)
                    .font(FontId::proportional(18.0))
                    .strong()
                    .color(self.theme.text_primary),
            );
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(description)
                    .font(FontId::proportional(12.5))
                    .color(self.theme.text_secondary),
            );

            if let Some(btn_text) = action_btn {
                ui.add_space(14.0);
                if ui.button(egui::RichText::new(btn_text).color(Color32::WHITE)).clicked() {
                    if btn_text.contains("PC") {
                        self.add_edit_dialog.open_add();
                    } else {
                        self.add_workspace_dialog.open();
                    }
                }
            }
        });
    }

    fn show_pc_card(&mut self, ui: &mut Ui, pc: &DeviceBookmark) {
        let card_size = vec2(230.0, 175.0);
        let (rect, response) = ui.allocate_exact_size(card_size, egui::Sense::click());

        let is_hovered = response.hovered();
        let bg = if is_hovered { self.theme.card_hover } else { self.theme.card_bg };

        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, self.theme.card_rounding(), bg);
        painter.rect_stroke(rect, self.theme.card_rounding(), self.theme.card_stroke());

        // Thumbnail Display Box inside Card (16:9 ratio)
        let thumb_rect = Rect::from_min_size(
            Pos2::new(rect.min.x + 10.0, rect.min.y + 10.0),
            vec2(210.0, 100.0),
        );
        painter.rect_filled(thumb_rect, Rounding::same(6.0), Color32::from_rgb(18, 24, 38));

        // Screen illustration inside thumbnail
        painter.text(
            thumb_rect.center(),
            Align2::CENTER_CENTER,
            "🖥",
            FontId::proportional(36.0),
            Color32::from_rgb(90, 160, 240),
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

        // Favorite Star
        let star_icon = if pc.is_favorite { "★" } else { "☆" };
        let star_color = if pc.is_favorite { Color32::from_rgb(255, 215, 0) } else { self.theme.text_secondary };
        painter.text(
            Pos2::new(rect.max.x - 20.0, rect.min.y + 122.0),
            Align2::CENTER_CENTER,
            star_icon,
            FontId::proportional(16.0),
            star_color,
        );

        // Double click or click to launch
        if response.clicked() {
            let mut updated_pc = pc.clone();
            updated_pc.last_connected = Some(Utc::now());
            if let Some(pos) = self.state.bookmarks.iter().position(|b| b.id == pc.id) {
                self.state.bookmarks[pos] = updated_pc.clone();
            }
            self.active_session = Some(ActiveSession::new(updated_pc));
        }

        // Secondary right click to edit
        response.context_menu(|ui| {
            if ui.button("Connect").clicked() {
                self.active_session = Some(ActiveSession::new(pc.clone()));
                ui.close_menu();
            }
            if ui.button("Edit...").clicked() {
                self.add_edit_dialog.open_edit(pc);
                ui.close_menu();
            }
            if ui.button("Toggle Favorite").clicked() {
                if let Some(pos) = self.state.bookmarks.iter().position(|b| b.id == pc.id) {
                    self.state.bookmarks[pos].is_favorite = !self.state.bookmarks[pos].is_favorite;
                }
                ui.close_menu();
            }
            if ui.button("Delete").clicked() {
                self.state.bookmarks.retain(|b| b.id != pc.id);
                ui.close_menu();
            }
        });

        ui.add_space(8.0);
    }

    fn show_pc_list(&mut self, ui: &mut Ui, pcs: &[DeviceBookmark]) {
        let mut connect_target = None;
        let mut edit_target = None;
        let mut toggle_fav = None;

        egui::Grid::new("device_list_table")
            .striped(true)
            .spacing([16.0, 8.0])
            .show(ui, |ui| {
                ui.label(egui::RichText::new("Name").strong());
                ui.label(egui::RichText::new("Hostname / IP").strong());
                ui.label(egui::RichText::new("Resolution").strong());
                ui.label(egui::RichText::new("Last Connected").strong());
                ui.label(egui::RichText::new("Actions").strong());
                ui.end_row();

                for pc in pcs {
                    ui.label(pc.display_title());
                    ui.label(&pc.pc_name);
                    ui.label(pc.resolution.display_label());
                    let last_conn_str = pc.last_connected
                        .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
                        .unwrap_or_else(|| "Never".to_string());
                    ui.label(last_conn_str);

                    ui.horizontal(|ui| {
                        if ui.button("Connect").clicked() {
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
            self.active_session = Some(ActiveSession::new(pc));
        }
        if let Some(pc) = edit_target {
            self.add_edit_dialog.open_edit(&pc);
        }
        if let Some(id) = toggle_fav {
            if let Some(pos) = self.state.bookmarks.iter().position(|b| b.id == id) {
                self.state.bookmarks[pos].is_favorite = !self.state.bookmarks[pos].is_favorite;
            }
        }
    }

    fn show_app_card(&mut self, ui: &mut Ui, app: &RemoteAppItem) {
        let card_size = vec2(170.0, 110.0);
        let (rect, response) = ui.allocate_exact_size(card_size, egui::Sense::click());

        let is_hovered = response.hovered();
        let bg = if is_hovered { self.theme.card_hover } else { self.theme.card_bg };

        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, self.theme.card_rounding(), bg);
        painter.rect_stroke(rect, self.theme.card_rounding(), self.theme.card_stroke());

        // App Icon
        let icon_emoji = match app.icon_name.as_str() {
            "terminal" => ">_",
            "folder" => "📁",
            "browser" => "🌐",
            _ => "🗔",
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
            "RemoteApp",
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
