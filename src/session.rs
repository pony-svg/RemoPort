//! A remote desktop session, drawn inside Remoport by its own RDP engine
//! (`remoport-rdp`): sign in, confirm the PC's identity when it cannot be
//! verified automatically, show the remote screen, pass keyboard and mouse,
//! follow the window's size, share the clipboard and play sound when this PC
//! has them, and bring a dropped connection back to the same session.

use crate::credentials::{self, Outcome, Pending};
use crate::keymap;
use crate::logging;
use crate::model::{self, AudioPlaybackMode, DeviceBookmark};
use crate::theme::Theme;
use egui::{
    pos2, vec2, Align2, Color32, ColorImage, Context, Event, FontId, Modifiers, PointerButton,
    Rect, RichText, Rounding, Sense, TextureHandle, TextureOptions, Ui,
};
use remoport_rdp::{
    CertificateInfo, ClipboardText, Command, ConnectConfig, Error, Event as EngineEvent,
    IdentityProblem, MouseButton, Rect as RemoteRect, ResumeCookie, Session, SharedFolder,
    SoundInput, SoundOutput, TrustReply, TrustStore,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use zeroize::{Zeroize, Zeroizing};

/// How long the window must keep a new size before the remote desktop follows.
const RESIZE_SETTLE: Duration = Duration::from_millis(400);

/// Remote desktop sizes the display-control channel accepts (MS-RDPEDISP 2.2.2.2.1).
const MIN_SIDE: u16 = 200;
const MAX_SIDE: u16 = 8192;

/// Limit for each step of the connection; the user's own decisions aside.
const STEP_TIMEOUT: Duration = Duration::from_secs(30);

/// Attempts to bring a dropped connection back before giving up.
const MAX_RECONNECTS: u32 = 20;

/// Limit for each step of a reconnection: a network that is still down
/// should cost the next attempt, not half a minute.
const RECONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Engine events handled per repaint at most, so a burst cannot stall the UI.
const EVENTS_PER_FRAME: usize = 2000;

const DANGER: Color32 = Color32::from_rgb(0xc4, 0x2b, 0x1c);

enum Phase {
    /// Asking for the user name and password.
    SignIn,
    Connecting,
    /// The PC's identity needs the user's decision; nothing has been sent.
    VerifyIdentity {
        info: CertificateInfo,
        problem: IdentityProblem,
        reply: Option<TrustReply>,
        /// For a changed certificate: the user confirmed it was replaced.
        replacement_confirmed: bool,
    },
    Connected,
    /// The connection dropped and is being brought back: `at` is when this
    /// attempt starts, `None` once it is under way.
    Reconnecting {
        attempt: u32,
        at: Option<Instant>,
    },
    /// The session is over; holds what to tell the user.
    Ended(String),
}

/// The remote desktop as last drawn, in RGBA, and its texture.
struct Screen {
    width: usize,
    height: usize,
    rgba: Vec<u8>,
    texture: Option<TextureHandle>,
}

impl Screen {
    fn new(width: u16, height: u16) -> Self {
        let (width, height) = (usize::from(width), usize::from(height));
        let mut rgba = vec![0; width * height * 4];
        for pixel in rgba.as_chunks_mut::<4>().0 {
            pixel[3] = 255;
        }
        Self {
            width,
            height,
            rgba,
            texture: None,
        }
    }

    /// Copies a BGRA region from the engine into the screen and its texture.
    /// A region that does not fit the screen is dropped.
    fn apply(&mut self, rect: RemoteRect, bgra: &[u8]) {
        let (x, y) = (usize::from(rect.x), usize::from(rect.y));
        let (w, h) = (usize::from(rect.width), usize::from(rect.height));
        if w == 0 || h == 0 || x + w > self.width || y + h > self.height || bgra.len() < w * h * 4 {
            return;
        }
        let region: Vec<u8> = bgra[..w * h * 4]
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|px| [px[2], px[1], px[0], 255])
            .collect();
        for row in 0..h {
            let dst = ((y + row) * self.width + x) * 4;
            self.rgba[dst..dst + w * 4].copy_from_slice(&region[row * w * 4..(row + 1) * w * 4]);
        }
        if let Some(texture) = &mut self.texture {
            texture.set_partial(
                [x, y],
                ColorImage::from_rgba_unmultiplied([w, h], &region),
                TextureOptions::LINEAR,
            );
        }
    }

    fn texture(&mut self, ctx: &Context) -> &TextureHandle {
        let (width, height, rgba) = (self.width, self.height, &self.rgba);
        self.texture.get_or_insert_with(|| {
            ctx.load_texture(
                "remote-screen",
                ColorImage::from_rgba_unmultiplied([width, height], rgba),
                TextureOptions::LINEAR,
            )
        })
    }
}

