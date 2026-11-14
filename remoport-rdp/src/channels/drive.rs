//! Device redirection (`RDPDR`) registration for shared local folders.

use std::sync::mpsc;

use ironrdp_connector::ClientConnector;

use crate::{Event, SharedFolder, drive};

/// Registers `RDPDR` when sharing folders or keeping audio enabled, returning
/// the count of announced local drives.
pub(crate) fn register(
    connector: &mut ClientConnector,
    folders: &[SharedFolder],
    client_name: &str,
    force_open: bool,
    events: &mpsc::Sender<Event>,
) -> usize {
    // The device channel carries the shared folders, and Windows sends sound
    // only alongside it; without folders it announces no device.
    if !force_open && folders.is_empty() {
        return 0;
    }
    let (backend, announced) = drive::DriveBackend::new(folders, events.clone());
    let count = announced.len();
    let mut rdpdr = ironrdp_rdpdr::Rdpdr::new(Box::new(backend), client_name.to_string());
    if !announced.is_empty() {
        rdpdr = rdpdr.with_drives(Some(announced));
    }
    connector.static_channels.insert(rdpdr);
    count
}
