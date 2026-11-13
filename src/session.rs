//! A remote desktop session, drawn inside Remoport by its own RDP engine
//! (`remoport-rdp`): sign in, confirm the PC's identity when it cannot be
//! verified automatically, show the remote screen, pass keyboard and mouse,
//! follow the window's size, share the clipboard and play sound when this PC
//! has them, and bring a dropped connection back to the same session.

use crate::i18n::{t, tf};
use crate::key_grab::KeyGrab;
use crate::keymap;
use crate::logging;
use crate::model::{self, AudioPlaybackMode, DeviceBookmark};
use crate::theme::Theme;
use egui::{
    pos2, vec2, Align2, Color32, Context, Event, FontId, Modifiers, PointerButton, Rect, RichText,
    Rounding, Sense, Ui, Vec2,
};
use remoport_rdp::{
    ClipboardText, Command, ConnectConfig, Error, Event as EngineEvent, IdentityProblem,
    MouseButton, Rect as RemoteRect, SharedFolder, SoundInput, SoundOutput, TrustStore,
};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub mod display;
pub use display::*;

pub mod signin;
pub use signin::*;

pub mod controller;
pub use controller::*;

pub mod screen;
pub use screen::*;

/// How near the top edge the pointer brings up the full-screen bar, in points.
const BAR_EDGE: f32 = 4.0;

/// How long full screen says how to leave it, since the bar is hidden there.
const FULLSCREEN_HINT: Duration = Duration::from_secs(5);

const DANGER: Color32 = Color32::from_rgb(0xc4, 0x2b, 0x1c);

pub struct ActiveSession {
    pub bookmark: DeviceBookmark,
    pub open_app: Option<String>,
    pub notification_msg: Option<String>,
    /// Whether the PC is saved, so its password may be remembered.
    saved: bool,
    pub controller: SessionController,
    pub signin: SignInState,
    pub screen: ScreenState,
    /// Modifier keys held on the remote side.
    modifiers: Modifiers,
    last_pointer: Option<(u16, u16)>,
    /// Text the PC copied, until it is taken (or not) on the next frame.
    remote_copy: Option<ClipboardText>,
    /// The PC has the microphone open.
    microphone_live: bool,
    /// The user muted the microphone; the PC then gets silence.
    microphone_muted: Arc<AtomicBool>,
    /// Give the remote screen the keyboard back after a menu took it.
    refocus: bool,
    /// Until when full screen says how to leave it.
    fullscreen_hint_until: Option<Instant>,
    /// The desktop's reserved keys, held back while in full screen.
    key_grab: Option<KeyGrab>,
    /// Asked for this stretch of full screen, whether it could be had or not.
    key_grab_tried: bool,
    /// Where the bar lies while it shows over a full-screen session.
    bar_rect: Option<Rect>,
    /// Events from the physical Wayland keyboard listener.
    wayland_keys_rx: Option<std::sync::mpsc::Receiver<crate::key_grab::WaylandKeyEvent>>,
    /// State machine tracking physical and fallback keys.
    input_state: crate::input::KeyState,
    /// Keys pressed via egui fallback when Wayland is not available.
    fallback_keys_down: HashSet<keymap::Scancode>,
    /// Size, scale, and layout state for the session's displays.
    pub display: DisplayState,
}

impl ActiveSession {
    /// A session for `bookmark`; `saved` tells whether it is one of the
    /// user's PCs (whose password may be remembered) or a one-off.
    pub fn new(bookmark: DeviceBookmark, saved: bool) -> Self {
        let screens = usize::from(bookmark.screens.clamp(1, 4));
        let share_clipboard = bookmark.share_clipboard;
        let signin = SignInState::new(bookmark.username.clone(), saved, bookmark.id);
        Self {
            bookmark,
            open_app: None,
            notification_msg: None,
            saved,
            controller: SessionController::new(),
            signin,
            screen: ScreenState::new(),
            modifiers: Modifiers::NONE,
            last_pointer: None,
            remote_copy: None,
            microphone_live: false,
            microphone_muted: Arc::new(AtomicBool::new(false)),
            refocus: false,
            fullscreen_hint_until: None,
            key_grab: None,
            key_grab_tried: false,
            bar_rect: None,
            wayland_keys_rx: None,
            input_state: crate::input::KeyState::new(false, share_clipboard),
            fallback_keys_down: HashSet::new(),
            display: DisplayState::new(screens),
        }
    }

    pub fn render(&mut self, ui: &mut Ui, theme: &Theme, on_disconnect: &mut bool) {
        self.poll_keyring();
        self.poll_engine();

        // In full screen the PC gets the whole screen, and the bar shows over
        // it when the pointer reaches the top edge.
        let fullscreen = ui.ctx().input(|i| i.viewport().fullscreen.unwrap_or(false));
        if fullscreen && self.controller.is_connected() {
            self.floating_bar(&ui.ctx().clone(), on_disconnect);
        } else {
            self.bar_rect = None;
            egui::Frame::none()
                .inner_margin(egui::Margin::symmetric(12.0, 6.0))
                .show(ui, |ui| self.top_bar(ui, on_disconnect));
            ui.separator();
            if let Some(ref msg) = self.notification_msg {
                ui.colored_label(theme.accent, format!("ℹ {msg}"));
            }
        }

        if let Some(app) = self.open_app.clone() {
            self.message(
                ui,
                &tf("\"{}\" would open here", &[&app]),
                t("Apps published by workspaces are not available yet."),
            );
            return;
        }

        if !self.controller.is_connected() {
            self.key_grab = None;
            self.wayland_keys_rx = None;
            self.key_grab_tried = false;
        } else if !self.key_grab_tried {
            self.key_grab_tried = true;
            let (tx, rx) = std::sync::mpsc::channel();
            let ctx = ui.ctx().clone();
            let wakeup = Box::new(move || ctx.request_repaint());
            self.key_grab = KeyGrab::start(tx, wakeup);
            if self.key_grab.is_some() {
                self.wayland_keys_rx = Some(rx);
            }
        }
        if let Some(grab) = &self.key_grab {
            grab.set_fullscreen(fullscreen);
        }

        if let Phase::Reconnecting {
            attempt,
            at: Some(at),
        } = self.controller.phase
        {
            if Instant::now() >= at {
                self.controller.phase = Phase::Reconnecting { attempt, at: None };
                self.start(RECONNECT_TIMEOUT);
            }
        }

        match self.controller.phase {
            Phase::SignIn => self.sign_in_form(ui, theme),
            Phase::Connecting => {
                self.measure_canvas(ui);
                self.message(
                    ui,
                    &tf("Connecting to {}…", &[&self.bookmark.pc_name]),
                    t("Checking the PC's identity before signing in."),
                );
            }
            Phase::VerifyIdentity { .. } => self.identity_question(ui, theme),
            Phase::Connected => self.remote_screens(ui),
            Phase::Reconnecting { attempt, .. } => self.reconnecting(ui, attempt),
            Phase::Ended(ref reason) => {
                let reason = reason.clone();
                self.ended(ui, theme, &reason);
            }
        }

        if matches!(
            self.controller.phase,
            Phase::Connecting | Phase::Connected | Phase::Reconnecting { .. }
        ) || self.signin.keyring.is_some()
        {
            ui.ctx().request_repaint_after(Duration::from_millis(16));
        }
    }

    // ── Top bar ─────────────────────────────────────────────────────────