pub struct ActiveSession {
    pub bookmark: DeviceBookmark,
    pub open_app: Option<String>,
    pub notification_msg: Option<String>,
    /// Whether the PC is saved, so its password may be remembered.
    saved: bool,
    phase: Phase,
    username: String,
    password: Zeroizing<String>,
    /// "Remember my password" is ticked.
    remember: bool,
    /// The password remembered in the keyring for this PC.
    saved_password: Option<Zeroizing<String>>,
    /// Whether the last attempt used the remembered password.
    used_saved_password: bool,
    keyring: Option<Pending>,
    /// A typed password to remember once the PC has accepted it.
    to_remember: Option<Zeroizing<String>>,
    sign_in_error: Option<String>,
    session: Option<Session>,
    screen: Option<Screen>,
    graphics: Option<String>,
    /// The drawing area's size in physical pixels, for the next connection.
    canvas_px: (u16, u16),
    /// A size the window settled on, and since when, before it is requested.
    pending_size: Option<((u16, u16), Instant)>,
    /// The size last requested, until the server answers with it.
    requested_size: Option<(u16, u16)>,
    /// Whether the server can change resolution without reconnecting.
    display_control: bool,
    remote_cursor_hidden: bool,
    /// Modifier keys held on the remote side.
    modifiers: Modifiers,
    last_pointer: Option<(u16, u16)>,
    /// Text the PC copied, until it is taken (or not) on the next frame.
    remote_copy: Option<ClipboardText>,
    /// The password this session signed in with, kept while the session
    /// lasts so a dropped connection can sign in again, as Windows App does.
    /// Wiped when the session ends or this view closes.
    session_password: Option<Zeroizing<String>>,
    /// The PC's latest cookie for resuming this session.
    resume: Option<ResumeCookie>,
    /// The PC has the microphone open.
    microphone_live: bool,
    /// The user muted the microphone; the PC then gets silence.
    microphone_muted: Arc<AtomicBool>,
}

