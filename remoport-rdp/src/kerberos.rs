//! Kerberos sign-in inside Network Level Authentication, for the PCs the
//! user turns it on for (accounts in Active Directory's "Protected Users"
//! group may not use NTLM).
//!
//! What sspi, under IronRDP, does with it:
//! - it signs in with the password typed in Remoport; a `kinit` ticket is
//!   never read;
//! - it finds the KDC from the address given for the PC, else
//!   `SSPI_KDC_URL_<REALM>`, `SSPI_KDC_URL` or `krb5.conf`; when none names
//!   one, Remoport tries the realm's own DNS name, which in Active Directory
//!   leads to the domain controllers;
//! - a KDC that cannot be reached fails the sign-in: there is no fallback to
//!   NTLM once Kerberos is chosen.
//!
//! The service principal is `TERMSRV/<host>`, so the PC must be named by its
//! DNS name, and the realm is the DNS domain.
//!
//! Ported from RustConn's Kerberos module (`rustconn-core`,
//! GPL-3.0-or-later).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use ironrdp_connector::credssp::KerberosConfig;

use crate::ConnectConfig;

/// The port a KDC listens on when its address names none (RFC 4120 7.2.1).
const KDC_DEFAULT_PORT: u16 = 88;

/// Why a KDC address cannot be used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KdcAddressError {
    #[error("the KDC address contains a space")]
    Whitespace,
    #[error("the KDC address must start with tcp://, udp://, http:// or https://, not {0}://")]
    UnsupportedScheme(String),
    #[error("the KDC address has no host")]
    MissingHost,
    #[error("the KDC address's host is not a host name or an IP address")]
    InvalidHost,
    #[error("the KDC address's port must be a number from 1 to 65535")]
    InvalidPort,
    #[error("a tcp:// or udp:// KDC address takes no path")]
    UnexpectedPath,
}

/// A KDC address as sspi takes it: `host`, `host:port`, `tcp://`, `udp://`,
/// or a KDC proxy over `http(s)://`. A bare host means `tcp://host:88`.
/// Blank means none. Credentials in the address (`user@host`) are refused.
///
/// # Errors
///
/// The first problem found in the address.
pub fn normalize_kdc_address(input: &str) -> Result<Option<String>, KdcAddressError> {
    let input = input.trim();
    if input.is_empty() {
        return Ok(None);
    }
    if input.chars().any(char::is_whitespace) {
        return Err(KdcAddressError::Whitespace);
    }
    let (scheme, rest) = match input.split_once("://") {
        Some((scheme, rest)) => (scheme.to_ascii_lowercase(), rest),
        None => (String::from("tcp"), input),
    };
    match scheme.as_str() {
        "tcp" | "udp" => {
            let authority = match rest.split_once('/') {
                Some((authority, "")) => authority,
                Some(_) => return Err(KdcAddressError::UnexpectedPath),
                None => rest,
            };
            let (host, port) = parse_authority(authority)?;
            let port = port.unwrap_or(KDC_DEFAULT_PORT);
            Ok(Some(format!("{scheme}://{host}:{port}")))
        }
        "http" | "https" => {
            let (authority, path) = match rest.find(['/', '?', '#']) {
                Some(index) => rest.split_at(index),
                None => (rest, ""),
            };
            let (host, port) = parse_authority(authority)?;
            let port = port.map(|port| format!(":{port}")).unwrap_or_default();
            Ok(Some(format!("{scheme}://{host}{port}{path}")))
        }
        _ => Err(KdcAddressError::UnsupportedScheme(scheme)),
    }
}

