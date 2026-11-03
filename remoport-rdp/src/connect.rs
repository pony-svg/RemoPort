//! The connection sequence, in the order security needs: reach the PC,
//! negotiate (NLA only, unless allowed per PC), TLS, check the PC's identity,
//! and only then sign in with CredSSP and finish the RDP handshake.
//!
//! The order of the identity check and the sign-in follows RustConn's client
//! (`rustconn-core`, GPL-3.0-or-later); the checks themselves are Remoport's.

use std::sync::mpsc;
use std::time::Duration;

use ironrdp_connector::{
    BitmapConfig, ClientConnector, Config, ConnectionOutcome, ConnectionResult, ConnectorError,
    Credentials, DesktopSize, ServerName,
};
use ironrdp_dvc::DrdynvcClient;
use ironrdp_egfx::client::GraphicsPipelineClient;
use ironrdp_pdu::gcc::{self, KeyboardType};
use ironrdp_pdu::rdp::capability_sets::{
    BitmapCodecs, MajorPlatformType, client_codecs_capabilities,
};
use ironrdp_pdu::rdp::client_info::{PerformanceFlags, TimezoneInfo};
use ironrdp_pdu::rdp::session_info::ServerAutoReconnect;
use ironrdp_svc::SvcMessage;
use ironrdp_tokio::TokioFramed;
use ironrdp_tokio::reqwest::ReqwestNetworkClient;
use tokio::time::timeout;

use crate::clipboard::{self, Clipboard};
use crate::error::{is_authentication_failure, is_nla_refusal};
use crate::identity::{self, Verdict};
use crate::{
    ConnectConfig, Error, Event, ResumeCookie, TrustReply, drive, gfx, kerberos, keyboard_layout,
    microphone, net, sound,
};

/// Microphone packets waiting for the session; beyond this, sound is dropped
/// rather than piling up behind a slow link.
const MICROPHONE_QUEUE: usize = 64;

/// How long the user has to decide about a PC's identity.
const IDENTITY_DECISION: Duration = Duration::from_secs(300);

/// A direct connection or a gateway tunnel, as the rest of the sequence sees it.
pub(crate) trait Transport:
    tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + Sync
{
}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + Sync> Transport for T {}

pub(crate) type Framed = TokioFramed<ironrdp_tls::TlsStream<Box<dyn Transport>>>;

/// A signed-in connection, ready for the session.
pub(crate) struct Established {
    pub framed: Framed,
    pub result: ConnectionResult,
    /// The session's side of the clipboard, when it is shared.
    pub clipboard: Option<Clipboard>,
    /// Microphone packets for the session to send, when the PC may use it.
    pub microphone: Option<tokio::sync::mpsc::Receiver<Vec<SvcMessage>>>,
}

