use crate::model::{
    AudioPlaybackMode, ColorQuality, DeviceBookmark, RedirectedFolder, ResolutionOption,
};
use crate::logging;
use crate::theme::Theme;
use egui::{vec2, Align2, Color32, Context, Ui, Window};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditPcTab {
    General,
    Display,
    DevicesAudio,
    Folders,
}

pub struct AddEditPcDialog {
    pub is_open: bool,
    pub is_edit: bool,
    pub active_tab: EditPcTab,
    pub pc_name: String,
    pub friendly_name: String,
    pub group_name: String,
    pub username: String,
    pub resolution: ResolutionOption,
    pub color_quality: ColorQuality,
    pub fit_to_window: bool,
    pub use_all_monitors: bool,
    pub hidpi_full_resolution: bool,
    pub audio_playback: AudioPlaybackMode,
    pub redirect_microphones: bool,
    pub redirect_cameras: bool,
    pub redirect_clipboard: bool,
    pub redirect_printers: bool,
    pub redirect_smartcards: bool,
    pub redirect_webauthn: bool,
    pub redirected_folders: Vec<RedirectedFolder>,
    pub new_folder_path: String,
    pub validation_error: Option<String>,
    /// The PC being edited, so Save keeps what the dialog does not show
    /// (favorite, last connection, identity). `None` when adding.
    pub original: Option<DeviceBookmark>,
}

impl Default for AddEditPcDialog {
    fn default() -> Self {
        Self {
            is_open: false,
            is_edit: false,
            active_tab: EditPcTab::General,
            pc_name: "".to_string(),
            friendly_name: "".to_string(),
            group_name: "Saved PCs".to_string(),
            username: "".to_string(),
            resolution: ResolutionOption::Native,
            color_quality: ColorQuality::High32Bit,
            fit_to_window: true,
            use_all_monitors: false,
            hidpi_full_resolution: true,
            audio_playback: AudioPlaybackMode::LocalDevice,
            redirect_microphones: false,
            redirect_cameras: false,
            redirect_clipboard: true,
            redirect_printers: false,
            redirect_smartcards: false,
            redirect_webauthn: true,
            redirected_folders: Vec::new(),
            new_folder_path: "".to_string(),
            validation_error: None,
            original: None,
        }
    }
}

impl AddEditPcDialog {
    /// Opens an empty dialog for a new PC, starting at `resolution`.
    pub fn open_add(&mut self, resolution: ResolutionOption) {
        *self = Self::default();
        self.resolution = resolution;
        self.is_open = true;
        self.is_edit = false;
        logging::event("Add PC dialog opened");
    }

    pub fn open_edit(&mut self, bookmark: &DeviceBookmark) {
        logging::event(format_args!(
            "Edit PC dialog opened for \"{}\" ({})",
            bookmark.display_title(),
            bookmark.pc_name
        ));
        self.is_open = true;
        self.is_edit = true;
        self.active_tab = EditPcTab::General;
        self.pc_name = bookmark.pc_name.clone();
        self.friendly_name = bookmark.friendly_name.clone();
        self.group_name = bookmark.group_name.clone();
        self.username = bookmark.username.clone();
        self.resolution = bookmark.resolution;
        self.color_quality = bookmark.color_quality;
        self.fit_to_window = bookmark.fit_to_window;
        self.use_all_monitors = bookmark.use_all_monitors;
        self.hidpi_full_resolution = bookmark.hidpi_full_resolution;
        self.audio_playback = bookmark.audio_playback;
        self.redirect_microphones = bookmark.redirect_microphones;
        self.redirect_cameras = bookmark.redirect_cameras;
        self.redirect_clipboard = bookmark.redirect_clipboard;
        self.redirect_printers = bookmark.redirect_printers;
        self.redirect_smartcards = bookmark.redirect_smartcards;
        self.redirect_webauthn = bookmark.redirect_webauthn;
        self.redirected_folders = bookmark.redirected_folders.clone();
        self.original = Some(bookmark.clone());
        self.validation_error = None;
    }