/// Splits `host`, `host:port`, `[v6]` or `[v6]:port`, the IPv6 address
/// coming back bracketed.
fn parse_authority(authority: &str) -> Result<(String, Option<u16>), KdcAddressError> {
    if authority.contains('@') {
        return Err(KdcAddressError::InvalidHost);
    }
    if let Some(bracketed) = authority.strip_prefix('[') {
        let (address, after) = bracketed
            .split_once(']')
            .ok_or(KdcAddressError::InvalidHost)?;
        if address.is_empty() {
            return Err(KdcAddressError::MissingHost);
        }
        let address: Ipv6Addr = address.parse().map_err(|_| KdcAddressError::InvalidHost)?;
        let port = match after {
            "" => None,
            _ => Some(parse_port(
                after
                    .strip_prefix(':')
                    .ok_or(KdcAddressError::InvalidHost)?,
            )?),
        };
        return Ok((format!("[{address}]"), port));
    }
    // An IPv6 address typed without brackets has several colons and no port.
    if authority.matches(':').count() > 1 {
        let address: Ipv6Addr = authority
            .parse()
            .map_err(|_| KdcAddressError::InvalidHost)?;
        return Ok((format!("[{address}]"), None));
    }
    let (host, port) = match authority.split_once(':') {
        Some((host, port)) => (host, Some(parse_port(port)?)),
        None => (authority, None),
    };
    if host.is_empty() {
        return Err(KdcAddressError::MissingHost);
    }
    if !host
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'))
    {
        return Err(KdcAddressError::InvalidHost);
    }
    let numeric = host.bytes().all(|b| b.is_ascii_digit() || b == b'.');
    if numeric && host.parse::<Ipv4Addr>().is_err() {
        return Err(KdcAddressError::InvalidHost);
    }
    Ok((host.to_owned(), port))
}

fn parse_port(text: &str) -> Result<u16, KdcAddressError> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(KdcAddressError::InvalidPort);
    }
    match text.parse::<u16>() {
        Ok(0) | Err(_) => Err(KdcAddressError::InvalidPort),
        Ok(port) => Ok(port),
    }
}

/// The realm sspi signs in to, upper-cased: the domain when one is given,
/// else the part of `user@example.com` after the `@`.
fn realm(username: &str, domain: Option<&str>) -> Option<String> {
    let domain = domain.map(str::trim).unwrap_or_default();
    let source = if domain.is_empty() {
        username
            .trim()
            .rsplit_once('@')
            .map_or("", |(_, suffix)| suffix)
    } else {
        domain
    };
    let source = source.trim();
    (!source.is_empty()).then(|| source.to_uppercase())
}

/// Settings Kerberos cannot work with, as notes for the log.
pub(crate) fn preflight(config: &ConnectConfig) -> Vec<String> {
    let mut notes = Vec::new();
    let host = config.host.trim().trim_end_matches('.');
    let bare = host
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .unwrap_or(host);
    if host.eq_ignore_ascii_case("localhost") || bare.parse::<IpAddr>().is_ok() {
        notes.push(format!(
            "Kerberos needs the PC's DNS name, not {host}: the PC is known to the domain as \
             TERMSRV/<its name>"
        ));
    }
    match realm(&config.username, config.domain.as_deref()) {
        None => notes.push(
            "Kerberos needs a domain: sign in as user@example.com or EXAMPLE.COM\\user".to_string(),
        ),
        Some(realm) if !realm.contains('.') => notes.push(format!(
            "Kerberos needs the DNS domain (such as {realm}.COM), not the short name {realm}"
        )),
        Some(_) => {}
    }
    notes
}

/// IronRDP's Kerberos settings for this connection: the KDC given for the
/// PC, else none when sspi finds one itself, else the realm's DNS name.
pub(crate) fn settings(config: &ConnectConfig, kdc: Option<&str>) -> Option<KerberosConfig> {
    let kdc = kdc
        .and_then(|kdc| normalize_kdc_address(kdc).ok().flatten())
        .or_else(|| fallback_kdc(config, sspi_finds_kdc));
    KerberosConfig::new(kdc, config.client_name.clone()).ok()
}

/// The realm's own DNS name as its KDC, unless sspi finds one by itself.
fn fallback_kdc(config: &ConnectConfig, discover: impl FnOnce(&str) -> bool) -> Option<String> {
    let realm = realm(&config.username, config.domain.as_deref())?;
    if discover(&realm) {
        return None;
    }
    normalize_kdc_address(&realm.to_lowercase()).ok().flatten()
}

