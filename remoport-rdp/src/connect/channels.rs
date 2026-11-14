//! IronRDP connector settings, monitor layout, resume cookie, and virtual channel registration.

use std::net::SocketAddr;
use std::sync::mpsc;

use ironrdp_connector::{BitmapConfig, ClientConnector, Config, Credentials, DesktopSize};
use ironrdp_displaycontrol::pdu::{DeviceScaleFactor, MonitorLayoutEntry};
use ironrdp_pdu::gcc::{self, KeyboardType};
use ironrdp_pdu::rdp::capability_sets::{
    BitmapCodecs, MajorPlatformType, client_codecs_capabilities,
};
use ironrdp_pdu::rdp::client_info::{PerformanceFlags, TimezoneInfo};
use ironrdp_pdu::rdp::session_info::ServerAutoReconnect;

use crate::channels::{self, Registered};
use crate::{ConnectConfig, Event, ResumeCookie, keyboard_layout};

/// Stage 2: builds the `ClientConnector` with display, keyboard, resumption,
/// and all configured virtual channels.
pub(crate) fn prepare(
    config: &mut ConnectConfig,
    client_addr: SocketAddr,
    events: &mpsc::Sender<Event>,
) -> (ClientConnector, Registered) {
    let (layout, layout_source) = keyboard_layout::detect_keyboard_layout();
    let _ = events.send(Event::Log(format!("Keyboard layout: {layout_source}")));
    let playing = config.sharing.sound.is_some();
    let capturing = config.sharing.microphone.is_some();
    let mut connector = with_resume(
        ClientConnector::new(
            connector_config(config, layout, playing, capturing),
            client_addr,
        ),
        config.credentials.resume.as_ref(),
    );
    if let Some(cookie) = &config.credentials.resume {
        let _ = events.send(Event::Log(format!(
            "Resuming session {} of this PC",
            cookie.logon_id
        )));
    }
    let registered = channels::register_all(&mut connector, config, events);
    (connector, registered)
}

/// The IronRDP connector settings (field by field as in RustConn's client).
///
/// SECURITY: `Credentials`, `Config` and the connector derive `Debug` without
/// redacting the password: never format them.
fn connector_config(
    config: &ConnectConfig,
    keyboard_layout: u32,
    sound: bool,
    microphone: bool,
) -> Config {
    let (desktop, monitor_layout) = screens(config);
    let (desktop_scale, device_scale) = scale_factors(config.display.scale);
    // With several screens, each one's scale goes with the list of screens.
    let monitor_extended_layout =
        monitor_layout
            .as_ref()
            .map(|layout| gcc::ClientMonitorExtendedData {
                extended_monitors_info: layout
                    .monitors
                    .iter()
                    .map(|_| gcc::ExtendedMonitorInfo {
                        physical_width: 0,
                        physical_height: 0,
                        orientation: gcc::MonitorOrientation::Landscape,
                        desktop_scale_factor: desktop_scale,
                        device_scale_factor: device_scale.value(),
                    })
                    .collect(),
            });
    let credentials = Credentials::UsernamePassword {
        username: config.credentials.username.clone(),
        // IronRDP needs an owned String; that copy is IronRDP's to keep.
        password: config.credentials.password.as_str().to_owned(),
    };
    let autologon =
        !config.credentials.username.is_empty() && !config.credentials.password.is_empty();
    Config {
        credentials,
        domain: config.credentials.domain.clone(),
        // Plain TLS (sign-in on the remote screen) only when allowed per PC.
        enable_tls: config.security.allow_without_nla,
        enable_credssp: true,
        keyboard_type: KeyboardType::IBM_ENHANCED,
        keyboard_subtype: 0,
        keyboard_functional_keys_count: 12,
        keyboard_layout,
        ime_file_name: String::new(),
        dig_product_id: String::new(),
        desktop_size: desktop,
        desktop_scale_factor: desktop_scale,
        monitor_layout,
        monitor_extended_layout,
        desktop_physical_size: None,
        desktop_orientation: None,
        device_scale_factor: Some(device_scale.value()),
        relative_mouse_input: false,
        connection_type: ironrdp_pdu::gcc::ConnectionType::Lan,
        // The original RDP security (RC4) is never offered.
        enable_standard_rdp_security: false,
        bitmap: Some(BitmapConfig {
            lossy_compression: true,
            color_depth: config.display.color_depth,
            codecs: client_codecs_capabilities(&[]).unwrap_or_else(|_| BitmapCodecs(vec![])),
        }),
        client_build: 0,
        client_name: config.destination.client_name.clone(),
        client_dir: String::new(),
        alternate_shell: String::new(),
        work_dir: String::new(),
        remote_application_mode: false,
        rail_support_level: ironrdp_pdu::rdp::capability_sets::RailSupportLevel::empty(),
        platform: MajorPlatformType::UNIX,
        hardware_id: None,
        request_data: None,
        autologon,
        enable_audio_playback: sound,
        enable_audio_capture: microphone,
        performance_flags: PerformanceFlags::default(),
        license_cache: None,
        timezone_info: timezone_info(),
        compression_type: Some(ironrdp_pdu::rdp::client_info::CompressionType::Rdp61),
        enable_server_pointer: true,
        pointer_software_rendering: false,
        multitransport_flags: None,
        // Advertising the pipeline early is what makes Windows open it at all.
        // Its pixels are always 32-bit, so with fewer it is not offered.
        support_dyn_vc_gfx_protocol: config.display.color_depth == 32,
    }
}