    pub fn show(&mut self, ctx: &Context, theme: &Theme, on_save: &mut Option<DeviceBookmark>) {
        if !self.is_open {
            return;
        }

        let title = if self.is_edit { "Edit PC" } else { "Add PC" };

        // `is_open` is cleared by the window's ×; Save and Cancel set `close`.
        // Both are applied after the window is drawn.
        let mut is_open = self.is_open;
        let mut close = false;
        let previous_tab = self.active_tab;
        Window::new(title)
            .open(&mut is_open)
            .collapsible(false)
            .resizable(false)
            .default_size(vec2(520.0, 480.0))
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                // Tab Selection Bar
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.active_tab, EditPcTab::General, "General");
                    ui.selectable_value(&mut self.active_tab, EditPcTab::Display, "Display");
                    ui.selectable_value(&mut self.active_tab, EditPcTab::DevicesAudio, "Devices & Audio");
                    ui.selectable_value(&mut self.active_tab, EditPcTab::Folders, "Folders");
                });
                ui.separator();

                ui.add_space(8.0);

                match self.active_tab {
                    EditPcTab::General => self.show_general_tab(ui),
                    EditPcTab::Display => self.show_display_tab(ui),
                    EditPcTab::DevicesAudio => self.show_devices_audio_tab(ui),
                    EditPcTab::Folders => self.show_folders_tab(ui),
                }

                if let Some(ref err) = self.validation_error {
                    ui.add_space(6.0);
                    ui.colored_label(Color32::from_rgb(255, 90, 90), format!("⚠ {}", err));
                }

                ui.add_space(16.0);
                ui.separator();

                // Dialog Action Buttons
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(theme.primary_button("Save")).clicked() {
                            if self.pc_name.trim().is_empty() {
                                self.validation_error = Some("PC name cannot be empty".to_string());
                                logging::event(format_args!("{title}: Save refused, the PC name is empty"));
                            } else {
                                let fresh = DeviceBookmark::new(
                                    self.pc_name.trim().to_string(),
                                    self.friendly_name.trim().to_string(),
                                    self.group_name.trim().to_string(),
                                );
                                let mut bookmark = match &self.original {
                                    Some(original) => DeviceBookmark {
                                        pc_name: fresh.pc_name,
                                        friendly_name: fresh.friendly_name,
                                        group_name: fresh.group_name,
                                        ..original.clone()
                                    },
                                    None => fresh,
                                };
                                bookmark.username = self.username.trim().to_string();
                                bookmark.resolution = self.resolution;
                                bookmark.color_quality = self.color_quality;
                                bookmark.fit_to_window = self.fit_to_window;
                                bookmark.use_all_monitors = self.use_all_monitors;
                                bookmark.hidpi_full_resolution = self.hidpi_full_resolution;
                                bookmark.audio_playback = self.audio_playback;
                                bookmark.redirect_microphones = self.redirect_microphones;
                                bookmark.redirect_cameras = self.redirect_cameras;
                                bookmark.redirect_clipboard = self.redirect_clipboard;
                                bookmark.redirect_printers = self.redirect_printers;
                                bookmark.redirect_smartcards = self.redirect_smartcards;
                                bookmark.redirect_webauthn = self.redirect_webauthn;
                                bookmark.redirected_folders = self.redirected_folders.clone();

                                logging::event(format_args!(
                                    "{title}: saved \"{}\" ({}) in group \"{}\"",
                                    bookmark.display_title(),
                                    bookmark.pc_name,
                                    bookmark.group_name
                                ));
                                *on_save = Some(bookmark);
                                close = true;
                            }
                        }

                        if ui.button("Cancel").clicked() {
                            logging::event(format_args!("{title}: cancelled"));
                            close = true;
                        }
                    });
                });
            });

        if self.active_tab != previous_tab {
            logging::event(format_args!("{title}: tab {:?}", self.active_tab));
        }
        if !is_open {
            logging::event(format_args!("{title}: closed with ×"));
        }
        self.is_open = is_open && !close;
    }

    fn show_general_tab(&mut self, ui: &mut Ui) {
        egui::Grid::new("edit_pc_general_grid")
            .num_columns(2)
            .spacing([12.0, 10.0])
            .show(ui, |ui| {
                ui.label("PC name:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.pc_name)
                        .hint_text("hostname, IP, or FQDN")
                        .desired_width(320.0),
                );
                ui.end_row();

                ui.label("Friendly name:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.friendly_name)
                        .hint_text("Optional")
                        .desired_width(320.0),
                );
                ui.end_row();

                ui.label("User account:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.username)
                        .hint_text("Leave empty to be asked when connecting")
                        .desired_width(320.0),
                );
                ui.end_row();

                ui.label("Group:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.group_name)
                        .hint_text("Saved PCs")
                        .desired_width(320.0),
                );
                ui.end_row();
            });
    }

    fn show_display_tab(&mut self, ui: &mut Ui) {
        ui.label("Configure remote screen resolution and display options:");
        ui.add_space(8.0);

        egui::ComboBox::from_label("Resolution")
            .selected_text(self.resolution.display_label())
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.resolution, ResolutionOption::Native, "Default for this display");
                ui.selectable_value(&mut self.resolution, ResolutionOption::Fhd1080p, "1920 x 1080 (1080p)");
                ui.selectable_value(&mut self.resolution, ResolutionOption::Qhd1440p, "2560 x 1440 (1440p)");
                ui.selectable_value(&mut self.resolution, ResolutionOption::Uhd4k, "3840 x 2160 (4K UHD)");
            });

        ui.add_space(8.0);

        egui::ComboBox::from_label("Color quality")
            .selected_text(match self.color_quality {
                ColorQuality::High32Bit => "High (32 bit)",
                ColorQuality::Medium16Bit => "Medium (16 bit)",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.color_quality, ColorQuality::High32Bit, "High (32 bit)");
                ui.selectable_value(&mut self.color_quality, ColorQuality::Medium16Bit, "Medium (16 bit)");
            });

        ui.add_space(8.0);
        ui.checkbox(&mut self.fit_to_window, "Fit session to window");
        ui.checkbox(&mut self.use_all_monitors, "Use all monitors");
        ui.checkbox(&mut self.hidpi_full_resolution, "Use full resolution on HiDPI displays");
    }

    fn show_devices_audio_tab(&mut self, ui: &mut Ui) {
        ui.label("Audio playback:");
        ui.horizontal(|ui| {
            ui.radio_value(&mut self.audio_playback, AudioPlaybackMode::LocalDevice, "Play on this device");
            ui.radio_value(&mut self.audio_playback, AudioPlaybackMode::RemotePC, "Play on remote PC");
            ui.radio_value(&mut self.audio_playback, AudioPlaybackMode::Disabled, "Don't play sound");
        });

        ui.add_space(10.0);
        ui.label("Device redirection:");
        ui.checkbox(&mut self.redirect_clipboard, "Clipboard");
        ui.checkbox(&mut self.redirect_printers, "Printers");
        ui.checkbox(&mut self.redirect_smartcards, "Smart cards");
        ui.checkbox(&mut self.redirect_microphones, "Microphones");
        ui.checkbox(&mut self.redirect_cameras, "Cameras");
        ui.checkbox(&mut self.redirect_webauthn, "WebAuthn / Passkeys");
    }

    fn show_folders_tab(&mut self, ui: &mut Ui) {
        ui.label("Choose the local folders to share with the remote session:");
        ui.add_space(6.0);

        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.new_folder_path).hint_text("/home/user/Documents").desired_width(280.0));
            if ui.button("+ Add Folder").clicked() && !self.new_folder_path.trim().is_empty() {
                self.redirected_folders.push(RedirectedFolder {
                    id: Uuid::new_v4(),
                    path: self.new_folder_path.trim().to_string(),
                    read_only: false,
                });
                self.new_folder_path.clear();
            }
        });

        ui.add_space(8.0);

        let mut remove_index = None;
        egui::ScrollArea::vertical().max_height(140.0).show(ui, |ui| {
            if self.redirected_folders.is_empty() {
                ui.label(egui::RichText::new("No folders added yet.").color(Color32::from_rgb(140, 140, 140)));
            } else {
                for (i, folder) in self.redirected_folders.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(format!("📁 {}", folder.path));
                        ui.checkbox(&mut folder.read_only, "Read-only");
                        if ui.button("❌").clicked() {
                            remove_index = Some(i);
                        }
                    });
                }
            }
        });

        if let Some(idx) = remove_index {
            self.redirected_folders.remove(idx);
        }
    }
}