/// Whether sspi finds a KDC for `realm` in the environment or `krb5.conf`,
/// asked exactly as sspi asks when signing in.
fn sspi_finds_kdc(realm: &str) -> bool {
    use ironrdp_connector::sspi;
    let realm = sspi::kerberos::client::principal::get_client_principal_realm("", realm);
    sspi::detect_kdc_url(&realm).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kdc_addresses_are_normalized() {
        let cases = [
            ("dc01.example.com", "tcp://dc01.example.com:88"),
            ("dc01.example.com:8888", "tcp://dc01.example.com:8888"),
            ("UDP://10.0.0.5", "udp://10.0.0.5:88"),
            ("tcp://dc/", "tcp://dc:88"),
            ("2001:db8::1", "tcp://[2001:db8::1]:88"),
            (
                "https://gw.example.com/KdcProxy",
                "https://gw.example.com/KdcProxy",
            ),
        ];
        for (input, expected) in cases {
            assert_eq!(
                normalize_kdc_address(input),
                Ok(Some(expected.to_string())),
                "{input}"
            );
        }
        assert_eq!(normalize_kdc_address("  "), Ok(None));
    }

    #[test]
    fn bad_kdc_addresses_are_refused() {
        assert_eq!(
            normalize_kdc_address("dc 01"),
            Err(KdcAddressError::Whitespace)
        );
        assert!(matches!(
            normalize_kdc_address("ldap://dc"),
            Err(KdcAddressError::UnsupportedScheme(_))
        ));
        assert_eq!(
            normalize_kdc_address("user@dc"),
            Err(KdcAddressError::InvalidHost)
        );
        assert_eq!(
            normalize_kdc_address("dc:0"),
            Err(KdcAddressError::InvalidPort)
        );
        assert_eq!(
            normalize_kdc_address("999.1.1.1"),
            Err(KdcAddressError::InvalidHost)
        );
        assert_eq!(
            normalize_kdc_address("tcp://dc/x"),
            Err(KdcAddressError::UnexpectedPath)
        );
    }

    #[test]
    fn the_realm_comes_from_the_domain_or_the_user_name() {
        assert_eq!(
            realm("ana", Some("example.com")),
            Some("EXAMPLE.COM".to_string())
        );
        assert_eq!(
            realm("ana@corp.example.com", None),
            Some("CORP.EXAMPLE.COM".to_string())
        );
        assert_eq!(realm("ana", None), None);
    }

    fn config(host: &str, username: &str, domain: Option<&str>) -> ConnectConfig {
        ConnectConfig {
            host: host.to_string(),
            port: 3389,
            username: username.to_string(),
            domain: domain.map(str::to_string),
            password: zeroize::Zeroizing::new(String::new()),
            width: 1280,
            height: 720,
            monitors: Vec::new(),
            scale: 100,
            allow_without_nla: false,
            trust_store: crate::TrustStore::new(std::env::temp_dir().join("unused.json")),
            timeout: std::time::Duration::from_secs(1),
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
    fn settings_kerberos_cannot_use_are_noted() {
        assert!(preflight(&config("pc.example.com", "ana", Some("example.com"))).is_empty());
        assert_eq!(
            preflight(&config("10.0.0.7", "ana", Some("EXAMPLE"))).len(),
            2
        );
        assert_eq!(preflight(&config("pc.example.com", "ana", None)).len(), 1);
    }

    #[test]
    fn the_kdc_falls_back_to_the_realm_unless_sspi_finds_one() {
        let pc = config("pc.example.com", "ana@example.com", None);
        assert_eq!(
            fallback_kdc(&pc, |_| false),
            Some("tcp://example.com:88".to_string())
        );
        assert_eq!(fallback_kdc(&pc, |_| true), None);
    }
}
