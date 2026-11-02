//! A remote desktop session drawn inside Remoport, through RustConn's
//! IronRDP engine (`rustconn-core`): sign in, show the remote screen, pass
//! keyboard and mouse, follow the window's size. FreeRDP stays available as
//! the way out when the engine cannot serve a PC.

use crate::keymap;
use crate::logging;
use crate::model::DeviceBookmark;
use crate::theme::Theme;
use egui::{
    pos2, vec2, Align2, Color32, ColorImage, Context, Event, FontId, Modifiers, PointerButton,
    Rect, RichText, Rounding, Sense, TextureHandle, TextureOptions, Ui,
};
use rustconn_core::rdp_client::{
    classify_rdp_failure, RdpClient, RdpClientCommand, RdpClientConfig, RdpClientEvent,
    RdpFailureClass, RdpRect,
};
use std::time::{Duration, Instant};
use zeroize::{Zeroize, Zeroizing};

/// FreeRDP clients to try, newest first: FreeRDP 3 renamed its binaries.
const FREERDP_CLIENTS: &[&str] = &["sdl-freerdp3", "xfreerdp3", "wlfreerdp", "xfreerdp"];

/// How long the window must keep a new size before the remote desktop follows.
const RESIZE_SETTLE: Duration = Duration::from_millis(400);

/// Remote desktop sizes the display-control channel accepts (MS-RDPEDISP 2.2.2.2.1).
const MIN_SIDE: u16 = 200;
const MAX_SIDE: u16 = 8192;

/// Frame events handled per repaint at most, so a burst cannot stall the UI.
const EVENTS_PER_FRAME: usize = 2000;

