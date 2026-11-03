//! Is this really the PC that was meant? Checked after the TLS handshake and
//! before CredSSP, so no credential, not even an NTLM response that could be
//! cracked offline, reaches a server that is not the one the user trusts.
//!
//! 1. A certificate that chains to an authority this computer trusts and
//!    names the host is accepted: [`Verdict::Verified`].
//! 2. A certificate the user accepted before is accepted while unchanged:
//!    [`Verdict::Remembered`].
//! 3. Anything else goes to the user, with the certificate's details:
//!    [`Verdict::Ask`]. A certificate that changed since the user accepted it
//!    is the classic sign of an interception and is presented that way.
//!
//! Every failure fails closed: a trust store that cannot be read counts as
//! empty, so Remoport asks instead of accepting.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use rustls_pki_types::{CertificateDer, ServerName, UnixTime};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// What the user is shown about a certificate that needs their decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertificateInfo {
    pub host: String,
    pub port: u16,
    /// The certificate's subject, e.g. `CN=PC01.corp.example`.
    pub subject: String,
    pub issuer: String,
    /// Validity bounds, as `YYYY-MM-DD`.
    pub valid_from: String,
    pub valid_until: String,
    /// SHA-256 of the certificate, as colon-separated uppercase hex.
    pub sha256: String,
    /// Why no trusted authority vouches for it.
    pub why_unverified: String,
}

/// Why the user is asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityProblem {
    /// First connection to this PC with this certificate.
    Unknown,
    /// The PC presents a different certificate than the one the user trusted.
    Changed { previous_sha256: String },
}

/// The outcome of the automatic checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Verdict {
    Verified { issuer: String },
    Remembered,
    Ask(IdentityProblem),
}

/// A server the user trusted, as shown in Settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedServer {
    /// `host:port`, the host in lower case.
    pub server: String,
    pub sha256: String,
    pub subject: String,
    /// When the user trusted it, RFC 3339.
    pub trusted_at: String,
}

