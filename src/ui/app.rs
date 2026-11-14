//! Application state container, frame update orchestration, and shared session actions.

use crate::ui::dialogs::{AddEditPcDialog, AddWorkspaceDialog};
use crate::ui::session::ActiveSession;
use crate::ui::shell;
use crate::ui::theme::Theme;
use crate::ui::widgets;
use chrono::Utc;
use egui::{Context, Margin};
use remoport_core::domain::{AppStateData, DeviceBookmark, RemoteAppItem};
use remoport_core::logging;
use remoport_core::ports::PlatformPorts;
use remoport_core::store::{self, secure_config_dir, state_path};

/// Project pages linked from the rail, the header and Settings.
pub(super) const HELP_URL: &str = "https://github.com/pony-svg/RemoPort#readme";
pub(super) const FEEDBACK_URL: &str = "https://github.com/pony-svg/RemoPort/issues";

const RAIL_WIDTH: f32 = 84.0;

pub struct RemoportApp {
    pub(super) theme: Theme,
    pub state: AppStateData,
    pub(super) search_query: String,
    /// Group shown on the Devices page; `None` shows them all.
    pub(super) group_filter: Option<String>,
    /// Workspace shown on the Apps page; `None` shows them all.
    pub(super) workspace_filter: Option<String>,
    pub(super) add_edit_dialog: AddEditPcDialog,
    pub(super) add_workspace_dialog: AddWorkspaceDialog,
    pub(super) active_session: Option<ActiveSession>,
    pub(super) quick_connect_host: String,
    /// A PC waiting for the user to confirm its deletion.
    pub(super) pending_delete: Option<DeviceBookmark>,
    /// Whether egui has been given the current theme yet.
    pub(super) theme_applied: bool,
    /// The state as last written to disk, to save only what changed.
    last_saved: String,
    /// Platform services wired by the binary crate.
    pub(super) ports: PlatformPorts,
}

impl RemoportApp {
    /// Starts with the state saved by the previous run.
    pub fn load(ports: PlatformPorts) -> Self {
        if let Err(err) = secure_config_dir() {
            logging::event(format_args!(
                "Could not make the configuration folder private: {err}"
            ));
        }
        let state = store::load();
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
            ports,
        }
    }

    /// Ends any session, as the app closes.
    pub fn end_session(&mut self) {
        self.active_session = None;
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
        widgets::show_delete_confirmation(self, ctx);

        if let Some(ref mut session) = self.active_session {
            let mut disconnect = false;
            egui::CentralPanel::default()
                .frame(egui::Frame::none().fill(self.theme.bg_dark))
                .show(ctx, |ui| session.render(ui, &self.theme, &mut disconnect));
            if disconnect {
                // The session logs how it ended; nothing to add here.
                self.active_session = None;
                // Full screen belongs to the session: the app behind it has no
                // bar, and so no way out of it.
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
            }
        } else {
            egui::SidePanel::left("rail")
                .exact_width(RAIL_WIDTH)
                .resizable(false)
                .show_separator_line(false)
                .frame(egui::Frame::none().fill(self.theme.sidebar_bg))
                .show(ctx, |ui| shell::show_rail(self, ui));
            egui::CentralPanel::default()
                .frame(
                    egui::Frame::none()
                        .fill(self.theme.bg_dark)
                        .inner_margin(Margin {
                            left: 32.0,
                            right: 28.0,
                            top: 22.0,
                            bottom: 0.0,
                        }),
                )
                .show(ctx, |ui| shell::show_page(self, ui));
        }

        self.save_if_changed();
    }

    fn save_if_changed(&mut self) {
        let Ok(json) = serde_json::to_string_pretty(&self.state) else {
            return;
        };
        if json != self.last_saved {
            if let Err(err) = store::save(&json) {
                logging::event(format_args!("Could not save the state: {err}"));
            }
            self.last_saved = json;
        }
    }

    pub(super) fn toggle_favorite(&mut self, id: uuid::Uuid) {
        if let Some(pc) = self.state.bookmarks.iter_mut().find(|b| b.id == id) {
            pc.is_favorite = !pc.is_favorite;
            let change = if pc.is_favorite {
                "Added to"
            } else {
                "Removed from"
            };
            logging::event(format_args!(
                "{change} favorites: \"{}\"",
                pc.display_title()
            ));
        }
    }

    pub(super) fn connect_to(&mut self, pc: &DeviceBookmark) {
        logging::event(format_args!(
            "Connect to \"{}\" ({})",
            pc.display_title(),
            pc.pc_name
        ));
        let mut updated_pc = pc.clone();
        updated_pc.last_connected = Some(Utc::now());
        if let Some(pos) = self.state.bookmarks.iter().position(|b| b.id == pc.id) {
            self.state.bookmarks[pos] = updated_pc.clone();
        }
        self.active_session = Some(ActiveSession::new(updated_pc, true, self.ports.clone()));
    }

    pub(super) fn start_quick_connect(&mut self, host: String) {
        logging::event(format_args!("Quick connect to {host}"));
        let pc = DeviceBookmark::new(host.clone(), host, String::new());
        self.active_session = Some(ActiveSession::new(pc, false, self.ports.clone()));
    }

    pub(super) fn open_app(&mut self, app: &RemoteAppItem) {
        logging::event(format_args!(
            "Open app \"{}\" from \"{}\"",
            app.name, app.workspace_name
        ));
        let mut session = ActiveSession::new(
            DeviceBookmark::new(app.workspace_name.clone(), app.name.clone(), String::new()),
            false,
            self.ports.clone(),
        );
        session.open_app = Some(app.name.clone());
        self.active_session = Some(session);
    }
}

impl Drop for RemoportApp {
    fn drop(&mut self) {
        logging::event("Remoport closed");
    }
}
