//! Server TLS certificate verification and TOFU prompt before any credential leaves this computer.

use std::sync::mpsc;
use std::time::Duration;

use tokio::time::timeout;

use crate::identity::{self, Verdict};
use crate::{ConnectConfig, Error, Event, TrustReply};

/// How long the user has to decide about a PC's identity.
const IDENTITY_DECISION: Duration = Duration::from_secs(300);

/// Stage 4: checks the PC's TLS certificate against system trust and `TrustStore`.
pub(crate) async fn verify_server(
    config: &ConnectConfig,
    certificate: &x509_cert::Certificate,
    events: &mpsc::Sender<Event>,
) -> Result<(), Error> {
    use x509_cert::der::Encode as _;

    let der = certificate
        .to_der()
        .map_err(|e| Error::Protocol(format!("the PC's certificate could not be read: {e}")))?;
    let chain = identity::chain_check(&der, &config.destination.host);
    let sha256 = identity::fingerprint(&der);
    let trusted = config
        .security
        .trust_store
        .trusted(&config.destination.host, config.destination.port);
    match identity::verdict(&chain, trusted.as_deref(), &sha256) {
        Verdict::Verified { issuer } => {
            let _ = events.send(Event::Log(format!(
                "Identity verified: certificate issued by {issuer}"
            )));
            Ok(())
        }
        Verdict::Remembered => {
            let _ = events.send(Event::Log(format!(
                "Identity: the certificate you trusted for this PC ({sha256})"
            )));
            Ok(())
        }
        Verdict::Ask(problem) => {
            let why = chain.err().unwrap_or_default();
            let info = identity::describe(
                certificate,
                &der,
                &config.destination.host,
                config.destination.port,
                why,
            );
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
                    if let Err(e) = config.security.trust_store.remember(&info) {
                        let _ = events.send(Event::Log(format!(
                            "The certificate could not be remembered, you will be asked again: {e}"
                        )));
                    }
                    let _ = events.send(Event::Log(format!(
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
