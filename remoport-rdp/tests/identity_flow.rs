//! The identity check against a fake RDP server that negotiates NLA and then
//! presents a self-signed certificate over TLS. What the server receives
//! after TLS is the point: nothing at all unless the user trusts it.

use std::time::{Duration, Instant};

use ironrdp_pdu::nego::{ConnectionConfirm, FailureCode, ResponseFlags, SecurityProtocol};
use ironrdp_pdu::x224::X224;
use remoport_rdp::{ConnectConfig, Error, Event, IdentityProblem, Session, TrustStore, fingerprint};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use zeroize::Zeroizing;

fn self_signed() -> (tokio_native_tls::native_tls::Identity, Vec<u8>) {
    let certified =
        rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).expect("certificate");
    let identity = tokio_native_tls::native_tls::Identity::from_pkcs8(
        certified.cert.pem().as_bytes(),
        certified.key_pair.serialize_pem().as_bytes(),
    )
    .expect("identity");
    (identity, certified.cert.der().to_vec())
}

/// Serves one client: answers its negotiation request with `reply`, and for a
/// successful one completes TLS and reports what the client sends next.
async fn fake_server(reply: ConnectionConfirm) -> (u16, Vec<u8>, oneshot::Receiver<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let (identity, der) = self_signed();
    let (report, received) = oneshot::channel();
    let tls = matches!(reply, ConnectionConfirm::Response { .. });
    tokio::spawn(async move {
        let Ok((mut stream, _)) = listener.accept().await else {
            return;
        };
        // The client's X.224 Connection Request, length from its TPKT header.
        let mut header = [0u8; 4];
        if stream.read_exact(&mut header).await.is_err() {
            return;
        }
        let mut rest = vec![0u8; usize::from(u16::from_be_bytes([header[2], header[3]])) - 4];
        if stream.read_exact(&mut rest).await.is_err() {
            return;
        }
        let answer = ironrdp_pdu::encode_vec(&X224(reply)).expect("encode");
        if stream.write_all(&answer).await.is_err() || !tls {
            return;
        }
        let acceptor = tokio_native_tls::TlsAcceptor::from(
            tokio_native_tls::native_tls::TlsAcceptor::new(identity).expect("acceptor"),
        );
        let Ok(mut secure) = acceptor.accept(stream).await else {
            return;
        };
        let mut buf = vec![0u8; 4096];
        let got = match tokio::time::timeout(Duration::from_secs(3), secure.read(&mut buf)).await {
            Ok(Ok(n)) => buf[..n].to_vec(),
            _ => Vec::new(),
        };
        let _ = report.send(got);
    });
    (port, der, received)
}

fn config(port: u16, store: &TrustStore) -> ConnectConfig {
    ConnectConfig {
        host: "localhost".to_string(),
        port,
        username: "tester".to_string(),
        domain: None,
        password: Zeroizing::new("not-a-real-password".to_string()),
        width: 1024,
        height: 768,
        allow_without_nla: false,
        trust_store: store.clone(),
        timeout: Duration::from_secs(5),
        client_name: "remoport-test".to_string(),
    }
}

fn scratch_store(name: &str) -> (std::path::PathBuf, TrustStore) {
    let dir = std::env::temp_dir().join(format!("remoport-identity-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let store = TrustStore::new(dir.join("trusted_servers.json"));
    (dir, store)
}

/// The next event that is not progress, within ten seconds.
async fn next_event(session: &Session) -> Event {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match session.try_event() {
            Some(Event::Progress(_)) => {}
            Some(event) => return event,
            None if Instant::now() > deadline => panic!("no event from the engine"),
            None => tokio::time::sleep(Duration::from_millis(10)).await,
        }
    }
}

fn hybrid() -> ConnectionConfirm {
    ConnectionConfirm::Response {
        flags: ResponseFlags::empty(),
        protocol: SecurityProtocol::HYBRID,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_untrusted_server_receives_nothing_after_tls() {
    let (port, der, received) = fake_server(hybrid()).await;
    let (dir, store) = scratch_store("reject");
    let session = Session::start(config(port, &store));

    let Event::VerifyIdentity { info, problem, reply } = next_event(&session).await else {
        panic!("expected the identity question");
    };
    assert_eq!(problem, IdentityProblem::Unknown);
    assert_eq!(info.sha256, fingerprint(&der));
    assert!(!info.why_unverified.is_empty());
    reply.answer(false);

    assert!(matches!(next_event(&session).await, Event::Ended(Some(Error::IdentityRejected))));
    let after_tls = received.await.expect("server report");
    assert!(after_tls.is_empty(), "no CredSSP byte may reach an untrusted server");
    assert_eq!(store.trusted("localhost", port), None);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_trusted_server_gets_credssp_and_is_remembered() {
    let (port, der, received) = fake_server(hybrid()).await;
    let (dir, store) = scratch_store("trust");
    let session = Session::start(config(port, &store));

    let Event::VerifyIdentity { reply, .. } = next_event(&session).await else {
        panic!("expected the identity question");
    };
    reply.answer(true);

    let after_tls = received.await.expect("server report");
    // CredSSP opens with a TSRequest, an ASN.1 SEQUENCE.
    assert_eq!(after_tls.first(), Some(&0x30), "CredSSP starts only after trust");
    assert_eq!(store.trusted("localhost", port), Some(fingerprint(&der)));
    drop(session);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_changed_certificate_is_reported_as_changed() {
    let (port, _der, received) = fake_server(hybrid()).await;
    let (dir, store) = scratch_store("changed");
    // The user trusted another certificate for this PC before.
    store
        .remember(&remoport_rdp::CertificateInfo {
            host: "localhost".into(),
            port,
            subject: "CN=localhost".into(),
            issuer: "CN=localhost".into(),
            valid_from: "2026-01-01".into(),
            valid_until: "2027-01-01".into(),
            sha256: "AA:BB".into(),
            why_unverified: String::new(),
        })
        .expect("remember");
    let session = Session::start(config(port, &store));

    let Event::VerifyIdentity { problem, reply, .. } = next_event(&session).await else {
        panic!("expected the identity question");
    };
    assert_eq!(
        problem,
        IdentityProblem::Changed {
            previous_sha256: "AA:BB".into()
        }
    );
    reply.answer(false);
    assert!(matches!(next_event(&session).await, Event::Ended(Some(Error::IdentityRejected))));
    assert!(received.await.expect("server report").is_empty());
    // The certificate the user trusted is kept.
    assert_eq!(store.trusted("localhost", port).as_deref(), Some("AA:BB"));
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_without_nla_is_refused() {
    let (port, _der, _received) = fake_server(ConnectionConfirm::Failure {
        code: FailureCode::SSL_REQUIRED_BY_SERVER,
    })
    .await;
    let (dir, store) = scratch_store("nla");
    let session = Session::start(config(port, &store));
    let event = next_event(&session).await;
    assert!(
        matches!(event, Event::Ended(Some(Error::NlaUnavailable))),
        "got {event:?}"
    );
    let _ = std::fs::remove_dir_all(dir);
}