enum Phase {
    /// Asking for the user name and password.
    SignIn,
    Connecting,
    Connected,
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
    fn apply(&mut self, rect: RdpRect, bgra: &[u8]) {
        let (x, y) = (usize::from(rect.x), usize::from(rect.y));
        let (w, h) = (usize::from(rect.width), usize::from(rect.height));
        if w == 0 || h == 0 || x + w > self.width || y + h > self.height || bgra.len() < w * h * 4
        {
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
    phase: Phase,
    username: String,
    password: Zeroizing<String>,
    sign_in_error: Option<String>,
    client: Option<RdpClient>,
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
}

impl ActiveSession {
    pub fn new(bookmark: DeviceBookmark) -> Self {
        Self {
            username: bookmark.username.clone(),
            bookmark,
            open_app: None,
            notification_msg: None,
            phase: Phase::SignIn,
            password: Zeroizing::new(String::new()),
            sign_in_error: None,
            client: None,
            screen: None,
            graphics: None,
            canvas_px: (1280, 720),
            pending_size: None,
            requested_size: None,
            display_control: false,
            remote_cursor_hidden: false,
            modifiers: Modifiers::NONE,
            last_pointer: None,
        }
    }

    pub fn render(&mut self, ui: &mut Ui, theme: &Theme, on_disconnect: &mut bool) {
        self.poll_events();

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

        match self.phase {
            Phase::SignIn => self.sign_in_form(ui, theme),
            Phase::Connecting => {
                self.measure_canvas(ui);
                self.message(
                    ui,
                    &format!("Connecting to {}…", self.bookmark.pc_name),
                    "Negotiating security and signing in.",
                );
            }
            Phase::Connected => self.remote_screen(ui),
            Phase::Ended(ref reason) => {
                let reason = reason.clone();
                self.ended(ui, theme, &reason);
            }
        }

        if matches!(self.phase, Phase::Connecting | Phase::Connected) {
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
                    .button(if connected { "❌ Disconnect" } else { "❌ Close" })
                    .clicked()
                {
                    logging::event(format_args!(
                        "{} {}",
                        if connected { "Disconnected from" } else { "Closed the session of" },
                        self.bookmark.pc_name
                    ));
                    *on_disconnect = true;
                }
                if connected && ui.button("⌨ Ctrl+Alt+Del").clicked() {
                    logging::event("Sent Ctrl+Alt+Del");
                    self.send(RdpClientCommand::SendCtrlAltDel);
                }
                if ui.button("🖥 Open in FreeRDP").clicked() {
                    self.open_in_freerdp();
                }
            });
        });
    }

    fn status(&self) -> String {
        match self.phase {
            Phase::SignIn => "Sign in".to_string(),
            Phase::Connecting => "Connecting…".to_string(),
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
            Phase::Ended(_) => "Disconnected".to_string(),
        }
    }

    fn open_in_freerdp(&mut self) {
        let host = self.bookmark.pc_name.clone();
        let (domain, user) = split_domain(self.username.trim());
        let mut args = vec![format!("/v:{host}")];
        if !user.is_empty() {
            args.push(format!("/u:{user}"));
        }
        if let Some(domain) = domain {
            args.push(format!("/d:{domain}"));
        }
        logging::event(format_args!("Open in FreeRDP: {}", args.join(" ")));
        std::thread::spawn(move || {
            let started = FREERDP_CLIENTS
                .iter()
                .find(|client| std::process::Command::new(client).args(&args).spawn().is_ok());
            match started {
                Some(client) => logging::event(format_args!("FreeRDP started: {client}")),
                None => logging::event(format_args!(
                    "No FreeRDP client found (tried {})",
                    FREERDP_CLIENTS.join(", ")
                )),
            }
        });
        self.notification_msg = Some("Opening the session in FreeRDP…".to_string());
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
                    let field = ui.add(
                        egui::TextEdit::singleline(&mut *self.password)
                            .password(true)
                            .desired_width(280.0),
                    );
                    submit |= field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.end_row();
                });
            });

            if let Some(error) = &self.sign_in_error {
                ui.add_space(8.0);
                ui.colored_label(Color32::from_rgb(0xc4, 0x2b, 0x1c), error);
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
                RichText::new("Your password is used for this connection only and is not saved.")
                    .size(12.0)
                    .color(theme.text_secondary),
            );
        });
    }

    fn connect(&mut self) {
        let (host, port) = split_host_port(self.bookmark.pc_name.trim());
        let username = self.username.trim().to_string();
        let (domain, user) = split_domain(&username);
        let (width, height) = desktop_size(self.canvas_px);

        let mut config = RdpClientConfig::new(host)
            .with_port(port)
            .with_resolution(width, height)
            // Phase 1 shows the desktop only: no clipboard or sound channel
            // that would wait on answers Remoport does not give yet.
            .with_clipboard(false)
            .with_audio(false)
            .with_connection_name(self.bookmark.display_title());
        if !user.is_empty() {
            config = config.with_username(user);
        }
        if let Some(domain) = domain {
            config = config.with_domain(domain);
        }
        if !self.password.is_empty() {
            config = config.with_password(self.password.as_str());
        }
        // The engine keeps its own protected copy; this one is wiped now.
        self.password.zeroize();

        logging::event(format_args!(
            "Connecting natively to {host}:{port} as {} at {width}×{height}{}",
            if user.is_empty() { "(no user name)" } else { user },
            domain.map(|d| format!(", domain {d}")).unwrap_or_default(),
        ));
        let mut client = RdpClient::new(config);
        match client.connect() {
            Ok(()) => {
                self.client = Some(client);
                self.sign_in_error = None;
                self.phase = Phase::Connecting;
            }
            Err(err) => self.end(format!("The connection could not start: {err}")),
        }
    }

    // ── Engine events ───────────────────────────────────────────────────

    fn poll_events(&mut self) {
        let Some(client) = &self.client else {
            return;
        };
        let mut events = Vec::new();
        while events.len() < EVENTS_PER_FRAME {
            match client.try_recv_event() {
                Some(event) => events.push(event),
                None => break,
            }
        }
        for event in events {
            self.handle_event(event);
        }
    }

    fn handle_event(&mut self, event: RdpClientEvent) {
        match event {
            RdpClientEvent::Connected { width, height } => {
                logging::event(format_args!("Connected to {} at {width}×{height}", self.bookmark.pc_name));
                self.screen = Some(Screen::new(width, height));
                self.phase = Phase::Connected;
            }
            RdpClientEvent::ResolutionChanged { width, height } => {
                logging::event(format_args!("Remote desktop is now {width}×{height}"));
                self.screen = Some(Screen::new(width, height));
                self.requested_size = None;
            }
            RdpClientEvent::FrameUpdate { rect, data } => {
                if let Some(screen) = &mut self.screen {
                    screen.apply(rect, &data);
                }
            }
            RdpClientEvent::FullFrameUpdate { width, height, data } => {
                let mut screen = Screen::new(width, height);
                screen.apply(RdpRect { x: 0, y: 0, width, height }, &data);
                self.screen = Some(screen);
            }
            RdpClientEvent::GraphicsModeActive { mode } => {
                logging::event(format_args!("Graphics: {}", mode.display_name()));
                self.graphics = Some(mode.display_name().to_string());
            }
            RdpClientEvent::DisplayControlReady => {
                logging::event("The remote desktop can follow the window's size");
                self.display_control = true;
            }
            RdpClientEvent::DisplayControlUnavailable { .. } => {
                logging::event("The remote desktop cannot change size without reconnecting");
                self.display_control = false;
            }
            RdpClientEvent::CursorHidden => self.remote_cursor_hidden = true,
            RdpClientEvent::CursorDefault | RdpClientEvent::CursorUpdate { .. } => {
                self.remote_cursor_hidden = false;
            }
            RdpClientEvent::ServerMessage(message) => {
                logging::event(format_args!("Message from the PC: {message}"));
                self.notification_msg = Some(message);
            }
            RdpClientEvent::AuthRequired => {
                logging::event("The PC asked for a user name and password");
                self.back_to_sign_in("This PC needs your user name and password.");
            }
            RdpClientEvent::Error(message) => {
                logging::event(format_args!("Connection error: {message}"));
                if classify_rdp_failure(&message) == RdpFailureClass::Authentication {
                    self.back_to_sign_in("The PC refused the sign-in. Check the user name and password.");
                } else {
                    self.end(message);
                }
            }
            RdpClientEvent::ConnectionLost(message) => {
                logging::event(format_args!("Connection lost: {message}"));
                self.end(format!("The connection was lost: {message}"));
            }
            RdpClientEvent::CertificateChanged {
                host,
                port,
                new_fingerprint,
                old_fingerprint,
            } => {
                logging::event(format_args!(
                    "Certificate of {host}:{port} changed: was {old_fingerprint}, now {new_fingerprint}"
                ));
                self.end(format!(
                    "The certificate of {host} changed since the last connection. Connect with \
                     FreeRDP to review it, or ask your administrator whether it was replaced."
                ));
            }
            RdpClientEvent::Disconnected => {
                if matches!(self.phase, Phase::Connecting | Phase::Connected) {
                    logging::event("The PC ended the session");
                    self.end("The remote PC ended the session.".to_string());
                }
            }
            _ => {}
        }
    }

    fn back_to_sign_in(&mut self, error: &str) {
        self.drop_client();
        self.sign_in_error = Some(error.to_string());
        self.phase = Phase::SignIn;
    }

    fn end(&mut self, reason: String) {
        self.drop_client();
        self.phase = Phase::Ended(reason);
    }

    fn drop_client(&mut self) {
        if let Some(mut client) = self.client.take() {
            client.disconnect();
        }
        self.screen = None;
        self.display_control = false;
        self.pending_size = None;
        self.requested_size = None;
        self.modifiers = Modifiers::NONE;
    }

    fn send(&self, command: RdpClientCommand) {
        if let Some(client) = &self.client {
            let _ = client.send_command(command);
        }
    }

    fn send_key(&self, (scancode, extended): keymap::Scancode, pressed: bool) {
        self.send(RdpClientCommand::KeyEvent {
            scancode: u16::from(scancode),
            pressed,
            extended,
        });
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
        ui.painter().rect_filled(rect, Rounding::ZERO, Color32::BLACK);
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
                    if let Some(at) = to_remote(pos) {
                        if self.last_pointer != Some(at) {
                            self.last_pointer = Some(at);
                            self.send(RdpClientCommand::PointerEvent { x: at.0, y: at.1, buttons: 0 });
                        }
                    }
                }
                Event::PointerButton { pos, button, pressed, .. } => {
                    // A release outside the picture still goes, so no button sticks.
                    let at = to_remote(pos).or(if pressed { None } else { self.last_pointer });
                    if let (Some((x, y)), Some(button)) = (at, rdp_button(button)) {
                        self.send(if pressed {
                            RdpClientCommand::MouseButtonPress { x, y, button }
                        } else {
                            RdpClientCommand::MouseButtonRelease { x, y, button }
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
                        self.send(RdpClientCommand::WheelEvent { horizontal, vertical });
                    }
                }
                Event::Key { key, physical_key, pressed, .. } if focused => {
                    if let Some(code) = keymap::scancode(physical_key.unwrap_or(key)) {
                        self.send_key(code, pressed);
                    }
                }
                // egui turns these shortcuts into clipboard commands and
                // swallows the key press; the release still arrives as a key.
                Event::Copy if focused => self.send_key(keymap::C, true),
                Event::Cut if focused => self.send_key(keymap::X, true),
                Event::Paste(_) if focused => self.send_key(keymap::V, true),
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
                    self.send(RdpClientCommand::SetDesktopSize {
                        width: target.0,
                        height: target.1,
                        scale_percent: None,
                    });
                    self.requested_size = Some(target);
                    self.pending_size = None;
                }
            }
            _ => self.pending_size = Some((target, Instant::now())),
        }
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
            ui.label(
                RichText::new("The session ended")
                    .font(crate::fonts::medium(20.0))
                    .color(theme.text_primary),
            );
            ui.add_space(8.0);
            ui.add(egui::Label::new(RichText::new(reason).color(theme.text_secondary)).wrap());
            ui.add_space(18.0);
            ui.horizontal(|ui| {
                let width = 2.0 * 160.0 + ui.spacing().item_spacing.x;
                ui.add_space((ui.available_width() - width).max(0.0) / 2.0);
                if ui.add_sized([160.0, 34.0], theme.primary_button("Try again")).clicked() {
                    logging::event("Try again");
                    self.sign_in_error = None;
                    self.phase = Phase::SignIn;
                }
                if ui.add_sized([160.0, 34.0], egui::Button::new("Open in FreeRDP")).clicked() {
                    self.open_in_freerdp();
                }
            });
        });
    }
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

fn rdp_button(button: PointerButton) -> Option<u8> {
    match button {
        PointerButton::Primary => Some(1),
        PointerButton::Secondary => Some(2),
        PointerButton::Middle => Some(3),
        PointerButton::Extra1 => Some(4),
        PointerButton::Extra2 => Some(5),
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
        assert_eq!(split_host_port("pc.example.com:3390"), ("pc.example.com", 3390));
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
            RdpRect { x: 1, y: 2, width: 2, height: 1 },
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
        screen.apply(RdpRect { x: 3, y: 0, width: 2, height: 1 }, &[1; 8]);
        screen.apply(RdpRect { x: 0, y: 0, width: 2, height: 2 }, &[1; 4]);
        assert_eq!(screen.rgba, before);
    }

    #[test]
    fn mouse_buttons_match_the_engine() {
        assert_eq!(rdp_button(PointerButton::Primary), Some(1));
        assert_eq!(rdp_button(PointerButton::Secondary), Some(2));
        assert_eq!(rdp_button(PointerButton::Middle), Some(3));
    }
}
