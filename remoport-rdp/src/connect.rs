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
use ironrdp_pdu::gcc::KeyboardType;
use ironrdp_pdu::rdp::capability_sets::{
    BitmapCodecs, MajorPlatformType, client_codecs_capabilities,
};
use ironrdp_pdu::rdp::client_info::{PerformanceFlags, TimezoneInfo};
use ironrdp_tokio::TokioFramed;
use ironrdp_tokio::reqwest::ReqwestNetworkClient;
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::error::{is_authentication_failure, is_nla_refusal};
use crate::identity::{self, Verdict};
use crate::{ConnectConfig, Error, Event, TrustReply, gfx, keyboard_layout, net};

/// How long the user has to decide about a PC's identity.
const IDENTITY_DECISION: Duration = Duration::from_secs(300);

pub(crate) type Framed = TokioFramed<ironrdp_tls::TlsStream<TcpStream>>;

/// A signed-in connection, ready for the session.
pub(crate) struct Established {
    pub framed: Framed,
    pub result: ConnectionResult,
}

pub(crate) async fn connect(
    config: &ConnectConfig,
    events: &mpsc::Sender<Event>,
) -> Result<Established, Error> {
    let progress = |message: String| {
        let _ = events.send(Event::Progress(message));
    };

    // 1. Reach the PC.
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

    // 2. The connector and its channels: display control (resizing) and the
    // graphics pipeline. Clipboard, sound and devices are not offered.
    let (layout, layout_source) = keyboard_layout::detect_keyboard_layout();
    progress(format!("Keyboard layout: {layout_source}"));
    let mut connector = ClientConnector::new(connector_config(config, layout), client_addr);
    let ready = events.clone();
    let display_control = ironrdp_displaycontrol::client::DisplayControlClient::new(move |_caps| {
        let _ = ready.send(Event::DisplayControlReady);
        Ok(Vec::new())
    });
    let (decoder, decoder_note) = gfx::h264_decoder();
    progress(decoder_note);
    let graphics =
        GraphicsPipelineClient::new(Box::new(gfx::Handler::new(events.clone())), decoder);
    let drdynvc = DrdynvcClient::new()
        .with_dynamic_channel(display_control)
        .with_dynamic_channel(graphics);
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
    progress("Signing in (Network Level Authentication)".to_string());
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
        None,
    ));
    let outcome = timeout(config.timeout, futures::FutureExt::catch_unwind(finalize))
        .await
        .map_err(|_| Error::Timeout("signing in"))?;
    match outcome {
        Ok(Ok(ConnectionOutcome::Connected(result))) => Ok(Established { framed, result }),
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
fn connector_config(config: &ConnectConfig, keyboard_layout: u32) -> Config {
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
        desktop_size: DesktopSize {
            width: config.width,
            height: config.height,
        },
        desktop_scale_factor: 100,
        monitor_layout: None,
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
        enable_audio_playback: false,
        enable_audio_capture: false,
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

fn timezone_info() -> TimezoneInfo {
    let offset = chrono::Local::now().offset().local_minus_utc();
    TimezoneInfo {
        bias: -(offset / 60),
        ..TimezoneInfo::default()
    }
}
