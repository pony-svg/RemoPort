use crate::ui::i18n::{resolution_label, t, tf};
use crate::ui::theme::Theme;
use egui::{vec2, Align2, Color32, Context, Ui, Window};
use remoport_core::domain::{
    AudioPlaybackMode, ColorQuality, DeviceBookmark, RedirectedFolder, ResolutionOption,
};
use remoport_core::logging;
use uuid::Uuid;

/// The display scales offered for a PC, besides the automatic one.
const SCALES: [u16; 7] = [100, 125, 150, 175, 200, 250, 300];

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
    pub draft: DeviceBookmark,
    pub new_folder_path: String,
    /// Why the folder typed could not be added.
    pub folder_error: Option<String>,
    pub validation_error: Option<String>,
}

impl Default for AddEditPcDialog {
    fn default() -> Self {
        let mut draft = DeviceBookmark::new(String::new(), String::new(), String::new());
        draft.group_name.clear();
        Self {
            is_open: false,
            is_edit: false,
            active_tab: EditPcTab::General,
            draft,
            new_folder_path: String::new(),
            folder_error: None,
            validation_error: None,
        }
    }
}

impl AddEditPcDialog {
    /// Opens an empty dialog for a new PC, starting at `resolution`.
    pub fn open_add(&mut self, resolution: ResolutionOption) {
        *self = Self::default();
        self.draft.resolution = resolution;
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
        *self = Self::default();
        self.is_open = true;
        self.is_edit = true;
        self.draft = bookmark.clone();
        self.draft.screens = self.draft.screens.clamp(1, 4);
        // The default group's name is stored in English: left empty here, the
        // field shows it translated, and saving empty keeps it.
        if self.draft.group_name == "Saved PCs" {
            self.draft.group_name.clear();
        }
    }

