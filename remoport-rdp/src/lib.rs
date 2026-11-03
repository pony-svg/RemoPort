//! Remoport's RDP engine, built on IronRDP.
//!
//! Security model:
//! - The PC's identity is checked after TLS and before CredSSP: a certificate
//!   from an authority this computer trusts is accepted, anything else is put
//!   to the user, and a certificate that changed since the user trusted it is
//!   presented as the warning it is. See [`identity`](crate::identity).
//! - An RD Gateway must present a certificate from an authority this computer
//!   trusts; any other is refused before credentials are sent. See
//!   [`gateway`](crate::gateway).
//! - Network Level Authentication is required unless a PC is explicitly
//!   allowed to sign in without it; the original RDP security is never used.
//! - The password lives in zeroizing memory on Remoport's side; IronRDP's
//!   credential types print it in `Debug`, so they are never formatted.
//! - The clipboard is shared, as text, only with the PCs the user allows it:
//!   the PC gets only what the user pastes into it. See
//!   [`clipboard`](crate::clipboard).
//! - The PC's sound can be played here, and the microphone and local folders
//!   offered to it, each only for the PCs the user allows. The microphone
//!   runs only while the PC holds it open; a shared folder confines the PC
//!   to it and is read-only unless the user allows writing. See
//!   [`microphone`](crate::microphone) and [`drive`](crate::drive). No
//!   printer, smart card or camera is offered.
//! - A dropped connection can resume the same session: the PC's cookie for it
//!   ([`ResumeCookie`]) is wiped when dropped, never printed and never saved.
//!
//! Parts follow RustConn's client (`rustconn-core`, by Anton Isaiev,
//! GPL-3.0-or-later), as noted in each module.

mod clipboard;
mod connect;
mod drive;
mod error;
mod gateway;
mod gfx;
mod identity;
mod kerberos;
mod keyboard_layout;
mod microphone;
mod net;
mod session;
mod sound;

pub use error::Error;
pub use identity::{
    CertificateInfo, IdentityProblem, TrustStore, TrustedServer, fingerprint, write_private,
};
pub use kerberos::{KdcAddressError, normalize_kdc_address};
pub use microphone::{MicrophonePackets, SoundInput};
pub use sound::SoundOutput;

use std::path::PathBuf;
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
    /// Shares the clipboard's text with this PC, both ways.
    pub clipboard: bool,
    /// Plays this PC's sound here; `None` leaves it silent.
    pub sound: Option<Box<dyn SoundOutput>>,
    /// Lets this PC use the microphone; `None` offers none.
    pub microphone: Option<Box<dyn SoundInput>>,
    /// Local folders this PC sees as drives.
    pub folders: Vec<SharedFolder>,
    /// The RD Gateway to reach this PC through, as `host` or `host:port`;
    /// it signs in with the PC's credentials.
    pub gateway: Option<String>,
    /// Signs in with Kerberos rather than NTLM.
    pub kerberos: Option<Kerberos>,
    /// Resumes the session this cookie came from, after a dropped connection.
    pub resume: Option<ResumeCookie>,
}

/// The PC's cookie for a session ([MS-RDPBCGR] 2.2.4.2): presented when
/// reconnecting, it proves this computer was the one attached, and the PC
/// resumes that session instead of signing in anew. It is credential
/// material: wiped when dropped, never printed, never saved.
///
/// [MS-RDPBCGR]: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpbcgr/
#[derive(Clone)]
pub struct ResumeCookie {
    logon_id: u32,
    random_bits: Zeroizing<[u8; 16]>,
}

impl ResumeCookie {
    pub(crate) fn new(logon_id: u32, random_bits: [u8; 16]) -> Self {
        Self {
            logon_id,
            random_bits: Zeroizing::new(random_bits),
        }
    }
}

impl std::fmt::Debug for ResumeCookie {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResumeCookie")
            .field("logon_id", &self.logon_id)
            .finish_non_exhaustive()
    }
}

/// Kerberos sign-in for a PC. See [`kerberos`](crate::kerberos).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Kerberos {
    /// The KDC to ask (see [`normalize_kdc_address`]); `None` finds it from
    /// the environment, `krb5.conf` or the realm's DNS name.
    pub kdc: Option<String>,
}

/// A local folder the PC sees as a drive under `\\tsclient`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedFolder {
    pub path: PathBuf,
    /// The PC may read the folder but not change it.
    pub read_only: bool,
}

/// Clipboard text: wiped from memory when dropped, and never printed.
#[derive(Clone, PartialEq, Eq)]
pub struct ClipboardText(Zeroizing<String>);

impl ClipboardText {
    #[must_use]
    pub fn new(text: String) -> Self {
        Self(Zeroizing::new(text))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for ClipboardText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ClipboardText({} characters)", self.0.chars().count())
    }
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
    Connected {
        width: u16,
        height: u16,
    },
    /// The remote desktop changed size; frames that follow use it.
    Resized {
        width: u16,
        height: u16,
    },
    /// New pixels for `rect`, BGRA, row after row.
    Frame {
        rect: Rect,
        bgra: Vec<u8>,
    },
    /// The graphics mode the PC chose.
    Graphics(String),
    /// The desktop can now follow the window's size.
    DisplayControlReady,
    PointerHidden(bool),
    /// The PC copied this text. It is the app's to take or leave.
    Clipboard(ClipboardText),
    /// The PC opened (`true`) or closed the microphone.
    Microphone(bool),
    /// The PC issued (or renewed, hourly) the cookie that resumes this
    /// session after a dropped connection; keep the latest.
    ResumeCookie(ResumeCookie),
    /// The session is over: `None` when it ended normally.
    Ended(Option<Error>),
}

/// What the user does in the session.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// The user pasted this text: the PC gets it, when the clipboard is
    /// shared with it, and then the V of Ctrl+V. Ctrl itself is sent as a key.
    Paste(ClipboardText),
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
    mut config: ConnectConfig,
    events: &mpsc::Sender<Event>,
    commands: tokio::sync::mpsc::UnboundedReceiver<Command>,
    mut cancelled: tokio::sync::watch::Receiver<bool>,
) -> Result<(), Error> {
    let established = tokio::select! {
        result = connect::connect(&mut config, events) => result?,
        _ = cancelled.changed() => return Ok(()),
    };
    // The password is not needed past the sign-in: wipe Remoport's copy now.
    drop(config);
    session::run(established, events, commands, cancelled).await
}
