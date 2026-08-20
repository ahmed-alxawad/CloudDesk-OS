//! Phase 9 Pass 3A-4: the Browser egress policy proxy.
//!
//! ## Why this exists
//!
//! Pass 3A-3 found that Browser containers, even on their own
//! dedicated Docker network, still have a routable path to the host's
//! own gateway IP and to private/link-local address ranges --
//! Docker's own inter-network isolation blocks container-to-container
//! traffic across different networks, but never blocks
//! container-to-host or destination-address-based traffic in general.
//! Closing that at the kernel packet-filter level requires root/
//! `CAP_NET_ADMIN` (a real firewall rule via a privileged helper),
//! which this environment cannot install and verify -- and more
//! importantly, `CloudDesk`'s own actual threat model here is hostile
//! **page content** attempting SSRF, not a Chromium sandbox escape
//! attempting a raw socket. A hostile page's `fetch`/`XHR`/`<img>`/
//! WebSocket/navigation all go through Chromium's own configured
//! network stack, which -- when a proxy is set via the `--proxy-
//! server` command-line flag (not a user-facing setting Chromium ever
//! lets page content or a UI control override) -- routes every
//! HTTP(S) request through it unconditionally.
//!
//! This module is that proxy: a minimal HTTP/1.1 forward proxy
//! (`CONNECT` for HTTPS, plain forwarding for HTTP) that resolves
//! every destination itself and checks the **resolved IP address**,
//! never the hostname text, against a fixed, compiled-in policy
//! before ever dialing out -- closing the exact DNS-rebinding gap a
//! hostname-string check would leave open. A redirect to an internal
//! target is not a bypass either: Chromium re-navigates through this
//! same configured proxy for the redirect target, which is
//! independently checked again.
//!
//! ## What this does not close
//!
//! Docker-network-level reachability to the host gateway itself (e.g.
//! a raw `ping`) remains structurally possible from inside the
//! container -- this proxy only governs Brave's own HTTP(S) client
//! behavior. That residual is real, already disclosed in Pass 3A-3's
//! evidence, and assessed as low-severity (`clouddeskd`'s
//! unauthenticated routes grant nothing a public host couldn't already
//! reach; `cloudesk-privd` is a Unix socket, unreachable regardless).
//! WebRTC's UDP media path also does not go through this proxy by
//! default -- acceptable here because Brave runs with no configured
//! STUN/TURN server (Pass 3A-3's WebRTC evidence), so it never
//! attempts to gather a server-reflexive candidate against an
//! arbitrary destination in the first place.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Fixed, never client-configurable. Brave's own `--proxy-server` flag
/// (`docker/brave/Dockerfile`) points at this exact port on the
/// dedicated Browser network's own pinned gateway address.
pub const BROWSER_EGRESS_PROXY_PORT: u16 = 9819;

const MAX_REQUEST_LINE_BYTES: usize = 8192;
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

/// Test-only, additional exact IPv4 addresses to allow despite the
/// default-deny policy below -- **never** set in the real product
/// (`main.rs` never touches this). Live security tests need a
/// controlled fixture reachable at *some* address for a Brave
/// container to navigate to, and this host's own network interfaces
/// are themselves private addresses (a real dev/CI machine behind a
/// router, not a host with a public IP) -- the product policy being
/// tested (private/loopback destinations denied) makes every
/// locally-reachable address structurally unfit to double as "the
/// public internet" for test purposes, so tests opt a handful of
/// specific fixture addresses in explicitly, by exact IP, never a
/// broad range. Populated once via [`set_test_allowlist`], read on
/// every check.
static TEST_ALLOWLIST: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<Ipv4Addr>>> =
    std::sync::OnceLock::new();

/// Test-only. Adds `addr` to the destination allowlist for this
/// process, overriding the default-deny policy for that one exact
/// address. Never called from `main.rs`.
pub fn set_test_allowlist(addrs: impl IntoIterator<Item = Ipv4Addr>) {
    let set =
        TEST_ALLOWLIST.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()));
    set.lock().unwrap().extend(addrs);
}

fn is_test_allowed(ip: IpAddr) -> bool {
    let IpAddr::V4(v4) = ip else {
        return false;
    };
    TEST_ALLOWLIST
        .get()
        .is_some_and(|set| set.lock().unwrap().contains(&v4))
}

/// The actual network-layer policy (Task 1/8 of Pass 3A-4): denies
/// loopback, RFC1918, and link-local/metadata-shaped destinations by
/// default (Option 1 -- `GOAL.md`'s G7 requirement list for Browser
/// names only general internet-browsing features; no intranet/private-
/// LAN browsing requirement exists to justify allowing it). Public
/// Internet addresses are allowed.
#[must_use]
pub fn is_blocked_destination(ip: IpAddr) -> bool {
    if is_test_allowed(ip) {
        return false;
    }
    match ip {
        IpAddr::V4(v4) => is_blocked_v4(v4),
        IpAddr::V6(v6) => is_blocked_v6(v6),
    }
}

