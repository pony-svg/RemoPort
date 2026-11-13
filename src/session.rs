//! A remote desktop session, drawn inside Remoport by its own RDP engine
//! (`remoport-rdp`): sign in, confirm the PC's identity when it cannot be
//! verified automatically, show the remote screen, pass keyboard and mouse,
//! follow the window's size, share the clipboard and play sound when this PC
//! has them, and bring a dropped connection back to the same session.

use crate::domain::{AudioPlaybackMode, DeviceBookmark};
use crate::i18n::t;
use crate::key_grab::KeyGrab;
use crate::keymap;
use crate::logging;
use crate::store;
use crate::theme::Theme;
use egui::{Context, Modifiers, Ui};
use remoport_rdp::{
    ClipboardText, Command, ConnectConfig, Error, Event as EngineEvent, IdentityProblem,
    Rect as RemoteRect, SharedFolder, SoundInput, SoundOutput, TrustStore,
};
use std::collections::HashSet;
use std::sync::atomic::AtomicBool;
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

mod view;
pub use view::screen_scale;
#[cfg(test)]
use view::{remote_button, scale_percent};
use view::{ViewState, FULLSCREEN_HINT};

struct SessionInput {
    /// Modifier keys held on the remote side.
    modifiers: Modifiers,
    last_pointer: Option<(u16, u16)>,
    /// The desktop's reserved keys, held back while in full screen.
    key_grab: Option<KeyGrab>,
    /// Asked for this stretch of full screen, whether it could be had or not.
    key_grab_tried: bool,
    /// Events from the physical Wayland keyboard listener.
    wayland_keys_rx: Option<std::sync::mpsc::Receiver<crate::key_grab::WaylandKeyEvent>>,
    /// State machine tracking physical and fallback keys.
    state: crate::input::KeyState,
    /// Keys pressed via egui fallback when Wayland is not available.
    fallback_keys_down: HashSet<keymap::Scancode>,
}

impl SessionInput {
    fn new(share_clipboard: bool) -> Self {
        Self {
            modifiers: Modifiers::NONE,
            last_pointer: None,
            key_grab: None,
            key_grab_tried: false,
            wayland_keys_rx: None,
            state: crate::input::KeyState::new(false, share_clipboard),
            fallback_keys_down: HashSet::new(),
        }
    }

    fn reset(&mut self) {
        self.modifiers = Modifiers::NONE;
        self.key_grab = None;
        self.key_grab_tried = false;
        self.wayland_keys_rx = None;
        self.state.handle_leave();
        self.fallback_keys_down.clear();
    }
}

struct SessionMedia {
    /// Text the PC copied, until it is taken (or not) on the next frame.
    remote_copy: Option<ClipboardText>,
    /// The PC has the microphone open.
    microphone_live: bool,
    /// The user muted the microphone; the PC then gets silence.
    microphone_muted: Arc<AtomicBool>,
}

impl SessionMedia {
    fn new() -> Self {
        Self {
            remote_copy: None,
            microphone_live: false,
            microphone_muted: Arc::new(AtomicBool::new(false)),
        }
    }

    fn reset(&mut self) {
        self.remote_copy = None;
        self.microphone_live = false;
    }
}