    /// The bar over a full-screen session: it shows when the pointer reaches
    /// the top edge, and stays while the pointer or one of its menus is there.
    /// It floats so that showing it does not resize the remote desktop.
    fn floating_bar(&mut self, ctx: &Context, on_disconnect: &mut bool) {
        let pointer = ctx.input(|i| i.pointer.latest_pos());
        let at_edge = pointer.is_some_and(|p| p.y <= BAR_EDGE);
        let on_bar = self
            .bar_rect
            .zip(pointer)
            .is_some_and(|(bar, p)| bar.expand(BAR_EDGE).contains(p));
        if !(at_edge || on_bar || ctx.memory(egui::Memory::any_popup_open)) {
            self.bar_rect = None;
            return;
        }
        let width = ctx.screen_rect().width();
        let shown = egui::Area::new(egui::Id::new("session-bar"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos2(0.0, 0.0))
            .show(ctx, |ui| {
                egui::Frame::none()
                    .fill(ctx.style().visuals.panel_fill)
                    .inner_margin(egui::Margin::symmetric(12.0, 6.0))
                    .show(ui, |ui| {
                        ui.set_width(width - 24.0);
                        self.top_bar(ui, on_disconnect);
                    });
            });
        self.bar_rect = Some(shown.response.rect);
    }

    fn top_bar(&mut self, ui: &mut Ui, on_disconnect: &mut bool) {
        ui.horizontal(|ui| {
            ui.label(format!("🖥 {}", self.bookmark.display_title()));
            ui.separator();
            ui.label(tf("Host: {}", &[&self.bookmark.pc_name]));
            ui.separator();
            ui.label(self.status());

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let connected = self.controller.is_connected();
                if ui
                    .button(if connected {
                        t("❌ Disconnect")
                    } else {
                        t("❌ Close")
                    })
                    .clicked()
                {
                    logging::event(format_args!(
                        "{} {}",
                        if connected {
                            "Disconnected from"
                        } else {
                            "Closed the session of"
                        },
                        self.bookmark.pc_name
                    ));
                    *on_disconnect = true;
                }
                if connected && self.microphone_live {
                    let muted = self.microphone_muted.load(Ordering::Relaxed);
                    let label = if muted {
                        RichText::new(t("Microphone muted"))
                    } else {
                        RichText::new(t("Microphone on")).color(DANGER)
                    };
                    if ui
                        .button(label)
                        .on_hover_text(if muted {
                            t("The PC gets silence. Click to unmute.")
                        } else {
                            t("The PC is using your microphone. Click to mute.")
                        })
                        .clicked()
                    {
                        self.microphone_muted.store(!muted, Ordering::Relaxed);
                        logging::event(if muted {
                            "Microphone unmuted"
                        } else {
                            "Microphone muted"
                        });
                    }
                }
                let fullscreen = ui.ctx().input(|i| i.viewport().fullscreen.unwrap_or(false));
                if connected
                    && ui
                        .button(if fullscreen {
                            t("⛶ Leave full screen")
                        } else {
                            t("⛶ Full screen")
                        })
                        .on_hover_text(t("Ctrl+Alt+Enter switches full screen on and off"))
                        .clicked()
                {
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fullscreen));
                    logging::event(if fullscreen {
                        "Left full screen"
                    } else {
                        "Full screen"
                    });
                    if !fullscreen {
                        self.fullscreen_hint_until = Some(Instant::now() + FULLSCREEN_HINT);
                    }
                    self.refocus = true;
                }
                if connected {
                    ui.menu_button(t("⌨ Keys"), |ui| {
                        if ui.button("Ctrl+Alt+Del").clicked() {
                            logging::event("Sent Ctrl+Alt+Del");
                            self.send(Command::CtrlAltDel);
                            self.refocus = true;
                            ui.close_menu();
                        }
                        for (label, keys) in keymap::COMBINATIONS {
                            if ui.button(t(label)).clicked() {
                                logging::event(format_args!("Sent {label}"));
                                for (key, pressed) in keymap::strokes(keys) {
                                    self.send_key(key, pressed);
                                }
                                self.refocus = true;
                                ui.close_menu();
                            }
                        }
                    })
                    .response
                    .on_hover_text(t("Key combinations this computer keeps for itself, \
                         or that cannot be typed here"));
                }
            });
        });
    }

    fn status(&self) -> String {
        match self.controller.phase {
            Phase::SignIn => t("Sign in").to_string(),
            Phase::Connecting => t("Connecting…").to_string(),
            Phase::VerifyIdentity { .. } => t("Waiting for your decision").to_string(),
            Phase::Connected => {
                let size = self
                    .screen
                    .screen
                    .as_ref()
                    .map(|s| format!(" · {}×{}", s.width, s.height))
                    .unwrap_or_default();
                let graphics = self
                    .screen
                    .graphics
                    .as_ref()
                    .map(|g| format!(" · {g}"))
                    .unwrap_or_default();
                let screens = if self.display.screens > 1 {
                    tf(" · {} screens", &[&self.display.screens])
                } else {
                    String::new()
                };
                format!("{}{size}{screens}{graphics}", t("Connected"))
            }
            Phase::Reconnecting { attempt, .. } => {
                tf("Reconnecting… ({} of {})", &[&attempt, &MAX_RECONNECTS])
            }
            Phase::Ended(_) => t("Disconnected").to_string(),
        }
    }

    // ── Keyring ─────────────────────────────────────────────────────────

    fn poll_keyring(&mut self) {
        match self.signin.poll_keyring(&self.bookmark.pc_name) {
            Some(KeyringEvent::Saved) => {
                self.notification_msg =
                    Some(t("Your password is remembered in the system keyring.").to_string());
            }
            Some(KeyringEvent::Failed(_)) => {
                self.notification_msg = Some(
                    t("The system keyring is not available, so no password is remembered.")
                        .to_string(),
                );
            }
            _ => {}
        }
    }

    // ── Sign in and connect ─────────────────────────────────────────────

    fn sign_in_form(&mut self, ui: &mut Ui, theme: &Theme) {
        self.measure_canvas(ui);
        ui.vertical_centered(|ui| {
            ui.add_space(70.0);
            ui.label(
                RichText::new(tf("Sign in to {}", &[&self.bookmark.display_title()]))
                    .font(crate::fonts::medium(22.0))
                    .color(theme.text_primary),
            );
            ui.label(RichText::new(&self.bookmark.pc_name).color(theme.text_secondary));
            ui.add_space(20.0);

            let mut submit = false;
            let mut forget = false;
            // A grid does not center itself: pad it to the middle.
            const FORM_WIDTH: f32 = 380.0;
            ui.horizontal(|ui| {
                ui.add_space(((ui.available_width() - FORM_WIDTH) / 2.0).max(0.0));
                egui::Grid::new("sign_in")
                    .num_columns(2)
                    .spacing([12.0, 10.0])
                    .show(ui, |ui| {
                        ui.label(t("User name"));
                        ui.add(
                            egui::TextEdit::singleline(&mut self.signin.username)
                                .hint_text(t("user, DOMAIN\\user or user@domain"))
                                .desired_width(280.0),
                        );
                        ui.end_row();
                        ui.label(t("Password"));
                        if self.signin.saved_password.is_some() {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(t("🔑 Saved in your keyring"))
                                        .color(theme.text_secondary),
                                );
                                forget = ui.small_button(t("Forget")).clicked();
                            });
                        } else {
                            let field = ui.add(
                                egui::TextEdit::singleline(&mut *self.signin.password)
                                    .password(true)
                                    .desired_width(280.0),
                            );
                            submit |=
                                field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        }
                        ui.end_row();
                        if self.saved && self.signin.saved_password.is_none() {
                            ui.label("");
                            ui.checkbox(
                                &mut self.signin.remember,
                                t("Remember in the system keyring"),
                            );
                            ui.end_row();
                        }
                        if self.gateway_account().is_some() {
                            ui.label(t("Gateway password"));
                            let field = ui.add(
                                egui::TextEdit::singleline(&mut *self.signin.gateway_password)
                                    .password(true)
                                    .hint_text(self.bookmark.gateway_username.trim().to_string())
                                    .desired_width(280.0),
                            );
                            submit |=
                                field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                            ui.end_row();
                        }
                    });
            });
            if forget {
                self.signin.forget_saved(self.bookmark.id);
            }

            if let Some(error) = &self.signin.sign_in_error {
                ui.add_space(8.0);
                ui.colored_label(DANGER, error);
            }
            ui.add_space(16.0);
            submit |= ui
                .add_sized([200.0, 34.0], theme.primary_button(t("Connect")))
                .clicked();
            if submit {
                self.connect();
            }
            ui.add_space(8.0);
            ui.label(
                RichText::new(t(
                    "Remoport confirms the PC's identity before your password is used.",
                ))
                .size(12.0)
                .color(theme.text_secondary),
            );
        });
    }

    fn connect(&mut self) {
        let has_gateway = self.gateway_account().is_some();
        self.signin.prepare_connect(self.saved, has_gateway);
        if self.start(STEP_TIMEOUT) {
            self.controller.phase = Phase::Connecting;
        }
    }

    /// Starts the engine with the session's password, resuming the session
    /// when the PC gave a cookie for it. Returns whether it started.
    fn start(&mut self, timeout: Duration) -> bool {
        let Some(trust_store) = model::trust_store_path().map(TrustStore::new) else {
            self.forget_session();
            self.controller.phase = Phase::Ended(
                t("Remoport has no folder to keep trusted certificates in (HOME is not set).")
                    .to_string(),
            );
            return false;
        };
        let target = self.bookmark.pc_name.trim().to_string();
        let (host, port) = split_host_port(&target);
        let username = self.signin.username.trim().to_string();
        let (domain, user) = split_domain(&username);
        let (width, height) = self.wanted_size(self.display.canvas_px);
        let scale = self.wanted_scale();
        self.display.sent_scale = scale;
        let color_depth = self.bookmark.color_quality.bits();
        logging::event(format_args!(
            "Display for the PC: scale {scale}% ({}), {color_depth}-bit color, {}{}",
            if self.bookmark.scale_percent.is_some() {
                "set for this PC"
            } else {
                "automatic"
            },
            match (
                self.bookmark.resolution.fixed_size(),
                self.bookmark.fit_to_window,
            ) {
                (Some(_), true) => "fixed size, fitted to the window",
                (Some(_), false) => "fixed size, with scroll bars",
                (None, true) => "size follows the window",
                (None, false) => "size of this display, with scroll bars",
            },
            if self
                .bookmark
                .resolution
                .uses_points(self.bookmark.hidpi_full_resolution)
            {
                ", counting points (full resolution off)"
            } else {
                ""
            },
        ));
        let password = self.signin.session_password.clone().unwrap_or_default();
        if let Some(first) = self.display.screen_px.first_mut() {
            *first = self.display.canvas_px;
        }
        let monitors: Vec<(u16, u16)> = if self.display.screens > 1 {
            self.display
                .screen_px
                .iter()
                .map(|&px| self.wanted_size(px))
                .collect()
        } else {
            Vec::new()
        };
        if !monitors.is_empty() {
            logging::event(format_args!("On {} screens: {monitors:?}", monitors.len()));
        }

        let sound = (self.bookmark.audio_playback == AudioPlaybackMode::LocalDevice)
            .then(|| Box::new(crate::audio::Speaker::new()) as Box<dyn SoundOutput>);
        let microphone = self.bookmark.redirect_microphones.then(|| {
            Box::new(crate::audio::Microphone::new(Arc::clone(
                &self.microphone_muted,
            ))) as Box<dyn SoundInput>
        });
        let folders: Vec<SharedFolder> = self
            .bookmark
            .redirected_folders
            .iter()
            .map(|folder| SharedFolder {
                path: folder.path.clone().into(),
                read_only: !folder.writable,
            })
            .collect();
        logging::event(format_args!(
            "{} {host}:{port} as {}{} at {width}×{height}{}{}{}{}{}{}",
            if self.signin.resume.is_some() {
                "Resuming the session on"
            } else {
                "Connecting to"
            },
            if user.is_empty() {
                "(no user name)"
            } else {
                user
            },
            domain.map(|d| format!(", domain {d}")).unwrap_or_default(),
            if self.signin.used_saved_password {
                ", saved password"
            } else {
                ""
            },
            if self.bookmark.allow_without_nla {
                ", NLA not required for this PC"
            } else {
                ""
            },
            if self.bookmark.share_clipboard {
                ", clipboard shared"
            } else {
                ""
            },
            if sound.is_some() {
                ", sound here"
            } else {
                ", no sound"
            },
            if microphone.is_some() {
                ", microphone allowed"
            } else {
                ""
            },
            if folders.is_empty() {
                String::new()
            } else {
                format!(", {} shared folder(s)", folders.len())
            },
        ));
        if !self.bookmark.gateway.trim().is_empty() {
            logging::event(format_args!(
                "Through the RD Gateway {}",
                self.bookmark.gateway.trim()
            ));
        }
        self.controller.start(ConnectConfig {
            host: host.to_string(),
            port,
            username: user.to_string(),
            domain: domain.map(str::to_string),
            password,
            width,
            height,
            monitors,
            scale,
            color_depth,
            allow_without_nla: self.bookmark.allow_without_nla,
            trust_store,
            timeout,
            client_name: client_name(),
            clipboard: self.bookmark.share_clipboard,
            sound,
            microphone,
            folders,
            gateway: Some(self.bookmark.gateway.trim().to_string())
                .filter(|g| !g.is_empty())
                .map(|address| remoport_rdp::Gateway {
                    address,
                    account: self.gateway_account().map(|user| {
                        (
                            user.to_string(),
                            self.signin
                                .session_gateway_password
                                .clone()
                                .unwrap_or_default(),
                        )
                    }),
                }),
            kerberos: self.bookmark.kerberos.then(|| remoport_rdp::Kerberos {
                kdc: Some(self.bookmark.kdc.trim().to_string()).filter(|k| !k.is_empty()),
            }),
            resume: self.signin.resume.clone(),
        });
        true
    }

    /// Wipes what the session kept for reconnecting.
    fn forget_session(&mut self) {
        self.signin.forget_session();
    }

    /// The gateway's own account, when this PC goes through a gateway that
    /// has one.
    fn gateway_account(&self) -> Option<&str> {
        let user = self.bookmark.gateway_username.trim();
        (!self.bookmark.gateway.trim().is_empty() && !user.is_empty()).then_some(user)
    }

    // ── Engine events ───────────────────────────────────────────────────

    fn poll_engine(&mut self) {
        let events = self.controller.poll_events();
        for event in events {
            self.handle(event);
        }
    }

    fn handle(&mut self, event: EngineEvent) {
        match event {
            EngineEvent::Progress(message) => logging::event(message),
            EngineEvent::VerifyIdentity {
                info,
                problem,
                reply,
            } => {
                logging::event(format_args!(
                    "Identity of {}:{} needs a decision ({}): {} issued by {}, SHA-256 {}",
                    info.host,
                    info.port,
                    match &problem {
                        IdentityProblem::Unknown => "not verified".to_string(),
                        IdentityProblem::Changed { previous_sha256 } => {
                            format!("CHANGED, was {previous_sha256}")
                        }
                    },
                    info.subject,
                    info.issuer,
                    info.sha256
                ));
                self.controller.phase = Phase::VerifyIdentity {
                    info,
                    problem,
                    reply: Some(reply),
                    replacement_confirmed: false,
                };
            }
            EngineEvent::Connected { width, height } => {
                logging::event(format_args!(
                    "{} {} at {width}×{height}",
                    if matches!(self.controller.phase, Phase::Reconnecting { .. }) {
                        "Reconnected to"
                    } else {
                        "Connected to"
                    },
                    self.bookmark.pc_name
                ));
                self.screen.resize(width, height);
                self.controller.phase = Phase::Connected;
                // The PC may have Caps Lock on from before. Where this computer
                // can say what its own lock keys are, that follows at once.
                self.send(Command::LockKeys {
                    caps: false,
                    num: true,
                    scroll: false,
                });
                self.signin.on_connected(self.bookmark.id);
            }
            EngineEvent::Resized { width, height } => {
                logging::event(format_args!("Remote desktop is now {width}×{height}"));
                self.screen.resize(width, height);
                // A PC that answers with another size keeps the request standing,
                // or it would be asked again and again.
                if self
                    .display
                    .requested_size
                    .is_some_and(|(w, h)| w.abs_diff(width) <= 8 && h.abs_diff(height) <= 8)
                {
                    self.display.requested_size = None;
                }
            }
            EngineEvent::Frame { rect, bgra } => {
                self.screen.apply_frame(rect, &bgra);
            }
            EngineEvent::Graphics(mode) => {
                logging::event(format_args!("Graphics: {mode}"));
                self.screen.graphics = Some(mode);
            }
            EngineEvent::DisplayControlReady => {
                logging::event("The remote desktop can follow the window's size");
                self.display.display_control = true;
            }
            EngineEvent::Pointer(pointer) => self.screen.set_pointer(pointer),
            EngineEvent::Clipboard(text) => self.remote_copy = Some(text),
            EngineEvent::Microphone(live) => self.microphone_live = live,
            EngineEvent::Monitors(mut monitors) => {
                monitors.sort_by_key(|m| (m.x, m.y));
                logging::event(format_args!(
                    "The PC's screens: {}",
                    monitors
                        .iter()
                        .map(|m| format!("{}×{} at {},{}", m.width, m.height, m.x, m.y))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
                self.display.remote_monitors = monitors;
            }
            EngineEvent::ResumeCookie(cookie) => {
                self.signin.on_resume_cookie(cookie);
            }
            EngineEvent::Ended(outcome) => self.session_ended(outcome),
        }
    }

    fn session_ended(&mut self, outcome: Option<Error>) {
        self.signin.on_session_ended();
        // Attempts made so far, when the session was up or coming back.
        let attempts = match self.controller.phase {
            Phase::Connected => Some(0),
            Phase::Reconnecting { attempt, .. } => Some(attempt),
            _ => None,
        };
        self.reset_session();
        if let (Some(done), Some(error)) = (attempts, &outcome) {
            if self.signin.session_password.is_some() && worth_retrying(error, done > 0) {
                if done < MAX_RECONNECTS {
                    let attempt = done + 1;
                    let wait = reconnect_delay(attempt);
                    logging::event(format_args!(
                        "{} ({error}); reconnecting in {} s, attempt {attempt} of {MAX_RECONNECTS}",
                        if done == 0 {
                            "Connection lost".to_string()
                        } else {
                            format!("Attempt {done} failed")
                        },
                        wait.as_secs()
                    ));
                    self.controller.phase = Phase::Reconnecting {
                        attempt,
                        at: Some(Instant::now() + wait),
                    };
                    return;
                }
                logging::event("Gave up reconnecting");
                self.forget_session();
                self.controller.phase = Phase::Ended(tf(
                    "The connection to {} was lost and could not be brought back after {} \
                     attempts ({}).",
                    &[&self.bookmark.pc_name, &MAX_RECONNECTS, error],
                ));
                return;
            }
        }
        self.forget_session();
        match outcome {
            None => {
                logging::event("Session ended");
                if !matches!(self.controller.phase, Phase::Ended(_)) {
                    self.controller.phase = Phase::Ended(t("The session ended.").to_string());
                }
            }
            Some(error) => {
                logging::event(format_args!("Session ended: {error}"));
                match error {
                    Error::Authentication(_) => {
                        let was_saved = self.signin.on_auth_failed();
                        let message = if was_saved {
                            t("The PC refused the saved password. Type it again.")
                        } else {
                            t("The PC refused the sign-in. Check the user name and password.")
                        };
                        self.signin.sign_in_error = Some(message.to_string());
                        self.controller.phase = Phase::SignIn;
                    }
                    Error::NlaUnavailable => {
                        self.controller.phase = Phase::Ended(tf(
                            "{} does not offer Network Level Authentication, so it would show \
                             its sign-in screen before checking who you are. Remoport does not \
                             connect that way unless you allow it for this PC (Edit → General), \
                             which is less secure.",
                            &[&self.bookmark.pc_name],
                        ));
                    }
                    Error::IdentityRejected => {
                        self.controller.phase = Phase::Ended(
                            t(
                                "You chose not to connect: the PC's identity was not confirmed. \
                             Nothing was sent to it.",
                            )
                            .to_string(),
                        );
                    }
                    other => self.controller.phase = Phase::Ended(explain(&other)),
                }
            }
        }
    }

    fn reset_session(&mut self) {
        self.controller.disconnect();
        self.screen.reset();
        self.display.display_control = false;
        self.display.reset();
        self.modifiers = Modifiers::NONE;
        self.remote_copy = None;
        self.microphone_live = false;
        self.key_grab = None;
        self.key_grab_tried = false;
        self.wayland_keys_rx = None;
        self.input_state.handle_leave();
        self.fallback_keys_down.clear();
        self.bar_rect = None;
    }

    fn send(&self, command: Command) {
        self.controller.send(command);
    }

    fn send_key(&self, (scancode, extended): keymap::Scancode, pressed: bool) {
        self.send(Command::Key {
            scancode,
            extended,
            pressed,
        });
    }

    fn execute_input_actions(
        &mut self,
        actions: Vec<crate::input::InputAction>,
        ctx: &egui::Context,
        fullscreen: bool,
    ) {
        for action in actions {
            match action {
                crate::input::InputAction::SendKey {
                    scancode,
                    extended,
                    pressed,
                } => {
                    if pressed {
                        match (scancode, extended) {
                            (0x5B, true) => logging::event("Sent the Windows key"),
                            (0x5C, true) => logging::event("Sent the right Windows key"),
                            (0x5D, true) => logging::event("Sent the Menu key"),
                            (0x37, true) => logging::event("Sent Print Screen"),
                            _ => {}
                        }
                    }
                    self.send(Command::Key {
                        scancode,
                        extended,
                        pressed,
                    });
                }
                crate::input::InputAction::Paste(text) => {
                    if self.bookmark.share_clipboard {
                        logging::event(format_args!(
                            "Pasted into the PC: {} characters",
                            text.chars().count()
                        ));
                        self.send(Command::Paste(ClipboardText::new(text)));
                    } else {
                        self.send(Command::Key {
                            scancode: 0x2F,
                            extended: false,
                            pressed: true,
                        });
                    }
                }
                crate::input::InputAction::ToggleFullscreen => {
                    if !fullscreen {
                        self.fullscreen_hint_until = Some(Instant::now() + FULLSCREEN_HINT);
                    }
                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fullscreen));
                    logging::event(if fullscreen {
                        "Left full screen"
                    } else {
                        "Full screen"
                    });
                }
            }
        }
    }

    fn poll_wayland_keys(&mut self, ctx: &egui::Context, focused: bool, fullscreen: bool) {
        let Some(rx) = &self.wayland_keys_rx else {
            return;
        };
        let now = Instant::now();
        let events: Vec<_> = rx.try_iter().collect();
        for event in events {
            match event {
                crate::key_grab::WaylandKeyEvent::Key { evdev, pressed } => {
                    if focused {
                        if pressed && self.input_state.is_alt_held() {
                            match evdev {
                                15 => logging::event("Sent Alt+Tab"),
                                62 => logging::event("Sent Alt+F4"),
                                _ => {}
                            }
                        }
                        let actions = self.input_state.handle_physical_key(evdev, pressed, now);
                        self.execute_input_actions(actions, ctx, fullscreen);
                    }
                }
                crate::key_grab::WaylandKeyEvent::Modifiers { caps, num } => {
                    self.send(Command::LockKeys {
                        caps,
                        num,
                        scroll: false,
                    });
                }
                crate::key_grab::WaylandKeyEvent::Enter { keys } => {
                    self.input_state.handle_enter(&keys);
                }
                crate::key_grab::WaylandKeyEvent::Leave => {
                    let actions = self.input_state.handle_leave();
                    self.execute_input_actions(actions, ctx, fullscreen);
                }
                crate::key_grab::WaylandKeyEvent::Repeat { evdev } => {
                    if focused {
                        let actions = self.input_state.handle_physical_key(evdev, true, now);
                        self.execute_input_actions(actions, ctx, fullscreen);
                    }
                }
            }
        }
    }

    // ── Identity ────────────────────────────────────────────────────────

    fn identity_question(&mut self, ui: &mut Ui, theme: &Theme) {
        let Phase::VerifyIdentity {
            info,
            problem,
            replacement_confirmed,
            ..
        } = &mut self.controller.phase
        else {
            return;
        };
        let changed = matches!(problem, IdentityProblem::Changed { .. });
        let mut decision = None;
        ui.vertical_centered(|ui| {
            ui.add_space(40.0);
            ui.set_max_width(620.0);
            let (title, color) = if changed {
                (t("Warning: this PC's certificate changed"), DANGER)
            } else {
                (
                    t("Remoport cannot confirm this PC's identity"),
                    theme.text_primary,
                )
            };
            ui.label(
                RichText::new(title)
                    .font(crate::fonts::medium(21.0))
                    .color(color),
            );
            ui.add_space(10.0);
            let explanation = if changed {
                tf(
                    "{} presents a different certificate from the one you trusted. Someone may \
                     be intercepting the connection to capture your password. Continue only if \
                     your administrator confirms the certificate was replaced.",
                    &[&info.host],
                )
            } else {
                tf(
                    "Remoport could not verify {}: {}. If this is your PC, compare the \
                     fingerprint below with the one your administrator gives you before you \
                     trust it. Nothing has been sent to it yet.",
                    &[&info.host, &info.why_unverified],
                )
            };
            ui.label(RichText::new(explanation).color(theme.text_secondary));
            ui.add_space(14.0);

            egui::Frame::none()
                .fill(theme.card_bg)
                .stroke(theme.card_stroke())
                .rounding(Rounding::same(8.0))
                .inner_margin(egui::Margin::same(14.0))
                .show(ui, |ui| {
                    egui::Grid::new("certificate")
                        .num_columns(2)
                        .spacing([14.0, 8.0])
                        .show(ui, |ui| {
                            let row = |ui: &mut Ui, label: &str, value: &str, mono: bool| {
                                ui.label(RichText::new(label).color(theme.text_secondary));
                                let text = RichText::new(value).color(theme.text_primary);
                                ui.add(
                                    egui::Label::new(if mono { text.monospace() } else { text })
                                        .wrap(),
                                );
                                ui.end_row();
                            };
                            row(ui, "PC", &format!("{}:{}", info.host, info.port), false);
                            row(ui, t("Certificate for"), &info.subject, false);
                            row(ui, t("Issued by"), &info.issuer, false);
                            row(
                                ui,
                                t("Valid"),
                                &tf("{} to {}", &[&info.valid_from, &info.valid_until]),
                                false,
                            );
                            if let IdentityProblem::Changed { previous_sha256 } = &problem {
                                row(ui, t("You trusted (SHA-256)"), previous_sha256, true);
                                row(ui, t("Now presented"), &info.sha256, true);
                            } else {
                                row(ui, t("Fingerprint (SHA-256)"), &info.sha256, true);
                            }
                        });
                });

            ui.add_space(16.0);
            if changed {
                ui.checkbox(
                    replacement_confirmed,
                    t("My administrator confirmed this certificate was replaced"),
                );
                ui.add_space(10.0);
            }
            ui.horizontal(|ui| {
                let width = 2.0 * 220.0 + ui.spacing().item_spacing.x;
                ui.add_space((ui.available_width() - width).max(0.0) / 2.0);
                if ui
                    .add_sized([220.0, 34.0], theme.primary_button(t("Don't connect")))
                    .clicked()
                {
                    decision = Some(false);
                }
                let trust = if changed {
                    egui::Button::new(
                        RichText::new(t("Trust the new certificate")).color(Color32::WHITE),
                    )
                    .fill(DANGER)
                } else {
                    egui::Button::new(t("Trust and connect"))
                };
                let enabled = !changed || *replacement_confirmed;
                if ui
                    .add_enabled_ui(enabled, |ui| ui.add_sized([220.0, 34.0], trust))
                    .inner
                    .clicked()
                {
                    decision = Some(true);
                }
            });
        });

        if let Some(trust) = decision {
            let (host, sha) = (info.host.clone(), info.sha256.clone());
            if let Phase::VerifyIdentity { reply, .. } = &mut self.controller.phase {
                if let Some(reply) = reply.take() {
                    reply.answer(trust);
                }
            }
            logging::event(format_args!(
                "{} the certificate of {host} ({sha})",
                if trust { "You trusted" } else { "You declined" }
            ));
            self.controller.phase = Phase::Connecting;
        }
    }

    // ── Remote screen ───────────────────────────────────────────────────

    fn measure_canvas(&mut self, ui: &Ui) {
        self.display.canvas_px = self.measured_px(ui);
        self.display.screen_scale = screen_scale(ui.ctx());
    }

    /// What this window measures in the desktop's pixels: its drawing area, or
    /// its whole screen when the session does not fit to the window.
    fn measured_px(&self, ui: &Ui) -> (u16, u16) {
        let ctx = ui.ctx();
        let screen = if self.bookmark.fit_to_window {
            None
        } else {
            ctx.input(|i| i.viewport().monitor_size)
        };
        let px = screen.unwrap_or_else(|| ui.available_size()) * self.desktop_pixels_per_point(ctx);
        (to_u16(px.x), to_u16(px.y))
    }

    /// How many of the desktop's pixels make one of this window's points.
    fn desktop_pixels_per_point(&self, ctx: &Context) -> f32 {
        if self
            .bookmark
            .resolution
            .uses_points(self.bookmark.hidpi_full_resolution)
        {
            1.0
        } else {
            ctx.pixels_per_point()
        }
    }

    /// The desktop size to ask for: the one set for the PC, or else what a
    /// window measures.
    fn wanted_size(&self, measured: (u16, u16)) -> (u16, u16) {
        self.display
            .wanted_size(measured, self.bookmark.resolution.fixed_size())
    }

    /// The scale for the PC: the one set for it, or else the automatic one.
    fn wanted_scale(&self) -> u32 {
        self.display.wanted_scale(
            self.bookmark.scale_percent,
            &self.bookmark.resolution,
            self.bookmark.hidpi_full_resolution,
        )
    }

    /// The remote desktop: the main window shows the first screen, and each
    /// further screen gets a window of its own.
    fn remote_screens(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let mut focus = self.remote_screen(ui, 0);
        for index in 1..self.display.screens {
            let title = tf(
                "{} · screen {}",
                &[&self.bookmark.display_title(), &(index + 1)],
            );
            let builder = egui::ViewportBuilder::default()
                .with_title(title)
                .with_inner_size([960.0, 600.0]);
            let id = egui::ViewportId::from_hash_of(("remoport-screen", index));
            ctx.show_viewport_immediate(id, builder, |ctx, _| {
                // The session ends from the main window; a screen's stays.
                if ctx.input(|i| i.viewport().close_requested()) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                }
                egui::CentralPanel::default()
                    .frame(egui::Frame::none().fill(Color32::BLACK))
                    .show(ctx, |ui| {
                        if let Some(found) = self.remote_screen(ui, index) {
                            focus = Some(found);
                        }
                    });
            });
        }
        let has_wayland = self.key_grab.is_some();
        if !has_wayland {
            self.sync_modifiers(focus.map_or(Modifiers::NONE, |(modifiers, _)| modifiers));
        }
        // The session lost the keyboard: release all held keys.
        if focus.is_none() {
            if has_wayland {
                let fullscreen = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
                let actions = self.input_state.handle_leave();
                self.execute_input_actions(actions, &ctx, fullscreen);
            } else {
                for key in std::mem::take(&mut self.fallback_keys_down) {
                    self.send_key(key, false);
                }
            }
        }
        if has_wayland {
            let fullscreen = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
            self.input_state.set_fullscreen(fullscreen);
            self.input_state
                .set_share_clipboard(self.bookmark.share_clipboard);
            self.poll_wayland_keys(&ctx, focus.is_some(), fullscreen);
            let tick_actions = self.input_state.tick(Instant::now());
            self.execute_input_actions(tick_actions, &ctx, fullscreen);
        }
        if let Some(text) = self.remote_copy.take() {
            // Only while the user works in the session: a PC copying on its
            // own does not get to replace this computer's clipboard.
            if focus.is_some_and(|(_, window_focused)| window_focused) {
                logging::event(format_args!(
                    "Copied from the PC: {} characters",
                    text.as_str().chars().count()
                ));
                ctx.output_mut(|o| o.copied_text = text.as_str().to_string());
            } else {
                logging::event("The PC's clipboard was not taken: the session was not in use");
            }
        }
        if self.display.screens > 1 {
            self.follow_layout();
        } else {
            self.follow_canvas();
        }
    }

    /// One window's part of the remote desktop, with its input. Returns the
    /// modifiers held and whether the window has the system's focus, when
    /// this window has the keyboard.
    fn remote_screen(&mut self, ui: &mut Ui, index: usize) -> Option<(Modifiers, bool)> {
        let px = self.measured_px(ui);
        if index == 0 {
            self.display.canvas_px = px;
            // ponytail: one scale for the whole session, the main window's;
            // screens with different scales would need one per screen here.
            self.display.screen_scale = screen_scale(ui.ctx());
        }
        if let Some(slot) = self.display.screen_px.get_mut(index) {
            *slot = px;
        }

        let ctx = ui.ctx().clone();
        let per_point = self.desktop_pixels_per_point(&ctx);
        let region = self.region(index);
        // The picture at its own size in this window.
        let size = region.map(|r| vec2(f32::from(r.width), f32::from(r.height)) / per_point);
        let room = ui.available_size();
        let (rect, scrolled) =
            match size.filter(|size| !self.bookmark.fit_to_window && !fits_window(*size, room)) {
                Some(size) => scroll_bars(ui, index, size),
                None => (
                    ui.allocate_exact_size(ui.available_size(), Sense::hover())
                        .0,
                    Vec2::ZERO,
                ),
            };
        // An id of its own: an automatic one depends on what came before in the
        // window, which changes as the bar hides in full screen, and with it
        // the screen would lose the keyboard on every switch.
        let response = ui.interact(
            rect,
            egui::Id::new(("session-screen", index)),
            Sense::click_and_drag(),
        );
        ui.painter()
            .rect_filled(rect, Rounding::ZERO, Color32::BLACK);
        let (region, size) = (region?, size?);
        let screen = self.screen.screen.as_mut()?;
        let (desktop_width, desktop_height) = (screen.width as f32, screen.height as f32);
        let (width, height) = (f32::from(region.width), f32::from(region.height));
        let placement = if !self.bookmark.fit_to_window {
            Placement::Scrolled(scrolled)
        } else if self.bookmark.resolution.fixed_size().is_some() {
            Placement::Fitted
        } else {
            Placement::Following
        };
        let image = picture_rect(rect, size, placement);
        let scale = image.width() / size.x;
        let texture = screen.texture(&ctx).id();
        let uv = Rect::from_min_max(
            pos2(
                f32::from(region.x) / desktop_width,
                f32::from(region.y) / desktop_height,
            ),
            pos2(
                (f32::from(region.x) + width) / desktop_width,
                (f32::from(region.y) + height) / desktop_height,
            ),
        );
        ui.painter_at(rect)
            .image(texture, image, uv, Color32::WHITE);

        let fullscreen = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
        if fullscreen
            && self
                .fullscreen_hint_until
                .is_some_and(|until| Instant::now() < until)
        {
            // On a dark plate: the remote desktop can be any color.
            let painter = ui.painter();
            let text = painter.layout_no_wrap(
                t("Ctrl+Alt+Enter leaves full screen; the bar shows at the top edge").to_string(),
                FontId::proportional(14.0),
                Color32::WHITE,
            );
            let at =
                Align2::CENTER_TOP.anchor_size(rect.center_top() + vec2(0.0, 16.0), text.size());
            painter.rect_filled(
                at.expand(8.0),
                Rounding::same(6.0),
                Color32::from_black_alpha(190),
            );
            painter.galley(at.min, text, Color32::WHITE);
        }
        if index > 0 && !fullscreen {
            ui.painter().text(
                rect.center_top() + vec2(0.0, 12.0),
                Align2::CENTER_TOP,
                t("Move this window to its screen, then press Ctrl+Alt+Enter"),
                FontId::proportional(13.0),
                Color32::from_white_alpha(210),
            );
        }
        if response.clicked()
            || response.drag_started()
            || (index == 0 && std::mem::take(&mut self.refocus))
        {
            response.request_focus();
        }
        let focused = response.has_focus();
        if focused {
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    response.id,
                    egui::EventFilter {
                        tab: true,
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        escape: true,
                    },
                );
            });
        } else if response.hovered() {
            ui.painter().text(
                rect.center_bottom() - vec2(0.0, 16.0),
                Align2::CENTER_BOTTOM,
                t("Click the remote screen to type into it"),
                FontId::proportional(12.0),
                Color32::from_white_alpha(160),
            );
        }
        self.screen.flush_pointer(&ctx);
        // Drawn here at the pointer's place, so it moves with no delay and
        // looks as the PC wants: an I-beam over text, a hand over a link.
        if response.hovered() || response.dragged() {
            let hover = ctx.input(|i| i.pointer.hover_pos());
            let over_picture = hover.filter(|pos| rect.contains(*pos) && image.contains(*pos));
            match (&self.screen.pointer, over_picture) {
                (PointerLook::Hidden, Some(_)) => ctx.set_cursor_icon(egui::CursorIcon::None),
                (
                    PointerLook::Drawn {
                        texture,
                        size,
                        hotspot,
                    },
                    Some(pos),
                ) => {
                    ctx.set_cursor_icon(egui::CursorIcon::None);
                    ui.painter().image(
                        texture.id(),
                        pointer_rect(pos, *size, *hotspot, scale / per_point),
                        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }
                _ => {}
            }
        }

        let bar = self.bar_rect.filter(|_| index == 0);
        let to_remote = |pos: egui::Pos2| -> Option<(u16, u16)> {
            if !rect.contains(pos) || bar.is_some_and(|bar| bar.contains(pos)) {
                return None;
            }
            remote_point(pos, image, region)
        };

        let has_wayland = self.key_grab.is_some();
        let (events, modifiers) = ui.input(|i| (i.events.clone(), i.modifiers));
        for event in events {
            match event {
                Event::PointerMoved(pos) => {
                    if let Some(at) = to_remote(pos).filter(|at| self.last_pointer != Some(*at)) {
                        self.last_pointer = Some(at);
                        self.send(Command::PointerMove { x: at.0, y: at.1 });
                    }
                }
                Event::PointerButton {
                    pos,
                    button,
                    pressed,
                    ..
                } => {
                    // A release outside the picture still goes, so no button sticks.
                    let at = to_remote(pos).or(if pressed { None } else { self.last_pointer });
                    if let Some((x, y)) = at {
                        self.send(Command::MouseButton {
                            x,
                            y,
                            button: remote_button(button),
                            pressed,
                        });
                    }
                }
                Event::MouseWheel { unit, delta, .. } if response.hovered() => {
                    let step = match unit {
                        egui::MouseWheelUnit::Point => 6.0,
                        egui::MouseWheelUnit::Line => 120.0,
                        egui::MouseWheelUnit::Page => 360.0,
                    };
                    let vertical = to_i16(delta.y * step);
                    let horizontal = to_i16(-delta.x * step);
                    if vertical != 0 || horizontal != 0 {
                        self.send(Command::Wheel {
                            vertical,
                            horizontal,
                        });
                    }
                }
                // Ctrl+Alt+Enter is Remoport's: full screen on and off.
                Event::Key {
                    key: egui::Key::Enter,
                    pressed,
                    modifiers,
                    ..
                } if focused && modifiers.ctrl && modifiers.alt => {
                    if !has_wayland && pressed {
                        if !fullscreen {
                            self.fullscreen_hint_until = Some(Instant::now() + FULLSCREEN_HINT);
                        }
                        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fullscreen));
                        logging::event(if fullscreen {
                            "Left full screen"
                        } else {
                            "Full screen"
                        });
                    }
                }
                Event::Key {
                    key,
                    physical_key,
                    pressed,
                    repeat,
                    modifiers,
                } if focused => {
                    if !has_wayland {
                        // Modifiers first: a key and its Ctrl can land in one frame.
                        self.sync_modifiers(modifiers);
                        // Reserved combinations only: what is typed is never logged.
                        if pressed && !repeat && modifiers.alt {
                            match physical_key.unwrap_or(key) {
                                egui::Key::Tab => logging::event("Sent Alt+Tab"),
                                egui::Key::F4 => logging::event("Sent Alt+F4"),
                                _ => {}
                            }
                        }
                        if let Some(code) = keymap::scancode(physical_key.unwrap_or(key)) {
                            let strokes = strokes_for(
                                &mut self.fallback_keys_down,
                                code,
                                pressed,
                                modifiers.ctrl,
                            );
                            for (key, down) in strokes {
                                self.send_key(key, down);
                            }
                        }
                    }
                }
                // egui turns these shortcuts into clipboard commands and
                // swallows the key press; the release still arrives as a key.
                Event::Copy if focused => {
                    if !has_wayland {
                        self.fallback_keys_down.insert(keymap::C);
                        self.send_key(keymap::C, true);
                    }
                }
                Event::Cut if focused => {
                    if !has_wayland {
                        self.fallback_keys_down.insert(keymap::X);
                        self.send_key(keymap::X, true);
                    }
                }
                Event::Paste(text) if focused => {
                    if has_wayland {
                        let actions = self.input_state.handle_egui_paste(text, Instant::now());
                        self.execute_input_actions(actions, &ctx, fullscreen);
                    } else {
                        self.fallback_keys_down.insert(keymap::V);
                        if self.bookmark.share_clipboard {
                            logging::event(format_args!(
                                "Pasted into the PC: {} characters",
                                text.chars().count()
                            ));
                            self.send(Command::Paste(ClipboardText::new(text)));
                        } else {
                            self.send_key(keymap::V, true);
                        }
                    }
                }
                _ => {}
            }
        }
        focused.then(|| (modifiers, ctx.input(|i| i.focused)))
    }

    /// The part of the remote desktop window `index` shows.
    fn region(&self, index: usize) -> Option<RemoteRect> {
        let screen = self.screen.screen.as_ref()?;
        self.display.region(index, screen.width, screen.height)
    }

    /// Presses or releases Shift, Ctrl and Alt on the remote side to match.
    fn sync_modifiers(&mut self, now: Modifiers) {
        for (held, was, code) in [
            (now.shift, self.modifiers.shift, keymap::SHIFT),
            (now.ctrl, self.modifiers.ctrl, keymap::CTRL),
            (now.alt, self.modifiers.alt, keymap::ALT),
        ] {
            if held != was {
                self.send_key(code, held);
            }
        }
        self.modifiers = now;
    }

    /// Asks the server to match the window once its size has settled.
    fn follow_canvas(&mut self) {
        let target = self.wanted_size(self.display.canvas_px);
        let scale = self.wanted_scale();
        let screen_size = self.screen.screen.as_ref().map(|s| (s.width, s.height));
        if let Some(cmd) = self
            .display
            .follow_canvas(screen_size, target, scale, Instant::now())
        {
            self.send(cmd);
        }
    }

    /// Asks the PC for the screens' sizes once the windows have settled.
    fn follow_layout(&mut self) {
        let target: Vec<(u16, u16)> = self
            .display
            .screen_px
            .iter()
            .map(|&px| self.wanted_size(px))
            .collect();
        let scale = self.wanted_scale();
        let has_screen = self.screen.screen.is_some();
        if let Some(cmd) = self
            .display
            .follow_layout(has_screen, target, scale, Instant::now())
        {
            self.send(cmd);
        }
    }

    // ── Reconnecting ────────────────────────────────────────────────────

    fn reconnecting(&mut self, ui: &mut Ui, attempt: u32) {
        self.measure_canvas(ui);
        ui.vertical_centered(|ui| {
            ui.add_space(110.0);
            ui.label(
                RichText::new(tf("Reconnecting to {}…", &[&self.bookmark.pc_name]))
                    .font(crate::fonts::medium(18.0)),
            );
            ui.add_space(6.0);
            ui.label(
                RichText::new(tf(
                    "The connection dropped. Attempt {} of {}: your session on the PC normally \
                     stays open and comes back as you left it.",
                    &[&attempt, &MAX_RECONNECTS],
                ))
                .weak(),
            );
            ui.add_space(18.0);
            if ui
                .add_sized([180.0, 34.0], egui::Button::new(t("Stop reconnecting")))
                .clicked()
            {
                logging::event("Stopped reconnecting");
                self.reset_session();
                self.forget_session();
                self.controller.phase = Phase::Ended(t("You stopped reconnecting.").to_string());
            }
        });
    }

    // ── Messages ────────────────────────────────────────────────────────

    fn message(&self, ui: &mut Ui, title: &str, detail: &str) {
        ui.vertical_centered(|ui| {
            ui.add_space(110.0);
            ui.label(RichText::new(title).font(crate::fonts::medium(18.0)));
            ui.add_space(6.0);
            ui.label(RichText::new(detail).weak());
        });
    }

    fn ended(&mut self, ui: &mut Ui, theme: &Theme, reason: &str) {
        ui.vertical_centered(|ui| {
            ui.add_space(90.0);
            ui.set_max_width(620.0);
            ui.label(
                RichText::new(t("The session ended"))
                    .font(crate::fonts::medium(20.0))
                    .color(theme.text_primary),
            );
            ui.add_space(8.0);
            ui.add(egui::Label::new(RichText::new(reason).color(theme.text_secondary)).wrap());
            ui.add_space(18.0);
            if ui
                .add_sized([180.0, 34.0], theme.primary_button(t("Try again")))
                .clicked()
            {
                logging::event("Try again");
                self.signin.sign_in_error = None;
                self.controller.phase = Phase::SignIn;
            }
        });
    }
}