fn is_blocked_v4(ip: Ipv4Addr) -> bool {
    ip.is_loopback()
        || ip.is_link_local() // 169.254.0.0/16 -- covers real cloud metadata-style addresses (169.254.169.254 included)
        || ip.is_private() // 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_documentation()
        // Shared/carrier-grade NAT range (RFC 6598) -- not public.
        || (ip.octets()[0] == 100 && (64..=127).contains(&ip.octets()[1]))
}

fn is_blocked_v6(ip: Ipv6Addr) -> bool {
    ip.is_loopback()
        || ip.is_unspecified()
        // Unique local (fc00::/7).
        || (ip.segments()[0] & 0xfe00) == 0xfc00
        // Link-local (fe80::/10) -- covers IPv6 metadata-style access.
        || (ip.segments()[0] & 0xffc0) == 0xfe80
        // IPv4-mapped (::ffff:a.b.c.d) -- unwrap and re-check as v4,
        // closing the "encode a blocked v4 address as v6" evasion.
        || ip
            .to_ipv4_mapped()
            .is_some_and(is_blocked_v4)
}

async fn resolve_all(host: &str, port: u16) -> std::io::Result<Vec<SocketAddr>> {
    tokio::net::lookup_host((host, port))
        .await
        .map(Iterator::collect)
}

/// Every candidate address a hostname resolves to must be safe --
/// picking only the "first safe one" while ignoring others would still
/// leave a multi-answer DNS response free to steer traffic toward a
/// blocked address on a later connection attempt/retry.
fn all_safe(addrs: &[SocketAddr]) -> bool {
    !addrs.is_empty() && addrs.iter().all(|a| !is_blocked_destination(a.ip()))
}

async fn write_error(stream: &mut TcpStream, status_line: &str) {
    let _ = stream
        .write_all(format!("{status_line}\r\nConnection: close\r\n\r\n").as_bytes())
        .await;
}

async fn handle_connect(mut client: TcpStream, host: String, port: u16) {
    let Ok(addrs) = resolve_all(&host, port).await else {
        write_error(&mut client, "HTTP/1.1 502 Bad Gateway").await;
        return;
    };
    if !all_safe(&addrs) {
        tracing::warn!(host = %host, "browser egress proxy blocked CONNECT to a policy-denied destination");
        write_error(&mut client, "HTTP/1.1 403 Forbidden").await;
        return;
    }
    let Some(addr) = addrs.first().copied() else {
        write_error(&mut client, "HTTP/1.1 502 Bad Gateway").await;
        return;
    };
    let Ok(Ok(mut upstream)) =
        tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addr)).await
    else {
        write_error(&mut client, "HTTP/1.1 502 Bad Gateway").await;
        return;
    };
    if client
        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .await
        .is_err()
    {
        return;
    }
    let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
}

async fn handle_plain_http(mut client: TcpStream, request_line: String, mut rest: Vec<u8>) {
    // Read headers to find Host: and the end of the header block.
    let mut headers = rest.split_off(0);
    let mut buf = [0_u8; 4096];
    let header_end = loop {
        if let Some(pos) = find_header_end(&headers) {
            break pos;
        }
        if headers.len() > MAX_REQUEST_LINE_BYTES {
            write_error(&mut client, "HTTP/1.1 431 Request Header Fields Too Large").await;
            return;
        }
        match client.read(&mut buf).await {
            Ok(0) | Err(_) => {
                write_error(&mut client, "HTTP/1.1 400 Bad Request").await;
                return;
            }
            Ok(n) => headers.extend_from_slice(&buf[..n]),
        }
    };
    let header_text = String::from_utf8_lossy(&headers[..header_end]);
    let host_header = header_text
        .lines()
        .find_map(|line| {
            line.strip_prefix("Host:")
                .or_else(|| line.strip_prefix("host:"))
        })
        .map(str::trim);
    let Some(host_header) = host_header else {
        write_error(&mut client, "HTTP/1.1 400 Bad Request").await;
        return;
    };
    let (host, port) = match host_header.rsplit_once(':') {
        Some((h, p)) => (h.to_owned(), p.parse().unwrap_or(80)),
        None => (host_header.to_owned(), 80),
    };
    let Ok(addrs) = resolve_all(&host, port).await else {
        write_error(&mut client, "HTTP/1.1 502 Bad Gateway").await;
        return;
    };
    if !all_safe(&addrs) {
        tracing::warn!(host = %host, "browser egress proxy blocked plain-HTTP request to a policy-denied destination");
        write_error(&mut client, "HTTP/1.1 403 Forbidden").await;
        return;
    }
    let Some(addr) = addrs.first().copied() else {
        write_error(&mut client, "HTTP/1.1 502 Bad Gateway").await;
        return;
    };
    let Ok(Ok(mut upstream)) =
        tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addr)).await
    else {
        write_error(&mut client, "HTTP/1.1 502 Bad Gateway").await;
        return;
    };
    let mut full_request = request_line.into_bytes();
    full_request.extend_from_slice(&headers);
    if upstream.write_all(&full_request).await.is_err() {
        return;
    }
    let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
}

