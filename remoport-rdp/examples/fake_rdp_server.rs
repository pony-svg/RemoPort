//! A stand-in RDP server for trying Remoport's identity screens by hand: it
//! negotiates NLA and presents a fresh self-signed certificate over TLS, then
//! waits. It never signs anyone in.
//!
//! ```text
//! cargo run -p remoport-rdp --example fake_rdp_server -- 33890
//! ```

use std::time::Duration;

use ironrdp_pdu::nego::{ConnectionConfirm, ResponseFlags, SecurityProtocol};
use ironrdp_pdu::x224::X224;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() {
    let port: u16 = std::env::args()
        .nth(1)
        .and_then(|p| p.parse().ok())
        .unwrap_or(33890);
    let certified =
        rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).expect("certificate");
    println!(
        "listening on 127.0.0.1:{port}, certificate SHA-256 {}",
        remoport_rdp::fingerprint(certified.cert.der())
    );
    let identity = tokio_native_tls::native_tls::Identity::from_pkcs8(
        certified.cert.pem().as_bytes(),
        certified.key_pair.serialize_pem().as_bytes(),
    )
    .expect("identity");
    let acceptor = tokio_native_tls::TlsAcceptor::from(
        tokio_native_tls::native_tls::TlsAcceptor::new(identity).expect("acceptor"),
    );
    let listener = TcpListener::bind(("127.0.0.1", port)).await.expect("bind");
    loop {
        let Ok((mut stream, peer)) = listener.accept().await else {
            continue;
        };
        let acceptor = acceptor.clone();
        tokio::spawn(async move {
            let mut header = [0u8; 4];
            if stream.read_exact(&mut header).await.is_err() {
                return;
            }
            let length = usize::from(u16::from_be_bytes([header[2], header[3]]));
            let mut rest = vec![0u8; length.saturating_sub(4)];
            if stream.read_exact(&mut rest).await.is_err() {
                return;
            }
            let reply = X224(ConnectionConfirm::Response {
                flags: ResponseFlags::empty(),
                protocol: SecurityProtocol::HYBRID,
            });
            let Ok(answer) = ironrdp_pdu::encode_vec(&reply) else {
                return;
            };
            if stream.write_all(&answer).await.is_err() {
                return;
            }
            let Ok(mut secure) = acceptor.accept(stream).await else {
                return;
            };
            let mut buf = vec![0u8; 4096];
            match tokio::time::timeout(Duration::from_secs(120), secure.read(&mut buf)).await {
                Ok(Ok(0)) | Err(_) => println!("{peer}: nothing sent after TLS"),
                Ok(Ok(n)) => println!("{peer}: {n} bytes after TLS (CredSSP started)"),
                Ok(Err(e)) => println!("{peer}: closed ({e})"),
            }
        });
    }
}