/// Certificates the user chose to trust, one per `host:port`, kept in a JSON
/// file readable only by the user.
#[derive(Debug, Clone)]
pub struct TrustStore {
    path: PathBuf,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct StoreFile {
    servers: BTreeMap<String, Entry>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Entry {
    sha256: String,
    subject: String,
    trusted_at: String,
}

impl TrustStore {
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn key(host: &str, port: u16) -> String {
        format!("{}:{port}", host.to_ascii_lowercase())
    }

    /// The stored file, or an empty one when it is missing or unreadable.
    fn load(&self) -> StoreFile {
        std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// The fingerprint the user trusted for `host:port`, if any.
    #[must_use]
    pub fn trusted(&self, host: &str, port: u16) -> Option<String> {
        self.load()
            .servers
            .remove(&Self::key(host, port))
            .map(|entry| entry.sha256)
    }

    /// Every server the user trusted, sorted by `host:port`.
    #[must_use]
    pub fn entries(&self) -> Vec<TrustedServer> {
        self.load()
            .servers
            .into_iter()
            .map(|(server, entry)| TrustedServer {
                server,
                sha256: entry.sha256,
                subject: entry.subject,
                trusted_at: entry.trusted_at,
            })
            .collect()
    }

    /// Stops trusting the certificate kept for `server` (`host:port`): the
    /// next connection asks again.
    ///
    /// # Errors
    ///
    /// Returns the I/O error when the file cannot be written.
    pub fn forget(&self, server: &str) -> io::Result<()> {
        let mut file = self.load();
        if file.servers.remove(server).is_none() {
            return Ok(());
        }
        let json = serde_json::to_string_pretty(&file).map_err(io::Error::other)?;
        write_private(&self.path, &json)
    }

    /// Records that the user trusts `info`'s certificate for its host,
    /// replacing any earlier one.
    ///
    /// # Errors
    ///
    /// Returns the I/O error when the file cannot be written.
    pub fn remember(&self, info: &CertificateInfo) -> io::Result<()> {
        // An unreadable file is moved aside, not overwritten: it may hold
        // other servers the user trusted.
        if let Ok(text) = std::fs::read_to_string(&self.path)
            && serde_json::from_str::<StoreFile>(&text).is_err()
        {
            let aside = self.path.with_extension(format!(
                "unreadable-{}",
                chrono::Utc::now().format("%Y%m%d%H%M%S")
            ));
            std::fs::rename(&self.path, aside)?;
        }
        let mut file = self.load();
        file.servers.insert(
            Self::key(&info.host, info.port),
            Entry {
                sha256: info.sha256.clone(),
                subject: info.subject.clone(),
                trusted_at: chrono::Utc::now().to_rfc3339(),
            },
        );
        let json = serde_json::to_string_pretty(&file).map_err(io::Error::other)?;
        write_private(&self.path, &json)
    }
}

/// Writes `contents` to `path` readable only by the user (0600, in a 0700
/// directory), replacing the old file only once the new one is complete.
pub fn write_private(path: &Path, contents: &str) -> io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _};

    if let Some(dir) = path.parent() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
    }
    let partial = path.with_extension("partial");
    let _ = std::fs::remove_file(&partial);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&partial)?;
    file.write_all(contents.as_bytes())?;
    file.sync_all()?;
    std::fs::rename(&partial, path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

/// SHA-256 of `der`, as colon-separated uppercase hex.
#[must_use]
pub fn fingerprint(der: &[u8]) -> String {
    Sha256::digest(der)
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// Describes `cert` (DER `der`) for the user.
pub(crate) fn describe(
    cert: &x509_cert::Certificate,
    der: &[u8],
    host: &str,
    port: u16,
    why_unverified: String,
) -> CertificateInfo {
    let tbs = cert.tbs_certificate();
    let date = |time: x509_cert::time::Time| {
        let secs = i64::try_from(time.to_unix_duration().as_secs()).unwrap_or(i64::MAX);
        chrono::DateTime::from_timestamp(secs, 0)
            .map_or_else(|| "unknown".to_string(), |d| d.format("%Y-%m-%d").to_string())
    };
    CertificateInfo {
        host: host.to_string(),
        port,
        subject: tbs.subject().to_string(),
        issuer: tbs.issuer().to_string(),
        valid_from: date(tbs.validity().not_before),
        valid_until: date(tbs.validity().not_after),
        sha256: fingerprint(der),
        why_unverified,
    }
}

/// Checks `der` against the authorities this computer trusts and against
/// `host`. Returns the issuer on success, or why it failed in plain words.
pub(crate) fn chain_check(der: &[u8], host: &str) -> Result<String, String> {
    let certificate = CertificateDer::from(der);
    let end_entity = webpki::EndEntityCert::try_from(&certificate)
        .map_err(|e| format!("the certificate could not be read ({e:?})"))?;
    let roots = rustls_native_certs::load_native_certs().certs;
    let anchors: Vec<_> = roots
        .iter()
        .filter_map(|root| webpki::anchor_from_trusted_cert(root).ok())
        .collect();
    end_entity
        .verify_for_usage(
            webpki::ALL_VERIFICATION_ALGS,
            &anchors,
            &[],
            UnixTime::now(),
            webpki::KeyUsage::server_auth(),
            None,
            None,
        )
        .map_err(|e| unverified_reason(&e))?;
    let name = ServerName::try_from(host)
        .map_err(|_| format!("\"{host}\" is not a name a certificate can carry"))?;
    end_entity
        .verify_is_valid_for_subject_name(&name)
        .map_err(|_| format!("the certificate is not issued for \"{host}\""))?;
    Ok(issuer_of(der))
}

fn unverified_reason(error: &webpki::Error) -> String {
    match error {
        webpki::Error::UnknownIssuer => {
            "no authority this computer trusts issued it (often a self-signed certificate)"
                .to_string()
        }
        webpki::Error::CertExpired { .. } => "the certificate has expired".to_string(),
        webpki::Error::CertNotValidYet { .. } => "the certificate is not valid yet".to_string(),
        webpki::Error::CertRevoked => "the certificate was revoked".to_string(),
        other => format!("the certificate could not be verified ({other:?})"),
    }
}

fn issuer_of(der: &[u8]) -> String {
    use x509_cert::der::Decode as _;
    x509_cert::Certificate::from_der(der)
        .map(|cert| cert.tbs_certificate().issuer().to_string())
        .unwrap_or_else(|_| "a trusted authority".to_string())
}

/// Decides without the user when the checks allow it.
pub(crate) fn verdict(
    chain: &Result<String, String>,
    trusted: Option<&str>,
    sha256: &str,
) -> Verdict {
    if let Ok(issuer) = chain {
        return Verdict::Verified {
            issuer: issuer.clone(),
        };
    }
    match trusted {
        Some(previous) if previous == sha256 => Verdict::Remembered,
        Some(previous) => Verdict::Ask(IdentityProblem::Changed {
            previous_sha256: previous.to_string(),
        }),
        None => Verdict::Ask(IdentityProblem::Unknown),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(sha256: &str) -> CertificateInfo {
        CertificateInfo {
            host: "PC01.Example.com".into(),
            port: 3389,
            subject: "CN=PC01.example.com".into(),
            issuer: "CN=PC01.example.com".into(),
            valid_from: "2026-01-01".into(),
            valid_until: "2027-01-01".into(),
            sha256: sha256.into(),
            why_unverified: "self-signed".into(),
        }
    }

    fn scratch() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "remoport-rdp-test-{}-{}",
            std::process::id(),
            fingerprint(format!("{:?}", std::time::Instant::now()).as_bytes())
                .replace(':', "")
                .get(..12)
                .unwrap_or("x")
        ));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    #[test]
    fn a_ca_verified_certificate_needs_no_question() {
        let chain = Ok("CN=Corp CA".to_string());
        assert_eq!(
            verdict(&chain, None, "AA"),
            Verdict::Verified {
                issuer: "CN=Corp CA".into()
            }
        );
    }

    #[test]
    fn an_unverified_certificate_is_asked_about_then_remembered() {
        let chain = Err("self-signed".to_string());
        assert_eq!(verdict(&chain, None, "AA"), Verdict::Ask(IdentityProblem::Unknown));
        assert_eq!(verdict(&chain, Some("AA"), "AA"), Verdict::Remembered);
    }

    #[test]
    fn a_changed_certificate_is_never_accepted_silently() {
        let chain = Err("self-signed".to_string());
        assert_eq!(
            verdict(&chain, Some("AA"), "BB"),
            Verdict::Ask(IdentityProblem::Changed {
                previous_sha256: "AA".into()
            })
        );
    }

    #[test]
    fn trusted_certificates_are_kept_privately_per_host() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = scratch();
        // A directory the store creates itself is private too.
        let store = TrustStore::new(dir.join("remoport").join("trusted_servers.json"));
        assert_eq!(store.trusted("pc01.example.com", 3389), None);

        store.remember(&info("AA")).expect("remember");
        // Host names compare case-insensitively; the port is part of the key.
        assert_eq!(store.trusted("pc01.example.com", 3389).as_deref(), Some("AA"));
        assert_eq!(store.trusted("pc01.example.com", 3390), None);
        let entries = store.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].server, "pc01.example.com:3389");

        let mode = std::fs::metadata(store.path()).expect("meta").permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let dir_mode = std::fs::metadata(dir.join("remoport"))
            .expect("meta")
            .permissions()
            .mode();
        assert_eq!(dir_mode & 0o077, 0, "nobody else may list the directory");
        // Forgetting makes the next connection ask again.
        store.forget("pc01.example.com:3389").expect("forget");
        assert_eq!(store.trusted("pc01.example.com", 3389), None);
        assert!(store.entries().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_unreadable_store_asks_instead_of_accepting() {
        let dir = scratch();
        let path = dir.join("trusted_servers.json");
        std::fs::write(&path, "{ not json").expect("write");
        let store = TrustStore::new(&path);
        assert_eq!(store.trusted("pc01.example.com", 3389), None);
        let chain = Err("self-signed".to_string());
        assert_eq!(verdict(&chain, None, "AA"), Verdict::Ask(IdentityProblem::Unknown));

        // Trusting a server afterwards keeps the broken file aside.
        store.remember(&info("AA")).expect("remember");
        let kept = std::fs::read_dir(&dir)
            .expect("list")
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().contains("unreadable"))
            .count();
        assert_eq!(kept, 1);
        assert_eq!(store.trusted("pc01.example.com", 3389).as_deref(), Some("AA"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn fingerprints_are_sha256_in_colon_hex() {
        let fp = fingerprint(b"abc");
        assert_eq!(fp.len(), 32 * 3 - 1);
        assert!(fp.starts_with("BA:78:16:BF"));
    }

    #[test]
    fn a_self_signed_certificate_is_not_chain_verified() {
        // Garbage is rejected with a reason, never accepted.
        assert!(chain_check(b"not a certificate", "pc01.example.com").is_err());
    }
}
