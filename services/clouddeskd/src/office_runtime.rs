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
