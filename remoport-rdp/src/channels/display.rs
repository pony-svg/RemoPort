//! Dynamic Display Control channel for resizing and monitor layout changes.

use std::sync::mpsc;

use ironrdp_displaycontrol::client::DisplayControlClient;
use ironrdp_dvc::DrdynvcClient;

use crate::Event;

/// Attaches the Display Control dynamic virtual channel to `drdynvc`.
pub(crate) fn register(drdynvc: DrdynvcClient, events: mpsc::Sender<Event>) -> DrdynvcClient {
    let display_control = DisplayControlClient::new(move |_caps| {
        let _ = events.send(Event::DisplayControlReady);
        Ok(Vec::new())
    });
    drdynvc.with_dynamic_channel(display_control)
}