impl Drop for ActiveSession {
    fn drop(&mut self) {
        // Any password still held is wiped by its Zeroizing wrapper; the
        // engine's thread is stopped here rather than left running.
        self.reset_session();
    }
}

/// A desktop size the server accepts: even width, each side within limits.
/// This computer's screen scale as a percentage, whatever the app's zoom.
pub fn screen_scale(ctx: &egui::Context) -> u32 {
    scale_percent(ctx.native_pixels_per_point().unwrap_or(1.0))
}

fn scale_percent(pixels_per_point: f32) -> u32 {
    (pixels_per_point * 100.0).round() as u32
}

/// Scroll bars for a picture that does not fit the window. Returns the space
/// left for the picture and how far it is scrolled. The wheel stays the PC's.
fn scroll_bars(ui: &mut Ui, index: usize, size: Vec2) -> (Rect, Vec2) {
    ui.ctx().input_mut(|i| i.smooth_scroll_delta = Vec2::ZERO);
    // Beside the picture rather than over it, where a click belongs to the PC.
    ui.spacing_mut().scroll = egui::style::ScrollStyle::solid();
    let output = egui::ScrollArea::both()
        .id_source(("session-scroll", index))
        .auto_shrink(false)
        .drag_to_scroll(false)
        .show(ui, |ui| {
            ui.allocate_space(size);
        });
    (output.inner_rect, output.state.offset)
}