pub(crate) async fn connect(
    config: &mut ConnectConfig,
    events: &mpsc::Sender<Event>,
) -> Result<Established, Error> {
    let progress = |message: String| {
        let _ = events.send(Event::Progress(message));
    };

    // 1. Reach the PC, directly or through its RD Gateway.
    let (stream, client_addr): (Box<dyn Transport>, _) = match config.gateway.as_deref() {
        Some(gateway) => {
            progress(format!("Opening a tunnel through the RD Gateway {gateway}"));
            let (tunnel, client_addr) =
                timeout(config.timeout, crate::gateway::open(gateway, config))
                    .await
                    .map_err(|_| Error::Timeout("reaching the RD Gateway"))??;
            progress(format!(
                "Reached {} through {gateway} (its certificate verified)",
                config.host
            ));
            (Box::new(tunnel), client_addr)
        }
        None => {
            let stream = timeout(config.timeout, net::connect_tcp(&config.host, config.port))
                .await
                .map_err(|_| Error::Timeout("reaching the PC"))?
                .map_err(|e| {
                    Error::Unreachable(format!(
                        "{}:{} could not be reached: {e}",
                        config.host, config.port
                    ))
                })?;
            net::tune(&stream);
            let client_addr = stream
                .local_addr()
                .map_err(|e| Error::Unreachable(e.to_string()))?;
            progress(format!(
                "Reached {} at {}",
                config.host,
                stream
                    .peer_addr()
                    .map_or_else(|_| "?".to_string(), |a| a.to_string())
            ));
            (Box::new(stream), client_addr)
        }
    };

    // 2. The connector and its channels: display control (resizing), the
    // graphics pipeline, and the clipboard, sound, microphone and folders
    // when this PC has them.
    let (layout, layout_source) = keyboard_layout::detect_keyboard_layout();
    progress(format!("Keyboard layout: {layout_source}"));
    let sound = config.sound.take();
    let input = config.microphone.take();
    let playing = sound.is_some();
    let mut connector = with_resume(
        ClientConnector::new(
            connector_config(config, layout, playing, input.is_some()),
            client_addr,
        ),
        config.resume.as_ref(),
    );
    if let Some(cookie) = &config.resume {
        progress(format!("Resuming session {} of this PC", cookie.logon_id));
    }
    let clipboard = config.clipboard.then(|| {
        let (channel, clipboard) = clipboard::channel();
        connector.static_channels.insert(channel);
        clipboard
    });
    if let Some(output) = sound {
        connector
            .static_channels
            .insert(ironrdp_rdpsnd::client::Rdpsnd::new(Box::new(
                sound::Handler::new(output),
            )));
    }
    // The device channel carries the shared folders, and Windows sends sound
    // only alongside it; without folders it announces no device.
    let mut drives = 0;
    if playing || !config.folders.is_empty() {
        let (backend, announced) = drive::DriveBackend::new(&config.folders, events.clone());
        drives = announced.len();
        let mut rdpdr = ironrdp_rdpdr::Rdpdr::new(Box::new(backend), config.client_name.clone());
        if !announced.is_empty() {
            rdpdr = rdpdr.with_drives(Some(announced));
        }
        connector.static_channels.insert(rdpdr);
    }
    progress(format!(
        "Clipboard {}, sound {}, microphone {}, {drives} shared folder{}",
        if clipboard.is_some() {
            "shared (text)"
        } else {
            "not shared"
        },
        if playing { "played here" } else { "off" },
        if input.is_some() { "allowed" } else { "off" },
        if drives == 1 { "" } else { "s" },
    ));
    let ready = events.clone();
    let display_control = ironrdp_displaycontrol::client::DisplayControlClient::new(move |_caps| {
        let _ = ready.send(Event::DisplayControlReady);
        Ok(Vec::new())
    });
    let (decoder, decoder_note) = gfx::h264_decoder();
    progress(decoder_note);
    let graphics =
        GraphicsPipelineClient::new(Box::new(gfx::Handler::new(events.clone())), decoder);
    let mut drdynvc = DrdynvcClient::new()
        .with_dynamic_channel(display_control)
        .with_dynamic_channel(graphics);
    // Microphone packets are sent between the PC's turns, so the channel
    // queues them for the session.
    let microphone = input.map(|input| {
        let (queue, packets) = tokio::sync::mpsc::channel(MICROPHONE_QUEUE);
        let uplink: ironrdp_rdpeai::client::DvcUplink = Box::new(move |_channel, messages| {
            // A full queue drops this packet: a gap, not a growing delay.
            match queue.try_send(messages) {
                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                    Err(ironrdp_pdu::pdu_other_err!("the session has ended"))
                }
                _ => Ok(()),
            }
        });
        drdynvc = std::mem::take(&mut drdynvc).with_dynamic_channel(
            ironrdp_rdpeai::client::RdpeaiClient::new(
                Box::new(microphone::Handler::new(input, events.clone())),
                uplink,
            ),
        );
        packets
    });
    // Advertising the pipeline early is what makes Windows open it at all.
    connector.config.support_dyn_vc_gfx_protocol = true;
    connector.static_channels.insert(drdynvc);

    // 3. Negotiate, then switch to TLS.
    let mut framed = TokioFramed::new(stream);
    let should_upgrade = timeout(
        config.timeout,
        ironrdp_tokio::connect_begin(&mut framed, &mut connector),
    )
    .await
    .map_err(|_| Error::Timeout("negotiating"))?
    .map_err(|e| negotiation_error(&e, config.allow_without_nla))?;
    let (tls, certificate) = timeout(
        config.timeout,
        ironrdp_tls::upgrade(framed.into_inner_no_leftover(), &config.host),
    )
    .await
    .map_err(|_| Error::Timeout("setting up the secure channel"))?
    .map_err(|e| Error::Protocol(format!("the secure channel could not be set up: {e}")))?;
    progress("Secure channel up (TLS)".to_string());

    // 4. The PC's identity, before any credential leaves this machine.
    check_identity(config, &certificate, events).await?;

    // 5. Sign in with CredSSP and finish the handshake.
    let kerberos = config.kerberos.as_ref().and_then(|settings| {
        for note in kerberos::preflight(config) {
            progress(note);
        }
        kerberos::settings(config, settings.kdc.as_deref())
    });
    progress(format!(
        "Signing in (Network Level Authentication, {})",
        match &kerberos {
            Some(settings) => format!(
                "Kerberos, KDC {}",
                settings
                    .kdc_proxy_url
                    .as_ref()
                    .map_or_else(|| "from krb5.conf".to_string(), ToString::to_string)
            ),
            None => "NTLM".to_string(),
        }
    ));
    let server_public_key = ironrdp_tls::extract_tls_server_public_key(&certificate)
        .map(<[u8]>::to_vec)
        .unwrap_or_default();
    let upgraded = ironrdp_tokio::mark_as_upgraded(should_upgrade, &mut connector);
    let mut framed = TokioFramed::new(tls);
    let mut network = ReqwestNetworkClient::new();
    // IronRDP can still panic on some unexpected server answers; that ends
    // this connection, not Remoport.
    let finalize = std::panic::AssertUnwindSafe(ironrdp_tokio::connect_finalize(
        upgraded,
        connector,
        &mut framed,
        &mut network,
        ServerName::new(&config.host),
        server_public_key,
        kerberos,
    ));
    let outcome = timeout(config.timeout, futures::FutureExt::catch_unwind(finalize))
        .await
        .map_err(|_| Error::Timeout("signing in"))?;
    match outcome {
        Ok(Ok(ConnectionOutcome::Connected(result))) => Ok(Established {
            framed,
            result,
            clipboard,
            microphone,
        }),
        Ok(Ok(ConnectionOutcome::Redirected(_))) => Err(Error::BrokerRedirect),
        Ok(Err(e)) => {
            let detail = format!("{e} [{:?}]", e.kind());
            if is_authentication_failure(&detail) {
                Err(Error::Authentication(e.to_string()))
            } else {
                Err(Error::Protocol(format!("the handshake failed: {detail}")))
            }
        }
        Err(_) => Err(Error::Protocol(
            "the RDP library stopped on an unexpected answer from the PC".to_string(),
        )),
    }
}