/// Presents the cookie of the session a reconnection resumes; without one the
/// PC signs in as usual, and so it does when it no longer takes the cookie.
fn with_resume(connector: ClientConnector, resume: Option<&ResumeCookie>) -> ClientConnector {
    let Some(cookie) = resume else {
        return connector;
    };
    // ponytail: IronRDP keeps the cookie as a plain copy that cannot be wiped
    // (its `Debug` does hide the bits); closing this needs a zeroizing field
    // in ironrdp-pdu upstream.
    connector.with_auto_reconnect_cookie(ServerAutoReconnect {
        logon_id: cookie.logon_id,
        random_bits: *cookie.random_bits,
    })
}

/// The two scale factors RDP sends for a scale: the one Windows displays at,
/// within what RDP allows, and the nearest of the three device factors.
pub(crate) fn scale_factors(percent: u32) -> (u32, DeviceScaleFactor) {
    let desktop = MonitorLayoutEntry::adjust_desktop_scale_factor(percent);
    (
        desktop,
        DeviceScaleFactor::from_desktop_scale_factor(desktop),
    )
}

/// The desktop size and, for several screens, where each lies: side by
/// side, left to right, the first one primary.
fn screens(config: &ConnectConfig) -> (DesktopSize, Option<gcc::ClientMonitorData>) {
    if config.display.monitors.len() < 2 {
        let desktop = DesktopSize {
            width: config.display.width,
            height: config.display.height,
        };
        return (desktop, None);
    }
    let mut left = 0i32;
    let monitors = config
        .display
        .monitors
        .iter()
        .enumerate()
        .map(|(index, &(width, height))| {
            let monitor = gcc::Monitor {
                left,
                top: 0,
                right: left + i32::from(width) - 1,
                bottom: i32::from(height) - 1,
                flags: if index == 0 {
                    gcc::MonitorFlags::PRIMARY
                } else {
                    gcc::MonitorFlags::empty()
                },
            };
            left += i32::from(width);
            monitor
        })
        .collect();
    let desktop = DesktopSize {
        width: config
            .display
            .monitors
            .iter()
            .map(|&(width, _)| width)
            .fold(0u16, u16::saturating_add),
        height: config
            .display
            .monitors
            .iter()
            .map(|&(_, height)| height)
            .max()
            .unwrap_or(0),
    };
    (desktop, Some(gcc::ClientMonitorData { monitors }))
}

