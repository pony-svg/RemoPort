//! CredSSP authentication (Kerberos / NTLM) and handshake completion.

use std::sync::mpsc;

use ironrdp_connector::{ClientConnector, ConnectionOutcome, ConnectionResult, ServerName};
use ironrdp_tokio::TokioFramed;
use ironrdp_tokio::reqwest::ReqwestNetworkClient;
use tokio::time::timeout;

use super::transport::{Framed, TlsChannel};
use crate::error::is_authentication_failure;
use crate::{ConnectConfig, Error, Event, kerberos};

/// Stage 5: authenticates via CredSSP and finishes the RDP handshake.
pub(crate) async fn sign_in(
    channel: TlsChannel,
    mut connector: ClientConnector,
    config: &ConnectConfig,
    events: &mpsc::Sender<Event>,
) -> Result<(Framed, ConnectionResult), Error> {
    let kerberos = config.credentials.kerberos.as_ref().and_then(|settings| {
        for note in kerberos::preflight(config) {
            let _ = events.send(Event::Log(note));
        }
        kerberos::settings(config, settings.kdc.as_deref())
    });
    let _ = events.send(Event::Log(format!(
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
    )));
    let server_public_key = ironrdp_tls::extract_tls_server_public_key(&channel.certificate)
        .map(<[u8]>::to_vec)
        .unwrap_or_default();
    let upgraded = ironrdp_tokio::mark_as_upgraded(channel.should_upgrade, &mut connector);
    let mut framed = TokioFramed::new(channel.tls);
    let mut network = ReqwestNetworkClient::new();
    // IronRDP can still panic on some unexpected server answers; that ends
    // this connection, not Remoport.
    let finalize = std::panic::AssertUnwindSafe(ironrdp_tokio::connect_finalize(
        upgraded,
        connector,
        &mut framed,
        &mut network,
        ServerName::new(&config.destination.host),
        server_public_key,
        kerberos,
    ));
    let outcome = timeout(
        config.destination.timeout,
        futures::FutureExt::catch_unwind(finalize),
    )
    .await
    .map_err(|_| Error::Timeout("signing in"))?;
    match outcome {
        Ok(Ok(ConnectionOutcome::Connected(result))) => Ok((framed, result)),
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