async fn check_identity(
    config: &ConnectConfig,
    certificate: &x509_cert::Certificate,
    events: &mpsc::Sender<Event>,
) -> Result<(), Error> {
    use x509_cert::der::Encode as _;

    let der = certificate
        .to_der()
        .map_err(|e| Error::Protocol(format!("the PC's certificate could not be read: {e}")))?;
    let chain = identity::chain_check(&der, &config.host);
    let sha256 = identity::fingerprint(&der);
    let trusted = config.trust_store.trusted(&config.host, config.port);
    match identity::verdict(&chain, trusted.as_deref(), &sha256) {
        Verdict::Verified { issuer } => {
            let _ = events.send(Event::Progress(format!(
                "Identity verified: certificate issued by {issuer}"
            )));
            Ok(())
        }
        Verdict::Remembered => {
            let _ = events.send(Event::Progress(format!(
                "Identity: the certificate you trusted for this PC ({sha256})"
            )));
            Ok(())
        }
        Verdict::Ask(problem) => {
            let why = chain.err().unwrap_or_default();
            let info = identity::describe(certificate, &der, &config.host, config.port, why);
            let (reply, answer) = tokio::sync::oneshot::channel();
            events
                .send(Event::VerifyIdentity {
                    info: info.clone(),
                    problem,
                    reply: TrustReply(reply),
                })
                .map_err(|_| Error::IdentityRejected)?;
            match timeout(IDENTITY_DECISION, answer).await {
                Ok(Ok(true)) => {
                    if let Err(e) = config.trust_store.remember(&info) {
                        let _ = events.send(Event::Progress(format!(
                            "The certificate could not be remembered, you will be asked again: {e}"
                        )));
                    }
                    let _ = events.send(Event::Progress(format!(
                        "You trusted this PC's certificate ({})",
                        info.sha256
                    )));
                    Ok(())
                }
                _ => Err(Error::IdentityRejected),
            }
        }
    }
}