fn timezone_info() -> TimezoneInfo {
    let offset = chrono::Local::now().offset().local_minus_utc();
    TimezoneInfo {
        bias: -(offset / 60),
        ..TimezoneInfo::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> ConnectConfig {
        ConnectConfig::new(
            "pc.example".to_string(),
            "jdoe".to_string(),
            zeroize::Zeroizing::new(String::new()),
            crate::TrustStore::new(std::env::temp_dir().join("unused.json")),
        )
    }

    #[test]
    fn several_screens_sit_side_by_side_the_first_primary() {
        let mut two = config();
        two.display.monitors = vec![(1920, 1080), (1280, 1024)];
        let (desktop, layout) = screens(&two);
        assert_eq!((desktop.width, desktop.height), (3200, 1080));
        let monitors = layout.expect("a layout").monitors;
        assert_eq!((monitors[0].left, monitors[0].right), (0, 1919));
        assert_eq!(
            (monitors[1].left, monitors[1].right, monitors[1].bottom),
            (1920, 3199, 1023)
        );
        assert!(monitors[0].flags.contains(gcc::MonitorFlags::PRIMARY));
        assert!(!monitors[1].flags.contains(gcc::MonitorFlags::PRIMARY));
        let (one, none) = screens(&config());
        assert_eq!((one.width, one.height), (1280, 720));
        assert!(none.is_none());
    }

    #[test]
    fn the_graphics_pipeline_is_offered_only_at_32_bit() {
        let depth = |settings: &Config| settings.bitmap.as_ref().map(|b| b.color_depth);
        let mut low = config();
        let settings = connector_config(&low, 0x409, false, false);
        assert_eq!(depth(&settings), Some(32));
        assert!(settings.support_dyn_vc_gfx_protocol);

        low.display.color_depth = 16;
        let settings = connector_config(&low, 0x409, false, false);
        assert_eq!(depth(&settings), Some(16));
        assert!(!settings.support_dyn_vc_gfx_protocol);
    }

    #[test]
    fn the_pc_is_told_the_scale_when_connecting() {
        let mut scaled = config();
        for (percent, desktop, device) in [
            (100, 100, 100),
            (125, 125, 100),
            (150, 150, 140),
            (200, 200, 180),
            (50, 100, 100),
            (900, 500, 180),
        ] {
            scaled.display.scale = percent;
            let settings = connector_config(&scaled, 0x409, false, false);
            assert_eq!(settings.desktop_scale_factor, desktop);
            assert_eq!(settings.device_scale_factor, Some(device));
            assert!(settings.monitor_extended_layout.is_none());
        }

        scaled.display.scale = 150;
        scaled.display.monitors = vec![(1920, 1080), (1280, 1024)];
        let settings = connector_config(&scaled, 0x409, false, false);
        let each = settings
            .monitor_extended_layout
            .expect("each screen's scale")
            .extended_monitors_info;
        assert_eq!(each.len(), 2);
        assert!(
            each.iter()
                .all(|m| m.desktop_scale_factor == 150 && m.device_scale_factor == 140)
        );
    }

    #[test]
    fn only_a_reconnection_presents_the_cookie_and_never_prints_it() {
        let fresh = || {
            ClientConnector::new(
                connector_config(&config(), 0x409, false, false),
                std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
            )
        };
        assert!(with_resume(fresh(), None).auto_reconnect_cookie.is_none());

        let cookie = ResumeCookie::new(7, [0xA5; 16]);
        let connector = with_resume(fresh(), Some(&cookie));
        let presented = connector.auto_reconnect_cookie.as_ref().expect("presented");
        assert_eq!((presented.logon_id, presented.random_bits), (7, [0xA5; 16]));
        // 0xA5 is 165: the bits appear in neither Debug.
        for debug in [format!("{cookie:?}"), format!("{presented:?}")] {
            assert!(!debug.contains("165"), "random bits printed: {debug}");
        }
        assert!(format!("{cookie:?}").contains("logon_id: 7"));
    }
}