impl ActiveSession {
    /// A session for `bookmark`; `saved` tells whether it is one of the
    /// user's PCs (whose password may be remembered) or a one-off.
    pub fn new(bookmark: DeviceBookmark, saved: bool) -> Self {
        let keyring = saved.then(|| credentials::lookup(bookmark.id));
        Self {
            username: bookmark.username.clone(),
            bookmark,
            open_app: None,
            notification_msg: None,
            saved,
            phase: Phase::SignIn,
            password: Zeroizing::new(String::new()),
            remember: false,
            saved_password: None,
            used_saved_password: false,
            keyring,
            to_remember: None,
            sign_in_error: None,
            session: None,
            screen: None,
            graphics: None,
            canvas_px: (1280, 720),
            pending_size: None,
            requested_size: None,
            display_control: false,
            remote_cursor_hidden: false,
            modifiers: Modifiers::NONE,
            last_pointer: None,
            remote_copy: None,
            session_password: None,
            resume: None,
            microphone_live: false,
            microphone_muted: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn render(&mut self, ui: &mut Ui, theme: &Theme, on_disconnect: &mut bool) {
        self.poll_keyring();
        self.poll_engine();

        egui::Frame::none()
            .inner_margin(egui::Margin::symmetric(12.0, 6.0))
            .show(ui, |ui| self.top_bar(ui, on_disconnect));
        ui.separator();
        if let Some(ref msg) = self.notification_msg {
            ui.colored_label(theme.accent, format!("ℹ {msg}"));
        }

        if let Some(app) = self.open_app.clone() {
            self.message(
                ui,
                &format!("\"{app}\" would open here"),
                "Apps published by workspaces are not available yet.",
            );
            return;
        }

        if let Phase::Reconnecting {
            attempt,
            at: Some(at),
        } = self.phase
        {
            if Instant::now() >= at {
                self.phase = Phase::Reconnecting { attempt, at: None };
                self.start(RECONNECT_TIMEOUT);
            }
        }

        match self.phase {
            Phase::SignIn => self.sign_in_form(ui, theme),
            Phase::Connecting => {
                self.measure_canvas(ui);
                self.message(
                    ui,
                    &format!("Connecting to {}…", self.bookmark.pc_name),
                    "Checking the PC's identity before signing in.",
                );
            }
            Phase::VerifyIdentity { .. } => self.identity_question(ui, theme),
            Phase::Connected => self.remote_screen(ui),
            Phase::Reconnecting { attempt, .. } => self.reconnecting(ui, attempt),
            Phase::Ended(ref reason) => {
                let reason = reason.clone();
                self.ended(ui, theme, &reason);
            }
        }

        if matches!(
            self.phase,
            Phase::Connecting | Phase::Connected | Phase::Reconnecting { .. }
        ) || self.keyring.is_some()
        {
            ui.ctx().request_repaint_after(Duration::from_millis(16));
        }
    }

    // ── Top bar ─────────────────────────────────────────────────────────

    fn top_bar(&mut self, ui: &mut Ui, on_disconnect: &mut bool) {
        ui.horizontal(|ui| {
            ui.label(format!("🖥 {}", self.bookmark.display_title()));
            ui.separator();
            ui.label(format!("Host: {}", self.bookmark.pc_name));
            ui.separator();
            ui.label(self.status());

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let connected = matches!(self.phase, Phase::Connected);
                if ui
                    .button(if connected {
                        "❌ Disconnect"
                    } else {
                        "❌ Close"
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
                        RichText::new("● Microphone muted")
                    } else {
                        RichText::new("● Microphone on").color(DANGER)
                    };
                    if ui
                        .button(label)
                        .on_hover_text(if muted {
                            "The PC gets silence. Click to unmute."
                        } else {
                            "The PC is using your microphone. Click to mute."
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
                if connected && ui.button("⌨ Ctrl+Alt+Del").clicked() {
                    logging::event("Sent Ctrl+Alt+Del");
                    self.send(Command::CtrlAltDel);
                }
            });
        });
    }

    fn status(&self) -> String {
        match self.phase {
            Phase::SignIn => "Sign in".to_string(),
            Phase::Connecting => "Connecting…".to_string(),
            Phase::VerifyIdentity { .. } => "Waiting for your decision".to_string(),
            Phase::Connected => {
                let size = self
                    .screen
                    .as_ref()
                    .map(|s| format!(" · {}×{}", s.width, s.height))
                    .unwrap_or_default();
                let graphics = self
                    .graphics
                    .as_ref()
                    .map(|g| format!(" · {g}"))
                    .unwrap_or_default();
                format!("Connected{size}{graphics}")
            }
            Phase::Reconnecting { attempt, .. } => {
                format!("Reconnecting… ({attempt} of {MAX_RECONNECTS})")
            }
            Phase::Ended(_) => "Disconnected".to_string(),
        }
    }

    // ── Keyring ─────────────────────────────────────────────────────────

    fn poll_keyring(&mut self) {
        let Some(outcome) = self.keyring.as_ref().and_then(Pending::poll) else {
            return;
        };
        self.keyring = None;
        match outcome {
            Outcome::Found(password) => {
                logging::event(format_args!(
                    "A password for {} is saved in the keyring",
                    self.bookmark.pc_name
                ));
                self.saved_password = Some(password);
            }
            Outcome::Missing => {}
            Outcome::Saved => {
                logging::event(format_args!(
                    "Password for {} remembered in the keyring",
                    self.bookmark.pc_name
                ));
                self.notification_msg =
                    Some("Your password is remembered in the system keyring.".to_string());
            }
            Outcome::Forgotten => {
                logging::event(format_args!(
                    "Saved password for {} forgotten",
                    self.bookmark.pc_name
                ));
            }
            Outcome::Failed(error) => {
                logging::event(format_args!("Keyring unavailable: {error}"));
                self.notification_msg = Some(
                    "The system keyring is not available, so no password is remembered."
                        .to_string(),
                );
            }
        }
    }

    // ── Sign in and connect ─────────────────────────────────────────────

    fn sign_in_form(&mut self, ui: &mut Ui, theme: &Theme) {
        self.measure_canvas(ui);
        ui.vertical_centered(|ui| {
            ui.add_space(70.0);
            ui.label(
                RichText::new(format!("Sign in to {}", self.bookmark.display_title()))
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
                        ui.label("User name");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.username)
                                .hint_text("user, DOMAIN\\user or user@domain")
                                .desired_width(280.0),
                        );
                        ui.end_row();
                        ui.label("Password");
                        if self.saved_password.is_some() {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new("🔑 Saved in your keyring")
                                        .color(theme.text_secondary),
                                );
                                forget = ui.small_button("Forget").clicked();
                            });
                        } else {
                            let field = ui.add(
                                egui::TextEdit::singleline(&mut *self.password)
                                    .password(true)
                                    .desired_width(280.0),
                            );
                            submit |=
                                field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        }
                        ui.end_row();
                        if self.saved && self.saved_password.is_none() {
                            ui.label("");
                            ui.checkbox(&mut self.remember, "Remember in the system keyring");
                            ui.end_row();
                        }
                    });
            });
            if forget {
                self.saved_password = None;
                self.keyring = Some(credentials::forget(self.bookmark.id));
            }

            if let Some(error) = &self.sign_in_error {
                ui.add_space(8.0);
                ui.colored_label(DANGER, error);
            }
            ui.add_space(16.0);
            submit |= ui
                .add_sized([200.0, 34.0], theme.primary_button("Connect"))
                .clicked();
            if submit {
                self.connect();
            }
            ui.add_space(8.0);
            ui.label(
                RichText::new("Remoport confirms the PC's identity before your password is used.")
                    .size(12.0)
                    .color(theme.text_secondary),
            );
        });
    }

    fn connect(&mut self) {
        self.used_saved_password = self.password.is_empty() && self.saved_password.is_some();
        let password = if self.used_saved_password {
            self.saved_password.clone().unwrap_or_default()
        } else {
            self.password.clone()
        };
        self.to_remember = (self.saved && self.remember && !self.password.is_empty())
            .then(|| self.password.clone());
        // Kept, wiped on drop, for reconnecting; the form's copy goes now.
        self.session_password = Some(password);
        self.password.zeroize();
        self.resume = None;
        self.sign_in_error = None;
        if self.start(STEP_TIMEOUT) {
            self.phase = Phase::Connecting;
        }
    }

    /// Starts the engine with the session's password, resuming the session
    /// when the PC gave a cookie for it. Returns whether it started.
    fn start(&mut self, timeout: Duration) -> bool {
        let Some(trust_store) = model::trust_store_path().map(TrustStore::new) else {
            self.forget_session();
            self.phase = Phase::Ended(
                "Remoport has no folder to keep trusted certificates in (HOME is not set)."
                    .to_string(),
            );
            return false;
        };
        let target = self.bookmark.pc_name.trim().to_string();
        let (host, port) = split_host_port(&target);
        let username = self.username.trim().to_string();
        let (domain, user) = split_domain(&username);
        let (width, height) = desktop_size(self.canvas_px);
        let password = self.session_password.clone().unwrap_or_default();

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
            if self.resume.is_some() {
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
            if self.used_saved_password {
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
        self.session = Some(Session::start(ConnectConfig {
            host: host.to_string(),
            port,
            username: user.to_string(),
            domain: domain.map(str::to_string),
            password,
            width,
            height,
            allow_without_nla: self.bookmark.allow_without_nla,
            trust_store,
            timeout,
            client_name: client_name(),
            clipboard: self.bookmark.share_clipboard,
            sound,
            microphone,
            folders,
            gateway: Some(self.bookmark.gateway.trim().to_string()).filter(|g| !g.is_empty()),
            kerberos: self.bookmark.kerberos.then(|| remoport_rdp::Kerberos {
                kdc: Some(self.bookmark.kdc.trim().to_string()).filter(|k| !k.is_empty()),
            }),
            resume: self.resume.clone(),
        }));
        true
    }

    /// Wipes what the session kept for reconnecting.
    fn forget_session(&mut self) {
        self.session_password = None;
        self.resume = None;
    }

    // ── Engine events ───────────────────────────────────────────────────

    fn poll_engine(&mut self) {
        let Some(session) = &self.session else {
            return;
        };
        let mut events = Vec::new();
        while events.len() < EVENTS_PER_FRAME {
            match session.try_event() {
                Some(event) => events.push(event),
                None => break,
            }
        }
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
                self.phase = Phase::VerifyIdentity {
                    info,
                    problem,
                    reply: Some(reply),
                    replacement_confirmed: false,
                };
            }
            EngineEvent::Connected { width, height } => {
                logging::event(format_args!(
                    "{} {} at {width}×{height}",
                    if matches!(self.phase, Phase::Reconnecting { .. }) {
                        "Reconnected to"
                    } else {
                        "Connected to"
                    },
                    self.bookmark.pc_name
                ));
                self.screen = Some(Screen::new(width, height));
                self.phase = Phase::Connected;
                if let Some(password) = self.to_remember.take() {
                    self.keyring = Some(credentials::save(self.bookmark.id, password));
                }
            }
            EngineEvent::Resized { width, height } => {
                logging::event(format_args!("Remote desktop is now {width}×{height}"));
                self.screen = Some(Screen::new(width, height));
                self.requested_size = None;
            }
            EngineEvent::Frame { rect, bgra } => {
                if let Some(screen) = &mut self.screen {
                    screen.apply(rect, &bgra);
                }
            }
            EngineEvent::Graphics(mode) => {
                logging::event(format_args!("Graphics: {mode}"));
                self.graphics = Some(mode);
            }
            EngineEvent::DisplayControlReady => {
                logging::event("The remote desktop can follow the window's size");
                self.display_control = true;
            }
            EngineEvent::PointerHidden(hidden) => self.remote_cursor_hidden = hidden,
            EngineEvent::Clipboard(text) => self.remote_copy = Some(text),
            EngineEvent::Microphone(live) => self.microphone_live = live,
            EngineEvent::ResumeCookie(cookie) => {
                logging::event("The PC gave a cookie to resume this session after a drop");
                self.resume = Some(cookie);
            }
            EngineEvent::Ended(outcome) => self.session_ended(outcome),
        }
    }

    fn session_ended(&mut self, outcome: Option<Error>) {
        self.to_remember = None;
        // Attempts made so far, when the session was up or coming back.
        let attempts = match self.phase {
            Phase::Connected => Some(0),
            Phase::Reconnecting { attempt, .. } => Some(attempt),
            _ => None,
        };
        self.reset_session();
        if let (Some(done), Some(error)) = (attempts, &outcome) {
            if self.session_password.is_some() && worth_retrying(error, done > 0) {
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
                    self.phase = Phase::Reconnecting {
                        attempt,
                        at: Some(Instant::now() + wait),
                    };
                    return;
                }
                logging::event("Gave up reconnecting");
                self.forget_session();
                self.phase = Phase::Ended(format!(
                    "The connection to {} was lost and could not be brought back after \
                     {MAX_RECONNECTS} attempts ({error}).",
                    self.bookmark.pc_name
                ));
                return;
            }
        }
        self.forget_session();
        match outcome {
            None => {
                logging::event("Session ended");
                if !matches!(self.phase, Phase::Ended(_)) {
                    self.phase = Phase::Ended("The session ended.".to_string());
                }
            }
            Some(error) => {
                logging::event(format_args!("Session ended: {error}"));
                match error {
                    Error::Authentication(_) => {
                        let message = if self.used_saved_password {
                            "The PC refused the saved password. Type it again."
                        } else {
                            "The PC refused the sign-in. Check the user name and password."
                        };
                        if self.used_saved_password {
                            self.saved_password = None;
                        }
                        self.sign_in_error = Some(message.to_string());
                        self.phase = Phase::SignIn;
                    }
                    Error::NlaUnavailable => {
                        self.phase = Phase::Ended(format!(
                            "{} does not offer Network Level Authentication, so it would show \
                             its sign-in screen before checking who you are. Remoport does not \
                             connect that way unless you allow it for this PC (Edit → General), \
                             which is less secure.",
                            self.bookmark.pc_name
                        ));
                    }
                    Error::IdentityRejected => {
                        self.phase = Phase::Ended(
                            "You chose not to connect: the PC's identity was not confirmed. \
                             Nothing was sent to it."
                                .to_string(),
                        );
                    }
                    other => self.phase = Phase::Ended(capitalize(&other.to_string())),
                }
            }
        }
    }

    fn reset_session(&mut self) {
        if let Some(mut session) = self.session.take() {
            session.disconnect();
        }
        self.screen = None;
        self.display_control = false;
        self.pending_size = None;
        self.requested_size = None;
        self.modifiers = Modifiers::NONE;
        self.remote_copy = None;
        self.microphone_live = false;
    }

    fn send(&self, command: Command) {
        if let Some(session) = &self.session {
            session.send(command);
        }
    }

    fn send_key(&self, (scancode, extended): keymap::Scancode, pressed: bool) {
        self.send(Command::Key {
            scancode,
            extended,
            pressed,
        });
    }

    // ── Identity ────────────────────────────────────────────────────────

    fn identity_question(&mut self, ui: &mut Ui, theme: &Theme) {
        let Phase::VerifyIdentity {
            info,
            problem,
            replacement_confirmed,
            ..
        } = &mut self.phase
        else {
            return;
        };
        let changed = matches!(problem, IdentityProblem::Changed { .. });
        let mut decision = None;
        ui.vertical_centered(|ui| {
            ui.add_space(40.0);
            ui.set_max_width(620.0);
            let (title, color) = if changed {
                ("Warning: this PC's certificate changed", DANGER)
            } else {
                (
                    "Remoport cannot confirm this PC's identity",
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
                format!(
                    "{} presents a different certificate from the one you trusted. Someone may \
                     be intercepting the connection to capture your password. Continue only if \
                     your administrator confirms the certificate was replaced.",
                    info.host
                )
            } else {
                format!(
                    "Remoport could not verify {}: {}. If this is your PC, compare the \
                     fingerprint below with the one your administrator gives you before you \
                     trust it. Nothing has been sent to it yet.",
                    info.host, info.why_unverified
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
                            row(ui, "Certificate for", &info.subject, false);
                            row(ui, "Issued by", &info.issuer, false);
                            row(
                                ui,
                                "Valid",
                                &format!("{} to {}", info.valid_from, info.valid_until),
                                false,
                            );
                            if let IdentityProblem::Changed { previous_sha256 } = &problem {
                                row(ui, "You trusted (SHA-256)", previous_sha256, true);
                                row(ui, "Now presented", &info.sha256, true);
                            } else {
                                row(ui, "Fingerprint (SHA-256)", &info.sha256, true);
                            }
                        });
                });

            ui.add_space(16.0);
            if changed {
                ui.checkbox(
                    replacement_confirmed,
                    "My administrator confirmed this certificate was replaced",
                );
                ui.add_space(10.0);
            }
            ui.horizontal(|ui| {
                let width = 2.0 * 220.0 + ui.spacing().item_spacing.x;
                ui.add_space((ui.available_width() - width).max(0.0) / 2.0);
                if ui
                    .add_sized([220.0, 34.0], theme.primary_button("Don't connect"))
                    .clicked()
                {
                    decision = Some(false);
                }
                let trust = if changed {
                    egui::Button::new(
                        RichText::new("Trust the new certificate").color(Color32::WHITE),
                    )
                    .fill(DANGER)
                } else {
                    egui::Button::new("Trust and connect")
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
            if let Phase::VerifyIdentity { reply, .. } = &mut self.phase {
                if let Some(reply) = reply.take() {
                    reply.answer(trust);
                }
            }
            logging::event(format_args!(
                "{} the certificate of {host} ({sha})",
                if trust { "You trusted" } else { "You declined" }
            ));
            self.phase = Phase::Connecting;
        }
    }

    // ── Remote screen ───────────────────────────────────────────────────

    fn measure_canvas(&mut self, ui: &Ui) {
        let px = ui.available_size() * ui.ctx().pixels_per_point();
        self.canvas_px = (to_u16(px.x), to_u16(px.y));
    }

    fn remote_screen(&mut self, ui: &mut Ui) {
        self.measure_canvas(ui);
        self.follow_canvas();

        let ctx = ui.ctx().clone();
        let ppp = ctx.pixels_per_point();
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        ui.painter()
            .rect_filled(rect, Rounding::ZERO, Color32::BLACK);
        let Some(screen) = &mut self.screen else {
            return;
        };
        let (width, height) = (screen.width as f32, screen.height as f32);
        let size = vec2(width, height) / ppp;
        // Never enlarge: until the server follows a resize, the picture is
        // shown at its own size or smaller, centered.
        let scale = (rect.width() / size.x).min(rect.height() / size.y).min(1.0);
        let image = Rect::from_center_size(rect.center(), size * scale);
        let texture = screen.texture(&ctx).id();
        ui.painter().image(
            texture,
            image,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );

        if response.clicked() || response.drag_started() {
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
                "Click the remote screen to type into it",
                FontId::proportional(12.0),
                Color32::from_white_alpha(160),
            );
        }
        if response.hovered() && self.remote_cursor_hidden {
            ctx.set_cursor_icon(egui::CursorIcon::None);
        }
        if let Some(text) = self.remote_copy.take() {
            // Only while the user works in the session: a PC copying on its
            // own does not get to replace this computer's clipboard.
            if focused && ctx.input(|i| i.focused) {
                logging::event(format_args!(
                    "Copied from the PC: {} characters",
                    text.as_str().chars().count()
                ));
                ctx.output_mut(|o| o.copied_text = text.as_str().to_string());
            } else {
                logging::event("The PC's clipboard was not taken: the session was not in use");
            }
        }

        let to_remote = |pos: egui::Pos2| -> Option<(u16, u16)> {
            if !image.contains(pos) {
                return None;
            }
            let x = (pos.x - image.min.x) / image.width() * width;
            let y = (pos.y - image.min.y) / image.height() * height;
            Some((to_u16(x.min(width - 1.0)), to_u16(y.min(height - 1.0))))
        };

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
                Event::Key {
                    key,
                    physical_key,
                    pressed,
                    ..
                } if focused => {
                    if let Some(code) = keymap::scancode(physical_key.unwrap_or(key)) {
                        self.send_key(code, pressed);
                    }
                }
                // egui turns these shortcuts into clipboard commands and
                // swallows the key press; the release still arrives as a key.
                Event::Copy if focused => self.send_key(keymap::C, true),
                Event::Cut if focused => self.send_key(keymap::X, true),
                Event::Paste(text) if focused => {
                    if self.bookmark.share_clipboard {
                        self.send(Command::Paste(ClipboardText::new(text)));
                    } else {
                        self.send_key(keymap::V, true);
                    }
                }
                _ => {}
            }
        }
        self.sync_modifiers(if focused { modifiers } else { Modifiers::NONE });
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
        if !self.display_control {
            return;
        }
        let Some(screen) = &self.screen else {
            return;
        };
        let target = desktop_size(self.canvas_px);
        let close_enough = usize::from(target.0).abs_diff(screen.width) <= 8
            && usize::from(target.1).abs_diff(screen.height) <= 8;
        if close_enough || self.requested_size == Some(target) {
            self.pending_size = None;
            return;
        }
        match self.pending_size {
            Some((size, since)) if size == target => {
                if since.elapsed() >= RESIZE_SETTLE {
                    logging::event(format_args!("Asked the PC for {}×{}", target.0, target.1));
                    self.send(Command::Resize {
                        width: target.0,
                        height: target.1,
                    });
                    self.requested_size = Some(target);
                    self.pending_size = None;
                }
            }
            _ => self.pending_size = Some((target, Instant::now())),
        }
    }

    // ── Reconnecting ────────────────────────────────────────────────────

    fn reconnecting(&mut self, ui: &mut Ui, attempt: u32) {
        self.measure_canvas(ui);
        ui.vertical_centered(|ui| {
            ui.add_space(110.0);
            ui.label(
                RichText::new(format!("Reconnecting to {}…", self.bookmark.pc_name))
                    .font(crate::fonts::medium(18.0)),
            );
            ui.add_space(6.0);
            ui.label(
                RichText::new(format!(
                    "The connection dropped. Attempt {attempt} of {MAX_RECONNECTS}: your \
                     session on the PC normally stays open and comes back as you left it."
                ))
                .weak(),
            );
            ui.add_space(18.0);
            if ui
                .add_sized([180.0, 34.0], egui::Button::new("Stop reconnecting"))
                .clicked()
            {
                logging::event("Stopped reconnecting");
                self.reset_session();
                self.forget_session();
                self.phase = Phase::Ended("You stopped reconnecting.".to_string());
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
                RichText::new("The session ended")
                    .font(crate::fonts::medium(20.0))
                    .color(theme.text_primary),
            );
            ui.add_space(8.0);
            ui.add(egui::Label::new(RichText::new(reason).color(theme.text_secondary)).wrap());
            ui.add_space(18.0);
            if ui
                .add_sized([180.0, 34.0], theme.primary_button("Try again"))
                .clicked()
            {
                logging::event("Try again");
                self.sign_in_error = None;
                self.phase = Phase::SignIn;
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

/// This computer's name as the PC will see it: the host name, cut to the 15
/// characters RDP carries, or "Remoport".
fn client_name() -> String {
    let name = std::fs::read_to_string("/etc/hostname").unwrap_or_default();
    let name: String = name.trim().chars().take(15).collect();
    if name.is_empty() {
        "Remoport".to_string()
    } else {
        name
    }
}

/// Whether a failure is the network's, so another attempt may bring the
/// session back. Only a dropped link starts reconnecting; while
/// reconnecting, a PC that cannot be reached yet is worth another try too.
fn worth_retrying(error: &Error, reconnecting: bool) -> bool {
    match error {
        Error::ConnectionLost(_) => true,
        Error::Unreachable(_) | Error::Timeout(_) | Error::Protocol(_) | Error::Gateway(_) => {
            reconnecting
        }
        _ => false,
    }
}

/// The wait before each attempt: quick at first, then every five seconds.
fn reconnect_delay(attempt: u32) -> Duration {
    Duration::from_secs(match attempt {
        0 | 1 => 1,
        2 => 2,
        3 => 4,
        _ => 5,
    })
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect::<String>() + "."
    })
}

/// `host` and `port` from `host` or `host:port` (port 3389 by default).
fn split_host_port(target: &str) -> (&str, u16) {
    match target.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => match port.parse() {
            Ok(port) => (host, port),
            Err(_) => (target, 3389),
        },
        _ => (target, 3389),
    }
}

/// The domain and user of `DOMAIN\user`; other forms (`user`, `user@domain`)
/// are passed whole.
fn split_domain(username: &str) -> (Option<&str>, &str) {
    match username.split_once('\\') {
        Some((domain, user)) if !domain.is_empty() => (Some(domain), user),
        _ => (None, username),
    }
}

/// A desktop size the server accepts: even width, each side within limits.
fn desktop_size((width, height): (u16, u16)) -> (u16, u16) {
    (
        width.clamp(MIN_SIDE, MAX_SIDE) & !1,
        height.clamp(MIN_SIDE, MAX_SIDE),
    )
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

/// Rounds and clamps to the `u16` range, so the cast cannot wrap.
fn to_u16(value: f32) -> u16 {
    value.round().clamp(0.0, f32::from(u16::MAX)) as u16
}

/// Rounds and clamps to ±240, two wheel notches, so the cast cannot wrap.
fn to_i16(value: f32) -> i16 {
    value.round().clamp(-240.0, 240.0) as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_and_port_are_split() {
        assert_eq!(split_host_port("pc.example.com"), ("pc.example.com", 3389));
        assert_eq!(
            split_host_port("pc.example.com:3390"),
            ("pc.example.com", 3390)
        );
        assert_eq!(split_host_port("10.0.0.5:abc"), ("10.0.0.5:abc", 3389));
        // A bare IPv6 address is not mistaken for host:port.
        assert_eq!(split_host_port("fe80::1"), ("fe80::1", 3389));
    }

    #[test]
    fn a_backslash_separates_the_domain() {
        assert_eq!(split_domain("SHD\\jdoe"), (Some("SHD"), "jdoe"));
        assert_eq!(split_domain("jdoe@shd.gov.co"), (None, "jdoe@shd.gov.co"));
        assert_eq!(split_domain("jdoe"), (None, "jdoe"));
        assert_eq!(split_domain("\\jdoe"), (None, "\\jdoe"));
    }

    #[test]
    fn desktop_sizes_are_even_and_within_limits() {
        assert_eq!(desktop_size((1217, 821)), (1216, 821));
        assert_eq!(desktop_size((50, 50)), (200, 200));
        assert_eq!(desktop_size((9000, 9000)), (8192, 8192));
    }

    #[test]
    fn regions_land_in_place_as_rgba() {
        let mut screen = Screen::new(4, 3);
        // A 2×1 BGRA region at (1, 2): blue then red.
        screen.apply(
            RemoteRect {
                x: 1,
                y: 2,
                width: 2,
                height: 1,
            },
            &[255, 0, 0, 0, 0, 0, 255, 0],
        );
        let at = |x: usize, y: usize| &screen.rgba[(y * 4 + x) * 4..(y * 4 + x) * 4 + 4];
        assert_eq!(at(1, 2), &[0, 0, 255, 255]);
        assert_eq!(at(2, 2), &[255, 0, 0, 255]);
        assert_eq!(at(0, 0), &[0, 0, 0, 255]);
    }

    #[test]
    fn a_region_outside_the_screen_is_dropped() {
        let mut screen = Screen::new(4, 3);
        let before = screen.rgba.clone();
        screen.apply(
            RemoteRect {
                x: 3,
                y: 0,
                width: 2,
                height: 1,
            },
            &[1; 8],
        );
        screen.apply(
            RemoteRect {
                x: 0,
                y: 0,
                width: 2,
                height: 2,
            },
            &[1; 4],
        );
        assert_eq!(screen.rgba, before);
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
    fn a_dropped_session_comes_back_until_it_cannot() {
        let pc = DeviceBookmark::new("pc.example".into(), String::new(), String::new());
        let mut view = ActiveSession::new(pc, false);
        view.phase = Phase::Connected;
        view.session_password = Some(Zeroizing::new("secret".into()));

        view.session_ended(Some(Error::ConnectionLost("reset".into())));
        assert!(matches!(
            view.phase,
            Phase::Reconnecting {
                attempt: 1,
                at: Some(_)
            }
        ));
        for expected in 2..=MAX_RECONNECTS {
            view.session_ended(Some(Error::Unreachable("no route".into())));
            assert!(
                matches!(view.phase, Phase::Reconnecting { attempt, .. } if attempt == expected)
            );
            assert!(view.session_password.is_some());
        }
        view.session_ended(Some(Error::Timeout("reaching the PC")));
        assert!(
            matches!(view.phase, Phase::Ended(ref why) if why.contains("could not be brought back"))
        );
        assert!(
            view.session_password.is_none(),
            "the password outlived the session"
        );
    }

    #[test]
    fn a_refused_password_while_reconnecting_asks_again_and_wipes_it() {
        let pc = DeviceBookmark::new("pc.example".into(), String::new(), String::new());
        let mut view = ActiveSession::new(pc, false);
        view.phase = Phase::Reconnecting {
            attempt: 3,
            at: None,
        };
        view.session_password = Some(Zeroizing::new("old".into()));
        view.session_ended(Some(Error::Authentication("changed".into())));
        assert!(matches!(view.phase, Phase::SignIn));
        assert!(view.session_password.is_none());

        // A session the PC ended itself is not brought back.
        view.phase = Phase::Connected;
        view.session_password = Some(Zeroizing::new("secret".into()));
        view.session_ended(None);
        assert!(matches!(view.phase, Phase::Ended(_)));
        assert!(view.session_password.is_none());
    }

    #[test]
    fn reconnecting_starts_quick_and_settles() {
        let waits: Vec<u64> = (1..=6).map(|a| reconnect_delay(a).as_secs()).collect();
        assert_eq!(waits, vec![1, 2, 4, 5, 5, 5]);
        let total: u64 = (1..=MAX_RECONNECTS)
            .map(|a| reconnect_delay(a).as_secs())
            .sum();
        assert!(total <= 120, "{total} s of waiting");
    }

    #[test]
    fn the_client_name_fits_rdp() {
        let name = client_name();
        assert!(!name.is_empty() && name.chars().count() <= 15);
    }
}
