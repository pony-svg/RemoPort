//! Reaching a PC through an RD Gateway (MS-TSGU over HTTPS).
//!
//! The gateway sees the user's credentials, so its certificate must chain to
//! an authority this computer trusts and match its name. IronRDP accepts any
//! certificate by default; here nothing is asked or remembered, and a gateway
//! that does not pass is refused before any credential is sent. The PC's own
//! identity is still checked, end to end, inside the tunnel.

use std::net::SocketAddr;

use ironrdp_mstsgu::{GwClient, GwConnectTarget, GwErrorKind};
use ironrdp_tls::CertificateValidation;

use crate::{ConnectConfig, Error, Gateway};

/// The gateway's target, whose password copy is wiped however the
/// connection ends.
struct Target(GwConnectTarget);

impl Drop for Target {
    fn drop(&mut self) {
        use zeroize::Zeroize as _;
        self.0.gw_pass.zeroize();
    }
}

/// Opens a tunnel through `gateway` to the PC in `config`, signing in to the
/// gateway with its own account or else the PC's.
pub(crate) async fn open(
    gateway: &Gateway,
    config: &ConnectConfig,
) -> Result<(GwClient, SocketAddr), Error> {
    let (user, password) = account(gateway, config);
    let target = Target(GwConnectTarget {
        gw_endpoint: endpoint(&gateway.address),
        gw_user: user,
        // IronRDP needs an owned String; `Target` wipes it.
        gw_pass: password.to_owned(),
        smart_card: None,
        server: config.host.clone(),
    });
    GwClient::connect_with_port_and_certificate_validation(
        &target.0,
        &config.client_name,
        config.port,
        CertificateValidation::Strict,
        None,
    )
    .await
    .map_err(|e| failure(&gateway.address, e.kind(), &e.report().to_string()))
}

/// Who signs in to the gateway: its own account, else the PC's.
fn account<'a>(gateway: &'a Gateway, config: &'a ConnectConfig) -> (String, &'a str) {
    match &gateway.account {
        Some((user, password)) => (user.trim().to_string(), password.as_str()),
        None => (
            match &config.domain {
                Some(domain) if !domain.is_empty() => format!("{domain}\\{}", config.username),
                _ => config.username.clone(),
            },
            config.password.as_str(),
        ),
    }
}

/// The gateway's address as IronRDP takes it: host and optional port, with
/// any `https://` and path the user pasted taken off.
pub(crate) fn endpoint(gateway: &str) -> String {
    let gateway = gateway.trim();
    let gateway = gateway
        .strip_prefix("https://")
        .or_else(|| gateway.strip_prefix("HTTPS://"))
        .unwrap_or(gateway);
    gateway.split('/').next().unwrap_or_default().to_string()
}

/// What went wrong, from the gateway error's kind and its full report.
fn failure(gateway: &str, kind: &GwErrorKind, detail: &str) -> Error {
    match kind {
        GwErrorKind::HttpStatus(status @ (401 | 403)) => Error::Authentication(format!(
            "the gateway {gateway} refused the sign-in (HTTP {status})"
        )),
        GwErrorKind::ConsentDeclined => Error::Gateway(format!(
            "the gateway {gateway} asks to accept a message first, which Remoport cannot show yet"
        )),
        _ if detail.to_lowercase().contains("certificate") => Error::Gateway(format!(
            "the gateway {gateway} is not trusted: its certificate does not come from an \
             authority this computer trusts, or does not match its name. Nothing was sent to it. \
             ({detail})"
        )),
        GwErrorKind::Connect => Error::Unreachable(format!(
            "the gateway {gateway} could not be reached: {detail}"
        )),
        _ => Error::Gateway(format!("the gateway {gateway} failed: {detail}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refused_sign_in_and_an_untrusted_certificate_are_told_apart() {
        assert!(matches!(
            failure("gw", &GwErrorKind::HttpStatus(401), "unauthorized"),
            Error::Authentication(_)
        ));
        let untrusted = failure(
            "gw",
            &GwErrorKind::Connect,
            "TLS: error:0A000086:SSL routines::certificate verify failed",
        );
        assert!(matches!(&untrusted, Error::Gateway(m) if m.contains("not trusted")));
        assert!(matches!(
            failure("gw", &GwErrorKind::Connect, "connection refused"),
            Error::Unreachable(_)
        ));
    }

    #[test]
    fn the_gateway_signs_in_with_its_own_account_or_the_pcs() {
        let mut config = ConnectConfig::new(
            "pc".to_string(),
            "ana".to_string(),
            zeroize::Zeroizing::new("pc-secret".to_string()),
            crate::TrustStore::new(std::env::temp_dir().join("unused.json")),
        );
        config.domain = Some("CORP".to_string());
        let shared = Gateway {
            address: "gw".to_string(),
            account: None,
        };
        assert_eq!(
            account(&shared, &config),
            ("CORP\\ana".to_string(), "pc-secret")
        );
        let own = Gateway {
            address: "gw".to_string(),
            account: Some((
                " gwuser@corp.example ".to_string(),
                zeroize::Zeroizing::new("gw-secret".to_string()),
            )),
        };
        assert_eq!(
            account(&own, &config),
            ("gwuser@corp.example".to_string(), "gw-secret")
        );
    }

    #[test]
    fn pasted_addresses_are_reduced_to_host_and_port() {
        assert_eq!(endpoint("gw.example.com"), "gw.example.com");
        assert_eq!(endpoint(" https://gw.example.com/rpc/ "), "gw.example.com");
        assert_eq!(endpoint("gw.example.com:8443"), "gw.example.com:8443");
        assert_eq!(endpoint("[2001:db8::1]:443"), "[2001:db8::1]:443");
    }
}
