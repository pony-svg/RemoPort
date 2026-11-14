//! Transport opening (direct TCP or RD Gateway) and TLS negotiation.

use std::net::SocketAddr;
use std::sync::mpsc;

use ironrdp_connector::{ClientConnector, ConnectorError};
use ironrdp_tokio::{ShouldUpgrade, TokioFramed};
use tokio::time::timeout;

use crate::error::is_nla_refusal;
use crate::{ConnectConfig, Error, Event, net};

/// A direct connection or a gateway tunnel, as the rest of the sequence sees it.
pub(crate) trait Transport:
    tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + Sync
{
}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + Sync> Transport for T {}

pub(crate) type Framed = TokioFramed<ironrdp_tls::TlsStream<Box<dyn Transport>>>;

/// An active TLS stream and server certificate awaiting identity verification
/// before CredSSP authentication starts.
pub(crate) struct TlsChannel {
    pub tls: ironrdp_tls::TlsStream<Box<dyn Transport>>,
    pub certificate: x509_cert::Certificate,
    pub should_upgrade: ShouldUpgrade,
}

/// Stage 1: reaches the PC directly over TCP or through its RD Gateway.
pub(crate) async fn open(
    config: &ConnectConfig,
    events: &mpsc::Sender<Event>,
) -> Result<(Box<dyn Transport>, SocketAddr), Error> {
    match &config.destination.gateway {
        Some(gateway) => {
            let _ = events.send(Event::Log(format!(
                "Opening a tunnel through the RD Gateway {}{}",
                gateway.address,
                if gateway.account.is_some() {
                    ", with its own account"
                } else {
                    ""
                }
            )));
            let (tunnel, client_addr) = timeout(
                config.destination.timeout,
                crate::gateway::open(gateway, config),
            )
            .await
            .map_err(|_| Error::Timeout("reaching the RD Gateway"))??;
            let _ = events.send(Event::Log(format!(
                "Reached {} through {} (its certificate verified)",
                config.destination.host, gateway.address
            )));
            Ok((Box::new(tunnel), client_addr))
        }
        None => {
            let stream = timeout(
                config.destination.timeout,
                net::connect_tcp(&config.destination.host, config.destination.port),
            )
            .await
            .map_err(|_| Error::Timeout("reaching the PC"))?
            .map_err(|e| {
                Error::Unreachable(format!(
                    "{}:{} could not be reached: {e}",
                    config.destination.host, config.destination.port
                ))
            })?;
            net::tune(&stream);
            let client_addr = stream
                .local_addr()
                .map_err(|e| Error::Unreachable(e.to_string()))?;
            let _ = events.send(Event::Log(format!(
                "Reached {} at {}",
                config.destination.host,
                stream
                    .peer_addr()
                    .map_or_else(|_| "?".to_string(), |a| a.to_string())
            )));
            Ok((Box::new(stream), client_addr))
        }
    }
}

/// Stage 3: runs X.224 security negotiation and upgrades the transport to TLS.
pub(crate) async fn negotiate_tls(
    stream: Box<dyn Transport>,
    connector: &mut ClientConnector,
    config: &ConnectConfig,
    events: &mpsc::Sender<Event>,
) -> Result<TlsChannel, Error> {
    let mut framed = TokioFramed::new(stream);
    let should_upgrade = timeout(
        config.destination.timeout,
        ironrdp_tokio::connect_begin(&mut framed, connector),
    )
    .await
    .map_err(|_| Error::Timeout("negotiating"))?
    .map_err(|e| negotiation_error(&e, config.security.allow_without_nla))?;
    let (tls, certificate) = timeout(
        config.destination.timeout,
        ironrdp_tls::upgrade(framed.into_inner_no_leftover(), &config.destination.host),
    )
    .await
    .map_err(|_| Error::Timeout("setting up the secure channel"))?
    .map_err(|e| Error::Protocol(format!("the secure channel could not be set up: {e}")))?;
    let _ = events.send(Event::Log("Secure channel up (TLS)".to_string()));
    Ok(TlsChannel {
        tls,
        certificate,
        should_upgrade,
    })
}

fn negotiation_error(error: &ConnectorError, allow_without_nla: bool) -> Error {
    let detail = format!("{error} [{:?}]", error.kind());
    if !allow_without_nla && is_nla_refusal(&detail) {
        Error::NlaUnavailable
    } else {
        Error::Protocol(format!("the PC refused the connection: {detail}"))
    }
}
