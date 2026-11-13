//! Session lifecycle, reconnection, and engine event polling.
//!
//! Encapsulates connection phase state transitions, network retry delays,
//! session dispatching, and error categorization.
//!
//! This module contains pure session control logic and does not depend on UI (R3).

use std::time::{Duration, Instant};

use remoport_rdp::{
    CertificateInfo, Command, ConnectConfig, Error, Event as EngineEvent, IdentityProblem, Session,
    TrustReply,
};

use crate::logging;

/// Limit for each step of the connection; the user's own decisions aside.
pub const STEP_TIMEOUT: Duration = Duration::from_secs(30);

/// Limit for each step of a reconnection: a network that is still down
/// should cost the next attempt, not half a minute.
pub const RECONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Attempts to bring a dropped connection back before giving up.
pub const MAX_RECONNECTS: u32 = 20;

/// Limit of events processed per UI frame to prevent stalling.
pub const EVENTS_PER_FRAME: usize = 2000;

/// Where a session stands.
#[derive(Debug)]
pub enum Phase {
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

/// The state of the active connection and its lifecycle phase.
pub struct SessionController {
    /// Current connection phase.
    pub phase: Phase,
    /// Background RDP engine session when connected or connecting.
    pub session: Option<Session>,
}

impl Default for SessionController {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionController {
    /// Creates a new controller in `SignIn` phase.
    #[must_use]
    pub fn new() -> Self {
        Self {
            phase: Phase::SignIn,
            session: None,
        }
    }

    /// Whether the session is currently connected.
    #[must_use]
    pub fn is_connected(&self) -> bool {
        matches!(self.phase, Phase::Connected)
    }

    /// Starts a new session with the given connection configuration.
    pub fn start(&mut self, config: ConnectConfig) {
        self.session = Some(Session::start(config));
    }

    /// Sends a command to the remote session if active.
    pub fn send(&self, command: Command) {
        if let Some(session) = &self.session {
            session.send(command);
        }
    }

    /// Polls pending engine events up to `EVENTS_PER_FRAME`.
    pub fn poll_events(&mut self) -> Vec<EngineEvent> {
        let Some(session) = &self.session else {
            return Vec::new();
        };
        let mut events = Vec::new();
        while events.len() < EVENTS_PER_FRAME {
            match session.try_event() {
                Some(event) => events.push(event),
                None => break,
            }
        }
        events
    }

    /// Disconnects and takes the session if running.
    pub fn disconnect(&mut self) -> Option<Session> {
        if let Some(mut session) = self.session.take() {
            session.disconnect();
            // What the engine said on its way out still belongs in the log.
            while let Some(event) = session.try_event() {
                if let EngineEvent::Progress(message) = event {
                    logging::event(message);
                }
            }
            Some(session)
        } else {
            None
        }
    }
}

/// Splits `pc.example.com:3390` into host and port; a bare host gets 3389.
#[must_use]
pub fn split_host_port(target: &str) -> (&str, u16) {
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
#[must_use]
pub fn split_domain(username: &str) -> (Option<&str>, &str) {
    match username.split_once('\\') {
        Some((domain, user)) if !domain.is_empty() => (Some(domain), user),
        _ => (None, username),
    }
}

/// The name this computer sends as the RDP client name. Windows truncates it
/// to 15 characters, so keep it to that.
#[must_use]
pub fn client_name() -> String {
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
#[must_use]
pub fn worth_retrying(error: &Error, reconnecting: bool) -> bool {
    match error {
        Error::ConnectionLost(_) => true,
        Error::Unreachable(_) | Error::Timeout(_) | Error::Protocol(_) | Error::Gateway(_) => {
            reconnecting
        }
        _ => false,
    }
}

/// The wait before each attempt: quick at first, then every five seconds.
#[must_use]
pub fn reconnect_delay(attempt: u32) -> Duration {
    Duration::from_secs(match attempt {
        0 | 1 => 1,
        2 => 2,
        3 => 4,
        _ => 5,
    })
}

/// What an engine failure means. The engine's own words carry the detail,
/// in English; in Spanish a lead-in says what happened.
#[must_use]
pub fn explain(error: &Error) -> String {
    let detail = error.to_string();
    if !crate::i18n::spanish() {
        return capitalize(&detail);
    }
    match error {
        Error::Unreachable(_) => format!("No se pudo llegar al PC: {detail}"),
        Error::Timeout(_) => format!("El PC tardó demasiado en responder: {detail}"),
        Error::Gateway(_) => format!("El RD Gateway no abrió el camino al PC: {detail}"),
        Error::ConnectionLost(_) => format!("Se perdió la conexión: {detail}"),
        Error::BrokerRedirect => {
            "El PC envió la sesión a otro servidor, y aún no se admiten los agentes de conexión."
                .to_string()
        }
        _ => format!("La conexión falló: {detail}"),
    }
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect::<String>() + "."
    })
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
        assert_eq!(split_domain("DOMAIN\\user"), (Some("DOMAIN"), "user"));
        assert_eq!(split_domain("user@domain"), (None, "user@domain"));
        assert_eq!(split_domain("user"), (None, "user"));
        assert_eq!(split_domain("\\user"), (None, "\\user"));
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
    fn only_network_failures_bring_the_session_back() {
        let dropped = Error::ConnectionLost("reset by peer".into());
        let refused = Error::Authentication("bad password".into());
        let unreachable = Error::Unreachable("no route".into());

        assert!(worth_retrying(&dropped, false));
        assert!(!worth_retrying(&refused, false));
        assert!(!worth_retrying(&refused, true));
        // A PC that was not reachable yet is worth another try only while
        // reconnecting.
        assert!(!worth_retrying(&unreachable, false));
        assert!(worth_retrying(&unreachable, true));
    }

    #[test]
    fn the_client_name_fits_rdp() {
        let name = client_name();
        assert!(!name.is_empty() && name.chars().count() <= 15);
    }
}