    pub fn show(&mut self, ctx: &Context, theme: &Theme, on_save: &mut Option<DeviceBookmark>) {
        if !self.is_open {
            return;
        }

        let title = if self.is_edit {
            t("Edit PC")
        } else {
            t("Add PC")
        };

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
                    ui.selectable_value(&mut self.active_tab, EditPcTab::General, t("General"));
                    ui.selectable_value(&mut self.active_tab, EditPcTab::Display, t("Display"));
                    ui.selectable_value(
                        &mut self.active_tab,
                        EditPcTab::DevicesAudio,
                        t("Devices & Audio"),
                    );
                    ui.selectable_value(&mut self.active_tab, EditPcTab::Folders, t("Folders"));
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
                        if ui.add(theme.primary_button(t("Save"))).clicked() {
                            if let Some(bookmark) = self.validate_and_take_draft(title) {
                                *on_save = Some(bookmark);
                                close = true;
                            }
                        }

                        if ui.button(t("Cancel")).clicked() {
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

    fn validate_and_take_draft(&mut self, title: &str) -> Option<DeviceBookmark> {
        let kdc_error = remoport_rdp::normalize_kdc_address(&self.draft.kdc).err();
        if self.draft.pc_name.trim().is_empty() {
            self.validation_error = Some(t("PC name cannot be empty").to_string());
            logging::event(format_args!("{title}: Save refused, the PC name is empty"));
            return None;
        }
        if let (true, Some(error)) = (self.draft.kerberos, kdc_error) {
            logging::event(format_args!("{title}: Save refused, {error}"));
            self.validation_error = Some(capitalize_first(&error.to_string()));
            return None;
        }
        let mut bookmark = self.draft.clone();
        bookmark.pc_name = bookmark.pc_name.trim().to_string();
        bookmark.friendly_name = bookmark.friendly_name.trim().to_string();
        let group = bookmark.group_name.trim();
        bookmark.group_name = if group.is_empty() {
            "Saved PCs".to_string()
        } else {
            group.to_string()
        };
        bookmark.username = bookmark.username.trim().to_string();
        bookmark.gateway = bookmark.gateway.trim().to_string();
        bookmark.gateway_username = bookmark.gateway_username.trim().to_string();
        bookmark.kdc = bookmark.kdc.trim().to_string();

        logging::event(format_args!(
            "{title}: saved \"{}\" ({}) in group \"{}\"",
            bookmark.display_title(),
            bookmark.pc_name,
            bookmark.group_name
        ));
        Some(bookmark)
    }

    fn show_general_tab(&mut self, ui: &mut Ui) {
        egui::Grid::new("edit_pc_general_grid")
            .num_columns(2)
            .spacing([12.0, 10.0])
            .show(ui, |ui| {
                ui.label(t("PC name:"));
                ui.add(
                    egui::TextEdit::singleline(&mut self.draft.pc_name)
                        .hint_text(t("hostname, IP, or FQDN"))
                        .desired_width(320.0),
                );
                ui.end_row();

                ui.label(t("Friendly name:"));
                ui.add(
                    egui::TextEdit::singleline(&mut self.draft.friendly_name)
                        .hint_text(t("Optional"))
                        .desired_width(320.0),
                );
                ui.end_row();

                ui.label(t("User account:"));
                ui.add(
                    egui::TextEdit::singleline(&mut self.draft.username)
                        .hint_text(t("Leave empty to be asked when connecting"))
                        .desired_width(320.0),
                );
                ui.end_row();

                ui.label(t("Group:"));
                ui.add(
                    egui::TextEdit::singleline(&mut self.draft.group_name)
                        .hint_text(t("Saved PCs"))
                        .desired_width(320.0),
                );
                ui.end_row();

                ui.label("RD Gateway:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.draft.gateway)
                        .hint_text(t("Optional, e.g. gateway.example.com"))
                        .desired_width(320.0),
                );
                ui.end_row();

                if !self.draft.gateway.trim().is_empty() {
                    ui.label(t("Gateway account:"));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.draft.gateway_username)
                            .hint_text(t("Leave empty to use the PC's account"))
                            .desired_width(320.0),
                    );
                    ui.end_row();
                }
            });
        if !self.draft.gateway.trim().is_empty() {
            ui.label(
                egui::RichText::new(t(
                    "The gateway's certificate must come from an authority this computer \
                     trusts. With an account of its own, its password is asked when connecting.",
                ))
                .size(11.5)
                .weak(),
            );
        }

        ui.add_space(12.0);
        ui.checkbox(&mut self.draft.kerberos, t("Sign in with Kerberos"));
        if self.draft.kerberos {
            ui.horizontal(|ui| {
                ui.label("KDC:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.draft.kdc)
                        .hint_text(t("Optional, e.g. dc01.example.com"))
                        .desired_width(280.0),
                );
            });
            ui.label(
                egui::RichText::new(t(
                    "For accounts that may not use NTLM. Name the PC by its DNS name and sign in \
                     as user@example.com. Without a KDC, Remoport looks in krb5.conf, then asks \
                     the domain itself.",
                ))
                .size(11.5)
                .weak(),
            );
        }
        ui.add_space(8.0);
        ui.checkbox(
            &mut self.draft.allow_without_nla,
            t("Allow signing in without Network Level Authentication"),
        );
        ui.label(
            egui::RichText::new(t(
                "Less secure: the PC shows its sign-in screen before checking who you are. \
                 Turn on only for an old PC you trust that cannot use NLA.",
            ))
            .size(11.5)
            .weak(),
        );
    }

