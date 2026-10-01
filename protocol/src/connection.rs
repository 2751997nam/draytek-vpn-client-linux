/// TLS connection and HTTP CONNECT handshake.
use anyhow::{bail, Context, Result};
use base64::Engine;
use openssl::ssl::{SslConnector, SslMethod, SslOptions, SslVerifyMode};
use std::net::SocketAddr;
use std::pin::Pin;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{lookup_host, TcpStream};
use tokio_openssl::SslStream;
use tracing::{debug, info};

use crate::constants::CLIENT_NAME;

/// Establish a TLS connection and perform the HTTP CONNECT handshake.
///
/// Returns the TLS stream ready for binary SSTP framing.
pub async fn connect(
    server: &str,
    port: u16,
    username: &str,
    password: &str,
    accept_self_signed: bool,
    prefer_ipv4: bool,
) -> Result<SslStream<TcpStream>> {
    // Step 1: TCP connection
    info!("Connecting to {server}:{port}");
    let tcp_stream = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        connect_tcp(server, port, prefer_ipv4),
    )
    .await
    .context("TCP connect timeout")??;

    tcp_stream
        .set_nodelay(true)
        .context("Failed to set TCP_NODELAY")?;

    // Step 2: TLS handshake
    info!("Starting TLS handshake");
    let mut builder =
        SslConnector::builder(SslMethod::tls()).context("Failed to create SSL connector")?;
    // DrayTek routers require legacy TLS renegotiation
    builder.set_options(SslOptions::ALLOW_UNSAFE_LEGACY_RENEGOTIATION);
    if accept_self_signed {
        builder.set_verify(SslVerifyMode::NONE);
    }

    let connector = builder.build();
    let ssl = connector
        .configure()
        .context("Failed to configure SSL")?
        .into_ssl(server)
        .context("Failed to create SSL instance")?;

    let mut tls_stream = SslStream::new(ssl, tcp_stream).context("Failed to create SslStream")?;

    Pin::new(&mut tls_stream)
        .connect()
        .await
        .context("TLS handshake failed")?;

    info!("TLS connection established");

    // Step 3: HTTP CONNECT handshake
    let credentials =
        base64::engine::general_purpose::STANDARD.encode(format!("{username}:{password}"));

    let request = format!(
        "CONNECT / HTTP/1.0\r\n\
         Host:{server}\r\n\
         Agent: {CLIENT_NAME}\r\n\
         Authorization: Basic {credentials}\r\n\
         \r\n"
    );

    debug!("Sending HTTP CONNECT");
    Pin::new(&mut tls_stream)
        .write_all(request.as_bytes())
        .await
        .context("Failed to send HTTP CONNECT")?;

    // Read HTTP response line
    let mut response_buf = Vec::with_capacity(256);
    let mut byte = [0u8; 1];
    loop {
        Pin::new(&mut tls_stream)
            .read_exact(&mut byte)
            .await
            .context("Failed to read HTTP response")?;
        response_buf.push(byte[0]);
        // Look for \r\n\r\n (end of HTTP headers)
        if response_buf.len() >= 4 && response_buf[response_buf.len() - 4..] == *b"\r\n\r\n" {
            break;
        }
        if response_buf.len() > 4096 {
            bail!("HTTP response too large");
        }
    }

    let response_str = String::from_utf8_lossy(&response_buf);
    debug!("HTTP response: {}", response_str.trim());

    // Parse status code from first line
    let first_line = response_str.lines().next().unwrap_or("");
    let status_code = parse_http_status(first_line)?;

    if status_code != 200 {
        bail!("HTTP CONNECT failed with status {status_code}: {first_line}");
    }

    info!("HTTP CONNECT successful, switching to binary mode");
    Ok(tls_stream)
}

/// Resolve `server:port` and connect to the first address that answers.
///
/// `TcpStream::connect` on a hostname already walks the resolver's list, but it
/// gives no say in the order. Resolving here lets IPv4 be tried first while
/// still falling back to every other address, so the preference never strands a
/// host that is only reachable over one family.
async fn connect_tcp(server: &str, port: u16, prefer_ipv4: bool) -> Result<TcpStream> {
    let mut addrs: Vec<SocketAddr> = lookup_host((server, port))
        .await
        .with_context(|| format!("Failed to resolve {server}:{port}"))?
        .collect();

    if addrs.is_empty() {
        bail!("{server}:{port} resolved to no addresses");
    }

    order_addresses(&mut addrs, prefer_ipv4);
    debug!("Resolved {server}:{port} to {addrs:?}");

    let mut last: Option<(SocketAddr, std::io::Error)> = None;
    for &addr in &addrs {
        match TcpStream::connect(addr).await {
            Ok(stream) => {
                info!("Connected to {addr}");
                return Ok(stream);
            }
            Err(e) => {
                debug!("Connection to {addr} failed: {e}");
                last = Some((addr, e));
            }
        }
    }

    let (addr, e) = last.expect("a non-empty address list always records a failure");
    Err(e).with_context(|| {
        format!("Could not connect to any address for {server}:{port} (last tried {addr})")
    })
}

/// Put IPv4 addresses first when preferred.
///
/// The sort is stable, so the resolver's own ordering survives within each
/// family, and leaving `prefer_ipv4` off keeps the system order untouched.
fn order_addresses(addrs: &mut [SocketAddr], prefer_ipv4: bool) {
    if prefer_ipv4 {
        addrs.sort_by_key(|addr| !addr.is_ipv4());
    }
}

/// Parse status code from an HTTP response line like "HTTP/1.0 200 OK".
fn parse_http_status(line: &str) -> Result<u16> {
    let re = regex::Regex::new(r"HTTP/[\d.]+ (\d+)").unwrap();
    let caps = re.captures(line).context("Invalid HTTP response line")?;
    let code: u16 = caps[1].parse().context("Invalid status code")?;
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addrs(specs: &[&str]) -> Vec<SocketAddr> {
        specs.iter().map(|s| s.parse().unwrap()).collect()
    }

    #[test]
    fn prefers_ipv4_while_keeping_resolver_order_within_a_family() {
        let mut list = addrs(&[
            "[2001:db8::1]:443",
            "192.0.2.1:443",
            "[2001:db8::2]:443",
            "192.0.2.2:443",
        ]);
        order_addresses(&mut list, true);
        assert_eq!(
            list,
            addrs(&[
                "192.0.2.1:443",
                "192.0.2.2:443",
                "[2001:db8::1]:443",
                "[2001:db8::2]:443"
            ])
        );
    }

    #[test]
    fn leaves_order_alone_when_not_preferring_ipv4() {
        let original = addrs(&["[2001:db8::1]:443", "192.0.2.1:443"]);
        let mut list = original.clone();
        order_addresses(&mut list, false);
        assert_eq!(list, original);
    }

    #[test]
    fn preference_is_a_no_op_for_single_family_results() {
        for specs in [
            &["192.0.2.1:443", "192.0.2.2:443"],
            &["[2001:db8::1]:443", "[2001:db8::2]:443"],
        ] {
            let original = addrs(specs);
            let mut list = original.clone();
            order_addresses(&mut list, true);
            assert_eq!(list, original, "single-family list must not be reordered");
        }
    }

    #[test]
    fn test_parse_http_status() {
        assert_eq!(parse_http_status("HTTP/1.0 200 OK").unwrap(), 200);
        assert_eq!(parse_http_status("HTTP/1.1 403 Forbidden").unwrap(), 403);
        assert_eq!(
            parse_http_status("HTTP/1.0 500 Internal Server Error").unwrap(),
            500
        );
        assert!(parse_http_status("garbage").is_err());
    }
}