pub struct AddWorkspaceDialog {
    pub is_open: bool,
    pub email_or_url: String,
    pub validation_message: Option<String>,
}

impl Default for AddWorkspaceDialog {
    fn default() -> Self {
        Self {
            is_open: false,
            email_or_url: "".to_string(),
            validation_message: None,
        }
    }
}

impl AddWorkspaceDialog {
    pub fn open(&mut self) {
        self.is_open = true;
        self.email_or_url.clear();
        self.validation_message = None;
        logging::event("Workspace dialog opened");
    }

    pub fn show(&mut self, ctx: &Context, theme: &Theme) {
        if !self.is_open {
            return;
        }

        // As in the PC dialog: × clears `is_open`, Cancel sets `close`.
        let mut is_open = self.is_open;
        let mut close = false;
        Window::new("Subscribe to a workspace")
            .open(&mut is_open)
            .collapsible(false)
            .resizable(false)
            .default_size(vec2(440.0, 240.0))
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.label("Enter the address of your organization's workspace feed, or your work email:");
                ui.add_space(8.0);

                ui.add(
                    egui::TextEdit::singleline(&mut self.email_or_url)
                        .hint_text("you@company.com or https://rdweb.example.com/RDWeb/Feed/webfeed.aspx")
                        .desired_width(400.0),
                );

                if let Some(ref msg) = self.validation_message {
                    ui.add_space(6.0);
                    ui.colored_label(Color32::from_rgb(100, 200, 255), format!("ℹ {}", msg));
                }

                ui.add_space(16.0);
                ui.separator();

                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(theme.primary_button("Subscribe")).clicked() {
                            let entry = self.email_or_url.trim();
                            if entry.is_empty() {
                                self.validation_message = Some("Please enter an email or workspace URL".to_string());
                                logging::event("Workspace dialog: Subscribe with nothing entered");
                            } else {
                                self.validation_message = Some("Workspace subscriptions are not available yet in this preview.".to_string());
                                let kind = if entry.contains("://") { "a feed URL" } else { "an email address" };
                                logging::event(format_args!(
                                    "Workspace dialog: Subscribe with {kind} (not available yet)"
                                ));
                            }
                        }

                        if ui.button("Cancel").clicked() {
                            logging::event("Workspace dialog: cancelled");
                            close = true;
                        }
                    });
                });
            });

        if !is_open {
            logging::event("Workspace dialog: closed with ×");
        }
        self.is_open = is_open && !close;
    }
}