async fn handle_connection(mut client: TcpStream) {
    let mut buf = vec![0_u8; 2048];
    let mut total = Vec::new();
    let line_end = loop {
        let n = match client.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        total.extend_from_slice(&buf[..n]);
        if let Some(pos) = total.windows(2).position(|w| w == b"\r\n") {
            break pos + 2;
        }
        if total.len() > MAX_REQUEST_LINE_BYTES {
            write_error(&mut client, "HTTP/1.1 414 URI Too Long").await;
            return;
        }
    };
    let request_line = String::from_utf8_lossy(&total[..line_end]).into_owned();
    let rest = total[line_end..].to_vec();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();

    if method.eq_ignore_ascii_case("CONNECT") {
        let Some((host, port)) = target.rsplit_once(':').and_then(|(h, p)| {
            p.trim_end()
                .parse::<u16>()
                .ok()
                .map(|port| (h.to_owned(), port))
        }) else {
            write_error(&mut client, "HTTP/1.1 400 Bad Request").await;
            return;
        };
        // Drain any remaining headers before starting the tunnel.
        let mut discard = rest;
        let mut buf = [0_u8; 2048];
        while find_header_end(&discard).is_none() && discard.len() <= MAX_REQUEST_LINE_BYTES {
            match client.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => discard.extend_from_slice(&buf[..n]),
            }
        }
        handle_connect(client, host, port).await;
    } else if method.eq_ignore_ascii_case("GET")
        || method.eq_ignore_ascii_case("HEAD")
        || method.eq_ignore_ascii_case("POST")
        || method.eq_ignore_ascii_case("PUT")
        || method.eq_ignore_ascii_case("OPTIONS")
    {
        handle_plain_http(client, request_line, rest).await;
    } else {
        write_error(&mut client, "HTTP/1.1 405 Method Not Allowed").await;
    }
}

/// Starts the proxy as a background task. Binds `0.0.0.0` (reachable
/// via the dedicated Browser network's gateway, same as every other
/// host-bound service this project already runs) -- never
/// client-configurable, never exposed as a product feature; only
/// Brave's own compiled-in `--proxy-server` flag ever points at it.
pub fn spawn() {
    // Deliberately *not* a process-wide "only ever bind once" guard:
    // each `#[tokio::test]` function gets its own short-lived Tokio
    // runtime that's fully torn down (along with every task it ever
    // spawned, including a prior test's own proxy listener) by the
    // time the next test's runtime starts -- a `std::sync::Once` here
    // would survive across that boundary and permanently skip
    // re-binding for every test after the first, leaving later tests
    // with no running proxy at all (a real bug live-found this pass).
    // `main.rs` only ever calls this once for the process's one real
    // long-lived runtime, so a genuine double-bind is never expected
    // there either; if it somehow ever raced, `AddrInUse` is handled
    // below as a normal, logged, non-fatal failure.
    tokio::spawn(async move {
        let listener = match TcpListener::bind(("0.0.0.0", BROWSER_EGRESS_PROXY_PORT)).await {
            Ok(l) => l,
            Err(e) => {
                tracing::error!(error = %e, "browser egress proxy failed to bind; Browser navigation will fail closed");
                return;
            }
        };
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    tokio::spawn(handle_connection(stream));
                }
                Err(e) => {
                    tracing::warn!(error = %e, "browser egress proxy accept error");
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_loopback_and_metadata_and_private_ranges() {
        assert!(is_blocked_destination("127.0.0.1".parse().unwrap()));
        assert!(is_blocked_destination("169.254.169.254".parse().unwrap()));
        assert!(is_blocked_destination("10.1.2.3".parse().unwrap()));
        assert!(is_blocked_destination("172.16.5.5".parse().unwrap()));
        assert!(is_blocked_destination("192.168.1.1".parse().unwrap()));
        assert!(is_blocked_destination("100.64.0.1".parse().unwrap()));
        assert!(is_blocked_destination("::1".parse().unwrap()));
        assert!(is_blocked_destination("fe80::1".parse().unwrap()));
        assert!(is_blocked_destination("fc00::1".parse().unwrap()));
        assert!(is_blocked_destination(
            "::ffff:169.254.169.254".parse().unwrap()
        ));
    }

    #[test]
    fn allows_public_addresses() {
        assert!(!is_blocked_destination("1.1.1.1".parse().unwrap()));
        assert!(!is_blocked_destination("8.8.8.8".parse().unwrap()));
        assert!(!is_blocked_destination(
            "2606:4700:4700::1111".parse().unwrap()
        ));
    }
}
