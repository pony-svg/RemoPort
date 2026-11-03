//! The graphics pipeline (MS-RDPEGFX).
//!
//! `ironrdp-egfx` decodes and composites every codec a Windows host sends and
//! `ironrdp-session` paints the result into the session image, so the handler
//! only reports what happens: the negotiated mode, a size reset, and content
//! no decoder could handle.

use std::path::PathBuf;
use std::sync::mpsc;

use ironrdp_egfx::client::{BitmapUpdate, GraphicsPipelineHandler};
use ironrdp_egfx::decode::{H264Decoder, OpenH264Decoder};
use ironrdp_egfx::pdu::{
    CapabilitiesV103Flags, CapabilitiesV104Flags, CapabilitiesV107Flags, CapabilitiesV10Flags,
    CapabilitiesV81Flags, CapabilitySet, GfxPdu,
};

use crate::Event;

/// Undecodable surface updates before the user is told why the picture stalls.
const UNDECODABLE_THRESHOLD: u32 = 5;

pub(crate) struct Handler {
    events: mpsc::Sender<Event>,
    undecodable: u32,
}

impl Handler {
    pub(crate) const fn new(events: mpsc::Sender<Event>) -> Self {
        Self {
            events,
            undecodable: 0,
        }
    }
}

impl GraphicsPipelineHandler for Handler {
    fn on_capabilities_confirmed(&mut self, caps: &CapabilitySet) {
        let _ = self.events.send(Event::Graphics(mode_name(caps).to_string()));
    }

    fn on_reset_graphics(&mut self, width: u32, height: u32) {
        let _ = self.events.send(Event::Resized {
            width: u16::try_from(width).unwrap_or(u16::MAX),
            height: u16::try_from(height).unwrap_or(u16::MAX),
        });
    }

    fn on_bitmap_updated(&mut self, _update: &BitmapUpdate) {}

    fn on_unhandled_pdu(&mut self, pdu: &GfxPdu) {
        if let GfxPdu::WireToSurface1(wire) = pdu {
            self.undecodable = self.undecodable.saturating_add(1);
            if self.undecodable == UNDECODABLE_THRESHOLD {
                let _ = self.events.send(Event::Progress(format!(
                    "The PC sends graphics in a format Remoport cannot decode ({:?})",
                    wire.codec_id
                )));
            }
        }
    }
}

/// The name of the graphics mode a confirmed capability set represents.
fn mode_name(caps: &CapabilitySet) -> &'static str {
    let avc444 = match caps {
        CapabilitySet::V10 { flags } | CapabilitySet::V10_2 { flags } => {
            !flags.contains(CapabilitiesV10Flags::AVC_DISABLED)
        }
        CapabilitySet::V10_1 => true,
        CapabilitySet::V10_3 { flags } => !flags.contains(CapabilitiesV103Flags::AVC_DISABLED),
        CapabilitySet::V10_4 { flags }
        | CapabilitySet::V10_5 { flags }
        | CapabilitySet::V10_6 { flags } => !flags.contains(CapabilitiesV104Flags::AVC_DISABLED),
        CapabilitySet::V10_7 { flags } => !flags.contains(CapabilitiesV107Flags::AVC_DISABLED),
        _ => false,
    };
    match caps {
        _ if avc444 => "GFX + AVC444",
        CapabilitySet::V8_1 { flags } if flags.contains(CapabilitiesV81Flags::AVC420_ENABLED) => {
            "GFX + H.264"
        }
        _ => "GFX",
    }
}

/// An H.264 decoder, if an OpenH264 library is found that loads.
///
/// The loader accepts only Cisco's own published binaries: it checks the
/// file's SHA-256 against Cisco's list before running any of its code, so a
/// tampered or unknown library is refused, not loaded. Searched, in order:
/// `$REMOPORT_OPENH264`, `~/.local/share/remoport/openh264/`, the copy RustConn
/// downloads, and the system library directories.
pub(crate) fn h264_decoder() -> (Option<Box<dyn H264Decoder>>, String) {
    for path in openh264_candidates() {
        if let Ok(decoder) = OpenH264Decoder::from_library_path(&path) {
            return (Some(Box::new(decoder)), format!("H.264 decoder: {}", path.display()));
        }
    }
    (
        None,
        "No Cisco OpenH264 library found: the PC will use a non-H.264 codec".to_string(),
    )
}

fn openh264_candidates() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        dirs.push(home.join(".local/share/remoport/openh264"));
        dirs.push(home.join(".local/share/rustconn/openh264"));
    }
    for dir in ["/usr/lib", "/usr/lib64", "/usr/lib/x86_64-linux-gnu", "/usr/lib/aarch64-linux-gnu"] {
        dirs.push(PathBuf::from(dir));
    }
    let mut candidates: Vec<PathBuf> = std::env::var_os("REMOPORT_OPENH264")
        .map(PathBuf::from)
        .into_iter()
        .collect();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut found: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("libopenh264") && name.contains(".so"))
            })
            .collect();
        // Newest version first.
        found.sort();
        found.reverse();
        candidates.extend(found);
    }
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_are_named_from_the_confirmed_capabilities() {
        assert_eq!(
            mode_name(&CapabilitySet::V10_7 {
                flags: CapabilitiesV107Flags::SMALL_CACHE
            }),
            "GFX + AVC444"
        );
        assert_eq!(
            mode_name(&CapabilitySet::V10_7 {
                flags: CapabilitiesV107Flags::AVC_DISABLED
            }),
            "GFX"
        );
    }
}
