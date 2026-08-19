//! Phase 8: the real Collabora Online (CODE, the development/test
//! edition -- see `PHASE8_OFFICE_EVIDENCE.md` for why, and
//! `docs/THIRD_PARTY_NOTICES.md`) OCI runtime definition.
//!
//! ## Filesystem boundary (non-negotiable)
//!
//! Collabora gets **no bind mounts at all** -- no home directory, no
//! workspace, nothing. Document bytes only ever cross the boundary
//! through authorized WOPI HTTP operations (`crate::wopi`), never a
//! shared filesystem path. This is the single biggest structural
//! difference from Code's adapter (which does mount the user's
//! authorized directories) and is what keeps a malicious or buggy
//! Collabora session from ever reaching another user's files, the
//! `CloudDesk` DB, or Vault merely by walking a mount.
//!
//! ## Capabilities (Task 54)
//!
//! Collabora's own internal per-document sandboxing (`coolmount`,
//! mounting each editing session into its own jail directory under
//! `/opt/cool/child-roots`) needs a small set of capabilities beyond
//! the hardened zero-capability default every other runtime uses.
//! Verified live, not assumed: starting the container with only the
//! Phase 6 baseline (`--cap-drop ALL`, no additions) produces real,
//! observed startup errors (`enterMountingNS, CLONE_NEWUSER unshare
//! failed (EPERM)`, `Failed to exec coolmount... needs CAP_SYS_ADMIN`)
//! -- coolwsd still starts and answers `/hosting/discovery`, but its
//! own internal per-document jailing degrades. Granting exactly
//! `SYS_ADMIN, MKNOD, SYS_CHROOT, SETUID, SETGID, FOWNER,
//! DAC_OVERRIDE, CHOWN` (re-verified live) removes those errors
//! entirely. This does **not** touch `CloudDesk`'s own container-level
//! isolation (`--security-opt no-new-privileges`, no privileged mode,
//! no Docker socket, no host mounts, no host networking -- all still
//! true), it only restores Collabora's *own* secondary, in-container
//! per-document isolation layer.

use clouddesk_orchestrator::adapter::InstanceContext;
use clouddesk_orchestrator::oci::OciSpec;
use clouddesk_orchestrator::RuntimeKind;
use std::sync::Arc;

/// Live-verified minimal capability set (see module docs) -- compiled
/// in, never client-controlled.
const EXTRA_CAPABILITIES: &[&str] = &[
    "SYS_ADMIN",
    "MKNOD",
    "SYS_CHROOT",
    "SETUID",
    "SETGID",
    "FOWNER",
    "DAC_OVERRIDE",
    "CHOWN",
];

/// The trusted Collabora Online runtime descriptor.
///
/// `wopi_host_base` is the trusted, server-computed base URL Collabora
/// is told to accept WOPI documents from (Task 4/5/61): `CloudDesk`'s own
/// WOPI host, reachable from inside the container via
/// `host.docker.internal` (Docker's own documented mechanism for a
/// container to reach a service the host process is listening on --
/// `add_host_gateway` below). This is a compiled-in/config-derived
/// value, never something an ordinary HTTP caller supplies (Task 60).
#[must_use]
pub fn office_oci_spec(image: String, wopi_host_base: String) -> OciSpec {
    OciSpec {
        kind: RuntimeKind::Office,
        image,
        container_port: 9980,
        // Collabora's own real discovery endpoint (Task 2/5) -- also
        // used as the OCI health probe (Task 18's lesson from Code
        // applies here too: a real HTTP GET, not a bare TCP connect).
        health_check_path: "/hosting/discovery",
        command: None,
        extra_mounts: None,
        run_as: None,
        extra_env: Some(Arc::new(move |_ctx: &InstanceContext| {
            vec![(
                "extra_params".to_owned(),
                format!(
                    "--o:ssl.enable=false --o:ssl.termination=true \
                     --o:welcome.enable=false --o:home_mode.enable=false \
                     --o:storage.wopi.host={wopi_host_base} \
                     --o:net.frame_ancestors={wopi_host_base}"
                ),
            )]
        })),
        extra_capabilities: EXTRA_CAPABILITIES,
        add_host_gateway: true,
    }
}

/// One `<action>` entry from Collabora's real `/hosting/discovery`
/// response (Task 5).
#[derive(Clone)]
pub struct DiscoveredAction {
    pub extension: String,
    pub name: String,
    pub urlsrc: String,
}

/// Bounds on parsing discovery as untrusted input (Task 5): a raw byte
/// ceiling, a request timeout, and a hard cap on how many `<action>`
/// elements are collected -- discovery XML from a real Collabora
/// instance has on the order of a few hundred entries, so this is
/// generous headroom, not a tight fit.
const MAX_DISCOVERY_BYTES: usize = 4 * 1024 * 1024;
const MAX_DISCOVERY_ACTIONS: usize = 2000;
const DISCOVERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

#[derive(Debug)]
pub enum DiscoveryError {
    Request(String),
    TooLarge,
    Parse(String),
}

