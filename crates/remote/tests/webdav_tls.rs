//! Regression test for the `WebDAV` TLS certificate-verification bypass
//! found and fixed in Phase 16A: `WebDavProvider::new` used to build its
//! `reqwest::Client` with `.danger_accept_invalid_certs(true)`
//! unconditionally, silently defeating server authentication for every
//! `WebDAV` remote connection (a MITM could impersonate any configured
//! `WebDAV` server). This proves the client now rejects a self-signed
//! certificate it has no reason to trust, the same way any normal HTTPS
//! client would -- and, crucially, that the test would have caught the
//! bug: the mock server answers every accepted connection with a valid
//! `WebDAV` response, so a client that skipped certificate validation
//! would observe a clean `Ok(..)`, not merely *some* unrelated error.
use std::net::SocketAddr;
use std::process::Command;
use std::sync::Arc;

use base64::Engine;
use clouddesk_vfs::VfsProvider;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio_rustls::rustls::ServerConfig;
use tokio_rustls::TlsAcceptor;

fn pem_body_to_der(pem: &str, label: &str) -> Vec<u8> {
    let begin = format!("-----BEGIN {label}-----");
    let end = format!("-----END {label}-----");
    let body: String = pem
        .lines()
        .skip_while(|l| l.trim() != begin)
        .skip(1)
        .take_while(|l| l.trim() != end)
        .collect();
    base64::engine::general_purpose::STANDARD
        .decode(body)
        .unwrap()
}

fn generate_self_signed(dir: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let key = dir.join("key.pem");
    let cert = dir.join("cert.pem");
    let status = Command::new("openssl")
        .args([
            "req", "-x509", "-newkey", "rsa:2048", "-sha256", "-nodes", "-days", "1", "-keyout",
        ])
        .arg(&key)
        .arg("-out")
        .arg(&cert)
        .args(["-subj", "/CN=untrusted-webdav-test.invalid"])
        .status()
        .expect("openssl must be available to generate a throwaway test certificate");
    assert!(
        status.success(),
        "openssl self-signed cert generation failed"
    );
    (cert, key)
}

/// Answers exactly one TLS connection with a minimal, valid `WebDAV`
/// PROPFIND (207 Multi-Status) response -- if the client's TLS
/// handshake succeeds (i.e. it accepted our untrusted certificate),
/// it will see this as an ordinary successful `stat()`.
async fn serve_one_tls_connection(listener: TcpListener, acceptor: TlsAcceptor) {
    let Ok((stream, _)) = listener.accept().await else {
        return;
    };
    let Ok(mut tls) = acceptor.accept(stream).await else {
        // Handshake rejected client-side before any bytes were even
        // sent -- exactly the outcome we expect from the fixed client.
        return;
    };
    let mut buf = [0u8; 4096];
    let _ = tls.read(&mut buf).await;
    let body = r#"<?xml version="1.0" encoding="utf-8" ?><D:multistatus xmlns:D="DAV:"><D:response><D:href>/mock-item</D:href><D:propstat><D:prop><D:resourcetype><D:collection/></D:resourcetype><D:getcontentlength>0</D:getcontentlength></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response></D:multistatus>"#;
    let response = format!(
        "HTTP/1.1 207 Multi-Status\r\nContent-Type: application/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = tls.write_all(response.as_bytes()).await;
    let _ = tls.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn webdav_rejects_untrusted_self_signed_certificate() {
    let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
    let tmp = tempfile::tempdir().unwrap();
    let (cert_path, key_path) = generate_self_signed(tmp.path());

    let cert_der = pem_body_to_der(&std::fs::read_to_string(&cert_path).unwrap(), "CERTIFICATE");
    let key_der = pem_body_to_der(&std::fs::read_to_string(&key_path).unwrap(), "PRIVATE KEY");
    let certs: Vec<CertificateDer<'static>> = vec![CertificateDer::from(cert_der)];
    let key: PrivateKeyDer<'static> = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_der));

    let server_config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(server_config));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(serve_one_tls_connection(listener, acceptor));

    let handle = tokio::runtime::Handle::current();
    let provider = clouddesk_remote::webdav::WebDavProvider::new(
        format!("https://{addr}/"),
        None,
        None,
        handle,
    );

    // A server presenting an untrusted self-signed certificate must
    // never be treated as a successful connection: if the client
    // skipped certificate validation, this `stat()` would return
    // `Ok(..)` because our mock server answers with a perfectly valid
    // WebDAV response as soon as the handshake completes.
    let result = tokio::task::spawn_blocking(move || provider.stat("/"))
        .await
        .unwrap();

    assert!(
        result.is_err(),
        "WebDAV client accepted an untrusted self-signed certificate and completed a real \
         request against it -- TLS verification is not enforced (got: {result:?})"
    );
}
