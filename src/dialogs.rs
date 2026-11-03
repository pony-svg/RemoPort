use crate::logging;
use crate::model::{
    AudioPlaybackMode, ColorQuality, DeviceBookmark, RedirectedFolder, ResolutionOption,
};
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
    pub screens: u8,
    pub hidpi_full_resolution: bool,
    pub audio_playback: AudioPlaybackMode,
    pub redirect_microphones: bool,
    pub redirect_cameras: bool,
    pub share_clipboard: bool,
    pub redirect_printers: bool,
    pub redirect_smartcards: bool,
    pub redirect_webauthn: bool,
    pub redirected_folders: Vec<RedirectedFolder>,
    pub allow_without_nla: bool,
    pub gateway: String,
    pub gateway_username: String,
    pub kerberos: bool,
    pub kdc: String,
    pub new_folder_path: String,
    /// Why the folder typed could not be added.
    pub folder_error: Option<String>,
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
            screens: 1,
            hidpi_full_resolution: true,
            audio_playback: AudioPlaybackMode::LocalDevice,
            redirect_microphones: false,
            redirect_cameras: false,
            share_clipboard: false,
            redirect_printers: false,
            redirect_smartcards: false,
            redirect_webauthn: false,
            redirected_folders: Vec::new(),
            allow_without_nla: false,
            gateway: String::new(),
            gateway_username: String::new(),
            kerberos: false,
            kdc: String::new(),
            new_folder_path: "".to_string(),
            folder_error: None,
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
        self.screens = bookmark.screens.clamp(1, 4);
        self.hidpi_full_resolution = bookmark.hidpi_full_resolution;
        self.audio_playback = bookmark.audio_playback;
        self.redirect_microphones = bookmark.redirect_microphones;
        self.redirect_cameras = bookmark.redirect_cameras;
        self.share_clipboard = bookmark.share_clipboard;
        self.redirect_printers = bookmark.redirect_printers;
        self.redirect_smartcards = bookmark.redirect_smartcards;
        self.redirect_webauthn = bookmark.redirect_webauthn;
        self.redirected_folders = bookmark.redirected_folders.clone();
        self.allow_without_nla = bookmark.allow_without_nla;
        self.gateway = bookmark.gateway.clone();
        self.gateway_username = bookmark.gateway_username.clone();
        self.kerberos = bookmark.kerberos;
        self.kdc = bookmark.kdc.clone();
        self.original = Some(bookmark.clone());
        self.validation_error = None;
        self.folder_error = None;
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
                    ui.selectable_value(
                        &mut self.active_tab,
                        EditPcTab::DevicesAudio,
                        "Devices & Audio",
                    );
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
                            let kdc_error = remoport_rdp::normalize_kdc_address(&self.kdc).err();
                            if self.pc_name.trim().is_empty() {
                                self.validation_error = Some("PC name cannot be empty".to_string());
                                logging::event(format_args!(
                                    "{title}: Save refused, the PC name is empty"
                                ));
                            } else if let (true, Some(error)) = (self.kerberos, kdc_error) {
                                logging::event(format_args!("{title}: Save refused, {error}"));
                                self.validation_error = Some(capitalize_first(&error.to_string()));
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
                                bookmark.screens = self.screens;
                                bookmark.hidpi_full_resolution = self.hidpi_full_resolution;
                                bookmark.audio_playback = self.audio_playback;
                                bookmark.redirect_microphones = self.redirect_microphones;
                                bookmark.redirect_cameras = self.redirect_cameras;
                                bookmark.share_clipboard = self.share_clipboard;
                                bookmark.redirect_printers = self.redirect_printers;
                                bookmark.redirect_smartcards = self.redirect_smartcards;
                                bookmark.redirect_webauthn = self.redirect_webauthn;
                                bookmark.redirected_folders = self.redirected_folders.clone();
                                bookmark.allow_without_nla = self.allow_without_nla;
                                bookmark.gateway = self.gateway.trim().to_string();
                                bookmark.gateway_username =
                                    self.gateway_username.trim().to_string();
                                bookmark.kerberos = self.kerberos;
                                bookmark.kdc = self.kdc.trim().to_string();

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

                ui.label("RD Gateway:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.gateway)
                        .hint_text("Optional, e.g. gateway.example.com")
                        .desired_width(320.0),
                );
                ui.end_row();

                if !self.gateway.trim().is_empty() {
                    ui.label("Gateway account:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.gateway_username)
                            .hint_text("Leave empty to use the PC's account")
                            .desired_width(320.0),
                    );
                    ui.end_row();
                }
            });
        if !self.gateway.trim().is_empty() {
            ui.label(
                egui::RichText::new(
                    "The gateway's certificate must come from an authority this computer \
                     trusts. With an account of its own, its password is asked when connecting.",
                )
                .size(11.5)
                .weak(),
            );
        }

        ui.add_space(12.0);
        ui.checkbox(&mut self.kerberos, "Sign in with Kerberos");
        if self.kerberos {
            ui.horizontal(|ui| {
                ui.label("KDC:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.kdc)
                        .hint_text("Optional, e.g. dc01.example.com")
                        .desired_width(280.0),
                );
            });
            ui.label(
                egui::RichText::new(
                    "For accounts that may not use NTLM. Name the PC by its DNS name and sign in \
                     as user@example.com. Without a KDC, Remoport looks in krb5.conf, then asks \
                     the domain itself.",
                )
                .size(11.5)
                .weak(),
            );
        }
        ui.add_space(8.0);
        ui.checkbox(
            &mut self.allow_without_nla,
            "Allow signing in without Network Level Authentication",
        );
        ui.label(
            egui::RichText::new(
                "Less secure: the PC shows its sign-in screen before checking who you are. \
                 Turn on only for an old PC you trust that cannot use NLA.",
            )
            .size(11.5)
            .weak(),
        );
    }

    fn show_display_tab(&mut self, ui: &mut Ui) {
        ui.label("Configure remote screen resolution and display options:");
        ui.add_space(8.0);

        egui::ComboBox::from_label("Resolution")
            .selected_text(self.resolution.display_label())
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut self.resolution,
                    ResolutionOption::Native,
                    "Default for this display",
                );
                ui.selectable_value(
                    &mut self.resolution,
                    ResolutionOption::Fhd1080p,
                    "1920 x 1080 (1080p)",
                );
                ui.selectable_value(
                    &mut self.resolution,
                    ResolutionOption::Qhd1440p,
                    "2560 x 1440 (1440p)",
                );
                ui.selectable_value(
                    &mut self.resolution,
                    ResolutionOption::Uhd4k,
                    "3840 x 2160 (4K UHD)",
                );
            });

        ui.add_space(8.0);

        egui::ComboBox::from_label("Color quality")
            .selected_text(match self.color_quality {
                ColorQuality::High32Bit => "High (32 bit)",
                ColorQuality::Medium16Bit => "Medium (16 bit)",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut self.color_quality,
                    ColorQuality::High32Bit,
                    "High (32 bit)",
                );
                ui.selectable_value(
                    &mut self.color_quality,
                    ColorQuality::Medium16Bit,
                    "Medium (16 bit)",
                );
            });

        ui.add_space(8.0);
        ui.checkbox(&mut self.fit_to_window, "Fit session to window");
        ui.horizontal(|ui| {
            ui.label("Screens:");
            for count in 1..=4u8 {
                ui.selectable_value(&mut self.screens, count, count.to_string());
            }
        });
        if self.screens > 1 {
            ui.label(
                egui::RichText::new(
                    "Remoport opens one window per screen. Move each to its screen and press \
                     Ctrl+Alt+Enter to make it fill that screen; the PC follows their sizes.",
                )
                .size(11.5)
                .weak(),
            );
        }
        ui.checkbox(
            &mut self.hidpi_full_resolution,
            "Use full resolution on HiDPI displays",
        );
    }

    fn show_devices_audio_tab(&mut self, ui: &mut Ui) {
        ui.label("Audio playback:");
        ui.horizontal(|ui| {
            ui.radio_value(
                &mut self.audio_playback,
                AudioPlaybackMode::LocalDevice,
                "Play on this device",
            );
            ui.add_enabled_ui(false, |ui| {
                ui.radio_value(
                    &mut self.audio_playback,
                    AudioPlaybackMode::RemotePC,
                    "Play on remote PC",
                )
                .on_disabled_hover_text("Not available yet");
            });
            ui.radio_value(
                &mut self.audio_playback,
                AudioPlaybackMode::Disabled,
                "Don't play sound",
            );
        });

        ui.add_space(10.0);
        ui.label("Shared with this PC:");
        ui.checkbox(&mut self.share_clipboard, "Clipboard (text)");
        ui.add(
            egui::Label::new(
                egui::RichText::new(
                    "The PC gets only the text you paste into the session, and what you copy \
                     there reaches this computer only while you are using the session.",
                )
                .weak()
                .size(12.0),
            )
            .wrap(),
        );
        ui.add_space(6.0);
        ui.checkbox(&mut self.redirect_microphones, "Microphone");
        ui.add(
            egui::Label::new(
                egui::RichText::new(
                    "The PC can use it only while it asks for it, and Remoport shows when it is \
                     on, with a button to mute it.",
                )
                .weak()
                .size(12.0),
            )
            .wrap(),
        );
        ui.add_space(6.0);
        // Not built yet: shown off, so nothing suggests they are shared.
        ui.add_enabled_ui(false, |ui| {
            for (label, value) in [
                ("Printers", &mut self.redirect_printers),
                ("Smart cards", &mut self.redirect_smartcards),
                ("Cameras", &mut self.redirect_cameras),
                ("WebAuthn / Passkeys", &mut self.redirect_webauthn),
            ] {
                *value = false;
                ui.checkbox(value, label)
                    .on_disabled_hover_text("Not available yet");
            }
        });
    }

    fn show_folders_tab(&mut self, ui: &mut Ui) {
        ui.label("Choose the local folders to share with the remote session:");
        ui.add_space(6.0);

        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.new_folder_path)
                    .hint_text("/home/user/Documents")
                    .desired_width(280.0),
            );
            if ui.button("+ Add Folder").clicked() && !self.new_folder_path.trim().is_empty() {
                let path = expand_home(self.new_folder_path.trim());
                if std::path::Path::new(&path).is_dir() {
                    logging::event(format_args!("Folder added to share: {path}"));
                    self.redirected_folders.push(RedirectedFolder {
                        id: Uuid::new_v4(),
                        path,
                        writable: false,
                    });
                    self.new_folder_path.clear();
                    self.folder_error = None;
                } else {
                    self.folder_error = Some(format!("{path} is not a folder on this computer."));
                }
            }
        });
        if let Some(error) = &self.folder_error {
            ui.colored_label(Color32::from_rgb(0xc4, 0x2b, 0x1c), error);
        }
        ui.add(
            egui::Label::new(
                egui::RichText::new(
                    "The PC sees each folder as a drive and cannot reach anything outside it. \
                     Folders are read-only unless you allow changes.",
                )
                .weak()
                .size(12.0),
            )
            .wrap(),
        );

        ui.add_space(8.0);

        let mut remove_index = None;
        egui::ScrollArea::vertical()
            .max_height(140.0)
            .show(ui, |ui| {
                if self.redirected_folders.is_empty() {
                    ui.label(
                        egui::RichText::new("No folders added yet.")
                            .color(Color32::from_rgb(140, 140, 140)),
                    );
                } else {
                    for (i, folder) in self.redirected_folders.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            ui.label(format!("📁 {}", folder.path));
                            ui.checkbox(&mut folder.writable, "Allow changes");
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

/// `~/…` as a path under the home folder.
fn expand_home(path: &str) -> String {
    match (path.strip_prefix("~/"), std::env::var("HOME")) {
        (Some(rest), Ok(home)) => format!("{}/{rest}", home.trim_end_matches('/')),
        _ => path.to_string(),
    }
}

fn capitalize_first(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}
