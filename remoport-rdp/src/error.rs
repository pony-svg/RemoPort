use thiserror::Error;

/// Why a session could not start, or why it ended.
#[derive(Debug, Error)]
pub enum Error {
    /// The PC could not be reached: unknown name, nothing listening, no route.
    #[error("{0}")]
    Unreachable(String),
    /// A step of the connection took too long.
    #[error("the PC did not answer in time ({0})")]
    Timeout(&'static str),
    /// The PC does not offer Network Level Authentication and this PC is not
    /// allowed to connect without it.
    #[error(
        "this PC does not offer Network Level Authentication, so the sign-in would happen \
         after the session has already started"
    )]
    NlaUnavailable,
    /// The user did not trust the server's identity.
    #[error("the PC's identity was not trusted")]
    IdentityRejected,
    /// The sign-in was refused (wrong password, locked account, ...).
    #[error("the sign-in was refused: {0}")]
    Authentication(String),
    /// A connection broker moved the session to another server.
    #[error("the PC sent the session to another server; connection brokers are not supported yet")]
    BrokerRedirect,
    /// The PC spoke RDP in a way the engine could not follow.
    #[error("{0}")]
    Protocol(String),
    /// An established session lost its connection.
    #[error("the connection was lost: {0}")]
    ConnectionLost(String),
}

impl Error {
    /// Whether signing in again with other credentials could help.
    #[must_use]
    pub const fn is_authentication(&self) -> bool {
        matches!(self, Self::Authentication(_))
    }
}

/// `NTSTATUS` codes and markers CredSSP/NLA reports for credential problems
/// (from RustConn's `rdp_client::failure`).
const AUTH_NSTATUS_CODES: &[&str] = &[
    "0xc0000064", // STATUS_NO_SUCH_USER
    "0xc000006d", // STATUS_LOGON_FAILURE
    "0xc000006a", // STATUS_WRONG_PASSWORD
    "0xc000006e", // STATUS_ACCOUNT_RESTRICTION
    "0xc000006f", // STATUS_INVALID_LOGON_HOURS
    "0xc0000070", // STATUS_INVALID_WORKSTATION
    "0xc0000071", // STATUS_PASSWORD_EXPIRED
    "0xc0000072", // STATUS_ACCOUNT_DISABLED
    "0xc000015b", // STATUS_LOGON_TYPE_NOT_GRANTED
    "0xc0000193", // STATUS_ACCOUNT_EXPIRED
    "0xc0000224", // STATUS_PASSWORD_MUST_CHANGE
    "0xc0000234", // STATUS_ACCOUNT_LOCKED_OUT
];
const AUTH_MARKERS: &[&str] = &[
    "authentication failed",
    "status_no_such_user",
    "status_logon_failure",
    "status_wrong_password",
    "status_password_expired",
    "status_password_must_change",
    "status_account_disabled",
    "status_account_expired",
    "status_account_locked_out",
    "status_account_restriction",
    "status_logon_type_not_granted",
    "accessdenied",
];

/// Whether a connector failure describes refused credentials.
pub(crate) fn is_authentication_failure(detail: &str) -> bool {
    let lower = detail.to_ascii_lowercase();
    AUTH_NSTATUS_CODES.iter().any(|code| lower.contains(code))
        || AUTH_MARKERS.iter().any(|marker| lower.contains(marker))
}

/// Whether a negotiation failure means the server will not do NLA.
pub(crate) fn is_nla_refusal(detail: &str) -> bool {
    // When only NLA is requested, a server that has it switched off answers
    // SSL_REQUIRED_BY_SERVER (failure code 1, MS-RDPBCGR 2.2.1.2.2). The code
    // is matched, not IronRDP's wording, which has changed between releases.
    let lower = detail.to_ascii_lowercase();
    lower.contains("negotiationfailure(failurecode(1))") || lower.contains("ssl_required_by_server")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refused_credentials_are_recognized() {
        assert!(is_authentication_failure(
            "Connection finalize failed: [CredSSP] [kind: Credssp(Error { nstatus: Some(NStatusCode(0xc000006d)) })]"
        ));
        assert!(is_authentication_failure("STATUS_ACCOUNT_LOCKED_OUT"));
        assert!(!is_authentication_failure(
            "Connection refused (os error 111)"
        ));
    }

    #[test]
    fn a_server_without_nla_is_recognized() {
        assert!(is_nla_refusal(
            "negotiation failure: server requires Enhanced RDP Security with TLS or CredSSP \
             [Negotiation(NegotiationFailure(FailureCode(1)))]"
        ));
        assert!(is_nla_refusal("SSL_REQUIRED_BY_SERVER"));
        // Other negotiation failures are not "no NLA".
        assert!(!is_nla_refusal(
            "[Negotiation(NegotiationFailure(FailureCode(2)))]"
        ));
        assert!(!is_nla_refusal("TLS upgrade failed"));
    }
}