    fn show_display_tab(&mut self, ui: &mut Ui) {
        ui.label(t("Configure remote screen resolution and display options:"));
        ui.add_space(8.0);

        egui::ComboBox::from_label(t("Resolution"))
            .selected_text(resolution_label(&self.draft.resolution))
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut self.draft.resolution,
                    ResolutionOption::Native,
                    t("Default for this display"),
                );
                ui.selectable_value(
                    &mut self.draft.resolution,
                    ResolutionOption::Fhd1080p,
                    "1920 x 1080 (1080p)",
                );
                ui.selectable_value(
                    &mut self.draft.resolution,
                    ResolutionOption::Qhd1440p,
                    "2560 x 1440 (1440p)",
                );
                ui.selectable_value(
                    &mut self.draft.resolution,
                    ResolutionOption::Uhd4k,
                    "3840 x 2160 (4K UHD)",
                );
            });

        ui.add_space(8.0);

        let automatic = tf(
            "Automatic ({}%)",
            &[&self.draft.resolution.automatic_scale(
                self.draft.hidpi_full_resolution,
                crate::ui::session::screen_scale(ui.ctx()),
            )],
        );
        egui::ComboBox::from_label(t("Scale"))
            .selected_text(match self.draft.scale_percent {
                None => automatic.clone(),
                Some(percent) => format!("{percent}%"),
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.draft.scale_percent, None, automatic);
                for percent in SCALES {
                    ui.selectable_value(
                        &mut self.draft.scale_percent,
                        Some(percent),
                        format!("{percent}%"),
                    );
                }
            });
        ui.label(
            egui::RichText::new(t(
                "Some programs already open on the PC may not adapt until you reopen them.",
            ))
            .size(11.5)
            .weak(),
        );

        ui.add_space(8.0);

        egui::ComboBox::from_label(t("Color quality"))
            .selected_text(match self.draft.color_quality {
                ColorQuality::High32Bit => t("High (32 bit)"),
                ColorQuality::Medium16Bit => t("Medium (16 bit)"),
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut self.draft.color_quality,
                    ColorQuality::High32Bit,
                    t("High (32 bit)"),
                );
                ui.selectable_value(
                    &mut self.draft.color_quality,
                    ColorQuality::Medium16Bit,
                    t("Medium (16 bit)"),
                );
            });
        if self.draft.color_quality == ColorQuality::Medium16Bit {
            ui.label(
                egui::RichText::new(t(
                    "At 16 bit the PC draws without H.264: colors lose depth and video may use \
                     more of the network.",
                ))
                .size(11.5)
                .weak(),
            );
        }

        ui.add_space(8.0);
        ui.checkbox(&mut self.draft.fit_to_window, t("Fit session to window"))
            .on_hover_text(t(
                "When off, the desktop keeps its full size and the window gets scroll bars.",
            ));
        ui.horizontal(|ui| {
            ui.label(t("Screens:"));
            for count in 1..=4u8 {
                ui.selectable_value(&mut self.draft.screens, count, count.to_string());
            }
        });
        if self.draft.screens > 1 {
            ui.label(
                egui::RichText::new(t(
                    "Remoport opens one window per screen. Move each to its screen and press \
                     Ctrl+Alt+Enter to make it fill that screen; the PC follows their sizes.",
                ))
                .size(11.5)
                .weak(),
            );
        }
        // With a fixed resolution there is nothing for it to decide.
        ui.add_enabled(
            self.draft.resolution.fixed_size().is_none(),
            egui::Checkbox::new(
                &mut self.draft.hidpi_full_resolution,
                t("Use full resolution on HiDPI displays"),
            ),
        );
    }

    fn show_devices_audio_tab(&mut self, ui: &mut Ui) {
        ui.label(t("Audio playback:"));
        ui.horizontal(|ui| {
            ui.radio_value(
                &mut self.draft.audio_playback,
                AudioPlaybackMode::LocalDevice,
                t("Play on this device"),
            );
            ui.add_enabled_ui(false, |ui| {
                ui.radio_value(
                    &mut self.draft.audio_playback,
                    AudioPlaybackMode::RemotePC,
                    t("Play on remote PC"),
                )
                .on_disabled_hover_text(t("Not available yet"));
            });
            ui.radio_value(
                &mut self.draft.audio_playback,
                AudioPlaybackMode::Disabled,
                t("Don't play sound"),
            );
        });

        ui.add_space(10.0);
        ui.label(t("Shared with this PC:"));
        ui.checkbox(&mut self.draft.share_clipboard, t("Clipboard (text)"));
        ui.add(
            egui::Label::new(
                egui::RichText::new(t(
                    "The PC gets only the text you paste into the session, and what you copy \
                     there reaches this computer only while you are using the session.",
                ))
                .weak()
                .size(12.0),
            )
            .wrap(),
        );
        ui.add_space(6.0);
        ui.checkbox(&mut self.draft.redirect_microphones, t("Microphone"));
        ui.add(
            egui::Label::new(
                egui::RichText::new(t(
                    "The PC can use it only while it asks for it, and Remoport shows when it is \
                     on, with a button to mute it.",
                ))
                .weak()
                .size(12.0),
            )
            .wrap(),
        );
        ui.add_space(6.0);
        // Not built yet: shown off, so nothing suggests they are shared.
        ui.add_enabled_ui(false, |ui| {
            for (label, value) in [
                (t("Printers"), &mut self.draft.redirect_printers),
                (t("Smart cards"), &mut self.draft.redirect_smartcards),
                (t("Cameras"), &mut self.draft.redirect_cameras),
                (t("WebAuthn / Passkeys"), &mut self.draft.redirect_webauthn),
            ] {
                *value = false;
                ui.checkbox(value, label)
                    .on_disabled_hover_text(t("Not available yet"));
            }
        });
    }

    fn show_folders_tab(&mut self, ui: &mut Ui) {
        ui.label(t(
            "Choose the local folders to share with the remote session:",
        ));
        ui.add_space(6.0);

        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.new_folder_path)
                    .hint_text("/home/user/Documents")
                    .desired_width(280.0),
            );
            if ui.button(t("+ Add Folder")).clicked() && !self.new_folder_path.trim().is_empty() {
                let path = expand_home(self.new_folder_path.trim());
                if std::path::Path::new(&path).is_dir() {
                    logging::event(format_args!("Folder added to share: {path}"));
                    self.draft.redirected_folders.push(RedirectedFolder {
                        id: Uuid::new_v4(),
                        path,
                        writable: false,
                    });
                    self.new_folder_path.clear();
                    self.folder_error = None;
                } else {
                    logging::event(format_args!(
                        "Folder not added: {path} is not a folder here"
                    ));
                    self.folder_error = Some(tf("{} is not a folder on this computer.", &[&path]));
                }
            }
        });
        if let Some(error) = &self.folder_error {
            ui.colored_label(Color32::from_rgb(0xc4, 0x2b, 0x1c), error);
        }
        ui.add(
            egui::Label::new(
                egui::RichText::new(t(
                    "The PC sees each folder as a drive and cannot reach anything outside it. \
                     Folders are read-only unless you allow changes.",
                ))
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
                if self.draft.redirected_folders.is_empty() {
                    ui.label(
                        egui::RichText::new(t("No folders added yet."))
                            .color(Color32::from_rgb(140, 140, 140)),
                    );
                } else {
                    for (i, folder) in self.draft.redirected_folders.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            ui.label(format!("📁 {}", folder.path));
                            ui.checkbox(&mut folder.writable, t("Allow changes"));
                            if ui.button("❌").clicked() {
                                remove_index = Some(i);
                            }
                        });
                    }
                }
            });

        if let Some(idx) = remove_index {
            self.draft.redirected_folders.remove(idx);
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
        Window::new(t("Subscribe to a workspace"))
            .open(&mut is_open)
            .collapsible(false)
            .resizable(false)
            .default_size(vec2(440.0, 240.0))
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.label(t("Enter the address of your organization's workspace feed, or your work email:"));
                ui.add_space(8.0);

                ui.add(
                    egui::TextEdit::singleline(&mut self.email_or_url)
                        .hint_text(t("you@company.com or https://rdweb.example.com/RDWeb/Feed/webfeed.aspx"))
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
                        if ui.add(theme.primary_button(t("Subscribe"))).clicked() {
                            let entry = self.email_or_url.trim();
                            if entry.is_empty() {
                                self.validation_message = Some(t("Please enter an email or workspace URL").to_string());
                                logging::event("Workspace dialog: Subscribe with nothing entered");
                            } else {
                                self.validation_message = Some(t("Workspace subscriptions are not available yet in this preview.").to_string());
                                let kind = if entry.contains("://") { "a feed URL" } else { "an email address" };
                                logging::event(format_args!(
                                    "Workspace dialog: Subscribe with {kind} (not available yet)"
                                ));
                            }
                        }

                        if ui.button(t("Cancel")).clicked() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn edit_preserves_unshown_fields_and_defaults_empty_group() {
        let mut pc = DeviceBookmark::new("  host.example  ".into(), "".into(), "".into());
        pc.is_favorite = true;
        pc.last_connected = Some(Utc::now());
        pc.screens = 2;

        let mut dialog = AddEditPcDialog::default();
        dialog.open_edit(&pc);
        assert_eq!(dialog.draft.group_name, "");

        let saved = dialog
            .validate_and_take_draft("Edit PC")
            .expect("valid draft");
        assert_eq!(saved.id, pc.id);
        assert_eq!(saved.is_favorite, pc.is_favorite);
        assert_eq!(saved.last_connected, pc.last_connected);
        assert_eq!(saved.pc_name, "host.example");
        assert_eq!(saved.group_name, "Saved PCs");
        assert_eq!(saved.screens, 2);
    }

    #[test]
    fn save_rejects_empty_pc_name_or_invalid_kdc() {
        let mut dialog = AddEditPcDialog::default();
        dialog.open_add(ResolutionOption::Fhd1080p);
        assert_eq!(dialog.draft.resolution, ResolutionOption::Fhd1080p);
        assert!(dialog.validate_and_take_draft("Add PC").is_none());
        assert!(dialog.validation_error.is_some());

        dialog.draft.pc_name = "server.example".into();
        dialog.draft.kerberos = true;
        dialog.draft.kdc = "ftp://invalid".into();
        dialog.validation_error = None;
        assert!(dialog.validate_and_take_draft("Add PC").is_none());
        assert!(dialog.validation_error.is_some());
    }
}