/// What to send for one key event. A release with no press behind it goes
/// nowhere, since the PC has no press to match, except Ctrl+V: with nothing to
/// paste on this computer egui keeps the press for itself, and the PC still
/// gets the keystroke, for what it has on its own clipboard.
fn strokes_for(
    down: &mut HashSet<keymap::Scancode>,
    key: keymap::Scancode,
    pressed: bool,
    ctrl: bool,
) -> Vec<(keymap::Scancode, bool)> {
    if pressed {
        down.insert(key);
        vec![(key, true)]
    } else if down.remove(&key) {
        vec![(key, false)]
    } else if key == keymap::V && ctrl {
        vec![(key, true), (key, false)]
    } else {
        Vec::new()
    }
}

fn remote_button(button: PointerButton) -> MouseButton {
    match button {
        PointerButton::Primary => MouseButton::Left,
        PointerButton::Secondary => MouseButton::Right,
        PointerButton::Middle => MouseButton::Middle,
        PointerButton::Extra1 => MouseButton::X1,
        PointerButton::Extra2 => MouseButton::X2,
    }
}

/// Rounds and clamps to ±240, two wheel notches, so the cast cannot wrap.
fn to_i16(value: f32) -> i16 {
    value.round().clamp(-240.0, 240.0) as i16
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeroize::Zeroizing;

    #[test]
    fn only_a_swallowed_ctrl_v_is_sent_without_its_press() {
        let mut down = HashSet::new();
        assert_eq!(
            strokes_for(&mut down, keymap::C, true, false),
            vec![(keymap::C, true)]
        );
        assert_eq!(
            strokes_for(&mut down, keymap::C, true, false),
            vec![(keymap::C, true)]
        );
        assert_eq!(
            strokes_for(&mut down, keymap::C, false, false),
            vec![(keymap::C, false)]
        );
        // Ctrl+V with an empty clipboard here: only the release arrives.
        assert_eq!(
            strokes_for(&mut down, keymap::V, false, true),
            vec![(keymap::V, true), (keymap::V, false)]
        );
        // The Enter of Ctrl+Alt+Enter, released after Ctrl and Alt: nothing.
        assert!(strokes_for(&mut down, keymap::ESCAPE, false, false).is_empty());
        assert!(strokes_for(&mut down, keymap::V, false, false).is_empty());
        assert!(down.is_empty());
    }

    #[test]
    fn mouse_buttons_map_to_the_engine() {
        assert_eq!(remote_button(PointerButton::Primary), MouseButton::Left);
        assert_eq!(remote_button(PointerButton::Secondary), MouseButton::Right);
        assert_eq!(remote_button(PointerButton::Middle), MouseButton::Middle);
    }

    #[test]
    fn only_network_failures_bring_the_session_back() {
        let lost = Error::ConnectionLost("reset".into());
        let unreachable = Error::Unreachable("no route".into());
        assert!(worth_retrying(&lost, false));
        // A PC unreachable at first is a plain failure, not a drop.
        assert!(!worth_retrying(&unreachable, false));
        assert!(worth_retrying(&unreachable, true));
        assert!(worth_retrying(&Error::Timeout("reaching the PC"), true));
        for refused in [
            Error::Authentication("bad password".into()),
            Error::IdentityRejected,
            Error::NlaUnavailable,
        ] {
            assert!(!worth_retrying(&refused, true), "{refused}");
        }
    }

    #[test]
    fn the_system_scale_reads_as_a_percentage() {
        assert_eq!(scale_percent(1.0), 100);
        assert_eq!(scale_percent(1.1), 110);
        assert_eq!(scale_percent(1.25), 125);
        assert_eq!(scale_percent(2.0), 200);
    }

    #[test]
    fn a_scale_set_for_the_pc_wins_over_this_screens() {
        let mut pc = DeviceBookmark::new("pc.example".into(), String::new(), String::new());
        pc.scale_percent = Some(200);
        let mut view = ActiveSession::new(pc, false);
        view.display.screen_scale = 125;
        assert_eq!(view.wanted_scale(), 200);
        view.bookmark.scale_percent = None;
        assert_eq!(view.wanted_scale(), 125);
    }

    #[test]
    fn automatic_scale_stays_at_100_when_this_computer_enlarges_the_picture() {
        let mut pc = DeviceBookmark::new("pc.example".into(), String::new(), String::new());
        pc.hidpi_full_resolution = false;
        let mut view = ActiveSession::new(pc, false);
        view.display.screen_scale = 125;
        assert_eq!(view.wanted_scale(), 100);
        view.bookmark.scale_percent = Some(150);
        assert_eq!(view.wanted_scale(), 150);
    }

    #[test]
    fn a_fixed_resolution_ignores_the_window() {
        let mut pc = DeviceBookmark::new("pc.example".into(), String::new(), String::new());
        pc.resolution = crate::model::ResolutionOption::Fhd1080p;
        let mut view = ActiveSession::new(pc, false);
        view.display.display_control = true;
        view.screen.resize(1920, 1080);
        view.display.canvas_px = (800, 600);
        view.follow_canvas();
        assert!(
            view.display.pending_size.is_none(),
            "the window does not matter"
        );

        // A desktop of another size is asked to take the one set.
        view.screen.resize(1280, 720);
        view.follow_canvas();
        assert_eq!(
            view.display.pending_size.map(|(size, _)| size),
            Some((1920, 1080))
        );
    }

    #[test]
    fn a_pc_that_answers_another_size_is_not_asked_again() {
        let mut pc = DeviceBookmark::new("pc.example".into(), String::new(), String::new());
        pc.resolution = crate::model::ResolutionOption::Fhd1080p;
        let mut view = ActiveSession::new(pc, false);
        view.display.display_control = true;
        view.display.requested_size = Some((1920, 1080));

        view.handle(EngineEvent::Resized {
            width: 1280,
            height: 720,
        });
        view.follow_canvas();
        assert!(view.display.pending_size.is_none(), "already asked");

        view.handle(EngineEvent::Resized {
            width: 1920,
            height: 1080,
        });
        assert_eq!(view.display.requested_size, None);
    }

    #[test]
    fn a_new_scale_alone_is_asked_of_the_pc() {
        let pc = DeviceBookmark::new("pc.example".into(), String::new(), String::new());
        let mut view = ActiveSession::new(pc, false);
        view.display.display_control = true;
        view.screen.resize(1280, 720);
        view.display.canvas_px = (1280, 720);
        view.follow_canvas();
        assert!(view.display.pending_size.is_none(), "nothing changed");

        view.display.screen_scale = 125;
        view.follow_canvas();
        assert_eq!(view.display.sent_scale, 100, "not before it settles");
        view.display.pending_size = Some(((1280, 720), Instant::now() - RESIZE_SETTLE));
        view.follow_canvas();
        assert_eq!(view.display.sent_scale, 125);
        view.follow_canvas();
        assert!(view.display.pending_size.is_none(), "asked only once");
    }

    #[test]
    fn a_new_scale_alone_reaches_every_screen() {
        let mut pc = DeviceBookmark::new("pc.example".into(), String::new(), String::new());
        pc.screens = 2;
        let mut view = ActiveSession::new(pc, false);
        view.display.display_control = true;
        view.screen.resize(1280, 720);
        let sizes = vec![(640, 720), (640, 720)];
        view.display.screen_px = sizes.clone();
        view.display.requested_layout = Some(sizes.clone());
        view.follow_layout();
        assert!(view.display.pending_layout.is_none(), "nothing changed");

        view.display.screen_scale = 150;
        view.follow_layout();
        assert_eq!(view.display.sent_scale, 100, "not before it settles");
        view.display.pending_layout = Some((sizes, Instant::now() - RESIZE_SETTLE));
        view.follow_layout();
        assert_eq!(view.display.sent_scale, 150);
    }

    #[test]
    fn a_dropped_session_comes_back_until_it_cannot() {
        let pc = DeviceBookmark::new("pc.example".into(), String::new(), String::new());
        let mut view = ActiveSession::new(pc, false);
        view.controller.phase = Phase::Connected;
        view.signin.session_password = Some(Zeroizing::new("secret".into()));

        view.session_ended(Some(Error::ConnectionLost("reset".into())));
        assert!(matches!(
            view.controller.phase,
            Phase::Reconnecting {
                attempt: 1,
                at: Some(_)
            }
        ));
        for expected in 2..=MAX_RECONNECTS {
            view.session_ended(Some(Error::Unreachable("no route".into())));
            assert!(
                matches!(view.controller.phase, Phase::Reconnecting { attempt, .. } if attempt == expected)
            );
            assert!(view.signin.session_password.is_some());
        }
        view.session_ended(Some(Error::Timeout("reaching the PC")));
        assert!(
            matches!(view.controller.phase, Phase::Ended(ref why) if why.contains("could not be brought back"))
        );
        assert!(
            view.signin.session_password.is_none(),
            "the password outlived the session"
        );
    }

    #[test]
    fn a_refused_password_while_reconnecting_asks_again_and_wipes_it() {
        let pc = DeviceBookmark::new("pc.example".into(), String::new(), String::new());
        let mut view = ActiveSession::new(pc, false);
        view.controller.phase = Phase::Reconnecting {
            attempt: 3,
            at: None,
        };
        view.signin.session_password = Some(Zeroizing::new("old".into()));
        view.session_ended(Some(Error::Authentication("changed".into())));
        assert!(matches!(view.controller.phase, Phase::SignIn));
        assert!(view.signin.session_password.is_none());

        // A session the PC ended itself is not brought back.
        view.controller.phase = Phase::Connected;
        view.signin.session_password = Some(Zeroizing::new("secret".into()));
        view.session_ended(None);
        assert!(matches!(view.controller.phase, Phase::Ended(_)));
        assert!(view.signin.session_password.is_none());
    }
}
