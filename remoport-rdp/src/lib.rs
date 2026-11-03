//! Remoport's RDP engine, built on IronRDP.
//!
//! Security model:
//! - The PC's identity is checked after TLS and before CredSSP: a certificate
//!   from an authority this computer trusts is accepted, anything else is put
//!   to the user, and a certificate that changed since the user trusted it is
//!   presented as the warning it is. See [`identity`](crate::identity).
//! - Network Level Authentication is required unless a PC is explicitly
//!   allowed to sign in without it; the original RDP security is never used.
//! - The password lives in zeroizing memory on Remoport's side; IronRDP's
//!   credential types print it in `Debug`, so they are never formatted.
//! - No clipboard, sound, device or drive channel is offered.
//!
//! Parts follow RustConn's client (`rustconn-core`, by Anton Isaiev,
//! GPL-3.0-or-later), as noted in each module.

mod connect;
mod error;
mod gfx;
mod identity;
mod keyboard_layout;
mod net;
mod session;

pub use error::Error;
pub use identity::{
    CertificateInfo, IdentityProblem, TrustStore, TrustedServer, fingerprint, write_private,
};

use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

use zeroize::Zeroizing;

/// What to connect to, and how.
pub struct ConnectConfig {
    pub host: String,
    pub port: u16,
    /// `user` or `user@domain`; a `DOMAIN\user` form goes in two parts.
    pub username: String,
    pub domain: Option<String>,
    pub password: Zeroizing<String>,
    pub width: u16,
    pub height: u16,
    /// Lets this PC sign in on its own screen when it does not offer NLA.
    pub allow_without_nla: bool,
    /// Where the certificates the user trusted are kept.
    pub trust_store: TrustStore,
    /// The limit for each step of the connection, the user's decisions aside.
    pub timeout: Duration,
    /// The name this computer gives the PC.
    pub client_name: String,
}

/// A rectangle of the remote desktop, in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    X1,
    X2,
}

/// The user's answer about a PC's identity. Dropping it without answering
/// counts as "no".
pub struct TrustReply(tokio::sync::oneshot::Sender<bool>);

impl TrustReply {
    pub fn answer(self, trust: bool) {
        let _ = self.0.send(trust);
    }
}

impl std::fmt::Debug for TrustReply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TrustReply")
    }
}

/// What the engine reports.
#[derive(Debug)]
pub enum Event {
    /// A step of the connection, for the log.
    Progress(String),
    /// The PC's identity needs the user's decision before anything is sent.
    VerifyIdentity {
        info: CertificateInfo,
        problem: IdentityProblem,
        reply: TrustReply,
    },
    Connected { width: u16, height: u16 },
    /// The remote desktop changed size; frames that follow use it.
    Resized { width: u16, height: u16 },
    /// New pixels for `rect`, BGRA, row after row.
    Frame { rect: Rect, bgra: Vec<u8> },
    /// The graphics mode the PC chose.
    Graphics(String),
    /// The desktop can now follow the window's size.
    DisplayControlReady,
    PointerHidden(bool),
    /// The session is over: `None` when it ended normally.
    Ended(Option<Error>),
}

/// What the user does in the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Key {
        scancode: u8,
        extended: bool,
        pressed: bool,
    },
    PointerMove {
        x: u16,
        y: u16,
    },
    MouseButton {
        x: u16,
        y: u16,
        button: MouseButton,
        pressed: bool,
    },
    Wheel {
        vertical: i16,
        horizontal: i16,
    },
    Resize {
        width: u16,
        height: u16,
    },
    CtrlAltDel,
}

/// A connection running on its own thread.
pub struct Session {
    commands: tokio::sync::mpsc::UnboundedSender<Command>,
    events: mpsc::Receiver<Event>,
    cancel: tokio::sync::watch::Sender<bool>,
    thread: Option<JoinHandle<()>>,
}

impl Session {
    /// Starts connecting. Progress, the identity question and the outcome
    /// arrive as [`Event`]s.
    #[must_use]
    pub fn start(config: ConnectConfig) -> Self {
        let (event_tx, events) = mpsc::channel();
        let (commands, command_rx) = tokio::sync::mpsc::unbounded_channel();
        let (cancel, cancelled) = tokio::sync::watch::channel(false);
        let thread = std::thread::Builder::new()
            .name("remoport-rdp".to_string())
            .spawn(move || {
                let result = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime.block_on(run(config, &event_tx, command_rx, cancelled)),
                    Err(e) => Err(Error::Protocol(format!(
                        "the network runtime could not start: {e}"
                    ))),
                };
                let _ = event_tx.send(Event::Ended(result.err()));
            })
            .ok();
        Self {
            commands,
            events,
            cancel,
            thread,
        }
    }

    #[must_use]
    pub fn try_event(&self) -> Option<Event> {
        self.events.try_recv().ok()
    }

    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }

    /// Ends the session, at any stage, and waits for its thread.
    pub fn disconnect(&mut self) {
        let _ = self.cancel.send(true);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.disconnect();
    }
}

async fn run(
    config: ConnectConfig,
    events: &mpsc::Sender<Event>,
    commands: tokio::sync::mpsc::UnboundedReceiver<Command>,
    mut cancelled: tokio::sync::watch::Receiver<bool>,
) -> Result<(), Error> {
    let established = tokio::select! {
        result = connect::connect(&config, events) => result?,
        _ = cancelled.changed() => return Ok(()),
    };
    // The password is not needed past the sign-in: wipe Remoport's copy now.
    drop(config);
    session::run(established, events, commands, cancelled).await
}