pub struct ActiveSession {
    pub bookmark: DeviceBookmark,
    pub open_app: Option<String>,
    /// Whether the PC is saved, so its password may be remembered.
    saved: bool,
    pub controller: SessionController,
    pub signin: SignInState,
    pub display: DisplayState,
    pub screen: ScreenState,
    input: SessionInput,
    media: SessionMedia,
    view: ViewState,
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
            saved,
            controller: SessionController::new(),
            signin,
            display: DisplayState::new(screens),
            screen: ScreenState::new(),
            input: SessionInput::new(share_clipboard),
            media: SessionMedia::new(),
            view: ViewState::new(),
        }
    }

    pub fn render(&mut self, ui: &mut Ui, theme: &Theme, on_disconnect: &mut bool) {
        view::render(self, ui, theme, on_disconnect);
    }

    // ── Keyring ─────────────────────────────────────────────────────────

    fn poll_keyring(&mut self) {
        match self.signin.poll_keyring(&self.bookmark.pc_name) {
            Some(KeyringEvent::Saved) => {
                self.view.notification_msg =
                    Some(t("Your password is remembered in the system keyring.").to_string());
            }
            Some(KeyringEvent::Failed(_)) => {
                self.view.notification_msg = Some(
                    t("The system keyring is not available, so no password is remembered.")
                        .to_string(),
                );
            }
            _ => {}
        }
    }

    // ── Connect ─────────────────────────────────────────────────────────

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
        let Some(trust_store) = store::trust_store_path().map(TrustStore::new) else {
            self.forget_session();
            self.controller.phase = Phase::Ended(EndReason::NoTrustStoreFolder);
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
                &self.media.microphone_muted,
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
            EngineEvent::Clipboard(text) => self.media.remote_copy = Some(text),
            EngineEvent::Microphone(live) => self.media.microphone_live = live,
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
                self.controller.phase = Phase::Ended(EndReason::ReconnectExhausted {
                    pc_name: self.bookmark.pc_name.clone(),
                    attempts: MAX_RECONNECTS,
                    detail: error.to_string(),
                });
                return;
            }
        }
        self.forget_session();
        match outcome {
            None => {
                logging::event("Session ended");
                if !matches!(self.controller.phase, Phase::Ended(_)) {
                    self.controller.phase = Phase::Ended(EndReason::ClosedByRemote);
                }
            }
            Some(error) => {
                logging::event(format_args!("Session ended: {error}"));
                match error {
                    Error::Authentication(_) => {
                        let was_saved = self.signin.on_auth_failed();
                        let kind = if was_saved {
                            SignInError::RefusedSavedPassword
                        } else {
                            SignInError::RefusedCredentials
                        };
                        self.signin.sign_in_error = Some(kind);
                        self.controller.phase = Phase::SignIn;
                    }
                    Error::NlaUnavailable => {
                        self.controller.phase = Phase::Ended(EndReason::NlaUnavailable {
                            pc_name: self.bookmark.pc_name.clone(),
                        });
                    }
                    Error::IdentityRejected => {
                        self.controller.phase = Phase::Ended(EndReason::IdentityRejected);
                    }
                    other => {
                        self.controller.phase = Phase::Ended(EndReason::ConnectionFailed(
                            ConnectionFailure::from_error(&other),
                        ));
                    }
                }
            }
        }
    }

    fn reset_session(&mut self) {
        self.controller.disconnect();
        self.screen.reset();
        self.display.display_control = false;
        self.display.reset();
        self.input.reset();
        self.media.reset();
        self.view.reset();
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
                        self.view.fullscreen_hint_until = Some(Instant::now() + FULLSCREEN_HINT);
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
        let Some(rx) = &self.input.wayland_keys_rx else {
            return;
        };
        let now = Instant::now();
        let events: Vec<_> = rx.try_iter().collect();
        for event in events {
            match event {
                crate::key_grab::WaylandKeyEvent::Key { evdev, pressed } => {
                    if focused {
                        if pressed && self.input.state.is_alt_held() {
                            match evdev {
                                15 => logging::event("Sent Alt+Tab"),
                                62 => logging::event("Sent Alt+F4"),
                                _ => {}
                            }
                        }
                        let actions = self.input.state.handle_physical_key(evdev, pressed, now);
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
                    self.input.state.handle_enter(&keys);
                }
                crate::key_grab::WaylandKeyEvent::Leave => {
                    let actions = self.input.state.handle_leave();
                    self.execute_input_actions(actions, ctx, fullscreen);
                }
                crate::key_grab::WaylandKeyEvent::Repeat { evdev } => {
                    if focused {
                        let actions = self.input.state.handle_physical_key(evdev, true, now);
                        self.execute_input_actions(actions, ctx, fullscreen);
                    }
                }
            }
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

    /// The part of the remote desktop window `index` shows.
    fn region(&self, index: usize) -> Option<RemoteRect> {
        let screen = self.screen.screen.as_ref()?;
        self.display.region(index, screen.width, screen.height)
    }

    /// Presses or releases Shift, Ctrl and Alt on the remote side to match.
    fn sync_modifiers(&mut self, now: Modifiers) {
        for (held, was, code) in [
            (now.shift, self.input.modifiers.shift, keymap::SHIFT),
            (now.ctrl, self.input.modifiers.ctrl, keymap::CTRL),
            (now.alt, self.input.modifiers.alt, keymap::ALT),
        ] {
            if held != was {
                self.send_key(code, held);
            }
        }
        self.input.modifiers = now;
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
}

impl Drop for ActiveSession {
    fn drop(&mut self) {
        // Any password still held is wiped by its Zeroizing wrapper; the
        // engine's thread is stopped here rather than left running.
        self.reset_session();
    }
}

/// What to send for one key event. A release with no press behind it goes
/// nowhere, since the PC has no press to match, except Ctrl+V: with nothing to
/// paste on this computer egui keeps the press for itself, and the PC still
/// gets the keystroke, for what it has on its own clipboard.
pub(super) fn strokes_for(
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

#[cfg(test)]
mod tests {
    use super::*;
    use egui::PointerButton;
    use remoport_rdp::MouseButton;
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
        pc.resolution = crate::domain::ResolutionOption::Fhd1080p;
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
        pc.resolution = crate::domain::ResolutionOption::Fhd1080p;
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
        assert!(matches!(
            view.controller.phase,
            Phase::Ended(EndReason::ReconnectExhausted { .. })
        ));
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
