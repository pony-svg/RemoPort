//! Reaching the PC: race its addresses, keep the link honest.

use std::collections::VecDeque;
use std::io;
use std::net::SocketAddr;
use std::time::Duration;

use futures::stream::{FuturesUnordered, StreamExt};
use tokio::net::TcpStream;

/// Head start an attempt gets before the next address is tried (RFC 8305 §5).
const ATTEMPT_DELAY: Duration = Duration::from_millis(250);

/// Connects to `host` on `port` through the first of its addresses that
/// answers. A name whose IPv6 address never answers (a VPN that hands out an
/// unrouted AAAA record) does not use up the whole timeout before IPv4.
pub(crate) async fn connect_tcp(host: &str, port: u16) -> io::Result<TcpStream> {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host, port)).await?.collect();
    let mut queue = interleave_families(addrs);
    let mut attempts = FuturesUnordered::new();
    let mut last_error = None;
    attempts.extend(queue.pop_front().map(TcpStream::connect));
    while !attempts.is_empty() {
        tokio::select! {
            Some(result) = attempts.next() => match result {
                Ok(stream) => return Ok(stream),
                Err(e) => {
                    last_error = Some(e);
                    attempts.extend(queue.pop_front().map(TcpStream::connect));
                }
            },
            () = tokio::time::sleep(ATTEMPT_DELAY), if !queue.is_empty() => {
                attempts.extend(queue.pop_front().map(TcpStream::connect));
            }
        }
    }
    Err(last_error.unwrap_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "the name resolved to no address")
    }))
}

/// Alternates address families, keeping the resolver's preferred one first.
fn interleave_families(addrs: Vec<SocketAddr>) -> VecDeque<SocketAddr> {
    let Some(prefers_v6) = addrs.first().map(SocketAddr::is_ipv6) else {
        return VecDeque::new();
    };
    let (preferred, other): (Vec<_>, Vec<_>) = addrs
        .into_iter()
        .partition(|addr| addr.is_ipv6() == prefers_v6);
    let mut interleaved = VecDeque::with_capacity(preferred.len() + other.len());
    let mut preferred = preferred.into_iter();
    let mut other = other.into_iter();
    loop {
        match (preferred.next(), other.next()) {
            (None, None) => return interleaved,
            (first, second) => interleaved.extend(first.into_iter().chain(second)),
        }
    }
}

/// Makes a dead link show up within about 30 s: TCP keepalive (first probe
/// after 15 s of silence, then three 5 s apart) and, on Linux, a 30 s limit on
/// unacknowledged data. Without them a suspended laptop leaves the session
/// frozen with no error. Also turns Nagle off, for typing latency.
pub(crate) fn tune(stream: &TcpStream) {
    let _ = stream.set_nodelay(true);
    let socket = socket2::SockRef::from(stream);
    let keepalive = socket2::TcpKeepalive::new()
        .with_time(Duration::from_secs(15))
        .with_interval(Duration::from_secs(5))
        .with_retries(3);
    let _ = socket.set_tcp_keepalive(&keepalive);
    #[cfg(target_os = "linux")]
    let _ = socket.set_tcp_user_timeout(Some(Duration::from_secs(30)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn families_alternate_from_the_preferred_one() {
        let parse = |s: &str| s.parse::<SocketAddr>().expect("address");
        let (v6a, v6b, v4) = (
            parse("[2001:db8::1]:3389"),
            parse("[2001:db8::2]:3389"),
            parse("192.0.2.1:3389"),
        );
        assert_eq!(
            interleave_families(vec![v6a, v6b, v4]),
            VecDeque::from([v6a, v4, v6b])
        );
    }

    #[tokio::test]
    async fn a_refused_address_hands_over_to_the_next() {
        let Ok(listener) = tokio::net::TcpListener::bind("127.0.0.1:0").await else {
            return;
        };
        let Ok(port) = listener.local_addr().map(|a| a.port()) else {
            return;
        };
        let accept = tokio::spawn(async move {
            let _ = listener.accept().await;
        });
        // `localhost` usually resolves to ::1 first, where nothing listens.
        assert!(connect_tcp("localhost", port).await.is_ok());
        accept.abort();
    }
}