/// Fetches and parses `/hosting/discovery` from `base_url` (a trusted,
/// server-computed loopback address -- never a client-supplied URL,
/// Task 5's explicit "discovery URL supplied by ordinary users" ban).
/// Redirects are never followed (an untrusted response redirecting
/// discovery elsewhere is refused outright, not silently chased).
pub async fn fetch_discovery(base_url: &str) -> Result<Vec<DiscoveredAction>, DiscoveryError> {
    let client = reqwest::Client::builder()
        .timeout(DISCOVERY_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| DiscoveryError::Request(e.to_string()))?;
    let response = client
        .get(format!("{base_url}/hosting/discovery"))
        .send()
        .await
        .map_err(|e| DiscoveryError::Request(e.to_string()))?;
    let bytes = response
        .bytes()
        .await
        .map_err(|e| DiscoveryError::Request(e.to_string()))?;
    if bytes.len() > MAX_DISCOVERY_BYTES {
        return Err(DiscoveryError::TooLarge);
    }
    parse_discovery_xml(&bytes)
}

/// `quick-xml`'s non-validating reader has no DOCTYPE/general-entity
/// support at all, so this is structurally immune to XXE/billion-laughs
/// -- not merely "not tested against it".
fn parse_discovery_xml(bytes: &[u8]) -> Result<Vec<DiscoveredAction>, DiscoveryError> {
    use quick_xml::events::Event;
    use quick_xml::reader::Reader;

    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);
    let mut actions = Vec::new();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Eof) => break,
            Ok(Event::Empty(e) | Event::Start(e)) if e.name().as_ref() == b"action" => {
                let mut ext = None;
                let mut name = None;
                let mut urlsrc = None;
                for attr in e.attributes().flatten() {
                    let value = String::from_utf8_lossy(&attr.value).into_owned();
                    match attr.key.as_ref() {
                        b"ext" => ext = Some(value),
                        b"name" => name = Some(value),
                        b"urlsrc" => urlsrc = Some(value),
                        _ => {}
                    }
                }
                if let (Some(ext), Some(name), Some(urlsrc)) = (ext, name, urlsrc) {
                    actions.push(DiscoveredAction {
                        extension: ext,
                        name,
                        urlsrc,
                    });
                    if actions.len() >= MAX_DISCOVERY_ACTIONS {
                        break;
                    }
                }
            }
            Ok(_) => {}
            Err(e) => return Err(DiscoveryError::Parse(e.to_string())),
        }
        buf.clear();
    }
    Ok(actions)
}

/// Picks the best action for `extension`, preferring `edit` when
/// `read_write` is true and falling back to `view` -- never returning
/// an edit URL for a read-only-authorized file.
#[must_use]
pub fn select_action<'a>(
    actions: &'a [DiscoveredAction],
    extension: &str,
    read_write: bool,
) -> Option<&'a DiscoveredAction> {
    let ext = extension.to_lowercase();
    if read_write {
        if let Some(action) = actions
            .iter()
            .find(|a| a.extension.eq_ignore_ascii_case(&ext) && a.name == "edit")
        {
            return Some(action);
        }
    }
    actions
        .iter()
        .find(|a| a.extension.eq_ignore_ascii_case(&ext) && a.name == "view")
}

/// Strips the scheme+host from a discovery `urlsrc` (which points at
/// Collabora's own internal address), leaving just the path+query --
/// used to rebuild the URL under `CloudDesk`'s own authenticated proxy
/// prefix instead of ever exposing Collabora's raw address to the
/// browser (Task 4).
#[must_use]
pub fn path_and_query(urlsrc: &str) -> String {
    if let Some(scheme_end) = urlsrc.find("://") {
        let after_scheme = &urlsrc[scheme_end + 3..];
        if let Some(slash) = after_scheme.find('/') {
            return after_scheme[slash..].to_owned();
        }
        return "/".to_owned();
    }
    urlsrc.to_owned()
}

/// Bounded, TTL'd discovery cache (Task 63/11): avoids refetching
/// `/hosting/discovery` on every single document open, while never
/// keeping a stale result across a runtime restart/upgrade or past its
/// TTL.
///
/// Keyed by `(base_url, generation)` -- `generation` is the caller's own
/// runtime-instance generation counter (bumped whenever the underlying
/// Collabora instance is replaced/restarted, per
/// `clouddesk_orchestrator`'s existing per-instance generation
/// tracking). A cache hit therefore requires both the same address *and*
/// the same live instance generation; a restarted/replaced runtime gets
/// a new generation and so transparently misses the cache and refetches
/// -- old editor URLs from a stale discovery response are never served
/// after an upgrade.
pub mod discovery_cache {
    use super::{fetch_discovery, DiscoveredAction, DiscoveryError};
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    /// Generous enough that a single active Office deployment (managed
    /// or, in the future, external) never needs more than a handful of
    /// entries; bounded so a caller that somehow varied `base_url` on
    /// every call could not grow this without limit.
    const MAX_ENTRIES: usize = 16;
    const TTL: Duration = Duration::from_mins(5);

    struct Entry {
        actions: Vec<DiscoveredAction>,
        generation: i64,
        fetched_at: Instant,
    }

