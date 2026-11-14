//! Virtual channels registered on an outgoing RDP connection.

pub(crate) mod clipboard;
pub(crate) mod display;
pub(crate) mod drive;
pub(crate) mod gfx;
pub(crate) mod microphone;
pub(crate) mod sound;

use std::sync::{Arc, mpsc};

use ironrdp_connector::ClientConnector;
use ironrdp_dvc::DrdynvcClient;
use ironrdp_svc::SvcMessage;

use crate::{ConnectConfig, Event};

/// Handles produced during channel registration for use in the active session.
pub(crate) struct Registered {
    /// The session's side of the clipboard, when shared.
    pub clipboard: Option<clipboard::Clipboard>,
    /// Microphone packets for the session to send, when the PC may use it.
    pub microphone: Option<(
        tokio::sync::mpsc::Receiver<Vec<SvcMessage>>,
        Arc<microphone::Delivery>,
    )>,
    /// Folders are shared, so their change notices need sending.
    pub drives: bool,
}

/// Registers every configured static and dynamic channel onto `connector`.
pub(crate) fn register_all(
    connector: &mut ClientConnector,
    config: &mut ConnectConfig,
    events: &mpsc::Sender<Event>,
) -> Registered {
    let sound_out = config.sharing.sound.take();
    let mic_in = config.sharing.microphone.take();
    let has_mic = mic_in.is_some();

    let clipboard = clipboard::register(connector, config.sharing.clipboard);
    let playing = sound::register(connector, sound_out, events);
    let drives = drive::register(
        connector,
        &config.sharing.folders,
        &config.destination.client_name,
        playing,
        events,
    );
    let _ = events.send(Event::Log(format!(
        "Clipboard {}, sound {}, microphone {}, {drives} shared folder{}",
        if clipboard.is_some() {
            "shared (text)"
        } else {
            "not shared"
        },
        if playing { "played here" } else { "off" },
        if has_mic { "allowed" } else { "off" },
        if drives == 1 { "" } else { "s" },
    )));

    let mut drdynvc = display::register(DrdynvcClient::new(), events.clone());
    drdynvc = gfx::register(
        drdynvc,
        config.display.color_depth,
        connector.config.support_dyn_vc_gfx_protocol,
        events,
    );
    let microphone = mic_in.map(|input| {
        let (with_microphone, packets, delivery) =
            microphone::register(std::mem::take(&mut drdynvc), input, events.clone());
        drdynvc = with_microphone;
        (packets, delivery)
    });
    connector.static_channels.insert(drdynvc);

    Registered {
        clipboard,
        microphone,
        drives: drives > 0,
    }
}