fn negotiation_error(error: &ConnectorError, allow_without_nla: bool) -> Error {
    let detail = format!("{error} [{:?}]", error.kind());
    if !allow_without_nla && is_nla_refusal(&detail) {
        Error::NlaUnavailable
    } else {
        Error::Protocol(format!("the PC refused the connection: {detail}"))
    }
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
    let credentials = Credentials::UsernamePassword {
        username: config.username.clone(),
        // IronRDP needs an owned String; that copy is IronRDP's to keep.
        password: config.password.as_str().to_owned(),
    };
    let autologon = !config.username.is_empty() && !config.password.is_empty();
    Config {
        credentials,
        domain: config.domain.clone(),
        // Plain TLS (sign-in on the remote screen) only when allowed per PC.
        enable_tls: config.allow_without_nla,
        enable_credssp: true,
        keyboard_type: KeyboardType::IBM_ENHANCED,
        keyboard_subtype: 0,
        keyboard_functional_keys_count: 12,
        keyboard_layout,
        ime_file_name: String::new(),
        dig_product_id: String::new(),
        desktop_size: desktop,
        desktop_scale_factor: 100,
        monitor_layout,
        monitor_extended_layout: None,
        desktop_physical_size: None,
        desktop_orientation: None,
        device_scale_factor: None,
        relative_mouse_input: false,
        connection_type: ironrdp_pdu::gcc::ConnectionType::Lan,
        // The original RDP security (RC4) is never offered.
        enable_standard_rdp_security: false,
        bitmap: Some(BitmapConfig {
            lossy_compression: true,
            color_depth: 32,
            codecs: client_codecs_capabilities(&[]).unwrap_or_else(|_| BitmapCodecs(vec![])),
        }),
        client_build: 0,
        client_name: config.client_name.clone(),
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
        support_dyn_vc_gfx_protocol: false,
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

/// The desktop size and, for several screens, where each lies: side by
/// side, left to right, the first one primary.
fn screens(config: &ConnectConfig) -> (DesktopSize, Option<gcc::ClientMonitorData>) {
    if config.monitors.len() < 2 {
        let desktop = DesktopSize {
            width: config.width,
            height: config.height,
        };
        return (desktop, None);
    }
    let mut left = 0i32;
    let monitors = config
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
            .monitors
            .iter()
            .map(|&(width, _)| width)
            .fold(0u16, u16::saturating_add),
        height: config
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
        ConnectConfig {
            host: "pc.example".to_string(),
            port: 3389,
            username: "jdoe".to_string(),
            domain: None,
            password: zeroize::Zeroizing::new(String::new()),
            width: 1280,
            height: 720,
            monitors: Vec::new(),
            allow_without_nla: false,
            trust_store: crate::TrustStore::new(std::env::temp_dir().join("unused.json")),
            timeout: Duration::from_secs(1),
            client_name: "test".to_string(),
            clipboard: false,
            sound: None,
            microphone: None,
            folders: Vec::new(),
            gateway: None,
            kerberos: None,
            resume: None,
        }
    }

    #[test]
    fn several_screens_sit_side_by_side_the_first_primary() {
        let mut two = config();
        two.monitors = vec![(1920, 1080), (1280, 1024)];
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
