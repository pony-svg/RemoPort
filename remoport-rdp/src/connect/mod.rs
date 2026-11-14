//! The connection sequence, in the order security needs: reach the PC,
//! negotiate (NLA only, unless allowed per PC), TLS, check the PC's identity,
//! and only then sign in with CredSSP and finish the RDP handshake.
//!
//! The order of the identity check and the sign-in follows RustConn's client
//! (`rustconn-core`, GPL-3.0-or-later); the checks themselves are Remoport's.

mod authenticate;
mod channels;
mod identity;
mod transport;

use std::sync::{Arc, mpsc};

use ironrdp_connector::ConnectionResult;
use ironrdp_svc::SvcMessage;

pub(crate) use channels::scale_factors;
pub(crate) use transport::Framed;

use crate::channels::clipboard::Clipboard;
use crate::channels::microphone;
use crate::{ConnectConfig, Error, Event};

/// A signed-in connection, ready for the session.
pub(crate) struct Established {
    pub framed: Framed,
    pub result: ConnectionResult,
    /// The session's side of the clipboard, when it is shared.
    pub clipboard: Option<Clipboard>,
    /// Microphone packets for the session to send, when the PC may use it.
    pub microphone: Option<(
        tokio::sync::mpsc::Receiver<Vec<SvcMessage>>,
        Arc<microphone::Delivery>,
    )>,
    /// Folders are shared, so their change notices need sending.
    pub drives: bool,
}

/// Runs the five-stage connection handshake in strict security order.
pub(crate) async fn connect(
    config: &mut ConnectConfig,
    events: &mpsc::Sender<Event>,
) -> Result<Established, Error> {
    // 1. Reach the PC, directly or through its RD Gateway.
    let (stream, client_addr) = transport::open(config, events).await?;

    // 2. The connector and its channels: display control (resizing), the
    // graphics pipeline, and the clipboard, sound, microphone and folders
    // when this PC has them.
    let (mut connector, registered) = channels::prepare(config, client_addr, events);

    // 3. Negotiate, then switch to TLS.
    let tls_channel = transport::negotiate_tls(stream, &mut connector, config, events).await?;

    // 4. The PC's identity, before any credential leaves this machine.
    identity::verify_server(config, &tls_channel.certificate, events).await?;

    // 5. Sign in with CredSSP and finish the handshake.
    let (framed, result) = authenticate::sign_in(tls_channel, connector, config, events).await?;

    Ok(Established {
        framed,
        result,
        clipboard: registered.clipboard,
        microphone: registered.microphone,
        drives: registered.drives,
    })
}