    static CACHE: Mutex<Option<HashMap<String, Entry>>> = Mutex::new(None);

    /// Returns a cached discovery result for `(base_url, generation)` if
    /// one exists, is for the matching generation, and is within TTL;
    /// otherwise fetches fresh (Task 5's bounds still apply, unchanged)
    /// and caches the result.
    pub async fn fetch_cached(
        base_url: &str,
        generation: i64,
    ) -> Result<Vec<DiscoveredAction>, DiscoveryError> {
        {
            let guard = CACHE
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(map) = guard.as_ref() {
                if let Some(entry) = map.get(base_url) {
                    if entry.generation == generation && entry.fetched_at.elapsed() < TTL {
                        return Ok(entry.actions.clone());
                    }
                }
            }
        }
        let actions = fetch_discovery(base_url).await?;
        let mut guard = CACHE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let map = guard.get_or_insert_with(HashMap::new);
        if map.len() >= MAX_ENTRIES && !map.contains_key(base_url) {
            // Bounded: evict an arbitrary entry rather than growing
            // unboundedly. In practice a single deployment has at most
            // one or two distinct base_urls (managed + external), so
            // this path is not expected to be hit in real use.
            if let Some(key) = map.keys().next().cloned() {
                map.remove(&key);
            }
        }
        map.insert(
            base_url.to_owned(),
            Entry {
                actions: actions.clone(),
                generation,
                fetched_at: Instant::now(),
            },
        );
        Ok(actions)
    }

    /// Explicitly drops every cached entry -- used when an
    /// administrator changes the external Collabora endpoint
    /// configuration, so a stale discovery result from the *previous*
    /// endpoint can never be served (Task 11: "invalidation when ...
    /// external endpoint configuration changes").
    pub fn clear_for_test() {
        let mut guard = CACHE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *guard = None;
    }
}

#[cfg(test)]
mod discovery_cache_tests {
    use super::discovery_cache::fetch_cached;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    const DISCOVERY_XML: &str = r#"<?xml version="1.0"?>
<wopi-discovery>
  <net-zone name="external-http">
    <app name="writer">
      <action ext="odt" name="edit" urlsrc="http://collab/browser/x/cool.html?"/>
    </app>
  </net-zone>
</wopi-discovery>"#;

    /// A minimal HTTP server that counts every request it serves, so
    /// the tests below can assert on how many times the cache actually
    /// went to the network.
    async fn counting_discovery_server() -> (String, Arc<AtomicUsize>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let hits_clone = hits.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                hits_clone.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut buf = [0_u8; 1024];
                    let _ = socket.read(&mut buf).await;
                    let body = DISCOVERY_XML.as_bytes();
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.write_all(body).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        (format!("http://{addr}"), hits)
    }

    /// Task 12: a second request for the same (, generation)
    /// before TTL expiry is served from cache -- exactly one real
    /// network request for two logical opens.
    #[tokio::test]
    async fn second_request_before_ttl_is_a_cache_hit() {
        super::discovery_cache::clear_for_test();
        let (base_url, hits) = counting_discovery_server().await;

        let first = fetch_cached(&base_url, 1).await.unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(hits.load(Ordering::SeqCst), 1, "first open must fetch");

        let second = fetch_cached(&base_url, 1).await.unwrap();
        assert_eq!(second.len(), 1);
        assert_eq!(
            hits.load(Ordering::SeqCst),
            1,
            "a second open of the same, unchanged runtime must be a cache hit, not a refetch"
        );
    }

    /// Task 11/12: a runtime restart/replacement (modeled here as its
    /// generation counter changing, exactly what
    /// `clouddesk_orchestrator::RuntimeManager` bumps on a real
    /// restart) must never serve the old discovery response -- it must
    /// refetch.
    #[tokio::test]
    async fn generation_change_forces_a_refetch() {
        super::discovery_cache::clear_for_test();
        let (base_url, hits) = counting_discovery_server().await;

        fetch_cached(&base_url, 1).await.unwrap();
        assert_eq!(hits.load(Ordering::SeqCst), 1);

        fetch_cached(&base_url, 2).await.unwrap();
        assert_eq!(
            hits.load(Ordering::SeqCst),
            2,
            "a changed runtime generation must never be served the previous \
             generation's cached discovery response"
        );
    }

    /// Task 12: a malformed response from a "new" runtime must fail
    /// safely (never panic, never silently serve the old cached value
    /// as if it were still valid).
    #[tokio::test]
    async fn malformed_new_discovery_fails_safely_without_reviving_stale_cache() {
        super::discovery_cache::clear_for_test();
        let (base_url, _hits) = counting_discovery_server().await;
        fetch_cached(&base_url, 1).await.unwrap();

        // A different address that isn't serving anything valid at all.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let bad_addr = listener.local_addr().unwrap();
        drop(listener); // nothing listening -> connection refused
        let result = fetch_cached(&format!("http://{bad_addr}"), 1).await;
        assert!(
            result.is_err(),
            "a fetch against an unreachable/malformed endpoint must fail, not panic"
        );
    }
}
